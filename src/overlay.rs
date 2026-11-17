//! Atajos de teclado globales y aviso en pantalla al silenciar o activar («Silenciado · Micrófono»).
//!
//! Vive en el motor (el proceso de la bandeja) y no en la ventana, para que funcione aunque la
//! ventana esté cerrada. Es un hilo con una ventana Win32 que:
//! - registra los atajos con `RegisterHotKey` y recibe `WM_HOTKEY`;
//! - se entera de cada cambio de configuración (`Shared::update` → `changed()`) y, si algo se ha
//!   silenciado o activado, venga de donde venga (atajo, bandeja, ventana, API, la tecla de silencio
//!   del teclado), lo anuncia;
//! - dibuja el aviso como ventana por capas: encima de todo, sin robar el foco y transparente al
//!   ratón (los clics pasan al juego que haya debajo).
//!
//! Solo hay temporizadores mientras el aviso está a la vista: en reposo no gasta nada.

use crate::config::{HotkeyAction, Hotkeys, MuteTarget, OverlayCorner, Settings};
use crate::hotkeys;
use crate::state::Shared;
use crate::ui::{icons, theme};
use eframe::egui::Color32;
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

const WM_CHANGED: u32 = WM_APP + 20;
const WM_PAUSE: u32 = WM_APP + 21;
const WM_PREVIEW: u32 = WM_APP + 22;
const TIMER_HIDE: usize = 1;
const TIMER_FADE: usize = 2;
const FADE_STEP_MS: u32 = 15;
const FADE_STEPS: u8 = 12;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

#[derive(Default)]
struct State {
    /// Lo silenciado la última vez que se miró, para saber qué ha cambiado.
    mutes: [bool; 7],
    /// Atajos registrados ahora mismo y si incluían los del modo streamer (None = ninguno).
    registered: Option<(Hotkeys, bool)>,
    /// La ventana está grabando un atajo: los registrados se sueltan para que la tecla le llegue.
    paused: bool,
    alpha: u8,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub fn spawn(shared: Arc<Shared>) {
    let _ = SHARED.set(shared);
    std::thread::Builder::new()
        .name("atajos".into())
        .spawn(|| unsafe {
            if let Err(e) = run() {
                eprintln!("[atajos] {e:#}");
            }
        })
        .unwrap();
}

fn post(msg: u32, wparam: usize) {
    let raw = HWND_RAW.load(Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as *mut _)), msg, WPARAM(wparam), LPARAM(0));
        }
    }
}

/// La configuración ha cambiado (lo llama `Shared::update`; en el proceso de la ventana no hace nada).
pub fn changed() {
    post(WM_CHANGED, 0);
}

/// Suelta (true) o vuelve a registrar (false) los atajos mientras la ventana graba uno nuevo.
pub fn set_paused(paused: bool) {
    post(WM_PAUSE, paused as usize);
}

/// Muestra el aviso del micrófono tal como está ahora (botón «Probar» de Ajustes).
pub fn preview() {
    post(WM_PREVIEW, 0);
}

