//! Instalador de Rust Sonar con la misma estética que la app: ventana sin marco, panel de marca con
//! los pasos, bienvenida, opciones con interruptores, progreso en vivo y pantalla final.
//!
//! El trabajo lo hace el instalador de Inno Setup que va incrustado (`RUST_SONAR_PAYLOAD`): se
//! extrae a la carpeta temporal y se ejecuta como administrador en modo invisible
//! (`/VERYSILENT`). Su registro (`/LOG`) se va leyendo para saber por qué paso va.

#![windows_subsystem = "windows"]

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, pos2, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Idioma del instalador: el de Windows (español si Windows está en español, si no inglés).
/// Para probar: RUST_SONAR_LANG=en|es.
fn en() -> bool {
    static EN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *EN.get_or_init(|| match std::env::var("RUST_SONAR_LANG").as_deref() {
        Ok("en") => true,
        Ok("es") => false,
        _ => (unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff) != 0x0a,
    })
}

/// Texto en español e inglés.
macro_rules! tr {
    ($es:expr, $en:expr $(,)?) => {
        if crate::en() { $en } else { $es }
    };
}

/// `format!` en español e inglés.
macro_rules! trf {
    ($es:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if crate::en() { format!($en $(, $arg)*) } else { format!($es $(, $arg)*) }
    };
}

/// Nombre visible de las constantes que están escritas en español.
fn label(es: &'static str) -> &'static str {
    if !en() {
        return es;
    }
    match es {
        "Juego" => "Game",
        "Micro" => "Mic",
        "Bienvenida" => "Welcome",
        "Opciones" => "Options",
        "Instalación" => "Install",
        "Listo" => "Done",
        other => other,
    }
}

const PAYLOAD: &[u8] = include_bytes!(env!("RUST_SONAR_PAYLOAD"));
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// AppId del script de Inno + "_is1": clave de desinstalación si ya está instalado.
const UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{6E0B5A43-2C1D-4C59-9C1E-5B7A3F0D8E21}_is1";

// ---------------------------------------------------------------------------------------------
// Estética (la misma paleta que la app)
// ---------------------------------------------------------------------------------------------

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
const BG: Color32 = rgb(0x0d, 0x0f, 0x13);
const SIDEBAR: Color32 = rgb(0x11, 0x14, 0x19);
const SURFACE: Color32 = rgb(0x16, 0x1a, 0x21);
const SURFACE_2: Color32 = rgb(0x1c, 0x21, 0x2a);
const SURFACE_3: Color32 = rgb(0x25, 0x2b, 0x36);
const BORDER: Color32 = rgb(0x24, 0x2a, 0x34);
const BORDER_STRONG: Color32 = rgb(0x34, 0x3c, 0x4a);
const TEXT: Color32 = rgb(0xe6, 0xe9, 0xef);
const TEXT_2: Color32 = rgb(0x9a, 0xa3, 0xb2);
const TEXT_3: Color32 = rgb(0x67, 0x70, 0x80);
const ACCENT: Color32 = rgb(0x2d, 0xd4, 0xbf);
const DANGER: Color32 = rgb(0xf8, 0x71, 0x71);
const OK: Color32 = rgb(0x4a, 0xde, 0x80);
const CHANNELS: [(&str, &str, Color32); 5] = [
    ("Juego", icons::GAME, rgb(0x84, 0xcc, 0x16)),
    ("Chat", icons::CHAT, rgb(0x38, 0xbd, 0xf8)),
    ("Media", icons::MEDIA, rgb(0xfb, 0xbf, 0x24)),
    ("Aux", icons::AUX, rgb(0xa7, 0x8b, 0xfa)),
    ("Micro", icons::MICROPHONE, rgb(0xf4, 0x72, 0xb6)),
];

mod icons {
    pub const GAME: &str = "\u{e26e}";
    pub const CHAT: &str = "\u{e17c}";
    pub const MEDIA: &str = "\u{e340}";
    pub const AUX: &str = "\u{e464}";
    pub const MICROPHONE: &str = "\u{e326}";
    pub const SLIDERS: &str = "\u{e434}";
    pub const SPARKLE: &str = "\u{e6a2}";
    pub const EQUALIZER: &str = "\u{ebbc}";
    pub const PLUGS: &str = "\u{eb5a}";
    pub const POWER: &str = "\u{e3da}";
    pub const DESKTOP: &str = "\u{e560}";
    pub const FOLDER: &str = "\u{e24a}";
    pub const ROCKET: &str = "\u{e3fe}";
    pub const ARROW_RIGHT: &str = "\u{e06c}";
    pub const ARROW_LEFT: &str = "\u{e058}";
    pub const SHIELD: &str = "\u{e40c}";
    pub const CHECK: &str = "\u{e182}";
    pub const SEAL: &str = "\u{e606}";
    pub const WARNING: &str = "\u{e4e0}";
    pub const FILE: &str = "\u{e23a}";
    pub const RETRY: &str = "\u{e036}";
    pub const CLOSE: &str = "\u{e4f6}";
    pub const MINIMIZE: &str = "\u{e32a}";
}

fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

fn install_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("inter", &include_bytes!("../../../assets/fonts/latin-400-normal.ttf")[..]),
        ("inter-medium", &include_bytes!("../../../assets/fonts/latin-500-normal.ttf")[..]),
        ("inter-semibold", &include_bytes!("../../../assets/fonts/latin-600-normal.ttf")[..]),
        ("phosphor", &include_bytes!("../../../assets/fonts/Phosphor.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |main: &str| {
        let mut v = vec![main.to_string(), "phosphor".to_string()];
        v.extend(defaults.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("inter"));
    fonts.families.insert(FontFamily::Name("medium".into()), family("inter-medium"));
    fonts.families.insert(FontFamily::Name("semibold".into()), family("inter-semibold"));
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["phosphor".into()]);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(10.0, 10.0);
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.extreme_bg_color = SURFACE;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.28);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
            w.corner_radius = CornerRadius::same(10);
        }
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    });
}

// ---------------------------------------------------------------------------------------------
// Sistema: drivers instalados, instalación previa, elevación
// ---------------------------------------------------------------------------------------------

