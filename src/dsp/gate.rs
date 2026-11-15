//! Puerta de ruido por nivel con seguidor de envolvente, "hold" y ataque/relajación suaves.

use super::db_to_lin;

pub struct NoiseGate {
    fs: f32,
    env: f32,
    gain: f32,
    hold_left: usize,
}

impl NoiseGate {
    const ATTACK_S: f32 = 0.002;
    const ENV_DECAY_S: f32 = 0.010;
    const HOLD_S: f32 = 0.050;

    pub fn new(fs: f32) -> Self {
        Self { fs, env: 0.0, gain: 0.0, hold_left: 0 }
    }

    pub fn process(&mut self, buf: &mut [f32], threshold_db: f32, release_ms: f32) {
        let thr = db_to_lin(threshold_db);
        let env_decay = (-1.0 / (Self::ENV_DECAY_S * self.fs)).exp();
        let attack = (-1.0 / (Self::ATTACK_S * self.fs)).exp();
        let release = (-1.0 / ((release_ms.max(5.0) / 1000.0) * self.fs)).exp();
        let hold_len = (Self::HOLD_S * self.fs) as usize;

        for x in buf.iter_mut() {
            let level = x.abs();
            self.env = if level > self.env { level } else { self.env * env_decay };

            let target = if self.env >= thr {
                self.hold_left = hold_len;
                1.0
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
                1.0
            } else {
                0.0
            };

            let coef = if target > self.gain { attack } else { release };
            self.gain = target + (self.gain - target) * coef;
            *x *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_on_quiet_signal_and_opens_on_loud() {
        let mut g = NoiseGate::new(48_000.0);
        let mut quiet = vec![0.001f32; 48_000];
        g.process(&mut quiet, -40.0, 100.0);
        assert!(quiet.last().unwrap().abs() < 1e-5);

        let mut loud = vec![0.5f32; 4_800];
        g.process(&mut loud, -40.0, 100.0);
        assert!((loud.last().unwrap() - 0.5).abs() < 1e-3);
    }
}
