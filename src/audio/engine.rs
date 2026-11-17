//! Motor principal: detecta aplicaciones, las enruta, captura cada canal, mezcla y saca el
//! resultado por los auriculares.
//!
//! Hay dos formas de alimentar un canal (se elige por canal en Ajustes → Dispositivos virtuales):
//!
//! ```text
//!  Modo dispositivo virtual (como Sonar):
//!   app ──(Windows la enruta)──► "Rust Sonar - Juego" (cable virtual) ──loopback──► Fifo ─► bus Juego
//!
//!  Modo sin drivers:
//!   app ──process loopback──► Fifo ─► bus del canal de la app
//!
//!  bus Juego ─► EQ ─► vol·ChatMix ─┐
//!  bus Chat  ─► EQ ─► vol·ChatMix ─┼─► master ─► limitador ─► auriculares
//!  bus Media ─► EQ ─► vol ─────────┤
//!  bus Aux   ─► EQ ─► vol ─────────┘
//! ```
//!
//! En modo streamer, cada bus (ya ecualizado) se suma además con su propio volumen a una segunda
//! mezcla, la de retransmisión, junto con el micro. Esa mezcla pasa por una cola (`Shared::stream_out`)
//! al hilo `stream_loop`, que la saca por su dispositivo (p. ej. un cable virtual que captura OBS).
//! Así el público oye su propia mezcla y tú la tuya.

use super::fifo::Fifo;
use super::notify;
use super::routing;
use super::wasapi::{self, CaptureStream, ComGuard, EndpointVolume, RenderStream};
use crate::config::{Channel, SAMPLE_RATE, Settings};
use crate::dsp::compressor::{Compressor, CompressorParams};
use crate::dsp::surround::Surround;
use crate::dsp::{SmoothedGain, db_to_lin, eq::Equalizer, peak, soft_clip};
use crate::state::{AppInfo, Devices, Shared};
use crate::virtual_devices as vdev;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::{EDataFlow, IMMDevice, eCapture, eCommunications, eConsole, eMultimedia, eRender};

/// Colchón por fuente: 20 ms (ver `Fifo`).
const SOURCE_LATENCY_FRAMES: usize = SAMPLE_RATE as usize * 20 / 1000;
/// Cuánto sigue capturando un canal después de que su última app deje de sonar (cola del sonido y
/// apps que paran y arrancan seguido).
const CAPTURE_HANG: Duration = Duration::from_secs(3);

/// Algo que estamos capturando: una aplicación concreta o un dispositivo virtual entero.
pub struct Source {
    pub pid: u32,
    /// Some(id) si es la captura de un dispositivo virtual de canal.
    pub device: Option<String>,
    pub name: String,
    channel: AtomicU8,
    fifo: Fifo,
    stop: AtomicBool,
    peak: crate::state::AtomicF32,
    error: Mutex<Option<String>>,
}

impl Source {
    fn new(pid: u32, device: Option<String>, name: String, channel: Channel) -> Arc<Self> {
        Arc::new(Source {
            pid,
            device,
            name,
            channel: AtomicU8::new(channel.index() as u8),
            fifo: Fifo::new(2, SOURCE_LATENCY_FRAMES),
            stop: AtomicBool::new(false),
            peak: Default::default(),
            error: Mutex::new(None),
        })
    }

    fn channel(&self) -> Channel {
        Channel::from_index(self.channel.load(Ordering::Relaxed) as usize)
    }
}

type SourceList = Arc<RwLock<Vec<Arc<Source>>>>;

/// Devuelve el hilo de sesiones: hay que esperarlo al cerrar para que restaure el enrutado.
pub fn spawn(shared: Arc<Shared>) -> thread::JoinHandle<()> {
    let sources: SourceList = Arc::new(RwLock::new(Vec::new()));

    let (s, list) = (shared.clone(), sources.clone());
    let sessions = thread::Builder::new().name("sesiones".into()).spawn(move || session_manager(s, list)).unwrap();

    let s = shared.clone();
    thread::Builder::new().name("volumen".into()).spawn(move || volume_sync(s)).unwrap();
    let s = shared.clone();
    thread::Builder::new().name("retransmision".into()).spawn(move || stream_loop(s)).unwrap();
    thread::Builder::new().name("mezclador".into()).spawn(move || output_loop(shared, sources)).unwrap();
    sessions
}

// ---------------------------------------------------------------------------------------------
// Detección y enrutado de aplicaciones
// ---------------------------------------------------------------------------------------------

/// Predeterminados de Windows que hemos puesto nosotros (para no repetir la llamada en cada vuelta).
#[derive(Default, PartialEq, Clone)]
struct Defaults {
    render: Option<String>,
    communications: Option<String>,
    capture: Option<String>,
}

/// Qué predeterminados queremos: Juego para todo, Chat para comunicaciones y el micro virtual
/// como micrófono. Solo dispositivos que existan ahora mismo.
fn wanted_defaults(s: &Settings, devices: &Devices) -> Defaults {
    let exists = |id: &Option<String>| id.clone().filter(|id| devices.outputs.iter().any(|d| &d.id == id));
    // Sin dispositivo de Juego, la salida silenciosa (modo sin dispositivos virtuales).
    let render = exists(&s.channel(Channel::Game).device).or_else(|| exists(&s.sink_device));
    let communications = exists(&s.channel(Channel::Chat).device).or_else(|| render.clone());
    let capture = if s.mic.enabled { vdev::virtual_mic_input(s, &devices.outputs, &devices.inputs) } else { None };
    Defaults { render, communications, capture }
}

fn apply_defaults(want: &Defaults, applied: &mut Defaults) {
    if want.render != applied.render {
        if let Some(id) = &want.render {
            let _ = routing::set_default_device(id, &[eConsole, eMultimedia]);
        }
        applied.render = want.render.clone();
    }
    if want.communications != applied.communications {
        if let Some(id) = &want.communications {
            let _ = routing::set_default_device(id, &[eCommunications]);
        }
        applied.communications = want.communications.clone();
    }
    if want.capture != applied.capture {
        if let Some(id) = &want.capture {
            let _ = routing::set_default_device(id, &routing::ALL_ROLES);
        }
        applied.capture = want.capture.clone();
    }
}

