//! Compatibilidad con las configuraciones compartidas de SteelSeries Sonar.
//!
//! Al compartir una configuración, Sonar da un enlace como
//! `https://www.steelseries.com/deeplink/gg/sonar/config/v1/import?url=<base64>`. El parámetro `url`
//! es, en base64, la dirección de un JSON público con los ajustes (EQ paramétrico de 10 filtros,
//! Graves / Voz / Agudos, volumen inteligente, ganancia y audio espacial). Aquí se lee ese JSON y
//! se traduce a los ajustes de Rust Sonar; también se puede exportar en el mismo formato.

use crate::config::{
    Channel, EqBand, EqSettings, EqTarget, FilterKind, Loudness, Settings, SpatialMode, SpatialSettings, Tone,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::os::windows::process::CommandExt;

/// Único servidor del que se descargan configuraciones.
const CONFIG_HOST: &str = "https://community-configs.steelseriescdn.com/";
const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// Nombres de los altavoces en el JSON, en el orden de `SpatialSettings::speakers`.
const SPEAKER_KEYS: [&str; 8] =
    ["frontLeft", "frontRight", "center", "subWoofer", "rearLeft", "rearRight", "sideLeft", "sideRight"];

#[derive(Clone, Debug, PartialEq)]
pub struct SonarConfig {
    pub name: String,
    pub author: String,
    /// Canal para el que se creó (None si el archivo no lo dice).
    pub target: Option<EqTarget>,
    pub eq: EqSettings,
    pub gain_db: f32,
    pub smart_volume: bool,
    pub smart_level: f32,
    pub smart_loudness: Loudness,
    pub surround: SpatialSettings,
}

/// Saca la dirección del JSON de lo que pegue el usuario: el enlace de compartir, o directamente
/// la dirección del archivo.
pub fn config_url(text: &str) -> Result<String> {
    let text = text.trim();
    let url = match text.split_once("url=") {
        Some((_, rest)) => {
            let encoded = percent_decode(rest.split('&').next().unwrap_or(rest));
            String::from_utf8(base64_decode(&encoded)?).context(tr!("el enlace no contiene una dirección válida", "the link doesn't contain a valid address"))?
        }
        None => text.to_string(),
    };
    if !url.starts_with(CONFIG_HOST) || url.contains(['"', ' ', '\n']) {
        bail!(trf!("no parece un enlace de configuración de Sonar", "this doesn't look like a Sonar config link"));
    }
    Ok(url)
}

/// Descarga el JSON con el `curl.exe` que trae Windows (evita meter un cliente HTTP en el binario).
pub fn fetch(url: &str) -> Result<String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-f", "-L", "--max-time", "15", "--max-filesize", "262144", "--proto", "=https", url])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context(tr!("no se pudo ejecutar curl.exe", "couldn't run curl.exe"))?;
    if !out.status.success() {
        bail!(trf!("no se pudo descargar la configuración (¿enlace caducado o sin conexión?)", "couldn't download the config (expired link or offline?)"));
    }
    if out.stdout.len() > MAX_CONFIG_BYTES {
        bail!(trf!("la configuración es demasiado grande", "the config is too large"));
    }
    String::from_utf8(out.stdout).context(tr!("la configuración no es texto", "the config isn't text"))
}

fn num(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|x| x as f32).filter(|x| x.is_finite())
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

fn filter_kind(name: &str) -> FilterKind {
    match name.to_ascii_lowercase().as_str() {
        "lowshelving" | "lowshelf" => FilterKind::LowShelf,
        "highshelving" | "highshelf" => FilterKind::HighShelf,
        "lowpass" => FilterKind::LowPass,
        "highpass" => FilterKind::HighPass,
        "notch" | "notchfilter" => FilterKind::Notch,
        _ => FilterKind::Peaking,
    }
}

fn kind_name(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Peaking => "peakingEQ",
        FilterKind::LowShelf => "lowShelving",
        FilterKind::HighShelf => "highShelving",
        FilterKind::LowPass => "lowPass",
        FilterKind::HighPass => "highPass",
        FilterKind::Notch => "notchFilter",
    }
}

fn target_from(name: &str) -> Option<EqTarget> {
    Some(match name.to_ascii_lowercase().as_str() {
        "game" | "gaming" => EqTarget::Channel(Channel::Game),
        "chat" | "chatrender" => EqTarget::Channel(Channel::Chat),
        "media" => EqTarget::Channel(Channel::Media),
        "aux" => EqTarget::Channel(Channel::Aux),
        "mic" | "microphone" | "chatcapture" => EqTarget::Mic,
        _ => return None,
    })
}

/// Valor de un control "{enabled, value}": apagado cuenta como 0 dB.
fn boost(data: &Value, key: &str) -> f32 {
    let state = &data[key];
    if flag(state, "enabled").unwrap_or(true) { num(state, "value").unwrap_or(0.0).clamp(-12.0, 12.0) } else { 0.0 }
}

