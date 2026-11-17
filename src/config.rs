//! Configuración persistente (se guarda como JSON en %APPDATA%\RustSonar\config.json).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Todo el motor trabaja a 48 kHz en float32; WASAPI convierte si el dispositivo usa otro formato.
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Channel {
    Game,
    Chat,
    Media,
    Aux,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Game, Channel::Chat, Channel::Media, Channel::Aux];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Channel {
        Self::ALL[i.min(3)]
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Game => tr!("Juego", "Game"),
            Channel::Chat => "Chat",
            Channel::Media => "Media",
            Channel::Aux => "Aux",
        }
    }

    /// Acepta "game", "juego", "0"... (para la API REST).
    pub fn parse(s: &str) -> Option<Channel> {
        match s.to_ascii_lowercase().as_str() {
            "game" | "juego" | "0" => Some(Channel::Game),
            "chat" | "1" => Some(Channel::Chat),
            "media" | "2" => Some(Channel::Media),
            "aux" | "3" => Some(Channel::Aux),
            _ => None,
        }
    }

    /// Canal por defecto según el nombre del ejecutable (heurística).
    pub fn guess_for_app(exe: &str) -> Option<Channel> {
        const CHAT: &[&str] = &[
            "discord", "discordptb", "discordcanary", "teamspeak", "ts3client_win64", "mumble",
            "skype", "zoom", "slack", "teams", "ms-teams", "whatsapp", "telegram", "signal",
        ];
        const MEDIA: &[&str] = &[
            "spotify", "chrome", "msedge", "firefox", "opera", "brave", "vivaldi", "vlc",
            "wmplayer", "music.ui", "itunes", "foobar2000", "mpv", "deezer", "tidal", "mpc-hc64",
        ];
        let exe = exe.to_ascii_lowercase();
        if CHAT.contains(&exe.as_str()) {
            Some(Channel::Chat)
        } else if MEDIA.contains(&exe.as_str()) {
            Some(Channel::Media)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl FilterKind {
    pub const ALL: [FilterKind; 6] = [
        FilterKind::Peaking,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::Notch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterKind::Peaking => tr!("Campana", "Bell"),
            FilterKind::LowShelf => tr!("Shelf graves", "Low shelf"),
            FilterKind::HighShelf => tr!("Shelf agudos", "High shelf"),
            FilterKind::LowPass => tr!("Paso bajo", "Low-pass"),
            FilterKind::HighPass => tr!("Paso alto", "High-pass"),
            FilterKind::Notch => "Notch",
        }
    }

    /// Los filtros de paso y el notch no usan ganancia.
    pub fn uses_gain(self) -> bool {
        matches!(self, FilterKind::Peaking | FilterKind::LowShelf | FilterKind::HighShelf)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled: bool,
    pub kind: FilterKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Los tres controles rápidos de tono que van bajo la gráfica (−12 … +12 dB).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tone {
    pub bass_db: f32,
    pub voice_db: f32,
    pub treble_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqSettings {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
    #[serde(default)]
    pub tone: Tone,
}

pub const EQ_FREQS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
pub const EQ_PRESETS: [&str; 6] = ["Plano", "Graves+", "Pasos (FPS)", "Voz clara", "Agudos+", "Nocturno"];

/// Nombre visible de un preset. Los integrados se guardan siempre con su nombre en español (es su
/// clave en la configuración y en la API); los del usuario se muestran tal cual.
pub fn preset_label(name: &str) -> &str {
    match name {
        "Plano" => tr!("Plano", "Flat"),
        "Graves+" => tr!("Graves+", "Bass+"),
        "Pasos (FPS)" => tr!("Pasos (FPS)", "Footsteps (FPS)"),
        "Voz clara" => tr!("Voz clara", "Clear voice"),
        "Agudos+" => tr!("Agudos+", "Treble+"),
        "Nocturno" => tr!("Nocturno", "Night"),
        other => other,
    }
}

impl EqSettings {
    /// 10 bandas en octavas: shelf en los extremos y campanas en medio.
    pub fn flat() -> Self {
        let bands = EQ_FREQS
            .iter()
            .enumerate()
            .map(|(i, &freq)| EqBand {
                enabled: true,
                kind: match i {
                    0 => FilterKind::LowShelf,
                    9 => FilterKind::HighShelf,
                    _ => FilterKind::Peaking,
                },
                freq,
                gain_db: 0.0,
                q: 1.41,
            })
            .collect();
        EqSettings { enabled: true, preamp_db: 0.0, bands, tone: Tone::default() }
    }

    /// Los controles Graves / Voz / Agudos como bandas de EQ (shelf grave, campana ancha en la zona
    /// de la voz y shelf agudo). Se suman a las bandas de la gráfica.
    pub fn tone_bands(&self) -> [EqBand; 3] {
        let band = |kind, freq, gain_db, q| EqBand { enabled: true, kind, freq, gain_db, q };
        [
            band(FilterKind::LowShelf, 110.0, self.tone.bass_db, 0.7),
            band(FilterKind::Peaking, 2200.0, self.tone.voice_db, 0.6),
            band(FilterKind::HighShelf, 7000.0, self.tone.treble_db, 0.7),
        ]
    }

    /// Todas las bandas que se procesan: las de la gráfica y las de tono.
    pub fn all_bands(&self) -> impl Iterator<Item = EqBand> + '_ {
        self.bands.iter().cloned().chain(self.tone_bands())
    }

    pub fn preset(name: &str) -> Option<Self> {
        // Ganancias en dB para las 10 bandas (31 Hz … 16 kHz).
        let gains: [f32; 10] = match name {
            "Plano" => [0.0; 10],
            "Graves+" => [6.0, 5.0, 3.5, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "Pasos (FPS)" => [-4.0, -3.0, -2.0, 0.0, 1.0, 2.0, 4.0, 5.0, 3.0, 0.0],
            "Voz clara" => [-6.0, -4.0, -2.0, 0.0, 1.0, 2.5, 3.5, 2.5, 0.0, -1.0],
            "Agudos+" => [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.5, 3.0, 4.5, 5.0],
            "Nocturno" => [-6.0, -4.0, -2.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -3.0],
            _ => return None,
        };
        let mut eq = Self::flat();
        for (band, g) in eq.bands.iter_mut().zip(gains) {
            band.gain_db = g;
        }
        // Evita que el refuerzo sature: compensa con el preamp.
        let max_boost = gains.iter().cloned().fold(0.0f32, f32::max);
        eq.preamp_db = -max_boost * 0.5;
        Some(eq)
    }
}

/// A qué se aplica un EQ/preset: uno de los 4 canales o el micrófono.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EqTarget {
    Channel(Channel),
    Mic,
}

impl EqTarget {
    #[allow(dead_code)] // usado en tests y útil para la API
    pub const ALL: [EqTarget; 5] = [
        EqTarget::Channel(Channel::Game),
        EqTarget::Channel(Channel::Chat),
        EqTarget::Channel(Channel::Media),
        EqTarget::Channel(Channel::Aux),
        EqTarget::Mic,
    ];

    /// Clave estable para mapas JSON y para la API.
    pub fn key(self) -> &'static str {
        match self {
            EqTarget::Channel(Channel::Game) => "game",
            EqTarget::Channel(Channel::Chat) => "chat",
            EqTarget::Channel(Channel::Media) => "media",
            EqTarget::Channel(Channel::Aux) => "aux",
            EqTarget::Mic => "mic",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EqTarget::Channel(ch) => ch.label(),
            EqTarget::Mic => tr!("Micro", "Mic"),
        }
    }

    pub fn parse(s: &str) -> Option<EqTarget> {
        match s.to_ascii_lowercase().as_str() {
            "mic" | "micro" => Some(EqTarget::Mic),
            other => Channel::parse(other).map(EqTarget::Channel),
        }
    }
}

/// Preset de EQ guardado por el usuario. Los favoritos aparecen como accesos rápidos en la tarjeta.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserPreset {
    pub name: String,
    pub target: EqTarget,
    pub favorite: bool,
    pub eq: EqSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelSettings {
    /// 0.0 … 1.0. Con dispositivo virtual es el volumen de ese dispositivo en Windows (así las
    /// teclas de volumen y el mezclador de Windows controlan el canal); sin él, ganancia lineal.
    pub volume: f32,
    pub muted: bool,
    pub eq: EqSettings,
    /// Dispositivo virtual del canal (ID WASAPI). None = captura por aplicación.
    pub device: Option<String>,
    /// Ganancia general del canal (−12 … +12 dB).
    pub boost_db: f32,
    /// Volumen inteligente: compresor que sube lo flojo y contiene lo fuerte.
    pub smart_volume: bool,
    /// Nivel extra del volumen inteligente (0 … 1).
    pub smart_level: f32,
    pub smart_loudness: Loudness,
    /// Audio espacial.
    pub surround: SpatialSettings,
}

/// Carácter del volumen inteligente.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Loudness {
    /// Contiene mucho lo fuerte (para no molestar de noche).
    Quiet,
    #[default]
    Balanced,
    /// Lo sube todo.
    Loud,
}

impl Loudness {
    pub const ALL: [Loudness; 3] = [Loudness::Quiet, Loudness::Balanced, Loudness::Loud];
    pub fn label(self) -> &'static str {
        match self {
            Loudness::Quiet => tr!("Silencioso", "Quiet"),
            Loudness::Balanced => tr!("Equilibrado", "Balanced"),
            Loudness::Loud => tr!("Alto", "Loud"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpatialMode {
    /// Para auriculares: cada altavoz virtual llega a cada oído con su retardo y su filtrado.
    #[default]
    Headphones,
    /// Para altavoces de verdad: sin simular la cabeza (ya la pones tú), solo reparto y sala.
    Speakers,
}

/// Un altavoz virtual: dónde está y cuánto suena.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeakerPos {
    /// Grados: 0 = delante, positivo = a la derecha, ±180 = detrás.
    pub azimuth: f32,
    pub gain_db: f32,
}

/// Orden de los altavoces en `SpatialSettings::speakers` (el de Windows para 7.1).
pub fn speaker_names() -> [&'static str; 8] {
    if crate::i18n::en() {
        ["Front left", "Front right", "Center", "Subwoofer", "Rear left", "Rear right", "Side left", "Side right"]
    } else {
        ["Frontal izq.", "Frontal der.", "Central", "Subwoofer", "Trasero izq.", "Trasero der.", "Lateral izq.", "Lateral der."]
    }
}
pub const DEFAULT_AZIMUTHS: [f32; 8] = [-30.0, 30.0, 0.0, 0.0, -150.0, 150.0, -90.0, 90.0];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpatialSettings {
    pub enabled: bool,
    pub mode: SpatialMode,
    /// 0 = Rendimiento (seco, máxima localización) … 1 = Inmersión (más sala).
    pub immersion: f32,
    /// 0 … 100: lo lejos que están los altavoces virtuales.
    pub distance: f32,
    pub speakers: [SpeakerPos; 8],
}

impl Default for SpatialSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SpatialMode::Headphones,
            immersion: 0.5,
            distance: 100.0,
            speakers: DEFAULT_AZIMUTHS.map(|azimuth| SpeakerPos { azimuth, gain_db: 0.0 }),
        }
    }
}

