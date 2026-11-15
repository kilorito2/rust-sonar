//! Ecualizador paramétrico multibanda sobre audio intercalado (LRLR...).

use super::biquad::{Biquad, Coeffs};
use super::db_to_lin;
use crate::config::{EqSettings, SAMPLE_RATE};

pub struct Equalizer {
    channels: usize,
    enabled: bool,
    preamp: f32,
    /// filters[banda][canal]
    filters: Vec<Vec<Biquad>>,
    active: Vec<bool>,
}

impl Equalizer {
    pub fn new(channels: usize) -> Self {
        Self { channels, enabled: false, preamp: 1.0, filters: Vec::new(), active: Vec::new() }
    }

    /// Recalcula coeficientes sin borrar el estado de los filtros (no hay clics al mover un slider).
    pub fn configure(&mut self, s: &EqSettings) {
        let fs = SAMPLE_RATE as f64;
        self.enabled = s.enabled;
        self.preamp = db_to_lin(s.preamp_db);
        let bands: Vec<_> = s.all_bands().collect();
        self.filters.resize_with(bands.len(), || vec![Biquad::default(); self.channels]);
        self.active.resize(bands.len(), false);
        for (i, band) in bands.iter().enumerate() {
            let c = Coeffs::design(band.kind, band.freq as f64, band.gain_db as f64, band.q as f64, fs);
            // Una campana/shelf a 0 dB es la identidad: nos ahorramos el cálculo.
            let neutral = band.kind.uses_gain() && band.gain_db.abs() < 0.01;
            self.active[i] = band.enabled && !neutral;
            for f in &mut self.filters[i] {
                f.set_coeffs(c);
            }
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if !self.enabled {
            return;
        }
        for frame in buf.chunks_exact_mut(self.channels) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample * self.preamp;
                for (band, filters) in self.filters.iter_mut().enumerate() {
                    if self.active[band] {
                        x = filters[ch].process(x);
                    }
                }
                *sample = x;
            }
        }
    }
}
