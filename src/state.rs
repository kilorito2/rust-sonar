//! Estado compartido entre el motor de audio, la interfaz y la API REST.

use crate::audio::fifo::Fifo;
use crate::audio::notify::Waker;
use std::sync::Arc;
use crate::config::{Channel, Settings};
use parking_lot::RwLock;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// f32 atómico (guardado como bits en un u32) para los vúmetros.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Meters {
    pub channels: [AtomicF32; 4],
    pub master: AtomicF32,
    pub mic_in: AtomicF32,
    pub mic_out: AtomicF32,
    pub mic_vad: AtomicF32,
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct MetersSnapshot {
    pub game: f32,
    pub chat: f32,
    pub media: f32,
    pub aux: f32,
    pub master: f32,
    pub mic_in: f32,
    pub mic_out: f32,
    pub mic_vad: f32,
}

impl Meters {
    pub fn snapshot(&self) -> MetersSnapshot {
        MetersSnapshot {
            game: self.channels[0].load(),
            chat: self.channels[1].load(),
            media: self.channels[2].load(),
            aux: self.channels[3].load(),
            master: self.master.load(),
            mic_in: self.mic_in.load(),
            mic_out: self.mic_out.load(),
            mic_vad: self.mic_vad.load(),
        }
    }

    /// Copia en los vúmetros locales lo que mide el motor (proceso de la interfaz).
    pub fn set(&self, m: &MetersSnapshot) {
        for (dst, v) in self.channels.iter().zip([m.game, m.chat, m.media, m.aux]) {
            dst.store(v);
        }
        self.master.store(m.master);
        self.mic_in.store(m.mic_in);
        self.mic_out.store(m.mic_out);
        self.mic_vad.store(m.mic_vad);
    }
}

/// Una aplicación que está reproduciendo audio (una sesión de WASAPI).
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct AppInfo {
    pub pid: u32,
    pub name: String,
    pub channel: Channel,
    pub peak: f32,
    /// None si todo va bien; Some(error) si Windows no dejó capturarla o enrutarla.
    pub error: Option<String>,
    /// true si su audio llega por el dispositivo virtual del canal (Windows la ha enrutado ahí).
    pub via_device: bool,
    /// true si suena directamente en los auriculares, fuera del mezclador (canal sin dispositivo
    /// virtual y la app reproduce en la misma salida que Rust Sonar): capturarla la duplicaría.
    pub direct: bool,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct Devices {
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct Status {
    pub output: String,
    pub mic: String,
    /// Aviso de conflicto con otro programa que usa los mismos dispositivos virtuales.
    pub conflict: Option<String>,
}

pub struct Shared {
    pub settings: RwLock<Settings>,
    /// Se incrementa en cada cambio; el hilo de audio lo compara para saber si debe recalcular.
    pub version: AtomicU64,
    pub dirty: AtomicBool,
    pub meters: Meters,
    pub apps: RwLock<Vec<AppInfo>>,
    pub devices: RwLock<Devices>,
    pub status: RwLock<Status>,
    pub quit: AtomicBool,
    /// El hilo de sesiones ya restauró el enrutado y los predeterminados de Windows.
    pub cleanup_done: AtomicBool,
    /// En pausa porque otro mezclador (SteelSeries GG) está usando los mismos dispositivos.
    pub paused: AtomicBool,
    /// Por canal: su dispositivo virtual está configurado y presente ahora mismo en Windows.
    pub device_ok: [AtomicBool; 4],
    /// ID del dispositivo real por el que está sonando la mezcla (None si no hay salida).
    pub active_output: RwLock<Option<String>>,
    /// ID del micrófono físico en uso.
    pub active_mic: RwLock<Option<String>>,
    /// Micro procesado hacia el mezclador, para escucharse a uno mismo (mono).
    pub monitor: Fifo,
    /// Despierta al hilo de sesiones (avisos de Windows, cambios desde la interfaz o la API).
    pub wake: Arc<Waker>,
    /// Despierta al mezclador cuando cambian las fuentes (para abrir o soltar la salida).
    pub out_wake: Arc<Waker>,
    /// Despierta al micro cuando pasa a hacer falta.
    pub mic_wake: Arc<Waker>,
    /// El micro hace falta: alguna app graba del micro virtual, la ventana está abierta o «Escucharme».
    pub mic_wanted: AtomicBool,
    /// Hasta cuándo (ms desde 1970) la ventana pide el micro activo para ver sus vúmetros.
    pub ui_mic_until: AtomicU64,
    /// Sin nada que mezclar: la salida está soltada y no se procesa audio.
    pub idle: AtomicBool,
    /// Proceso de la ventana: es una copia del estado del motor (que va en otro proceso) y no guarda en disco.
    pub mirror: bool,
    /// Proceso de la ventana: la ventana se ve (no está minimizada). Sin ella no se piden vúmetros.
    pub ui_visible: AtomicBool,
    /// Proceso de la ventana: la pestaña Micrófono está abierta (sus vúmetros necesitan el micro activo).
    pub ui_wants_mic: AtomicBool,
}

/// Milisegundos desde 1970 (para plazos compartidos entre hilos).
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Shared {
    pub fn new(settings: Settings) -> Self {
        crate::i18n::set(settings.language);
        Self {
            settings: RwLock::new(settings),
            version: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
            meters: Meters::default(),
            apps: RwLock::new(Vec::new()),
            devices: RwLock::new(Devices::default()),
            status: RwLock::new(Status::default()),
            quit: AtomicBool::new(false),
            cleanup_done: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            device_ok: Default::default(),
            active_output: RwLock::new(None),
            active_mic: RwLock::new(None),
            monitor: Fifo::new(1, crate::config::SAMPLE_RATE as usize * 20 / 1000),
            wake: Default::default(),
            out_wake: Default::default(),
            mic_wake: Default::default(),
            mic_wanted: AtomicBool::new(false),
            ui_mic_until: AtomicU64::new(0),
            idle: AtomicBool::new(false),
            mirror: false,
            ui_visible: AtomicBool::new(true),
            ui_wants_mic: AtomicBool::new(false),
        }
    }

    /// Estado del proceso de la ventana: se rellena desde el motor por la API local.
    pub fn mirror(settings: Settings) -> Self {
        Self { mirror: true, ..Self::new(settings) }
    }

    /// Única forma de modificar la configuración: avisa al motor y marca para guardar en disco.
    pub fn update<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> R {
        let r = {
            let mut s = self.settings.write();
            let r = f(&mut s);
            crate::i18n::set(s.language);
            r
        };
        self.version.fetch_add(1, Ordering::Release);
        self.dirty.store(true, Ordering::Relaxed);
        // Los hilos del motor duermen hasta que pasa algo: un cambio de configuración también cuenta.
        self.wake.wake();
        self.out_wake.wake();
        self.mic_wake.wake();
        r
    }

    /// Sustituye la configuración entera (la manda la ventana, que va en otro proceso). Solo avisa
    /// al motor si de verdad cambia algo.
    pub fn replace(&self, new: Settings) {
        if *self.settings.read() != new {
            self.update(|s| *s = new);
        }
    }

    pub fn snapshot(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn save_if_dirty(&self) {
        if self.mirror {
            return; // guarda el motor
        }
        if self.dirty.swap(false, Ordering::Relaxed) {
            if let Err(e) = self.snapshot().save() {
                eprintln!("[config] no se pudo guardar: {e:#}");
            }
        }
    }
}