impl SpatialSettings {
    /// Nivel de la reverberación en dB: −12 (Rendimiento) … 0 (Inmersión).
    pub fn reverb_gain_db(&self) -> f32 {
        -12.0 + 12.0 * self.immersion.clamp(0.0, 1.0)
    }
    pub fn set_reverb_gain_db(&mut self, db: f32) {
        self.immersion = ((db + 12.0) / 12.0).clamp(0.0, 1.0);
    }
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
            eq: EqSettings::flat(),
            device: None,
            boost_db: 0.0,
            smart_volume: false,
            smart_level: 0.0,
            smart_loudness: Loudness::Balanced,
            surround: SpatialSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MicSettings {
    pub enabled: bool,
    /// ID WASAPI del micrófono (None = predeterminado de Windows).
    pub input_device: Option<String>,
    /// Dispositivo donde se envía el micro procesado (p. ej. "CABLE Input" de VB-Cable).
    pub output_device: Option<String>,
    pub gain_db: f32,
    pub volume: f32,
    pub muted: bool,
    /// Supresión de ruido con RNNoise (red neuronal recurrente).
    pub noise_suppression: bool,
    /// Silencia cuando la probabilidad de voz de RNNoise baja de `vad_threshold`.
    pub vad_gate: bool,
    pub vad_threshold: f32,
    /// Puerta de ruido clásica por nivel.
    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    pub gate_release_ms: f32,
    pub eq: EqSettings,
    /// Compresor de voz: iguala el nivel entre hablar bajo y alto.
    pub compressor: bool,
    /// Escucharte a ti mismo por los auriculares (para probar cómo suenas).
    pub monitor: bool,
    pub monitor_volume: f32,
}

impl Default for MicSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device: None,
            output_device: None,
            gain_db: 0.0,
            volume: 1.0,
            muted: false,
            noise_suppression: true,
            vad_gate: false,
            vad_threshold: 0.6,
            gate_enabled: false,
            gate_threshold_db: -45.0,
            gate_release_ms: 150.0,
            eq: EqSettings::flat(),
            compressor: false,
            monitor: false,
            monitor_volume: 0.7,
        }
    }
}

