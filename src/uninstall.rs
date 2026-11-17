//! «Desinstalar» (Ajustes): quita Rust Sonar y los drivers de VB-Audio que instala (VB-CABLE,
//! Hi-Fi Cable y Voicemeeter) sin dejar rastro, para que al volver a instalarlo empiece de cero.
//!
//! La ventana, sin permisos, copia el ejecutable a %TEMP% y lanza esa copia como administrador con
//! `--uninstall-all` (un aviso de Windows). Trabajar desde la copia permite borrar la carpeta de
//! instalación (el ejecutable original ya no está en uso) y que el desinstalador de Inno, que cierra
//! `rust_sonar.exe` por nombre, no la cierre a ella. La copia se borra sola al reiniciar.
//!
//! Qué se quita de cada driver: sus dispositivos (también los fantasma), su paquete del almacén de
//! drivers, sus servicios y claves de ajustes, los .sys sueltos, sus carpetas en Archivos de
//! programa, su entrada en «Aplicaciones», sus claves de registro y sus accesos directos.

use anyhow::{Context, Result, bail};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DICS_FLAG_GLOBAL, DIREG_DRV, DiUninstallDevice, GUID_DEVCLASS_MEDIA, HDEVINFO, SETUP_DI_GET_CLASS_DEVS_FLAGS,
    SETUP_DI_REGISTRY_PROPERTY, SP_DEVINFO_DATA, SPDRP_FRIENDLYNAME, SPDRP_SERVICE, SUOI_FORCEDELETE,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
    SetupDiOpenDevRegKey, SetupGetInfDriverStoreLocationW, SetupUninstallOEMInfW,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND};
use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegDeleteTreeW, RegEnumKeyExW, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::core::{BOOL, GUID, HSTRING, PCWSTR, PWSTR};

/// Argumento de la copia elevada que hace la desinstalación. Le sigue la carpeta APPDATA del
/// usuario (la de su configuración), por si el administrador que acepta el aviso es otra cuenta.
pub const UNINSTALL_ALL_FLAG: &str = "--uninstall-all";

/// AppId del instalador de Inno (RustSonar.iss) + "_is1": su entrada en «Aplicaciones».
const INNO_KEY: &str = "{6E0B5A43-2C1D-4C59-9C1E-5B7A3F0D8E21}_is1";
const UNINSTALL_KEYS: [&str; 2] = [
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
];
/// Clase de los extremos de audio («Entradas y salidas de audio» en el Administrador de dispositivos).
const GUID_DEVCLASS_AUDIOENDPOINT: GUID = GUID::from_u128(0xc166523c_fe0c_4a94_a586_f1a80cfbbf3e);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Un driver de VB-Audio de los que instala Rust Sonar y todo lo que deja en el sistema.
pub struct Driver {
    pub name: &'static str,
    /// Comienzo del nombre original del .inf (en minúsculas), p. ej. "vbmmecable64_win10.inf".
    inf_prefixes: &'static [&'static str],
    /// Servicios del driver y claves de ajustes que guarda junto a ellos en Services.
    services: &'static [&'static str],
    /// Final del nombre de sus dispositivos de audio: "CABLE Input (VB-Audio Virtual Cable)".
    endpoint_tag: &'static str,
    /// Comienzo de los .sys que algunos instaladores copian sueltos en System32\drivers.
    sys_prefixes: &'static [&'static str],
    /// Carpetas: (variable de entorno, subcarpeta).
    dirs: &'static [(&'static str, &'static str)],
    /// Comienzo del nombre de su entrada en «Aplicaciones» (Uninstall).
    uninstall_entries: &'static [&'static str],
    /// Subclaves de HKLM\SOFTWARE\VB-Audio (y su gemela de 32 bits).
    software_keys: &'static [&'static str],
    /// Subcarpetas de «VB Audio» en el menú Inicio.
    start_menu: &'static [&'static str],
}