unsafe fn run() -> windows::core::Result<()> {
    unsafe {
        // Con la escala de cada monitor: si no, Windows estiraría el aviso y se vería borroso.
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        load_fonts();

        let instance = GetModuleHandleW(None)?;
        let class = w!("RustSonarOverlay");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Rust Sonar (aviso)"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        HWND_RAW.store(hwnd.0 as isize, Ordering::Release);

        // Estado de partida: lo que ya está silenciado al arrancar no se anuncia.
        if let Some(shared) = SHARED.get() {
            let s = shared.settings.read().clone();
            STATE.with_borrow_mut(|st| {
                st.mutes = MuteTarget::ALL.map(|t| t.is_muted(&s));
                sync_hotkeys(hwnd, st, &s, shared);
            });
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(shared) = SHARED.get() else { return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) } };
    match msg {
        WM_HOTKEY => {
            if let Some(a) = HotkeyAction::from_id(wparam.0 as i32) {
                // Los silencios se anuncian solos (update() → changed() → WM_CHANGED); los cambios de
                // volumen, aquí.
                if let Some(t) = shared.update(|s| a.run(s)) {
                    let s = shared.settings.read().clone();
                    if s.overlay.enabled {
                        show(hwnd, t, &s, true);
                    }
                }
            }
        }
        WM_CHANGED => on_changed(hwnd, shared),
        WM_PAUSE => STATE.with_borrow_mut(|st| {
            st.paused = wparam.0 != 0;
            if st.paused {
                unregister_all(hwnd);
                st.registered = None;
            } else {
                let s = shared.settings.read().clone();
                sync_hotkeys(hwnd, st, &s, shared);
            }
        }),
        WM_PREVIEW => {
            let s = shared.settings.read().clone();
            show(hwnd, MuteTarget::Mic, &s, false);
        }
        WM_TIMER => unsafe { on_timer(hwnd, wparam.0) },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

fn on_changed(hwnd: HWND, shared: &Shared) {
    let s = shared.settings.read().clone();
    let mutes = MuteTarget::ALL.map(|t| t.is_muted(&s));
    let flipped = STATE.with_borrow_mut(|st| {
        let flipped = MuteTarget::ALL.into_iter().find(|t| st.mutes[t.index()] != mutes[t.index()]);
        st.mutes = mutes;
        sync_hotkeys(hwnd, st, &s, shared);
        flipped
    });
    if let Some(t) = flipped
        && s.overlay.enabled
    {
        show(hwnd, t, &s, false);
    }
}

fn unregister_all(hwnd: HWND) {
    for a in HotkeyAction::all() {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), a.id());
        }
    }
}

/// Registra los atajos si han cambiado. Los del modo streamer, solo con el modo activado (si no, se
/// quedarían esas combinaciones para nada). Los que Windows no deja registrar (otro programa ya usa
/// esa combinación) se apuntan en el estado para que la ventana lo avise.
fn sync_hotkeys(hwnd: HWND, st: &mut State, s: &Settings, shared: &Shared) {
    let wanted = (s.hotkeys.clone(), s.streamer.enabled);
    if st.paused || st.registered.as_ref() == Some(&wanted) {
        return;
    }
    unregister_all(hwnd);
    let mut failed = Vec::new();
    for a in HotkeyAction::all().filter(|a| s.streamer.enabled || !a.streamer_only()) {
        let Some(h) = s.hotkeys.get(a) else { continue };
        let ok = hotkeys::find(&h.key)
            .is_some_and(|def| unsafe { RegisterHotKey(Some(hwnd), a.id(), HOT_KEY_MODIFIERS(hotkeys::modifiers(h)), def.vk).is_ok() });
        if !ok {
            failed.push(a);
        }
    }
    shared.status.write().hotkey_conflicts = failed;
    st.registered = Some(wanted);
}

unsafe fn on_timer(hwnd: HWND, id: usize) {
    unsafe {
        match id {
            TIMER_HIDE => {
                let _ = KillTimer(Some(hwnd), TIMER_HIDE);
                SetTimer(Some(hwnd), TIMER_FADE, FADE_STEP_MS, None);
            }
            TIMER_FADE => {
                let alpha = STATE.with_borrow_mut(|st| {
                    st.alpha = st.alpha.saturating_sub(255 / FADE_STEPS + 1);
                    st.alpha
                });
                if alpha == 0 {
                    let _ = KillTimer(Some(hwnd), TIMER_FADE);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                } else {
                    // Solo cambia la opacidad: no hace falta volver a dibujar.
                    let blend = blend(alpha);
                    let _ = UpdateLayeredWindow(hwnd, None, None, None, None, None, COLORREF(0), Some(&blend), ULW_ALPHA);
                }
            }
            _ => {}
        }
    }
}

fn blend(alpha: u8) -> BLENDFUNCTION {
    BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 }
}

// ---------------------------------------------------------------------------------------------
// Dibujo
// ---------------------------------------------------------------------------------------------

/// Tamaño del aviso en píxeles lógicos (a 100 % de escala).
const W: f32 = 360.0;
const H: f32 = 92.0;
const MARGIN: f32 = 20.0;

/// Inter e iconos Phosphor (las mismas fuentes que la ventana), solo para este proceso.
fn load_fonts() {
    static FONTS: [&[u8]; 3] = [
        include_bytes!("../assets/fonts/latin-400-normal.ttf"),
        include_bytes!("../assets/fonts/latin-600-normal.ttf"),
        include_bytes!("../assets/fonts/Phosphor.ttf"),
    ];
    for data in FONTS {
        let n = 0u32;
        unsafe {
            let _ = AddFontMemResourceEx(data.as_ptr().cast(), data.len() as u32, None, &n);
        }
    }
}

