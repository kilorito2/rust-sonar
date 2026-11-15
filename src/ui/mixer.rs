//! Vista Mezclador, con pestañas arriba como SteelSeries Sonar:
//!
//! - **Mezcla**: una tira por canal (Master, Juego, Chat, Media, Aux, Micro) con preset, origen,
//!   fader y apps, y el ChatMix debajo. Las tiras se reparten el ancho y el fader se ajusta al alto
//!   de la ventana para que todo quepa sin desplazarse.
//! - **Un canal**: su configuración completa (volumen, ecualizador, audio espacial, volumen
//!   inteligente y ganancia). La rueda de cada tira lleva aquí.
//! - **Micrófono**: la vista del micro (`mic.rs`).

use super::eq_editor::{eq_editor, preset_combo};
use super::icons;
use super::theme::{self, *};
use super::widgets::{self, card, card_with_margin, fader, hmeter, icon_button, toggle, vmeter};
use super::{AppDrag, SonarApp, Tab};
use crate::config::{Channel, EqTarget, Settings};
use crate::state::AppInfo;
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};

const STRIP_MIN: f32 = 90.0;
const STRIP_MAX: f32 = 220.0;
const STRIP_PAD: f32 = 14.0;
/// Separación entre tarjetas.
const GAP: f32 = 16.0;
const FADER_MIN: f32 = 120.0;
const FADER_MAX: f32 = 280.0;
/// Alto de una tira sin contar el fader (cabecera, preset, origen, valor, apps) más el ChatMix.
const STRIP_FIXED_H: f32 = 478.0;

impl SonarApp {
    pub(super) fn mixer_view(&mut self, ui: &mut Ui, s: &mut Settings) {
        for problem in s.virtual_device_problems() {
            widgets::notice(ui, icons::WARNING, DANGER, &problem);
            ui.add_space(6.0);
        }
        match self.tab {
            Tab::Mix => self.mix_tab(ui, s),
            Tab::Channel(ch) => self.channel_tab(ui, s, ch),
            Tab::Mic => self.mic_view(ui, s),
        }
    }

    fn mix_tab(&mut self, ui: &mut Ui, s: &mut Settings) {
        let meters = self.shared.meters.snapshot();
        let apps = self.shared.apps.read().clone();
        let outputs = self.shared.devices.read().outputs.clone();
        let peaks = [meters.game, meters.chat, meters.media, meters.aux];
        let keys = ["game", "chat", "media", "aux"];
        let device_name = |id: &Option<String>| -> Option<String> {
            // Un dispositivo configurado que ya no existe no se muestra: el canal captura por app.
            id.as_ref().and_then(|id| outputs.iter().find(|d| &d.id == id).map(|d| d.name.clone()))
        };

        // Cada tarjeta suma su margen y 1 px de borde por lado.
        let strip_w = ((ui.available_width() - 5.0 * GAP - 2.0) / 6.0 - 2.0 * (STRIP_PAD + 1.0)).clamp(STRIP_MIN, STRIP_MAX);
        let fader_h = (self.content_h - STRIP_FIXED_H).clamp(FADER_MIN, FADER_MAX);
        let mut moved: Option<(String, Channel)> = None;
        let mut open: Option<Tab> = None;
        egui::ScrollArea::horizontal().id_salt("strips").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let level = self.meter_level("master", meters.master);
                let out_name = device_name(&s.output_device).unwrap_or_else(|| tr!("Predeterminado", "Default").into());
                master_strip(ui, strip_w, fader_h, level, s, &out_name);

                for ch in Channel::ALL {
                    let level = self.meter_level(keys[ch.index()], peaks[ch.index()]);
                    let mine: Vec<&AppInfo> = apps.iter().filter(|a| a.channel == ch).collect();
                    let t = EqTarget::Channel(ch);
                    let source = device_name(&s.channel(ch).device);
                    let r = strip(ui, strip_w, fader_h, t, level, s, Some(&mine), source.as_deref());
                    if r.open {
                        open = Some(Tab::Channel(ch));
                    }
                    if let Some(name) = r.dropped {
                        moved = Some((name, ch));
                    }
                }

                let level = self.meter_level("mic", meters.mic_out);
                let source = device_name(&s.mic.output_device);
                if strip(ui, strip_w, fader_h, EqTarget::Mic, level, s, None, source.as_deref()).open {
                    open = Some(Tab::Mic);
                }
            });
        });
        if let Some((name, ch)) = moved {
            s.app_routes.insert(name.to_ascii_lowercase(), ch);
        }
        if let Some(tab) = open {
            self.tab = tab;
        }
        ui.add_space(GAP);
        chatmix_card(ui, s);
    }

    fn channel_tab(&mut self, ui: &mut Ui, s: &mut Settings, ch: Channel) {
        let meters = self.shared.meters.snapshot();
        let apps = self.shared.apps.read().clone();
        let outputs = self.shared.devices.read().outputs.clone();
        let peak = [meters.game, meters.chat, meters.media, meters.aux][ch.index()];
        let level = self.meter_level(["game", "chat", "media", "aux"][ch.index()], peak);
        let source = s.channel(ch).device.as_ref().and_then(|id| outputs.iter().find(|d| &d.id == id)).map(|d| d.name.clone());
        let mine: Vec<&AppInfo> = apps.iter().filter(|a| a.channel == ch).collect();
        let t = EqTarget::Channel(ch);

        let moved = channel_header(ui, s, ch, level, source.as_deref(), &mine);
        if let Some(name) = moved {
            s.app_routes.insert(name.to_ascii_lowercase(), ch);
        }
        ui.add_space(GAP);
        if let Some(link) = eq_editor(ui, &mut self.eq_state, s, t) {
            self.import_sonar(link, t);
        }
        ui.add_space(GAP);
        // Audio espacial y dinámica, en dos columnas si caben.
        let w = ui.available_width();
        if w >= 820.0 {
            let col = (w - GAP) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                ui.vertical(|ui| {
                    ui.set_width(col);
                    spatial_card(ui, s, ch);
                });
                ui.vertical(|ui| {
                    ui.set_width(col);
                    dynamics_card(ui, s, ch);
                });
            });
        } else {
            spatial_card(ui, s, ch);
            ui.add_space(GAP);
            dynamics_card(ui, s, ch);
        }
    }
}

