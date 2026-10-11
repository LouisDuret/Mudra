//! Petit overlay d'état (EF-37) : fenêtre sans bordure, au premier plan,
//! traversable par la souris, qui ne prend jamais le focus. Elle affiche l'état
//! du moteur et le dernier geste reconnu ; elle sert aussi de notification.

use egui::{Align, Color32, Layout, Sense, Ui, Vec2};

use super::neu;
use super::theme::{Weight, palette};

pub const WIDTH: f64 = 400.0;
pub const HEIGHT: f64 = 52.0;

pub struct OverlayModel<'a> {
    pub color: Color32,
    pub title: &'a str,
    pub detail: &'a str,
}

pub fn draw(ui: &mut Ui, model: &OverlayModel<'_>) {
    let p = palette();
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(p.bg)
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
                ui.label(neu::text(model.title, 15.0, Weight::Bold, p.text));
                if !model.detail.is_empty() {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(neu::text(model.detail, 13.0, Weight::Regular, p.muted))
                            .truncate(),
                    );
                }
            });
        });
}
