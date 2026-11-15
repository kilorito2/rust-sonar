//! Compresor de dinámica (feed-forward, con los canales enlazados).
//!
//! Se usa para dos cosas:
//! - **Volumen inteligente** de un canal: sube lo flojo (pasos, diálogos) y contiene lo fuerte
//!   (explosiones), para poder oírlo todo sin tocar el volumen.
//! - **Compresor del micro**: iguala la voz para que no haya gritos ni susurros.

use super::{db_to_lin, lin_to_db};

#[derive(Clone, Copy, Debug)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub makeup_db: f32,
    pub attack_s: f32,
    pub release_s: f32,
}

impl CompressorParams {
    pub const SMART_VOLUME: Self = Self { threshold_db: -30.0, ratio: 4.0, makeup_db: 9.0, attack_s: 0.005, release_s: 0.30 };
    /// Volumen inteligente según su carácter y su nivel extra (0 … 1 → hasta +6 dB).
    pub fn smart(loudness: crate::config::Loudness, level: f32) -> Self {
        use crate::config::Loudness::*;
        let base = match loudness {
            Quiet => Self { threshold_db: -36.0, ratio: 6.0, makeup_db: 6.0, ..Self::SMART_VOLUME },
            Balanced => Self::SMART_VOLUME,
            Loud => Self { makeup_db: 12.0, ..Self::SMART_VOLUME },
        };
        Self { makeup_db: base.makeup_db + 6.0 * level.clamp(0.0, 1.0), ..base }
    }

    pub const VOICE: Self = Self { threshold_db: -24.0, ratio: 3.0, makeup_db: 6.0, attack_s: 0.003, release_s: 0.15 };
}

pub struct Compressor {
    fs: f32,
    /// Reducción de ganancia actual en dB (≤ 0).
    reduction_db: f32,
    /// Ganancia de compensación suavizada, para que activarlo/desactivarlo no dé un salto.
    makeup: f32,
}

impl Compressor {
    pub fn new(fs: f32) -> Self {
        Self { fs, reduction_db: 0.0, makeup: 1.0 }
    }

    /// Procesa un buffer intercalado. Con `enabled = false` vuelve suavemente a ganancia unidad.
    pub fn process(&mut self, buf: &mut [f32], channels: usize, p: &CompressorParams, enabled: bool) {
        if !enabled && self.reduction_db > -0.01 && (self.makeup - 1.0).abs() < 1e-3 {
            return;
        }
        let attack = (-1.0 / (p.attack_s * self.fs)).exp();
        let release = (-1.0 / (p.release_s * self.fs)).exp();
        let slope = 1.0 - 1.0 / p.ratio.max(1.0);
        let makeup_target = if enabled { db_to_lin(p.makeup_db) } else { 1.0 };
        let makeup_coef = 1.0 - (-1.0 / (0.05 * self.fs)).exp();

        for frame in buf.chunks_exact_mut(channels) {
            let level = frame.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let over = lin_to_db(level) - p.threshold_db;
            let wanted = if enabled && over > 0.0 { -over * slope } else { 0.0 };
            let coef = if wanted < self.reduction_db { attack } else { release };
            self.reduction_db = wanted + (self.reduction_db - wanted) * coef;
            self.makeup += (makeup_target - self.makeup) * makeup_coef;
            let gain = db_to_lin(self.reduction_db) * self.makeup;
            for x in frame {
                *x *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(level: f32, p: &CompressorParams) -> f32 {
        let mut c = Compressor::new(48_000.0);
        let mut buf = vec![level; 48_000];
        c.process(&mut buf, 1, p, true);
        *buf.last().unwrap()
    }

    #[test]
    fn quiet_signal_gets_makeup_gain_only() {
        let p = CompressorParams::SMART_VOLUME;
        let out = run(db_to_lin(-50.0), &p);
        assert!((lin_to_db(out) - (-50.0 + p.makeup_db)).abs() < 0.2);
    }

    #[test]
    fn loud_signal_is_reduced_by_ratio() {
        let p = CompressorParams { threshold_db: -20.0, ratio: 4.0, makeup_db: 0.0, attack_s: 0.005, release_s: 0.1 };
        // 20 dB por encima del umbral con 4:1 -> queda 5 dB por encima: -15 dBFS.
        let out = run(1.0, &p);
        assert!((lin_to_db(out) + 15.0).abs() < 0.3, "{}", lin_to_db(out));
    }

    #[test]
    fn disabled_is_transparent() {
        let mut c = Compressor::new(48_000.0);
        let mut buf = vec![0.7f32; 1000];
        c.process(&mut buf, 2, &CompressorParams::VOICE, false);
        assert!(buf.iter().all(|x| *x == 0.7));
    }
}
