//! De multicanal (5.1 / 7.1) a estéreo para auriculares.
//!
//! Los dispositivos virtuales de canal suelen ser 7.1: los juegos lo detectan y sacan sonido
//! envolvente. Hay dos formas de llevarlo a dos auriculares:
//!
//! - **Mezcla simple** (`spatial = false`): cada altavoz se reparte entre L y R con coeficientes fijos.
//!   El contenido estéreo pasa intacto.
//! - **Audio espacial** (`spatial = true`): cada altavoz virtual se coloca alrededor de la cabeza con
//!   un modelo de cabeza esférica (Brown y Duda, 1998): a cada oído le llega con el retardo
//!   (diferencia de tiempo interaural) y el filtrado (sombra de la cabeza) que tendría un sonido
//!   real desde esa dirección. Los altavoces traseros llevan además un recorte de agudos que imita
//!   la sombra del pabellón de la oreja, la pista principal para distinguir delante de detrás.

use super::biquad::{Biquad, Coeffs};
use super::db_to_lin;
use super::reverb::Reverb;
use crate::config::{DEFAULT_AZIMUTHS, FilterKind, SpatialMode, SpatialSettings};

// Bits de posición de altavoz de Windows (dwChannelMask); los canales van en orden de bit.
pub const SPEAKER_FRONT_LEFT: u32 = 0x1;
pub const SPEAKER_FRONT_RIGHT: u32 = 0x2;
pub const SPEAKER_FRONT_CENTER: u32 = 0x4;
pub const SPEAKER_LOW_FREQUENCY: u32 = 0x8;
pub const SPEAKER_BACK_LEFT: u32 = 0x10;
pub const SPEAKER_BACK_RIGHT: u32 = 0x20;
pub const SPEAKER_BACK_CENTER: u32 = 0x100;
pub const SPEAKER_SIDE_LEFT: u32 = 0x200;
pub const SPEAKER_SIDE_RIGHT: u32 = 0x400;

/// Máscara habitual para `channels` canales cuando el dispositivo no la indica.
pub fn default_mask(channels: usize) -> u32 {
    match channels {
        1 => SPEAKER_FRONT_CENTER,
        2 => 0x3,
        4 => 0x33,
        6 => 0x3F,
        8 => 0x63F,
        n => (1u32 << n.min(18)) - 1,
    }
}

/// Posiciones (un bit cada una) de los canales de una máscara, en orden.
pub fn speakers_of(mask: u32, channels: usize) -> Vec<u32> {
    let mut out: Vec<u32> = (0..32).map(|b| 1u32 << b).filter(|b| mask & b != 0).take(channels).collect();
    // Si la máscara trae menos bits que canales, el resto se trata como centro (se reparte igual).
    out.resize(channels, SPEAKER_FRONT_CENTER);
    out
}

const HEAD_RADIUS_M: f32 = 0.0875;
const SPEED_OF_SOUND: f32 = 343.0;
const DELAY_LEN: usize = 64;

/// Camino de un altavoz virtual hasta un oído: retardo + sombra de la cabeza.
struct EarPath {
    delay: [f32; DELAY_LEN],
    delay_samples: usize,
    // Filtro de un polo y un cero: y = b0·x + b1·x₁ − a1·y₁
    b0: f32,
    b1: f32,
    a1: f32,
    x1: f32,
    y1: f32,
}