/// Cabecera de la pestaña de un canal, compacta: nombre y origen, volumen con silencio y vúmetro,
/// y sus apps en una fila. Devuelve la app que se haya soltado en la lista.
fn channel_header(ui: &mut Ui, s: &mut Settings, ch: Channel, level: f32, source: Option<&str>, apps: &[&AppInfo]) -> Option<String> {
    let color = channel_color(ch);
    let mut dropped = None;
    card_with_margin(ui, Some(color), false, 18, |ui| {
        let c = s.channel_mut(ch);
        ui.horizontal(|ui| {
            widgets::badge_icon(ui, theme::channel_icon(ch), color, 30.0);
            ui.label(RichText::new(ch.label()).font(semibold(17.0)).color(TEXT));
            ui.add_space(4.0);
            let icon = if c.muted { icons::SPEAKER_OFF } else { icons::SPEAKER };
            if icon_button(ui, icon, if c.muted { tr!("Activar sonido", "Unmute") } else { tr!("Silenciar", "Mute") }, c.muted, DANGER).clicked() {
                c.muted = !c.muted;
            }
            let value_w = 74.0;
            let meter_w = (ui.available_width() * 0.22).clamp(80.0, 200.0);
            let source_w = (ui.available_width() * 0.22).clamp(110.0, 230.0);
            ui.spacing_mut().slider_width = (ui.available_width() - value_w - meter_w - source_w - 40.0).max(100.0);
            ui.add(egui::Slider::new(&mut c.volume, 0.0..=1.0).show_value(false)).on_hover_text(tr!("Volumen del canal", "Channel volume"));
            let txt = if c.muted { tr!("Silenciado", "Muted").to_string() } else { format!("{:.0} %", c.volume * 100.0) };
            ui.add_sized([value_w, 20.0], egui::Label::new(RichText::new(txt).font(semibold(14.0)).color(if c.muted { DANGER } else { TEXT })));
            hmeter(ui, if c.muted { 0.0 } else { level }, meter_w, Some(color));
            let (icon, text, tip) = match source {
                Some(d) => (icons::PLUG, d.split(" (").next().unwrap_or(d).to_string(), d.to_string()),
                None => (icons::WAVEFORM, tr!("Captura por aplicación", "Per-app capture").to_string(), tr!("Sin dispositivo virtual: se captura cada app", "No virtual device: each app is captured separately").to_string()),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::truncated(ui, RichText::new(text).size(12.0).color(TEXT_3)).on_hover_text(tip);
                ui.label(RichText::new(icon).font(theme::icon(13.0)).color(TEXT_3));
            });
        });
        ui.add_space(10.0);
        dropped = app_chips(ui, apps, color);
    });
    dropped
}

