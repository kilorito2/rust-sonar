//! Editor de ecualizador: barra de presets, gráfica interactiva y panel de la banda seleccionada.
//!
//! En la gráfica: arrastrar un punto = frecuencia (horizontal) y ganancia (vertical);
//! rueda del ratón sobre la gráfica = Q de la banda seleccionada; doble clic en vacío = nueva banda.

use super::icons;
use super::theme::{self, *};
use super::widgets::{self, card, icon_button, secondary_button, toggle};
use crate::config::{EQ_PRESETS, preset_label, EqBand, EqSettings, EqTarget, FilterKind, SAMPLE_RATE, Settings};
use crate::dsp::biquad::Coeffs;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, Layout, Mesh, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use std::collections::HashMap;

const MAX_BANDS: usize = 16;
const DB_RANGE: f32 = 18.0;
const NODE_RADIUS: f32 = 8.0;

#[derive(Default)]
pub struct EqUiState {
    pub new_preset_name: String,
    /// Enlace de configuración de Sonar pegado para importar.
    pub import_link: String,
    selected: HashMap<&'static str, usize>,
}

impl EqUiState {
    fn selected(&self, t: EqTarget, len: usize) -> Option<usize> {
        self.selected.get(t.key()).copied().filter(|&i| i < len)
    }
    fn select(&mut self, t: EqTarget, i: Option<usize>) {
        match i {
            Some(i) => self.selected.insert(t.key(), i),
            None => self.selected.remove(t.key()),
        };
    }
}

/// Devuelve Some(enlace) si el usuario ha pedido importar una configuración de Sonar.
pub fn eq_editor(ui: &mut Ui, state: &mut EqUiState, s: &mut Settings, t: EqTarget) -> Option<String> {
    let color = target_color(t);
    let mut import = None;
    card(ui, None, false, |ui| {
        // Cabecera
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::EQUALIZER).font(theme::icon(18.0)).color(TEXT_2));
            ui.label(RichText::new(tr!("Ecualizador", "Equalizer")).font(semibold(15.0)));
            ui.add_space(4.0);
            widgets::pill(ui, t.label(), color);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                toggle(ui, &mut s.eq_for_mut(t).enabled);
                ui.label(RichText::new(tr!("Activado", "Enabled")).size(12.5).color(TEXT_2));
            });
        });
        ui.add_space(8.0);
        preset_toolbar(ui, state, s, t);
        ui.add_space(10.0);

        let selected = state.selected(t, s.eq_for(t).bands.len());
        let mut new_sel = selected;
        graph(ui, s.eq_for_mut(t), color, &mut new_sel);
        if new_sel != selected {
            state.select(t, new_sel);
        }
        ui.add_space(10.0);
        let sel = state.selected(t, s.eq_for(t).bands.len());
        if let Some(removed) = band_inspector(ui, s.eq_for_mut(t), sel, color) {
            let _ = removed;
            state.select(t, None);
        }

        // Controles rápidos de tono, como bajo la gráfica de Sonar.
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let tone = &mut s.eq_for_mut(t).tone;
            let w = ((ui.available_width() - 3.0 * 150.0) / 3.0).clamp(90.0, 260.0);
            ui.spacing_mut().slider_width = w;
            for (label, value) in [(tr!("Graves", "Bass"), &mut tone.bass_db), (tr!("Voz", "Voice"), &mut tone.voice_db), (tr!("Agudos", "Treble"), &mut tone.treble_db)] {
                ui.label(RichText::new(label).size(12.5).color(TEXT_2));
                ui.add(egui::Slider::new(value, -12.0..=12.0).suffix(" dB").fixed_decimals(1))
                    .on_hover_text(tr!("Doble clic para volver a 0 dB", "Double-click to reset to 0 dB"));
                ui.add_space(10.0);
            }
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::BROADCAST).font(theme::icon(15.0)).color(TEXT_2));
            let w = (ui.available_width() - 250.0).max(200.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.import_link)
                    .hint_text(tr!("Pega aquí un enlace de configuración de SteelSeries Sonar", "Paste a SteelSeries Sonar config link here"))
                    .desired_width(w)
                    .margin(vec2(8.0, 6.0)),
            );
            let link = state.import_link.trim().to_owned();
            if secondary_button(ui, icons::PLUS, &trf!("Importar a {}", "Import to {}", t.label()), !link.is_empty())
                .on_hover_text(tr!("Descarga la configuración compartida y la aplica a este canal (EQ, tono, ganancia, audio espacial)", "Downloads the shared config and applies it to this channel (EQ, tone, gain, spatial audio)"))
                .clicked()
            {
                import = Some(link);
                state.import_link.clear();
            }
        });
    });
    import
}

