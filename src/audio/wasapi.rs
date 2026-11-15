//! Envoltorios finos y seguros sobre WASAPI (Windows Audio Session API) con el crate `windows`.
//!
//! Piezas clave:
//! - `CaptureStream::process_loopback(pid)`: captura el audio de UNA aplicación (Windows 10 2004+),
//!   sin driver propio. Es lo que sustituye a los dispositivos virtuales de Sonar.
//! - `CaptureStream::device(...)`: captura de un micrófono.
//! - `RenderStream`: reproducción en un dispositivo (auriculares o cable virtual).

use crate::config::SAMPLE_RATE;
use crate::state::DeviceInfo;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl, IAudioMeterInformation};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::core::{HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Ref, implement};

// ---------------------------------------------------------------------------------------------
// Utilidades COM
// ---------------------------------------------------------------------------------------------

/// Inicializa COM (apartamento multihilo) en el hilo actual y lo libera al salir del ámbito.
pub struct ComGuard;

impl ComGuard {
    pub fn init() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        ComGuard
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

/// Evento Win32 de auto-reset que WASAPI señala cada vez que hay un bloque de audio listo.
struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self> {
        Ok(Event(unsafe { CreateEventW(None, false, false, PCWSTR::null())? }))
    }

    fn wait(&self, ms: u32) {
        unsafe {
            WaitForSingleObject(self.0, ms);
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// Las interfaces COM de audio usadas aquí son "free-threaded" (MTA): moverlas entre hilos es seguro.
unsafe impl Send for Event {}

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// Formato interno: float32, 48 kHz, `channels` canales. Con AUTOCONVERTPCM Windows remuestrea solo.
fn float_format(channels: u16) -> WAVEFORMATEX {
    let block = 4 * channels;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: channels,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * block as u32,
        nBlockAlign: block,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Igual que `float_format` pero indicando qué altavoz es cada canal (obligatorio con más de 2).
fn float_format_extensible(channels: u16, mask: u32) -> WAVEFORMATEXTENSIBLE {
    let mut format = float_format(channels);
    format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
    format.cbSize = (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()) as u16;
    WAVEFORMATEXTENSIBLE {
        Format: format,
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: mask,
        SubFormat: windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

/// Canales y máscara de altavoces con los que el dispositivo mezcla internamente.
fn mix_layout(client: &IAudioClient) -> Result<(u16, u32)> {
    unsafe {
        let p = client.GetMixFormat()?;
        let channels = (*p).nChannels;
        let mask = if (*p).wFormatTag == WAVE_FORMAT_EXTENSIBLE && (*p).cbSize >= 22 {
            (*(p as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
        } else {
            0
        };
        CoTaskMemFree(Some(p as *const _));
        let mask = if mask.count_ones() as u16 == channels { mask } else { crate::dsp::surround::default_mask(channels as usize) };
        Ok((channels, mask))
    }
}

/// 20 ms expresados en unidades de 100 ns (REFERENCE_TIME).
const BUFFER_HNS: i64 = 200_000;

pub(crate) unsafe fn pwstr_to_string_and_free(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

// ---------------------------------------------------------------------------------------------
// Dispositivos
// ---------------------------------------------------------------------------------------------

pub fn enumerator() -> Result<IMMDeviceEnumerator> {
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
}

fn device_id(dev: &IMMDevice) -> Result<String> {
    unsafe { Ok(pwstr_to_string_and_free(dev.GetId()?)) }
}

fn device_name(dev: &IMMDevice) -> Result<String> {
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ)?;
        let mut value: PROPVARIANT = store.GetValue(&PKEY_Device_FriendlyName)?;
        let name = PropVariantToStringAlloc(&value).map(|p| pwstr_to_string_and_free(p));
        let _ = PropVariantClear(&mut value);
        Ok(name?)
    }
}

pub fn list_devices(flow: EDataFlow) -> Result<Vec<DeviceInfo>> {
    unsafe {
        let coll = enumerator()?.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..coll.GetCount()? {
            let dev = coll.Item(i)?;
            out.push(DeviceInfo { id: device_id(&dev)?, name: device_name(&dev).unwrap_or_else(|_| "¿?".into()) });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

/// Busca un dispositivo por ID; si no se indica, usa el predeterminado de Windows.
/// Devuelve (dispositivo, nombre, id).
pub fn get_device(id: Option<&str>, flow: EDataFlow) -> Result<(IMMDevice, String, String)> {
    unsafe {
        let en = enumerator()?;
        let dev = match id {
            Some(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
                en.GetDevice(PCWSTR(wide.as_ptr())).context(tr!("el dispositivo elegido ya no está disponible", "the selected device is no longer available"))?
            }
            None => en.GetDefaultAudioEndpoint(flow, eConsole)?,
        };
        let name = device_name(&dev).unwrap_or_default();
        let id = device_id(&dev)?;
        Ok((dev, name, id))
    }
}

/// ID del dispositivo predeterminado de Windows para ese flujo y rol (None si no hay ninguno).
pub fn default_device_id(flow: EDataFlow, role: ERole) -> Option<String> {
    unsafe {
        let dev = enumerator().ok()?.GetDefaultAudioEndpoint(flow, role).ok()?;
        device_id(&dev).ok()
    }
}

/// Volumen y silencio de un dispositivo, tal como se ven en el mezclador de Windows.
pub struct EndpointVolume(IAudioEndpointVolume, Option<IAudioEndpointVolumeCallback>);

/// Avisa (despierta un `Waker`) cuando cambia el volumen o el silencio de un dispositivo.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents {
    waker: std::sync::Arc<super::notify::Waker>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeEvents_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        self.waker.wake();
        Ok(())
    }
}

impl EndpointVolume {
    pub fn open(id: &str) -> Result<Self> {
        unsafe {
            let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
            let dev = enumerator()?.GetDevice(PCWSTR(wide.as_ptr()))?;
            Ok(Self(dev.Activate(CLSCTX_ALL, None)?, None))
        }
    }

    /// Como `open`, pero además despierta `waker` cada vez que el volumen cambia (teclas de volumen,
    /// mezclador de Windows…), para no tener que consultarlo periódicamente.
    pub fn open_watched(id: &str, waker: std::sync::Arc<super::notify::Waker>) -> Result<Self> {
        let mut ep = Self::open(id)?;
        let cb: IAudioEndpointVolumeCallback = VolumeEvents { waker }.into();
        unsafe { ep.0.RegisterControlChangeNotify(&cb)? };
        ep.1 = Some(cb);
        Ok(ep)
    }

    /// (volumen 0..1 en la escala del deslizador de Windows, silenciado).
    pub fn get(&self) -> Result<(f32, bool)> {
        unsafe { Ok((self.0.GetMasterVolumeLevelScalar()?, self.0.GetMute()?.as_bool())) }
    }

    pub fn set(&self, volume: f32, muted: bool) -> Result<()> {
        unsafe {
            self.0.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null())?;
            self.0.SetMute(muted, std::ptr::null())?;
        }
        Ok(())
    }
}

impl Drop for EndpointVolume {
    fn drop(&mut self) {
        if let Some(cb) = &self.1 {
            unsafe {
                let _ = self.0.UnregisterControlChangeNotify(cb);
            }
        }
    }
}

/// Sube la prioridad del hilo actual con MMCSS ("Pro Audio"), para que el audio no se entrecorte
/// cuando un juego tiene la CPU al 100 %. Windows la retira sola al terminar el hilo.
pub fn boost_thread_priority() {
    unsafe {
        let mut task = 0u32;
        let _ = AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task);
    }
}

/// ¿Hay algún proceso en marcha con uno de estos nombres de ejecutable (sin .exe, sin distinguir
/// mayúsculas)?
pub fn process_running(names: &[&str]) -> bool {
    let mut found = false;
    for_each_process(|entry| {
        let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
        let exe = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
        let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
        found |= names.iter().any(|n| n.eq_ignore_ascii_case(stem));
    });
    found
}

fn for_each_process(mut f: impl FnMut(&PROCESSENTRY32W)) {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                f(&entry);
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
}

// ---------------------------------------------------------------------------------------------
// Sesiones de audio (qué procesos están sonando)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SessionProcess {
    pub pid: u32,
    pub name: String,
    /// Pico de la sesión en este instante (0..1), para saber si la app está sonando.
    pub peak: f32,
    /// IDs de los dispositivos de salida en los que está sonando ahora (sesión activa).
    pub devices: Vec<String>,
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned())
    }
}

/// Recorre todos los dispositivos de salida activos y devuelve los procesos con sesión de audio.
pub fn list_session_processes() -> Result<Vec<SessionProcess>> {
    let own_pid = std::process::id();
    let mut out: Vec<SessionProcess> = Vec::new();
    unsafe {
        let coll = enumerator()?.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..coll.GetCount()? {
            let dev = coll.Item(i)?;
            let dev_id = device_id(&dev).unwrap_or_default();
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(sessions) = mgr.GetSessionEnumerator() else { continue };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(ctl) = sessions.GetSession(j) else { continue };
                let Ok(state) = ctl.GetState() else { continue };
                if state == AudioSessionStateExpired {
                    continue;
                }
                // Solo cuenta como "suena en este dispositivo" si la sesión está activa: las apps
                // dejan sesiones viejas e inactivas en la salida que usaban antes.
                let active = state == AudioSessionStateActive;
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if ctl2.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
                    continue;
                }
                let Ok(pid) = ctl2.GetProcessId() else { continue };
                if pid == 0 || pid == own_pid {
                    continue;
                }
                let peak = ctl.cast::<IAudioMeterInformation>().and_then(|m| m.GetPeakValue()).unwrap_or(0.0);
                if let Some(known) = out.iter_mut().find(|p| p.pid == pid) {
                    known.peak = known.peak.max(peak);
                    if active && !known.devices.contains(&dev_id) {
                        known.devices.push(dev_id.clone());
                    }
                    continue;
                }
                let name = process_name(pid).unwrap_or_else(|| format!("pid {pid}"));
                out.push(SessionProcess { pid, name, peak, devices: if active { vec![dev_id.clone()] } else { Vec::new() } });
            }
        }
    }
    // La captura incluye el árbol de procesos: si el padre (o un antepasado) de una sesión también
    // tiene sesión, capturar ambos duplicaría el audio. Nos quedamos solo con el antepasado.
    let pids: HashSet<u32> = out.iter().map(|p| p.pid).collect();
    // Recorrer todos los procesos del sistema cuesta: solo se repite si cambian las apps con sesión.
    static PARENTS: std::sync::Mutex<Option<(HashSet<u32>, HashMap<u32, u32>)>> = std::sync::Mutex::new(None);
    let mut cache = PARENTS.lock().unwrap();
    if cache.as_ref().is_none_or(|(known, _)| known != &pids) {
        *cache = Some((pids.clone(), if pids.len() > 1 { parent_map() } else { HashMap::new() }));
    }
    let parents = &cache.as_ref().unwrap().1;
    out.retain(|p| {
        let mut cur = p.pid;
        for _ in 0..16 {
            match parents.get(&cur) {
                Some(&ppid) if ppid != 0 && ppid != cur => {
                    if pids.contains(&ppid) {
                        return false;
                    }
                    cur = ppid;
                }
                _ => break,
            }
        }
        true
    });
    Ok(out)
}

/// pid -> pid del padre, para todos los procesos del sistema.
fn parent_map() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    for_each_process(|entry| {
        map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
    });
    map
}

