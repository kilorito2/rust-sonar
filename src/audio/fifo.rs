//! Cola de muestras entre un hilo productor (captura) y uno consumidor (mezclador), con
//! *jitter buffer* y compensación de la deriva de reloj.
//!
//! Los relojes de dos dispositivos nunca van exactamente a la misma velocidad. Si el productor es
//! un 0,01 % más rápido la cola crece sin parar; si es más lento, se vacía y hay chasquidos. Por eso:
//!
//! - **Precarga**: tras quedarse vacía no entrega nada hasta acumular `target` frames, así el
//!   consumidor siempre tiene un colchón contra los retrasos del planificador.
//! - **Remuestreo adaptativo**: si el nivel medio se aleja del objetivo, se lee un pelín más rápido o
//!   más lento (como mucho ±0,3 %, inaudible) con interpolación cúbica, en vez de tirar o inventar
//!   bloques enteros.
//! - **Recorte de emergencia**: si aun así se acumula demasiado (el consumidor estuvo parado), se
//!   descarta lo más antiguo.

use crate::config::SAMPLE_RATE;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Frames que la interpolación cúbica necesita tener siempre delante.
const TAPS: usize = 4;
const MAX_CORRECTION: f32 = 0.003;

pub struct Fifo {
    inner: Mutex<Inner>,
    channels: usize,
    /// Nivel objetivo, en frames.
    target: usize,
    /// A partir de aquí se recorta hasta `target`.
    max: usize,
}

struct Inner {
    buf: VecDeque<f32>,
    primed: bool,
    /// Posición fraccionaria de lectura entre el frame 1 y el 2 de la cola (0.0 ..< 1.0).
    pos: f32,
    /// Nivel medio (frames) suavizado, para que la corrección no siga el diente de sierra normal.
    avg: f32,
}

impl Fifo {
    pub fn new(channels: usize, target_frames: usize) -> Self {
        let target = target_frames.max(TAPS * 2);
        Self {
            inner: Mutex::new(Inner {
                buf: VecDeque::with_capacity(target * channels * 6),
                primed: false,
                pos: 0.0,
                avg: target as f32,
            }),
            channels,
            target,
            max: target * 5,
        }
    }

    pub fn push(&self, samples: &[f32]) {
        let mut b = self.inner.lock();
        b.buf.extend(samples.iter().copied());
        let frames = b.buf.len() / self.channels;
        if frames > self.max {
            let excess = (frames - self.target) * self.channels;
            b.buf.drain(..excess);
            b.avg = self.target as f32;
        }
    }

    /// Rellena `out` entero; devuelve cuántos frames eran audio real (el resto es silencio).
    pub fn pop_into(&self, out: &mut [f32]) -> usize {
        let ch = self.channels;
        let frames_out = out.len() / ch;
        let mut guard = self.inner.lock();
        let b = &mut *guard;
        let avail = b.buf.len() / ch;

        if !b.primed {
            if avail < self.target {
                out.fill(0.0);
                return 0;
            }
            b.primed = true;
            b.pos = 0.0;
            b.avg = avail as f32;
        }

        // Nivel medio con constante de tiempo de ~1 s (se llama cada ~10 ms).
        b.avg += (avail as f32 - b.avg) * 0.01;
        let err = b.avg - self.target as f32;
        let band = self.target as f32 * 0.5;
        let in_band = err.abs() <= band;
        let ratio = if in_band {
            // Dentro de la banda no se corrige; solo se termina de llegar a una posición entera
            // para volver a copiar las muestras tal cual.
            if b.pos > 0.0 { 1.0002 } else { 1.0 }
        } else {
            let excess = err - band * err.signum();
            1.0 + (excess / (SAMPLE_RATE as f32 * 2.0)).clamp(-MAX_CORRECTION, MAX_CORRECTION)
        };

        let mut produced = 0;
        while produced < frames_out {
            if b.buf.len() < TAPS * ch {
                break;
            }
            let base = produced * ch;
            if b.pos == 0.0 {
                for c in 0..ch {
                    out[base + c] = b.buf[ch + c];
                }
            } else {
                let t = b.pos;
                for c in 0..ch {
                    out[base + c] = hermite(b.buf[c], b.buf[ch + c], b.buf[2 * ch + c], b.buf[3 * ch + c], t);
                }
            }
            b.pos += ratio;
            while b.pos >= 1.0 {
                b.buf.drain(..ch);
                b.pos -= 1.0;
                if in_band {
                    b.pos = 0.0;
                }
                if b.buf.len() < TAPS * ch {
                    break;
                }
            }
            produced += 1;
        }

        if produced < frames_out {
            // Nos quedamos sin audio: silencio y a precargar de nuevo.
            out[produced * ch..].fill(0.0);
            b.primed = false;
        }
        produced
    }