mod sys {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, PCWSTR};

    pub fn key_exists(path: &str) -> bool {
        let mut key = HKEY::default();
        let path = HSTRING::from(path);
        unsafe {
            let ok = RegOpenKeyExW(HKEY_LOCAL_MACHINE, &path, None, KEY_READ | KEY_WOW64_64KEY, &mut key).is_ok();
            if ok {
                let _ = RegCloseKey(key);
            }
            ok
        }
    }

    pub fn read_string(path: &str, value: &str) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut size = std::mem::size_of_val(&buf) as u32;
        let (path, value) = (HSTRING::from(path), HSTRING::from(value));
        unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, &path, &value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)).ok().ok()?;
        }
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    /// Proceso lanzado como administrador.
    pub struct Elevated(HANDLE);

    impl Elevated {
        /// Lanza `exe` como administrador (aviso de UAC). Error si se cancela.
        pub fn spawn(exe: &std::path::Path, params: &str) -> Result<Self, String> {
            let (verb, file, params) = (HSTRING::from("runas"), HSTRING::from(exe.as_os_str()), HSTRING::from(params));
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: PCWSTR(verb.as_ptr()),
                lpFile: PCWSTR(file.as_ptr()),
                lpParameters: PCWSTR(params.as_ptr()),
                nShow: SW_HIDE.0,
                ..Default::default()
            };
            unsafe { ShellExecuteExW(&mut info) }.map_err(|_| tr!("Hace falta aceptar el aviso de administrador de Windows para instalar.", "You need to accept the Windows administrator prompt to install.").to_string())?;
            if info.hProcess.is_invalid() {
                return Err(tr!("No se pudo lanzar el instalador.", "Couldn't start the installer.").into());
            }
            Ok(Self(info.hProcess))
        }

        /// Espera hasta `ms` milisegundos; Some(código de salida) si ha terminado.
        pub fn wait(&self, ms: u32) -> Option<u32> {
            unsafe {
                if WaitForSingleObject(self.0, ms) != WAIT_OBJECT_0 {
                    return None;
                }
                let mut code = 1u32;
                let _ = GetExitCodeProcess(self.0, &mut code);
                Some(code)
            }
        }
    }

    impl Drop for Elevated {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct Driver {
    name: &'static str,
    purpose: &'static str,
    service: &'static str,
    /// Nombre del instalador de VB-Audio, tal como aparece en el registro de Inno.
    setup: &'static str,
    /// Paquete que se descarga de vb-audio.com (también aparece en el registro de Inno).
    download: &'static str,
    installed: bool,
}

fn detect_drivers() -> Vec<Driver> {
    [
        ("VB-CABLE", tr!("Micrófono virtual (lo que oye Discord)", "Virtual microphone (what Discord hears)"), "VBAudioVACMME", "vbmmecable64", "VBCABLE_Setup_x64.exe", "VBCABLE_Driver_Pack46.zip"),
        ("Hi-Fi Cable", tr!("Salida del canal Juego", "Game channel output"), "VBAudioHFVAIOMME", "vbhfvaio64", "HiFiCableAsioBridgeSetup.exe", "HiFiCableAsioBridgeSetup_v1007.zip"),
        ("Voicemeeter", tr!("Salidas de Chat, Media y Aux", "Chat, Media and Aux outputs"), "VBVoicemeeterVAIOMME", "vbvoicemeetervaio64", "Voicemeeter8Setup.exe", "Voicemeeter8Setup_v3130.zip"),
    ]
    .into_iter()
    .map(|(name, purpose, service, package, setup, download)| Driver {
        name,
        purpose,
        service,
        setup,
        download,
        installed: driver_installed(service, package),
    })
    .collect()
}

/// Un driver está instalado si tiene su servicio en el registro Y su paquete en el almacén de drivers
/// de Windows. Solo con el servicio no basta: al borrar los drivers a mano (pnputil, el desinstalador
/// de VB-Audio) la clave del servicio se queda huérfana y parecería instalado sin estarlo.
fn driver_installed(service: &str, package_prefix: &str) -> bool {
    sys::key_exists(&format!("SYSTEM\\CurrentControlSet\\Services\\{service}")) && in_driver_store(package_prefix)
}

/// ¿Hay en `%SystemRoot%\System32\DriverStore\FileRepository` una carpeta que empiece por `prefix`?
fn in_driver_store(prefix: &str) -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let repo = std::path::Path::new(&root).join("System32").join("DriverStore").join("FileRepository");
    std::fs::read_dir(repo)
        .map(|dir| dir.flatten().any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with(prefix)))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Progreso de la instalación (lo actualiza el hilo que lee el registro de Inno)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum ItemState {
    Pending,
    Running,
    Done,
}

#[derive(Clone)]
struct Item {
    label: String,
    detail: String,
    state: ItemState,
    /// Instalador de driver al que corresponde (para reconocerlo en el registro).
    setup: Option<&'static str>,
    /// Paquete del driver en vb-audio.com (para reconocer su descarga en el registro).
    download: Option<&'static str>,
    /// Se está descargando (antes de copiar archivos; Inno baja los paquetes al preparar).
    downloading: bool,
    /// Detalle original, para volver a él al acabar la descarga.
    base_detail: String,
}

#[derive(Default)]
struct Progress {
    items: Vec<Item>,
    files: usize,
    result: Option<Result<bool, String>>,
    log: Option<PathBuf>,
}

const ITEM_ADMIN: usize = 0;
const ITEM_FILES: usize = 1;
const ITEM_SHORTCUTS: usize = 2;

impl Progress {
    fn new(drivers: &[&Driver]) -> Self {
        let item = |label: &str, detail: &str, setup, download| Item { label: label.into(), detail: detail.into(), state: ItemState::Pending, setup, download, downloading: false, base_detail: detail.into() };
        let mut items = vec![
            item(tr!("Permisos de administrador", "Administrator rights"), tr!("Acepta el aviso de Windows", "Accept the Windows prompt"), None, None),
            item(tr!("Copiando Rust Sonar", "Copying Rust Sonar"), tr!("Archivos de programa", "Program Files"), None, None),
            item(tr!("Menú Inicio y desinstalador", "Start menu and uninstaller"), tr!("Accesos directos", "Shortcuts"), None, None),
        ];
        for d in drivers {
            items.push(item(&trf!("Instalando {}", "Installing {}", d.name), d.purpose, Some(d.setup), Some(d.download)));
        }
        items[ITEM_ADMIN].state = ItemState::Running;
        Self { items, ..Default::default() }
    }

