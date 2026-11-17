//! Cadena del micrófono:
//!
//! ```text
//! micro ─► ganancia ─► RNNoise (supresión + VAD) ─► puerta por voz ─► puerta por nivel ─► EQ ─► compresor ─► volumen ─┬─► cable virtual
//!                                                                                                              ├─► escucha (auriculares)
//!                                                                                                              └─► retransmisión (modo streamer)
//! ```
//!
//! La salida va a un dispositivo de reproducción (p. ej. "CABLE Input" de VB-Cable); en Discord se elige
//! su pareja de grabación ("CABLE Output") como micrófono.

use super::fifo::Fifo;
use super::wasapi::{self, CaptureStream, ComGuard, RenderStream};
use crate::config::{MicSettings, SAMPLE_RATE};
use crate::dsp::compressor::{Compressor, CompressorParams};
use crate::dsp::biquad::{Biquad, Coeffs};
use crate::dsp::{SmoothedGain, db_to_lin, eq::Equalizer, gate::NoiseGate, gate::VoiceFloor, peak};
use crate::state::Shared;
use crate::virtual_devices as vdev;
use nnnoiseless::DenoiseState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::{eCapture, eRender};

const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 muestras = 10 ms a 48 kHz
/// Colchón hacia el cable virtual: 20 ms (dos bloques de RNNoise).
const OUT_LATENCY_FRAMES: usize = FRAME * 2;

pub fn spawn(shared: Arc<Shared>) {
    thread::Builder::new().name("micro".into()).spawn(move || supervisor(shared)).unwrap();
}

/// Arranca/para la cadena según la configuración (activado, dispositivos elegidos).
fn supervisor(shared: Arc<Shared>) {
    while !shared.quit.load(Ordering::Relaxed) {
        let mic = shared.settings.read().mic.clone();
        if mic.enabled && shared.paused.load(Ordering::Relaxed) {
            shared.status.write().mic = tr!("En pausa (SteelSeries GG está abierto)", "Paused (SteelSeries GG is open)").into();
            shared.meters.mic_in.store(0.0);
            shared.meters.mic_out.store(0.0);
            thread::sleep(Duration::from_millis(300));
            continue;
        }
        if !mic.enabled {
            shared.status.write().mic = tr!("Desactivado", "Off").into();
            shared.meters.mic_in.store(0.0);
            shared.meters.mic_out.store(0.0);
            thread::sleep(Duration::from_millis(300));
            continue;
        }
        if mic.output_device.is_none() {
            shared.status.write().mic = tr!("Falta la salida virtual: pulsa «Configurar automáticamente» en Ajustes", "Missing virtual output: press “Set up automatically” in Settings").into();
            thread::sleep(Duration::from_millis(300));
            continue;
        }
        // Bajo demanda: sin nadie grabando del micro virtual (ni la ventana abierta) no se abre el
        // micrófono ni se pasa la IA. Ahorra CPU y Windows no muestra el aviso de micro en uso.
        if !shared.mic_wanted.load(Ordering::Relaxed) {
            shared.status.write().mic = tr!("En reposo: se activa en cuanto una app use el micrófono", "Idle: wakes up as soon as an app uses the microphone").into();
            shared.meters.mic_in.store(0.0);
            shared.meters.mic_out.store(0.0);
            shared.meters.mic_vad.store(0.0);
            shared.mic_wake.wait(Duration::from_secs(2));
            continue;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let fifo = Arc::new(Fifo::new(1, OUT_LATENCY_FRAMES));

        let (s, st, f) = (shared.clone(), stop.clone(), fifo.clone());
        let input_dev = mic.input_device.clone();
        let cap = thread::spawn(move || {
            if let Err(e) = capture_chain(&s, &st, &f, input_dev.as_deref()) {
                s.status.write().mic = trf!("Error en el micro: {e:#}", "Microphone error: {e:#}");
                st.store(true, Ordering::Relaxed);
            }
        });

        let (s, st, f) = (shared.clone(), stop.clone(), fifo.clone());
        let output_dev = mic.output_device.clone();
        let out = thread::spawn(move || {
            if let Err(e) = output_chain(&s, &st, &f, output_dev.as_deref()) {
                s.status.write().mic = trf!("Error en la salida del micro: {e:#}", "Mic output error: {e:#}");
                st.store(true, Ordering::Relaxed);
            }
        });

        // Vigila cambios de dispositivo / desactivación, y suelta el micro cuando deja de hacer falta.
        let mut unused_since: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) && !shared.quit.load(Ordering::Relaxed) {
            let now = shared.settings.read().mic.clone();
            let paused = shared.paused.load(Ordering::Relaxed);
            if paused || !now.enabled || now.input_device != mic.input_device || now.output_device != mic.output_device {
                break;
            }
            if shared.mic_wanted.load(Ordering::Relaxed) {
                unused_since = None;
            } else if unused_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
                break;
            }
            shared.mic_wake.wait(Duration::from_millis(250));
        }
        stop.store(true, Ordering::Relaxed);
        let _ = cap.join();
        let _ = out.join();
        shared.monitor.clear();
        shared.stream_mic.clear();
        thread::sleep(Duration::from_millis(300));
    }
}

