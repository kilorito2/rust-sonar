//! Interfaz gráfica (egui/eframe).
//!
//! - `theme`: paleta, tipografía (Inter) y estilo de egui.
//! - `icons`: iconos Phosphor.
//! - `widgets`: fader, vúmetros, interruptores, tarjetas… dibujados a mano.
//! - `eq_editor`: ecualizador con gráfica interactiva.
//! - `mixer`, `mic`, `settings`: vistas. El mezclador tiene pestañas arriba (Mezcla, un canal cada
//!   una y Micrófono), como SteelSeries Sonar; Ajustes va aparte en el menú lateral.
//!
//! Patrón de estado: en cada frame se clona la configuración, las vistas editan la copia y, si algo
//! cambió, se publica con `Shared::update` (avisa al motor de audio y marca para guardar).

pub mod client;
mod eq_editor;
pub(crate) mod icons;
mod mic;
mod mixer;
mod settings;
pub(crate) mod theme;
mod widgets;

use crate::config::{Channel, EqTarget};
use crate::dsp::lin_to_db;
use crate::state::Shared;
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, pos2, vec2};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Mixer,
    Settings,
}

/// Pestañas de la parte de arriba del mezclador.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    /// Todas las tiras de canal juntas y el ChatMix.
    Mix,
    /// Configuración completa de un canal: volumen, EQ, audio espacial…
    Channel(Channel),
    Mic,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Mix,
        Tab::Channel(Channel::Game),
        Tab::Channel(Channel::Chat),
        Tab::Channel(Channel::Media),
        Tab::Channel(Channel::Aux),
        Tab::Mic,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Mix => tr!("Mezcla", "Mix"),
            Tab::Channel(ch) => ch.label(),
            Tab::Mic => tr!("Micrófono", "Microphone"),
        }
    }
}

/// Pestaña inicial para desarrollo y capturas: `RUST_SONAR_TAB=juego|chat|media|aux|micro|ajustes`.
fn start_tab() -> Option<Tab> {
    match std::env::var("RUST_SONAR_TAB").ok()?.as_str() {
        "juego" => Some(Tab::Channel(Channel::Game)),
        "chat" => Some(Tab::Channel(Channel::Chat)),
        "media" => Some(Tab::Channel(Channel::Media)),
        "aux" => Some(Tab::Channel(Channel::Aux)),
        "micro" => Some(Tab::Mic),
        _ => None,
    }
}

const SIDEBAR_W: f32 = 210.0;
const TOPBAR_H: f32 = 64.0;
/// Margen de arriba y abajo de la zona de contenido.
const CONTENT_PAD_Y: f32 = 22.0;
const SIDEBAR_COLLAPSED_W: f32 = 68.0;
/// Margen lateral del menú.
const SIDE_PAD_X: f32 = 14.0;

/// Lo que se arrastra al mover una app de un canal a otro.
pub(crate) struct AppDrag {
    pub name: String,
}

pub struct SonarApp {
    shared: Arc<Shared>,
    view: View,
    tab: Tab,
    /// Alto visible de la zona de contenido (sin la cabecera): las tiras ajustan sus faders a él
    /// para que todo quepa sin desplazarse, también en pantallas de 768 px.
    content_h: f32,
    meters: HashMap<&'static str, f32>,
    eq_state: eq_editor::EqUiState,
    /// Resultado de la última acción lanzada en segundo plano (ok, mensaje).
    action_msg: Arc<Mutex<Option<(bool, String)>>>,
    /// Inicio automático con Windows (se lee del registro al abrir).
    autostart: bool,
    /// En Ajustes, ofrecer todos los dispositivos y no solo los virtuales.
    show_all_devices: bool,
    /// Acción cuyo atajo se está grabando (esperando a que se pulse la combinación).
    capturing: Option<crate::config::MuteTarget>,
    /// Aviso mientras se graba (p. ej. «hace falta Ctrl o Alt»).
    capture_hint: Option<&'static str>,
}