/// Envío de un canal (o del micro) a la mezcla de retransmisión.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamSend {
    pub volume: f32,
    pub muted: bool,
}

impl Default for StreamSend {
    fn default() -> Self {
        Self { volume: 1.0, muted: false }
    }
}

/// Modo streamer: además de la mezcla personal (la de tus auriculares) hay una segunda mezcla, la
/// de retransmisión, con su propio volumen por canal. Sale por otro dispositivo (normalmente un
/// cable virtual) que OBS o el programa de directo captura como fuente de audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamerSettings {
    pub enabled: bool,
    /// Salida de la mezcla de retransmisión (ID WASAPI). None = sin mezcla de retransmisión.
    pub device: Option<String>,
    /// Volumen general de la mezcla de retransmisión.
    pub volume: f32,
    pub muted: bool,
    pub channels: [StreamSend; 4],
    /// El micro procesado en la retransmisión (lo que oye tu público de tu voz).
    pub mic: StreamSend,
}

impl Default for StreamerSettings {
    fn default() -> Self {
        Self { enabled: false, device: None, volume: 1.0, muted: false, channels: Default::default(), mic: StreamSend::default() }
    }
}

/// Lo que se puede silenciar con un atajo (y lo que anuncia el aviso en pantalla).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MuteTarget {
    Mic,
    /// Todas las salidas (el master).
    Master,
    Channel(Channel),
    /// La mezcla de retransmisión entera (modo streamer).
    Stream,
}

impl MuteTarget {
    pub const ALL: [MuteTarget; 7] = [
        MuteTarget::Mic,
        MuteTarget::Master,
        MuteTarget::Channel(Channel::Game),
        MuteTarget::Channel(Channel::Chat),
        MuteTarget::Channel(Channel::Media),
        MuteTarget::Channel(Channel::Aux),
        MuteTarget::Stream,
    ];

