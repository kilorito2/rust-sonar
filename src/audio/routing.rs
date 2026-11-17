//! Enrutado a nivel de Windows, igual que hace un mezclador con dispositivos virtuales:
//!
//! - **Salida por aplicación**: cambia el dispositivo en el que suena cada app (lo mismo que
//!   *Configuración → Sonido → Volumen de aplicaciones y preferencias de dispositivos*). Interfaz
//!   interna `IAudioPolicyConfigFactory` de `AudioSes.dll`, activada por WinRT.
//! - **Salida predeterminada de Windows**: interfaz COM interna `IPolicyConfig`.
//! - **Renombrar dispositivos**: escribe `PKEY_Device_DeviceDesc` en el almacén de propiedades del
//!   endpoint. Requiere administrador, así que se hace relanzando este mismo .exe elevado.
//! - **Ocultar dispositivos**: `IPolicyConfig::SetEndpointVisibility` (el «Deshabilitar» del panel
//!   de sonido). También se hace desde el proceso elevado.
//!
//! Las dos primeras interfaces no están documentadas por Microsoft: se llaman por índice en la
//! vtable. Las usan herramientas conocidas desde Windows 10 y su forma es estable, pero si una
//! actualización de Windows las cambiara, estas funciones devolverían error (nunca se cuelgan).

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::c_void;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_DeviceDesc;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{EDataFlow, ERole, eCommunications, eConsole, eMultimedia, eRender};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, STGM_READWRITE};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR};

#[link(name = "combase", kind = "raw-dylib")]
unsafe extern "system" {
    fn RoGetActivationFactory(class_id: *mut c_void, iid: *const GUID, factory: *mut *mut c_void) -> HRESULT;
}

/// Puntero COM "en crudo" (para interfaces sin definición pública). Hace Release al soltarse.
struct RawCom(*mut c_void);

impl RawCom {
    /// Función número `index` de la vtable.
    unsafe fn method(&self, index: usize) -> *const c_void {
        unsafe {
            let vtable = *(self.0 as *const *const *const c_void);
            *vtable.add(index)
        }
    }
}

