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

/// Atenuador de restos guiado por el detector de voz de la red neuronal.
///
/// La red quita el ruido constante, pero entre palabras deja pasar restos (tecleo, respiración,
/// clics). Aquí, cuando la probabilidad de voz es baja, la señal se baja hasta un suelo; al hablar
/// se abre casi al instante y se mantiene abierta un momento tras la última sílaba para no comerse
/// los finales de palabra. No es un corte: con el suelo en −9 dB apenas se nota que actúa.
pub struct VoiceFloor {
    fs: f32,
    gain: f32,
    hold_left: f32,
}

impl VoiceFloor {
    /// Por debajo, no hay voz; por encima de `OPEN`, seguro que sí. En medio, proporcional.
    const CLOSED: f32 = 0.12;
    const OPEN: f32 = 0.45;
    const ATTACK_S: f32 = 0.003;
    const RELEASE_S: f32 = 0.12;

    pub fn new(fs: f32) -> Self {
        Self { fs, gain: 1.0, hold_left: 0.0 }
    }

    /// `vad`: probabilidad de voz (0..1) del bloque. `floor_db`: suelo cuando no hay voz (0 = sin
    /// efecto). `hold_s`: cuánto sigue abierta tras la última sílaba.
    pub fn process(&mut self, buf: &mut [f32], vad: f32, floor_db: f32, hold_s: f32) {
        let floor = db_to_lin(floor_db.min(0.0));
        let mut open = ((vad - Self::CLOSED) / (Self::OPEN - Self::CLOSED)).clamp(0.0, 1.0);
        if open >= 1.0 {
            self.hold_left = hold_s;
        } else if self.hold_left > 0.0 {
            self.hold_left -= buf.len() as f32 / self.fs;
            open = 1.0;
        }
        let target = floor + (1.0 - floor) * open;
        let attack = 1.0 - (-1.0 / (Self::ATTACK_S * self.fs)).exp();
        let release = 1.0 - (-1.0 / (Self::RELEASE_S * self.fs)).exp();
        for x in buf.iter_mut() {
            let k = if target > self.gain { attack } else { release };
            self.gain += (target - self.gain) * k;
            *x *= self.gain;
        }
    }

    /// Ganancia actual (para tests y diagnóstico).
    #[allow(dead_code)]
    pub fn gain(&self) -> f32 {
        self.gain
    }
}

#[cfg(test)]
mod voice_floor_tests {
    use super::*;

    fn run(f: &mut VoiceFloor, vad: f32, floor_db: f32, blocks: usize) -> f32 {
        let mut last = 0.0;
        for _ in 0..blocks {
            let mut buf = [1.0f32; 480];
            f.process(&mut buf, vad, floor_db, 0.28);
            last = buf[479];
        }
        last
    }

    #[test]
    fn closes_to_the_floor_without_voice_and_reopens_fast() {
        let mut f = VoiceFloor::new(48_000.0);
        // 1 s sin voz: llega al suelo (−22 dB ≈ 0,079).
        let quiet = run(&mut f, 0.02, -22.0, 100);
        assert!((quiet - db_to_lin(-22.0)).abs() < 0.005, "{quiet}");
        // Un solo bloque con voz (10 ms) ya la deja pasar entera.
        let talking = run(&mut f, 0.9, -22.0, 1);
        assert!(talking > 0.95, "{talking}");
    }

    #[test]
    fn holds_open_after_the_last_syllable() {
        let mut f = VoiceFloor::new(48_000.0);
        run(&mut f, 0.9, -40.0, 5);
        // 200 ms después de dejar de detectar voz sigue abierta (finales de palabra).
        assert!(run(&mut f, 0.0, -40.0, 20) > 0.95);
        // Pasado el margen, se cierra.
        assert!(run(&mut f, 0.0, -40.0, 80) < 0.05);
    }

    #[test]
    fn zero_floor_never_touches_the_signal() {
        let mut f = VoiceFloor::new(48_000.0);
        assert_eq!(run(&mut f, 0.0, 0.0, 50), 1.0);
    }
}