impl EarPath {
    /// `incidence`: ángulo (rad, 0..π) entre la dirección del sonido y el eje del oído.
    fn new(incidence: f32, fs: f32) -> Self {
        let a_over_c = HEAD_RADIUS_M / SPEED_OF_SOUND;
        // Retardo de llegada (desplazado para que nunca sea negativo).
        let tau = if incidence <= std::f32::consts::FRAC_PI_2 {
            a_over_c * (1.0 - incidence.cos())
        } else {
            a_over_c * (1.0 + incidence - std::f32::consts::FRAC_PI_2)
        };
        // Sombra de la cabeza: realce de agudos de cara al oído, recorte fuerte al otro lado.
        const ALPHA_MIN: f32 = 0.1;
        const THETA_MIN: f32 = 150.0 * std::f32::consts::PI / 180.0;
        let alpha = (1.0 + ALPHA_MIN / 2.0) + (1.0 - ALPHA_MIN / 2.0) * (incidence / THETA_MIN * std::f32::consts::PI).cos();
        let w0 = SPEED_OF_SOUND / HEAD_RADIUS_M;
        let norm = w0 + fs;
        Self {
            delay: [0.0; DELAY_LEN],
            delay_samples: ((tau * fs).round() as usize).min(DELAY_LEN - 1),
            b0: (w0 + alpha * fs) / norm,
            b1: (w0 - alpha * fs) / norm,
            a1: (w0 - fs) / norm,
            x1: 0.0,
            y1: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, write: usize) -> f32 {
        self.delay[write] = x;
        let d = self.delay[(write + DELAY_LEN - self.delay_samples) % DELAY_LEN];
        let y = self.b0 * d + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = d;
        self.y1 = y;
        y
    }
}

/// Un canal de entrada con su reparto simple y su camino espacial.
struct Speaker {
    /// Bit de posición de Windows de este canal.
    position: u32,
    /// Ganancias de la mezcla simple (L, R).
    plain: (f32, f32),
    /// None = sin posición (subwoofer): va igual a los dos oídos.
    ears: Option<(EarPath, EarPath)>,
    /// Recorte de agudos de los altavoces traseros.
    rear: Option<Biquad>,
    gain: f32,
}

pub struct Surround {
    fs: f32,
    speakers: Vec<Speaker>,
    has_sides: bool,
    write: usize,
    enabled: bool,
    headphones: bool,
    /// Nivel de la reverberación (lineal) y ganancia de la señal directa según la distancia.
    wet: f32,
    direct: f32,
    reverb: Reverb,
    /// Mezcla suave entre simple (0) y espacial (1), para que el interruptor no dé un chasquido.
    blend: f32,
    blend_coef: f32,
    applied: Option<SpatialSettings>,
}

/// Índice de una posición de Windows dentro de `SpatialSettings::speakers`.
fn settings_index(speaker: u32) -> Option<usize> {
    Some(match speaker {
        SPEAKER_FRONT_LEFT => 0,
        SPEAKER_FRONT_RIGHT => 1,
        SPEAKER_FRONT_CENTER => 2,
        SPEAKER_LOW_FREQUENCY => 3,
        SPEAKER_BACK_LEFT => 4,
        SPEAKER_BACK_RIGHT => 5,
        SPEAKER_SIDE_LEFT => 6,
        SPEAKER_SIDE_RIGHT => 7,
        _ => return None,
    })
}

/// Azimut de fábrica (0 = delante, positivo = derecha) de cada posición; None = sin dirección.
fn default_azimuth(speaker: u32, has_sides: bool) -> Option<f32> {
    Some(match speaker {
        SPEAKER_LOW_FREQUENCY => return None,
        // En 5.1 los "traseros" son en realidad los envolventes (±110°); en 7.1 van detrás.
        SPEAKER_BACK_LEFT if !has_sides => -110.0,
        SPEAKER_BACK_RIGHT if !has_sides => 110.0,
        SPEAKER_BACK_CENTER => 180.0,
        0x40 => -15.0, // delantero izquierdo de centro
        0x80 => 15.0,
        other => settings_index(other).map(|i| DEFAULT_AZIMUTHS[i]).unwrap_or(0.0),
    })
}

/// Reparto de la mezcla simple según la dirección.
fn plain_gains(azimuth: Option<f32>) -> (f32, f32) {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    match azimuth {
        None => (0.5, 0.5),
        Some(a) if a.abs() < 1.0 || a.abs() > 179.0 => (s, s),
        Some(a) if a.abs() <= 45.0 => if a < 0.0 { (1.0, 0.0) } else { (0.0, 1.0) },
        Some(a) => if a < 0.0 { (s, 0.0) } else { (0.0, s) },
    }
}

impl Surround {
    pub fn new(speakers: &[u32], fs: f32) -> Self {
        let has_sides = speakers.contains(&SPEAKER_SIDE_LEFT);
        let list = speakers
            .iter()
            .map(|&position| Speaker { position, plain: plain_gains(default_azimuth(position, has_sides)), ears: None, rear: None, gain: 1.0 })
            .collect();
        let mut this = Self {
            fs,
            speakers: list,
            has_sides,
            write: 0,
            enabled: false,
            headphones: true,
            wet: 0.0,
            direct: 1.0,
            reverb: Reverb::new(),
            blend: 0.0,
            blend_coef: 1.0 - (-1.0 / (0.03 * fs)).exp(),
            applied: None,
        };
        this.configure(&SpatialSettings::default());
        this
    }

