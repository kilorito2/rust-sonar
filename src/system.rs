//! Integración con el escritorio de Windows: instancia única, ventana principal (mostrar/ocultar),
//! inicio automático con la sesión y cierre de programas en conflicto.

use anyhow::{Context, Result, bail};
use std::os::windows::process::CommandExt;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, PostMessageW, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow, WM_CLOSE,
};
use windows::core::{HSTRING, PCWSTR, w};

/// Título de la ventana principal (también sirve para encontrarla desde otra instancia).
pub const WINDOW_TITLE: &str = "Rust Sonar";
/// Argumento con el que arranca escondido en la bandeja (lo usa el inicio automático).
pub const MINIMIZED_FLAG: &str = "--minimized";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Argumento con el que se abre solo la ventana (proceso aparte del motor).
pub const UI_FLAG: &str = "--ui";
const ENGINE_MUTEX: PCWSTR = w!("Local\\RustSonarSingleInstance");

/// Devuelve false si ya hay otro motor de Rust Sonar abierto.
/// Dos instancias se pelearían por el enrutado y sonaría todo doble.
pub fn single_instance() -> bool {
    named_mutex(ENGINE_MUTEX)
}

/// Devuelve false si ya hay una ventana de Rust Sonar abierta (en otro proceso).
pub fn single_ui_instance() -> bool {
    named_mutex(w!("Local\\RustSonarUI"))
}

fn named_mutex(name: PCWSTR) -> bool {
    unsafe {
        // El mutex se queda abierto hasta que termina el proceso (a propósito, no se cierra).
        let created = CreateMutexW(None, true, name);
        !(created.is_ok() && GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

/// ¿Está el motor en marcha (en otro proceso)?
pub fn engine_running() -> bool {
    unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, ENGINE_MUTEX).is_ok() }
}

/// Muestra la ventana; si no está abierta, la abre como proceso aparte. La interfaz solo existe
/// mientras se usa: cerrada, el motor de la bandeja no carga gráficos ni ocupa memoria de GPU.
pub fn open_ui() {
    if show_main_window() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(UI_FLAG).spawn();
    }
}

fn main_window() -> Option<HWND> {
    unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(WINDOW_TITLE)).ok().filter(|h| !h.is_invalid()) }
}

/// Trae la ventana al frente. false si no hay ventana abierta.
pub fn show_main_window() -> bool {
    unsafe {
        let Some(hwnd) = main_window() else { return false };
        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(hwnd);
        true
    }
}

/// Pide a la ventana principal que se cierre (como pulsar la X).
pub fn close_main_window() {
    unsafe {
        if let Some(hwnd) = main_window() {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Inicio automático (HKCU\...\Run: no necesita administrador)
// ---------------------------------------------------------------------------------------------

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("RustSonar");

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key).ok()? };
    Ok(key)
}

/// Línea de comandos que se registra: este ejecutable, arrancando en la bandeja.
fn autostart_command() -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" {MINIMIZED_FLAG}", exe.display()))
}

/// ¿Está registrado el inicio automático apuntando a este ejecutable?
pub fn autostart_enabled() -> bool {
    let Ok(key) = open_run_key(KEY_READ) else { return false };
    let mut buf = [0u16; 1024];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let ok = unsafe {
        RegQueryValueExW(key, RUN_VALUE, None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut size)).is_ok()
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if !ok {
        return false;
    }
    let len = (size as usize / 2).min(buf.len());
    let value = String::from_utf16_lossy(&buf[..len]);
    let value = value.trim_end_matches('\0');
    autostart_command().map(|cmd| cmd.eq_ignore_ascii_case(value)).unwrap_or(false)
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let key = open_run_key(KEY_SET_VALUE)?;
    let result = unsafe {
        if enabled {
            let wide: Vec<u16> = autostart_command()?.encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)).ok()
        } else {
            // Que no existiera ya no es un error.
            let _ = RegDeleteValueW(key, RUN_VALUE);
            Ok(())
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result.context(tr!("no se pudo cambiar el inicio automático", "couldn't change start with Windows"))
}

// ---------------------------------------------------------------------------------------------
// Programas en conflicto
// ---------------------------------------------------------------------------------------------

/// Cierra SteelSeries GG y sus procesos (Sonar, Engine…). Mientras están abiertos capturan los
/// mismos dispositivos virtuales y el audio sale doble.
pub fn close_steelseries() -> Result<String> {
    let images = [
        "SteelSeriesGG.exe",
        "SteelSeriesGGClient.exe",
        "SteelSeriesSonar.exe",
        "SteelSeriesEngine.exe",
        "SteelSeriesPrism.exe",
        "SteelSeriesGGEZ.exe",
    ];
    for image in images {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/IM", image])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    if crate::audio::wasapi::process_running(&crate::virtual_devices::STEELSERIES_PROCESSES) {
        bail!(trf!("no se pudo cerrar SteelSeries GG; ciérralo desde su icono de la bandeja", "couldn't close SteelSeries GG; close it from its tray icon"));
    }
    Ok(tr!("SteelSeries GG cerrado. Para que no vuelva a abrirse solo, desactiva su inicio con Windows.", "SteelSeries GG closed. To stop it reopening by itself, turn off its start with Windows.").into())
}