    pub fn clear(&self) {
        let mut b = self.inner.lock();
        b.buf.clear();
        b.primed = false;
        b.pos = 0.0;
    }
}

/// Interpolación cúbica de Hermite (Catmull-Rom) entre `y1` y `y2`.
#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + c0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_primed_then_plays_in_order() {
        let f = Fifo::new(1, 16);
        f.push(&[1.0; 8]);
        let mut out = [9.0; 4];
        assert_eq!(f.pop_into(&mut out), 0, "aún no hay colchón suficiente");
        assert_eq!(out, [0.0; 4]);

        let data: Vec<f32> = (0..32).map(|i| i as f32).collect();
        f.push(&data);
        assert_eq!(f.pop_into(&mut out), 4);
        // Dentro de la banda y sin posición fraccionaria: copia exacta (con un frame de historia).
        assert_eq!(out, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn stereo_frames_stay_aligned() {
        let f = Fifo::new(2, 8);
        let data: Vec<f32> = (0..40).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        f.push(&data);
        let mut out = [0.0; 12];
        assert_eq!(f.pop_into(&mut out), 6);
        for fr in out.chunks_exact(2) {
            assert_eq!(fr, [1.0, -1.0]);
        }
    }

    #[test]
    fn underrun_fills_silence_and_reprimes() {
        let f = Fifo::new(1, 8);
        f.push(&[0.5; 10]);
        let mut out = [9.0; 16];
        let n = f.pop_into(&mut out);
        assert!(n < 16);
        assert!(out[n..].iter().all(|x| *x == 0.0));
        // Tras el vaciado vuelve a exigir precarga.
        f.push(&[0.5; 4]);
        assert_eq!(f.pop_into(&mut out), 0);
    }

    #[test]
    fn overflow_trims_to_target() {
        let f = Fifo::new(1, 8);
        f.push(&vec![0.25; 1000]);
        let mut out = vec![0.0; 1000];
        let n = f.pop_into(&mut out);
        assert!(n <= 8, "solo debe quedar el objetivo, quedaban {n}");
    }

    /// Productor un 0,1 % más rápido que el consumidor durante 60 s: sin remuestreo la cola
    /// desbordaría; con él, el nivel se queda acotado y no hay ni vaciados ni recortes.
    #[test]
    fn drift_is_absorbed_without_dropouts() {
        let target = 960;
        let f = Fifo::new(1, target);
        let mut phase = 0.0f32;
        let mut out = vec![0.0f32; 480];
        let mut dropouts = 0;
        let mut carry = 0.0f32;
        for i in 0..6000 {
            carry += 480.0 * 1.001;
            let n = carry as usize;
            carry -= n as f32;
            let block: Vec<f32> = (0..n)
                .map(|_| {
                    phase += 0.05;
                    phase.sin()
                })
                .collect();
            f.push(&block);
            let got = f.pop_into(&mut out);
            if i > 10 && got < 480 {
                dropouts += 1;
            }
        }
        assert_eq!(dropouts, 0);
        let level = f.inner.lock().buf.len();
        assert!(level < target * 4, "nivel fuera de control: {level}");
    }

    #[test]
    fn hermite_passes_through_samples() {
        assert_eq!(hermite(1.0, 2.0, 3.0, 4.0, 0.0), 2.0);
        assert!((hermite(1.0, 2.0, 3.0, 4.0, 0.5) - 2.5).abs() < 1e-6);
    }
}
