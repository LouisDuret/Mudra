//! Apprentissage d'une pose personnalisée dans l'onglet « Gestes » : un nom,
//! quelques prises guidées avec la caméra en vignette, puis la pose rejoint la
//! liste des gestes. La même séance ajoute des exemples à une pose existante
//! ou à un geste de base.

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
const TWO_HAND_INSTRUCTIONS: [&str; 3] = [
    "Forme la pose des deux mains face à la caméra et tiens-la.",
    "Baisse les mains, reforme la pose et incline-la légèrement.",
    "Encore une fois, un peu plus près ou plus loin de la caméra. Si l'écart entre \
     les mains ne doit pas compter, change-le.",
];

/// Séance d'apprentissage en cours.
pub struct Trainer {
    pub name: String,
    /// La pose ne se fait qu'à deux mains ; figé à la première prise.
    pub two_hands: bool,
    /// Pose ou geste de base à compléter (label) ; `None` pour une nouvelle pose.
    pub existing: Option<String>,
    /// Les exemples complètent un geste de base, pas une pose perso.
    pub base: bool,
    /// Nombre de prises demandées avant de pouvoir terminer.
    needed: usize,
    /// La séance vient de s'ouvrir : la page défile jusqu'à elle.
    fresh: bool,
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
            two_hands: false,
            existing: None,
            base: false,
            needed: TAKES_FOR_NEW_POSE,
            fresh: true,
            label: None,
            takes: Vec::new(),
            error: None,
        }
    }

    pub fn more_examples(label: &str, name: &str, two_hands: bool) -> Self {
        Self {
            name: name.to_string(),
            two_hands,
            existing: Some(label.to_string()),
            base: false,
            needed: 1,
            fresh: true,
            label: Some(label.to_string()),
            takes: Vec::new(),
            error: None,
        }
    }

    /// Exemples pour le geste de base `label`. Les premiers demandent autant
    /// de prises qu'une nouvelle pose : c'est leur écart qui règle la tolérance.
    pub fn base_examples(label: &str, name: &str, first: bool) -> Self {
        Self {
            base: true,
            needed: if first { TAKES_FOR_NEW_POSE } else { 1 },
            ..Self::more_examples(label, name, false)
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
    let needed = trainer.needed;
    if std::mem::take(&mut trainer.fresh) {
        ui.scroll_to_cursor(Some(Align::Min));
    }

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
                        let two_tracked = camera.config.inference.max_hands >= 2;
                        // La case a par défaut la couleur et l'arrondi des
                        // cartes : on en fait un carré qui se détache du fond.
                        ui.scope(|ui| {
                            let widgets = &mut ui.visuals_mut().widgets;
                            for state in [
                                &mut widgets.inactive,
                                &mut widgets.hovered,
                                &mut widgets.active,
                            ] {
                                state.bg_fill = BORDER;
                                state.corner_radius = egui::CornerRadius::same(3);
                            }
                            ui.add_enabled(
                                done == 0 && !recording && two_tracked,
                                egui::Checkbox::new(&mut trainer.two_hands, "Pose à deux mains"),
                            )
                            .on_hover_text(
                                "La pose n'existe que si les deux mains la forment ensemble : \
                                 leur forme et leur position l'une par rapport à l'autre comptent.",
                            )
                            .on_disabled_hover_text(if two_tracked {
                                "Ne se change plus une fois la première prise faite."
                            } else {
                                "Mudra ne suit qu'une main (inference.max_hands = 1)."
                            });
                        });
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
                    let instructions = if trainer.two_hands {
                        TWO_HAND_INSTRUCTIONS
                    } else {
                        INSTRUCTIONS
                    };
                    let instruction = if done < needed || needed == 1 {
                        instructions[done.min(instructions.len() - 1)]
                    } else if trainer.base {
                        "C'est assez pour compléter le geste. Une prise de plus le rend plus tolérant."
                    } else {
                        "C'est assez pour apprendre la pose. Une prise de plus la rend plus tolérante."
                    };
                    ui.label(instruction);
                    if trainer.base {
                        ui.label(
                            RichText::new(
                                "Fais le geste à ta façon, surtout là où Mudra le reconnaît mal : \
                                 tes exemples s'ajoutent à ce qu'il reconnaît déjà.",
                            )
                            .small()
                            .color(MUTED),
                        );
                    }
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
                                let finish = if trainer.base {
                                    "Enregistrer mes exemples"
                                } else if trainer.existing.is_some() {
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
                                        two_hands: trainer.two_hands,
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
                        } else if trainer.two_hands && camera.status.hands.len() < 2 {
                            ui.label(
                                RichText::new(
                                    "Une seule main détectée : place les deux dans le cadre, sans \
                                     que l'une cache l'autre.",
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
                            "Seuls les points-clés des mains sont enregistrés, jamais d'image. \
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