    /// Marca `i` como en curso y todo lo anterior como hecho.
    fn start(&mut self, i: usize) {
        for (j, item) in self.items.iter_mut().enumerate() {
            if j < i {
                item.state = ItemState::Done;
            } else if j == i && item.state == ItemState::Pending {
                item.state = ItemState::Running;
            }
        }
    }

    fn finish_all(&mut self) {
        for item in &mut self.items {
            item.state = ItemState::Done;
        }
    }

    /// Termina el paso de descarga: los drivers vuelven a "pendiente" hasta que se ejecute su instalador.
    fn end_downloads(&mut self) {
        for item in self.items.iter_mut().filter(|it| it.downloading) {
            item.downloading = false;
            item.state = ItemState::Pending;
            item.detail = item.base_detail.clone();
        }
    }

    /// Interpreta una línea del registro de Inno Setup.
    fn on_log_line(&mut self, line: &str) {
        if line.contains("Downloading temporary file from") {
            self.end_downloads();
            if let Some(item) = self.items.iter_mut().find(|it| it.download.is_some_and(|z| line.contains(z))) {
                item.downloading = true;
                item.state = ItemState::Running;
                item.detail = tr!("Descargando de vb-audio.com…", "Downloading from vb-audio.com…").into();
            }
            return;
        }
        if line.contains("Dest filename:") || line.contains("Start extracting archive") {
            self.end_downloads();
        }
        if line.contains("Dest filename:") {
            if line.to_ascii_lowercase().ends_with(".lnk") {
                self.start(ITEM_SHORTCUTS);
            } else {
                self.files += 1;
                self.start(ITEM_FILES);
            }
        } else if line.contains("Filename:") {
            let found = self.items.iter().position(|it| it.setup.is_some_and(|s| line.contains(s)));
            if let Some(i) = found {
                self.start(i);
                if self.items[i].setup == Some("Voicemeeter8Setup.exe") {
                    self.items[i].detail = tr!("Pulsa «Install» en su ventana y después OK", "Press “Install” in its window, then OK").into();
                }
            }
        } else if line.contains("Process exit code:") {
            if let Some(item) = self.items.iter_mut().find(|it| it.setup.is_some() && it.state == ItemState::Running) {
                item.state = ItemState::Done;
            }
        }
    }

    /// Fracción completada (0..1) para la barra.
    fn fraction(&self) -> f32 {
        if self.result.as_ref().is_some_and(|r| r.is_ok()) {
            return 1.0;
        }
        let n = self.items.len() as f32;
        let mut done = 0.0;
        for (i, item) in self.items.iter().enumerate() {
            done += match item.state {
                ItemState::Done => 1.0,
                // Copiando: avanza con cada archivo; el resto, medio paso mientras está en curso.
                ItemState::Running if i == ITEM_FILES => (self.files as f32 / 40.0).min(0.9),
                ItemState::Running => 0.45,
                ItemState::Pending => 0.0,
            };
        }
        (done / n).clamp(0.02, 0.98)
    }

    fn current(&self) -> Option<&Item> {
        self.items.iter().rev().find(|it| it.state == ItemState::Running)
    }
}

fn exit_message(code: u32) -> String {
    match code {
        2 | 5 => tr!("Se canceló la instalación.", "The installation was cancelled.").into(),
        7 => tr!("No se pudo preparar la instalación. Cierra Rust Sonar si está abierto y vuelve a intentarlo.", "Couldn't prepare the installation. Close Rust Sonar if it's open and try again.").into(),
        3 | 4 => tr!("Hubo un error durante la instalación. El registro tiene los detalles.", "Something went wrong during the installation. The log has the details.").into(),
        c => trf!("El instalador terminó con el código {c}. El registro tiene los detalles.", "The installer exited with code {c}. The log has the details."),
    }
}

/// Hilo de instalación: extrae el instalador interno, lo lanza elevado y sigue su registro.
fn run_install(tasks: String, dir: String, progress: Arc<Mutex<Progress>>) {
    let result = (|| -> Result<bool, String> {
        let tmp = std::env::temp_dir();
        let exe = tmp.join(format!("RustSonar-Setup-{}.exe", std::process::id()));
        let log = tmp.join(format!("RustSonar-Instalacion-{}.log", std::process::id()));
        std::fs::write(&exe, PAYLOAD).map_err(|e| trf!("No se pudo preparar el instalador: {e}", "Couldn't prepare the installer: {e}"))?;
        progress.lock().unwrap().log = Some(log.clone());
        let params = format!("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TASKS=\"{tasks}\" /DIR=\"{dir}\" /LOG=\"{}\" /LANG={}", log.display(), if en() { "en" } else { "es" });
        let proc = sys::Elevated::spawn(&exe, &params);
        let proc = match proc {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&exe);
                return Err(e);
            }
        };
        progress.lock().unwrap().start(ITEM_FILES);

        let mut seen = 0usize;
        let code = loop {
            let finished = proc.wait(150);
            if let Ok(bytes) = std::fs::read(&log) {
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<&str> = text.lines().collect();
                // La última línea puede estar a medias: solo se leen las completas (salvo al terminar).
                let complete = if finished.is_some() { lines.len() } else { lines.len().saturating_sub(1) };
                if complete > seen {
                    let mut p = progress.lock().unwrap();
                    for line in &lines[seen..complete] {
                        p.on_log_line(line);
                    }
                    seen = complete;
                }
            }
            if let Some(code) = finished {
                break code;
            }
        };
        let _ = std::fs::remove_file(&exe);
        if code != 0 {
            return Err(exit_message(code));
        }
        let restart = std::fs::read(&log).map(|b| String::from_utf8_lossy(&b).contains("Need to restart Windows? Yes")).unwrap_or(false);
        Ok(restart)
    })();
    let mut p = progress.lock().unwrap();
    if result.is_ok() {
        p.finish_all();
    }
    p.result = Some(result);
}

// ---------------------------------------------------------------------------------------------
// Asistente
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Page {
    Welcome,
    Options,
    Installing,
    Done,
}