impl SonarApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            shared,
            view: if start_tab().is_none() && std::env::var("RUST_SONAR_TAB").as_deref() == Ok("ajustes") { View::Settings } else { View::Mixer },
            tab: start_tab().unwrap_or(Tab::Mix),
            content_h: 600.0,
            meters: HashMap::new(),
            eq_state: Default::default(),
            action_msg: Arc::new(Mutex::new(None)),
            autostart: crate::system::autostart_enabled(),
            show_all_devices: false,
            capturing: None,
            capture_hint: None,
        }
    }

    /// Configuración en un clic: reparte los dispositivos virtuales y elige auriculares y micro.
    fn run_auto_setup(&self) {
        let shared = self.shared.clone();
        self.run_action(move || {
            use crate::audio::wasapi;
            use windows::Win32::Media::Audio::{eCapture, eConsole, eRender};
            let outputs = wasapi::list_devices(eRender)?;
            let inputs = wasapi::list_devices(eCapture)?;
            let default_out = wasapi::default_device_id(eRender, eConsole);
            let default_in = wasapi::default_device_id(eCapture, eConsole);
            shared
                .update(|s| crate::virtual_devices::auto_setup(s, &outputs, &inputs, default_out.as_deref(), default_in.as_deref()))
                .map(|summary| trf!("Listo: {summary}", "Done: {summary}"))
                .map_err(anyhow::Error::msg)
        });
    }

    /// Importa una configuración compartida de SteelSeries Sonar al canal `t`.
    fn import_sonar(&self, link: String, t: EqTarget) {
        let shared = self.shared.clone();
        self.run_action(move || crate::sonar_import::import_link(&shared, &link, Some(t)));
    }

    /// Avisos que se ven en todas las vistas: conflicto con otro mezclador y primera configuración.
    fn banners(&mut self, ui: &mut Ui, s: &crate::config::Settings) {
        if let Some(conflict) = self.shared.status.read().conflict.clone() {
            egui::Frame::new()
                .fill(DANGER.gamma_multiply(0.10))
                .stroke(Stroke::new(1.0, DANGER.gamma_multiply(0.35)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::WARNING).font(theme::icon(16.0)).color(DANGER));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::secondary_button(ui, icons::CLOSE, tr!("Cerrar SteelSeries GG", "Close SteelSeries GG"), true).clicked() {
                                self.run_action(crate::system::close_steelseries);
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                                ui.label(RichText::new(conflict).size(12.5).color(TEXT));
                            });
                        });
                    });
                });
            ui.add_space(8.0);
        }

        // Resultado de la última acción (configurar, importar, renombrar…). Clic para quitarlo.
        let last = self.action_msg.lock().clone();
        if let Some((ok, msg)) = last {
            let (icon, color) = if ok { (icons::CHECK, OK) } else { (icons::WARNING, DANGER) };
            let before = ui.cursor().top();
            widgets::notice(ui, icon, color, &msg);
            let rect = egui::Rect::from_min_max(pos2(ui.max_rect().left(), before), pos2(ui.max_rect().right(), ui.cursor().top()));
            if ui.interact(rect, ui.scope_id().with("dismiss_msg"), Sense::click()).on_hover_text(tr!("Clic para ocultar", "Click to dismiss")).clicked() {
                *self.action_msg.lock() = None;
            }
            ui.add_space(8.0);
        }

        let configured = !s.virtual_device_ids().is_empty();
        let has_virtual = self.shared.devices.read().outputs.iter().any(|d| crate::virtual_devices::is_virtual_cable(&d.name));
        if !configured && has_virtual && self.view != View::Settings {
            egui::Frame::new()
                .fill(ACCENT.gamma_multiply(0.08))
                .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.35)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::MAGIC).font(theme::icon(16.0)).color(ACCENT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::primary_button(ui, icons::MAGIC, tr!("Configurar automáticamente", "Set up automatically")).clicked() {
                                self.run_auto_setup();
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                                ui.label(
                                    RichText::new(
                                        "He encontrado dispositivos de audio virtuales. Con un clic cada canal tendrá su salida en Windows \
                                         y las apps se enrutarán solas.",
                                    )
                                    .size(12.5)
                                    .color(TEXT),
                                );
                            });
                        });
                    });
                });
            ui.add_space(8.0);
        }
    }

    /// Vúmetro con caída suave: sube al instante y baja poco a poco. Escala de -60 a 0 dBFS.
    fn meter_level(&mut self, key: &'static str, peak: f32) -> f32 {
        let level = ((lin_to_db(peak) + 60.0) / 60.0).clamp(0.0, 1.0);
        let shown = self.meters.entry(key).or_insert(0.0);
        *shown = level.max(*shown - 0.025);
        *shown
    }

    /// Ejecuta una acción del sistema (COM) en otro hilo para no congelar la ventana.
    fn run_action(&self, f: impl FnOnce() -> anyhow::Result<String> + Send + 'static) {
        let msg = self.action_msg.clone();
        *msg.lock() = Some((true, tr!("Trabajando…", "Working…").into()));
        std::thread::spawn(move || {
            let _com = crate::audio::wasapi::ComGuard::init();
            *msg.lock() = Some(match f() {
                Ok(m) => (true, m),
                Err(e) => (false, format!("{e:#}")),
            });
        });
    }
}