impl Drop for RawCom {
    fn drop(&mut self) {
        unsafe {
            let release: extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(self.method(2));
            release(self.0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Salida por aplicación
// ---------------------------------------------------------------------------------------------

/// El GUID de la interfaz cambió en Windows 10 21H2; probamos el nuevo y luego el antiguo.
const POLICY_FACTORY_IIDS: [GUID; 2] = [
    GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324),
    GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258),
];
// IUnknown (3) + IInspectable (3) + 19 métodos que no usamos.
const VT_SET_PERSISTED_ENDPOINT: usize = 25;
#[allow(dead_code)]
const VT_GET_PERSISTED_ENDPOINT: usize = 26;
const VT_CLEAR_ALL_PERSISTED: usize = 27;

const MMDEVAPI_PREFIX: &str = r"\\?\SWD#MMDEVAPI#";
const RENDER_INTERFACE_SUFFIX: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";

type SetPersistedFn = extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT;
#[allow(dead_code)]
type GetPersistedFn = extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut *mut c_void) -> HRESULT;
type ClearAllFn = extern "system" fn(*mut c_void) -> HRESULT;

fn policy_factory() -> Result<RawCom> {
    let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
    for iid in &POLICY_FACTORY_IIDS {
        let mut ptr = std::ptr::null_mut();
        let hr = unsafe { RoGetActivationFactory(std::mem::transmute_copy(&class), iid, &mut ptr) };
        if hr.is_ok() && !ptr.is_null() {
            return Ok(RawCom(ptr));
        }
    }
    bail!(trf!("esta versión de Windows no permite cambiar la salida de cada aplicación", "this version of Windows doesn't allow changing each app's output"))
}

/// Hace que el proceso `pid` reproduzca en `device_id` (None = volver al predeterminado de Windows).
/// Windows lo recuerda por ejecutable, como en la configuración de sonido.
pub fn set_app_output(pid: u32, device_id: Option<&str>) -> Result<()> {
    let factory = policy_factory()?;
    let path = device_id.map(|id| HSTRING::from(format!("{MMDEVAPI_PREFIX}{id}{RENDER_INTERFACE_SUFFIX}")));
    unsafe {
        let set: SetPersistedFn = std::mem::transmute(factory.method(VT_SET_PERSISTED_ENDPOINT));
        // Un HSTRING vacío (puntero nulo) significa "sin preferencia".
        let raw: *mut c_void = match &path {
            Some(h) => std::mem::transmute_copy(h),
            None => std::ptr::null_mut(),
        };
        for role in [eConsole, eMultimedia] {
            set(factory.0, pid, eRender, role, raw).ok().context(tr!("no se pudo cambiar la salida de la app", "couldn't change the app's output"))?;
        }
    }
    Ok(())
}

/// Dispositivo de salida configurado para `pid` (None = sigue al predeterminado).
#[allow(dead_code)] // diagnóstico y tests
pub fn get_app_output(pid: u32) -> Result<Option<String>> {
    let factory = policy_factory()?;
    unsafe {
        let get: GetPersistedFn = std::mem::transmute(factory.method(VT_GET_PERSISTED_ENDPOINT));
        let mut raw = std::ptr::null_mut();
        get(factory.0, pid, eRender, eMultimedia, &mut raw).ok()?;
        if raw.is_null() {
            return Ok(None);
        }
        let h: HSTRING = std::mem::transmute(raw);
        let s = h.to_string();
        let id = s.trim_start_matches(MMDEVAPI_PREFIX).trim_end_matches(RENDER_INTERFACE_SUFFIX);
        Ok(if id.is_empty() { None } else { Some(id.to_string()) })
    }
}

/// Borra las salidas por aplicación de TODAS las apps (vuelven al predeterminado de Windows).
pub fn clear_all_app_outputs() -> Result<()> {
    let factory = policy_factory()?;
    unsafe {
        let clear: ClearAllFn = std::mem::transmute(factory.method(VT_CLEAR_ALL_PERSISTED));
        clear(factory.0).ok()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Salida predeterminada de Windows
// ---------------------------------------------------------------------------------------------

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
const VT_SET_DEFAULT_ENDPOINT: usize = 13;
const VT_SET_ENDPOINT_VISIBILITY: usize = 14;

type SetDefaultFn = extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT;

/// Pone `device_id` (de reproducción o de grabación) como predeterminado de Windows para esos roles.
pub fn set_default_device(device_id: &str, roles: &[ERole]) -> Result<()> {
    unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut ptr = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut ptr).ok()?;
        let policy = RawCom(ptr);
        let set: SetDefaultFn = std::mem::transmute(policy.method(VT_SET_DEFAULT_ENDPOINT));
        let wide: Vec<u16> = device_id.encode_utf16().chain(Some(0)).collect();
        for role in roles {
            set(policy.0, PCWSTR(wide.as_ptr()), *role).ok().context(tr!("no se pudo cambiar el predeterminado", "couldn't change the default device"))?;
        }
    }
    Ok(())
}

pub const ALL_ROLES: [ERole; 3] = [eConsole, eMultimedia, eCommunications];

// ---------------------------------------------------------------------------------------------
// Renombrar dispositivos
// ---------------------------------------------------------------------------------------------

/// PROPVARIANT de tipo VT_LPWSTR construido a mano (misma disposición que el de Windows).
#[repr(C)]
struct StringPropVariant {
    vt: u16,
    reserved: [u16; 3],
    ptr: *const u16,
    _pad: usize,
}

const VT_LPWSTR: u16 = 31;
const _: () = assert!(std::mem::size_of::<StringPropVariant>() == std::mem::size_of::<PROPVARIANT>());

/// Cambia el nombre de un dispositivo (lo que se ve en Windows antes del paréntesis).
/// Necesita permisos de administrador.
pub fn rename_device(device_id: &str, name: &str) -> Result<()> {
    unsafe {
        let wide_id: Vec<u16> = device_id.encode_utf16().chain(Some(0)).collect();
        let dev = super::wasapi::enumerator()?.GetDevice(PCWSTR(wide_id.as_ptr()))?;
        let store = dev.OpenPropertyStore(STGM_READWRITE).context(tr!("sin permisos para modificar el dispositivo", "no permission to modify the device"))?;
        let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let value = StringPropVariant { vt: VT_LPWSTR, reserved: [0; 3], ptr: wide_name.as_ptr(), _pad: 0 };
        store.SetValue(&PKEY_Device_DeviceDesc, &value as *const StringPropVariant as *const PROPVARIANT)?;
        store.Commit()?;
    }
    Ok(())
}

/// Muestra u oculta un dispositivo en Windows (lo mismo que «Deshabilitar» en el panel de sonido
/// clásico). Oculto, ninguna app lo ve: ni en Windows ni en Discord. Necesita administrador.
pub fn set_device_visible(device_id: &str, visible: bool) -> Result<()> {
    type SetVisibilityFn = extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT;
    unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut ptr = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut ptr).ok()?;
        let policy = RawCom(ptr);
        let set: SetVisibilityFn = std::mem::transmute(policy.method(VT_SET_ENDPOINT_VISIBILITY));
        let wide: Vec<u16> = device_id.encode_utf16().chain(Some(0)).collect();
        set(policy.0, PCWSTR(wide.as_ptr()), visible as i32).ok().context(tr!("no se pudo cambiar la visibilidad", "couldn't change visibility"))?;
    }
    Ok(())
}