fn font(px: f32, face: &str, weight: i32) -> HFONT {
    let face = HSTRING::from(face);
    unsafe {
        CreateFontW(
            -(px.round() as i32),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            &face,
        )
    }
}

fn rgb(c: Color32) -> [f32; 3] {
    [c.r() as f32, c.g() as f32, c.b() as f32]
}

/// Mapa de píxeles BGRA (el de la sección DIB) con figuras suavizadas.
struct Canvas<'a> {
    px: &'a mut [u32],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    /// Cobertura (0..1) del píxel (x, y) dentro de un rectángulo redondeado (función de distancia).
    fn coverage(x: i32, y: i32, r: [f32; 4], radius: f32) -> f32 {
        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let (hw, hh) = ((r[2] - r[0]) / 2.0 - radius, (r[3] - r[1]) / 2.0 - radius);
        let qx = (x as f32 + 0.5 - cx).abs() - hw;
        let qy = (y as f32 + 0.5 - cy).abs() - hh;
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        let d = outside + qx.max(qy).min(0.0) - radius;
        (0.5 - d).clamp(0.0, 1.0)
    }

    fn blend(&mut self, x: i32, y: i32, c: [f32; 3], a: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let p = &mut self.px[(y * self.w + x) as usize];
        let old = [(*p >> 16 & 0xff) as f32, (*p >> 8 & 0xff) as f32, (*p & 0xff) as f32];
        let mix = |i: usize| (old[i] + (c[i] - old[i]) * a).round() as u32;
        *p = mix(0) << 16 | mix(1) << 8 | mix(2);
    }

    /// Rectángulo redondeado relleno (r = [x0, y0, x1, y1]) con opacidad `a`.
    fn fill(&mut self, r: [f32; 4], radius: f32, c: Color32, a: f32) {
        let c = rgb(c);
        for y in r[1].floor() as i32..r[3].ceil() as i32 {
            for x in r[0].floor() as i32..r[2].ceil() as i32 {
                self.blend(x, y, c, a * Self::coverage(x, y, r, radius));
            }
        }
    }

    /// Borde de un rectángulo redondeado de grosor `t`.
    fn stroke(&mut self, r: [f32; 4], radius: f32, t: f32, c: Color32, a: f32) {
        let c = rgb(c);
        let inner = [r[0] + t, r[1] + t, r[2] - t, r[3] - t];
        for y in r[1].floor() as i32..r[3].ceil() as i32 {
            for x in r[0].floor() as i32..r[2].ceil() as i32 {
                let cov = Self::coverage(x, y, r, radius) - Self::coverage(x, y, inner, (radius - t).max(0.0));
                self.blend(x, y, c, a * cov.max(0.0));
            }
        }
    }
}

