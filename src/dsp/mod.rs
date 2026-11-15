//! Procesado de señal: filtros, ecualizador, puerta de ruido y utilidades de ganancia.

pub mod biquad;
pub mod compressor;
pub mod eq;
pub mod gate;
pub mod reverb;
pub mod surround;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-6).log10()
}

/// Ganancia con rampa (filtro de un polo) para que los cambios de volumen no produzcan "zipper noise".
pub struct SmoothedGain {
    current: f32,
    coef: f32,
}

impl SmoothedGain {
    pub fn new(fs: f32, time_s: f32) -> Self {
        Self { current: 0.0, coef: 1.0 - (-1.0 / (time_s * fs)).exp() }
    }

    /// Aplica la ganancia a un buffer intercalado de `channels` canales.
    pub fn apply(&mut self, buf: &mut [f32], channels: usize, target: f32) {
        for frame in buf.chunks_exact_mut(channels) {
            self.current += (target - self.current) * self.coef;
            for s in frame {
                *s *= self.current;
            }
        }
    }
}

/// Limitador suave: lineal hasta 0.8 y curva tanh por encima, así nunca pasa de ±1.0.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE { x } else { x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh()) }
}

pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_clip_is_bounded_and_transparent_below_knee() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert!(soft_clip(10.0) <= 1.0);
        assert!(soft_clip(-10.0) >= -1.0);
    }

    #[test]
    fn db_conversions_roundtrip() {
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-4);
        assert!((lin_to_db(0.5) + 6.0206).abs() < 1e-3);
    }
}