impl eframe::App for SonarApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // Solo se redibuja cuando hace falta: 30 veces por segundo si algo se mueve (vúmetros), 4 si
        // todo está quieto, y nada si la ventana está minimizada.
        let minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
        self.shared.ui_visible.store(!minimized, std::sync::atomic::Ordering::Relaxed);
        let mic_tab = self.view == View::Mixer && self.tab == Tab::Mic;
        self.shared.ui_wants_mic.store(mic_tab && !minimized, std::sync::atomic::Ordering::Relaxed);
        if !minimized {
            let m = self.shared.meters.snapshot();
            let moving = [m.game, m.chat, m.media, m.aux, m.master, m.mic_in, m.mic_out].iter().any(|v| *v > 0.0005)
                || self.meters.values().any(|v| *v > 0.0);
            ui.ctx().request_repaint_after(Duration::from_millis(if moving { 33 } else { 250 }));
        }

        // La ventana es un proceso aparte: cerrarla no para el audio, que sigue en la bandeja. Si
        // «cerrar a la bandeja» está desactivado, se cierra también el motor.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.shared.settings.read().close_to_tray {
            client::quit_engine(self.shared.settings.read().api_port);
        }

        let mut s = self.shared.snapshot();
        let before = s.clone();

        let collapsed = s.sidebar_collapsed;
        let side_w = if collapsed { SIDEBAR_COLLAPSED_W } else { SIDEBAR_W };
        let side_margin = Margin { left: SIDE_PAD_X as i8, right: SIDE_PAD_X as i8, top: 0, bottom: 18 };
        egui::Panel::left("sidebar")
            .exact_size(side_w)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(side_margin).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| self.sidebar(ui, &mut s));

        // Márgenes algo más estrechos si la ventana es pequeña (portátiles de 1366×768).
        let narrow = ui.available_width() < 1160.0;
        let pad: i8 = if narrow { 22 } else { 32 };
        egui::Panel::top("topbar")
            .exact_size(TOPBAR_H)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin { left: pad, right: 0, top: 0, bottom: 0 }))
            .show(ui, |ui| self.top_bar(ui, &mut s));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.content_h = ui.available_height() - 2.0 * CONTENT_PAD_Y;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: pad, right: pad, top: CONTENT_PAD_Y as i8, bottom: CONTENT_PAD_Y as i8 }).show(ui, |ui| {
                    self.banners(ui, &s);
                    match self.view {
                        View::Mixer => self.mixer_view(ui, &mut s),
                        View::Settings => self.settings_view(ui, &mut s),
                    }
                });
            });
        });

        window_edges(ui.ctx());

        if s != before {
            // Si otro hilo cambió la configuración durante este frame (teclas de volumen, API,
            // configuración automática), gana ese cambio: pisarlo con nuestra copia lo desharía.
            self.shared.update(|cfg| {
                if *cfg == before {
                    *cfg = s;
                }
            });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shared.save_if_dirty();
    }
}