pub static DRIVERS: [Driver; 3] = [
    Driver {
        name: "VB-CABLE",
        // Solo el cable principal: VB-CABLE A+B y C+D (vbMmeCable64A_…) no los instala Rust Sonar.
        inf_prefixes: &["vbmmecable64_", "vbmmecable_"],
        services: &["VBAudioVACMME", "VB-Cable"],
        endpoint_tag: "(vb-audio virtual cable)",
        sys_prefixes: &["vbaudio_cable"],
        dirs: &[("ProgramFiles", "VB\\CABLE")],
        uninstall_entries: &["VB:VBCABLE"],
        software_keys: &["Cable"],
        start_menu: &[],
    },
    Driver {
        name: "Hi-Fi Cable",
        inf_prefixes: &["vbhfvaio"],
        services: &["VBAudioHFVAIOMME"],
        endpoint_tag: "(vb-audio hi-fi cable)",
        sys_prefixes: &["vbaudio_hfvaio"],
        dirs: &[("ProgramFiles", "VB\\CABLEHiFi"), ("ProgramFiles(x86)", "VB\\ASIOBridge")],
        uninstall_entries: &["VB:ASIOBridge"],
        software_keys: &[],
        start_menu: &["ASIO Bridge"],
    },
    Driver {
        name: "Voicemeeter",
        // Potato 8 usa un solo driver (vbvoicemeetervaio); las versiones antiguas, uno por bus.
        inf_prefixes: &["vbvoicemeetervaio", "vbvmvaio", "vbvmauxvaio"],
        services: &["VBVoicemeeterVAIOMME", "VBAudioVMVAIOMME", "VBAudioVMAUXVMME", "VBAudioVMVAIO3MME", "VB-MultiCable"],
        endpoint_tag: "(vb-audio voicemeeter",
        sys_prefixes: &["vbvoicemeetervaio", "vbaudio_vmvaio", "vbaudio_vmauxvaio"],
        dirs: &[("ProgramFiles", "VB\\VBVoicemeeterVAIOs"), ("ProgramFiles(x86)", "VB\\Voicemeeter")],
        uninstall_entries: &["VB:Voicemeeter"],
        software_keys: &["MultiCable", "Voicemeeter"],
        start_menu: &["VoiceMeeter"],
    },
];

// ---------------------------------------------------------------------------------------------
// Lanzamiento (desde la ventana, sin permisos)
// ---------------------------------------------------------------------------------------------