    /// Aplica los ajustes (posiciones, ganancias, modo, inmersión, distancia). Barato si no cambian.
    pub fn configure(&mut self, cfg: &SpatialSettings) {
        if self.applied.as_ref() == Some(cfg) {
            return;
        }
        let geometry_changed = self.applied.as_ref().map(|old| old.speakers != cfg.speakers).unwrap_or(true);
        self.enabled = cfg.enabled;
        self.headphones = cfg.mode == SpatialMode::Headphones;
        let distance = (cfg.distance / 100.0).clamp(0.0, 1.0);
        // Más lejos = más sala y algo menos de señal directa; cerca = seco y presente.
        self.wet = db_to_lin(cfg.reverb_gain_db()) * 0.5 * (0.25 + 0.75 * distance);
        self.direct = db_to_lin(-3.0 * distance);
        for sp in &mut self.speakers {
            let user = settings_index(sp.position).map(|i| cfg.speakers[i]);
            let azimuth = match (default_azimuth(sp.position, self.has_sides), user) {
                (None, _) => None,
                // En 5.1 se respeta la posición de fábrica de los envolventes salvo que se haya movido.
                (Some(def), Some(u)) => {
                    let factory = settings_index(sp.position).map(|i| DEFAULT_AZIMUTHS[i]).unwrap_or(def);
                    Some(if (u.azimuth - factory).abs() < 0.01 { def } else { u.azimuth })
                }
                (Some(def), None) => Some(def),
            };
            let user_gain = db_to_lin(user.map(|u| u.gain_db).unwrap_or(0.0).clamp(-24.0, 12.0));
            let (pl, pr) = plain_gains(azimuth);
            sp.plain = (pl * user_gain, pr * user_gain);
            // Los frontales definen el nivel; el resto algo por debajo para que el total no sature.
            sp.gain = user_gain
                * match azimuth {
                    None => 0.5,
                    Some(a) if a.abs() <= 45.0 && a.abs() >= 1.0 => 0.9,
                    Some(_) => 0.75,
                };
            if geometry_changed {
                sp.ears = azimuth.map(|a| {
                    let rad = a.to_radians();
                    // Oído izquierdo a −90°, derecho a +90°.
                    let incidence = |ear: f32| {
                        let mut d = (rad - ear).abs() % std::f32::consts::TAU;
                        if d > std::f32::consts::PI {
                            d = std::f32::consts::TAU - d;
                        }
                        d
                    };
                    (
                        EarPath::new(incidence(-std::f32::consts::FRAC_PI_2), self.fs),
                        EarPath::new(incidence(std::f32::consts::FRAC_PI_2), self.fs),
                    )
                });
                sp.rear = azimuth.filter(|a| a.abs() > 100.0).map(|_| {
                    let mut b = Biquad::default();
                    b.set_coeffs(Coeffs::design(FilterKind::HighShelf, 4500.0, -6.0, 0.7, self.fs as f64));
                    b
                });
            }
        }
        self.applied = Some(cfg.clone());
    }