fn preset_toolbar(ui: &mut Ui, state: &mut EqUiState, s: &mut Settings, t: EqTarget) {
    ui.horizontal_wrapped(|ui| {
        preset_combo(ui, s, t, 200.0, "editor");

        let user_active = s.active_preset(t).filter(|n| s.is_user_preset(t, n)).map(str::to_owned);
        if let Some(name) = &user_active {
            let modified = s.preset_modified(t);
            ui.add_enabled_ui(modified, |ui| {
                if icon_button(ui, icons::SAVE, tr!("Guardar cambios en el preset", "Save changes to the preset"), false, ACCENT).clicked() {
                    s.save_preset(t, name);
                }
            });
            let fav = s.presets_for(t).any(|p| &p.name == name && p.favorite);
            if icon_button(ui, icons::STAR, if fav { tr!("Quitar de favoritos", "Remove from favorites") } else { tr!("Añadir a favoritos", "Add to favorites") }, fav, WARN).clicked() {
                s.toggle_favorite(t, name);
            }
            if icon_button(ui, icons::TRASH, tr!("Borrar preset", "Delete preset"), false, DANGER).clicked() {
                s.delete_preset(t, name);
            }
        }

        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.new_preset_name)
                .hint_text(tr!("Nombre del nuevo preset", "New preset name"))
                .desired_width(170.0)
                .margin(vec2(8.0, 6.0)),
        );
        let name = state.new_preset_name.trim().to_owned();
        if secondary_button(ui, icons::PLUS, tr!("Guardar como", "Save as"), !name.is_empty()).clicked() {
            s.save_preset(t, &name);
            state.new_preset_name.clear();
        }

        // Sin sitio para el preamp a la derecha: a la línea siguiente (si no, se monta sobre los botones).
        if ui.available_width() < 300.0 {
            ui.end_row();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let eq = s.eq_for_mut(t);
            ui.add(egui::Slider::new(&mut eq.preamp_db, -15.0..=15.0).suffix(" dB").fixed_decimals(1));
            ui.label(RichText::new("Preamp").size(12.5).color(TEXT_2));
        });
    });
}

