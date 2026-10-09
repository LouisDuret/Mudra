//! Petit overlay d'état (EF-37) : fenêtre sans bordure, au premier plan,
//! traversable par la souris, qui ne prend jamais le focus. Elle affiche l'état
//! du moteur et le dernier geste reconnu ; elle sert aussi de notification.

use egui::{Align, Color32, Layout, RichText, Sense, Ui, Vec2};

use super::theme::{MUTED, TEXT};

pub const WIDTH: f64 = 400.0;
pub const HEIGHT: f64 = 52.0;

pub struct OverlayModel<'a> {
    pub color: Color32,
    pub title: &'a str,
    pub detail: &'a str,
}

pub fn draw(ui: &mut Ui, model: &OverlayModel<'_>) {
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(super::theme::PANEL)
                .inner_margin(egui::Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 6.0, model.color);
                ui.painter().circle_stroke(
                    dot.center(),
                    9.0,
                    egui::Stroke::new(1.5, model.color.gamma_multiply(0.35)),
                );
                ui.add_space(4.0);
                ui.label(RichText::new(model.title).size(15.0).strong().color(TEXT));
                if !model.detail.is_empty() {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(RichText::new(model.detail).size(13.5).color(MUTED))
                            .truncate(),
                    );
                }
            });
        });
}