impl SonarApp {
    fn sidebar(&mut self, ui: &mut Ui, s: &mut crate::config::Settings) {
        let collapsed = s.sidebar_collapsed;
        // Cabecera del menú con el mismo alto que la barra de arriba: sus líneas inferiores coinciden.
        let w = ui.available_width();
        let panel_top = ui.max_rect().top();
        // La cabecera del menú también sirve para mover la ventana (los botones de encima tienen prioridad).
        let header = egui::Rect::from_min_size(pos2(ui.max_rect().left() - SIDE_PAD_X, panel_top), vec2(w + 2.0 * SIDE_PAD_X, TOPBAR_H));
        drag_area(ui, header, "sidebar_drag");
        ui.allocate_ui_with_layout(vec2(w, TOPBAR_H), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(vec2(w, TOPBAR_H));
            if collapsed {
                ui.add_space(((w - 32.0) / 2.0).max(0.0));
                let r = ui.allocate_ui(vec2(32.0, 32.0), |ui| widgets::logo(ui, 32.0)).response.interact(Sense::click());
                if r.on_hover_text(tr!("Desplegar el menú", "Expand the menu")).clicked() {
                    s.sidebar_collapsed = false;
                }
            } else {
                widgets::logo(ui, 32.0);
                // Bloque de alto fijo para que quede centrado con el logo.
                ui.allocate_ui_with_layout(vec2(120.0, 34.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new("Rust Sonar").font(semibold(15.5)).color(TEXT));
                    ui.label(RichText::new(tr!("Mezclador de audio", "Audio mixer")).size(11.0).color(TEXT_3));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, icons::CARET_DOUBLE_LEFT, tr!("Plegar el menú", "Collapse the menu"), false, ACCENT).clicked() {
                        s.sidebar_collapsed = true;
                    }
                });
            }
        });
        let full = ui.max_rect().expand2(vec2(SIDE_PAD_X, 0.0));
        ui.painter().hline(full.x_range(), panel_top + TOPBAR_H - 0.5, Stroke::new(1.0, BORDER));
        ui.add_space(20.0);
        if collapsed {
            if widgets::icon_button(ui, icons::CARET_DOUBLE_RIGHT, tr!("Desplegar el menú", "Expand the menu"), false, ACCENT).clicked() {
                s.sidebar_collapsed = false;
            }
            ui.add_space(8.0);
        } else {
            widgets::caption(ui, tr!("Menú", "Menu"));
            ui.add_space(4.0);
        }
        ui.spacing_mut().item_spacing.y = 6.0;

        for (view, icon, label) in [(View::Mixer, icons::MIXER, tr!("Mezclador", "Mixer")), (View::Settings, icons::SETTINGS, tr!("Ajustes", "Settings"))] {
            let r = nav_item(ui, icon, label, self.view == view, collapsed);
            let r = if collapsed { r.on_hover_text(label) } else { r };
            if r.clicked() {
                self.view = view;
            }
        }

        {
            let status = self.shared.status.read().clone();
            let paused = self.shared.paused.load(std::sync::atomic::Ordering::Relaxed);
            let ok = (status.output.starts_with("Sonando") || status.output.starts_with("Playing")) && !paused;
            let idle = self.shared.idle.load(std::sync::atomic::Ordering::Relaxed);
            let color = if ok { OK } else if paused { WARN } else { DANGER };
            let label = if ok && idle { tr!("En reposo", "Idle") } else if ok { tr!("Motor activo", "Engine running") } else if paused { tr!("En pausa", "Paused") } else { tr!("Sin salida", "No output") };
            // La caja de estado va abajo del todo: se rellena el hueco que sobra.
            ui.add_space((ui.available_height() - if collapsed { 34.0 } else { 78.0 }).max(0.0));
            if collapsed {
                // Plegado: solo el punto de estado, con el detalle al pasar el ratón.
                ui.vertical_centered(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                    resp.on_hover_text(format!("{label}\n{}", status.output));
                });
                return;
            }
            let port = self.shared.settings.read().api_port;
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::activity_dot(ui, ok, color);
                            ui.label(RichText::new(label).font(medium(12.5)).color(TEXT));
                        });
                        let detail = status.output.trim_start_matches("Sonando en: ").trim_start_matches("Playing on: ").to_string();
                        widgets::truncated(ui, RichText::new(&detail).size(11.0).color(TEXT_3)).on_hover_text(&status.output);
                        ui.label(RichText::new(trf!("{}  API en el puerto {port}", "{}  API on port {port}", icons::BROADCAST)).size(11.0).color(TEXT_3));
                    });
                });
        }
    }

    /// Barra de arriba, de lado a lado: pestañas (mezclador) o título (ajustes) a la izquierda;
    /// volumen general y salida a la derecha, visibles desde cualquier pestaña.
    fn top_bar(&mut self, ui: &mut Ui, s: &mut crate::config::Settings) {
        let rect = ui.max_rect();
        // Línea inferior de la barra (el subrayado de la pestaña elegida se pinta encima).
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, BORDER));
        // Toda la barra mueve la ventana (doble clic = maximizar); pestañas y botones van encima.
        drag_area(ui, rect, "topbar_drag");

        let outputs = self.shared.devices.read().outputs.clone();
        let left_w = if self.view == View::Mixer { tabs_width(ui) } else { 320.0 };
        let free = ui.available_width() - left_w - WINDOW_BUTTONS_W;
        let combo_w = if free >= 470.0 { 230.0 } else { (free - 30.0).clamp(150.0, 230.0) };
        let show_master = free >= combo_w + 330.0;

        ui.horizontal(|ui| {
            ui.set_height(TOPBAR_H);
            match self.view {
                View::Mixer => {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for tab in Tab::ALL {
                        if tab_button(ui, tab, self.tab == tab).clicked() {
                            self.tab = tab;
                        }
                        if tab == Tab::Mix {
                            // Separador entre «Mezcla» y los canales, como en Sonar.
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
                            ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, BORDER_STRONG));
                        }
                    }
                }
                View::Settings => {
                    ui.label(RichText::new(icons::SETTINGS).font(theme::icon(20.0)).color(ACCENT));
                    ui.add_space(2.0);
                    ui.label(RichText::new(tr!("Ajustes", "Settings")).font(semibold(19.0)).color(TEXT));
                    ui.add_space(10.0);
                    ui.label(RichText::new(tr!("Dispositivos virtuales, aplicaciones y API", "Virtual devices, apps and API")).size(12.5).color(TEXT_3));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                window_buttons(ui);
                ui.add_space(14.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                widgets::device_combo(ui, "out_dev", icons::HEADPHONES, &mut s.output_device, &outputs, tr!("Predeterminado de Windows", "Windows default"), combo_w);
                if show_master {
                    ui.add_space(6.0);
                    master_mini(ui, s);
                }
            });
        });
    }
}

