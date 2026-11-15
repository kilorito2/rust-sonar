//! API REST local (http://127.0.0.1:7878) para controlar el mezclador desde fuera:
//! Stream Deck, AutoHotkey, una web, un bot de Discord...

use crate::config::{Channel, EqSettings, EqTarget};
use crate::state::Shared;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

type AppState = Arc<Shared>;
type ApiResult = Result<Json<Value>, (StatusCode, String)>;

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("api".into())
        .spawn(move || {
            // Un solo hilo basta para una API local: menos hilos y menos memoria.
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            if let Err(e) = rt.block_on(serve(shared)) {
                eprintln!("[api] {e:#}");
            }
        })
        .unwrap();
}

pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/state", get(state))
        .route("/api/devices", get(devices))
        .route("/api/apps", get(apps))
        .route("/api/apps/{name}", put(set_app_channel))
        .route("/api/master", patch(set_master))
        .route("/api/chatmix", put(set_chatmix))
        .route("/api/channels/{ch}", patch(patch_channel))
        .route("/api/channels/{ch}/eq", put(set_eq))
        .route("/api/channels/{ch}/eq/preset/{name}", post(set_eq_preset))
        .route("/api/mic", patch(patch_mic))
        .route("/api/setup", post(auto_setup))
        .route("/api/devices/tidy", post(tidy_devices))
        .route("/api/devices/show-hidden", post(show_hidden_devices))
        .route("/api/import", post(import_sonar))
        .route("/api/channels/{ch}/export", get(export_sonar))
        .route("/api/quit", post(quit))
        // Sincronización con la ventana (que va en su propio proceso).
        .route("/api/ui/state", get(ui_state))
        .route("/api/ui/meters", get(ui_meters))
        .route("/api/ui/settings", put(ui_settings))
        .with_state(shared)
}

async fn serve(shared: Arc<Shared>) -> anyhow::Result<()> {
    let port = shared.settings.read().api_port;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("[api] escuchando en http://127.0.0.1:{port}/api/state");
    axum::serve(listener, router(shared)).await?;
    Ok(())
}

fn parse_channel(s: &str) -> Result<Channel, (StatusCode, String)> {
    Channel::parse(s).ok_or((StatusCode::NOT_FOUND, trf!("canal desconocido: {s} (game|chat|media|aux)", "unknown channel: {s} (game|chat|media|aux)")))
}

fn full_state(sh: &Shared) -> Value {
    json!({
        "settings": *sh.settings.read(),
        "apps": *sh.apps.read(),
        "meters": sh.meters.snapshot(),
        "status": *sh.status.read(),
        "active_output": *sh.active_output.read(),
        "active_mic": *sh.active_mic.read(),
        "paused": sh.paused.load(std::sync::atomic::Ordering::Relaxed),
    })
}

/// GET /api/ui/state: todo lo que la ventana necesita, salvo los vúmetros.
async fn ui_state(State(sh): State<AppState>) -> Json<Value> {
    use std::sync::atomic::Ordering::Relaxed;
    Json(json!({
        "settings": *sh.settings.read(),
        "apps": *sh.apps.read(),
        "devices": *sh.devices.read(),
        "status": *sh.status.read(),
        "paused": sh.paused.load(Relaxed),
        "idle": sh.idle.load(Relaxed),
        "active_output": *sh.active_output.read(),
    }))
}

#[derive(Deserialize)]
struct MetersQuery {
    mic: Option<u8>,
}

/// GET /api/ui/meters?mic=1: vúmetros. Con mic=1 la ventana pide además que el micro esté activo
/// (para ver su nivel) durante un segundo y medio; deja de procesarse si deja de pedirlo.
async fn ui_meters(State(sh): State<AppState>, Query(q): Query<MetersQuery>) -> Json<crate::state::MetersSnapshot> {
    use std::sync::atomic::Ordering::Relaxed;
    if q.mic == Some(1) {
        sh.ui_mic_until.store(crate::state::unix_ms() + 1500, Relaxed);
        if !sh.mic_wanted.load(Relaxed) {
            sh.wake.wake();
        }
    }
    Json(sh.meters.snapshot())
}

/// PUT /api/ui/settings: la ventana manda la configuración entera tras cada cambio.
async fn ui_settings(State(sh): State<AppState>, Json(new): Json<crate::config::Settings>) -> StatusCode {
    sh.replace(new);
    StatusCode::NO_CONTENT
}

async fn state(State(sh): State<AppState>) -> Json<Value> {
    Json(full_state(&sh))
}

async fn devices(State(sh): State<AppState>) -> Json<Value> {
    Json(json!(*sh.devices.read()))
}

async fn apps(State(sh): State<AppState>) -> Json<Value> {
    Json(json!(*sh.apps.read()))
}