/// Con el micro virtual apagado nadie escribe en su cable: si Windows lo sigue teniendo como
/// micrófono (de una sesión anterior o de cuando estaba encendido), Discord y compañía graban
/// silencio. Se devuelve al micro real.
fn release_silent_mic(s: &Settings, devices: &Devices) {
    if s.mic.enabled {
        return;
    }
    let Some(virt) = vdev::virtual_mic_input(s, &devices.outputs, &devices.inputs) else { return };
    let Some(real) = vdev::pick_real(&devices.inputs, &[], s.mic.input_device.as_deref()) else { return };
    for role in routing::ALL_ROLES {
        if wasapi::default_device_id(eCapture, role).as_deref() == Some(virt.as_str()) {
            let _ = routing::set_default_device(&real, &[role]);
        }
    }
}

/// Devuelve los predeterminados de Windows a los dispositivos reales: si al cerrar se quedara un
/// dispositivo virtual como predeterminado, no se oiría nada (ya nadie lo captura).
fn restore_defaults(shared: &Shared, settings: &Settings, devices: &Devices) {
    let virtual_ids = settings.virtual_device_ids();
    let real_out = shared
        .active_output
        .read()
        .clone()
        .or_else(|| vdev::pick_real(&devices.outputs, &virtual_ids, settings.output_device.as_deref()));
    if let Some(real) = real_out {
        for role in routing::ALL_ROLES {
            if wasapi::default_device_id(eRender, role).is_some_and(|id| virtual_ids.contains(&id)) {
                let _ = routing::set_default_device(&real, &[role]);
            }
        }
    }
    let virtual_in = vdev::virtual_mic_input(settings, &devices.outputs, &devices.inputs);
    let real_in = shared
        .active_mic
        .read()
        .clone()
        .or_else(|| vdev::pick_real(&devices.inputs, &[], settings.mic.input_device.as_deref()));
    if let (Some(virt), Some(real)) = (virtual_in, real_in) {
        for role in routing::ALL_ROLES {
            if wasapi::default_device_id(eCapture, role).as_deref() == Some(virt.as_str()) {
                let _ = routing::set_default_device(&real, &[role]);
            }
        }
    }
}

fn list_all_devices() -> Devices {
    Devices {
        outputs: wasapi::list_devices(eRender).unwrap_or_default(),
        inputs: wasapi::list_devices(eCapture).unwrap_or_default(),
    }
}