const STEPS: [(Page, &str); 4] = [(Page::Welcome, "Bienvenida"), (Page::Options, "Opciones"), (Page::Installing, "Instalación"), (Page::Done, "Listo")];

struct Setup {
    page: Page,
    page_since: Instant,
    drivers: Vec<Driver>,
    install_drivers: bool,
    autostart: bool,
    desktop: bool,
    dir: String,
    upgrade: bool,
    progress: Arc<Mutex<Progress>>,
    /// Valor mostrado de la barra (persigue al real con suavidad).
    shown: f32,
}

impl Setup {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_style(&cc.egui_ctx);
        let upgrade = sys::key_exists(UNINSTALL_KEY);
        let default_dir = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| "C:\\Program Files".into());
        let dir = sys::read_string(UNINSTALL_KEY, "InstallLocation")
            .map(|d| d.trim_end_matches('\\').to_string())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("{default_dir}\\Rust Sonar"));
        let mut setup = Self {
            page: Page::Welcome,
            page_since: Instant::now(),
            drivers: detect_drivers(),
            install_drivers: true,
            autostart: true,
            desktop: false,
            dir,
            upgrade,
            progress: Arc::new(Mutex::new(Progress::default())),
            shown: 0.0,
        };
        setup.demo();
        // Prueba de extremo a extremo sin pulsar nada (instala de verdad con las opciones por defecto).
        if std::env::var("RUST_SONAR_SETUP_AUTO").as_deref() == Ok("1") {
            setup.start_install();
        }
        setup
    }

    /// Solo para revisar el diseño sin instalar nada:
    /// `RUST_SONAR_SETUP_DEMO=bienvenida|opciones|instalando|listo|reinicio|error`.
    fn demo(&mut self) {
        let Ok(page) = std::env::var("RUST_SONAR_SETUP_DEMO") else { return };
        // Simula un PC sin drivers para ver todos los estados.
        for d in &mut self.drivers {
            d.installed = d.name == "VB-CABLE";
        }
        let drivers: Vec<&Driver> = self.drivers.iter().filter(|d| !d.installed).collect();
        let mut p = Progress::new(&drivers);
        match page.as_str() {
            "opciones" => self.page = Page::Options,
            "instalando" => {
                p.files = 40;
                p.start(4);
                p.items[4].detail = tr!("Pulsa «Install» en su ventana y después OK", "Press “Install” in its window, then OK").into();
                self.page = Page::Installing;
                self.shown = p.fraction();
            }
            "listo" | "reinicio" => {
                p.finish_all();
                p.result = Some(Ok(page == "reinicio"));
                self.page = Page::Done;
            }
            "error" => {
                p.result = Some(Err(exit_message(7)));
                self.page = Page::Done;
            }
            _ => {}
        }
        self.progress = Arc::new(Mutex::new(p));
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.page_since = Instant::now();
    }

    fn missing_drivers(&self) -> Vec<&Driver> {
        self.drivers.iter().filter(|d| !d.installed).collect()
    }

    fn start_install(&mut self) {
        let drivers: Vec<&Driver> = if self.install_drivers { self.missing_drivers() } else { Vec::new() };
        let mut tasks = Vec::new();
        if !drivers.is_empty() {
            tasks.push("drivers");
        }
        if self.autostart {
            tasks.push("autostart");
        }
        if self.desktop {
            tasks.push("desktopicon");
        }
        self.progress = Arc::new(Mutex::new(Progress::new(&drivers)));
        self.shown = 0.0;
        let (progress, tasks, dir) = (self.progress.clone(), tasks.join(","), self.dir.trim().trim_end_matches('\\').to_string());
        std::thread::spawn(move || run_install(tasks, dir, progress));
        self.go(Page::Installing);
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        if self.page == Page::Installing && self.progress.lock().unwrap().result.is_some() {
            self.go(Page::Done);
        }

        egui::Panel::left("brand")
            .exact_size(260.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin { left: 28, right: 24, top: 30, bottom: 24 }))
            .show(ui, |ui| self.brand(ui));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let full = ui.max_rect();
            ui.painter().rect_filled(full, CornerRadius::ZERO, BG);
            // Brillo turquesa muy suave arriba a la derecha (círculos concéntricos = degradado).
            for k in 0..14 {
                let f = k as f32 / 14.0;
                ui.painter().circle_filled(pos2(full.right() - 30.0, full.top() + 10.0), 260.0 * (1.0 - f), ACCENT.gamma_multiply(0.006));
            }
            self.title_bar(ui, full);
            let content = Rect::from_min_max(pos2(full.left() + 44.0, full.top() + 52.0), pos2(full.right() - 44.0, full.bottom() - 32.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
            // Entrada suave de cada página.
            let t = (self.page_since.elapsed().as_secs_f32() / 0.28).min(1.0);
            let ease = 1.0 - (1.0 - t).powi(3);
            child.multiply_opacity(ease);
            child.add_space((1.0 - ease) * 14.0);
            match self.page {
                Page::Welcome => self.welcome(&mut child),
                Page::Options => self.options(&mut child),
                Page::Installing => self.installing(&mut child),
                Page::Done => self.done(&mut child),
            }
        });

        // Borde fino de la ventana.
        let screen = ui.ctx().content_rect();
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("border")))
            .rect_stroke(screen, CornerRadius::ZERO, Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);
    }
}

impl Setup {
    // ----- Panel de marca --------------------------------------------------------------------