/// Copia el ejecutable a %TEMP% y lo lanza como administrador para desinstalar. Vuelve en cuanto
/// se acepta el aviso de Windows (o con error si se cancela).
pub fn launch() -> Result<()> {
    use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let exe = std::env::current_exe()?;
    let dir = std::env::temp_dir().join("RustSonar-Desinstalar");
    std::fs::create_dir_all(&dir)?;
    let copy = dir.join("RustSonar-Desinstalar.exe");
    std::fs::copy(&exe, &copy).context(tr!("no se pudo preparar el desinstalador", "couldn't prepare the uninstaller"))?;
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    if appdata.contains('"') {
        bail!("APPDATA no válido");
    }
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(copy.as_os_str());
    let params = HSTRING::from(format!("{UNINSTALL_ALL_FLAG} \"{appdata}\""));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.context(tr!("cancelado o sin permisos de administrador", "cancelled or no administrator rights"))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Pasos (en la copia elevada)
// ---------------------------------------------------------------------------------------------

/// Resultado de un paso: texto para la ventana y si hace falta reiniciar para terminarlo.
pub struct StepResult {
    pub detail: String,
    /// Algo falló (el detalle dice qué).
    pub warn: bool,
    /// false: no había nada que quitar.
    pub did_something: bool,
    pub reboot: bool,
}

/// 1. Cierra Rust Sonar (motor y ventana): primero por su API, para que devuelva los
///    predeterminados de Windows y el enrutado de las apps; si no responde, a la fuerza.
pub fn close_rust_sonar(port: u16) -> StepResult {
    let was_running = crate::system::engine_running();
    if was_running {
        crate::ui::client::quit_engine(port);
        let start = Instant::now();
        while crate::system::engine_running() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let killed = kill_processes(|name, _| name.starts_with("rust_sonar"));
    StepResult {
        detail: if was_running || killed > 0 { tr!("Cerrado", "Closed").into() } else { tr!("No estaba abierto", "It wasn't running").into() },
        warn: false,
        did_something: true,
        reboot: false,
    }
}

/// 2. Dispositivos visibles con su nombre de fábrica, predeterminados en los reales, sin salidas
///    forzadas por app y sin inicio automático.
pub fn restore_windows() -> StepResult {
    let errors = crate::virtual_devices::uninstall_cleanup();
    StepResult {
        detail: if errors == 0 {
            tr!("Nombres, visibilidad y predeterminados como antes", "Names, visibility and defaults as before").into()
        } else {
            trf!("{errors} cambios no se pudieron deshacer", "{errors} changes couldn't be undone")
        },
        warn: errors > 0,
        did_something: true,
        reboot: false,
    }
}

/// 3. Quita un driver de VB-Audio y todo lo que dejó.
pub fn remove_driver(d: &Driver) -> StepResult {
    let mut reboot = false;
    let mut errors: Vec<String> = Vec::new();

    // Sus programas abiertos (Voicemeeter, paneles de control) bloquearían los archivos.
    let dirs: Vec<PathBuf> = d.dirs.iter().filter_map(|(var, sub)| std::env::var_os(var).map(|b| PathBuf::from(b).join(sub))).collect();
    let dir_prefixes: Vec<String> = dirs.iter().map(|p| format!("{}\\", p.display()).to_lowercase()).collect();
    kill_processes(|_, path| dir_prefixes.iter().any(|p| path.starts_with(p.as_str())));

    // Paquetes del almacén de drivers (oemNN.inf) de este driver.
    let packages = driver_packages(d);
    let package_names: Vec<String> = packages.iter().map(|p| p.to_lowercase()).collect();

    // Dispositivos: el del driver (raíz MEDIA) y luego sus extremos de audio, que se quedan como
    // fantasmas al quitarlo.
    let mut devices = 0;
    match remove_devices(&GUID_DEVCLASS_MEDIA, |set, data| {
        let service = device_string(set, data, SPDRP_SERVICE).unwrap_or_default();
        let inf = device_inf(set, data).unwrap_or_default().to_lowercase();
        d.services.iter().any(|s| s.eq_ignore_ascii_case(&service)) || package_names.contains(&inf)
    }) {
        Ok((n, r)) => {
            devices += n;
            reboot |= r;
        }
        Err(e) => errors.push(format!("{e:#}")),
    }
    for inf in &packages {
        let ok = unsafe { SetupUninstallOEMInfW(&HSTRING::from(inf.as_str()), SUOI_FORCEDELETE, None) }.as_bool();
        if !ok && !pnputil_delete(inf) {
            errors.push(trf!("no se pudo borrar el paquete {inf}", "couldn't delete package {inf}"));
        }
    }
    let mut endpoints = 0;
    if let Ok((n, r)) = remove_devices(&GUID_DEVCLASS_AUDIOENDPOINT, |set, data| {
        device_string(set, data, SPDRP_FRIENDLYNAME).is_some_and(|name| name.to_lowercase().contains(d.endpoint_tag))
    }) {
        endpoints = n;
        reboot |= r;
    }

    // Servicios y claves de ajustes.
    let mut services = 0;
    for s in d.services {
        match delete_service(s) {
            ServiceDeleted::Yes => services += 1,
            ServiceDeleted::Pending => {
                services += 1;
                reboot = true;
            }
            ServiceDeleted::NotFound => {}
        }
    }

    // Archivos: .sys sueltos y carpetas del programa.
    let mut files = 0;
    if let Some(windir) = std::env::var_os("SystemRoot") {
        let drivers_dir = PathBuf::from(windir).join("System32").join("drivers");
        if let Ok(entries) = std::fs::read_dir(&drivers_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.ends_with(".sys") && d.sys_prefixes.iter().any(|p| name.starts_with(p)) {
                    files += 1;
                    reboot |= delete_path(&e.path());
                }
            }
        }
    }
    for dir in &dirs {
        if dir.exists() {
            files += 1;
            reboot |= delete_path(dir);
            remove_dir_if_empty(dir.parent());
        }
    }

    // Registro: entrada en «Aplicaciones», ajustes en SOFTWARE y drivers ASIO de Voicemeeter.
    let mut keys = 0;
    for base in UNINSTALL_KEYS {
        for sub in reg_subkeys(HKEY_LOCAL_MACHINE, base) {
            if d.uninstall_entries.iter().any(|p| sub.starts_with(p)) && reg_delete_tree(HKEY_LOCAL_MACHINE, &format!("{base}\\{sub}")) {
                keys += 1;
            }
        }
    }
    for root in ["SOFTWARE\\VB-Audio", "SOFTWARE\\WOW6432Node\\VB-Audio"] {
        for sub in d.software_keys {
            keys += reg_delete_tree(HKEY_LOCAL_MACHINE, &format!("{root}\\{sub}")) as usize;
        }
        if reg_subkeys(HKEY_LOCAL_MACHINE, root).is_empty() {
            reg_delete_tree(HKEY_LOCAL_MACHINE, root);
        }
    }
    for sub in d.software_keys {
        keys += reg_delete_tree(HKEY_CURRENT_USER, &format!("Software\\VB-Audio\\{sub}")) as usize;
    }
    if reg_subkeys(HKEY_CURRENT_USER, "Software\\VB-Audio").is_empty() {
        reg_delete_tree(HKEY_CURRENT_USER, "Software\\VB-Audio");
    }
    if d.name == "Voicemeeter" {
        keys += remove_asio_drivers(&["voicemeeter", "vb-audio"]);
    }

    // Accesos directos.
    if let Some(data) = std::env::var_os("ProgramData") {
        let vb = PathBuf::from(data).join("Microsoft\\Windows\\Start Menu\\Programs\\VB Audio");
        for sub in d.start_menu {
            let p = vb.join(sub);
            if p.exists() {
                delete_path(&p);
            }
        }
        remove_dir_if_empty(Some(&vb));
    }
    if d.name == "Voicemeeter" {
        for desktop in [std::env::var_os("PUBLIC").map(|p| PathBuf::from(p).join("Desktop")), std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop"))]
            .into_iter()
            .flatten()
        {
            if let Ok(entries) = std::fs::read_dir(&desktop) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("voicemeeter") && name.ends_with(".lnk") {
                        delete_path(&e.path());
                    }
                }
            }
        }
    }

    let found = devices + packages.len() + endpoints + services + files + keys;
    let mut detail = if found == 0 {
        tr!("No estaba instalado", "It wasn't installed").to_string()
    } else {
        trf!(
            "{} dispositivos, {} paquetes de driver, {} servicios, {} carpetas o archivos, {} claves de registro",
            "{} devices, {} driver packages, {} services, {} folders or files, {} registry keys",
            devices + endpoints,
            packages.len(),
            services,
            files,
            keys
        )
    };
    if !errors.is_empty() {
        detail = format!("{detail}. {}", errors.join("; "));
    }
    StepResult { detail, warn: !errors.is_empty(), did_something: found > 0, reboot: reboot || devices > 0 }
}