/// Apps del canal en fila (pestaña de canal). También acepta apps soltadas.
fn app_chips(ui: &mut Ui, apps: &[&AppInfo], color: Color32) -> Option<String> {
    let frame = egui::Frame::new().fill(BG).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 8));
    let (inner, dropped) = ui.dnd_drop_zone::<AppDrag, ()>(frame, |ui| {
        ui.set_min_size(vec2(ui.available_width(), 28.0));
        ui.horizontal_wrapped(|ui| {
            widgets::caption(ui, &trf!("Aplicaciones ({})", "Apps ({})", apps.len()));
            ui.add_space(6.0);
            if apps.is_empty() {
                ui.label(RichText::new(tr!("Ninguna app suena ahora en este canal. Para cambiar una app de canal, arrástrala en la pestaña Mezcla", "No app is playing on this channel right now. To move an app to another channel, drag it in the Mix tab")).size(11.5).color(TEXT_3));
            }
            for app in apps {
                ui.scope(|ui| {
                    ui.set_max_width(180.0);
                    app_row(ui, app, color);
                });
            }
        });
    });
    if inner.response.dnd_hover_payload::<AppDrag>().is_some() {
        widgets::outline(ui, inner.response.rect, color);
    }
    dropped.map(|p| p.name.clone())
}

/// Volumen inteligente y ganancia del canal.
fn dynamics_card(ui: &mut Ui, s: &mut Settings, ch: Channel) {
    use crate::config::Loudness;
    let c = s.channel_mut(ch);
    card(ui, None, false, |ui| {
        ui.horizontal(|ui| {
            toggle(ui, &mut c.smart_volume);
            ui.label(RichText::new(tr!("Volumen inteligente", "Smart volume")).font(semibold(15.0)));
        });
        ui.label(RichText::new(tr!("Sube lo flojo (pasos, diálogos) y contiene lo fuerte (explosiones)", "Lifts quiet sounds (footsteps, dialogue) and tames loud ones (explosions)")).size(12.5).color(TEXT_3));
        ui.add_space(6.0);
        ui.add_enabled_ui(c.smart_volume, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr!("Carácter", "Character")).size(12.5).color(TEXT_2));
                for l in Loudness::ALL {
                    ui.selectable_value(&mut c.smart_loudness, l, l.label());
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr!("Nivel", "Level")).size(12.5).color(TEXT_2));
                ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(100.0, 320.0);
                ui.add(egui::Slider::new(&mut c.smart_level, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
            });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new(tr!("Ganancia", "Gain")).font(semibold(15.0)));
        ui.label(RichText::new(tr!("Sube o baja todo el canal antes del fader, hasta ±12 dB", "Raises or lowers the whole channel before the fader, up to ±12 dB")).size(12.5).color(TEXT_3));
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(100.0, 320.0);
            ui.add(egui::Slider::new(&mut c.boost_db, -12.0..=12.0).suffix(" dB").fixed_decimals(1)).on_hover_text(tr!("Doble clic para volver a 0 dB", "Double-click to reset to 0 dB"));
        });
    });
}