fn session_manager(shared: Arc<Shared>, sources: SourceList) {
    let _com = ComGuard::init();
    // Avisos de Windows: el hilo duerme hasta que pasa algo (o como mucho unos segundos).
    let waker = shared.wake.clone();
    let devices_dirty = Arc::new(AtomicBool::new(true));
    let _device_watch = notify::DeviceWatcher::new(waker.clone(), devices_dirty.clone()).ok();
    let mut watchers: HashMap<String, notify::SessionWatcher> = HashMap::new();
    // Última vez que el dispositivo de cada canal tuvo alguna app sonando.
    let mut last_active: HashMap<Channel, Instant> = HashMap::new();
    let mut last_devices: Option<Instant> = None;
    let mut last_scan: Option<Instant> = None;
    let mut last_conflict: Option<(Instant, Option<String>)> = None;
    let mut had_apps = false;
    let mut timeout = Duration::ZERO;
    let mut procs_active: HashMap<u32, Arc<Source>> = HashMap::new();
    let mut device_sources: HashMap<Channel, Arc<Source>> = HashMap::new();
    // Último dispositivo que le pedimos a Windows para cada pid (None = predeterminado).
    let mut applied: HashMap<u32, Option<String>> = HashMap::new();
    let mut routing_errors: HashMap<u32, String> = HashMap::new();
    // true en cuanto cambiamos la salida de alguna app: al cerrar hay que deshacerlo.
    let mut routed_any = false;
    let mut defaults = Defaults::default();
    let mut managing = false;

    while !shared.quit.load(Ordering::Relaxed) {
        let woke = waker.wait(timeout);
        if shared.quit.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        // La lista de dispositivos solo se rehace cuando Windows avisa de un cambio (o cada minuto por si acaso).
        if devices_dirty.swap(false, Ordering::Relaxed) || last_devices.is_none_or(|t| now.duration_since(t) > Duration::from_secs(60)) {
            *shared.devices.write() = list_all_devices();
            last_devices = Some(now);
        }

        let settings = shared.snapshot();
        let devices = shared.devices.read().clone();
        // Un dispositivo configurado que ya no existe (driver desinstalado, programa que lo creaba
        // cerrado…) cuenta como "sin dispositivo": el canal pasa a capturar cada app por separado.
        let channel_device: Vec<Option<String>> = settings
            .channels
            .iter()
            .map(|c| c.device.clone().filter(|id| devices.outputs.iter().any(|d| &d.id == id)))
            .collect();
        let mut changed = false;
        for (flag, dev) in shared.device_ok.iter().zip(&channel_device) {
            changed |= flag.swap(dev.is_some(), Ordering::Relaxed) != dev.is_some();
        }
        if changed {
            shared.version.fetch_add(1, Ordering::Release); // el mezclador recalcula ganancias
        }

        // Otro mezclador usando los mismos dispositivos virtuales = audio doble y enrutado loco.
        // Vale también sin configurar: con GG abierto, las apps ya suenan a través de él.
        let uses_steelseries = devices.outputs.iter().any(|d| vdev::is_steelseries(&d.name));
        // Recorrer los procesos cuesta: como mucho cada 3 s.
        let conflict = match &last_conflict {
            Some((t, c)) if now.duration_since(*t) < Duration::from_secs(3) => c.clone(),
            _ => {
                let c = (uses_steelseries && wasapi::process_running(&vdev::STEELSERIES_PROCESSES)).then(|| {
                    tr!("SteelSeries GG está abierto y usa los mismos dispositivos virtuales. Rust Sonar está en pausa para no duplicar el audio: cierra GG para empezar.", "SteelSeries GG is open and uses the same virtual devices. Rust Sonar is paused so audio isn't doubled: close GG to start.")
                        .to_string()
                });
                last_conflict = Some((now, c.clone()));
                c
            }
        };
        if shared.status.read().conflict != conflict {
            shared.status.write().conflict = conflict;
        }

        // En pausa no se toca nada: ni capturas, ni enrutado, ni predeterminados. El otro programa
        // sigue mandando y aquí no suena nada hasta que se cierre.
        let paused = shared.status.read().conflict.is_some();
        shared.paused.store(paused, Ordering::Relaxed);
        if paused {
            for src in device_sources.values().chain(procs_active.values()) {
                src.stop.store(true, Ordering::Relaxed);
            }
            device_sources.clear();
            procs_active.clear();
            applied.clear();
            // Los predeterminados y el enrutado pasan a ser cosa del otro programa.
            defaults = Defaults::default();
            managing = false;
            routed_any = false;
            sources.write().clear();
            shared.apps.write().clear();
            watchers.clear();
            shared.mic_wanted.store(false, Ordering::Relaxed);
            shared.out_wake.wake();
            timeout = Duration::from_secs(1);
            continue;
        }

        // 0) Predeterminados de Windows. Solo cuando la mezcla ya sale por unos auriculares reales:
        //    si no, mandar todo el sonido a un dispositivo virtual dejaría el PC mudo.
        let output_ok = shared.active_output.read().is_some();
        if settings.manage_defaults && output_ok {
            apply_defaults(&wanted_defaults(&settings, &devices), &mut defaults);
            release_silent_mic(&settings, &devices);
            // Solo "gestionamos" si de verdad hemos puesto algún predeterminado: si no, al salir
            // no hay nada nuestro que deshacer.
            managing = defaults != Defaults::default();
        } else if managing && !settings.manage_defaults {
            restore_defaults(&shared, &settings, &devices);
            defaults = Defaults::default();
            managing = false;
        }

        // ¿Quién suena (o graba) en nuestros dispositivos? Lo dicen los avisos de sesión de Windows.
        let mic_input = if settings.mic.enabled { vdev::virtual_mic_input(&settings, &devices.outputs, &devices.inputs) } else { None };
        let mut watched: Vec<(String, EDataFlow)> = channel_device.iter().flatten().map(|id| (id.clone(), eRender)).collect();
        if let Some(id) = &mic_input {
            watched.push((id.clone(), eCapture));
        }
        watchers.retain(|id, _| watched.iter().any(|(w, _)| w == id));
        for (id, flow) in &watched {
            if !watchers.contains_key(id) {
                if let Ok(w) = notify::SessionWatcher::new(id, *flow, waker.clone()) {
                    watchers.insert(id.clone(), w);
                }
            }
        }
        let active: HashMap<String, bool> = watchers.iter_mut().map(|(id, w)| (id.clone(), w.refresh())).collect();
        // Sin vigilante (Windows no dejó suscribirse) se captura siempre, como antes.
        let is_active = |id: &String| active.get(id).copied().unwrap_or(true);

        // 1) Captura de cada dispositivo virtual de canal, solo mientras alguna app suena en él (y
        //    unos segundos más). Sin capturas abiertas, Windows no procesa nada para Rust Sonar.
        for ch in Channel::ALL {
            match &channel_device[ch.index()] {
                Some(id) if is_active(id) => {
                    last_active.insert(ch, now);
                }
                Some(_) => {}
                None => {
                    last_active.remove(&ch);
                }
            }
        }
        let wanted = |ch: Channel| last_active.get(&ch).is_some_and(|t| now.duration_since(*t) < CAPTURE_HANG);
        let hanging = Channel::ALL.iter().any(|&ch| wanted(ch) && channel_device[ch.index()].as_ref().is_some_and(|id| !is_active(id)));
        device_sources.retain(|ch, src| {
            let keep = channel_device[ch.index()].as_ref() == src.device.as_ref() && wanted(*ch);
            if !keep {
                src.stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for ch in Channel::ALL {
            if let Some(id) = &channel_device[ch.index()] {
                if wanted(ch) && !device_sources.contains_key(&ch) {
                    let src = Source::new(0, Some(id.clone()), trf!("dispositivo {}", "device {}", ch.label()), ch);
                    let (s2, sh) = (src.clone(), shared.clone());
                    thread::Builder::new()
                        .name(format!("disp-{}", ch.label()))
                        .spawn(move || device_capture_thread(s2, sh))
                        .unwrap();
                    device_sources.insert(ch, src);
                }
            }
        }

        // 2) Aplicaciones que suenan: al recibir un aviso o cada pocos segundos.
        if woke || last_scan.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(3)) {
        last_scan = Some(now);
        let mut procs = wasapi::list_session_processes().unwrap_or_default();
        procs.retain(|p| !settings.is_ignored(&p.name));

        // La primera vez que vemos una app adivinamos su canal y lo guardamos.
        for p in &procs {
            let key = p.name.to_ascii_lowercase();
            if !settings.app_routes.contains_key(&key) {
                if let Some(ch) = Channel::guess_for_app(&p.name) {
                    shared.update(|s| {
                        s.app_routes.entry(key).or_insert(ch);
                    });
                }
            }
        }
        let settings = shared.snapshot();

        let active_output = shared.active_output.read().clone();
        let mut infos = Vec::new();
        for p in &procs {
            let ch = settings.route_for(&p.name);
            let target = channel_device[ch.index()].clone();

            // Enrutado real en Windows. Si nunca hemos tocado la app y su canal no tiene
            // dispositivo, no cambiamos nada (respetamos lo que el usuario tuviera puesto).
            let needs_change = match (&target, applied.get(&p.pid)) {
                (None, None) => false,
                (t, Some(prev)) => t != prev,
                (Some(_), None) => true,
            };
            if needs_change {
                match routing::set_app_output(p.pid, target.as_deref()) {
                    Ok(()) => {
                        routed_any = true;
                        routing_errors.remove(&p.pid);
                    }
                    Err(e) => {
                        routing_errors.insert(p.pid, format!("{e:#}"));
                    }
                }
                applied.insert(p.pid, target.clone());
            }

            // Captura por proceso solo si el canal no tiene dispositivo virtual (si lo tiene, el
            // audio ya llega por la captura del dispositivo) y la app no suena ya directamente en
            // nuestra salida (capturarla la haría sonar dos veces).
            let direct = target.is_none() && active_output.as_ref().is_some_and(|out| p.devices.contains(out));
            if target.is_none() && !direct {
                let src = procs_active.entry(p.pid).or_insert_with(|| {
                    let src = Source::new(p.pid, None, p.name.clone(), ch);
                    let s2 = src.clone();
                    thread::Builder::new()
                        .name(format!("captura-{}", p.pid))
                        .spawn(move || capture_thread(s2))
                        .unwrap();
                    src
                });
                src.channel.store(ch.index() as u8, Ordering::Relaxed);
            } else if let Some(src) = procs_active.remove(&p.pid) {
                src.stop.store(true, Ordering::Relaxed);
            }

            let src = procs_active.get(&p.pid);
            infos.push(AppInfo {
                pid: p.pid,
                name: p.name.clone(),
                channel: ch,
                peak: src.map(|s| s.peak.load()).unwrap_or(p.peak),
                error: routing_errors.get(&p.pid).cloned().or_else(|| src.and_then(|s| s.error.lock().clone())),
                via_device: target.is_some(),
                direct,
            });
        }

        // Apps que ya no suenan: paramos su captura y olvidamos su estado.
        procs_active.retain(|pid, src| {
            let alive = procs.iter().any(|p| p.pid == *pid);
            if !alive {
                src.stop.store(true, Ordering::Relaxed);
            }
            alive
        });
        applied.retain(|pid, _| procs.iter().any(|p| p.pid == *pid));
        routing_errors.retain(|pid, _| procs.iter().any(|p| p.pid == *pid));

        infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.pid.cmp(&b.pid)));
        had_apps = !infos.is_empty();
        *shared.apps.write() = infos;
        }

        // 3) El micro solo se procesa si alguien lo usa: una app grabando del micro virtual, la
        //    ventana abierta (vúmetros), «Escucharme» o la retransmisión. Así tampoco sale el aviso
        //    de micro en uso.
        let ui_wants = shared.ui_mic_until.load(Ordering::Relaxed) > crate::state::unix_ms();
        let recording = mic_input.as_ref().is_some_and(|id| is_active(id));
        let streaming = settings.stream_mic_gain() > 0.0;
        let mic_wanted = settings.mic.enabled && (recording || ui_wants || settings.mic.monitor || streaming);
        if shared.mic_wanted.swap(mic_wanted, Ordering::Relaxed) != mic_wanted {
            shared.mic_wake.wake();
        }

        let mut list: Vec<Arc<Source>> = procs_active.values().cloned().collect();
        list.extend(device_sources.values().cloned());
        let changed = {
            let current = sources.read();
            current.len() != list.len() || current.iter().zip(&list).any(|(a, b)| !Arc::ptr_eq(a, b))
        };
        if changed {
            *sources.write() = list;
            shared.out_wake.wake(); // el mezclador abre o suelta la salida
        }

        // Hasta el próximo repaso (los avisos de Windows lo adelantan).
        timeout = if hanging || ui_wants {
            Duration::from_millis(500)
        } else if had_apps || !device_sources.is_empty() || !procs_active.is_empty() {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(10)
        };
    }

    // Al salir hay que dejar Windows usable sin Rust Sonar:
    // - Las salidas por aplicación vuelven al predeterminado. Windows las recuerda por programa, así
    //   que no basta con restaurar las apps abiertas: una que se cerró antes volvería a sonar en un
    //   dispositivo virtual que ya nadie escucha.
    // - Los predeterminados vuelven a los auriculares y al micro reales.
    let settings = shared.snapshot();
    if routed_any {
        let _ = routing::clear_all_app_outputs();
    }
    // En pausa manda el otro mezclador: sus predeterminados no se tocan.
    if managing && !shared.paused.load(Ordering::Relaxed) {
        restore_defaults(&shared, &settings, &list_all_devices());
    }
    shared.cleanup_done.store(true, Ordering::Release);
}