pub fn parse(json_text: &str) -> Result<SonarConfig> {
    let root: Value = serde_json::from_str(json_text).context(tr!("la configuración no es JSON válido", "the config isn't valid JSON"))?;
    let outer = &root["data"];
    let data = &outer["data"];
    if !data.is_object() {
        bail!(trf!("el archivo no tiene el formato de una configuración de Sonar", "the file isn't in Sonar config format"));
    }
    let global = flag(data, "globalEnableState").unwrap_or(true);

    let peq = &data["parametricEQ"];
    let mut bands = Vec::new();
    for i in 1..=32 {
        let f = &peq[format!("filter{i}")];
        if !f.is_object() {
            break;
        }
        bands.push(EqBand {
            enabled: flag(f, "enabled").unwrap_or(true),
            kind: filter_kind(f.get("type").and_then(Value::as_str).unwrap_or("peakingEQ")),
            freq: num(f, "frequency").unwrap_or(1000.0).clamp(10.0, 22_000.0),
            gain_db: num(f, "gain").unwrap_or(0.0).clamp(-24.0, 24.0),
            q: num(f, "qFactor").unwrap_or(0.707).clamp(0.1, 20.0),
        });
    }
    let mut eq = EqSettings::flat();
    if !bands.is_empty() {
        eq.bands = bands;
    }
    eq.enabled = global;
    if !flag(peq, "enabled").unwrap_or(true) {
        // EQ paramétrico apagado: sus filtros no actúan, pero Graves/Voz/Agudos sí.
        for b in &mut eq.bands {
            b.enabled = false;
        }
    }
    eq.tone = Tone {
        bass_db: boost(data, "bassBoostState"),
        voice_db: boost(data, "voiceClarityState"),
        treble_db: boost(data, "trebleBoostState"),
    };

    let mut surround = SpatialSettings {
        enabled: global && flag(data, "virtualSurroundState").unwrap_or(false),
        mode: match data.get("formFactor").and_then(Value::as_str) {
            Some(s) if s.eq_ignore_ascii_case("speakers") => SpatialMode::Speakers,
            _ => SpatialMode::Headphones,
        },
        ..Default::default()
    };
    if let Some(db) = num(data, "reverbGainDB") {
        surround.set_reverb_gain_db(db);
    }
    if let Some(d) = num(data, "distance") {
        surround.distance = d.clamp(0.0, 100.0);
    }
    for (slot, key) in surround.speakers.iter_mut().zip(SPEAKER_KEYS) {
        let sp = &data["virtualSurroundChannels"][key];
        // Sonar mide los grados hacia la izquierda; aquí positivo es a la derecha.
        if let Some(pos) = num(sp, "position") {
            slot.azimuth = -pos.clamp(-180.0, 180.0);
        }
        if let Some(g) = num(sp, "gain") {
            slot.gain_db = g.clamp(-24.0, 12.0);
        }
    }

    let smart = &data["smartVolume"];
    Ok(SonarConfig {
        name: root["metadata"]["name"].as_str().unwrap_or(tr!("Configuración de Sonar", "Sonar config")).trim().chars().take(60).collect(),
        author: root["metadata"]["author"].as_str().unwrap_or("").chars().take(60).collect(),
        target: outer.get("virtualAudioDevice").and_then(Value::as_str).and_then(target_from),
        eq,
        gain_db: num(data, "generalGain").unwrap_or(0.0).clamp(-12.0, 12.0),
        smart_volume: global && flag(smart, "enabled").unwrap_or(false),
        smart_level: num(smart, "volumeLevel").unwrap_or(0.0).clamp(0.0, 1.0),
        smart_loudness: match smart.get("loudness").and_then(Value::as_str).unwrap_or("balanced") {
            "quiet" => Loudness::Quiet,
            "loud" => Loudness::Loud,
            _ => Loudness::Balanced,
        },
        surround,
    })
}

/// Aplica la configuración a `target` (o al canal para el que se creó) y la guarda como preset.
/// Devuelve el canal al que se aplicó.
pub fn apply(s: &mut Settings, cfg: &SonarConfig, target: Option<EqTarget>) -> EqTarget {
    let t = target.or(cfg.target).unwrap_or(EqTarget::Channel(Channel::Game));
    *s.eq_for_mut(t) = cfg.eq.clone();
    if let EqTarget::Channel(ch) = t {
        let c = s.channel_mut(ch);
        c.boost_db = cfg.gain_db;
        c.smart_volume = cfg.smart_volume;
        c.smart_level = cfg.smart_level;
        c.smart_loudness = cfg.smart_loudness;
        c.surround = cfg.surround.clone();
    }
    s.save_preset(t, &cfg.name);
    t
}

