//! Thème sombre de Mudra.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use mudra_gestures::EngineState;

pub const BACKGROUND: Color32 = Color32::from_rgb(0x0D, 0x0F, 0x14);
pub const PANEL: Color32 = Color32::from_rgb(0x13, 0x17, 0x1E);
pub const CARD: Color32 = Color32::from_rgb(0x1A, 0x1F, 0x29);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x22, 0x29, 0x35);
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x31, 0x3D);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xEA, 0xF0);
pub const MUTED: Color32 = Color32::from_rgb(0x8B, 0x95, 0xA5);
pub const ACCENT: Color32 = Color32::from_rgb(0x5B, 0x9D, 0xFF);
pub const GREEN: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
pub const AMBER: Color32 = Color32::from_rgb(0xF5, 0xB7, 0x4E);
pub const RED: Color32 = Color32::from_rgb(0xF2, 0x61, 0x6A);
pub const GRAY: Color32 = Color32::from_rgb(0x6B, 0x74, 0x80);

/// Couleur associée à un état du moteur, la même que celle de l'icône de notification.
pub fn state_color(state: EngineState) -> Color32 {
    match state {
        EngineState::Paused => GRAY,
        EngineState::Locked => MUTED,
        EngineState::Armed => ACCENT,
        EngineState::Continuous => GREEN,
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BACKGROUND;
    visuals.faint_bg_color = CARD;
    visuals.code_bg_color = CARD;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = RED;

    let radius = CornerRadius::same(6);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = radius;
    widgets.inactive.bg_fill = CARD;
    widgets.inactive.weak_bg_fill = CARD;
    widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.inactive.corner_radius = radius;
    widgets.hovered.bg_fill = CARD_HOVER;
    widgets.hovered.weak_bg_fill = CARD_HOVER;
    widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    widgets.hovered.corner_radius = radius;
    widgets.active.bg_fill = ACCENT.gamma_multiply(0.45);
    widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.45);
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    widgets.active.corner_radius = radius;
    widgets.open.bg_fill = CARD_HOVER;
    widgets.open.corner_radius = radius;

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(17.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// Nom français d'un geste connu ; l'identifiant brut sinon.
pub fn gesture_name(id: &str) -> &str {
    match id {
        "none" => "aucun",
        "open_palm" => "Paume ouverte",
        "fist" => "Poing",
        "thumbs_up" => "Pouce levé",
        "thumbs_down" => "Pouce baissé",
        "v_sign" => "V",
        "rock" => "Rock",
        "three_fingers" => "Trois doigts",
        "pinch" => "Pincement",
        "pointing" => "Index tendu",
        "swipe_left" => "Balayage gauche",
        "swipe_right" => "Balayage droite",
        "swipe_up" => "Balayage haut",
        "swipe_down" => "Balayage bas",
        "rotate_cw" => "Rotation horaire",
        "rotate_ccw" => "Rotation anti-horaire",
        other => other,
    }
}