/// Volumen general en la barra de arriba, en su propia cápsula: silencio, deslizador y porcentaje.
fn master_mini(ui: &mut Ui, s: &mut crate::config::Settings) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin { left: 12, right: 4, top: 1, bottom: 1 })
        .show(ui, |ui| {
            // La barra coloca de derecha a izquierda (y la fila hereda el sentido): se añade al revés
            // para que se vea «altavoz · deslizador · %».
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let txt = if s.master_muted { tr!("Mudo", "Muted").to_string() } else { format!("{:.0}%", s.master_volume * 100.0) };
                ui.add_sized([38.0, 20.0], egui::Label::new(RichText::new(txt).font(semibold(13.0)).color(if s.master_muted { DANGER } else { TEXT })));
                ui.spacing_mut().slider_width = 100.0;
                ui.add(egui::Slider::new(&mut s.master_volume, 0.0..=1.0).show_value(false)).on_hover_text(tr!("Volumen general (Master)", "Overall volume (Master)"));
                let icon = if s.master_muted { icons::SPEAKER_OFF } else { icons::SPEAKER };
                if widgets::icon_button(ui, icon, if s.master_muted { tr!("Activar sonido", "Unmute") } else { tr!("Silenciar todo", "Mute all") }, s.master_muted, DANGER).clicked() {
                    s.master_muted = !s.master_muted;
                }
            });
        });
}

// ---------------------------------------------------------------------------------------------
// Ventana sin marco de Windows: botones, arrastre y bordes para redimensionar
// ---------------------------------------------------------------------------------------------

const WINDOW_BUTTON_W: f32 = 46.0;
const WINDOW_BUTTONS_W: f32 = 3.0 * WINDOW_BUTTON_W + 14.0;
/// Grosor de la franja del borde que redimensiona la ventana.
const RESIZE_EDGE: f32 = 5.0;

fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Zona que mueve la ventana al arrastrarla y la maximiza o restaura con doble clic.
fn drag_area(ui: &mut Ui, rect: egui::Rect, id: &str) {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click_and_drag());
    if resp.double_clicked() {
        let max = is_maximized(ui.ctx());
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// Minimizar, maximizar/restaurar y cerrar, de todo el alto de la barra y pegados al borde derecho.
/// Se colocan de derecha a izquierda. Cerrar respeta «cerrar a la bandeja».
fn window_buttons(ui: &mut Ui) {
    let max = is_maximized(ui.ctx());
    if window_button(ui, icons::CLOSE, tr!("Cerrar", "Close"), true).clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
    let (icon, tip) = if max { (icons::WIN_RESTORE, tr!("Restaurar", "Restore")) } else { (icons::WIN_MAXIMIZE, tr!("Maximizar", "Maximize")) };
    if window_button(ui, icon, tip, false).clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
    if window_button(ui, icons::WIN_MINIMIZE, tr!("Minimizar", "Minimize"), false).clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

fn window_button(ui: &mut Ui, icon: &str, tooltip: &str, close: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(WINDOW_BUTTON_W, TOPBAR_H - 1.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    if hovered {
        let fill = if close { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { SURFACE_3 };
        p.rect_filled(rect, CornerRadius::ZERO, fill);
    }
    let color = if hovered && close { Color32::WHITE } else if hovered { TEXT } else { TEXT_2 };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon, theme::icon(15.0), color);
    resp.on_hover_text(tooltip)
}

/// Sin marco, Windows no ofrece bordes para redimensionar: se hace a mano con una franja fina en
/// cada borde y esquina. También dibuja un borde de 1 px para separar la ventana del fondo.
fn window_edges(ctx: &egui::Context) {
    if is_maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window_border")))
        .rect_stroke(screen, CornerRadius::ZERO, Stroke::new(1.0, BORDER_STRONG), egui::StrokeKind::Inside);

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    use egui::ResizeDirection as D;
    let (left, right) = (pos.x - screen.left() < RESIZE_EDGE, screen.right() - pos.x < RESIZE_EDGE);
    let (top, bottom) = (pos.y - screen.top() < RESIZE_EDGE, screen.bottom() - pos.y < RESIZE_EDGE);
    let dir = match (left, right, top, bottom) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (_, _, _, true) => D::South,
        _ => return,
    };
    let cursor = match dir {
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        D::West | D::East => egui::CursorIcon::ResizeHorizontal,
        D::North | D::South => egui::CursorIcon::ResizeVertical,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

fn tab_color(tab: Tab) -> Color32 {
    match tab {
        Tab::Mix => ACCENT,
        Tab::Channel(ch) => channel_color(ch),
        Tab::Mic => MIC,
    }
}

fn tab_icon(tab: Tab) -> &'static str {
    match tab {
        Tab::Mix => icons::MIXER,
        Tab::Channel(ch) => theme::channel_icon(ch),
        Tab::Mic => icons::MICROPHONE,
    }
}

const TAB_PAD: f32 = 14.0;

fn tab_size(ui: &Ui, tab: Tab) -> egui::Vec2 {
    vec2(widgets::text_width(ui, tab.label(), semibold(14.0)) + 2.0 * TAB_PAD + 26.0, TOPBAR_H)
}

fn tabs_width(ui: &Ui) -> f32 {
    Tab::ALL.iter().map(|t| tab_size(ui, *t).x + 2.0).sum::<f32>() + 20.0
}

/// Pestaña de la barra de arriba: ocupa todo el alto; la elegida lleva debajo una línea del color
/// del canal, como las pestañas de Sonar.
fn tab_button(ui: &mut Ui, tab: Tab, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(tab_size(ui, tab), Sense::click());
    let color = tab_color(tab);
    let hovered = resp.hovered();
    let p = ui.painter();
    let pill = rect.shrink2(vec2(2.0, 14.0));
    if selected {
        p.rect_filled(pill, CornerRadius::same(8), color.gamma_multiply(0.12));
        let line = egui::Rect::from_min_max(pos2(rect.left() + 10.0, rect.bottom() - 3.0), pos2(rect.right() - 10.0, rect.bottom()));
        p.rect_filled(line, CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, color);
    } else if hovered {
        p.rect_filled(pill, CornerRadius::same(8), SURFACE_2);
    }
    let icon_color = if selected || hovered { color } else { color.gamma_multiply(0.75) };
    p.text(pos2(rect.left() + TAB_PAD + 9.0, rect.center().y), egui::Align2::CENTER_CENTER, tab_icon(tab), theme::icon(17.0), icon_color);
    let text_color = if selected { TEXT } else if hovered { TEXT } else { TEXT_2 };
    let font = if selected { semibold(14.0) } else { medium(14.0) };
    p.text(pos2(rect.left() + TAB_PAD + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, tab.label(), font, text_color);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Elemento del menú lateral con indicador de selección. Plegado, solo el icono centrado.
fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, collapsed: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    if selected {
        p.rect_filled(rect, CornerRadius::same(9), SURFACE_2);
        p.rect_filled(
            egui::Rect::from_min_size(pos2(rect.left(), rect.top() + 10.0), vec2(3.0, rect.height() - 20.0)),
            CornerRadius::same(2),
            ACCENT,
        );
    } else if hovered {
        p.rect_filled(rect, CornerRadius::same(9), SURFACE);
    }
    let color: Color32 = if selected { TEXT } else if hovered { TEXT } else { TEXT_2 };
    let icon_color = if selected { ACCENT } else { color };
    if collapsed {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, icon, theme::icon(19.0), icon_color);
    } else {
        p.text(pos2(rect.left() + 22.0, rect.center().y), egui::Align2::CENTER_CENTER, icon, theme::icon(18.0), icon_color);
        p.text(pos2(rect.left() + 42.0, rect.center().y), egui::Align2::LEFT_CENTER, label, medium(14.0), color);
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}