/// 4. Ejecuta el desinstalador de Inno (programa, menú Inicio, entrada en «Aplicaciones») si
///    Rust Sonar está instalado, y borra lo que quede de su carpeta.
pub fn remove_app() -> StepResult {
    let Some((base, uninstaller)) =
        UNINSTALL_KEYS.iter().find_map(|base| reg_string(HKEY_LOCAL_MACHINE, &format!("{base}\\{INNO_KEY}"), "UninstallString").map(|u| (*base, u)))
    else {
        return StepResult { detail: tr!("No estaba instalado", "It wasn't installed").into(), warn: false, did_something: false, reboot: false };
    };
    let key = format!("{base}\\{INNO_KEY}");
    let location = reg_string(HKEY_LOCAL_MACHINE, &key, "InstallLocation");
    let exe = uninstaller.trim().trim_matches('"').to_string();
    let ran = std::process::Command::new(&exe)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok();
    // El desinstalador de Inno se copia a %TEMP% y sigue desde ahí: se espera a que desaparezca su
    // entrada en «Aplicaciones».
    let start = Instant::now();
    while ran && reg_string(HKEY_LOCAL_MACHINE, &key, "UninstallString").is_some() && start.elapsed() < Duration::from_secs(90) {
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut reboot = false;
    if let Some(dir) = location.map(PathBuf::from).filter(|d| d.exists()) {
        reboot |= delete_path(&dir);
    }
    if reg_string(HKEY_LOCAL_MACHINE, &key, "UninstallString").is_some() {
        // No terminó: se quita al menos la entrada para que no quede colgada en «Aplicaciones».
        reg_delete_tree(HKEY_LOCAL_MACHINE, &key);
        return StepResult {
            detail: tr!("El desinstalador no terminó; se borraron sus archivos a mano", "The uninstaller didn't finish; its files were deleted by hand").into(),
            warn: true,
            did_something: true,
            reboot,
        };
    }
    StepResult { detail: tr!("Programa, accesos directos y entrada en «Aplicaciones»", "Program, shortcuts and “Apps” entry").into(), warn: false, did_something: true, reboot }
}

/// 5. Borra la configuración (%APPDATA%\RustSonar) y el inicio automático.
pub fn remove_settings() -> StepResult {
    let _ = crate::system::set_autostart(false);
    let dir = crate::config::Settings::path().parent().map(Path::to_path_buf);
    match dir.filter(|d| d.exists()) {
        Some(d) => StepResult { detail: d.display().to_string(), warn: false, did_something: true, reboot: delete_path(&d) },
        None => StepResult { detail: tr!("No había configuración", "There were no settings").into(), warn: false, did_something: false, reboot: false },
    }
}

/// Al cerrar la ventana: la copia de %TEMP% se borra en el próximo reinicio.
pub fn schedule_self_delete() {
    if let Ok(exe) = std::env::current_exe() {
        delay_delete(&exe);
        if let Some(dir) = exe.parent() {
            delay_delete(dir);
        }
    }
}

pub fn restart_windows() {
    let _ = std::process::Command::new("shutdown").args(["/r", "/t", "0"]).creation_flags(CREATE_NO_WINDOW).spawn();
}

// ---------------------------------------------------------------------------------------------
// Dispositivos y paquetes de drivers (SetupAPI)
// ---------------------------------------------------------------------------------------------

/// Nombres publicados (oemNN.inf) de los paquetes de este driver en el almacén de drivers.
fn driver_packages(d: &Driver) -> Vec<String> {
    let Some(windir) = std::env::var_os("SystemRoot") else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(PathBuf::from(windir).join("INF")) else { return Vec::new() };
    let mut found = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if !(lower.starts_with("oem") && lower.ends_with(".inf")) {
            continue;
        }
        let mut buf = [0u16; 1024];
        let path = HSTRING::from(e.path().as_os_str());
        if unsafe { SetupGetInfDriverStoreLocationW(&path, None, PCWSTR::null(), &mut buf, None) }.is_err() {
            continue;
        }
        let store = String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())]);
        if matches_inf(d, &store) {
            found.push(name);
        }
    }
    found
}