/// Cambio sobre un dispositivo que necesita permisos de administrador.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceChange {
    Rename { id: String, name: String },
    Hide(String),
    Show(String),
}

impl DeviceChange {
    fn to_arg(&self) -> String {
        match self {
            DeviceChange::Rename { id, name } => format!("rename|{id}|{name}"),
            DeviceChange::Hide(id) => format!("hide|{id}"),
            DeviceChange::Show(id) => format!("show|{id}"),
        }
    }

    fn from_arg(arg: &str) -> Option<Self> {
        let mut parts = arg.splitn(3, '|');
        match (parts.next()?, parts.next()?, parts.next()) {
            ("rename", id, Some(name)) => Some(DeviceChange::Rename { id: id.into(), name: name.into() }),
            ("hide", id, None) => Some(DeviceChange::Hide(id.into())),
            ("show", id, None) => Some(DeviceChange::Show(id.into())),
            _ => None,
        }
    }

    fn apply(&self) -> Result<()> {
        match self {
            DeviceChange::Rename { id, name } => rename_device(id, name),
            DeviceChange::Hide(id) => set_device_visible(id, false),
            DeviceChange::Show(id) => set_device_visible(id, true),
        }
    }
}

pub const DEVICE_ADMIN_FLAG: &str = "--device-admin";

/// Relanza este ejecutable como administrador (aparece el aviso de UAC una sola vez) para aplicar
/// todos los cambios.
/// Aplica los cambios aquí mismo si ya somos administrador (p. ej. lanzados por el instalador) o,
/// si no, relanzándonos como administrador (un aviso de Windows).
pub fn apply_device_changes(changes: &[DeviceChange]) -> Result<()> {
    if !crate::system::is_elevated() {
        return apply_device_changes_elevated(changes);
    }
    let args: Vec<String> = changes.iter().map(DeviceChange::to_arg).collect();
    match run_device_admin_helper(&args) {
        0 => Ok(()),
        n => Err(anyhow!(trf!("fallaron {n} de {} cambios", "{n} of {} changes failed", changes.len()))),
    }
}

pub fn apply_device_changes_elevated(changes: &[DeviceChange]) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let mut args = String::from(DEVICE_ADMIN_FLAG);
    for change in changes {
        let arg = change.to_arg();
        if arg.contains('"') {
            bail!("nombre no válido: {arg}");
        }
        args.push_str(&format!(" \"{arg}\""));
    }
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).context(tr!("cancelado o sin permisos de administrador", "cancelled or no administrator rights"))?;
        if info.hProcess.is_invalid() {
            bail!(trf!("no se pudo lanzar el proceso elevado", "couldn't start the elevated process"));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code != 0 {
            return Err(anyhow!(trf!("el proceso elevado falló en {code} de {} cambios", "the elevated process failed on {code} of {} changes", changes.len())));
        }
    }
    Ok(())
}

