//! Widgets dibujados a mano para que la app no parezca "egui por defecto": fader de mesa de
//! mezclas, vúmetro de LEDs, interruptor animado, botones de icono, slider de ChatMix, tarjetas…

use super::theme::{self, *};
use crate::state::DeviceInfo;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, InnerResponse, Layout, Margin, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------
// Contenedores
// ---------------------------------------------------------------------------------------------

/// Tarjeta: superficie con borde fino. Con `accent`, dibuja una línea de color arriba.
pub fn card<R>(ui: &mut Ui, accent: Option<Color32>, selected: bool, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    card_with_margin(ui, accent, selected, 20, add)
}

pub fn card_with_margin<R>(
    ui: &mut Ui,
    accent: Option<Color32>,
    selected: bool,
    margin: i8,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let stroke = match (selected, accent) {
        (true, Some(c)) => Stroke::new(1.5, c.gamma_multiply(0.85)),
        _ => Stroke::new(1.0, BORDER),
    };
    let resp = egui::Frame::new()
        .fill(SURFACE)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(margin))
        .show(ui, |ui| ui.vertical(add).inner);
    if let Some(c) = accent {
        let r = resp.response.rect;
        let line = Rect::from_min_size(pos2(r.left() + 18.0, r.top()), vec2(r.width() - 36.0, 2.0));
        ui.painter().rect_filled(line, 1.0, c);
    }
    resp
}

/// Título de sección con icono y subtítulo opcional.
pub fn section_header(ui: &mut Ui, icon: &str, title: &str, subtitle: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).font(theme::icon(18.0)).color(TEXT_2));
        ui.label(RichText::new(title).font(semibold(15.0)).color(TEXT));
    });
    if let Some(sub) = subtitle {
        ui.label(RichText::new(sub).size(12.5).color(TEXT_3));
    }
    ui.add_space(6.0);
}

/// Etiqueta pequeña en mayúsculas para separar grupos dentro de una tarjeta.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).font(medium(10.5)).color(TEXT_3));
}

/// Icono dentro de un cuadrado redondeado teñido del color del canal.
pub fn badge_icon(ui: &mut Ui, icon: &str, color: Color32, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((size * 0.28) as u8), color.gamma_multiply(0.16));
    p.text(rect.center(), Align2::CENTER_CENTER, icon, theme::icon(size * 0.56), color);
    resp
}

/// Píldora de estado: punto + texto sobre fondo teñido.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), medium(11.5), color);
    let size = vec2(galley.size().x + 26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), color.gamma_multiply(0.14));
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, color);
    p.galley(pos2(rect.left() + 19.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp
}

// ---------------------------------------------------------------------------------------------
// Botones
// ---------------------------------------------------------------------------------------------

/// Botón cuadrado de icono. `active` lo tiñe con `accent` (p. ej. silencio activado).
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, active: bool, accent: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    let bg = if active {
        accent.gamma_multiply(0.18)
    } else if hovered {
        SURFACE_3
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, CornerRadius::same(8), bg);
    let color = if active {
        accent
    } else if hovered {
        TEXT
    } else {
        TEXT_2
    };
    p.text(rect.center(), Align2::CENTER_CENTER, icon, theme::icon(17.0), color);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp.on_hover_text(tooltip)
}

/// Botón principal (relleno de acento).
pub fn primary_button(ui: &mut Ui, icon: &str, label: &str) -> Response {
    let text = RichText::new(format!("{icon}  {label}")).font(medium(13.5)).color(BG);
    ui.add(egui::Button::new(text).fill(ACCENT).corner_radius(CornerRadius::same(8)).min_size(vec2(0.0, 32.0)))
}

/// Botón secundario (superficie con borde).
pub fn secondary_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    let text = RichText::new(format!("{icon}  {label}")).font(medium(13.5));
    ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .fill(SURFACE_2)
            .stroke(Stroke::new(1.0, BORDER_STRONG))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 32.0)),
    )
}

// ---------------------------------------------------------------------------------------------
// Interruptores
// ---------------------------------------------------------------------------------------------

/// Interruptor animado (on/off).
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let p = ui.painter();
    let track = lerp_color(SURFACE_3, ACCENT, t);
    p.rect_filled(rect, CornerRadius::same(11), track);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    p.circle_filled(pos2(x, rect.center().y), 8.0, if *on { BG } else { TEXT_2 });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// Fila "título + descripción … interruptor".