    pub fn index(self) -> usize {
        match self {
            MuteTarget::Mic => 0,
            MuteTarget::Master => 1,
            MuteTarget::Channel(ch) => 2 + ch.index(),
            MuteTarget::Stream => 6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MuteTarget::Mic => tr!("Micrófono", "Microphone"),
            MuteTarget::Master => tr!("Todas las salidas", "All outputs"),
            MuteTarget::Channel(ch) => ch.label(),
            MuteTarget::Stream => tr!("Mezcla de retransmisión", "Stream mix"),
        }
    }

    pub fn is_muted(self, s: &Settings) -> bool {
        match self {
            MuteTarget::Mic => s.mic.muted,
            MuteTarget::Master => s.master_muted,
            MuteTarget::Channel(ch) => s.channel(ch).muted,
            MuteTarget::Stream => s.streamer.muted,
        }
    }

    /// Volumen (0 … 1) que muestra el aviso.
    pub fn volume(self, s: &Settings) -> f32 {
        match self {
            MuteTarget::Mic => s.mic.volume,
            MuteTarget::Master => s.master_volume,
            MuteTarget::Channel(ch) => s.channel(ch).volume,
            MuteTarget::Stream => s.streamer.volume,
        }
    }

    pub fn toggle(self, s: &mut Settings) {
        let m = match self {
            MuteTarget::Mic => &mut s.mic.muted,
            MuteTarget::Master => &mut s.master_muted,
            MuteTarget::Channel(ch) => &mut s.channel_mut(ch).muted,
            MuteTarget::Stream => &mut s.streamer.muted,
        };
        *m = !*m;
    }
}

/// Todo lo que se puede hacer con un atajo: silenciar algo o subir/bajar el volumen general de una
/// mezcla (la personal es el Master; la de retransmisión, la del modo streamer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyAction {
    Mute(MuteTarget),
    Volume { stream: bool, up: bool },
}

impl HotkeyAction {
    /// Paso de volumen de los atajos (5 %).
    pub const VOLUME_STEP: f32 = 0.05;

    pub fn all() -> impl Iterator<Item = HotkeyAction> {
        MuteTarget::ALL.into_iter().map(HotkeyAction::Mute).chain(
            [(false, false), (false, true), (true, false), (true, true)].into_iter().map(|(stream, up)| HotkeyAction::Volume { stream, up }),
        )
    }

    /// Identificador para `RegisterHotKey` (1 … 11).
    pub fn id(self) -> i32 {
        match self {
            HotkeyAction::Mute(t) => t.index() as i32 + 1,
            HotkeyAction::Volume { stream, up } => 8 + 2 * stream as i32 + up as i32,
        }
    }

    pub fn from_id(id: i32) -> Option<HotkeyAction> {
        Self::all().find(|a| a.id() == id)
    }

    /// Solo tiene sentido con el modo streamer activado.
    pub fn streamer_only(self) -> bool {
        matches!(self, HotkeyAction::Mute(MuteTarget::Stream) | HotkeyAction::Volume { stream: true, .. })
    }

    /// Lo hace. Devuelve qué anunciar en pantalla si es un cambio de volumen (los silencios los
    /// anuncia solo el aviso al ver que han cambiado).
    pub fn run(self, s: &mut Settings) -> Option<MuteTarget> {
        match self {
            HotkeyAction::Mute(t) => {
                t.toggle(s);
                None
            }
            HotkeyAction::Volume { stream, up } => {
                let (v, m, t) = if stream {
                    (&mut s.streamer.volume, &mut s.streamer.muted, MuteTarget::Stream)
                } else {
                    (&mut s.master_volume, &mut s.master_muted, MuteTarget::Master)
                };
                // Al siguiente múltiplo del paso (73 % → 75 % al subir, → 70 % al bajar), para que
                // los valores queden redondos.
                let steps = *v / Self::VOLUME_STEP;
                let next = if up { (steps + 1e-3).floor() + 1.0 } else { (steps - 1e-3).ceil() - 1.0 };
                *v = (next * Self::VOLUME_STEP).clamp(0.0, 1.0);
                // Subir el volumen de algo silenciado lo vuelve a activar, como las teclas de Windows.
                if up {
                    *m = false;
                }
                Some(t)
            }
        }
    }
}

/// Una combinación de teclas. `key` es el nombre de la tecla («M», «F9», «Pausa»…, ver `hotkeys::KEYS`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub win: bool,
    pub key: String,
}

impl Hotkey {
    pub fn ctrl_alt(key: &str) -> Self {
        Self { ctrl: true, alt: true, shift: false, win: false, key: key.into() }
    }
}

/// Un atajo por cada acción (None = sin atajo).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub mic: Option<Hotkey>,
    pub master: Option<Hotkey>,
    pub channels: [Option<Hotkey>; 4],
    /// Silenciar la mezcla de retransmisión.
    pub stream: Option<Hotkey>,
    /// Volumen: personal bajar, personal subir, retransmisión bajar, retransmisión subir.
    pub volume: [Option<Hotkey>; 4],
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            mic: Some(Hotkey::ctrl_alt("M")),
            master: Some(Hotkey::ctrl_alt("S")),
            channels: Default::default(),
            stream: None,
            volume: Default::default(),
        }
    }
}