/// Abre el micrófono físico. Nunca el micro virtual por el que sale nuestra propia señal (sería un
/// bucle): si el predeterminado de Windows es ese, se busca uno real.
fn open_input(shared: &Shared, wanted: Option<&str>) -> anyhow::Result<(CaptureStream, String, String)> {
    let outputs = wasapi::list_devices(eRender)?;
    let inputs = wasapi::list_devices(eCapture)?;
    let virtual_in = vdev::virtual_mic_input(&shared.settings.read(), &outputs, &inputs);
    let (mut dev, mut name, mut id) = wasapi::get_device(wanted, eCapture)?;
    if Some(&id) == virtual_in.as_ref() || (wanted.is_none() && vdev::is_virtual_cable(&name)) {
        if wanted.is_some() {
            anyhow::bail!(trf!("«{name}» es el micrófono virtual; elige tu micrófono físico", "“{name}” is the virtual microphone; pick your physical microphone"));
        }
        let real = vdev::pick_real(&inputs, &[], None).ok_or_else(|| anyhow::anyhow!(trf!("no hay ningún micrófono físico conectado", "no physical microphone connected")))?;
        (dev, name, id) = wasapi::get_device(Some(&real), eCapture)?;
    }
    Ok((CaptureStream::device(&dev)?, name, id))
}

fn capture_chain(shared: &Shared, stop: &AtomicBool, fifo: &Fifo, device: Option<&str>) -> anyhow::Result<()> {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let (mut stream, name, id) = open_input(shared, device)?;
    debug_assert_eq!(stream.channels(), 1);
    *shared.active_mic.write() = Some(id);
    shared.status.write().mic = trf!("Micro: {name}", "Mic: {name}");

    let mut proc = MicProcessor::new();
    let mut version = 0;
    let mut to_stream = false;
    let mut scratch = Vec::new();
    let mut pending: Vec<f32> = Vec::with_capacity(FRAME * 2);

    while !stop.load(Ordering::Relaxed) {
        let v = shared.version();
        if v != version {
            if let Some(s) = shared.settings.try_read() {
                proc.configure(&s.mic);
                to_stream = s.stream_mic_gain() > 0.0;
                version = v;
            }
        }
        stream.read(100, &mut scratch, |samples| {
            shared.meters.mic_in.store(peak(samples));
            pending.extend_from_slice(samples);
        })?;
        // RNNoise trabaja en bloques fijos de 480 muestras.
        while pending.len() >= FRAME {
            let mut frame = [0.0f32; FRAME];
            frame.copy_from_slice(&pending[..FRAME]);
            pending.drain(..FRAME);
            proc.process(&mut frame);
            shared.meters.mic_out.store(peak(&frame));
            shared.meters.mic_vad.store(proc.last_vad);
            fifo.push(&frame);
            if proc.cfg.monitor {
                shared.monitor.push(&frame);
            }
            if to_stream {
                shared.stream_mic.push(&frame);
            }
        }
    }
    Ok(())
}

fn output_chain(shared: &Shared, stop: &AtomicBool, fifo: &Fifo, device: Option<&str>) -> anyhow::Result<()> {
    let _com = ComGuard::init();
    wasapi::boost_thread_priority();
    let (dev, name, _) = wasapi::get_device(device, eRender)?;
    let mut stream = RenderStream::open(&dev, 2)?;
    {
        let mut st = shared.status.write();
        st.mic = format!("{} → {name}", st.mic);
    }
    let mut mono = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        stream.write(50, |out| {
            // Mono → estéreo duplicando la muestra en L y R.
            mono.resize(out.len() / 2, 0.0);
            fifo.pop_into(&mut mono);
            for (frame, &x) in out.chunks_exact_mut(2).zip(&mono) {
                frame[0] = x;
                frame[1] = x;
            }
        })?;
    }
    Ok(())
}