pub fn toggle_row(ui: &mut Ui, on: &mut bool, title: &str, subtitle: &str) -> Response {
    let mut out = None;
    ui.horizontal(|ui| {
        // Deja sitio al interruptor para que el texto largo no se meta debajo.
        let text_w = (ui.available_width() - 56.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_max_width(text_w);
            ui.label(RichText::new(title).font(medium(13.5)).color(TEXT));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.0).color(TEXT_3));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            out = Some(toggle(ui, on));
        });
    });
    out.unwrap()
}

// ---------------------------------------------------------------------------------------------
// Fader y vúmetros
// ---------------------------------------------------------------------------------------------

/// Fader vertical de mesa de mezclas. Arrastrar o clic = mover; rueda = ajuste fino;
/// doble clic = volver al 100 %.
pub fn fader(ui: &mut Ui, value: &mut f32, color: Color32, height: f32, dimmed: bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, height), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(6.0, height - 18.0));

    if resp.double_clicked() {
        *value = 1.0;
        resp.mark_changed();
    } else if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            *value = ((track.bottom() - p.y) / track.height()).clamp(0.0, 1.0);
            resp.mark_changed();
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            *value = (*value + scroll * 0.0008).clamp(0.0, 1.0);
            resp.mark_changed();
        }
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    let p = ui.painter();
    // Marcas cada 25 %.
    for i in 0..=4 {
        let y = track.bottom() - track.height() * i as f32 / 4.0;
        let w = if i % 2 == 0 { 6.0 } else { 3.0 };
        p.line_segment([pos2(track.left() - 6.0 - w, y), pos2(track.left() - 6.0, y)], Stroke::new(1.0, BORDER_STRONG));
    }
    p.rect_filled(track, CornerRadius::same(3), SURFACE_3);
    let y = track.bottom() - track.height() * *value;
    let fill_color = if dimmed { TEXT_3 } else { color };
    p.rect_filled(Rect::from_min_max(pos2(track.left(), y), track.right_bottom()), CornerRadius::same(3), fill_color);

    // Tirador: rectángulo claro con sombra y una línea del color del canal.
    let thumb = Rect::from_center_size(pos2(track.center().x, y), vec2(30.0, 14.0));
    p.rect_filled(thumb.translate(vec2(0.0, 2.0)), CornerRadius::same(4), Color32::from_black_alpha(90));
    let thumb_fill = if resp.hovered() || resp.dragged() { Color32::WHITE } else { rgb_e9() };
    p.rect_filled(thumb, CornerRadius::same(4), thumb_fill);
    p.line_segment(
        [pos2(thumb.left() + 7.0, thumb.center().y), pos2(thumb.right() - 7.0, thumb.center().y)],
        Stroke::new(2.0, fill_color),
    );
    resp
}

fn rgb_e9() -> Color32 {
    Color32::from_rgb(0xdd, 0xe2, 0xea)
}

fn led_color(frac: f32) -> Color32 {
    if frac > 0.9 {
        DANGER
    } else if frac > 0.72 {
        WARN
    } else {
        OK
    }
}

/// Vúmetro vertical de LEDs (verde → ámbar → rojo). `level` en 0..1 (escala dB ya aplicada).
pub fn vmeter(ui: &mut Ui, level: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, height), Sense::hover());
    let rect = rect.shrink2(vec2(0.0, 9.0));
    let p = ui.painter();
    let seg_h = 3.0;
    let gap = 2.0;
    let n = ((rect.height() + gap) / (seg_h + gap)) as usize;
    for i in 0..n {
        let frac = (i as f32 + 0.5) / n as f32;
        let y = rect.bottom() - (i as f32) * (seg_h + gap) - seg_h;
        let seg = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), seg_h));
        let c = led_color(frac);
        let lit = frac <= level;
        p.rect_filled(seg, CornerRadius::same(1), if lit { c } else { c.gamma_multiply(0.10) });
    }
}

/// Vúmetro horizontal de LEDs. Con `color`, una sola tinta en lugar de verde/ámbar/rojo.
pub fn hmeter(ui: &mut Ui, level: f32, width: f32, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let p = ui.painter();
    let seg_w = 4.0;
    let gap = 2.0;
    let n = ((rect.width() + gap) / (seg_w + gap)) as usize;
    for i in 0..n {
        let frac = (i as f32 + 0.5) / n as f32;
        let x = rect.left() + i as f32 * (seg_w + gap);
        let seg = Rect::from_min_size(pos2(x, rect.top()), vec2(seg_w, rect.height()));
        let c = color.unwrap_or_else(|| led_color(frac));
        p.rect_filled(seg, CornerRadius::same(1), if frac <= level { c } else { c.gamma_multiply(0.10) });
    }
}

