//! Filtro biquad (IIR de 2º orden) con las fórmulas del "Audio EQ Cookbook" de R. Bristow-Johnson.

use crate::config::FilterKind;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for Coeffs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Coeffs {
    pub const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    pub fn design(kind: FilterKind, freq: f64, gain_db: f64, q: f64, fs: f64) -> Coeffs {
        let freq = freq.clamp(10.0, fs * 0.49);
        let q = q.max(0.05);
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let sq = 2.0 * a.sqrt() * alpha;

        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cos + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - sq),
                (a + 1.0) + (a - 1.0) * cos + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - sq,
            ),
            FilterKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cos + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - sq),
                (a + 1.0) - (a - 1.0) * cos + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - sq,
            ),
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        };

        Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Respuesta en magnitud |H(e^jw)| en dB a la frecuencia `freq` (para dibujar la curva del EQ).
    pub fn magnitude_db(&self, freq: f64, fs: f64) -> f64 {
        let w = 2.0 * PI * freq / fs;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im).max(1e-30);
        10.0 * mag2.max(1e-30).log10()
    }
}

/// Biquad en forma directa II transpuesta. El estado va en f64 para que los graves no pierdan precisión.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    c: Coeffs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn set_coeffs(&mut self, c: Coeffs) {
        self.c = c;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FS: f64 = 48_000.0;

    #[test]
    fn peaking_hits_gain_at_center() {
        let c = Coeffs::design(FilterKind::Peaking, 1000.0, 6.0, 1.0, FS);
        assert!((c.magnitude_db(1000.0, FS) - 6.0).abs() < 0.01);
        assert!(c.magnitude_db(20.0, FS).abs() < 0.1);
    }

    #[test]
    fn lowpass_is_minus_3db_at_cutoff_with_butterworth_q() {
        let c = Coeffs::design(FilterKind::LowPass, 2000.0, 0.0, std::f64::consts::FRAC_1_SQRT_2, FS);
        assert!((c.magnitude_db(2000.0, FS) + 3.01).abs() < 0.05);
        assert!(c.magnitude_db(100.0, FS).abs() < 0.05);
    }

    #[test]
    fn low_shelf_boosts_bass_only() {
        let c = Coeffs::design(FilterKind::LowShelf, 100.0, 8.0, 0.707, FS);
        assert!((c.magnitude_db(20.0, FS) - 8.0).abs() < 0.3);
        assert!(c.magnitude_db(5000.0, FS).abs() < 0.1);
    }

    #[test]
    fn processing_matches_magnitude() {
        // Una senoide de 1 kHz por un peaking de +6 dB debe salir con amplitud ~2x.
        let mut f = Biquad::default();
        f.set_coeffs(Coeffs::design(FilterKind::Peaking, 1000.0, 6.0, 1.0, FS));
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = (2.0 * PI * 1000.0 * n as f64 / FS).sin() as f32 * 0.25;
            let y = f.process(x);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak / 0.25 - 1.995).abs() < 0.02, "peak={peak}");
    }
}
