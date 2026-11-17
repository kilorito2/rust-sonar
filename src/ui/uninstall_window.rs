//! Ventana de la desinstalación completa (la copia elevada que lanza «Desinstalar» de Ajustes):
//! lista de pasos con su estado y, al final, la opción de reiniciar.

use super::theme::{self, *};
use super::{icons, widgets};
use crate::uninstall::{self, StepResult};
use eframe::egui::{self, Align, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Pending,
    Running,
    Done,
    /// No había nada que quitar.
    Skipped,
    Warn,
}

struct Step {
    label: String,
    state: State,
    detail: String,
}

struct Progress {
    steps: Vec<Step>,
    finished: bool,
    reboot: bool,
}

struct UninstallApp {
    progress: Arc<Mutex<Progress>>,
}

/// `appdata`: carpeta APPDATA del usuario que pulsó «Desinstalar» (la de su configuración).
pub fn run(appdata: Option<String>, icon: egui::IconData) -> eframe::Result {
    if let Some(dir) = appdata.filter(|a| !a.is_empty()) {
        // Aún no hay más hilos.
        unsafe { std::env::set_var("APPDATA", dir) };
    }
    let settings = crate::config::Settings::load();
    crate::i18n::set(settings.language);
    let port = settings.api_port;

    let mut labels = vec![
        tr!("Cerrar Rust Sonar", "Close Rust Sonar").to_string(),
        tr!("Devolver los dispositivos de Windows a como estaban", "Put Windows' devices back as they were").to_string(),
    ];
    labels.extend(uninstall::DRIVERS.iter().map(|d| trf!("Quitar {} (driver, dispositivos y archivos)", "Remove {} (driver, devices and files)", d.name)));
    labels.push(tr!("Quitar el programa Rust Sonar", "Remove the Rust Sonar program").into());
    labels.push(tr!("Borrar la configuración", "Delete the settings").into());
    let progress = Arc::new(Mutex::new(Progress {
        steps: labels.into_iter().map(|label| Step { label, state: State::Pending, detail: String::new() }).collect(),
        finished: false,
        reboot: false,
    }));

    let title = tr!("Desinstalar Rust Sonar", "Uninstall Rust Sonar");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([640.0, 620.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let p = progress.clone();
            std::thread::spawn(move || work(&p, &ctx, port));
            Ok(Box::new(UninstallApp { progress }))
        }),
    )
}

/// Hace los pasos uno detrás de otro (en otro hilo, para que la ventana no se congele).
fn work(p: &Mutex<Progress>, ctx: &egui::Context, port: u16) {
    let _com = crate::audio::wasapi::ComGuard::init();
    let mut tasks: Vec<Box<dyn FnOnce() -> StepResult>> = vec![Box::new(move || uninstall::close_rust_sonar(port)), Box::new(uninstall::restore_windows)];
    for d in &uninstall::DRIVERS {
        tasks.push(Box::new(move || uninstall::remove_driver(d)));
    }
    tasks.push(Box::new(uninstall::remove_app));
    tasks.push(Box::new(uninstall::remove_settings));

    for (i, task) in tasks.into_iter().enumerate() {
        p.lock().steps[i].state = State::Running;
        ctx.request_repaint();
        // Un fallo inesperado en un paso no debe dejar a medias los demás.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).unwrap_or_else(|_| StepResult {
            detail: tr!("Error inesperado", "Unexpected error").into(),
            warn: true,
            did_something: false,
            reboot: false,
        });
        let mut g = p.lock();
        g.reboot |= r.reboot;
        let step = &mut g.steps[i];
        step.state = match (r.warn, r.did_something) {
            (true, _) => State::Warn,
            (false, true) => State::Done,
            (false, false) => State::Skipped,
        };
        step.detail = r.detail;
        drop(g);
        ctx.request_repaint();
    }
    p.lock().finished = true;
    ctx.request_repaint();
}