#[derive(Deserialize)]
struct ChannelBody {
    channel: String,
}

/// PUT /api/apps/discord {"channel":"chat"}
async fn set_app_channel(State(sh): State<AppState>, Path(name): Path<String>, Json(b): Json<ChannelBody>) -> ApiResult {
    let ch = parse_channel(&b.channel)?;
    sh.update(|s| {
        s.app_routes.insert(name.to_ascii_lowercase(), ch);
    });
    Ok(Json(json!({ "app": name, "channel": ch })))
}

/// PATCH /api/master {"volume":0.8,"muted":false}
async fn set_master(State(sh): State<AppState>, Json(b): Json<ChannelPatch>) -> ApiResult {
    sh.update(|s| {
        if let Some(v) = b.volume {
            s.master_volume = v.clamp(0.0, 1.0);
        }
        if let Some(m) = b.muted {
            s.master_muted = m;
        }
    });
    Ok(Json(full_state(&sh)))
}

#[derive(Deserialize)]
struct ChatMixBody {
    value: Option<f32>,
    enabled: Option<bool>,
}

/// PUT /api/chatmix {"value":-0.5}   (-1 = todo juego, 1 = todo chat)
async fn set_chatmix(State(sh): State<AppState>, Json(b): Json<ChatMixBody>) -> ApiResult {
    sh.update(|s| {
        if let Some(v) = b.value {
            s.chatmix = v.clamp(-1.0, 1.0);
        }
        if let Some(e) = b.enabled {
            s.chatmix_enabled = e;
        }
    });
    Ok(Json(full_state(&sh)))
}

/// POST /api/devices/tidy: renombra los dispositivos de Rust Sonar y oculta el resto (pide UAC).
async fn tidy_devices(State(sh): State<AppState>) -> ApiResult {
    device_admin(sh, crate::virtual_devices::tidy_windows_devices).await
}

/// POST /api/devices/show-hidden: vuelve a mostrar lo ocultado (pide UAC).
async fn show_hidden_devices(State(sh): State<AppState>) -> ApiResult {
    device_admin(sh, crate::virtual_devices::show_hidden_devices).await
}

