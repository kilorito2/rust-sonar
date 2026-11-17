//! Icono en la bandeja del sistema (junto al reloj), con Win32 directo.
//!
//! Vive en su propio hilo con su propio bucle de mensajes, independiente de la ventana de egui:
//! así sigue respondiendo aunque la ventana esté oculta.
//!
//! - Clic: muestra la ventana.
//! - Clic derecho: menú (abrir, silenciar micro, silenciar todo, modo streamer, salir).

use crate::state::Shared;
use crate::system;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

const CMD_OPEN: usize = 1;
const CMD_MUTE_MIC: usize = 2;
const CMD_MUTE_ALL: usize = 3;
const CMD_STREAMER: usize = 4;
const CMD_QUIT: usize = 9;

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
/// Mensaje que Windows difunde cuando el Explorador se reinicia: hay que volver a poner el icono.
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

pub fn spawn(shared: Arc<Shared>, icon_rgba: Vec<u8>, icon_size: u32) {
    let _ = SHARED.set(shared);
    std::thread::Builder::new()
        .name("bandeja".into())
        .spawn(move || unsafe {
            if let Err(e) = run(&icon_rgba, icon_size) {
                eprintln!("[bandeja] {e:#}");
            }
        })
        .unwrap();
}

/// Quita el icono (si no, se queda un icono "fantasma" hasta pasar el ratón por encima).
pub fn remove() {
    let raw = TRAY_HWND.swap(0, Ordering::AcqRel);
    if raw != 0 {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(HWND(raw as *mut _)));
        }
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: TRAY_ID, ..Default::default() }
}

unsafe fn add_icon(hwnd: HWND, icon: HICON) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    for (dst, src) in data.szTip.iter_mut().zip("Rust Sonar".encode_utf16()) {
        *dst = src;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

/// Icono de Windows a partir de RGBA (CreateIcon quiere BGRA y una máscara AND aparte).
unsafe fn make_icon(rgba: &[u8], size: u32) -> windows::core::Result<HICON> {
    let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mask = vec![0u8; (size * size / 8) as usize];
    unsafe { CreateIcon(None, size as i32, size as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }
}

static ICON: AtomicIsize = AtomicIsize::new(0);

unsafe fn run(icon_rgba: &[u8], icon_size: u32) -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("RustSonarTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));

        // Ventana normal pero nunca visible: una de solo-mensajes no recibiría "TaskbarCreated".
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Rust Sonar (bandeja)"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        TRAY_HWND.store(hwnd.0 as isize, Ordering::Release);

        let icon = make_icon(icon_rgba, icon_size)?;
        ICON.store(icon.0 as isize, Ordering::Release);
        add_icon(hwnd, icon);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TRAY {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => system::open_ui(),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        if msg == WM_COMMAND {
            on_command(wparam.0 & 0xffff);
            return LRESULT(0);
        }
        if Some(&msg) == TASKBAR_CREATED.get() {
            add_icon(hwnd, HICON(ICON.load(Ordering::Acquire) as *mut _));
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Some(shared) = SHARED.get() else { return };
    let (mic_muted, all_muted, streamer) = {
        let s = shared.settings.read();
        (s.mic.muted, s.master_muted, s.streamer.enabled)
    };
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let open = HSTRING::from(tr!("Abrir Rust Sonar", "Open Rust Sonar"));
        let mute_mic = HSTRING::from(tr!("Silenciar micrófono", "Mute microphone"));
        let mute_all = HSTRING::from(tr!("Silenciar todo", "Mute all"));
        let streamer_text = HSTRING::from(tr!("Modo streamer", "Streamer mode"));
        let quit = HSTRING::from(tr!("Salir", "Quit"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN, PCWSTR(open.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING | check(mic_muted), CMD_MUTE_MIC, PCWSTR(mute_mic.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING | check(all_muted), CMD_MUTE_ALL, PCWSTR(mute_all.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING | check(streamer), CMD_STREAMER, PCWSTR(streamer_text.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, PCWSTR(quit.as_ptr()));
        let _ = SetMenuDefaultItem(menu, CMD_OPEN as u32, 0);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Sin esto el menú no se cierra al hacer clic fuera (comportamiento documentado de Win32).
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn on_command(cmd: usize) {
    let Some(shared) = SHARED.get() else { return };
    match cmd {
        CMD_OPEN => system::open_ui(),
        CMD_MUTE_MIC => shared.update(|s| s.mic.muted = !s.mic.muted),
        CMD_MUTE_ALL => shared.update(|s| s.master_muted = !s.master_muted),
        CMD_STREAMER => shared.update(|s| s.streamer.enabled = !s.streamer.enabled),
        CMD_QUIT => crate::request_quit(shared),
        _ => {}
    }
}