/// ¿El .inf del almacén (ruta completa) es de este driver? Se mira el nombre original del archivo.
fn matches_inf(d: &Driver, store_path: &str) -> bool {
    let file = store_path.rsplit(['\\', '/']).next().unwrap_or(store_path).to_lowercase();
    d.inf_prefixes.iter().any(|p| file.starts_with(p))
}

/// Desinstala los dispositivos de una clase (presentes o no) que cumplan `select`.
/// Devuelve cuántos y si Windows pide reiniciar.
fn remove_devices(class: &GUID, select: impl Fn(HDEVINFO, &SP_DEVINFO_DATA) -> bool) -> Result<(usize, bool)> {
    // Sin DIGCF_PRESENT: también los dispositivos fantasma (desconectados o de drivers ya quitados).
    let set = unsafe { SetupDiGetClassDevsW(Some(class), PCWSTR::null(), None, SETUP_DI_GET_CLASS_DEVS_FLAGS(0)) }?;
    let mut count = 0;
    let mut reboot = false;
    let mut i = 0;
    loop {
        let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        if unsafe { SetupDiEnumDeviceInfo(set, i, &mut data) }.is_err() {
            break;
        }
        i += 1;
        if !select(set, &data) {
            continue;
        }
        let mut need = BOOL(0);
        if unsafe { DiUninstallDevice(HWND::default(), set, &data, 0, Some(&mut need)) }.is_ok() {
            count += 1;
            reboot |= need.as_bool();
        }
    }
    unsafe {
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    Ok((count, reboot))
}

fn device_string(set: HDEVINFO, data: &SP_DEVINFO_DATA, prop: SETUP_DI_REGISTRY_PROPERTY) -> Option<String> {
    let mut buf = [0u8; 2048];
    unsafe { SetupDiGetDeviceRegistryPropertyW(set, data, prop, None, Some(&mut buf), None) }.ok()?;
    Some(utf16_bytes(&buf))
}

/// Paquete de driver (oemNN.inf) instalado en el dispositivo, de su clave de driver.
fn device_inf(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Option<String> {
    let key = unsafe { SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0) }.ok()?;
    let value = query_string(key, "InfPath");
    unsafe {
        let _ = RegCloseKey(key);
    }
    value
}

/// Último recurso para borrar un paquete del almacén: la herramienta de Windows.
fn pnputil_delete(inf: &str) -> bool {
    std::process::Command::new("pnputil")
        .args(["/delete-driver", inf, "/uninstall", "/force"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success() || s.code() == Some(3010))
}

enum ServiceDeleted {
    Yes,
    /// Marcado para borrar: se va al reiniciar (el driver aún estaba cargado).
    Pending,
    NotFound,
}

/// Borra un servicio con `sc delete`. Si no es un servicio (claves de ajustes que VB-Audio guarda
/// en Services, como «VB-Cable»), borra la clave directamente.
fn delete_service(name: &str) -> ServiceDeleted {
    let key = format!("SYSTEM\\CurrentControlSet\\Services\\{name}");
    if hklm_key_exists(&key).is_none() {
        return ServiceDeleted::NotFound;
    }
    let is_service = reg_string(HKEY_LOCAL_MACHINE, &key, "ImagePath").is_some() || reg_dword_exists(&key, "Type");
    if !is_service {
        return if reg_delete_tree(HKEY_LOCAL_MACHINE, &key) { ServiceDeleted::Yes } else { ServiceDeleted::NotFound };
    }
    let code = std::process::Command::new("sc").args(["delete", name]).creation_flags(CREATE_NO_WINDOW).status().ok().and_then(|s| s.code());
    match code {
        Some(0) if hklm_key_exists(&key).is_none() => ServiceDeleted::Yes,
        Some(0) | Some(1072) => ServiceDeleted::Pending, // 1072: ya estaba marcado para borrar
        _ => {
            // Servicio a medio borrar (sin driver): la clave sobra.
            if reg_delete_tree(HKEY_LOCAL_MACHINE, &key) { ServiceDeleted::Yes } else { ServiceDeleted::Pending }
        }
    }
}

/// Controladores ASIO registrados (HKLM\SOFTWARE\ASIO) cuyo nombre contiene alguno de `names`, con su CLSID.
fn remove_asio_drivers(names: &[&str]) -> usize {
    let mut removed = 0;
    for (asio, classes) in [("SOFTWARE\\ASIO", "SOFTWARE\\Classes\\CLSID"), ("SOFTWARE\\WOW6432Node\\ASIO", "SOFTWARE\\Classes\\WOW6432Node\\CLSID")] {
        for sub in reg_subkeys(HKEY_LOCAL_MACHINE, asio) {
            let lower = sub.to_lowercase();
            if !names.iter().any(|n| lower.contains(n)) {
                continue;
            }
            let key = format!("{asio}\\{sub}");
            if let Some(clsid) = reg_string(HKEY_LOCAL_MACHINE, &key, "CLSID").filter(|c| c.starts_with('{') && !c.contains('\\')) {
                reg_delete_tree(HKEY_LOCAL_MACHINE, &format!("{classes}\\{clsid}"));
            }
            removed += reg_delete_tree(HKEY_LOCAL_MACHINE, &key) as usize;
        }
    }
    removed
}

// ---------------------------------------------------------------------------------------------
// Procesos
// ---------------------------------------------------------------------------------------------

/// Termina los procesos (salvo este) para los que `select(nombre, ruta)` es true; ambos en minúsculas.
fn kill_processes(select: impl Fn(&str, &str) -> bool) -> usize {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
        TerminateProcess, WaitForSingleObject,
    };
    let me = std::process::id();
    let mut killed = 0;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return 0 };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            if entry.th32ProcessID != me && entry.th32ProcessID != 0 {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, false, entry.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    let path = if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok() {
                        String::from_utf16_lossy(&buf[..size as usize]).to_lowercase()
                    } else {
                        String::new()
                    };
                    if select(&name, &path) && TerminateProcess(h, 1).is_ok() {
                        WaitForSingleObject(h, 3000);
                        killed += 1;
                    }
                    let _ = CloseHandle(h);
                }
            }
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    killed
}