/// Audio espacial del canal.
fn spatial_card(ui: &mut Ui, s: &mut Settings, ch: Channel) {
    use crate::config::{SpatialMode, speaker_names};
    let color = channel_color(ch);
    let c = s.channel_mut(ch);
    card(ui, None, false, |ui| {
        ui.horizontal(|ui| {
            toggle(ui, &mut c.surround.enabled);
            ui.label(RichText::new(tr!("Audio espacial", "Spatial audio")).font(semibold(15.0)));
            widgets::pill(ui, ch.label(), color);
        });
        ui.horizontal(|ui| {
            for (mode, label) in [(SpatialMode::Headphones, tr!("Modo auriculares", "Headphone mode")), (SpatialMode::Speakers, tr!("Modo altavoz", "Speaker mode"))] {
                let on = c.surround.mode == mode;
                let text = RichText::new(label).font(theme::medium(12.5)).color(if on { BG } else { TEXT_2 });
                let b = egui::Button::new(text)
                    .fill(if on { ACCENT } else { SURFACE_2 })
                    .stroke(Stroke::new(1.0, if on { ACCENT } else { BORDER }))
                    .corner_radius(CornerRadius::same(8));
                if ui.add(b).clicked() {
                    c.surround.mode = mode;
                }
            }
        });
        ui.label(
            RichText::new(
                "Hacia Rendimiento el sonido es más seco y se localiza mejor (shooters competitivos); hacia Inmersión gana \
                 ambiente y sensación de sala (juegos de historia y películas).",
            )
            .size(12.5)
            .color(TEXT_3),
        );
        ui.add_space(6.0);
        ui.add_enabled_ui(c.surround.enabled, |ui| {
            let w = (ui.available_width() - 190.0).max(120.0);
            ui.spacing_mut().slider_width = w;
            ui.horizontal(|ui| {
                ui.add_sized([90.0, 18.0], egui::Label::new(RichText::new(tr!("Rendimiento", "Performance")).size(12.5).color(TEXT_2)));
                ui.add(egui::Slider::new(&mut c.surround.immersion, 0.0..=1.0).show_value(false));
                ui.label(RichText::new(tr!("Inmersión", "Immersion")).size(12.5).color(TEXT_2));
            });
            ui.horizontal(|ui| {
                ui.add_sized([90.0, 18.0], egui::Label::new(RichText::new(tr!("Distancia", "Distance")).size(12.5).color(TEXT_2)));
                ui.add(egui::Slider::new(&mut c.surround.distance, 0.0..=100.0).fixed_decimals(0));
            });
            egui::CollapsingHeader::new(RichText::new(tr!("Posición y nivel de cada altavoz virtual", "Position and level of each virtual speaker")).color(TEXT_2))
                .id_salt(("speakers", ch.index()))
                .show(ui, |ui| {
                    ui.spacing_mut().slider_width = (ui.available_width() - 200.0).clamp(100.0, 220.0);
                    egui::Grid::new(("speaker_grid", ch.index())).num_columns(3).spacing([18.0, 6.0]).show(ui, |ui| {
                        for (i, name) in speaker_names().iter().enumerate() {
                            let sp = &mut c.surround.speakers[i];
                            ui.label(RichText::new(*name).color(TEXT_2));
                            // El subwoofer no tiene dirección.
                            ui.add_enabled(i != 3, egui::Slider::new(&mut sp.azimuth, -180.0..=180.0).suffix("°").fixed_decimals(0));
                            ui.add(egui::DragValue::new(&mut sp.gain_db).range(-24.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB"));
                            ui.end_row();
                        }
                    });
                    if widgets::secondary_button(ui, icons::RESET, tr!("Posiciones de fábrica", "Factory positions"), true).clicked() {
                        c.surround.speakers = crate::config::SpatialSettings::default().speakers;
                    }
                });
        });

    });
}

struct StripResult {
    open: bool,
    dropped: Option<String>,
}

/// Cabecera de tira: icono y nombre del canal y, si se puede abrir, la rueda que lleva a su pestaña.
fn strip_header(ui: &mut Ui, icon: &str, name: &str, color: Color32, can_open: bool) -> bool {
    let mut open = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let r = widgets::badge_icon(ui, icon, color, 28.0).interact(Sense::click());
        if can_open {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                open |= icon_button(ui, icons::SETTINGS, &trf!("Configurar {name}", "Configure {name}"), false, color).clicked();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let title = widgets::truncated(ui, RichText::new(name).font(semibold(14.0)).color(color)).interact(Sense::click());
                    if title.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    open |= title.clicked();
                });
            });
            open |= r.clicked();
        } else {
            widgets::truncated(ui, RichText::new(name).font(semibold(14.0)).color(color));
        }
    });
    open
}

/// Línea pequeña con el origen del audio (dispositivo virtual o modo de captura).
fn source_line(ui: &mut Ui, icon: &str, text: &str, tooltip: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new(icon).font(theme::icon(12.5)).color(TEXT_3));
        widgets::truncated(ui, RichText::new(text).size(11.5).color(TEXT_3)).on_hover_text(tooltip);
    });
}