// ---------------------------------------------------------------------------------------------
// Activación de "process loopback" (captura por aplicación)
// ---------------------------------------------------------------------------------------------

/// ActivateAudioInterfaceAsync es asíncrona: Windows llama a este objeto COM cuando termina.
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct ActivationHandler {
    done: std::sync::Mutex<Option<mpsc::Sender<()>>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Some(tx) = self.done.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

impl IAgileObject_Impl for ActivationHandler_Impl {}

/// PROPVARIANT de tipo VT_BLOB con la misma disposición en memoria que el de Windows (24 bytes en x64).
/// Lo construimos a mano porque solo necesitamos apuntar a nuestra estructura de parámetros.
#[repr(C)]
struct BlobPropVariant {
    vt: u16,
    reserved: [u16; 3],
    cb_size: u32,
    data: *const u8,
}

const VT_BLOB: u16 = 65;
const _: () = assert!(std::mem::size_of::<BlobPropVariant>() == std::mem::size_of::<PROPVARIANT>());

fn activate_process_loopback(pid: u32) -> Result<IAudioClient> {
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    // Incluye procesos hijos (navegadores y Discord reproducen desde subprocesos).
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let prop = BlobPropVariant {
            vt: VT_BLOB,
            reserved: [0; 3],
            cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            data: &params as *const _ as *const u8,
        };

        let (tx, rx) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler { done: std::sync::Mutex::new(Some(tx)) }.into();

        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&prop as *const BlobPropVariant as *const PROPVARIANT),
            &handler,
        )?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow!(trf!("timeout activando la captura", "timed out starting the capture")))?;

        let mut hr = HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk)?;
        hr.ok().context(tr!("Windows rechazó la captura de la aplicación", "Windows refused to capture the app"))?;
        Ok(unk.ok_or_else(|| anyhow!("activación sin interfaz"))?.cast()?)
    }
}

