//! Apprentissage d'une pose personnalisée dans l'onglet « Gestes » : un nom,
//! quelques prises guidées avec la caméra en vignette, puis la pose rejoint la
//! liste des gestes.

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Ui, Vec2};
use mudra_config::KnownGestures;
use mudra_gestures::EngineState;

use super::preview::{CameraView, RecordForm, UiAction, camera_view};
use super::theme::{self, ACCENT, AMBER, BORDER, GREEN, MUTED, RED};
use crate::custom::{TAKE_SECONDS, TAKES_FOR_NEW_POSE, unique_label};

const THUMBNAIL: Vec2 = Vec2::new(320.0, 180.0);

/// Consigne de chaque prise : varier un peu d'une prise à l'autre apprend à
/// la pose sa tolérance.
const INSTRUCTIONS: [&str; 3] = [
    "Forme la pose face à la caméra et tiens-la.",
    "Baisse la main, reforme la pose et incline-la légèrement.",
    "Encore une fois, un peu plus près ou plus loin de la caméra.",
];

/// Séance d'apprentissage en cours.
pub struct Trainer {
    pub name: String,
    /// Pose existante à compléter (label) ; `None` pour une nouvelle pose.
    pub existing: Option<String>,
    /// Identifiant de la pose, figé à la première prise.
    pub label: Option<String>,
    /// Prises enregistrées pendant cette séance.
    pub takes: Vec<PathBuf>,
    pub error: Option<String>,
}

impl Trainer {
    pub fn new_pose() -> Self {
        Self {
            name: String::new(),
            existing: None,
            label: None,
            takes: Vec::new(),
            error: None,
        }
    }

    pub fn more_examples(label: &str, name: &str) -> Self {
        Self {
            name: name.to_string(),
            existing: Some(label.to_string()),
            label: Some(label.to_string()),
            takes: Vec::new(),
            error: None,
        }
    }

    fn takes_needed(&self) -> usize {
        if self.existing.is_some() {
            1
        } else {
            TAKES_FOR_NEW_POSE
        }
    }
}