/// Fader + vúmetro centrados, el valor y el botón de silencio debajo (como en Sonar).
fn fader_block(ui: &mut Ui, height: f32, level: f32, color: Color32, volume: &mut f32, muted: &mut bool, mute_icons: (&str, &str)) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let w = ui.available_width();
        ui.add_space(((w - 54.0) / 2.0).max(0.0));
        vmeter(ui, if *muted { 0.0 } else { level }, height);
        ui.add_space(4.0);
        fader(ui, volume, color, height, *muted);
    });
    ui.vertical_centered(|ui| {
        let txt = if *muted { tr!("Silenciado", "Muted").to_string() } else { format!("{:.0}", *volume * 100.0) };
        let font = if *muted { theme::medium(12.5) } else { theme::semibold(19.0) };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let w = ui.available_width();
            let galley_w = widgets::text_width(ui, &txt, font.clone()) + if *muted { 0.0 } else { 14.0 };
            ui.add_space(((w - galley_w) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(RichText::new(&txt).font(font).color(if *muted { DANGER } else { TEXT }));
            if !*muted {
                ui.label(RichText::new("%").size(11.5).color(TEXT_3));
            }
        });
        let (off, on) = mute_icons;
        let icon = if *muted { off } else { on };
        if icon_button(ui, icon, if *muted { tr!("Activar sonido", "Unmute") } else { tr!("Silenciar", "Mute") }, *muted, DANGER).clicked() {
            *muted = !*muted;
        }
    });
}

fn master_strip(ui: &mut Ui, width: f32, fader_h: f32, level: f32, s: &mut Settings, output: &str) {
    card_with_margin(ui, Some(MASTER), false, STRIP_PAD as i8, |ui| {
        ui.set_width(width);
        strip_header(ui, icons::HEADPHONES, "Master", MASTER, false);
        ui.add_space(4.0);
        // Mismo alto que la fila de presets de las otras tiras, para alinear los faders.
        ui.add_sized([width, 32.0], egui::Label::new(RichText::new(tr!("Salida final", "Final output")).size(11.5).color(TEXT_3)));
        ui.add_space(2.0);
        source_line(ui, icons::HEADPHONES, output, output);
        let mut muted = s.master_muted;
        fader_block(ui, fader_h, level, MASTER, &mut s.master_volume, &mut muted, (icons::SPEAKER_OFF, icons::SPEAKER));
        s.master_muted = muted;
    });
}

#[allow(clippy::too_many_arguments)]
fn strip(
    ui: &mut Ui,
    width: f32,
    fader_h: f32,
    t: EqTarget,
    level: f32,
    s: &mut Settings,
    apps: Option<&[&AppInfo]>,
    source: Option<&str>,
) -> StripResult {
    let color = target_color(t);
    let mut result = StripResult { open: false, dropped: None };

    card_with_margin(ui, Some(color), false, STRIP_PAD as i8, |ui| {
        ui.set_width(width);
        result.open |= strip_header(ui, target_icon(t), t.label(), color, true);
        ui.add_space(4.0);
        preset_combo(ui, s, t, width - 6.0, "strip");
        ui.add_space(2.0);

        match (source, t) {
            (Some(d), _) => source_line(ui, icons::PLUG, d.split(" (").next().unwrap_or(d), d),
            (None, EqTarget::Mic) => source_line(ui, icons::WARNING, tr!("Sin salida virtual", "No virtual output"), tr!("Asígnala en Ajustes → Dispositivos virtuales", "Assign it in Settings → Virtual devices")),
            (None, _) => source_line(ui, icons::WAVEFORM, tr!("Por aplicación", "Per app"), tr!("Sin dispositivo virtual: se captura cada app", "No virtual device: each app is captured separately")),
        }

        let (mut volume, mut muted) = match t {
            EqTarget::Channel(ch) => (s.channel(ch).volume, s.channel(ch).muted),
            EqTarget::Mic => (s.mic.volume, s.mic.muted),
        };
        let mute_icons = if t == EqTarget::Mic { (icons::MICROPHONE_OFF, icons::MICROPHONE) } else { (icons::SPEAKER_OFF, icons::SPEAKER) };
        fader_block(ui, fader_h, level, color, &mut volume, &mut muted, mute_icons);
        match t {
            EqTarget::Channel(ch) => {
                let c = s.channel_mut(ch);
                c.volume = volume;
                c.muted = muted;
            }
            EqTarget::Mic => {
                s.mic.volume = volume;
                s.mic.muted = muted;
            }
        }

        ui.add_space(8.0);
        match apps {
            Some(apps) => {
                widgets::caption(ui, &trf!("Aplicaciones ({})", "Apps ({})", apps.len()));
                result.dropped = app_drop_zone(ui, apps, color);
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if s.mic.enabled { tr!("Procesando", "Processing") } else { tr!("Apagado", "Off") }).size(12.0).color(TEXT_2));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        toggle(ui, &mut s.mic.enabled).on_hover_text(tr!("Activar el procesado del micro", "Turn on mic processing"));
                    });
                });
            }
        }
    });
    result
}