    /// `input`: intercalado, con un canal por altavoz de la disposición. Deja en `out` el estéreo.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = self.speakers.len();
        out.clear();
        let target = if self.enabled { 1.0 } else { 0.0 };
        for frame in input.chunks_exact(n) {
            self.blend += (target - self.blend) * self.blend_coef;
            let spatial_active = self.blend > 1e-4;
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            let (mut sl, mut sr) = (0.0f32, 0.0f32);
            for (sp, &x) in self.speakers.iter_mut().zip(frame) {
                pl += x * sp.plain.0;
                pr += x * sp.plain.1;
                if !spatial_active || !self.headphones {
                    continue;
                }
                match &mut sp.ears {
                    Some((left, right)) => {
                        let x = match &mut sp.rear {
                            Some(shelf) => shelf.process(x),
                            None => x,
                        } * sp.gain;
                        sl += left.process(x, self.write);
                        sr += right.process(x, self.write);
                    }
                    None => {
                        sl += x * sp.gain;
                        sr += x * sp.gain;
                    }
                }
            }
            self.write = (self.write + 1) % DELAY_LEN;
            if !spatial_active {
                out.push(pl);
                out.push(pr);
                continue;
            }
            if !self.headphones {
                (sl, sr) = (pl, pr);
            }
            let (rl, rr) = self.reverb.process(pl, pr);
            let l = sl * self.direct + rl * self.wet;
            let r = sr * self.direct + rr * self.wet;
            out.push(pl + (l - pl) * self.blend);
            out.push(pr + (r - pr) * self.blend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_71() -> Vec<u32> {
        speakers_of(default_mask(8), 8)
    }

    /// Reproduce una señal por un único canal y devuelve la energía en cada oído.
    fn energy(channel: usize, spatial: bool, freq: f32) -> (f32, f32) {
        let mut s = Surround::new(&layout_71(), 48_000.0);
        // Seco (sin sala) para medir solo el modelo de cabeza.
        s.configure(&SpatialSettings { enabled: spatial, immersion: 0.0, distance: 0.0, ..Default::default() });
        s.wet = 0.0;
        let mut input = vec![0.0f32; 8 * 9600];
        for (i, frame) in input.chunks_exact_mut(8).enumerate() {
            frame[channel] = (i as f32 * 2.0 * std::f32::consts::PI * freq / 48_000.0).sin();
        }
        let mut out = Vec::new();
        s.process(&input, &mut out);
        // Se descarta el arranque (rampa de mezcla y retardos).
        let tail = &out[out.len() / 2..];
        let l: f32 = tail.chunks_exact(2).map(|f| f[0] * f[0]).sum();
        let r: f32 = tail.chunks_exact(2).map(|f| f[1] * f[1]).sum();
        (l, r)
    }

    #[test]
    fn mask_gives_windows_channel_order() {
        assert_eq!(
            layout_71(),
            [
                SPEAKER_FRONT_LEFT,
                SPEAKER_FRONT_RIGHT,
                SPEAKER_FRONT_CENTER,
                SPEAKER_LOW_FREQUENCY,
                SPEAKER_BACK_LEFT,
                SPEAKER_BACK_RIGHT,
                SPEAKER_SIDE_LEFT,
                SPEAKER_SIDE_RIGHT
            ]
        );
        assert_eq!(speakers_of(0x3, 2), [SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT]);
    }

    #[test]
    fn plain_downmix_keeps_stereo_untouched() {
        let mut s = Surround::new(&layout_71(), 48_000.0);
        let mut input = vec![0.0f32; 8 * 4];
        for (i, frame) in input.chunks_exact_mut(8).enumerate() {
            frame[0] = 0.1 * (i + 1) as f32;
            frame[1] = -0.2;
        }
        let mut out = Vec::new();
        s.process(&input, &mut out);
        assert_eq!(out, [0.1, -0.2, 0.2, -0.2, 0.3, -0.2, 0.4, -0.2]);
    }

    #[test]
    fn plain_downmix_folds_center_and_surrounds() {
        let (l, r) = energy(2, false, 500.0);
        assert!((l - r).abs() < 1e-3, "el centro va igual a los dos lados");
        let (l, r) = energy(6, false, 500.0);
        assert!(l > 0.0 && r == 0.0, "el lateral izquierdo solo va a la izquierda");
    }

    #[test]
    fn spatial_side_speaker_is_louder_in_near_ear() {
        // Lateral izquierdo a 4 kHz: la cabeza hace sombra al oído derecho.
        let (l, r) = energy(6, true, 4000.0);
        assert!(l > r * 4.0, "izq {l} der {r}");
        let (l, r) = energy(7, true, 4000.0);
        assert!(r > l * 4.0);
    }

    #[test]
    fn spatial_center_is_symmetric_and_reaches_both_ears() {
        let (l, r) = energy(2, true, 1000.0);
        assert!(l > 0.0 && (l - r).abs() / l < 1e-3);
    }

    #[test]
    fn spatial_rear_is_duller_than_front() {
        // Mismo lado, 8 kHz: detrás llega con menos agudos que delante.
        let (front, _) = energy(0, true, 8000.0);
        let (back, _) = energy(4, true, 8000.0);
        assert!(back < front * 0.6, "delante {front} detrás {back}");
    }

    #[test]
    fn moving_a_speaker_changes_which_ear_is_louder() {
        // El frontal izquierdo movido a la derecha del todo suena más en el oído derecho.
        let mut cfg = SpatialSettings { enabled: true, immersion: 0.0, distance: 0.0, ..Default::default() };
        cfg.speakers[0].azimuth = 90.0;
        let mut s = Surround::new(&layout_71(), 48_000.0);
        s.configure(&cfg);
        s.wet = 0.0;
        s.blend = 1.0;
        let mut input = vec![0.0f32; 8 * 4800];
        for (i, frame) in input.chunks_exact_mut(8).enumerate() {
            frame[0] = (i as f32 * 0.5).sin();
        }
        let mut out = Vec::new();
        s.process(&input, &mut out);
        let l: f32 = out.chunks_exact(2).map(|f| f[0] * f[0]).sum();
        let r: f32 = out.chunks_exact(2).map(|f| f[1] * f[1]).sum();
        assert!(r > l * 2.0, "izq {l} der {r}");
    }

    #[test]
    fn immersion_adds_a_tail_after_the_sound_stops() {
        let tail = |immersion: f32| {
            let mut s = Surround::new(&[SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT], 48_000.0);
            s.configure(&SpatialSettings { enabled: true, immersion, ..Default::default() });
            s.blend = 1.0;
            let mut input = vec![0.0f32; 2 * 9600];
            input[..200].fill(0.5);
            let mut out = Vec::new();
            s.process(&input, &mut out);
            out[4800..].iter().map(|x| x * x).sum::<f32>()
        };
        assert!(tail(1.0) > tail(0.0) * 3.0);
    }

    #[test]
    fn near_ear_hears_it_first() {
        let mut s = Surround::new(&layout_71(), 48_000.0);
        let mut input = vec![0.0f32; 8 * 64];
        input[6] = 1.0; // impulso en el lateral izquierdo
        let mut out = Vec::new();
        // Con la mezcla ya en espacial y sin sala.
        s.configure(&SpatialSettings { enabled: true, ..Default::default() });
        s.wet = 0.0;
        s.blend = 1.0;
        s.process(&input, &mut out);
        let first = |ear: usize| out.chunks_exact(2).position(|f| f[ear].abs() > 1e-4).unwrap();
        assert!(first(0) < first(1), "la izquierda debe llegar antes: {} vs {}", first(0), first(1));
        // La diferencia máxima entre oídos en una cabeza real ronda 0,6-0,7 ms (~30 muestras).
        assert!(first(1) - first(0) < 40);
    }
}