/// Punto de entrada del proceso elevado: `rust_sonar.exe --device-admin "hide|id" "rename|id|nombre" ...`.
/// Devuelve el número de errores (código de salida).
pub fn run_device_admin_helper(args: &[String]) -> i32 {
    let _com = super::wasapi::ComGuard::init();
    let mut errors = 0;
    for arg in args {
        let result = DeviceChange::from_arg(arg).ok_or_else(|| anyhow!("argumento no válido")).and_then(|c| c.apply());
        if let Err(e) = result {
            eprintln!("[dispositivos] {arg}: {e:#}");
            errors += 1;
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prueba real contra Windows sobre el propio proceso de test (no toca otras apps):
    /// fija una salida, la lee y la borra. `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn app_output_roundtrip_on_own_process() {
        let _com = crate::audio::wasapi::ComGuard::init();
        let (_, _, id) = crate::audio::wasapi::get_device(None, eRender).unwrap();
        let pid = std::process::id();
        set_app_output(pid, Some(&id)).unwrap();
        assert_eq!(get_app_output(pid).unwrap().as_deref(), Some(id.as_str()));
        set_app_output(pid, None).unwrap();
        assert_eq!(get_app_output(pid).unwrap(), None);
    }

    /// Diagnóstico (solo lee): por cada app que suena, en qué dispositivo está de verdad y qué salida
    /// tiene pedida en Windows. `cargo test --release -- --ignored probe_app_routing --nocapture`
    #[test]
    #[ignore]
    fn probe_app_routing() {
        use crate::audio::wasapi;
        let _com = wasapi::ComGuard::init();
        let outs = wasapi::list_devices(eRender).unwrap();
        let name = |id: &str| outs.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_else(|| format!("? {id}"));
        for p in wasapi::list_session_processes().unwrap() {
            let wanted = match get_app_output(p.pid) {
                Ok(Some(id)) => name(&id),
                Ok(None) => "(predeterminado)".into(),
                Err(e) => format!("ERROR {e:#}"),
            };
            let on: Vec<String> = p.devices.iter().map(|id| name(id)).collect();
            println!("APP {} pid {} pico {:.3} | pedida: {wanted} | suena en: {on:?}", p.name, p.pid, p.peak);
        }
    }

    /// ¿Respeta Windows la salida por aplicación? Sobre el propio proceso de test: pide cada salida
    /// virtual, mira cuál pasa a ser SU predeterminada y dónde acaba sonando un tono abierto en «la
    /// predeterminada». Al final quita la preferencia.
    /// `cargo test --release -- --ignored probe_own_routing --nocapture`
    #[test]
    #[ignore]
    fn probe_own_routing() {
        use crate::audio::wasapi::{self, CaptureStream, RenderStream};
        let _com = wasapi::ComGuard::init();
        let pid = std::process::id();
        let outs = wasapi::list_devices(eRender).unwrap();
        let name = |id: &str| outs.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_else(|| format!("? {id}"));
        println!("ANTES predeterminada del proceso: {}", wasapi::default_device_id(eRender, eConsole).map(|i| name(&i)).unwrap_or_default());
        for target in outs.iter().filter(|d| d.name.contains("Rust Sonar - ") && !d.name.contains("micro") && !d.name.contains("Mic")) {
            set_app_output(pid, Some(&target.id)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            let def = wasapi::default_device_id(eRender, eConsole).unwrap_or_default();
            // Tono en «la predeterminada» y escucha en el cable pedido.
            let (tdev, _, _) = wasapi::get_device(Some(&target.id), eRender).unwrap();
            let mut cap = CaptureStream::loopback(&tdev).unwrap();
            let (ddev, _, _) = wasapi::get_device(None, eRender).unwrap();
            let mut out = RenderStream::open(&ddev, 2).unwrap();
            let (mut scratch, mut phase, mut pk) = (Vec::new(), 0.0f32, 0.0f32);
            for i in 0..60 {
                out.write(50, |o| {
                    for f in o.chunks_exact_mut(2) {
                        let v = phase.sin() * 0.01;
                        phase += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
                        f[0] = v;
                        f[1] = v;
                    }
                })
                .unwrap();
                cap.read(5, &mut scratch, |s| {
                    if i > 20 {
                        pk = pk.max(crate::dsp::peak(s));
                    }
                })
                .unwrap();
            }
            println!("PEDIDA {} | predeterminada del proceso: {} | tono que llega al cable pedido: {pk:.5}", target.name, name(&def));
        }
        set_app_output(pid, None).unwrap();
    }

    /// Simula que Windows cambia los predeterminados por su cuenta: pone la primera salida y la primera
    /// entrada REALES como predeterminadas y, a los 4 s, dice cuáles lo son. Con Rust Sonar abierto y
    /// «Gestionar los predeterminados» activado, deben haber vuelto a ser las suyas.
    /// `cargo test --release -- --ignored probe_defaults_are_restored --nocapture`
    #[test]
    #[ignore]
    fn probe_defaults_are_restored() {
        use crate::audio::wasapi;
        use windows::Win32::Media::Audio::eCapture;
        let _com = wasapi::ComGuard::init();
        for (label, flow) in [("salida", eRender), ("entrada", eCapture)] {
            let devices = wasapi::list_devices(flow).unwrap();
            let name = |id: &str| devices.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_default();
            let before = wasapi::default_device_id(flow, eConsole).unwrap_or_default();
            let real = devices.iter().find(|d| !crate::virtual_devices::is_virtual_cable(&d.name)).expect("un dispositivo real");
            set_default_device(&real.id, &ALL_ROLES).unwrap();
            let forced = wasapi::default_device_id(flow, eConsole).unwrap_or_default();
            std::thread::sleep(std::time::Duration::from_secs(4));
            let after = wasapi::default_device_id(flow, eConsole).unwrap_or_default();
            println!("DEFAULT {label}: antes «{}» | forzada «{}» | 4 s después «{}»", name(&before), name(&forced), name(&after));
        }
    }

    /// Vuelve a poner como predeterminado el dispositivo que ya lo es (no cambia nada).
    #[test]
    #[ignore]
    fn set_default_output_is_accepted() {
        let _com = crate::audio::wasapi::ComGuard::init();
        let (_, _, id) = crate::audio::wasapi::get_device(None, eRender).unwrap();
        set_default_device(&id, &[eConsole]).unwrap();
        let (_, _, after) = crate::audio::wasapi::get_device(None, eRender).unwrap();
        assert_eq!(after, id);
    }
}