impl Hotkeys {
    pub fn get(&self, a: HotkeyAction) -> Option<&Hotkey> {
        self.slot(a).as_ref()
    }

    fn slot(&self, a: HotkeyAction) -> &Option<Hotkey> {
        match a {
            HotkeyAction::Mute(MuteTarget::Mic) => &self.mic,
            HotkeyAction::Mute(MuteTarget::Master) => &self.master,
            HotkeyAction::Mute(MuteTarget::Channel(ch)) => &self.channels[ch.index()],
            HotkeyAction::Mute(MuteTarget::Stream) => &self.stream,
            HotkeyAction::Volume { stream, up } => &self.volume[2 * stream as usize + up as usize],
        }
    }

    pub fn get_mut(&mut self, a: HotkeyAction) -> &mut Option<Hotkey> {
        match a {
            HotkeyAction::Mute(MuteTarget::Mic) => &mut self.mic,
            HotkeyAction::Mute(MuteTarget::Master) => &mut self.master,
            HotkeyAction::Mute(MuteTarget::Channel(ch)) => &mut self.channels[ch.index()],
            HotkeyAction::Mute(MuteTarget::Stream) => &mut self.stream,
            HotkeyAction::Volume { stream, up } => &mut self.volume[2 * stream as usize + up as usize],
        }
    }