    fn brand(&mut self, ui: &mut Ui) {
        // Arrastrar la ventana desde el panel.
        let r = ui.max_rect().expand2(vec2(28.0, 30.0));
        if ui.interact(r, ui.scope_id().with("drag_brand"), Sense::drag()).drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        paint_logo(ui.painter(), rect.center(), 30.0);
        ui.add_space(8.0);
        ui.label(RichText::new("Rust Sonar").font(semibold(24.0)).color(TEXT));
        ui.label(RichText::new(format!("{} · v{VERSION}", if self.upgrade { tr!("Actualización", "Update") } else { tr!("Instalador", "Installer") })).font(regular(12.5)).color(TEXT_3));
        ui.add_space(34.0);

        // Pasos.
        let current = self.page;
        let failed = self.page == Page::Done && self.progress.lock().unwrap().result.as_ref().is_some_and(|r| r.is_err());
        let mut prev: Option<Pos2> = None;
        for (page, step) in STEPS {
            let label = self::label(step);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            let c = pos2(row.left() + 13.0, row.center().y);
            let p = ui.painter();
            if let Some(pc) = prev {
                let color = if page <= current { ACCENT.gamma_multiply(0.7) } else { BORDER_STRONG };
                p.line_segment([pos2(pc.x, pc.y + 14.0), pos2(c.x, c.y - 14.0)], Stroke::new(2.0, color));
            }
            let (text_color, font) = if page == current { (TEXT, semibold(14.5)) } else if page < current { (TEXT_2, medium(14.0)) } else { (TEXT_3, medium(14.0)) };
            if page < current || (page == Page::Done && current == Page::Done && !failed) {
                p.circle_filled(c, 12.0, ACCENT);
                p.text(c, Align2::CENTER_CENTER, icons::CHECK, icon(13.0), BG);
            } else if page == current {
                let color = if failed { DANGER } else { ACCENT };
                let pulse = (ui.input(|i| i.time) as f32 * 2.4).sin() * 0.5 + 0.5;
                p.circle_filled(c, 12.0 + 4.0 * pulse, color.gamma_multiply(0.10));
                p.circle_stroke(c, 11.0, Stroke::new(2.0, color));
                p.circle_filled(c, 4.5, color);
            } else {
                p.circle_stroke(c, 11.0, Stroke::new(1.5, BORDER_STRONG));
            }
            p.text(pos2(row.left() + 38.0, c.y), Align2::LEFT_CENTER, label, font, text_color);
            prev = Some(c);
        }

        // Abajo: franja de canales y firma.
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icons::SEAL).font(icon(14.0)).color(ACCENT));
                ui.label(RichText::new(tr!("Firmado por kilorito", "Signed by kilorito")).font(medium(12.0)).color(TEXT_3));
            });
            ui.add_space(14.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            let t = ui.input(|i| i.time) as f32;
            for (i, (_, _, color)) in CHANNELS.iter().enumerate() {
                // Barras que "suenan" suavemente, como un vúmetro.
                let h = 14.0 + 26.0 * (0.5 + 0.5 * (t * (1.3 + i as f32 * 0.37) + i as f32 * 1.7).sin()).powf(1.5);
                let x = rect.left() + i as f32 * 22.0;
                let bar = Rect::from_min_max(pos2(x, rect.bottom() - h), pos2(x + 14.0, rect.bottom()));
                ui.painter().rect_filled(bar, CornerRadius::same(4), *color);
            }
        });
    }

    // ----- Barra de título -------------------------------------------------------------------

    fn title_bar(&mut self, ui: &mut Ui, full: Rect) {
        let bar = Rect::from_min_size(full.min, vec2(full.width(), 40.0));
        if ui.interact(bar, ui.scope_id().with("drag_title"), Sense::drag()).drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let installing = self.page == Page::Installing;
        let close = Rect::from_min_size(pos2(full.right() - 46.0, full.top()), vec2(46.0, 36.0));
        let min = close.translate(vec2(-46.0, 0.0));
        if window_button(ui, min, icons::MINIMIZE, false, true).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        let r = window_button(ui, close, icons::CLOSE, true, !installing);
        if r.clicked() && !installing {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if installing {
            r.on_hover_text(tr!("Espera a que termine la instalación", "Wait for the installation to finish"));
        }
    }

    // ----- Páginas ---------------------------------------------------------------------------

    fn welcome(&mut self, ui: &mut Ui) {
        pill(ui, &trf!("v{VERSION} · Windows 10 y 11", "v{VERSION} · Windows 10 and 11"), ACCENT);
        ui.add_space(6.0);
        let title = if self.upgrade { tr!("Actualizar Rust Sonar", "Update Rust Sonar") } else { tr!("Bienvenido a Rust Sonar", "Welcome to Rust Sonar") };
        ui.label(RichText::new(title).font(semibold(30.0)).color(TEXT));
        let text = if self.upgrade {
            tr!("Ya tienes Rust Sonar instalado. Se actualizará a esta versión conservando tu configuración, tus presets y tus dispositivos.", "Rust Sonar is already installed. It will be updated to this version, keeping your settings, presets and devices.")
        } else {
            tr!("Mezcla el juego, el chat, la música y tu micro por separado, con ecualizador, ChatMix y limpieza de voz con IA. Todo en tu PC, sin cuentas.", "Mix your game, chat, music and mic separately, with EQ, ChatMix and AI voice cleanup. All on your PC, no accounts.")
        };
        ui.label(RichText::new(text).font(regular(14.5)).color(TEXT_2));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for (name, ic, color) in CHANNELS {
                channel_chip(ui, ic, label(name), color);
            }
        });
        ui.add_space(14.0);
        for (ic, title, sub) in [
            (icons::SLIDERS, tr!("Cada app en su canal", "Every app on its own channel"), tr!("Windows enruta solo cada programa al canal que elijas, y lo recuerda.", "Windows routes each program to the channel you pick, and remembers it.")),
            (icons::SPARKLE, tr!("Micrófono con IA", "AI microphone"), tr!("Quita el ruido del teclado y los ventiladores antes de que llegue a Discord.", "Removes keyboard and fan noise before it reaches Discord.")),
            (icons::EQUALIZER, tr!("Ecualizador y audio espacial", "Equalizer and spatial audio"), tr!("Presets, curva en vivo y sonido 7.1 alrededor de tu cabeza.", "Presets, a live curve and 7.1 sound around your head.")),
        ] {
            feature_row(ui, ic, title, sub);
        }

        footer(ui, |ui| {
            if primary_button(ui, Some(icons::ARROW_RIGHT), tr!("Empezar", "Get started")).clicked() {
                self.go(Page::Options);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(tr!("Descarga los drivers de VB-Audio (donationware) de vb-audio.com", "Downloads the VB-Audio drivers (donationware) from vb-audio.com")).font(regular(11.5)).color(TEXT_3));
            });
        });
    }

    fn options(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(tr!("Elige cómo instalarlo", "Choose how to install it")).font(semibold(26.0)).color(TEXT));
        ui.label(RichText::new(tr!("Puedes cambiar todo esto más tarde desde Ajustes.", "You can change all of this later in Settings.")).font(regular(13.5)).color(TEXT_2));
        ui.add_space(2.0);

        let missing = self.missing_drivers().len();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                badge(ui, icons::PLUGS, ACCENT);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(tr!("Cables virtuales", "Virtual cables")).font(semibold(15.0)).color(TEXT));
                    let sub = if missing == 0 { tr!("Ya tienes todos los que hacen falta", "You already have everything you need") } else { tr!("Una salida por canal y el micrófono virtual", "One output per channel plus the virtual microphone") };
                    ui.label(RichText::new(sub).font(regular(12.5)).color(TEXT_3));
                });
                if missing > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        toggle(ui, &mut self.install_drivers);
                    });
                }
            });
            ui.spacing_mut().item_spacing.y = 6.0;
            for d in &self.drivers {
                ui.horizontal(|ui| {
                    ui.add_space(46.0);
                    ui.label(RichText::new(d.name).font(medium(13.5)).color(TEXT));
                    ui.label(RichText::new(d.purpose).font(regular(12.5)).color(TEXT_3));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (text, color) = if d.installed {
                            (tr!("Instalado", "Installed"), OK)
                        } else if self.install_drivers {
                            (tr!("Se instalará", "Will be installed"), ACCENT)
                        } else {
                            (tr!("No se instalará", "Won't be installed"), TEXT_3)
                        };
                        pill(ui, text, color);
                    });
                });
            }
            if self.install_drivers && self.drivers.iter().any(|d| !d.installed && d.service == "VBVoicemeeterVAIOMME") {
                ui.horizontal(|ui| {
                    ui.add_space(46.0);
                    ui.label(RichText::new(tr!("Voicemeeter abrirá su propia ventana: pulsa «Install».", "Voicemeeter opens its own window: press “Install”.")).font(regular(12.0)).color(TEXT_2));
                });
            }
        });
        ui.add_space(4.0);
        card(ui, |ui| {
            toggle_row(ui, icons::POWER, tr!("Iniciar con Windows", "Start with Windows"), tr!("Arranca en la bandeja, sin abrir la ventana", "Starts in the tray, without opening the window"), &mut self.autostart);
            ui.add_space(6.0);
            toggle_row(ui, icons::DESKTOP, tr!("Acceso directo en el escritorio", "Desktop shortcut"), tr!("Además del menú Inicio", "In addition to the Start menu"), &mut self.desktop);
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::FOLDER).font(icon(17.0)).color(TEXT_2));
            ui.label(RichText::new(tr!("Carpeta", "Folder")).font(medium(13.5)).color(TEXT_2));
            ui.add(egui::TextEdit::singleline(&mut self.dir).desired_width(ui.available_width()).margin(vec2(10.0, 7.0)).font(regular(13.5)));
        });
        // Si se borra la carpeta, al salir del campo vuelve la de siempre (si no, «Instalar» se queda
        // desactivado sin que se vea por qué).
        if self.dir.trim().is_empty() && !ui.ctx().memory(|m| m.focused().is_some()) {
            let base = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| r"C:\Program Files".into());
            self.dir = format!(r"{base}\Rust Sonar");
        }

        footer(ui, |ui| {
            let label = if self.upgrade { tr!("Actualizar", "Update") } else { tr!("Instalar", "Install") };
            let ok = !self.dir.trim().is_empty();
            if primary_button_enabled(ui, Some(icons::SHIELD), label, ok).on_hover_text(tr!("Windows pedirá permisos de administrador (editor: kilorito)", "Windows will ask for administrator rights (publisher: kilorito)")).clicked() {
                self.start_install();
            }
            if secondary_button(ui, Some(icons::ARROW_LEFT), tr!("Atrás", "Back")).clicked() {
                self.go(Page::Welcome);
            }
        });
    }

    fn installing(&mut self, ui: &mut Ui) {
        let p = self.progress.lock().unwrap();
        let target = p.fraction();
        let items = p.items.clone();
        let current = p.current().cloned();
        drop(p);
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        self.shown += (target - self.shown) * (dt * 4.0).min(1.0);

        ui.label(RichText::new(if self.upgrade { tr!("Actualizando Rust Sonar", "Updating Rust Sonar") } else { tr!("Instalando Rust Sonar", "Installing Rust Sonar") }).font(semibold(26.0)).color(TEXT));
        let sub = current.as_ref().map(|c| format!("{} · {}", c.label, c.detail)).unwrap_or_else(|| tr!("Preparando…", "Preparing…").into());
        ui.label(RichText::new(sub).font(regular(13.5)).color(TEXT_2));
        ui.add_space(10.0);
        progress_bar(ui, self.shown);
        ui.add_space(14.0);

        for item in &items {
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                match item.state {
                    ItemState::Done => {
                        ui.painter().circle_filled(r.center(), 10.0, ACCENT);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, icons::CHECK, icon(12.0), BG);
                    }
                    ItemState::Running => {
                        // Arco que gira.
                        let t = ui.input(|i| i.time) as f32 * 5.0;
                        ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(2.0, ACCENT.gamma_multiply(0.18)));
                        let pts: Vec<Pos2> = (0..=20).map(|k| {
                            let a = t + k as f32 / 20.0 * 4.0;
                            r.center() + vec2(a.cos(), a.sin()) * 9.0
                        }).collect();
                        ui.painter().add(egui::Shape::line(pts, Stroke::new(2.2, ACCENT)));
                    }
                    ItemState::Pending => {
                        ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.5, BORDER_STRONG));
                    }
                }
                let color = match item.state {
                    ItemState::Pending => TEXT_3,
                    _ => TEXT,
                };
                ui.label(RichText::new(&item.label).font(medium(14.0)).color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(&item.detail).font(regular(12.5)).color(TEXT_3));
                });
            });
        }

        if current.as_ref().is_some_and(|c| c.setup.is_some() && !c.downloading) {
            ui.add_space(10.0);
            notice(ui, icons::SHIELD, ACCENT, tr!("Si Windows pregunta si quieres instalar el software de dispositivo de VB-Audio, pulsa Instalar.", "If Windows asks whether to install VB-Audio's device software, press Install."));
        }
    }

    fn done(&mut self, ui: &mut Ui) {
        let result = self.progress.lock().unwrap().result.clone();
        let log = self.progress.lock().unwrap().log.clone();
        let t = (self.page_since.elapsed().as_secs_f32() / 0.5).min(1.0);
        // Rebote al aparecer.
        let pop = 1.0 + 0.12 * (t * std::f32::consts::PI).sin() * (1.0 - t);
        let ease = 1.0 - (1.0 - t).powi(3);

        let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
        let c = rect.center();
        match &result {
            Some(Ok(_)) => {
                ui.painter().circle_filled(c, 40.0 * ease * pop, ACCENT.gamma_multiply(0.14));
                ui.painter().circle_filled(c, 28.0 * ease * pop, ACCENT);
                ui.painter().text(c, Align2::CENTER_CENTER, icons::CHECK, icon(30.0 * ease), BG);
            }
            _ => {
                ui.painter().circle_filled(c, 40.0 * ease * pop, DANGER.gamma_multiply(0.14));
                ui.painter().text(c, Align2::CENTER_CENTER, icons::WARNING, icon(40.0 * ease), DANGER);
            }
        }
        ui.add_space(6.0);

        match result {
            Some(Ok(restart)) => {
                let (title, text) = if restart {
                    (
                        tr!("Falta un reinicio", "Restart needed"),
                        tr!("Rust Sonar está instalado. Los cables virtuales nuevos se activan al reiniciar el PC; al volver, Rust Sonar se configurará solo.", "Rust Sonar is installed. The new virtual cables become active after a restart; when you're back, Rust Sonar sets itself up."),
                    )
                } else {
                    (
                        if self.upgrade { tr!("¡Actualizado!", "Updated!") } else { tr!("¡Listo!", "All set!") },
                        tr!("Rust Sonar está instalado. Lo tienes en el menú Inicio y, si lo elegiste, arrancará con Windows en la bandeja.", "Rust Sonar is installed. You'll find it in the Start menu and, if you chose so, it starts with Windows in the tray."),
                    )
                };
                ui.label(RichText::new(title).font(semibold(30.0)).color(TEXT));
                ui.label(RichText::new(text).font(regular(14.5)).color(TEXT_2));
                ui.add_space(8.0);
                notice(ui, icons::SLIDERS, ACCENT, tr!("Consejo: en Ajustes, «Ordenar dispositivos de Windows» deja en Discord solo tus dispositivos y los de Rust Sonar.", "Tip: in Settings, “Tidy up Windows devices” leaves only your devices and Rust Sonar's in Discord."));
                footer(ui, |ui| {
                    if restart {
                        if primary_button(ui, Some(icons::POWER), tr!("Reiniciar ahora", "Restart now")).on_hover_text(tr!("Guarda lo que tengas abierto: el PC se reinicia en 5 segundos", "Save your work: the PC restarts in 5 seconds")).clicked() {
                            let _ = std::process::Command::new("shutdown").args(["/r", "/t", "5"]).spawn();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if secondary_button(ui, None, tr!("Más tarde", "Later")).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    } else {
                        if primary_button(ui, Some(icons::ROCKET), tr!("Abrir Rust Sonar", "Open Rust Sonar")).clicked() {
                            let exe = Path::new(self.dir.trim()).join("rust_sonar.exe");
                            let _ = std::process::Command::new(exe).spawn();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if secondary_button(ui, None, tr!("Cerrar", "Close")).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
            }
            Some(Err(msg)) => {
                ui.label(RichText::new(tr!("No se pudo instalar", "Installation failed")).font(semibold(30.0)).color(TEXT));
                ui.label(RichText::new(msg).font(regular(14.5)).color(TEXT_2));
                footer(ui, |ui| {
                    if primary_button(ui, Some(icons::RETRY), tr!("Volver a intentarlo", "Try again")).clicked() {
                        self.drivers = detect_drivers();
                        self.go(Page::Options);
                    }
                    if let Some(log) = log.filter(|l| l.exists()) {
                        if secondary_button(ui, Some(icons::FILE), tr!("Ver registro", "View log")).clicked() {
                            let _ = std::process::Command::new("notepad.exe").arg(log).spawn();
                        }
                    }
                });
            }
            None => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Componentes
// ---------------------------------------------------------------------------------------------

/// Logo: anillo turquesa con disco tenue y tres barras.
fn paint_logo(p: &egui::Painter, c: Pos2, r: f32) {
    let w = r * 0.09;
    p.circle_filled(c, r, ACCENT.gamma_multiply(0.14));
    p.circle_stroke(c, r - w * 0.8, Stroke::new(w * 1.6, ACCENT));
    for (bx, h) in [(-0.36, 0.33), (0.0, 0.58), (0.36, 0.42)] {
        let x = c.x + bx * r;
        p.rect_filled(Rect::from_min_max(pos2(x - w, c.y - h * r), pos2(x + w, c.y + h * r)), CornerRadius::same(w as u8), ACCENT);
    }
}

/// Pie de página con los botones, alineados a la derecha y pegados abajo.
fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        // Fila de alto fijo: si no, los botones se centrarían en todo el hueco que sobra.
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 48.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            add(ui);
        });
    });
}

fn button(ui: &mut Ui, ic: Option<&str>, label: &str, primary: bool, enabled: bool) -> egui::Response {
    let font = semibold(14.0);
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x;
    let w = text_w + 40.0 + if ic.is_some() { 24.0 } else { 0.0 };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 42.0), sense);
    let hovered = resp.hovered() && enabled;
    let lift = ui.ctx().animate_bool(resp.id.with("hover"), hovered);
    let p = ui.painter();
    let (fill, fg, stroke) = if primary {
        let fill = if enabled { lerp(ACCENT, Color32::WHITE, 0.12 * lift) } else { SURFACE_3 };
        (fill, if enabled { BG } else { TEXT_3 }, Stroke::NONE)
    } else {
        (lerp(SURFACE_2, SURFACE_3, lift), TEXT, Stroke::new(1.0, BORDER_STRONG))
    };
    if primary && enabled {
        p.rect_filled(rect.expand(3.0 + 2.0 * lift), CornerRadius::same(14), ACCENT.gamma_multiply(0.10 + 0.08 * lift));
    }
    p.rect_filled(rect, CornerRadius::same(11), fill);
    if stroke != Stroke::NONE {
        p.rect_stroke(rect, CornerRadius::same(11), stroke, StrokeKind::Inside);
    }
    // El icono va después del texto si es una flecha a la derecha, antes en el resto.
    let after = ic == Some(icons::ARROW_RIGHT);
    let mut x = rect.left() + 20.0;
    if let (Some(ic), false) = (ic, after) {
        p.text(pos2(x + 8.0, rect.center().y), Align2::CENTER_CENTER, ic, icon(16.0), fg);
        x += 24.0;
    }
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    if let (Some(ic), true) = (ic, after) {
        p.text(pos2(x + text_w + 14.0 + 3.0 * lift, rect.center().y), Align2::CENTER_CENTER, ic, icon(16.0), fg);
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn primary_button(ui: &mut Ui, ic: Option<&str>, label: &str) -> egui::Response {
    button(ui, ic, label, true, true)
}
fn primary_button_enabled(ui: &mut Ui, ic: Option<&str>, label: &str, enabled: bool) -> egui::Response {
    button(ui, ic, label, true, enabled)
}
fn secondary_button(ui: &mut Ui, ic: Option<&str>, label: &str) -> egui::Response {
    button(ui, ic, label, false, true)
}

fn window_button(ui: &mut Ui, rect: Rect, ic: &str, close: bool, enabled: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with(ic), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = resp.hovered() && enabled;
    if hovered {
        let fill = if close { rgb(0xc4, 0x2b, 0x1c) } else { SURFACE_3 };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }
    let color = if !enabled { BORDER_STRONG } else if hovered && close { Color32::WHITE } else if hovered { TEXT } else { TEXT_2 };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, ic, icon(14.0), color);
    resp
}

fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), lerp(SURFACE_3, ACCENT, t));
    let x = rect.left() + 12.0 + t * (rect.width() - 24.0);
    p.circle_filled(pos2(x, rect.center().y), 8.5, if *on { BG } else { TEXT_2 });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn toggle_row(ui: &mut Ui, ic: &str, title: &str, sub: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        badge(ui, ic, TEXT_2);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(semibold(14.5)).color(TEXT));
            ui.label(RichText::new(sub).font(regular(12.5)).color(TEXT_3));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            toggle(ui, on);
        });
    });
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn badge(ui: &mut Ui, ic: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), color.gamma_multiply(0.14));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, ic, icon(19.0), color);
}

fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), medium(11.5), color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 22.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), color.gamma_multiply(0.14));
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, color);
    p.galley(pos2(rect.left() + 19.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

fn channel_chip(ui: &mut Ui, ic: &str, name: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(name.to_owned(), semibold(13.0), color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 32.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(10), color.gamma_multiply(0.12));
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, color.gamma_multiply(0.35)), StrokeKind::Inside);
    p.text(pos2(rect.left() + 18.0, rect.center().y), Align2::CENTER_CENTER, ic, icon(15.0), color);
    p.galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

fn feature_row(ui: &mut Ui, ic: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        badge(ui, ic, ACCENT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(semibold(14.5)).color(TEXT));
            ui.label(RichText::new(sub).font(regular(12.5)).color(TEXT_3));
        });
    });
    ui.add_space(2.0);
}

fn notice(ui: &mut Ui, ic: &str, color: Color32, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.30)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            let w = ui.available_width();
            ui.set_width(w);
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(ic).font(icon(16.0)).color(color));
                ui.add(egui::Label::new(RichText::new(text).font(regular(12.5)).color(TEXT)).wrap());
            });
        });
}

/// Barra de progreso con degradado y un brillo que la recorre.
fn progress_bar(ui: &mut Ui, frac: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let bar = Rect::from_min_size(pos2(rect.left(), rect.top() + 4.0), vec2(rect.width() - 56.0, 10.0));
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::same(5), SURFACE_3);
    let fill = Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height()));
    if fill.width() > 1.0 {
        p.rect_filled(fill, CornerRadius::same(5), ACCENT);
        // Brillo móvil.
        let t = (ui.input(|i| i.time) as f32 * 0.6).fract();
        let x = fill.left() + (fill.width() + 80.0) * t - 40.0;
        let shine = Rect::from_center_size(pos2(x, fill.center().y), vec2(60.0, fill.height())).intersect(fill);
        if shine.width() > 0.0 {
            p.rect_filled(shine, CornerRadius::same(5), Color32::from_white_alpha(40));
        }
        p.circle_filled(pos2(fill.right(), fill.center().y), 9.0, ACCENT.gamma_multiply(0.18));
    }
    p.text(pos2(rect.right(), bar.center().y), Align2::RIGHT_CENTER, format!("{:.0}%", frac * 100.0), semibold(15.0), TEXT);
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Icono de la ventana (el mismo dibujo que el de la app).
fn window_icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    let bars = [(-11.0f32, 10.0f32), (0.0, 18.0), (11.0, 13.0)];
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let ring = (1.0 - ((d - 28.5).abs() - 2.5).clamp(0.0, 1.0)).max(0.0);
            let disc = (31.5 - d).clamp(0.0, 1.0) * 0.18;
            let bar = bars
                .iter()
                .map(|&(bx, h)| (1.0 - ((dx - bx).abs() - 2.5).clamp(0.0, 1.0)) * (1.0 - (dy.abs() - h).clamp(0.0, 1.0)))
                .fold(0.0f32, f32::max);
            let i = (y * N + x) * 4;
            rgba[i..i + 3].copy_from_slice(&[0x2d, 0xd4, 0xbf]);
            rgba[i + 3] = (ring.max(bar).max(disc) * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: N as u32, height: N as u32 }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr!("Instalar Rust Sonar", "Install Rust Sonar"))
            .with_inner_size([880.0, 600.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_decorations(false)
            .with_icon(window_icon()),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("Instalar Rust Sonar", options, Box::new(|cc| Ok(Box::new(Setup::new(cc)))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn driver_store_is_readable_and_prefixes_match() {
        // Prefijo vacío: cualquier carpeta vale, así que solo comprueba que el almacén se puede leer.
        assert!(super::in_driver_store(""));
        assert!(!super::in_driver_store("rust-sonar-no-existe"));
    }
}