/// Dessine la séance ; renvoie `false` quand l'utilisateur la ferme.
pub(super) fn draw(
    ui: &mut Ui,
    trainer: &mut Trainer,
    camera: &CameraView<'_>,
    record: &RecordForm,
    known: &KnownGestures,
    actions: &mut Vec<UiAction>,
) -> bool {
    let mut open = true;
    let recording = record.active;
    let paused = camera.status.state == EngineState::Paused || camera.pipeline_error.is_some();
    let done = trainer.takes.len();
    let needed = trainer.takes_needed();

    // Tant qu'aucune prise n'est faite, l'identifiant suit le nom saisi.
    let label = trainer.label.clone().or_else(|| {
        let taken: Vec<&str> = known
            .static_
            .iter()
            .chain(&known.dynamic)
            .map(String::as_str)
            .collect();
        unique_label(&trainer.name, &taken)
    });

    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.6)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let title = match &trainer.existing {
                    Some(_) => format!("Ajouter des exemples à « {} »", trainer.name),
                    None => "Nouvelle pose".to_string(),
                };
                ui.label(RichText::new(title).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Annuler").clicked() {
                        if recording {
                            actions.push(UiAction::CancelRecording);
                        }
                        if !trainer.takes.is_empty() {
                            actions.push(UiAction::DiscardTakes(std::mem::take(
                                &mut trainer.takes,
                            )));
                        }
                        open = false;
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(THUMBNAIL, Layout::top_down(Align::Min), |ui| {
                    ui.set_min_size(THUMBNAIL);
                    camera_view(ui, camera);
                });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    if trainer.existing.is_none() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Nom").color(MUTED));
                            ui.add_enabled(
                                done == 0 && !recording,
                                egui::TextEdit::singleline(&mut trainer.name)
                                    .desired_width(260.0)
                                    .hint_text("par exemple : Téléphone"),
                            );
                        });
                        let hint = match (&label, trainer.name.trim().is_empty()) {
                            (_, true) => "Donne un nom à la pose pour commencer.".to_string(),
                            (Some(label), _) => format!("Identifiant dans la configuration : {label}"),
                            (None, _) => "Le nom doit contenir au moins une lettre.".to_string(),
                        };
                        ui.label(RichText::new(hint).small().color(MUTED));
                        ui.add_space(4.0);
                    }

                    // Avancement : une pastille par prise.
                    ui.horizontal(|ui| {
                        let shown = needed.max(done);
                        for i in 0..shown {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                            let color = if i < done { GREEN } else { BORDER };
                            ui.painter().circle_filled(rect.center(), 5.0, color);
                        }
                        let text = if done < needed {
                            format!("Prise {} sur {needed}", done + 1)
                        } else if done == 1 {
                            "1 prise enregistrée".to_string()
                        } else {
                            format!("{done} prises enregistrées")
                        };
                        ui.label(RichText::new(text).strong());
                    });
                    let instruction = if done < needed || trainer.existing.is_some() {
                        INSTRUCTIONS[done.min(INSTRUCTIONS.len() - 1)]
                    } else {
                        "C'est assez pour apprendre la pose. Une prise de plus la rend plus tolérante."
                    };
                    ui.label(instruction);
                    ui.add_space(4.0);

                    if recording {
                        if let Some((text, _)) = &record.status {
                            ui.label(RichText::new(text).color(ACCENT));
                        }
                        ui.add(
                            egui::ProgressBar::new(record.progress.unwrap_or(0.0))
                                .desired_width(260.0)
                                .desired_height(8.0),
                        );
                        if ui.button("Arrêter cette prise").clicked() {
                            actions.push(UiAction::CancelRecording);
                        }
                        ui.ctx().request_repaint();
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            let enough = done >= needed;
                            let start = if enough {
                                "⏺  Prise supplémentaire".to_string()
                            } else {
                                format!("⏺  Démarrer la prise {}", done + 1)
                            };
                            if ui
                                .add_enabled(label.is_some() && !paused, egui::Button::new(start))
                                .on_hover_text(format!(
                                    "Compte à rebours de 3 s, puis {TAKE_SECONDS:.0} s de capture."
                                ))
                                .clicked()
                                && let Some(label) = &label
                            {
                                trainer.error = None;
                                trainer.label = Some(label.clone());
                                actions.push(UiAction::TrainTake {
                                    label: label.clone(),
                                });
                            }
                            if done > 0 && ui.button("Refaire la dernière prise").clicked() {
                                trainer.error = None;
                                if let Some(last) = trainer.takes.pop() {
                                    actions.push(UiAction::DiscardTakes(vec![last]));
                                }
                            }
                            if enough && let Some(label) = &label {
                                let finish = if trainer.existing.is_some() {
                                    "Mettre à jour la pose"
                                } else {
                                    "Créer la pose"
                                };
                                let button = egui::Button::new(RichText::new(finish).strong())
                                    .fill(ACCENT.gamma_multiply(0.35));
                                if ui.add(button).clicked() {
                                    trainer.error = None;
                                    actions.push(UiAction::TrainFinish {
                                        label: label.clone(),
                                        name: trainer
                                            .existing
                                            .is_none()
                                            .then(|| trainer.name.trim().to_string()),
                                    });
                                }
                            }
                        });
                        if paused {
                            ui.label(
                                RichText::new("Mudra est en pause : reprends pour enregistrer.")
                                    .small()
                                    .color(AMBER),
                            );
                        } else if camera.status.hands.is_empty() {
                            ui.label(
                                RichText::new(
                                    "Aucune main détectée : place-la dans le cadre avant de démarrer.",
                                )
                                .small()
                                .color(AMBER),
                            );
                        }
                    }
                    if let Some(error) = &trainer.error {
                        ui.label(RichText::new(error).color(RED));
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Seuls les points-clés de la main sont enregistrés, jamais d'image. \
                             Aucune action ne se déclenche pendant une prise.",
                        )
                        .small()
                        .color(MUTED),
                    );
                });
            });
        });
    open
}