/// Mantiene iguales el volumen/silencio de cada canal y el de su dispositivo virtual en Windows.
///
/// La captura en loopback llega ya atenuada por el volumen del dispositivo, así que ese volumen ES
/// el volumen del canal: las teclas de volumen del teclado (que mueven el dispositivo predeterminado,
/// Juego) y el mezclador de Windows controlan el canal, y el fader de Rust Sonar mueve el de Windows.
///
/// En modo streamer no puede ser así: la retransmisión necesita el canal entero aunque tú lo tengas
/// bajo o silenciado. Los dispositivos de canal se quedan al 100 % (si algo los mueve en Windows, se
/// devuelven) y los dos volúmenes del canal los pone el mezclador.
fn volume_sync(shared: Arc<Shared>) {
    let _com = ComGuard::init();
    // Ranuras 0-3: canales con dispositivo virtual. Ranura 4: Master ↔ salida silenciosa (en el modo
    // sin dispositivos virtuales es la predeterminada, así que las teclas de volumen mueven el Master).
    // Cada una: (id del dispositivo, interfaz, último valor sincronizado).
    let mut state: [Option<(String, EndpointVolume, (f32, bool))>; 5] = Default::default();
    // Windows avisa de cada cambio de volumen; los cambios propios se ven por la versión de la configuración.
    let waker = Arc::new(notify::Waker::default());
    let mut seen_version = 0u64;
    let mut retry_at: Option<Instant> = None;

    while !shared.quit.load(Ordering::Relaxed) {
        let woke = waker.wait(Duration::from_millis(250));
        if shared.paused.load(Ordering::Relaxed) {
            state = Default::default();
            continue;
        }
        let v = shared.version();
        let retry = retry_at.is_some_and(|t| Instant::now() >= t);
        if !woke && v == seen_version && !retry {
            continue; // nada ha cambiado: ni una llamada a Windows
        }
        seen_version = v;
        retry_at = None;
        for i in 0..5 {
            let (device, ours, pinned) = {
                let s = shared.settings.read();
                let pinned = s.stream_active();
                match Channel::ALL.get(i) {
                    Some(&ch) if pinned => (s.channel(ch).device.clone(), (1.0, false), true),
                    Some(&ch) => (s.channel(ch).device.clone(), (s.channel(ch).volume, s.channel(ch).muted), false),
                    None if s.channel(Channel::Game).device.is_none() => (s.sink_device.clone(), (s.master_volume, s.master_muted), false),
                    None => (None, (0.0, false), false),
                }
            };
            let Some(device) = device else {
                state[i] = None;
                continue;
            };
            if state[i].as_ref().map(|(id, ..)| id != &device).unwrap_or(true) {
                // Dispositivo nuevo: adoptamos el volumen que tenga en Windows.
                state[i] = EndpointVolume::open_watched(&device, waker.clone()).ok().and_then(|ep| {
                    let now = ep.get().ok()?;
                    Some((device.clone(), ep, (f32::NAN, now.1)))
                });
            }
            let Some((_, ep, last)) = state[i].as_mut() else {
                retry_at = Some(Instant::now() + Duration::from_secs(5));
                continue;
            };
            let Ok(theirs) = ep.get() else {
                state[i] = None; // el dispositivo desapareció; se reintenta
                retry_at = Some(Instant::now() + Duration::from_secs(5));
                continue;
            };
            let differs = |a: (f32, bool), b: (f32, bool)| (a.0 - b.0).abs() > 0.004 || a.1 != b.1;
            if last.0.is_nan() || differs(theirs, *last) {
                // Cambió en Windows (teclas de volumen, mezclador): manda Windows, salvo en modo
                // streamer, donde el dispositivo vuelve al 100 %.
                *last = theirs;
                if differs(theirs, ours) && pinned {
                    if ep.set(ours.0, ours.1).is_ok() {
                        *last = ours;
                    }
                } else if differs(theirs, ours) {
                    shared.update(|s| match Channel::ALL.get(i) {
                        Some(&ch) => {
                            let c = s.channel_mut(ch);
                            c.volume = theirs.0;
                            c.muted = theirs.1;
                        }
                        None => {
                            s.master_volume = theirs.0;
                            s.master_muted = theirs.1;
                        }
                    });
                }
            } else if differs(ours, *last) {
                // Cambió en Rust Sonar (fader, API): lo llevamos a Windows.
                if ep.set(ours.0, ours.1).is_ok() {
                    *last = ours;
                }
            }
        }
    }
}