// ---------------------------------------------------------------------------------------------
// Archivos
// ---------------------------------------------------------------------------------------------

/// Borra un archivo o carpeta. Lo que está en uso se borra al reiniciar: devuelve true si quedó algo
/// pendiente.
fn delete_path(path: &Path) -> bool {
    let ok = if path.is_dir() { std::fs::remove_dir_all(path).is_ok() } else { std::fs::remove_file(path).is_ok() };
    if ok || !path.exists() {
        return false;
    }
    // Primero el contenido (en orden: los archivos antes que su carpeta) y después la carpeta.
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for e in entries.flatten() {
                delete_path(&e.path());
            }
        }
        let _ = std::fs::remove_dir(path);
    }
    if path.exists() {
        delay_delete(path);
        return true;
    }
    false
}

fn delay_delete(path: &Path) {
    unsafe {
        let _ = MoveFileExW(&HSTRING::from(path.as_os_str()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
    }
}

fn remove_dir_if_empty(dir: Option<&Path>) {
    if let Some(dir) = dir {
        if std::fs::read_dir(dir).is_ok_and(|mut e| e.next().is_none()) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Registro
// ---------------------------------------------------------------------------------------------

fn open_key(root: HKEY, path: &str) -> Option<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &mut key) }.ok().ok()?;
    Some(key)
}

/// Borra la clave con todo lo que tiene. true si existía y se borró.
fn reg_delete_tree(root: HKEY, path: &str) -> bool {
    unsafe { RegDeleteTreeW(root, &HSTRING::from(path)) == ERROR_SUCCESS }
}

fn reg_subkeys(root: HKEY, path: &str) -> Vec<String> {
    let Some(key) = open_key(root, path) else { return Vec::new() };
    let mut names = Vec::new();
    let mut i = 0;
    loop {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        if unsafe { RegEnumKeyExW(key, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) } != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
        i += 1;
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    names
}

fn reg_string(root: HKEY, path: &str, value: &str) -> Option<String> {
    let key = open_key(root, path)?;
    let v = query_string(key, value);
    unsafe {
        let _ = RegCloseKey(key);
    }
    v
}

fn query_string(key: HKEY, value: &str) -> Option<String> {
    let mut buf = [0u8; 4096];
    let mut size = buf.len() as u32;
    unsafe { RegQueryValueExW(key, &HSTRING::from(value), None, None, Some(buf.as_mut_ptr()), Some(&mut size)) }.ok().ok()?;
    Some(utf16_bytes(&buf[..size as usize])).filter(|s| !s.is_empty())
}

fn reg_dword_exists(path: &str, value: &str) -> bool {
    let Some(key) = open_key(HKEY_LOCAL_MACHINE, path) else { return false };
    let mut size = 0u32;
    let ok = unsafe { RegQueryValueExW(key, &HSTRING::from(value), None, None, None, Some(&mut size)) }.is_ok();
    unsafe {
        let _ = RegCloseKey(key);
    }
    ok
}

/// Some(()) si la clave de HKLM existe.
fn hklm_key_exists(path: &str) -> Option<()> {
    let key = open_key(HKEY_LOCAL_MACHINE, path)?;
    unsafe {
        let _ = RegCloseKey(key);
    }
    Some(())
}

/// Texto UTF-16 (bytes en little endian) hasta el primer nulo.
fn utf16_bytes(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    String::from_utf16_lossy(&wide)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inf_names_map_to_their_driver() {
        let store = |f: &str| format!("C:\\Windows\\System32\\DriverStore\\FileRepository\\{f}_amd64_9ed1e8995f3f9e21\\{f}");
        let owner = |f: &str| DRIVERS.iter().find(|d| matches_inf(d, &store(f))).map(|d| d.name);
        assert_eq!(owner("vbmmecable64_win10.inf"), Some("VB-CABLE"));
        assert_eq!(owner("VBhfvaio64_win7.inf"), Some("Hi-Fi Cable"));
        assert_eq!(owner("vbvoicemeetervaio64_win10.inf"), Some("Voicemeeter"));
        assert_eq!(owner("VBvmauxvaio64_win10.inf"), Some("Voicemeeter"));
        // Otros drivers (también otros de VB-Audio, como VB-CABLE A+B) no se tocan.
        assert_eq!(owner("vbMmeCable64A_win10.inf"), None);
        assert_eq!(owner("nvhda64v.inf"), None);
        assert_eq!(owner("vbaudio_vbmatrix.inf"), None);
    }

    #[test]
    fn endpoint_tags_match_renamed_and_original_names() {
        let owner = |name: &str| DRIVERS.iter().find(|d| name.to_lowercase().contains(d.endpoint_tag)).map(|d| d.name);
        assert_eq!(owner("CABLE Input (VB-Audio Virtual Cable)"), Some("VB-CABLE"));
        assert_eq!(owner("Rust Sonar - Juego (VB-Audio Hi-Fi Cable)"), Some("Hi-Fi Cable"));
        assert_eq!(owner("Voicemeeter Out B1 (VB-Audio Voicemeeter VAIO)"), Some("Voicemeeter"));
        assert_eq!(owner("VoiceMeeter Aux Input (VB-Audio VoiceMeeter AUX VAIO)"), Some("Voicemeeter"));
        assert_eq!(owner("Altavoces (High Definition Audio Device)"), None);
    }

    #[test]
    fn registry_tree_is_deleted_with_its_subkeys() {
        use windows::Win32::System::Registry::{KEY_WRITE, REG_OPTION_NON_VOLATILE, RegCreateKeyExW};
        let path = "Software\\RustSonarTestUninstall\\Hijo\\Nieto";
        let mut key = HKEY::default();
        unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None).ok().unwrap();
            let _ = RegCloseKey(key);
        }
        assert_eq!(reg_subkeys(HKEY_CURRENT_USER, "Software\\RustSonarTestUninstall"), ["Hijo"]);
        assert!(reg_delete_tree(HKEY_CURRENT_USER, "Software\\RustSonarTestUninstall"));
        assert!(open_key(HKEY_CURRENT_USER, "Software\\RustSonarTestUninstall").is_none());
        assert!(!reg_delete_tree(HKEY_CURRENT_USER, "Software\\RustSonarTestUninstall"));
    }

    /// Lo que quitaría «Desinstalar» en este PC, sin tocar nada:
    /// `cargo test --release -- --ignored probe_uninstall_plan --nocapture`
    #[test]
    #[ignore]
    fn probe_uninstall_plan() {
        fn names(class: &GUID, select: impl Fn(HDEVINFO, &SP_DEVINFO_DATA) -> bool) -> Vec<String> {
            let set = unsafe { SetupDiGetClassDevsW(Some(class), PCWSTR::null(), None, SETUP_DI_GET_CLASS_DEVS_FLAGS(0)) }.unwrap();
            let mut out = Vec::new();
            let mut i = 0;
            loop {
                let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                if unsafe { SetupDiEnumDeviceInfo(set, i, &mut data) }.is_err() {
                    break;
                }
                i += 1;
                if select(set, &data) {
                    out.push(device_string(set, &data, SPDRP_FRIENDLYNAME).or_else(|| device_string(set, &data, windows::Win32::Devices::DeviceAndDriverInstallation::SPDRP_DEVICEDESC)).unwrap_or_default());
                }
            }
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(set);
            }
            out
        }
        for d in &DRIVERS {
            let packages = driver_packages(d);
            let lower: Vec<String> = packages.iter().map(|p| p.to_lowercase()).collect();
            println!("== {}", d.name);
            println!("  paquetes: {packages:?}");
            println!(
                "  dispositivos: {:?}",
                names(&GUID_DEVCLASS_MEDIA, |set, data| {
                    let service = device_string(set, data, SPDRP_SERVICE).unwrap_or_default();
                    let inf = device_inf(set, data).unwrap_or_default().to_lowercase();
                    d.services.iter().any(|s| s.eq_ignore_ascii_case(&service)) || lower.contains(&inf)
                })
            );
            let endpoints = names(&GUID_DEVCLASS_AUDIOENDPOINT, |set, data| {
                device_string(set, data, SPDRP_FRIENDLYNAME).is_some_and(|n| n.to_lowercase().contains(d.endpoint_tag))
            });
            println!("  extremos de audio: {} {:?}", endpoints.len(), endpoints);
            let services: Vec<&str> = d.services.iter().copied().filter(|s| hklm_key_exists(&format!("SYSTEM\\CurrentControlSet\\Services\\{s}")).is_some()).collect();
            println!("  servicios/claves: {services:?}");
            let dirs: Vec<PathBuf> = d.dirs.iter().filter_map(|(v, s)| std::env::var_os(v).map(|b| PathBuf::from(b).join(s))).filter(|p| p.exists()).collect();
            println!("  carpetas: {dirs:?}");
            for base in UNINSTALL_KEYS {
                for sub in reg_subkeys(HKEY_LOCAL_MACHINE, base) {
                    if d.uninstall_entries.iter().any(|p| sub.starts_with(p)) {
                        println!("  aplicaciones: {sub}");
                    }
                }
            }
        }
        println!("== Rust Sonar: {:?}", UNINSTALL_KEYS.iter().find_map(|b| reg_string(HKEY_LOCAL_MACHINE, &format!("{b}\\{INNO_KEY}"), "UninstallString")));
    }

    #[test]
    fn delete_path_removes_folders() {
        let dir = std::env::temp_dir().join("rust_sonar_test_delete_path");
        std::fs::create_dir_all(dir.join("a\\b")).unwrap();
        std::fs::write(dir.join("a\\b\\f.txt"), "x").unwrap();
        assert!(!delete_path(&dir));
        assert!(!dir.exists());
    }
}
