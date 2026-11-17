//! Proceso de la ventana: mantiene su copia del estado (`Shared::mirror`) sincronizada con el motor,
//! que va en otro proceso, a través de la API local.
//!
//! - Vúmetros: 30 veces por segundo, solo mientras la ventana se ve.
//! - Resto del estado (apps, dispositivos, estado, configuración): 4 veces por segundo.
//! - Cambios hechos en la ventana: se mandan enteros al motor en cuanto ocurren.
//!
//! Si la configuración cambia a la vez en los dos lados (raro), gana la ventana: es lo que el usuario
//! está tocando en ese momento.

use crate::config::Settings;
use crate::state::{AppInfo, Devices, MetersSnapshot, Shared, Status};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Error, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Cliente HTTP/1.1 mínimo con conexión persistente (solo para 127.0.0.1).
pub struct Http {
    addr: SocketAddr,
    conn: Option<BufReader<TcpStream>>,
}

impl Http {
    pub fn new(port: u16) -> Self {
        Self { addr: SocketAddr::from(([127, 0, 0, 1], port)), conn: None }
    }

    pub fn request(&mut self, method: &str, path: &str, body: Option<&[u8]>) -> std::io::Result<Vec<u8>> {
        // Si la conexión se cerró por inactividad, el primer intento falla: se reintenta con una nueva.
        match self.try_request(method, path, body) {
            Ok(b) => Ok(b),
            Err(_) => {
                self.conn = None;
                let r = self.try_request(method, path, body);
                if r.is_err() {
                    self.conn = None;
                }
                r
            }
        }
    }

    fn try_request(&mut self, method: &str, path: &str, body: Option<&[u8]>) -> std::io::Result<Vec<u8>> {
        if self.conn.is_none() {
            let s = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500))?;
            s.set_nodelay(true)?;
            s.set_read_timeout(Some(Duration::from_secs(3)))?;
            self.conn = Some(BufReader::new(s));
        }
        let conn = self.conn.as_mut().unwrap();
        let body = body.unwrap_or(&[]);
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        conn.get_mut().write_all(head.as_bytes())?;
        conn.get_mut().write_all(body)?;

        let mut line = String::new();
        if conn.read_line(&mut line)? == 0 {
            return Err(Error::from(ErrorKind::UnexpectedEof));
        }
        let code: u16 = line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| Error::from(ErrorKind::InvalidData))?;
        let (mut len, mut close) = (0usize, false);
        loop {
            line.clear();
            if conn.read_line(&mut line)? == 0 {
                return Err(Error::from(ErrorKind::UnexpectedEof));
            }
            if line == "\r\n" {
                break;
            }
            let l = line.to_ascii_lowercase();
            if let Some(v) = l.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            } else if l.starts_with("connection:") && l.contains("close") {
                close = true;
            }
        }
        let mut buf = vec![0u8; len];
        conn.read_exact(&mut buf)?;
        if close {
            self.conn = None;
        }
        if !(200..300).contains(&code) {
            return Err(Error::other(format!("HTTP {code}")));
        }
        Ok(buf)
    }
}

#[derive(Deserialize)]
struct UiState {
    settings: Settings,
    apps: Vec<AppInfo>,
    devices: Devices,
    status: Status,
    paused: bool,
    idle: bool,
    active_output: Option<String>,
}

/// Avisa al motor de que la ventana empieza (true) o termina (false) de grabar un atajo. En otro
/// hilo para no frenar la ventana.
pub fn pause_hotkeys(port: u16, paused: bool) {
    let body = format!(r#"{{"paused":{paused}}}"#);
    std::thread::spawn(move || {
        let _ = Http::new(port).request("POST", "/api/ui/hotkeys/pause", Some(body.as_bytes()));
    });
}

/// Pide al motor que enseñe el aviso en pantalla, para verlo.
pub fn preview_overlay(port: u16) {
    std::thread::spawn(move || {
        let _ = Http::new(port).request("POST", "/api/ui/overlay/preview", None);
    });
}

/// Pide al motor que se cierre (cuando «cerrar a la bandeja» está desactivado).
pub fn quit_engine(port: u16) {
    let _ = Http::new(port).request("POST", "/api/quit", None);
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new().name("sync".into()).spawn(move || run(shared)).unwrap();
}

fn run(shared: Arc<Shared>) {
    let port = shared.settings.read().api_port;
    let mut http = Http::new(port);
    // Última configuración en la que ventana y motor estaban de acuerdo.
    let mut base: Option<Settings> = None;
    let mut last_state: Option<Instant> = None;
    let mut failing_since: Option<Instant> = None;

    loop {
        let visible = shared.ui_visible.load(Ordering::Relaxed);
        let mut ok = true;

        // 1) Cambios hechos en la ventana → motor.
        if let Some(b) = &base {
            let local = shared.snapshot();
            if *b != local {
                match serde_json::to_vec(&local).map_err(Error::other).and_then(|body| http.request("PUT", "/api/ui/settings", Some(&body))) {
                    Ok(_) => base = Some(local),
                    Err(_) => ok = false,
                }
            }
        }

        // 2) Vúmetros, solo con la ventana a la vista. El micro solo se pide activo en su pestaña.
        let mut moving = false;
        if visible {
            let path = if shared.ui_wants_mic.load(Ordering::Relaxed) { "/api/ui/meters?mic=1" } else { "/api/ui/meters" };
            match http.request("GET", path, None) {
                Ok(body) => {
                    if let Ok(m) = serde_json::from_slice::<MetersSnapshot>(&body) {
                        moving = [m.game, m.chat, m.media, m.aux, m.master, m.mic_in, m.mic_out].iter().any(|v| *v > 0.0005);
                        shared.meters.set(&m);
                    }
                }
                Err(_) => ok = false,
            }
        }

        // 3) Resto del estado.
        if last_state.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
            last_state = Some(Instant::now());
            match http.request("GET", "/api/ui/state", None).ok().and_then(|b| serde_json::from_slice::<UiState>(&b).ok()) {
                Some(st) => {
                    *shared.apps.write() = st.apps;
                    *shared.devices.write() = st.devices;
                    *shared.status.write() = st.status;
                    *shared.active_output.write() = st.active_output;
                    shared.paused.store(st.paused, Ordering::Relaxed);
                    shared.idle.store(st.idle, Ordering::Relaxed);
                    // La configuración del motor se adopta si la ventana no tiene cambios sin mandar
                    // (p. ej. teclas de volumen, API, configuración automática del primer arranque).
                    let local = shared.snapshot();
                    let adopt = match &base {
                        None => true,
                        Some(b) => *b == local && st.settings != *b,
                    };
                    if adopt {
                        *shared.settings.write() = st.settings.clone();
                        crate::i18n::set(st.settings.language);
                        base = Some(st.settings);
                    }
                }
                None => ok = false,
            }
        }

        // Sin motor durante unos segundos (se cerró desde la bandeja): la ventana también se cierra.
        if ok {
            failing_since = None;
        } else if failing_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(4) {
            crate::system::close_main_window();
            failing_since = None;
        }

        // 30 veces por segundo solo si algo se mueve; si no, 4 (o 2 con la ventana minimizada).
        std::thread::sleep(Duration::from_millis(if !visible { 500 } else if moving { 33 } else { 250 }));
    }
}
