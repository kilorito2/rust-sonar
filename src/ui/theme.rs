//! Identidad visual de Rust Sonar: paleta grafito con acento turquesa, tipografía Inter e iconos
//! Phosphor. Todo el estilo de egui se configura aquí para que las vistas no repitan colores.

use super::icons;
use crate::config::{Channel, EqTarget};
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, vec2};
use std::sync::Arc;

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const BG: Color32 = rgb(0x0d, 0x0f, 0x13);
pub const SIDEBAR: Color32 = rgb(0x11, 0x14, 0x19);
pub const SURFACE: Color32 = rgb(0x16, 0x1a, 0x21);
pub const SURFACE_2: Color32 = rgb(0x1c, 0x21, 0x2a);
pub const SURFACE_3: Color32 = rgb(0x25, 0x2b, 0x36);
pub const BORDER: Color32 = rgb(0x24, 0x2a, 0x34);
pub const BORDER_STRONG: Color32 = rgb(0x34, 0x3c, 0x4a);
pub const TEXT: Color32 = rgb(0xe6, 0xe9, 0xef);
pub const TEXT_2: Color32 = rgb(0x9a, 0xa3, 0xb2);
pub const TEXT_3: Color32 = rgb(0x67, 0x70, 0x80);
pub const ACCENT: Color32 = rgb(0x2d, 0xd4, 0xbf);
pub const DANGER: Color32 = rgb(0xf8, 0x71, 0x71);
pub const WARN: Color32 = rgb(0xfb, 0xbf, 0x24);
pub const OK: Color32 = rgb(0x4a, 0xde, 0x80);
pub const MASTER: Color32 = rgb(0xe5, 0xe7, 0xeb);
pub const MIC: Color32 = rgb(0xf4, 0x72, 0xb6);

pub fn channel_color(ch: Channel) -> Color32 {
    match ch {
        Channel::Game => rgb(0x84, 0xcc, 0x16),
        Channel::Chat => rgb(0x38, 0xbd, 0xf8),
        Channel::Media => rgb(0xfb, 0xbf, 0x24),
        Channel::Aux => rgb(0xa7, 0x8b, 0xfa),
    }
}

pub fn channel_icon(ch: Channel) -> &'static str {
    match ch {
        Channel::Game => icons::GAME,
        Channel::Chat => icons::CHAT,
        Channel::Media => icons::MEDIA,
        Channel::Aux => icons::AUX,
    }
}

pub fn target_color(t: EqTarget) -> Color32 {
    match t {
        EqTarget::Channel(ch) => channel_color(ch),
        EqTarget::Mic => MIC,
    }
}

pub fn target_icon(t: EqTarget) -> &'static str {
    match t {
        EqTarget::Channel(ch) => channel_icon(ch),
        EqTarget::Mic => icons::MICROPHONE,
    }
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

/// Registra fuentes y estilo. Se llama una vez al crear la ventana.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add("inter", include_bytes!("../../assets/fonts/latin-400-normal.ttf"));
    add("inter-medium", include_bytes!("../../assets/fonts/latin-500-normal.ttf"));
    add("inter-semibold", include_bytes!("../../assets/fonts/latin-600-normal.ttf"));
    add("phosphor", include_bytes!("../../assets/fonts/Phosphor.ttf"));

    // Inter primero; Phosphor como respaldo (iconos en línea con el texto); después las de egui
    // para símbolos que Inter-latin no tiene (→, …).
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |main: &str| {
        let mut v = vec![main.to_string(), "phosphor".to_string()];
        v.extend(defaults.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("inter"));
    fonts.families.insert(FontFamily::Name("medium".into()), family("inter-medium"));
    fonts.families.insert(FontFamily::Name("semibold".into()), family("inter-semibold"));
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["phosphor".into()]);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(22.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();

        let sp = &mut style.spacing;
        sp.item_spacing = vec2(10.0, 10.0);
        sp.button_padding = vec2(14.0, 7.0);
        sp.interact_size.y = 32.0;
        sp.slider_width = 180.0;
        sp.combo_width = 200.0;
        sp.icon_width = 16.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE_2;
        v.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = BG;
        v.faint_bg_color = SURFACE_2;
        v.code_bg_color = BG;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.28);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.slider_trailing_fill = true;

        let radius = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
        w.noninteractive.corner_radius = radius;

        w.inactive.weak_bg_fill = SURFACE_2;
        w.inactive.bg_fill = SURFACE_3;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = radius;

        w.hovered.weak_bg_fill = SURFACE_3;
        w.hovered.bg_fill = BORDER_STRONG;
        w.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.weak_bg_fill = rgb(0x2b, 0x32, 0x40);
        w.active.bg_fill = ACCENT;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, TEXT);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.weak_bg_fill = SURFACE_3;
        w.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        w.open.corner_radius = radius;
    });
}