/// Descarga, interpreta y aplica un enlace compartido. Devuelve un resumen legible.
pub fn import_link(shared: &crate::state::Shared, link: &str, target: Option<EqTarget>) -> Result<String> {
    let cfg = parse(&fetch(&config_url(link)?)?)?;
    let t = shared.update(|s| apply(s, &cfg, target));
    let by = if cfg.author.is_empty() { String::new() } else { trf!(" de {}", " by {}", cfg.author) };
    Ok(trf!("«{}»{by} aplicada a {} y guardada como preset.", "“{}”{by} applied to {} and saved as a preset.", cfg.name, t.label()))
}

/// Los ajustes de un canal en el formato de Sonar (para compartirlos o guardarlos).
pub fn export(s: &Settings, t: EqTarget, name: &str) -> Value {
    let eq = s.eq_for(t);
    let mut peq = serde_json::Map::new();
    peq.insert("enabled".into(), json!(eq.bands.iter().any(|b| b.enabled)));
    for (i, b) in eq.bands.iter().enumerate() {
        peq.insert(
            format!("filter{}", i + 1),
            json!({ "enabled": b.enabled, "qFactor": b.q, "frequency": b.freq, "gain": b.gain_db, "type": kind_name(b.kind) }),
        );
    }
    let boost = |v: f32| json!({ "enabled": v != 0.0, "value": v });
    let (channel, device) = match t {
        EqTarget::Channel(ch) => (Some(s.channel(ch)), ["game", "chat", "media", "aux"][ch.index()]),
        EqTarget::Mic => (None, "mic"),
    };
    let surround = channel.map(|c| c.surround.clone()).unwrap_or_default();
    let mut speakers = serde_json::Map::new();
    for (sp, key) in surround.speakers.iter().zip(SPEAKER_KEYS) {
        speakers.insert(key.into(), json!({ "position": -sp.azimuth, "gain": sp.gain_db }));
    }
    let loudness = match channel.map(|c| c.smart_loudness).unwrap_or_default() {
        Loudness::Quiet => "quiet",
        Loudness::Balanced => "balanced",
        Loudness::Loud => "loud",
    };
    json!({
        "data": {
            "data": {
                "bassBoostState": boost(eq.tone.bass_db),
                "trebleBoostState": boost(eq.tone.treble_db),
                "voiceClarityState": boost(eq.tone.voice_db),
                "smartVolume": {
                    "enabled": channel.map(|c| c.smart_volume).unwrap_or(false),
                    "volumeLevel": channel.map(|c| c.smart_level).unwrap_or(0.0),
                    "loudness": loudness,
                },
                "generalGain": channel.map(|c| c.boost_db).unwrap_or(0.0),
                "parametricEQ": peq,
                "virtualSurroundState": surround.enabled,
                "virtualSurroundChannels": speakers,
                "reverbGainDB": surround.reverb_gain_db(),
                "formFactor": if surround.mode == SpatialMode::Speakers { "speakers" } else { "headphones" },
                "globalEnableState": eq.enabled,
            },
            "schemaVersion": 5,
            "virtualAudioDevice": device,
        },
        "metadata": { "name": name, "author": "Rust Sonar" },
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len()).then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok()).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Base64 estándar o "URL-safe", con o sin relleno.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return Err(anyhow!(trf!("el enlace está incompleto o dañado", "the link is incomplete or damaged"))),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://www.steelseries.com/deeplink/gg/sonar/config/v1/import?url=aHR0cHM6Ly9jb21tdW5pdHktY29uZmlncy5zdGVlbHNlcmllc2Nkbi5jb20vdjEvYzY2NTZhNWUtNWM0Ni00NWJiLWI4MTQtMzI1OGZkNzFlY2Ex";

    /// Una configuración real exportada por Sonar (esquema 5), con el autor cambiado.
    const SAMPLE: &str = r#"{"data":{"data":{"bassBoostState":{"enabled":true,"value":12},"trebleBoostState":{"enabled":true,"value":-5.9},"voiceClarityState":{"enabled":true,"value":6.9},"smartVolume":{"enabled":false,"volumeLevel":0,"loudness":"balanced"},"generalGain":0,"parametricEQ":{"enabled":true,"filter1":{"enabled":true,"qFactor":0.5000000000000007,"frequency":27.573642812193516,"gain":10.792151162790699,"type":"peakingEQ"},"filter2":{"enabled":true,"qFactor":0.5000000000000007,"frequency":81.23621635138717,"gain":10.876453488372096,"type":"peakingEQ"},"filter3":{"enabled":true,"qFactor":0.5000000000000007,"frequency":196.51039032115102,"gain":10.754360465116275,"type":"peakingEQ"},"filter4":{"enabled":true,"qFactor":0.707,"frequency":423.6744520854899,"gain":5.574127906976749,"type":"peakingEQ"},"filter5":{"enabled":true,"qFactor":0.707,"frequency":762.190413562339,"gain":3.4375,"type":"peakingEQ"},"filter6":{"enabled":true,"qFactor":0.707,"frequency":1226.5518258778422,"gain":-2.0784883720930267,"type":"peakingEQ"},"filter7":{"enabled":true,"qFactor":0.707,"frequency":2637.4551942928956,"gain":-3.517441860465116,"type":"peakingEQ"},"filter8":{"enabled":true,"qFactor":2.131999999999998,"frequency":4415.425131248015,"gain":2.3982558139534866,"type":"peakingEQ"},"filter9":{"enabled":true,"qFactor":0.707,"frequency":11519.099623032425,"gain":5.957848837209301,"type":"peakingEQ"},"filter10":{"enabled":true,"qFactor":0.5000000000000007,"frequency":19999.999999999985,"gain":2.8779069767441854,"type":"peakingEQ"}},"virtualSurroundState":false,"virtualSurroundChannels":{"frontLeft":{"position":30,"gain":0},"frontRight":{"position":-30,"gain":0},"center":{"position":0,"gain":0},"subWoofer":{"position":0,"gain":0},"rearLeft":{"position":150,"gain":0},"rearRight":{"position":-150,"gain":0},"sideLeft":{"position":90,"gain":0},"sideRight":{"position":-90,"gain":0}},"reverbGainDB":-6,"formFactor":"headphones","globalEnableState":true},"schemaVersion":5,"virtualAudioDevice":"aux"},"metadata":{"name":"Music: Deep Bass","author":"alguien#0000"}}"#;

    #[test]
    fn share_link_decodes_to_the_config_url() {
        let url = config_url(LINK).unwrap();
        assert_eq!(url, "https://community-configs.steelseriescdn.com/v1/c6656a5e-5c46-45bb-b814-3258fd71eca1");
        // La dirección directa también vale; cualquier otra web, no.
        assert_eq!(config_url(&url).unwrap(), url);
        assert!(config_url("https://example.com/config.json").is_err());
        assert!(config_url("https://x.test/import?url=aHR0cHM6Ly9ldmlsLnRlc3Qv").is_err());
    }

    #[test]
    fn parses_a_real_sonar_config() {
        let cfg = parse(SAMPLE).unwrap();
        assert_eq!(cfg.name, "Music: Deep Bass");
        assert_eq!(cfg.target, Some(EqTarget::Channel(Channel::Aux)));
        assert_eq!(cfg.eq.bands.len(), 10);
        assert!((cfg.eq.bands[7].q - 2.132).abs() < 1e-3);
        assert!((cfg.eq.bands[0].freq - 27.57).abs() < 0.01);
        assert_eq!(cfg.eq.tone, Tone { bass_db: 12.0, voice_db: 6.9, treble_db: -5.9 });
        assert!(!cfg.surround.enabled);
        assert_eq!(cfg.surround.mode, SpatialMode::Headphones);
        assert!((cfg.surround.immersion - 0.5).abs() < 1e-6);
        // 30° a la izquierda en Sonar = −30° aquí.
        assert_eq!(cfg.surround.speakers[0].azimuth, -30.0);
        assert_eq!(cfg.surround.speakers[5].azimuth, 150.0);
        assert_eq!(cfg.surround, SpatialSettings::default());
    }

    #[test]
    fn apply_goes_to_its_channel_and_saves_a_preset() {
        let mut s = Settings::default();
        let cfg = parse(SAMPLE).unwrap();
        let t = apply(&mut s, &cfg, None);
        assert_eq!(t, EqTarget::Channel(Channel::Aux));
        assert_eq!(s.channel(Channel::Aux).eq.tone.bass_db, 12.0);
        assert_eq!(s.active_preset(t), Some("Music: Deep Bass"));
        assert!(!s.preset_modified(t));
        // Se puede forzar otro canal.
        assert_eq!(apply(&mut s, &cfg, Some(EqTarget::Channel(Channel::Game))), EqTarget::Channel(Channel::Game));
        assert_eq!(s.channel(Channel::Game).eq.bands.len(), 10);
    }

    #[test]
    fn export_roundtrips_through_parse() {
        let mut s = Settings::default();
        let cfg = parse(SAMPLE).unwrap();
        let t = apply(&mut s, &cfg, None);
        let again = parse(&export(&s, t, "Music: Deep Bass").to_string()).unwrap();
        assert_eq!(again.eq, cfg.eq);
        assert_eq!(again.surround, cfg.surround);
        assert_eq!(again.target, cfg.target);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("no es json").is_err());
        assert!(parse(r#"{"data":{}}"#).is_err());
        assert!(config_url("…?url=!!!").is_err());
    }
}