/// Texto con GDI en el rectángulo `r` (píxeles físicos).
unsafe fn text(dc: HDC, s: &str, r: [f32; 4], font: HFONT, c: Color32, format: DRAW_TEXT_FORMAT) {
    unsafe {
        let old = SelectObject(dc, font.into());
        SetTextColor(dc, COLORREF(c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16));
        let mut rect = RECT { left: r[0] as i32, top: r[1] as i32, right: r[2] as i32, bottom: r[3] as i32 };
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        DrawTextW(dc, &mut wide, &mut rect, format | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// Monitor donde está la ventana activa (el juego), su escala y la zona donde cabe el aviso.
fn placement() -> (RECT, f32) {
    unsafe {
        let fg = GetForegroundWindow();
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        // Juego a pantalla completa: la barra de tareas no se ve, se usa el monitor entero.
        let mut wr = RECT::default();
        let fullscreen = !fg.is_invalid()
            && GetWindowRect(fg, &mut wr).is_ok()
            && wr.left <= info.rcMonitor.left
            && wr.top <= info.rcMonitor.top
            && wr.right >= info.rcMonitor.right
            && wr.bottom >= info.rcMonitor.bottom;
        (if fullscreen { info.rcMonitor } else { info.rcWork }, dx as f32 / 96.0)
    }
}

/// Aviso ya dibujado en una sección DIB (BGRA premultiplicado), listo para `UpdateLayeredWindow`.
struct Painted {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Drop for Painted {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Dibuja el aviso de `t` a la escala `scale` (1.0 = 100 %). Con `volume_change`, anuncia el volumen
/// en vez del silencio.
fn paint(t: MuteTarget, s: &Settings, scale: f32, volume_change: bool) -> Option<Painted> {
    let muted = t.is_muted(s);
    let volume = (t.volume(s).clamp(0.0, 1.0) * 100.0).round();
    let (icon, color) = match t {
        MuteTarget::Mic => (icons::MICROPHONE, theme::MIC),
        MuteTarget::Master => (icons::HEADPHONES, theme::MASTER),
        MuteTarget::Channel(ch) => (theme::channel_icon(ch), theme::channel_color(ch)),
        MuteTarget::Stream => (icons::BROADCAST, theme::STREAM),
    };
    let (state_icon, state_color) = match (t, muted) {
        (MuteTarget::Mic, true) => (icons::MICROPHONE_OFF, theme::DANGER),
        (MuteTarget::Mic, false) => (icons::MICROPHONE, theme::ACCENT),
        (_, true) => (icons::SPEAKER_OFF, theme::DANGER),
        (_, false) => (icons::SPEAKER, theme::ACCENT),
    };
    let title = match (volume_change, muted) {
        (true, _) => tr!("Volumen", "Volume"),
        (false, true) => tr!("Silenciado", "Muted"),
        (false, false) => tr!("Activado", "Unmuted"),
    };

    let k = |v: f32| v * scale;
    let (w, h) = (k(W).round() as i32, k(H).round() as i32);

    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // de arriba abajo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, bmp.into());
        let out = Painted { dc, bmp, old, bits: bits.cast(), w, h };
        let px = std::slice::from_raw_parts_mut(out.bits, (w * h) as usize);

        // Fondo, borde, franja de color a la izquierda.
        let card = [0.0, 0.0, w as f32, h as f32];
        let bg = theme::SURFACE;
        px.fill((bg.r() as u32) << 16 | (bg.g() as u32) << 8 | bg.b() as u32);
        let mut cv = Canvas { px, w, h };
        cv.fill([0.0, 0.0, k(4.0), h as f32], 0.0, color, 1.0);
        cv.stroke(card, k(12.0), k(1.0), theme::BORDER_STRONG, 1.0);

        // Barra de volumen.
        let bar = [k(64.0), k(62.0), k(64.0 + 196.0), k(68.0)];
        cv.fill(bar, k(3.0), theme::SURFACE_3, 1.0);
        let fill_to = bar[0] + (bar[2] - bar[0]) * volume / 100.0;
        if fill_to > bar[0] + 1.0 {
            cv.fill([bar[0], bar[1], fill_to, bar[3]], k(3.0), if muted { theme::TEXT_3 } else { theme::ACCENT }, 1.0);
        }

        // Recuadro del estado a la derecha.
        let side = k(38.0);
        let bx = [w as f32 - k(18.0) - side, (h as f32 - side) / 2.0, w as f32 - k(18.0), (h as f32 + side) / 2.0];
        cv.fill(bx, k(8.0), state_color, 0.14);
        cv.stroke(bx, k(8.0), k(1.5), state_color, 0.7);

        // Textos e iconos con GDI (suavizado en gris: con ClearType saldrían franjas de color).
        SetBkMode(dc, TRANSPARENT);
        let f_title = font(k(17.0), "Inter SemiBold", 600);
        let f_sub = font(k(13.5), "Inter", 400);
        let f_num = font(k(12.0), "Inter", 400);
        let f_icon = font(k(28.0), "Phosphor", 400);
        let f_state = font(k(20.0), "Phosphor", 400);
        let center = DT_CENTER | DT_VCENTER;
        text(dc, icon, [k(14.0), 0.0, k(58.0), h as f32], f_icon, color, center);
        text(dc, title, [k(64.0), k(14.0), bx[0] - k(8.0), k(38.0)], f_title, theme::TEXT, DT_LEFT | DT_END_ELLIPSIS);
        text(dc, t.label(), [k(64.0), k(37.0), bx[0] - k(8.0), k(56.0)], f_sub, theme::TEXT_2, DT_LEFT | DT_END_ELLIPSIS);
        text(dc, &format!("{volume:.0}"), [bar[2] + k(10.0), k(55.0), bx[0], k(75.0)], f_num, theme::TEXT_3, DT_LEFT | DT_VCENTER);
        text(dc, state_icon, bx, f_state, state_color, center);
        for f in [f_title, f_sub, f_num, f_icon, f_state] {
            let _ = DeleteObject(f.into());
        }
        let _ = GdiFlush();

        // Esquinas redondeadas: opacidad por píxel (premultiplicada, como pide UpdateLayeredWindow).
        // GDI deja a 0 el canal alfa de lo que pinta, así que se recalcula entero aquí.
        let px = std::slice::from_raw_parts_mut(out.bits, (w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = Canvas::coverage(x, y, card, k(12.0));
                let p = &mut px[(y * w + x) as usize];
                let ch = |shift: u32| ((((*p >> shift) & 0xff) as f32) * a).round() as u32;
                *p = ((a * 255.0).round() as u32) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
            }
        }
        Some(out)
    }
}

/// Dibuja y muestra el aviso de `t` y programa que se desvanezca.
fn show(hwnd: HWND, t: MuteTarget, s: &Settings, volume_change: bool) {
    let (area, scale) = placement();
    let Some(painted) = paint(t, s, scale, volume_change) else { return };
    let (w, h) = (painted.w, painted.h);
    unsafe {
        let corner = s.overlay.corner;
        let m = (MARGIN * scale).round() as i32;
        let x = match corner {
            OverlayCorner::TopLeft | OverlayCorner::BottomLeft => area.left + m,
            _ => area.right - m - w,
        };
        let y = match corner {
            OverlayCorner::TopLeft | OverlayCorner::TopRight => area.top + m,
            _ => area.bottom - m - h,
        };
        let pos = POINT { x, y };
        let size = SIZE { cx: w, cy: h };
        let src = POINT::default();
        let blend = blend(255);
        let _ = UpdateLayeredWindow(hwnd, None, Some(&pos), Some(&size), Some(painted.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        drop(painted);

        STATE.with_borrow_mut(|st| st.alpha = 255);
        let _ = KillTimer(Some(hwnd), TIMER_FADE);
        // Encima incluso de otras ventanas «siempre visibles», sin quitarle el foco a nadie.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let ms = (s.overlay.seconds.clamp(0.5, 10.0) * 1000.0) as u32;
        SetTimer(Some(hwnd), TIMER_HIDE, ms, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Channel;

    /// Guarda el aviso en `target/aviso-*.bmp` para verlo sin mostrarlo en pantalla:
    /// `cargo test --release -- --ignored overlay_snapshots`
    #[test]
    #[ignore]
    fn overlay_snapshots() {
        load_fonts();
        let mut s = Settings::default();
        s.mic.muted = true;
        s.channel_mut(Channel::Chat).volume = 0.62;
        s.streamer.volume = 0.45;
        for (name, t, vol) in [
            ("mic-silenciado", MuteTarget::Mic, false),
            ("todo-activado", MuteTarget::Master, false),
            ("chat-activado", MuteTarget::Channel(Channel::Chat), false),
            ("retransmision-volumen", MuteTarget::Stream, true),
        ] {
            for scale in [1.0f32, 1.5] {
                let p = paint(t, &s, scale, vol).expect("dibujar");
                let px = unsafe { std::slice::from_raw_parts(p.bits, (p.w * p.h) as usize) };
                // BMP de 32 bits de arriba abajo (las sondas lo convierten a PNG para mirarlo).
                let mut f = Vec::new();
                let data_len = (p.w * p.h * 4) as u32;
                f.extend_from_slice(b"BM");
                f.extend_from_slice(&(54 + data_len).to_le_bytes());
                f.extend_from_slice(&0u32.to_le_bytes());
                f.extend_from_slice(&54u32.to_le_bytes());
                f.extend_from_slice(&40u32.to_le_bytes());
                f.extend_from_slice(&p.w.to_le_bytes());
                f.extend_from_slice(&(-p.h).to_le_bytes());
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&32u16.to_le_bytes());
                f.extend_from_slice(&[0u8; 24]);
                for v in px {
                    f.extend_from_slice(&v.to_le_bytes());
                }
                std::fs::write(format!("target/aviso-{name}-{scale}.bmp"), f).unwrap();
            }
        }
    }
}