pub struct MicProcessor {
    denoise: Box<DenoiseState<'static>>,
    /// Segunda pasada de la red (nivel Máxima).
    denoise2: Box<DenoiseState<'static>>,
    /// Paso alto antes de la red: fuera el zumbido y los golpes de mesa, que no son voz.
    rumble: Biquad,
    voice_floor: VoiceFloor,
    gate: NoiseGate,
    vad_gain: SmoothedGain,
    out_gain: SmoothedGain,
    eq: Equalizer,
    compressor: Compressor,
    cfg: MicSettings,
    pub last_vad: f32,
}

impl MicProcessor {
    pub fn new() -> Self {
        let fs = SAMPLE_RATE as f32;
        Self {
            denoise: DenoiseState::new(),
            denoise2: DenoiseState::new(),
            rumble: {
                let mut b = Biquad::default();
                b.set_coeffs(Coeffs::design(crate::config::FilterKind::HighPass, 90.0, 0.0, 0.707, fs as f64));
                b
            },
            voice_floor: VoiceFloor::new(fs),
            gate: NoiseGate::new(fs),
            vad_gain: SmoothedGain::new(fs, 0.03),
            out_gain: SmoothedGain::new(fs, 0.015),
            eq: Equalizer::new(1),
            compressor: Compressor::new(fs),
            cfg: MicSettings::default(),
            last_vad: 0.0,
        }
    }

    pub fn configure(&mut self, cfg: &MicSettings) {
        self.eq.configure(&cfg.eq);
        self.cfg = cfg.clone();
    }

    pub fn process(&mut self, frame: &mut [f32; FRAME]) {
        let pre = db_to_lin(self.cfg.gain_db);
        for x in frame.iter_mut() {
            *x *= pre;
        }

        if self.cfg.noise_suppression || self.cfg.vad_gate {
            let level = self.cfg.suppression;
            if self.cfg.noise_suppression {
                for x in frame.iter_mut() {
                    *x = self.rumble.process(*x);
                }
            }
            // RNNoise espera la escala de un i16 (±32768), no ±1.0.
            let mut input = [0.0f32; FRAME];
            for (i, x) in frame.iter().enumerate() {
                input[i] = x * 32768.0;
            }
            let mut output = [0.0f32; FRAME];
            self.last_vad = self.denoise.process_frame(&mut output, &input);
            if self.cfg.noise_suppression {
                if level.passes() > 1 {
                    // La segunda pasada trabaja sobre lo ya limpio y se lleva lo que quedaba.
                    let first = output;
                    self.denoise2.process_frame(&mut output, &first);
                }
                for (x, y) in frame.iter_mut().zip(output) {
                    *x = y / 32768.0;
                }
                // Restos entre palabras: abajo cuando la red no oye voz.
                self.voice_floor.process(frame, self.last_vad, level.floor_db(), level.hold_s());
            }
        }

        if self.cfg.vad_gate {
            let target = if self.last_vad >= self.cfg.vad_threshold { 1.0 } else { 0.0 };
            self.vad_gain.apply(frame, 1, target);
        }
        if self.cfg.gate_enabled {
            self.gate.process(frame, self.cfg.gate_threshold_db, self.cfg.gate_release_ms);
        }
        self.eq.process(frame);
        self.compressor.process(frame, 1, &CompressorParams::VOICE, self.cfg.compressor);
        for x in frame.iter_mut() {
            *x = crate::dsp::soft_clip(*x);
        }
        let vol = if self.cfg.muted { 0.0 } else { self.cfg.volume };
        self.out_gain.apply(frame, 1, vol);
    }
}

#[cfg(test)]
mod suppression_tests {
    use super::*;
    use crate::config::Suppression;