/// Desplegable de presets: "Mis presets" + "Integrados". Devuelve true si se eligió uno.
pub fn preset_combo(ui: &mut Ui, s: &mut Settings, t: EqTarget, width: f32, place: &str) -> bool {
    let active = s.active_preset(t).map(str::to_owned);
    let mut text = active.as_deref().map(|n| preset_label(n).to_string()).unwrap_or_else(|| tr!("Personalizado", "Custom").into());
    if active.is_some() && s.preset_modified(t) {
        text.push_str(tr!("  ·  editado", "  ·  edited"));
    }
    let mut chosen = None;
    let user: Vec<(String, bool)> = s.presets_for(t).map(|p| (p.name.clone(), p.favorite)).collect();
    egui::ComboBox::from_id_salt(("preset", t.key(), place))
        .selected_text(RichText::new(format!("{}  {text}", icons::FADERS)).color(TEXT))
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            if !user.is_empty() {
                widgets::caption(ui, tr!("Mis presets", "My presets"));
                for (name, fav) in &user {
                    let label = if *fav { format!("{}  {name}", icons::STAR) } else { name.clone() };
                    if ui.selectable_label(active.as_deref() == Some(name), label).clicked() {
                        chosen = Some(name.clone());
                    }
                }
                ui.separator();
            }
            widgets::caption(ui, tr!("Integrados", "Built-in"));
            for name in EQ_PRESETS {
                if ui.selectable_label(active.as_deref() == Some(name), preset_label(name)).clicked() {
                    chosen = Some(name.to_string());
                }
            }
        });
    match chosen {
        Some(name) => s.apply_preset(t, &name),
        None => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Gráfica
// ---------------------------------------------------------------------------------------------

struct Axes {
    rect: Rect,
    lo: f32,
    hi: f32,
}

impl Axes {
    fn x(&self, f: f32) -> f32 {
        self.rect.left() + (f.max(1.0).log10() - self.lo) / (self.hi - self.lo) * self.rect.width()
    }
    fn f(&self, x: f32) -> f32 {
        10f32.powf(self.lo + (x - self.rect.left()) / self.rect.width() * (self.hi - self.lo))
    }
    fn y(&self, db: f32) -> f32 {
        self.rect.center().y - (db / DB_RANGE).clamp(-1.05, 1.05) * self.rect.height() * 0.5
    }
    fn db(&self, y: f32) -> f32 {
        (self.rect.center().y - y) / (self.rect.height() * 0.5) * DB_RANGE
    }
}

fn band_coeffs(b: &EqBand) -> Coeffs {
    Coeffs::design(b.kind, b.freq as f64, b.gain_db as f64, b.q as f64, SAMPLE_RATE as f64)
}

fn total_db(coeffs: &[(bool, Coeffs)], preamp: f32, enabled: bool, f: f32) -> f32 {
    if !enabled {
        return 0.0;
    }
    let fs = SAMPLE_RATE as f64;
    preamp + coeffs.iter().filter(|(on, _)| *on).map(|(_, c)| c.magnitude_db(f as f64, fs) as f32).sum::<f32>()
}

/// Relleno entre una curva y la línea de 0 dB (malla de triángulos).
fn fill_to_zero(points: &[Pos2], zero_y: f32, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for (i, p) in points.iter().enumerate() {
        mesh.colored_vertex(*p, color);
        mesh.colored_vertex(pos2(p.x, zero_y), color.gamma_multiply(0.25));
        if i > 0 {
            let k = (i as u32) * 2;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
    }
    mesh
}

fn graph(ui: &mut Ui, eq: &mut EqSettings, color: Color32, selected: &mut Option<usize>) {
    let width = ui.available_width();
    let (outer, resp) = ui.allocate_exact_size(vec2(width, 250.0), Sense::click_and_drag());
    let rect = outer.shrink2(vec2(8.0, 14.0));
    let ax = Axes { rect, lo: 20f32.log10(), hi: 20_000f32.log10() };
    let drag_key = resp.id.with("drag");

    let coeffs: Vec<(bool, Coeffs)> = eq.all_bands().map(|b| (b.enabled, band_coeffs(&b))).collect();
    let node_pos = |_i: usize, b: &EqBand| -> Pos2 {
        let y = if b.kind.uses_gain() {
            ax.y(b.gain_db)
        } else {
            ax.y(total_db(&coeffs, eq.preamp_db, true, b.freq))
        };
        pos2(ax.x(b.freq), y)
    };
    let positions: Vec<Pos2> = eq.bands.iter().enumerate().map(|(i, b)| node_pos(i, b)).collect();
    let nearest = |p: Pos2| -> Option<usize> {
        positions
            .iter()
            .enumerate()
            .map(|(i, q)| (i, q.distance(p)))
            .filter(|(_, d)| *d <= NODE_RADIUS + 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    // --- Interacción ---
    let hover_node = resp.hover_pos().and_then(nearest);
    if resp.drag_started() {
        let hit = resp.interact_pointer_pos().and_then(nearest);
        ui.memory_mut(|m| m.data.insert_temp(drag_key, hit));
        if hit.is_some() {
            *selected = hit;
        }
    }
    let dragging: Option<usize> = ui.memory(|m| m.data.get_temp(drag_key)).flatten();
    if resp.dragged() {
        if let (Some(i), Some(p)) = (dragging, resp.interact_pointer_pos()) {
            let band = &mut eq.bands[i];
            band.freq = ax.f(p.x).clamp(20.0, 20_000.0).round();
            if band.kind.uses_gain() {
                band.gain_db = (ax.db(p.y).clamp(-15.0, 15.0) * 10.0).round() / 10.0;
            }
        }
        ui.ctx().set_cursor_icon(if dragging.is_some() { CursorIcon::Grabbing } else { CursorIcon::Default });
    } else if hover_node.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    if resp.drag_stopped() {
        ui.memory_mut(|m| m.data.remove::<Option<usize>>(drag_key));
    }
    if resp.clicked() {
        *selected = resp.interact_pointer_pos().and_then(nearest).or(*selected);
    }
    if resp.double_clicked() && hover_node.is_none() && eq.bands.len() < MAX_BANDS {
        if let Some(p) = resp.interact_pointer_pos() {
            let gain = (ax.db(p.y).clamp(-15.0, 15.0) * 10.0).round() / 10.0;
            eq.bands.push(EqBand { enabled: true, kind: FilterKind::Peaking, freq: ax.f(p.x).round(), gain_db: gain, q: 1.0 });
            *selected = Some(eq.bands.len() - 1);
        }
    }
    if resp.hovered() {
        if let Some(i) = *selected {
            // La rueda ajusta la Q y no desplaza la página.
            let scroll = ui.input_mut(|inp| std::mem::take(&mut inp.smooth_scroll_delta.y));
            if scroll != 0.0 {
                let b = &mut eq.bands[i];
                b.q = (b.q * (scroll * 0.004).exp()).clamp(0.1, 10.0);
            }
        }
    }

    // --- Dibujo ---
    let coeffs: Vec<(bool, Coeffs)> = eq.all_bands().map(|b| (b.enabled, band_coeffs(&b))).collect();
    let p = ui.painter_at(outer);
    p.rect_filled(outer, CornerRadius::same(10), BG);

    let font = theme::medium(10.0);
    let grid = Stroke::new(1.0, Color32::from_rgb(0x1a, 0x1f, 0x27));
    for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
        let x = ax.x(f);
        p.line_segment([pos2(x, outer.top()), pos2(x, outer.bottom())], grid);
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        p.text(pos2(x + 3.0, outer.bottom() - 3.0), Align2::LEFT_BOTTOM, label, font.clone(), TEXT_3);
    }
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = ax.y(db);
        let stroke = if db == 0.0 { Stroke::new(1.0, BORDER_STRONG) } else { grid };
        p.line_segment([pos2(outer.left(), y), pos2(outer.right(), y)], stroke);
        p.text(pos2(outer.left() + 6.0, y - 2.0), Align2::LEFT_BOTTOM, format!("{db:+.0} dB"), font.clone(), TEXT_3);
    }

    let curve_color = if eq.enabled { color } else { TEXT_3 };
    let steps = (rect.width() as usize / 2).max(2);
    let xs: Vec<f32> = (0..=steps).map(|i| rect.left() + rect.width() * i as f32 / steps as f32).collect();
    let zero = ax.y(0.0);

    // Curva de la banda seleccionada (sola, tenue).
    if let Some(i) = *selected {
        if let Some(b) = eq.bands.get(i).filter(|b| b.enabled) {
            let c = band_coeffs(b);
            let pts: Vec<Pos2> = xs
                .iter()
                .map(|&x| pos2(x, ax.y(c.magnitude_db(ax.f(x) as f64, SAMPLE_RATE as f64) as f32)))
                .collect();
            p.add(fill_to_zero(&pts, zero, curve_color.gamma_multiply(0.10)));
            p.add(egui::Shape::line(pts, Stroke::new(1.0, curve_color.gamma_multiply(0.5))));
        }
    }

    // Curva total.
    let pts: Vec<Pos2> = xs.iter().map(|&x| pos2(x, ax.y(total_db(&coeffs, eq.preamp_db, eq.enabled, ax.f(x))))).collect();
    p.add(fill_to_zero(&pts, zero, curve_color.gamma_multiply(0.16)));
    p.add(egui::Shape::line(pts, Stroke::new(2.0, curve_color)));

    // Puntos de las bandas.
    for (i, b) in eq.bands.iter().enumerate() {
        let pos = node_pos(i, b);
        let is_sel = *selected == Some(i);
        let r = if is_sel || hover_node == Some(i) { NODE_RADIUS + 1.0 } else { NODE_RADIUS };
        let c = if b.enabled && eq.enabled { color } else { TEXT_3 };
        if is_sel {
            p.circle_filled(pos, r + 5.0, c.gamma_multiply(0.15));
            p.circle_filled(pos, r, c);
        } else {
            p.circle_filled(pos, r, SURFACE_2);
            p.circle_stroke(pos, r, Stroke::new(2.0, c));
        }
        let txt = if is_sel { BG } else { TEXT };
        p.text(pos, Align2::CENTER_CENTER, format!("{}", i + 1), theme::semibold(10.0), txt);
    }
    p.rect_stroke(outer, CornerRadius::same(10), Stroke::new(1.0, BORDER), StrokeKind::Inside);
}

// ---------------------------------------------------------------------------------------------
// Panel de banda
// ---------------------------------------------------------------------------------------------

/// Controles numéricos de la banda seleccionada. Devuelve Some(i) si se borró la banda i.
fn band_inspector(ui: &mut Ui, eq: &mut EqSettings, selected: Option<usize>, color: Color32) -> Option<usize> {
    let mut removed = None;
    egui::Frame::new()
        .fill(SURFACE_2)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                match selected {
                    Some(i) => {
                        let band = &mut eq.bands[i];
                        widgets::badge_icon(ui, &format!("{}", i + 1), color, 26.0);
                        toggle(ui, &mut band.enabled).on_hover_text(tr!("Activar banda", "Enable band"));
                        egui::ComboBox::from_id_salt("band_kind").selected_text(band.kind.label()).width(120.0).show_ui(
                            ui,
                            |ui| {
                                for k in FilterKind::ALL {
                                    ui.selectable_value(&mut band.kind, k, k.label());
                                }
                            },
                        );
                        field(ui, tr!("Frecuencia", "Frequency"), |ui| {
                            let speed = (band.freq * 0.01).max(1.0);
                            ui.add(egui::DragValue::new(&mut band.freq).range(20.0..=20_000.0).speed(speed).max_decimals(0).suffix(" Hz"))
                        });
                        field(ui, tr!("Ganancia", "Gain"), |ui| {
                            ui.add_enabled(
                                band.kind.uses_gain(),
                                egui::DragValue::new(&mut band.gain_db).range(-15.0..=15.0).speed(0.1).fixed_decimals(1).suffix(" dB"),
                            )
                        });
                        field(ui, "Q", |ui| ui.add(egui::DragValue::new(&mut band.q).range(0.1..=10.0).speed(0.01).fixed_decimals(2)));
                        if icon_button(ui, icons::TRASH, tr!("Quitar banda", "Remove band"), false, DANGER).clicked() {
                            removed = Some(i);
                        }
                    }
                    None => {
                        ui.label(RichText::new(icons::INFO).font(theme::icon(15.0)).color(TEXT_3));
                        ui.label(
                            RichText::new(tr!("Arrastra un punto para ajustar · rueda = ancho (Q) · doble clic en la gráfica = nueva banda", "Drag a point to adjust · wheel = width (Q) · double-click the graph = new band"))
                                .size(12.5)
                                .color(TEXT_3),
                        );
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if secondary_button(ui, icons::RESET, tr!("Plano", "Flat"), true).clicked() {
                        *eq = EqSettings::flat();
                    }
                    let can_add = eq.bands.len() < MAX_BANDS;
                    if secondary_button(ui, icons::PLUS, tr!("Banda", "Band"), can_add).clicked() {
                        eq.bands.push(EqBand { enabled: true, kind: FilterKind::Peaking, freq: 1000.0, gain_db: 0.0, q: 1.0 });
                    }
                });
            });
        });
    if let Some(i) = removed {
        eq.bands.remove(i);
    }
    removed
}

fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> egui::Response) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(label).size(10.5).color(TEXT_3));
        add(ui);
    });
}