    /// Asigna `key` a `a` y se lo quita a cualquier otra acción que lo tuviera (no puede haber dos
    /// acciones en la misma combinación).
    pub fn assign(&mut self, a: HotkeyAction, key: Option<Hotkey>) {
        if let Some(k) = &key {
            for other in HotkeyAction::all() {
                if other != a && self.get(other) == Some(k) {
                    *self.get_mut(other) = None;
                }
            }
        }
        *self.get_mut(a) = key;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayCorner {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl OverlayCorner {
    pub const ALL: [OverlayCorner; 4] = [OverlayCorner::TopLeft, OverlayCorner::TopRight, OverlayCorner::BottomLeft, OverlayCorner::BottomRight];
    pub fn label(self) -> &'static str {
        match self {
            OverlayCorner::TopLeft => tr!("Arriba a la izquierda", "Top left"),
            OverlayCorner::TopRight => tr!("Arriba a la derecha", "Top right"),
            OverlayCorner::BottomLeft => tr!("Abajo a la izquierda", "Bottom left"),
            OverlayCorner::BottomRight => tr!("Abajo a la derecha", "Bottom right"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub corner: OverlayCorner,
    /// Segundos que se queda a la vista.
    pub seconds: f32,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self { enabled: true, corner: OverlayCorner::TopLeft, seconds: 2.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// ID WASAPI de los auriculares (None = predeterminado de comunicaciones/consola).
    pub output_device: Option<String>,
    pub master_volume: f32,
    pub master_muted: bool,
    pub chatmix_enabled: bool,
    /// -1.0 = todo juego, 0.0 = equilibrado, +1.0 = todo chat.
    pub chatmix: f32,
    pub channels: [ChannelSettings; 4],
    /// Nombre del ejecutable en minúsculas -> canal.
    pub app_routes: BTreeMap<String, Channel>,
    /// Ejecutables que nunca se capturan (en minúsculas). Incluye por defecto los que podrían
    /// crear un bucle de realimentación: el motor de audio de Windows y el propio Sonar de SteelSeries.
    pub ignored_apps: BTreeSet<String>,
    pub default_channel: Channel,
    pub mic: MicSettings,
    pub presets: Vec<UserPreset>,
    /// target.key() -> nombre del último preset aplicado en ese canal.
    pub active_presets: BTreeMap<String, String>,
    pub api_port: u16,
    /// Al arrancar pone Juego como salida predeterminada de Windows (y el micro virtual como
    /// entrada); al cerrar devuelve los predeterminados a los dispositivos reales.
    pub manage_defaults: bool,
    /// La X de la ventana la esconde en la bandeja del sistema en vez de cerrar el programa.
    pub close_to_tray: bool,
    /// Salida silenciosa para el modo sin dispositivos virtuales: una salida real que no escuchas
    /// (p. ej. la de la placa sin nada enchufado). Se pone como predeterminada de Windows; las apps
    /// suenan ahí sin oírse y Rust Sonar las captura una a una y las mezcla en tus auriculares.
    pub sink_device: Option<String>,
    /// Dispositivos que «Ordenar dispositivos de Windows» ha ocultado (para poder volver a mostrarlos).
    pub hidden_devices: Vec<String>,
    /// Menú lateral plegado (solo iconos).
    pub sidebar_collapsed: bool,
    /// Ya se hizo (o no hacía falta) la configuración automática del primer arranque.
    pub auto_setup_done: bool,
    /// Nombre original de cada dispositivo que hemos renombrado (id -> nombre), para devolvérselo
    /// al desinstalar.
    pub renamed_devices: BTreeMap<String, String>,
    /// Idioma de la interfaz (Auto = el de Windows).
    pub language: crate::i18n::Language,
    /// Atajos de teclado globales para silenciar o activar el micro y las salidas.
    pub hotkeys: Hotkeys,
    /// Aviso en pantalla al silenciar o activar algo.
    pub overlay: OverlaySettings,
    /// Modo streamer: segunda mezcla para la retransmisión.
    pub streamer: StreamerSettings,
    /// Ya se ordenaron solos los dispositivos de Windows tras la primera configuración (se hace una vez).
    pub devices_tidied: bool,
    /// Alguna vez estuvieron asignadas las cinco funciones (4 canales y micro). Hasta entonces, la
    /// configuración se va completando sola con los cables que aparezcan.
    pub setup_complete: bool,
}

pub const DEFAULT_IGNORED: [&str; 4] = ["audiodg", "steelseriessonar", "steelseriesgg", "rust_sonar"];

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_device: None,
            master_volume: 1.0,
            master_muted: false,
            chatmix_enabled: true,
            chatmix: 0.0,
            channels: Default::default(),
            app_routes: BTreeMap::new(),
            ignored_apps: DEFAULT_IGNORED.iter().map(|s| s.to_string()).collect(),
            default_channel: Channel::Game,
            mic: MicSettings::default(),
            presets: Vec::new(),
            active_presets: BTreeMap::new(),
            api_port: 7878,
            manage_defaults: true,
            close_to_tray: true,
            sink_device: None,
            hidden_devices: Vec::new(),
            sidebar_collapsed: false,
            auto_setup_done: false,
            renamed_devices: BTreeMap::new(),
            language: crate::i18n::Language::Auto,
            hotkeys: Hotkeys::default(),
            overlay: OverlaySettings::default(),
            streamer: StreamerSettings::default(),
            devices_tidied: false,
            setup_complete: false,
        }
    }
}

impl Settings {
    pub fn channel(&self, ch: Channel) -> &ChannelSettings {
        &self.channels[ch.index()]
    }

    pub fn channel_mut(&mut self, ch: Channel) -> &mut ChannelSettings {
        &mut self.channels[ch.index()]
    }

    pub fn eq_for(&self, t: EqTarget) -> &EqSettings {
        match t {
            EqTarget::Channel(ch) => &self.channel(ch).eq,
            EqTarget::Mic => &self.mic.eq,
        }
    }

    pub fn eq_for_mut(&mut self, t: EqTarget) -> &mut EqSettings {
        match t {
            EqTarget::Channel(ch) => &mut self.channel_mut(ch).eq,
            EqTarget::Mic => &mut self.mic.eq,
        }
    }

    pub fn presets_for(&self, t: EqTarget) -> impl Iterator<Item = &UserPreset> {
        self.presets.iter().filter(move |p| p.target == t)
    }

    fn user_preset(&self, t: EqTarget, name: &str) -> Option<&UserPreset> {
        self.presets_for(t).find(|p| p.name == name)
    }

    pub fn is_user_preset(&self, t: EqTarget, name: &str) -> bool {
        self.user_preset(t, name).is_some()
    }

    /// EQ guardado en un preset (primero los del usuario, luego los integrados).
    pub fn preset_eq(&self, t: EqTarget, name: &str) -> Option<EqSettings> {
        self.user_preset(t, name).map(|p| p.eq.clone()).or_else(|| EqSettings::preset(name))
    }

    pub fn active_preset(&self, t: EqTarget) -> Option<&str> {
        self.active_presets.get(t.key()).map(String::as_str)
    }

    /// true si el EQ actual ya no coincide con el preset que se aplicó.
    pub fn preset_modified(&self, t: EqTarget) -> bool {
        match self.active_preset(t).and_then(|n| self.preset_eq(t, n)) {
            Some(eq) => &eq != self.eq_for(t),
            None => false,
        }
    }

    pub fn apply_preset(&mut self, t: EqTarget, name: &str) -> bool {
        let Some(eq) = self.preset_eq(t, name) else { return false };
        *self.eq_for_mut(t) = eq;
        self.active_presets.insert(t.key().into(), name.into());
        true
    }

    /// Guarda el EQ actual con ese nombre (sobrescribe si ya existe) y lo marca como activo.
    pub fn save_preset(&mut self, t: EqTarget, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let eq = self.eq_for(t).clone();
        match self.presets.iter_mut().find(|p| p.target == t && p.name == name) {
            Some(p) => p.eq = eq,
            None => self.presets.push(UserPreset { name: name.into(), target: t, favorite: false, eq }),
        }
        self.active_presets.insert(t.key().into(), name.into());
    }

    pub fn delete_preset(&mut self, t: EqTarget, name: &str) {
        self.presets.retain(|p| !(p.target == t && p.name == name));
        if self.active_preset(t) == Some(name) {
            self.active_presets.remove(t.key());
        }
    }

    pub fn toggle_favorite(&mut self, t: EqTarget, name: &str) {
        if let Some(p) = self.presets.iter_mut().find(|p| p.target == t && p.name == name) {
            p.favorite = !p.favorite;
        }
    }

    /// IDs de todos los dispositivos que no son la salida real (canales, envío del micro, salida
    /// silenciosa y salida de la retransmisión).
    pub fn virtual_device_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.channels.iter().filter_map(|c| c.device.clone()).collect();
        ids.extend(self.mic.output_device.clone());
        ids.extend(self.sink_device.clone());
        ids.extend(self.streamer.device.clone());
        ids
    }

    /// Problemas de configuración de los dispositivos virtuales (para avisar en la UI).
    pub fn virtual_device_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let ids = self.virtual_device_ids();
        for (i, id) in ids.iter().enumerate() {
            if ids[..i].contains(id) {
                out.push(tr!("El mismo dispositivo virtual está asignado a dos funciones; cada una necesita el suyo.", "The same virtual device is assigned to two roles; each one needs its own.").into());
                break;
            }
        }
        if let Some(out_dev) = &self.output_device {
            if ids.contains(out_dev) {
                out.push(tr!("La salida (auriculares) no puede ser un dispositivo virtual, la salida silenciosa ni la de retransmisión.", "The output (headphones) can't be a virtual device, the silent output or the stream output.").into());
            }
        }
        out
    }

    /// El modo streamer está activado y su mezcla tiene por dónde salir.
    pub fn stream_active(&self) -> bool {
        self.streamer.enabled && self.streamer.device.is_some()
    }

    /// Ganancia de un canal en la mezcla de retransmisión. El ChatMix no cuenta (es para lo que oyes
    /// tú); la ganancia del canal sí, porque es parte del sonido del canal.
    pub fn stream_gain(&self, ch: Channel) -> f32 {
        let send = &self.streamer.channels[ch.index()];
        if !self.stream_active() || send.muted {
            return 0.0;
        }
        send.volume * crate::dsp::db_to_lin(self.channel(ch).boost_db.clamp(-12.0, 12.0))
    }

    /// Volumen general de la mezcla de retransmisión.
    pub fn stream_master_gain(&self) -> f32 {
        if !self.stream_active() || self.streamer.muted { 0.0 } else { self.streamer.volume }
    }

    /// Ganancia del micro en la retransmisión (va después de su volumen y su silencio: silenciar el
    /// micro lo silencia también en el directo).
    pub fn stream_mic_gain(&self) -> f32 {
        let send = &self.streamer.mic;
        if !self.stream_active() || !self.mic.enabled || send.muted { 0.0 } else { send.volume }
    }

    pub fn is_ignored(&self, exe: &str) -> bool {
        self.ignored_apps.contains(&exe.to_ascii_lowercase())
    }

    pub fn route_for(&self, exe: &str) -> Channel {
        self.app_routes
            .get(&exe.to_ascii_lowercase())
            .copied()
            .unwrap_or(self.default_channel)
    }

    /// Ganancia extra que aplica el ChatMix a cada canal (solo afecta a Juego y Chat).
    pub fn chatmix_gain(&self, ch: Channel) -> f32 {
        if !self.chatmix_enabled {
            return 1.0;
        }
        let m = self.chatmix.clamp(-1.0, 1.0);
        match ch {
            Channel::Game if m > 0.0 => 1.0 - m,
            Channel::Chat if m < 0.0 => 1.0 + m,
            _ => 1.0,
        }
    }

    pub fn path() -> PathBuf {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("RustSonar").join("config.json")
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path())
            .ok()
            // Un editor de Windows (o PowerShell 5) puede guardarlo con BOM: no es motivo para perder la configuración.
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Escribe en un temporal y renombra para no dejar el JSON a medias si se cierra el programa.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatmix_center_keeps_both() {
        let s = Settings::default();
        assert_eq!(s.chatmix_gain(Channel::Game), 1.0);
        assert_eq!(s.chatmix_gain(Channel::Chat), 1.0);
    }

    #[test]
    fn chatmix_towards_chat_lowers_game() {
        let s = Settings { chatmix: 0.75, ..Default::default() };
        assert!((s.chatmix_gain(Channel::Game) - 0.25).abs() < 1e-6);
        assert_eq!(s.chatmix_gain(Channel::Chat), 1.0);
        assert_eq!(s.chatmix_gain(Channel::Media), 1.0);
    }

    #[test]
    fn settings_roundtrip_json() {
        let mut s = Settings::default();
        s.app_routes.insert("discord".into(), Channel::Chat);
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.route_for("Discord"), Channel::Chat);
        assert_eq!(back.route_for("game"), Channel::Game);
    }