    /// Ruido de prueba: siseo constante a unos −34 dBFS con un clic fuerte (tecla) cada 150 ms.
    fn noise(seconds: f32) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let hiss = ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.035;
                let t = i % 7200;
                let click = if t < 96 { (t as f32 * 0.9).sin() * 0.25 * (1.0 - t as f32 / 96.0) } else { 0.0 };
                hiss + click
            })
            .collect()
    }

    /// Nivel RMS (dB) de lo que sale por el micro con ese ruido, descartando el primer segundo
    /// (lo que tarda la red en adaptarse).
    fn residual_db(level: Option<Suppression>) -> f32 {
        let mut p = MicProcessor::new();
        let cfg = MicSettings { noise_suppression: level.is_some(), suppression: level.unwrap_or_default(), ..Default::default() };
        p.configure(&cfg);
        let input = noise(4.0);
        let (mut sum, mut count) = (0.0f64, 0usize);
        for (i, chunk) in input.chunks_exact(FRAME).enumerate() {
            let mut frame = [0.0f32; FRAME];
            frame.copy_from_slice(chunk);
            p.process(&mut frame);
            if i >= 100 {
                sum += frame.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
                count += FRAME;
            }
        }
        10.0 * ((sum / count as f64).max(1e-12)).log10() as f32
    }

    /// WAV PCM de 16 bits, mono, 48 kHz (el que genera la voz sintética de Windows).
    fn read_wav(path: &str) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("no se pudo leer el WAV");
        let data = bytes.windows(4).position(|w| w == b"data").expect("sin bloque data") + 8;
        bytes[data..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
    }

    fn run(level: Option<Suppression>, input: &[f32]) -> Vec<f32> {
        let mut p = MicProcessor::new();
        p.configure(&MicSettings { noise_suppression: level.is_some(), suppression: level.unwrap_or_default(), ..Default::default() });
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks_exact(FRAME) {
            let mut frame = [0.0f32; FRAME];
            frame.copy_from_slice(chunk);
            p.process(&mut frame);
            out.extend_from_slice(&frame);
        }
        out
    }

    fn rms_db(x: impl Iterator<Item = f32>) -> f32 {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for v in x {
            sum += (v as f64).powi(2);
            n += 1;
        }
        10.0 * ((sum / n.max(1) as f64).max(1e-12)).log10() as f32
    }

    /// Con una voz de verdad (`RUST_SONAR_VOICE_WAV`): cuánta voz se pierde en cada nivel y cuánto
    /// ruido queda en las pausas, con la voz sola y con ruido encima.
    /// `cargo test --release -- --ignored probe_voice_quality --nocapture`
    #[test]
    #[ignore]
    fn probe_voice_quality() {
        let voice = read_wav(&std::env::var("RUST_SONAR_VOICE_WAV").expect("falta RUST_SONAR_VOICE_WAV"));
        // Dos segundos de silencio al final: ahí se ve lo que queda cuando de verdad no hablas.
        let mut voice = voice;
        let spoken = voice.len() / FRAME;
        voice.extend(std::iter::repeat(0.0).take(2 * SAMPLE_RATE as usize));
        let hiss = noise(voice.len() as f32 / SAMPLE_RATE as f32 + 1.0);
        let noisy: Vec<f32> = voice.iter().zip(&hiss).map(|(v, n)| v + n).collect();
        let tail = |x: &[f32]| rms_db(x.chunks_exact(FRAME).skip(spoken + 50).flat_map(|c| c.iter().copied()));
        // Dónde hay voz y dónde pausa, según la voz limpia (bloques de 10 ms).
        let speech: Vec<bool> = voice.chunks_exact(FRAME).map(|c| rms_db(c.iter().copied()) > -40.0).collect();
        let part = |x: &[f32], want: bool| rms_db(x.chunks_exact(FRAME).zip(&speech).skip(100).filter(|(_, s)| **s == want).flat_map(|(c, _)| c.iter().copied()));
        println!("VOZ en la entrada: {:.1} dB al hablar | ruido añadido en las pausas: {:.1} dB", part(&voice, true), part(&noisy, false));
        for level in [None].into_iter().chain(Suppression::ALL.into_iter().map(Some)) {
            let clean = run(level, &voice);
            let mixed = run(level, &noisy);
            let name = level.map(|l| l.label()).unwrap_or("sin supresión");
            println!(
                "NIVEL {name:<14} voz sola: {:+.1} dB | con ruido: al hablar {:.1} dB, entre palabras {:.1} dB, en silencio {:.1} dB",
                part(&clean, true) - part(&voice, true),
                part(&mixed, true),
                rms_db(mixed.chunks_exact(FRAME).zip(&speech).take(spoken).skip(100).filter(|(_, s)| !**s).flat_map(|(c, _)| c.iter().copied())),
                tail(&mixed)
            );
        }
    }

    #[test]
    fn stronger_levels_leave_less_noise() {
        let off = residual_db(None);
        let levels: Vec<f32> = Suppression::ALL.iter().map(|l| residual_db(Some(*l))).collect();
        println!("RUIDO sin supresión {off:.1} dB | suave {:.1} | normal {:.1} | fuerte {:.1} | máxima {:.1}", levels[0], levels[1], levels[2], levels[3]);
        assert!(levels[0] < off - 4.0, "la red sola ya debe quitar ruido");
        for pair in levels.windows(2) {
            assert!(pair[1] < pair[0] - 3.0, "cada nivel debe limpiar más que el anterior: {levels:?}");
        }
    }
}
