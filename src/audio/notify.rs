//! Avisos de Windows en lugar de consultas periódicas: así los hilos de Rust Sonar duermen sin gastar
//! CPU y despiertan al instante cuando pasa algo.
//!
//! - `DeviceWatcher` (IMMNotificationClient): dispositivos conectados, quitados, renombrados o
//!   predeterminados cambiados.
//! - `SessionWatcher` (IAudioSessionNotification + IAudioSessionEvents): una app empieza o deja de
//!   sonar en un dispositivo (o de grabar de un micrófono). Con esto cada canal solo captura cuando
//!   tiene algo que capturar y el micro solo se procesa cuando alguien lo graba.
//! - Volumen de un dispositivo: `wasapi::EndpointVolume::open_watched` (IAudioEndpointVolumeCallback).
//!
//! Windows llama a estos objetos desde sus propios hilos; lo único que hacen es despertar un `Waker`.

use super::wasapi;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::core::{BOOL, GUID, Interface, PCWSTR, Ref, Result, implement};

/// Despertador entre hilos: `wake` desde cualquier sitio, `wait` duerme hasta un aviso o un tiempo máximo.
#[derive(Default)]
pub struct Waker {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Waker {
    pub fn wake(&self) {
        *self.flag.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// Duerme hasta un aviso o `timeout`. Devuelve true si lo despertó un aviso.
    pub fn wait(&self, timeout: Duration) -> bool {
        let guard = self.flag.lock().unwrap();
        let (mut guard, _) = self.cv.wait_timeout_while(guard, timeout, |woken| !*woken).unwrap();
        std::mem::replace(&mut *guard, false)
    }
}

// ---------------------------------------------------------------------------------------------
// Dispositivos
// ---------------------------------------------------------------------------------------------

#[implement(IMMNotificationClient)]
struct DeviceEvents {
    waker: Arc<Waker>,
    dirty: Arc<AtomicBool>,
}

impl DeviceEvents {
    fn signal(&self) -> Result<()> {
        self.dirty.store(true, Ordering::Relaxed);
        self.waker.wake();
        Ok(())
    }
}

impl IMMNotificationClient_Impl for DeviceEvents_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        self.signal()
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        self.signal()
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        self.signal()
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> Result<()> {
        self.signal()
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, key: &PROPERTYKEY) -> Result<()> {
        // Solo interesan los nombres (al renombrar dispositivos); Windows avisa de muchas otras propiedades.
        if key.fmtid == PKEY_Device_FriendlyName.fmtid { self.signal() } else { Ok(()) }
    }
}

/// Mientras exista, `dirty` se pone a true (y se despierta `waker`) cada vez que cambian los dispositivos.
pub struct DeviceWatcher {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

impl DeviceWatcher {
    pub fn new(waker: Arc<Waker>, dirty: Arc<AtomicBool>) -> anyhow::Result<Self> {
        let enumerator = wasapi::enumerator()?;
        let client: IMMNotificationClient = DeviceEvents { waker, dirty }.into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&client)? };
        Ok(Self { enumerator, client })
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sesiones de audio de un dispositivo
// ---------------------------------------------------------------------------------------------

#[implement(IAudioSessionNotification)]
struct NewSessionEvents {
    waker: Arc<Waker>,
}

impl IAudioSessionNotification_Impl for NewSessionEvents_Impl {
    fn OnSessionCreated(&self, _session: Ref<IAudioSessionControl>) -> Result<()> {
        self.waker.wake();
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionEvents {
    waker: Arc<Waker>,
}

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _name: &PCWSTR, _ctx: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _path: &PCWSTR, _ctx: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _volume: f32, _mute: BOOL, _ctx: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _count: u32, _volumes: *const f32, _changed: u32, _ctx: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _param: *const GUID, _ctx: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, _state: AudioSessionState) -> Result<()> {
        self.waker.wake();
        Ok(())
    }
    fn OnSessionDisconnected(&self, _reason: AudioSessionDisconnectReason) -> Result<()> {
        self.waker.wake();
        Ok(())
    }
}

/// Vigila las sesiones de audio de un dispositivo (de salida o de grabación) y avisa cuando se crea
/// una o cambia de estado (empieza o deja de sonar/grabar).
pub struct SessionWatcher {
    manager: IAudioSessionManager2,
    notification: IAudioSessionNotification,
    sessions: Vec<(String, IAudioSessionControl, IAudioSessionEvents)>,
    waker: Arc<Waker>,
}

impl SessionWatcher {
    pub fn new(device_id: &str, flow: EDataFlow, waker: Arc<Waker>) -> anyhow::Result<Self> {
        let (device, _, _) = wasapi::get_device(Some(device_id), flow)?;
        unsafe {
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            // Windows solo avisa de sesiones nuevas si antes se ha pedido la lista.
            let _ = manager.GetSessionEnumerator()?;
            let notification: IAudioSessionNotification = NewSessionEvents { waker: waker.clone() }.into();
            manager.RegisterSessionNotification(&notification)?;
            let mut watcher = Self { manager, notification, sessions: Vec::new(), waker };
            watcher.refresh();
            Ok(watcher)
        }
    }

    /// Repasa las sesiones (se suscribe a las nuevas) y devuelve si alguna de OTRO proceso está
    /// activa. Incluye la de sonidos del sistema.
    pub fn refresh(&mut self) -> bool {
        let own_pid = std::process::id();
        let mut active = false;
        let mut seen: Vec<String> = Vec::new();
        unsafe {
            let Ok(list) = self.manager.GetSessionEnumerator() else { return false };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(control) = list.GetSession(i) else { continue };
                let Ok(state) = control.GetState() else { continue };
                if state == AudioSessionStateExpired {
                    continue;
                }
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                let id = control2.GetSessionInstanceIdentifier().map(|p| wasapi::pwstr_to_string_and_free(p)).unwrap_or_default();
                let pid = control2.GetProcessId().unwrap_or(0);
                if !self.sessions.iter().any(|(known, ..)| known == &id) {
                    let events: IAudioSessionEvents = SessionEvents { waker: self.waker.clone() }.into();
                    if control.RegisterAudioSessionNotification(&events).is_ok() {
                        self.sessions.push((id.clone(), control.clone(), events));
                    }
                }
                seen.push(id);
                if pid != own_pid && state == AudioSessionStateActive {
                    active = true;
                }
            }
        }
        self.sessions.retain(|(id, control, events)| {
            let keep = seen.contains(id);
            if !keep {
                unsafe {
                    let _ = control.UnregisterAudioSessionNotification(events);
                }
            }
            keep
        });
        active
    }
}

impl Drop for SessionWatcher {
    fn drop(&mut self) {
        unsafe {
            for (_, control, events) in &self.sessions {
                let _ = control.UnregisterAudioSessionNotification(events);
            }
            let _ = self.manager.UnregisterSessionNotification(&self.notification);
        }
    }
}