impl eframe::App for UninstallApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let finished = self.progress.lock().finished;
        // A medias no se puede cerrar: quedaría un driver medio quitado.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !finished {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().fill(BG).inner_margin(Margin::same(28)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.content(ui);
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        uninstall::schedule_self_delete();
    }
}

impl UninstallApp {
    fn content(&mut self, ui: &mut Ui) {
        let (finished, reboot, any_warn) = {
            let g = self.progress.lock();
            (g.finished, g.reboot, g.steps.iter().any(|s| s.state == State::Warn))
        };
        ui.horizontal(|ui| {
            widgets::logo(ui, 40.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(tr!("Desinstalar Rust Sonar", "Uninstall Rust Sonar")).font(semibold(19.0)));
                ui.label(
                    RichText::new(tr!(
                        "Se quitan el programa y los drivers de VB-Audio que instaló, sin dejar rastro.",
                        "Removing the program and the VB-Audio drivers it installed, leaving no trace."
                    ))
                    .size(12.5)
                    .color(TEXT_3),
                );
            });
        });
        ui.add_space(18.0);

        widgets::card(ui, None, false, |ui| {
            let g = self.progress.lock();
            for (i, step) in g.steps.iter().enumerate() {
                if i > 0 {
                    ui.add_space(10.0);
                }
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(vec2(24.0, 22.0), Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| status_icon(ui, step.state));
                    ui.vertical(|ui| {
                        let color = if step.state == State::Pending { TEXT_3 } else { TEXT };
                        ui.label(RichText::new(&step.label).font(medium(13.5)).color(color));
                        if !step.detail.is_empty() {
                            let color = if step.state == State::Warn { WARN } else { TEXT_3 };
                            ui.add(egui::Label::new(RichText::new(&step.detail).size(12.0).color(color)).wrap());
                        }
                    });
                });
            }
        });

        ui.add_space(18.0);
        if !finished {
            ui.label(RichText::new(tr!("No cierres esta ventana: puede tardar un minuto.", "Don't close this window: it can take a minute.")).size(12.5).color(TEXT_3));
            return;
        }
        let (icon, color, text) = match (any_warn, reboot) {
            (true, _) => (
                icons::WARNING,
                WARN,
                tr!(
                    "Terminado, pero algo no se pudo quitar (mira los avisos). Reinicia el PC y, si sigue ahí, vuelve a desinstalar.",
                    "Finished, but something couldn't be removed (see the warnings). Restart the PC and, if it's still there, uninstall again."
                ),
            ),
            (false, true) => (icons::CHECK, OK, tr!("Listo. Reinicia el PC para que Windows termine de quitar los drivers.", "Done. Restart the PC so Windows finishes removing the drivers.")),
            (false, false) => (icons::CHECK, OK, tr!("Listo. No queda nada de Rust Sonar ni de sus drivers.", "Done. Nothing is left of Rust Sonar or its drivers.")),
        };
        widgets::notice(ui, icon, color, text);
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if reboot || any_warn {
                if widgets::primary_button(ui, icons::RESET, tr!("Reiniciar ahora", "Restart now")).clicked() {
                    uninstall::schedule_self_delete();
                    uninstall::restart_windows();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if widgets::secondary_button(ui, icons::CLOSE, tr!("Más tarde", "Later"), true).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            } else if widgets::primary_button(ui, icons::CHECK, tr!("Cerrar", "Close")).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

fn status_icon(ui: &mut Ui, state: State) {
    match state {
        State::Running => {
            ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
        }
        State::Pending => {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            ui.painter().circle_stroke(rect.center(), 6.0, Stroke::new(1.5, TEXT_3));
        }
        State::Done => {
            ui.label(RichText::new(icons::CHECK).font(theme::icon(17.0)).color(OK));
        }
        State::Skipped => {
            ui.label(RichText::new(icons::CHECK).font(theme::icon(17.0)).color(TEXT_3));
        }
        State::Warn => {
            ui.label(RichText::new(icons::WARNING).font(theme::icon(17.0)).color(WARN));
        }
    }
}