// ---------------------------------------------------------------------------------------------
// Captura
// ---------------------------------------------------------------------------------------------

pub struct CaptureStream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
    channels: usize,
    /// Posición de altavoz de cada canal (bits de `dwChannelMask`).
    speakers: Vec<u32>,
}

unsafe impl Send for CaptureStream {}

impl CaptureStream {
    /// Captura todo lo que reproduce el proceso `pid` (y sus hijos), en estéreo.
    pub fn process_loopback(pid: u32) -> Result<Self> {
        let client = activate_process_loopback(pid)?;
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;
        Self::init(client, flags, 2)
    }

    /// Captura en loopback todo lo que se reproduce en un dispositivo de salida, con los mismos
    /// canales que el dispositivo (2, 6, 8…). Pedirlo en estéreo haría que Windows redujera el 7.1
    /// con su matriz normalizada, que deja los frontales a −13,5 dB.
    pub fn loopback(dev: &IMMDevice) -> Result<Self> {
        let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None)? };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let (channels, mask) = mix_layout(&client)?;
        if channels <= 2 {
            return Self::init(client, flags, 2);
        }
        let fmt = float_format_extensible(channels, mask);
        let speakers = crate::dsp::surround::speakers_of(mask, channels as usize);
        Self::init_with(client, flags, &fmt.Format, speakers)
    }

    /// Captura un dispositivo de entrada (micrófono) en mono.
    pub fn device(dev: &IMMDevice) -> Result<Self> {
        let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None)? };
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        Self::init(client, flags, 1)
    }

    fn init(client: IAudioClient, flags: u32, channels: u16) -> Result<Self> {
        let fmt = float_format(channels);
        let speakers = crate::dsp::surround::speakers_of(crate::dsp::surround::default_mask(channels as usize), channels as usize);
        Self::init_with(client, flags, &fmt, speakers)
    }

    fn init_with(client: IAudioClient, flags: u32, fmt: *const WAVEFORMATEX, speakers: Vec<u32>) -> Result<Self> {
        unsafe {
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, fmt, None)?;
            let event = Event::new()?;
            client.SetEventHandle(event.0)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self { client, capture, event, channels: speakers.len(), speakers })
        }
    }

    /// Posición de altavoz de cada canal capturado.
    pub fn speakers(&self) -> &[u32] {
        &self.speakers
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Espera al siguiente bloque (máx. `timeout_ms`) y entrega cada paquete a `sink`.
    /// Los paquetes marcados como silencio se entregan como ceros.
    pub fn read(&mut self, timeout_ms: u32, scratch: &mut Vec<f32>, mut sink: impl FnMut(&[f32])) -> Result<()> {
        self.event.wait(timeout_ms);
        unsafe {
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize * self.channels;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    scratch.clear();
                    scratch.resize(n, 0.0);
                    sink(scratch);
                } else {
                    sink(std::slice::from_raw_parts(data as *const f32, n));
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reproducción
// ---------------------------------------------------------------------------------------------

pub struct RenderStream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: Event,
    buffer_frames: u32,
    channels: usize,
}

unsafe impl Send for RenderStream {}

impl RenderStream {
    pub fn open(dev: &IMMDevice, channels: u16) -> Result<Self> {
        unsafe {
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
            let fmt = float_format(channels);
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, &fmt, None)?;
            let event = Event::new()?;
            client.SetEventHandle(event.0)?;
            let buffer_frames = client.GetBufferSize()?;
            if buffer_frames == 0 {
                bail!(trf!("buffer de salida vacío", "empty output buffer"));
            }
            let render: IAudioRenderClient = client.GetService()?;
            client.Start()?;
            Ok(Self { client, render, event, buffer_frames, channels: channels as usize })
        }
    }

    /// Espera a que el dispositivo pida audio y llama a `fill` con el hueco libre (intercalado).
    pub fn write(&mut self, timeout_ms: u32, fill: impl FnOnce(&mut [f32])) -> Result<()> {
        self.event.wait(timeout_ms);
        unsafe {
            let padding = self.client.GetCurrentPadding()?;
            let free = self.buffer_frames.saturating_sub(padding);
            if free == 0 {
                return Ok(());
            }
            let ptr = self.render.GetBuffer(free)?;
            let out = std::slice::from_raw_parts_mut(ptr as *mut f32, free as usize * self.channels);
            fill(out);
            self.render.ReleaseBuffer(free, 0)?;
        }
        Ok(())
    }
}

impl Drop for RenderStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre una captura loopback del dispositivo de salida predeterminado y lee 300 ms.
    /// Comprueba que Windows acepta el formato interno (48 kHz float) con conversión automática.
    #[test]
    #[ignore]
    fn loopback_capture_of_render_device_opens_and_reads() {
        let _com = ComGuard::init();
        let (dev, _, _) = get_device(None, eRender).unwrap();
        let mut stream = CaptureStream::loopback(&dev).unwrap();
        let ch = stream.channels();
        assert_eq!(stream.speakers().len(), ch);
        let mut scratch = Vec::new();
        let mut samples = 0usize;
        for _ in 0..3 {
            stream.read(100, &mut scratch, |s| samples += s.len()).unwrap();
        }
        assert_eq!(samples % ch, 0, "las muestras deben llegar en frames completos");
    }
    /// Sonda: ¿se comportan como cables los dispositivos virtuales instalados?
    /// Reproduce un tono muy bajo en cada salida virtual y mide lo que llega por loopback y por
    /// la entrada emparejada. `cargo test --release -- --ignored probe --nocapture`
    #[test]
    #[ignore]
    fn probe_virtual_endpoints() {
        let _com = ComGuard::init();
        let outs = list_devices(eRender).unwrap();
        let ins = list_devices(eCapture).unwrap();
        for d in &outs {
            println!("OUT {} | {}", d.name, d.id);
        }
        for d in &ins {
            println!("IN  {} | {}", d.name, d.id);
        }
        let tone = |stream: &mut RenderStream, phase: &mut f32| {
            stream
                .write(50, |out| {
                    for f in out.chunks_exact_mut(2) {
                        let v = (*phase).sin() * 0.01;
                        *phase += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
                        f[0] = v;
                        f[1] = v;
                    }
                })
                .unwrap();
        };
        for d in outs.iter().filter(|d| d.name.contains("Sonar") || d.name.contains("CABLE")) {
            let (dev, _, _) = get_device(Some(&d.id), eRender).unwrap();
            let mut cap = match CaptureStream::loopback(&dev) {
                Ok(c) => c,
                Err(e) => {
                    println!("LOOPBACK FALLA {}: {e:#}", d.name);
                    continue;
                }
            };
            let mut scratch = Vec::new();
            let mut base = 0.0f32;
            for _ in 0..10 {
                cap.read(30, &mut scratch, |s| base = base.max(crate::dsp::peak(s))).unwrap();
            }
            let mut out = RenderStream::open(&dev, 2).unwrap();
            let mut phase = 0.0;
            let mut pk = 0.0f32;
            let mut n = 0usize;
            for _ in 0..40 {
                tone(&mut out, &mut phase);
                cap.read(5, &mut scratch, |s| {
                    pk = pk.max(crate::dsp::peak(s));
                    n += s.len();
                })
                .unwrap();
            }
            println!("LOOPBACK {} -> {} canales, base {base:.5} con tono {pk:.5} ({n} muestras)", d.name, cap.channels());
        }
        // Par de micrófono: salida "Microphone" -> entrada "Microphone".
        for d in outs.iter().filter(|d| d.name.contains("Microphone") || d.name.contains("CABLE")) {
            let Some(input) = ins.iter().find(|i| i.name.contains("Sonar") || i.name.contains("CABLE")) else { continue };
            let (rdev, _, _) = get_device(Some(&d.id), eRender).unwrap();
            let (cdev, _, _) = get_device(Some(&input.id), eCapture).unwrap();
            let mut cap = CaptureStream::device(&cdev).unwrap();
            let mut scratch = Vec::new();
            let mut base = 0.0f32;
            for _ in 0..15 {
                cap.read(30, &mut scratch, |s| base = base.max(crate::dsp::peak(s))).unwrap();
            }
            let mut out = RenderStream::open(&rdev, 2).unwrap();
            let mut phase = 0.0;
            let mut pk = 0.0f32;
            for _ in 0..40 {
                tone(&mut out, &mut phase);
                cap.read(5, &mut scratch, |s| pk = pk.max(crate::dsp::peak(s))).unwrap();
            }
            println!("MIC {} -> {} : base {base:.5} con tono {pk:.5}", d.name, input.name);
        }
    }
    /// Mide el pico de cada entrada real durante `MIC_SECS` segundos (3 por defecto): sirve para
    /// saber qué micro recibe voz. `cargo test --release -- --ignored probe_mic_levels --nocapture`
    #[test]
    #[ignore]
    fn probe_mic_levels() {
        let _com = ComGuard::init();
        let secs: u32 = std::env::var("MIC_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
        let mut streams: Vec<(String, CaptureStream, f32)> = Vec::new();
        for d in list_devices(eCapture).unwrap() {
            match get_device(Some(&d.id), eCapture).and_then(|(dev, _, _)| CaptureStream::device(&dev)) {
                Ok(s) => streams.push((d.name, s, 0.0)),
                Err(e) => println!("MIC no se abre {}: {e}", d.name),
            }
        }
        let mut scratch = Vec::new();
        for _ in 0..secs * 50 {
            for (_, s, peak) in streams.iter_mut() {
                let _ = s.read(0, &mut scratch, |buf| *peak = buf.iter().fold(*peak, |p, x| p.max(x.abs())));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        for (name, _, peak) in &streams {
            let db = 20.0 * peak.max(1e-9).log10();
            println!("MIC pico {db:7.1} dB  {name}");
        }
    }
    /// Imprime los predeterminados de Windows y el volumen de cada salida (diagnóstico).
    /// `cargo test --release -- --ignored audio_state --nocapture`
    #[test]
    #[ignore]
    fn audio_state() {
        let _com = ComGuard::init();
        let name = |flow, id: &Option<String>| {
            id.as_ref().and_then(|id| get_device(Some(id), flow).ok()).map(|(_, n, _)| n).unwrap_or_else(|| "-".into())
        };
        for (label, flow) in [("salida", eRender), ("entrada", eCapture)] {
            for (rl, role) in [("consola", eConsole), ("multimedia", eMultimedia), ("comunicaciones", eCommunications)] {
                println!("PREDETERMINADO {label} {rl}: {}", name(flow, &default_device_id(flow, role)));
            }
        }
        for d in list_devices(eRender).unwrap() {
            let v = EndpointVolume::open(&d.id).and_then(|e| e.get());
            println!("VOLUMEN {:?} {}", v.ok(), d.name);
        }
    }
    /// ¿Afecta el volumen del dispositivo en Windows a lo que se captura por loopback?
    /// Mide un tono de amplitud 0,01 con el volumen al 100 % y al 50 % en cada salida virtual.
    #[test]
    #[ignore]
    fn probe_volume_affects_loopback() {
        let _com = ComGuard::init();
        for d in list_devices(eRender).unwrap().iter().filter(|d| d.name.contains("Sonar") || d.name.contains("CABLE")) {
            let (dev, _, _) = get_device(Some(&d.id), eRender).unwrap();
            let ep = EndpointVolume::open(&d.id).unwrap();
            let before = ep.get().unwrap();
            let mut levels = Vec::new();
            for vol in [1.0f32, 0.5] {
                ep.set(vol, false).unwrap();
                let mut cap = CaptureStream::loopback(&dev).unwrap();
                let mut out = RenderStream::open(&dev, 2).unwrap();
                let mut scratch = Vec::new();
                let (mut phase, mut pk) = (0.0f32, 0.0f32);
                for i in 0..50 {
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
                levels.push(pk);
            }
            ep.set(before.0, before.1).unwrap();
            println!("VOL {} -> 100%: {:.5}  50%: {:.5}", d.name, levels[0], levels[1]);
        }
    }
    /// Reproduce un tono en la salida cuyo nombre contenga TONE_DEVICE (vacío = predeterminada)
    /// durante TONE_SECS segundos, por el canal TONE_CH de un flujo de TONE_CHANNELS canales.
    #[test]
    #[ignore]
    fn play_tone() {
        let _com = ComGuard::init();
        let want = std::env::var("TONE_DEVICE").unwrap_or_default();
        let secs: f32 = std::env::var("TONE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
        let channels: u16 = std::env::var("TONE_CHANNELS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
        let only: Option<usize> = std::env::var("TONE_CH").ok().and_then(|s| s.parse().ok());
        let id = if want.is_empty() { None } else { list_devices(eRender).unwrap().into_iter().find(|d| d.name.contains(&want)).map(|d| d.id) };
        let (dev, name, _) = get_device(id.as_deref(), eRender).unwrap();
        println!("TONO en {name}");
        let mut out = RenderStream::open(&dev, channels).unwrap();
        let mut phase = 0.0f32;
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f32() < secs {
            out.write(50, |o| {
                for f in o.chunks_exact_mut(channels as usize) {
                    let v = phase.sin() * 0.05;
                    phase += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
                    for (i, x) in f.iter_mut().enumerate() {
                        *x = if only.map(|c| c == i).unwrap_or(true) { v } else { 0.0 };
                    }
                }
            })
            .unwrap();
        }
    }
}