/// Zona donde se sueltan las apps arrastradas desde otro canal.
fn app_drop_zone(ui: &mut Ui, apps: &[&AppInfo], color: Color32) -> Option<String> {
    let frame = egui::Frame::new().fill(BG).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(6));
    let (inner, dropped) = ui.dnd_drop_zone::<AppDrag, ()>(frame, |ui| {
        ui.set_min_size(vec2(ui.available_width(), 64.0));
        ui.spacing_mut().item_spacing.y = 4.0;
        if apps.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(tr!("Arrastra aquí una app", "Drag an app here")).size(11.5).color(TEXT_3));
            });
        }
        for app in apps {
            app_row(ui, app, color);
        }
    });
    // Resalta la zona mientras se arrastra algo encima.
    if inner.response.dnd_hover_payload::<AppDrag>().is_some() {
        widgets::outline(ui, inner.response.rect, color);
    }
    dropped.map(|p| p.name.clone())
}

fn app_row(ui: &mut Ui, app: &AppInfo, color: Color32) {
    let id = ui.scope_id().with(("app", app.pid));
    let resp = ui.dnd_drag_source(id, AppDrag { name: app.name.clone() }, |ui| {
        egui::Frame::new()
            .fill(SURFACE_2)
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(icons::GRIP).font(theme::icon(12.0)).color(TEXT_3));
                    let warn = app.error.is_some();
                    let icon = if warn { icons::WARNING } else { icons::APP };
                    ui.label(RichText::new(icon).font(theme::icon(13.0)).color(if warn { DANGER } else { TEXT_2 }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        widgets::activity_dot(ui, app.peak > 0.001 || app.via_device, color);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            widgets::truncated(ui, RichText::new(&app.name).size(12.0).color(TEXT));
                        });
                    });
                });
            });
    });
    let tip = match &app.error {
        Some(e) => trf!("No se puede capturar: {e}", "Can't capture: {e}"),
        None if app.via_device => trf!("PID {} · suena en el dispositivo virtual del canal", "PID {} · playing on the channel's virtual device", app.pid),
        None if app.direct => trf!("PID {} · suena directamente en los auriculares, fuera del mezclador", "PID {} · playing straight to the headphones, outside the mixer", app.pid),
        None => trf!("PID {} · arrástrala a otro canal", "PID {} · drag it to another channel", app.pid),
    };
    resp.response.on_hover_text(tip);
}

/// ChatMix en una sola fila: Juego · deslizador · Chat · interruptor.
fn chatmix_card(ui: &mut Ui, s: &mut Settings) {
    let game = channel_color(Channel::Game);
    let chat = channel_color(Channel::Chat);
    card_with_margin(ui, None, false, 16, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new(icons::FADERS).font(theme::icon(17.0)).color(TEXT_2));
            ui.label(RichText::new("ChatMix").font(semibold(14.5)))
                .on_hover_text(tr!("Equilibrio entre el juego y el chat de voz. Doble clic en el deslizador para centrar", "Balance between game and voice chat. Double-click the slider to center it"));
            ui.add_space(14.0);
            let g = s.chatmix_gain(Channel::Game) * 100.0;
            let c = s.chatmix_gain(Channel::Chat) * 100.0;
            side_label(ui, theme::channel_icon(Channel::Game), tr!("Juego", "Game"), g, game);
            let w = (ui.available_width() - 190.0).max(120.0);
            ui.add_space(6.0);
            widgets::chatmix_slider(ui, &mut s.chatmix, game, chat, s.chatmix_enabled, w).on_hover_text(tr!("Doble clic para centrar", "Double-click to center"));
            ui.add_space(6.0);
            side_label(ui, theme::channel_icon(Channel::Chat), "Chat", c, chat);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                toggle(ui, &mut s.chatmix_enabled);
            });
        });
    });
}

fn side_label(ui: &mut Ui, icon: &str, name: &str, pct: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(icon).font(theme::icon(14.0)).color(color));
        ui.label(RichText::new(format!("{name} {pct:.0}%")).font(theme::medium(12.5)).color(TEXT_2));
    });
}