// ---------------------------------------------------------------------------------------------
// ChatMix
// ---------------------------------------------------------------------------------------------

/// Slider bipolar con degradado del color de Juego al de Chat y marca central.
pub fn chatmix_slider(ui: &mut Ui, value: &mut f32, game: Color32, chat: Color32, enabled: bool, width: f32) -> Response {
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 28.0), sense);
    let track = Rect::from_center_size(rect.center(), vec2(width - 20.0, 6.0));
    if enabled {
        if resp.double_clicked() {
            *value = 0.0;
            resp.mark_changed();
        } else if resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
                let mut v = t * 2.0 - 1.0;
                if v.abs() < 0.04 {
                    v = 0.0; // imán en el centro
                }
                *value = v;
                resp.mark_changed();
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    }

    let p = ui.painter();
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.35) };
    // Degradado con una malla de 2 colores.
    let mut mesh = egui::Mesh::default();
    let (a, b) = (dim(game), dim(chat));
    let tl = track.left_top();
    let br = track.right_bottom();
    mesh.colored_vertex(tl, a);
    mesh.colored_vertex(pos2(br.x, tl.y), b);
    mesh.colored_vertex(br, b);
    mesh.colored_vertex(pos2(tl.x, br.y), a);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
    p.line_segment([pos2(track.center().x, rect.top() + 4.0), pos2(track.center().x, rect.bottom() - 4.0)], Stroke::new(1.0, TEXT_3));

    let x = track.left() + (*value * 0.5 + 0.5) * track.width();
    let knob = pos2(x, track.center().y);
    p.circle_filled(knob + vec2(0.0, 1.5), 9.0, Color32::from_black_alpha(90));
    p.circle_filled(knob, 9.0, if enabled { Color32::WHITE } else { TEXT_3 });
    p.circle_filled(knob, 3.0, lerp_color(a, b, *value * 0.5 + 0.5));
    resp
}

// ---------------------------------------------------------------------------------------------
// Varios
// ---------------------------------------------------------------------------------------------

/// Desplegable de dispositivo con icono delante del nombre.
pub fn device_combo(
    ui: &mut Ui,
    id: &str,
    icon: &str,
    value: &mut Option<String>,
    devices: &[DeviceInfo],
    default_label: &str,
    width: f32,
) {
    let name = match value {
        None => default_label.to_string(),
        Some(id) => devices.iter().find(|d| &d.id == id).map(|d| d.name.clone()).unwrap_or(tr!("(no disponible)", "(not available)").into()),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(RichText::new(format!("{icon}  {name}")).color(TEXT))
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, default_label);
            for d in devices {
                ui.selectable_value(value, Some(d.id.clone()), &d.name);
            }
        });
}

/// Mensaje con icono (info / aviso / error / ok).
pub fn notice(ui: &mut Ui, icon: &str, color: Color32, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).font(theme::icon(15.0)).color(color));
                ui.label(RichText::new(text).size(12.5).color(TEXT));
            });
        });
}

/// Marco del logotipo: anillo con tres barras (forma de onda). Dibujado, sin imágenes.
pub fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, size / 2.0, ACCENT.gamma_multiply(0.14));
    p.circle_stroke(c, size / 2.0 - 1.5, Stroke::new(2.0, ACCENT));
    let bars = [0.35, 0.65, 0.45];
    for (i, h) in bars.iter().enumerate() {
        let x = c.x + (i as f32 - 1.0) * size * 0.17;
        let half = size * h / 2.0;
        p.line_segment([pos2(x, c.y - half), pos2(x, c.y + half)], Stroke::new(2.5, ACCENT));
    }
}

/// Texto que se recorta con "…" si no cabe.
pub fn truncated(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(egui::Label::new(text).truncate())
}

/// Punto de actividad (encendido si `on`).
pub fn activity_dot(ui: &mut Ui, on: bool, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, if on { color } else { BORDER_STRONG });
}

/// Borde de un rect (útil para zonas de soltar).
pub fn outline(ui: &Ui, rect: Rect, color: Color32) {
    ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, color), StrokeKind::Inside);
}

pub fn text_width(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.painter().layout_no_wrap(text.to_owned(), font, TEXT).size().x
}