async fn device_admin(sh: AppState, f: fn(&crate::state::Shared) -> anyhow::Result<String>) -> ApiResult {
    let result = tokio::task::spawn_blocking(move || {
        let _com = crate::audio::wasapi::ComGuard::init();
        f(&sh).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match result {
        Ok(summary) => Ok(Json(json!({ "summary": summary }))),
        Err(e) => Err((StatusCode::CONFLICT, e)),
    }
}

#[derive(Deserialize)]
struct ChannelPatch {
    volume: Option<f32>,
    muted: Option<bool>,
    boost_db: Option<f32>,
    smart_volume: Option<bool>,
    spatial: Option<bool>,
}

/// POST /api/setup: configuración en un clic (igual que el botón de Ajustes).
async fn auto_setup(State(sh): State<AppState>) -> ApiResult {
    let result = tokio::task::spawn_blocking(move || {
        use crate::audio::wasapi;
        use windows::Win32::Media::Audio::{eCapture, eConsole, eRender};
        let _com = wasapi::ComGuard::init();
        let outputs = wasapi::list_devices(eRender).map_err(|e| format!("{e:#}"))?;
        let inputs = wasapi::list_devices(eCapture).map_err(|e| format!("{e:#}"))?;
        let default_out = wasapi::default_device_id(eRender, eConsole);
        let default_in = wasapi::default_device_id(eCapture, eConsole);
        let summary = sh.update(|s| crate::virtual_devices::auto_setup(s, &outputs, &inputs, default_out.as_deref(), default_in.as_deref()))?;
        Ok::<_, String>((summary, full_state(&sh)))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match result {
        Ok((summary, state)) => Ok(Json(json!({ "summary": summary, "state": state }))),
        Err(e) => Err((StatusCode::CONFLICT, e)),
    }
}

#[derive(Deserialize)]
struct ImportBody {
    link: String,
    channel: Option<String>,
}

/// POST /api/import {"link":"https://www.steelseries.com/deeplink/gg/sonar/config/v1/import?url=…"}
/// Importa una configuración compartida de SteelSeries Sonar. `channel` es opcional.
async fn import_sonar(State(sh): State<AppState>, Json(b): Json<ImportBody>) -> ApiResult {
    let target = match &b.channel {
        Some(c) => Some(EqTarget::parse(c).ok_or((StatusCode::NOT_FOUND, trf!("canal desconocido: {c}", "unknown channel: {c}")))?),
        None => None,
    };
    let result = tokio::task::spawn_blocking(move || {
        crate::sonar_import::import_link(&sh, &b.link, target).map(|msg| (msg, full_state(&sh))).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match result {
        Ok((summary, state)) => Ok(Json(json!({ "summary": summary, "state": state }))),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

/// GET /api/channels/{game|chat|media|aux|mic}/export: los ajustes en el formato de Sonar.
async fn export_sonar(State(sh): State<AppState>, Path(target): Path<String>) -> ApiResult {
    let t = EqTarget::parse(&target).ok_or((StatusCode::NOT_FOUND, trf!("canal desconocido: {target}", "unknown channel: {target}")))?;
    let s = sh.settings.read();
    let name = s.active_preset(t).unwrap_or("Rust Sonar").to_string();
    Ok(Json(crate::sonar_import::export(&s, t, &name)))
}

/// POST /api/quit: cierra Rust Sonar ordenadamente (restaura el sonido de Windows).
async fn quit(State(sh): State<AppState>) -> ApiResult {
    crate::request_quit(&sh);
    Ok(Json(json!({ "quitting": true })))
}

/// PATCH /api/channels/game {"volume":0.5,"muted":false}
async fn patch_channel(State(sh): State<AppState>, Path(ch): Path<String>, Json(b): Json<ChannelPatch>) -> ApiResult {
    let ch = parse_channel(&ch)?;
    sh.update(|s| {
        let c = s.channel_mut(ch);
        if let Some(v) = b.volume {
            c.volume = v.clamp(0.0, 1.0);
        }
        if let Some(m) = b.muted {
            c.muted = m;
        }
        if let Some(v) = b.boost_db {
            c.boost_db = v.clamp(-12.0, 12.0);
        }
        if let Some(v) = b.smart_volume {
            c.smart_volume = v;
        }
        if let Some(v) = b.spatial {
            c.surround.enabled = v;
        }
    });
    Ok(Json(full_state(&sh)))
}

/// PUT /api/channels/game/eq {<EqSettings completo>}
async fn set_eq(State(sh): State<AppState>, Path(ch): Path<String>, Json(eq): Json<EqSettings>) -> ApiResult {
    let ch = parse_channel(&ch)?;
    if eq.bands.len() > 32 {
        return Err((StatusCode::BAD_REQUEST, tr!("máximo 32 bandas", "at most 32 bands").into()));
    }
    sh.update(|s| s.channel_mut(ch).eq = eq);
    Ok(Json(full_state(&sh)))
}

/// POST /api/channels/{game|chat|media|aux|mic}/eq/preset/{nombre}
/// Vale tanto para presets integrados como para los guardados por el usuario.
async fn set_eq_preset(State(sh): State<AppState>, Path((target, name)): Path<(String, String)>) -> ApiResult {
    let t = EqTarget::parse(&target).ok_or((StatusCode::NOT_FOUND, trf!("canal desconocido: {target}", "unknown channel: {target}")))?;
    if !sh.update(|s| s.apply_preset(t, &name)) {
        return Err((StatusCode::NOT_FOUND, trf!("preset desconocido: {name}", "unknown preset: {name}")));
    }
    Ok(Json(full_state(&sh)))
}

#[derive(Deserialize)]
struct MicPatch {
    enabled: Option<bool>,
    muted: Option<bool>,
    volume: Option<f32>,
    gain_db: Option<f32>,
    noise_suppression: Option<bool>,
    vad_gate: Option<bool>,
    vad_threshold: Option<f32>,
    gate_enabled: Option<bool>,
    gate_threshold_db: Option<f32>,
    compressor: Option<bool>,
    monitor: Option<bool>,
    monitor_volume: Option<f32>,
}

/// PATCH /api/mic {"muted":true}
async fn patch_mic(State(sh): State<AppState>, Json(b): Json<MicPatch>) -> ApiResult {
    sh.update(|s| {
        let m = &mut s.mic;
        if let Some(v) = b.enabled { m.enabled = v }
        if let Some(v) = b.muted { m.muted = v }
        if let Some(v) = b.volume { m.volume = v.clamp(0.0, 1.0) }
        if let Some(v) = b.gain_db { m.gain_db = v.clamp(-20.0, 30.0) }
        if let Some(v) = b.noise_suppression { m.noise_suppression = v }
        if let Some(v) = b.vad_gate { m.vad_gate = v }
        if let Some(v) = b.vad_threshold { m.vad_threshold = v.clamp(0.0, 1.0) }
        if let Some(v) = b.gate_enabled { m.gate_enabled = v }
        if let Some(v) = b.gate_threshold_db { m.gate_threshold_db = v.clamp(-90.0, 0.0) }
        if let Some(v) = b.compressor { m.compressor = v }
        if let Some(v) = b.monitor { m.monitor = v }
        if let Some(v) = b.monitor_volume { m.monitor_volume = v.clamp(0.0, 1.0) }
    });
    Ok(Json(full_state(&sh)))
}
