//! Rust Sonar: mezclador de audio por canales para Windows, inspirado en la idea de SteelSeries Sonar
//! pero escrito desde cero en Rust (WASAPI + egui + axum).

// En release no se abre consola; en debug sí, para ver los logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod api;
mod audio;
mod config;
mod dsp;
mod sonar_import;
mod state;
mod system;
mod tray;
mod ui;
mod virtual_devices;

use state::Shared;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Deja Windows como estaba (dispositivos, nombres, predeterminados) antes de desinstalar.
const UNINSTALL_FLAG: &str = "--uninstall-cleanup";

fn main() -> eframe::Result {
    // Modo auxiliar: la propia app se relanza como administrador solo para renombrar u ocultar
    // dispositivos.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(audio::routing::DEVICE_ADMIN_FLAG) {
        std::process::exit(audio::routing::run_device_admin_helper(&args[2..]));
    }
    // Lo lanza el desinstalador (ya como administrador) antes de borrar los archivos.
    if args.get(1).map(String::as_str) == Some(UNINSTALL_FLAG) {
        std::process::exit(virtual_devices::uninstall_cleanup() as i32);
    }
    if args.get(1).map(String::as_str) == Some(system::UI_FLAG) {
        return run_ui();
    }
    let start_hidden = args.iter().any(|a| a == system::MINIMIZED_FLAG);
    if !system::single_instance() {
        // El motor ya está en marcha: solo hace falta la ventana.
        if !start_hidden {
            system::open_ui();
        }
        return Ok(());
    }
    run_engine(start_hidden);
    Ok(())
}

/// Motor: audio, enrutado, micro, API y bandeja, sin ventana ni gráficos. Es lo que queda en
/// marcha en la bandeja; la ventana es otro proceso (`run_ui`) que solo existe mientras se usa.
fn run_engine(start_hidden: bool) {
    let shared = Arc::new(Shared::new(config::Settings::load()));

    let sessions = audio::engine::spawn(shared.clone());
    audio::mic::spawn(shared.clone());
    api::spawn(shared.clone());
    let icon = app_icon();
    tray::spawn(shared.clone(), icon.rgba.clone(), icon.width);

    // Recién instalado (y reiniciado): si ya hay cables virtuales y nada asignado, se configura solo.
    let first = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let _com = audio::wasapi::ComGuard::init();
        if let Err(e) = virtual_devices::first_run_setup(&first) {
            eprintln!("[configuración inicial] {e:#}");
        }
    });

    if !start_hidden {
        system::open_ui();
    }

    // Guardado de la configuración (como mucho dos escrituras por segundo) hasta que se pida salir.
    while !shared.quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));
        shared.save_if_dirty();
    }
    finish(&shared, Duration::from_secs(6));
    let _ = sessions.join();
}

/// Ventana: proceso aparte que muestra y edita el estado del motor a través de la API local.
fn run_ui() -> eframe::Result {
    if !system::single_ui_instance() {
        system::show_main_window();
        return Ok(());
    }
    if !system::engine_running() {
        // Abierta sin motor (no debería pasar): se arranca en la bandeja y se espera a su API.
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg(system::MINIMIZED_FLAG).spawn();
        }
        let port = config::Settings::load().api_port;
        for _ in 0..30 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let shared = Arc::new(Shared::mirror(config::Settings::load()));
    ui::client::spawn(shared.clone());

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(system::WINDOW_TITLE)
            .with_inner_size([1180.0, 690.0])
            .with_maximized(true)
            .with_min_inner_size([960.0, 560.0])
            // Sin la barra de título de Windows: minimizar, maximizar y cerrar van en la barra de
            // arriba de la propia interfaz (ui/mod.rs).
            .with_decorations(false)
            .with_icon(app_icon()),
        // OpenGL: mucha menos memoria que wgpu (Vulkan/DX12) con el mismo resultado para esta interfaz.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(system::WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(ui::SonarApp::new(cc, shared)))))
}

/// Salida ordenada pedida desde fuera de la ventana (menú de la bandeja, API).
pub fn request_quit(shared: &Shared) {
    shared.quit.store(true, Ordering::Relaxed);
    // Despierta a los hilos que duermen esperando avisos, para que terminen ya.
    shared.wake.wake();
    shared.out_wake.wake();
    shared.mic_wake.wake();
    // La ventana (otro proceso) también se cierra.
    system::close_main_window();
}

/// Espera a que el motor deje Windows como estaba (enrutado y predeterminados), guarda la
/// configuración y quita el icono de la bandeja. Se puede llamar más de una vez.
fn finish(shared: &Shared, timeout: Duration) {
    let start = Instant::now();
    while !shared.cleanup_done.load(Ordering::Acquire) && start.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(20));
    }
    shared.save_if_dirty();
    tray::remove();
}

/// Icono de la ventana dibujado por código (anillo turquesa con tres barras), sin archivos de imagen.
fn app_icon() -> eframe::egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    let teal = [0x2d, 0xd4, 0xbf];
    let bars = [(-11.0f32, 10.0f32), (0.0, 18.0), (11.0, 13.0)]; // (x, media altura)
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            // Cobertura con antialiasing simple (1 px de transición).
            let ring = (1.0 - ((d - 28.5).abs() - 2.5).clamp(0.0, 1.0)).max(0.0);
            let disc = (31.5 - d).clamp(0.0, 1.0) * 0.18;
            let bar = bars
                .iter()
                .map(|&(bx, h)| (1.0 - ((dx - bx).abs() - 2.5).clamp(0.0, 1.0)) * (1.0 - (dy.abs() - h).clamp(0.0, 1.0)))
                .fold(0.0f32, f32::max);
            let a = ring.max(bar).max(disc);
            let i = (y * N + x) * 4;
            rgba[i..i + 3].copy_from_slice(&teal);
            rgba[i + 3] = (a * 255.0) as u8;
        }
    }
    eframe::egui::IconData { rgba, width: N as u32, height: N as u32 }
}