fn capture_thread(src: Arc<Source>) {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let mut stream = match CaptureStream::process_loopback(src.pid) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[captura] {} ({}): {e:#}", src.name, src.pid);
            *src.error.lock() = Some(format!("{e:#}"));
            return;
        }
    };
    let mut scratch = Vec::new();
    while !src.stop.load(Ordering::Relaxed) {
        if let Err(e) = stream.read(100, &mut scratch, |samples| src.fifo.push(samples)) {
            *src.error.lock() = Some(format!("{e:#}"));
            break;
        }
    }
}

/// Captura (loopback) de todo lo que suena en un dispositivo virtual de canal, con sus canales
/// nativos (normalmente 7.1), y lo lleva a estéreo: mezcla simple o audio espacial.
/// Si el dispositivo desaparece (p. ej. se desinstala el driver), reintenta cada 2 s.
fn device_capture_thread(src: Arc<Source>, shared: Arc<Shared>) {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let id = src.device.clone().unwrap_or_default();
    let channel = src.channel();
    let mut scratch = Vec::new();
    let mut stereo = Vec::new();
    while !src.stop.load(Ordering::Relaxed) {
        let opened = wasapi::get_device(Some(&id), eRender).and_then(|(dev, _, _)| CaptureStream::loopback(&dev));
        let mut stream = match opened {
            Ok(s) => s,
            Err(e) => {
                *src.error.lock() = Some(format!("{e:#}"));
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        *src.error.lock() = None;
        let mut surround = Surround::new(stream.speakers(), SAMPLE_RATE as f32);
        let mut version = 0; // captura nueva: hay que volver a aplicarle los ajustes
        while !src.stop.load(Ordering::Relaxed) {
            let v = shared.version();
            if v != version {
                if let Some(s) = shared.settings.try_read() {
                    surround.configure(&s.channel(channel).surround);
                    version = v;
                }
            }
            let result = stream.read(100, &mut scratch, |samples| {
                surround.process(samples, &mut stereo);
                src.fifo.push(&stereo);
            });
            if let Err(e) = result {
                *src.error.lock() = Some(format!("{e:#}"));
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Mezcla y salida
// ---------------------------------------------------------------------------------------------

/// Estado del mezclador que vive solo en el hilo de audio (sin locks en el camino crítico).
struct Mixer {
    version: u64,
    eqs: Vec<Equalizer>,
    gains: Vec<SmoothedGain>,
    compressors: Vec<Compressor>,
    smart: [bool; 4],
    smart_params: [CompressorParams; 4],
    /// Audio espacial de los canales sin dispositivo virtual (sobre el estéreo de las apps). Con
    /// dispositivo lo hace el hilo de captura, que tiene los canales 5.1/7.1 por separado.
    surrounds: Vec<Surround>,
    stereo: Vec<f32>,
    device_ok: [bool; 4],
    master_gain: SmoothedGain,
    monitor_gain: SmoothedGain,
    targets: [f32; 4],
    master_target: f32,
    monitor_target: f32,
    monitor_on: bool,
    buses: Vec<Vec<f32>>,
    tmp: Vec<f32>,
    mono: Vec<f32>,
    /// Mezcla de retransmisión (modo streamer).
    stream_on: bool,
    stream_gains: Vec<SmoothedGain>,
    stream_targets: [f32; 4],
    stream_master: SmoothedGain,
    stream_master_target: f32,
    stream_mic: SmoothedGain,
    stream_mic_target: f32,
    stream_buf: Vec<f32>,
    send: Vec<f32>,
}

impl Mixer {
    fn new() -> Self {
        let fs = SAMPLE_RATE as f32;
        Self {
            version: 0,
            eqs: (0..4).map(|_| Equalizer::new(2)).collect(),
            gains: (0..4).map(|_| SmoothedGain::new(fs, 0.015)).collect(),
            compressors: (0..4).map(|_| Compressor::new(fs)).collect(),
            smart: [false; 4],
            smart_params: [CompressorParams::SMART_VOLUME; 4],
            surrounds: (0..4)
                .map(|_| Surround::new(&[crate::dsp::surround::SPEAKER_FRONT_LEFT, crate::dsp::surround::SPEAKER_FRONT_RIGHT], fs))
                .collect(),
            stereo: Vec::new(),
            device_ok: [false; 4],
            master_gain: SmoothedGain::new(fs, 0.015),
            monitor_gain: SmoothedGain::new(fs, 0.015),
            targets: [0.0; 4],
            master_target: 0.0,
            monitor_target: 0.0,
            monitor_on: false,
            buses: vec![Vec::new(); 4],
            tmp: Vec::new(),
            mono: Vec::new(),
            stream_on: false,
            stream_gains: (0..4).map(|_| SmoothedGain::new(fs, 0.015)).collect(),
            stream_targets: [0.0; 4],
            stream_master: SmoothedGain::new(fs, 0.015),
            stream_master_target: 0.0,
            stream_mic: SmoothedGain::new(fs, 0.015),
            stream_mic_target: 0.0,
            stream_buf: Vec::new(),
            send: Vec::new(),
        }
    }

    fn apply_settings(&mut self, s: &Settings, shared: &Shared) {
        for (mine, flag) in self.device_ok.iter_mut().zip(&shared.device_ok) {
            *mine = flag.load(Ordering::Relaxed);
        }
        for ch in Channel::ALL {
            let cs = s.channel(ch);
            self.eqs[ch.index()].configure(&cs.eq);
            self.smart[ch.index()] = cs.smart_volume;
            self.smart_params[ch.index()] = CompressorParams::smart(cs.smart_loudness, cs.smart_level);
            self.surrounds[ch.index()].configure(&cs.surround);
            self.targets[ch.index()] = channel_gain(s, ch, self.device_ok[ch.index()]);
            self.stream_targets[ch.index()] = s.stream_gain(ch);
        }
        self.master_target = if s.master_muted { 0.0 } else { s.master_volume };
        self.monitor_on = s.mic.enabled && s.mic.monitor;
        self.monitor_target = if self.monitor_on { s.mic.monitor_volume } else { 0.0 };
        self.stream_on = s.stream_active();
        self.stream_master_target = s.stream_master_gain();
        self.stream_mic_target = s.stream_mic_gain();
    }

    fn render(&mut self, out: &mut [f32], shared: &Shared, sources: &[Arc<Source>]) {
        // Solo recalculamos si la configuración cambió, y con try_read para no bloquear nunca el audio.
        let v = shared.version();
        if v != self.version {
            if let Some(s) = shared.settings.try_read() {
                self.apply_settings(&s, shared);
                self.version = v;
            }
        }

        let n = out.len();
        for bus in &mut self.buses {
            bus.clear();
            bus.resize(n, 0.0);
        }
        self.tmp.resize(n, 0.0);

        for src in sources {
            src.fifo.pop_into(&mut self.tmp);
            src.peak.store(peak(&self.tmp));
            let bus = &mut self.buses[src.channel().index()];
            for (b, x) in bus.iter_mut().zip(&self.tmp) {
                *b += *x;
            }
        }

        out.fill(0.0);
        if self.stream_on {
            self.stream_buf.clear();
            self.stream_buf.resize(n, 0.0);
            self.send.resize(n, 0.0);
        }
        for i in 0..4 {
            let bus = &mut self.buses[i];
            if !self.device_ok[i] {
                self.surrounds[i].process(bus, &mut self.stereo);
                bus.copy_from_slice(&self.stereo);
            }
            self.eqs[i].process(bus);
            self.compressors[i].process(bus, 2, &self.smart_params[i], self.smart[i]);
            // Envío a la retransmisión: el canal ya ecualizado, antes de su volumen personal.
            if self.stream_on {
                self.send.copy_from_slice(bus);
                self.stream_gains[i].apply(&mut self.send, 2, self.stream_targets[i]);
                shared.meters.stream_channels[i].store(peak(&self.send));
                for (o, x) in self.stream_buf.iter_mut().zip(&self.send) {
                    *o += *x;
                }
            }
            self.gains[i].apply(bus, 2, self.targets[i]);
            shared.meters.channels[i].store(peak(bus));
            for (o, x) in out.iter_mut().zip(bus.iter()) {
                *o += *x;
            }
        }
        self.master_gain.apply(out, 2, self.master_target);

        // Escucha del propio micro: va después del master para que su volumen sea independiente.
        if self.monitor_on {
            self.mono.resize(n / 2, 0.0);
            shared.monitor.pop_into(&mut self.mono);
            self.tmp.clear();
            self.tmp.extend(self.mono.iter().flat_map(|x| [*x, *x]));
            self.monitor_gain.apply(&mut self.tmp, 2, self.monitor_target);
            for (o, x) in out.iter_mut().zip(&self.tmp) {
                *o += *x;
            }
        }

        for x in out.iter_mut() {
            *x = soft_clip(*x);
        }
        shared.meters.master.store(peak(out));

        if self.stream_on {
            self.stream_master.apply(&mut self.stream_buf, 2, self.stream_master_target);
            // El micro va después del volumen general de la retransmisión, como la escucha en la
            // personal: bajar la mezcla no baja tu voz.
            self.mono.resize(n / 2, 0.0);
            shared.stream_mic.pop_into(&mut self.mono);
            self.tmp.clear();
            self.tmp.extend(self.mono.iter().flat_map(|x| [*x, *x]));
            self.stream_mic.apply(&mut self.tmp, 2, self.stream_mic_target);
            for (o, x) in self.stream_buf.iter_mut().zip(&self.tmp) {
                *o = soft_clip(*o + *x);
            }
            shared.meters.stream.store(peak(&self.stream_buf));
            shared.stream_out.push(&self.stream_buf);
        } else if self.stream_buf.len() > 0 {
            // Recién apagada: los vúmetros a cero una vez.
            self.stream_buf.clear();
            shared.meters.stream.store(0.0);
            for m in &shared.meters.stream_channels {
                m.store(0.0);
            }
        }
    }
}

/// Ganancia que aplica el mezclador a un canal en la mezcla personal.
///
/// Con dispositivo virtual el volumen ya lo ha aplicado Windows en el propio dispositivo (ver
/// `volume_sync`), así que aquí solo quedan el ChatMix y el refuerzo. En modo streamer el
/// dispositivo está al 100 % y el volumen se aplica aquí.
fn channel_gain(s: &Settings, ch: Channel, device_present: bool) -> f32 {
    let cs = s.channel(ch);
    if cs.muted {
        return 0.0;
    }
    let volume = if device_present && !s.stream_active() { 1.0 } else { cs.volume };
    volume * s.chatmix_gain(ch) * db_to_lin(cs.boost_db.clamp(-12.0, 12.0))
}

/// Abre el dispositivo real de salida. Con «Predeterminado de Windows», si el predeterminado es un
/// dispositivo virtual (lo normal: Juego), busca unos auriculares reales en vez de fallar.
fn resolve_output(wanted: Option<&str>, virtual_ids: &[String]) -> anyhow::Result<(IMMDevice, String, String)> {
    let (mut dev, mut name, mut id) = wasapi::get_device(wanted, eRender)?;
    if virtual_ids.contains(&id) || (wanted.is_none() && vdev::is_virtual_cable(&name)) {
        if wanted.is_some() {
            // Si la salida fuera uno de nuestros dispositivos virtuales, el audio se capturaría a sí
            // mismo en bucle.
            anyhow::bail!(trf!("«{name}» es un dispositivo virtual; elige tus auriculares reales en Salida", "“{name}” is a virtual device; pick your real headphones under Output"));
        }
        let outputs = wasapi::list_devices(eRender)?;
        let real = vdev::pick_real(&outputs, virtual_ids, None)
            .ok_or_else(|| anyhow::anyhow!(trf!("no hay auriculares o altavoces reales conectados", "no real headphones or speakers connected")))?;
        (dev, name, id) = wasapi::get_device(Some(&real), eRender)?;
    }
    Ok((dev, name, id))
}

fn output_loop(shared: Arc<Shared>, sources: SourceList) {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let mut mixer = Mixer::new();

    while !shared.quit.load(Ordering::Relaxed) {
        let (wanted, virtual_ids) = {
            let s = shared.settings.read();
            (s.output_device.clone(), s.virtual_device_ids())
        };
        let (device, name) = match resolve_output(wanted.as_deref(), &virtual_ids) {
            Ok((device, name, id)) => {
                *shared.active_output.write() = Some(id);
                (device, name)
            }
            Err(e) => {
                *shared.active_output.write() = None;
                shared.status.write().output = format!("Error: {e:#}");
                shared.out_wake.wait(Duration::from_secs(1));
                continue;
            }
        };
        shared.status.write().output = trf!("Sonando en: {name}", "Playing on: {name}");

        // La salida solo se abre cuando hay algo que mezclar. Sin fuentes se suelta: Windows deja de
        // procesar audio para Rust Sonar y este hilo duerme hasta que el de sesiones lo despierte.
        let mut stream: Option<RenderStream> = None;
        loop {
            let changed = {
                let s = shared.settings.read();
                s.output_device != wanted || s.virtual_device_ids() != virtual_ids
            };
            if shared.quit.load(Ordering::Relaxed) || changed {
                break; // reabrimos con el dispositivo nuevo
            }
            let list = sources.read().clone();
            // El micro también hace sonar algo sin apps: en la escucha o en la retransmisión.
            let mic_only = shared.mic_wanted.load(Ordering::Relaxed) && {
                let s = shared.settings.read();
                s.mic.enabled && (s.mic.monitor || s.stream_mic_gain() > 0.0)
            };
            if list.is_empty() && !mic_only {
                if stream.take().is_some() {
                    shared.meters.master.store(0.0);
                    for m in shared.meters.channels.iter().chain(&shared.meters.stream_channels) {
                        m.store(0.0);
                    }
                }
                set_idle(&shared, true);
                shared.out_wake.wait(Duration::from_secs(2));
                continue;
            }
            if stream.is_none() {
                match RenderStream::open(&device, 2) {
                    Ok(s) => {
                        // Lo que hubiera en las colas de antes ya no vale.
                        for src in list.iter() {
                            src.fifo.clear();
                        }
                        stream = Some(s);
                        set_idle(&shared, false);
                    }
                    Err(e) => {
                        shared.status.write().output = trf!("No se pudo abrir la salida ({e:#}); reintentando…", "Couldn't open the output ({e:#}); retrying…");
                        thread::sleep(Duration::from_millis(500));
                        break;
                    }
                }
            }
            let Some(out) = stream.as_mut() else { continue };
            if let Err(e) = out.write(50, |buf| mixer.render(buf, &shared, &list)) {
                // Típico: AUDCLNT_E_DEVICE_INVALIDATED al desconectar los auriculares.
                shared.status.write().output = trf!("Dispositivo perdido ({e:#}); reintentando…", "Device lost ({e:#}); retrying…");
                thread::sleep(Duration::from_millis(500));
                break;
            }
        }
    }
}

/// Marca si el mezclador está en reposo y, si cambia, avisa a la retransmisión (que va a su ritmo).
fn set_idle(shared: &Shared, idle: bool) {
    if shared.idle.swap(idle, Ordering::Relaxed) != idle {
        shared.stream_wake.wake();
    }
}

/// Saca la mezcla de retransmisión por su dispositivo. El ritmo lo marcan los auriculares (el
/// mezclador); este hilo lo lee de la cola, que absorbe la diferencia entre los dos relojes.
/// Solo tiene la salida abierta mientras el mezclador trabaja: en reposo no gasta nada.
fn stream_loop(shared: Arc<Shared>) {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let set_status = |text: String| {
        if shared.status.read().stream != text {
            shared.status.write().stream = text;
        }
    };
    let sleep = |text: String, secs: u64| {
        set_status(text);
        shared.stream_out.clear();
        shared.meters.stream.store(0.0);
        shared.stream_wake.wait(Duration::from_secs(secs));
    };
    while !shared.quit.load(Ordering::Relaxed) {
        let wanted = {
            let s = shared.settings.read();
            s.streamer.device.clone().filter(|_| s.stream_active())
        };
        let Some(id) = wanted else {
            sleep(String::new(), 10);
            continue;
        };
        if shared.paused.load(Ordering::Relaxed) || shared.idle.load(Ordering::Relaxed) {
            sleep(tr!("En reposo: se activa en cuanto haya algo que retransmitir", "Idle: wakes up as soon as there's something to stream").into(), 10);
            continue;
        }
        let opened = wasapi::get_device(Some(&id), eRender).and_then(|(dev, name, _)| Ok((RenderStream::open(&dev, 2)?, name)));
        let (mut out, name) = match opened {
            Ok(x) => x,
            Err(e) => {
                sleep(trf!("No se pudo abrir la salida de retransmisión ({e:#}); reintentando…", "Couldn't open the stream output ({e:#}); retrying…"), 2);
                continue;
            }
        };
        set_status(trf!("Retransmitiendo en: {name}", "Streaming to: {name}"));
        shared.stream_out.clear();
        let mut version = shared.version();
        while !shared.quit.load(Ordering::Relaxed) && !shared.paused.load(Ordering::Relaxed) && !shared.idle.load(Ordering::Relaxed) {
            let v = shared.version();
            if v != version {
                version = v;
                let s = shared.settings.read();
                if !s.stream_active() || s.streamer.device.as_deref() != Some(id.as_str()) {
                    break;
                }
            }
            if let Err(e) = out.write(50, |buf| {
                shared.stream_out.pop_into(buf);
            }) {
                set_status(trf!("Salida de retransmisión perdida ({e:#}); reintentando…", "Stream output lost ({e:#}); retrying…"));
                thread::sleep(Duration::from_millis(500));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_channels_leave_volume_to_windows() {
        let mut s = Settings::default();
        s.channel_mut(Channel::Media).volume = 0.25;
        assert_eq!(channel_gain(&s, Channel::Media, false), 0.25);
        // Con dispositivo virtual presente, el 25 % ya lo aplica Windows: aquí ganancia unidad.
        assert_eq!(channel_gain(&s, Channel::Media, true), 1.0);
        s.channel_mut(Channel::Media).boost_db = 6.0;
        assert!((channel_gain(&s, Channel::Media, true) - 1.9953).abs() < 1e-3);
        s.channel_mut(Channel::Media).muted = true;
        assert_eq!(channel_gain(&s, Channel::Media, true), 0.0);
    }

    #[test]
    fn streamer_mode_applies_personal_volume_in_the_mixer() {
        let mut s = Settings::default();
        s.channel_mut(Channel::Media).volume = 0.25;
        s.streamer.enabled = true;
        s.streamer.device = Some("obs".into());
        // El dispositivo está al 100 % en Windows: el volumen personal lo pone el mezclador.
        assert_eq!(channel_gain(&s, Channel::Media, true), 0.25);
        // Silenciar el canal para ti no lo quita de la retransmisión.
        s.channel_mut(Channel::Media).muted = true;
        assert_eq!(channel_gain(&s, Channel::Media, true), 0.0);
        assert_eq!(s.stream_gain(Channel::Media), 1.0);
    }

    /// Mide lo que suena en una salida (captura en loopback) durante 3 s. Sirve para comprobar que la
    /// mezcla de retransmisión llega a su dispositivo:
    /// `$env:RUST_SONAR_PROBE_OUT="{id}"; cargo test --release -- --ignored probe_output_level --nocapture`
    #[test]
    #[ignore]
    fn probe_output_level() {
        let _com = ComGuard::init();
        let id = std::env::var("RUST_SONAR_PROBE_OUT").expect("falta RUST_SONAR_PROBE_OUT con el ID de la salida");
        let (dev, name, _) = wasapi::get_device(Some(&id), eRender).unwrap();
        let mut cap = CaptureStream::loopback(&dev).unwrap();
        let (mut scratch, mut max, t) = (Vec::new(), 0f32, Instant::now());
        while t.elapsed() < Duration::from_secs(3) {
            cap.read(100, &mut scratch, |x| max = max.max(peak(x))).unwrap();
        }
        println!("{name}: pico {:.1} dBFS", crate::dsp::lin_to_db(max));
    }

    #[test]
    fn chatmix_still_applies_to_device_channels() {
        let s = Settings { chatmix: 0.5, ..Default::default() };
        assert_eq!(channel_gain(&s, Channel::Game, true), 0.5);
    }

    #[test]
    fn wanted_defaults_need_existing_devices() {
        let mut s = Settings::default();
        s.channel_mut(Channel::Game).device = Some("game".into());
        let none = wanted_defaults(&s, &Devices::default());
        assert_eq!(none.render, None);
        let devices = Devices {
            outputs: vec![crate::state::DeviceInfo { id: "game".into(), name: "SteelSeries Sonar - Gaming (X)".into() }],
            inputs: vec![],
        };
        let d = wanted_defaults(&s, &devices);
        assert_eq!(d.render.as_deref(), Some("game"));
        // Sin dispositivo de Chat, las comunicaciones también van a Juego.
        assert_eq!(d.communications.as_deref(), Some("game"));
        assert_eq!(d.capture, None);
    }

    #[test]
    fn sink_is_the_default_when_there_is_no_game_device() {
        let s = Settings { sink_device: Some("hd".into()), ..Default::default() };
        let devices = Devices {
            outputs: vec![crate::state::DeviceInfo { id: "hd".into(), name: "Altavoces (High Definition Audio Device)".into() }],
            inputs: vec![],
        };
        assert_eq!(wanted_defaults(&s, &devices).render.as_deref(), Some("hd"));
    }
}