    #[test]
    fn presets_save_apply_and_detect_changes() {
        let mut s = Settings::default();
        let t = EqTarget::Channel(Channel::Game);
        assert!(s.apply_preset(t, "Graves+"));
        assert_eq!(s.active_preset(t), Some("Graves+"));
        assert!(!s.preset_modified(t));

        s.eq_for_mut(t).bands[3].gain_db = 4.0;
        assert!(s.preset_modified(t));

        s.save_preset(t, "Mi FPS");
        assert!(s.is_user_preset(t, "Mi FPS"));
        assert!(!s.preset_modified(t));
        // Un preset de Juego no aparece en Chat.
        assert_eq!(s.presets_for(EqTarget::Channel(Channel::Chat)).count(), 0);

        s.toggle_favorite(t, "Mi FPS");
        assert!(s.presets_for(t).next().unwrap().favorite);

        s.delete_preset(t, "Mi FPS");
        assert_eq!(s.active_preset(t), None);
        assert!(!s.apply_preset(t, "Mi FPS"));
    }

    #[test]
    fn eq_target_keys_roundtrip() {
        for t in EqTarget::ALL {
            assert_eq!(EqTarget::parse(t.key()), Some(t));
        }
    }

    #[test]
    fn detects_virtual_device_conflicts() {
        let mut s = Settings::default();
        assert!(s.virtual_device_problems().is_empty());
        s.channel_mut(Channel::Game).device = Some("A".into());
        s.channel_mut(Channel::Chat).device = Some("A".into());
        assert_eq!(s.virtual_device_problems().len(), 1);
        s.channel_mut(Channel::Chat).device = Some("B".into());
        s.output_device = Some("B".into());
        assert_eq!(s.virtual_device_problems().len(), 1);
        assert_eq!(s.virtual_device_ids(), vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn hotkey_assign_moves_combination() {
        let mut h = Hotkeys::default();
        let mic = h.mic.clone();
        let chat = HotkeyAction::Mute(MuteTarget::Channel(Channel::Chat));
        h.assign(chat, mic.clone());
        assert_eq!(h.mic, None);
        assert_eq!(h.get(chat), mic.as_ref());
        // También entre silencios y volumen.
        let up = HotkeyAction::Volume { stream: true, up: true };
        h.assign(up, mic.clone());
        assert_eq!(h.get(chat), None);
        assert_eq!(h.volume[3], mic);
        let mut s = Settings::default();
        MuteTarget::Channel(Channel::Aux).toggle(&mut s);
        assert!(MuteTarget::Channel(Channel::Aux).is_muted(&s));
        assert!(!MuteTarget::Mic.is_muted(&s));
    }

    #[test]
    fn hotkey_ids_are_unique_and_roundtrip() {
        let ids: Vec<i32> = HotkeyAction::all().map(HotkeyAction::id).collect();
        assert_eq!(ids.len(), 11);
        for (i, id) in ids.iter().enumerate() {
            assert!(!ids[..i].contains(id));
            assert_eq!(HotkeyAction::from_id(*id).map(HotkeyAction::id), Some(*id));
        }
    }

    #[test]
    fn volume_hotkeys_step_and_unmute() {
        let mut s = Settings { master_volume: 0.73, master_muted: true, ..Default::default() };
        let t = HotkeyAction::Volume { stream: false, up: true }.run(&mut s);
        assert_eq!(t, Some(MuteTarget::Master));
        assert!((s.master_volume - 0.75).abs() < 1e-6);
        assert!(!s.master_muted);
        s.streamer.volume = 0.02;
        HotkeyAction::Volume { stream: true, up: false }.run(&mut s);
        assert_eq!(s.streamer.volume, 0.0);
        assert!((s.master_volume - 0.75).abs() < 1e-6);
    }

    #[test]
    fn stream_gains_need_streamer_mode_and_device() {
        let mut s = Settings::default();
        s.streamer.channels[Channel::Media.index()].volume = 0.5;
        assert_eq!(s.stream_gain(Channel::Media), 0.0);
        s.streamer.enabled = true;
        assert_eq!(s.stream_gain(Channel::Media), 0.0, "sin dispositivo no hay retransmisión");
        s.streamer.device = Some("obs".into());
        assert_eq!(s.stream_gain(Channel::Media), 0.5);
        // El ChatMix no toca la retransmisión.
        s.chatmix = 1.0;
        assert_eq!(s.stream_gain(Channel::Game), 1.0);
        s.streamer.channels[Channel::Game.index()].muted = true;
        assert_eq!(s.stream_gain(Channel::Game), 0.0);
        assert_eq!(s.stream_mic_gain(), 0.0, "micro apagado");
        s.mic.enabled = true;
        assert_eq!(s.stream_mic_gain(), 1.0);
        // La salida de retransmisión cuenta como dispositivo no real.
        assert!(s.virtual_device_ids().contains(&"obs".to_string()));
    }

    #[test]
    fn old_config_without_streamer_loads() {
        let json = r#"{"master_volume":0.5,"hotkeys":{"mic":{"ctrl":true,"alt":true,"key":"M"}}}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(!s.streamer.enabled);
        assert_eq!(s.streamer.volume, 1.0);
        assert_eq!(s.hotkeys.volume, [None, None, None, None]);
    }

    #[test]
    fn feedback_sources_ignored_by_default() {
        let s = Settings::default();
        assert!(s.is_ignored("SteelSeriesSonar"));
        assert!(s.is_ignored("audiodg"));
        assert!(!s.is_ignored("discord"));
    }
}
