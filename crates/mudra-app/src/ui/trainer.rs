//! Apprentissage d'une pose ou d'un mouvement personnalisé dans l'onglet
//! « Gestes » : un nom, quelques prises guidées, puis le geste rejoint la
//! liste. La même séance ajoute des exemples à un geste déjà appris ou à un
//! geste de base. La vignette de la colonne latérale montre les mains pendant
//! les prises.

use std::path::PathBuf;

use egui::{Align, Layout, Ui, Vec2};
use mudra_config::{Config, KnownGestures};
use mudra_gestures::EngineState;

use super::icons;
use super::neu::{self, Button, Card};
use super::preview::{RecordForm, UiAction};
use super::theme::{Weight, palette};
use crate::custom::{MOVE_TAKE_SECONDS, TAKE_SECONDS, TAKES_FOR_NEW_POSE, unique_label};
use crate::pipeline::Status;

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

/// Consigne de chaque prise d'un mouvement : le geste est fait une fois par prise.
const MOVE_INSTRUCTIONS: [&str; 3] = [
    "Au signal, fais le mouvement une fois, franchement, puis garde la main immobile.",
    "Encore une fois, de la même façon.",
    "Une dernière fois, un peu plus vite ou un peu plus lentement.",
];

/// Séance d'apprentissage en cours.
pub struct Trainer {
    pub name: String,
    /// La séance apprend un mouvement, pas une pose.
    pub movement: bool,
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
            movement: false,
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
            movement: false,
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

    /// Nouveau mouvement : autant de prises qu'une nouvelle pose.
    pub fn new_movement() -> Self {
        Self {
            movement: true,
            ..Self::new_pose()
        }
    }

    /// Une prise de plus pour le mouvement perso `label`.
    pub fn more_movement_examples(label: &str, name: &str) -> Self {
        Self {
            movement: true,
            ..Self::more_examples(label, name, false)
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

/// Ce que la séance lit de l'application.
pub(super) struct Context<'a> {
    pub status: &'a Status,
    pub config: &'a Config,
    pub pipeline_error: Option<&'a str>,
    pub record: &'a RecordForm,
    pub known: &'a KnownGestures,
}

/// Dessine la séance ; renvoie `false` quand l'utilisateur la ferme.
pub(super) fn draw(
    ui: &mut Ui,
    trainer: &mut Trainer,
    context: &Context<'_>,
    actions: &mut Vec<UiAction>,
) -> bool {
    let p = palette();
    let record = context.record;
    let mut open = true;
    let recording = record.active;
    let paused = context.status.state == EngineState::Paused || context.pipeline_error.is_some();
    let done = trainer.takes.len();
    let needed = trainer.needed;
    if std::mem::take(&mut trainer.fresh) {
        ui.scroll_to_cursor(Some(Align::Min));
    }

    // Tant qu'aucune prise n'est faite, l'identifiant suit le nom saisi.
    let label = trainer.label.clone().or_else(|| {
        let taken: Vec<&str> = context
            .known
            .static_
            .iter()
            .chain(&context.known.dynamic)
            .map(String::as_str)
            .collect();
        unique_label(&trainer.name, &taken)
    });
    let note = |ui: &mut Ui, text: &str, color: egui::Color32| {
        ui.label(neu::text(text, 11.5, Weight::Regular, color));
    };

    Card::LARGE.gap(12.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            neu::accent_dot(ui, 8.0);
            let title = match &trainer.existing {
                Some(_) => format!("Ajouter des exemples à « {} »", trainer.name),
                None if trainer.movement => "Nouveau mouvement".to_string(),
                None => "Nouvelle pose".to_string(),
            };
            ui.label(neu::text(title, 16.0, Weight::Bold, p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(Button::new("Annuler").icon(&icons::CROSS, 10.0))
                    .clicked()
                {
                    if recording {
                        actions.push(UiAction::CancelRecording);
                    }
                    if !trainer.takes.is_empty() {
                        actions.push(UiAction::DiscardTakes(std::mem::take(&mut trainer.takes)));
                    }
                    open = false;
                }
            });
        });

        if trainer.existing.is_none() {
            ui.horizontal(|ui| {
                ui.set_min_height(34.0);
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(neu::text("Nom", 12.5, Weight::Regular, p.muted));
                let (example, first) = if trainer.movement {
                    (
                        "par exemple : Cercle",
                        "Donne un nom au mouvement pour commencer.",
                    )
                } else {
                    (
                        "par exemple : Téléphone",
                        "Donne un nom à la pose pour commencer.",
                    )
                };
                ui.add_enabled_ui(done == 0 && !recording, |ui| {
                    neu::text_field(ui, &mut trainer.name, 260.0, example);
                });
                let hint = match (&label, trainer.name.trim().is_empty()) {
                    (_, true) => first.to_string(),
                    (Some(label), _) => format!("Identifiant dans la configuration : {label}"),
                    (None, _) => "Le nom doit contenir au moins une lettre.".to_string(),
                };
                ui.label(neu::text(hint, 11.5, Weight::Regular, p.muted));
            });
            let two_tracked = context.config.inference.max_hands >= 2;
            // Un mouvement se fait d'une main.
            if !trainer.movement {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.add_enabled_ui(done == 0 && !recording && two_tracked, |ui| {
                        neu::switch(ui, &mut trainer.two_hands, Vec2::new(40.0, 22.0))
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
                    ui.label(neu::text(
                        "Pose à deux mains",
                        12.5,
                        Weight::Semibold,
                        p.text,
                    ));
                });
            }
        }

        // Avancement : une pastille par prise.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let shown = needed.max(done);
            for i in 0..shown {
                if i < done {
                    neu::accent_dot(ui, 10.0);
                } else {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                    ui.painter().add(neu::inset(rect, 5.0, 1.5));
                }
            }
            ui.add_space(4.0);
            let text = if done < needed {
                format!("Prise {} sur {needed}", done + 1)
            } else if done == 1 {
                "1 prise enregistrée".to_string()
            } else {
                format!("{done} prises enregistrées")
            };
            ui.label(neu::text(text, 13.0, Weight::Bold, p.text));
        });
        let instructions = if trainer.movement {
            MOVE_INSTRUCTIONS
        } else if trainer.two_hands {
            TWO_HAND_INSTRUCTIONS
        } else {
            INSTRUCTIONS
        };
        let instruction = if done < needed || needed == 1 {
            instructions[done.min(instructions.len() - 1)]
        } else if trainer.movement {
            "C'est assez pour apprendre le mouvement. Une prise de plus le rend plus tolérant."
        } else if trainer.base {
            "C'est assez pour compléter le geste. Une prise de plus le rend plus tolérant."
        } else {
            "C'est assez pour apprendre la pose. Une prise de plus la rend plus tolérante."
        };
        ui.label(neu::text(instruction, 13.0, Weight::Regular, p.text));
        if trainer.base {
            note(
                ui,
                "Fais le geste à ta façon, surtout là où Mudra le reconnaît mal : tes exemples \
                 s'ajoutent à ce qu'il reconnaît déjà.",
                p.muted,
            );
        }
        if trainer.movement {
            note(
                ui,
                "Un mouvement est bref (moins d'une seconde) et ample (plus d'une paume). La \
                 forme de la main compte : fait main fermée, il ne se confond pas avec les \
                 balayages, qui se font paume ouverte.",
                p.muted,
            );
        }

        if recording {
            if let Some((text, _)) = &record.status {
                ui.label(neu::text(text, 12.5, Weight::Semibold, p.accent));
            }
            ui.scope(|ui| {
                ui.set_max_width(320.0);
                neu::bar(ui, record.progress.unwrap_or(0.0), 10.0, None, None);
            });
            if ui.add(Button::new("Arrêter cette prise")).clicked() {
                actions.push(UiAction::CancelRecording);
            }
            ui.ctx().request_repaint();
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                let enough = done >= needed;
                let start = if enough {
                    "Prise supplémentaire".to_string()
                } else {
                    format!("Démarrer la prise {}", done + 1)
                };
                let start = Button::new(&start).icon(&icons::RECORD, 10.0);
                // Tant qu'il manque des prises, démarrer est le geste attendu.
                let start = if enough {
                    start.icon_color(p.danger)
                } else {
                    start.primary()
                };
                if ui
                    .add_enabled(label.is_some() && !paused, start)
                    .on_hover_text(if trainer.movement {
                        format!(
                            "Compte à rebours de 2 s, puis {MOVE_TAKE_SECONDS:.0} s pour faire \
                             le mouvement."
                        )
                    } else {
                        format!("Compte à rebours de 3 s, puis {TAKE_SECONDS:.0} s de capture.")
                    })
                    .clicked()
                    && let Some(label) = &label
                {
                    trainer.error = None;
                    trainer.label = Some(label.clone());
                    actions.push(UiAction::TrainTake {
                        label: label.clone(),
                        movement: trainer.movement,
                    });
                }
                if done > 0 && ui.add(Button::new("Refaire la dernière prise")).clicked() {
                    trainer.error = None;
                    if let Some(last) = trainer.takes.pop() {
                        actions.push(UiAction::DiscardTakes(vec![last]));
                    }
                }
                if enough && let Some(label) = &label {
                    let finish = match (trainer.movement, trainer.existing.is_some()) {
                        _ if trainer.base => "Enregistrer mes exemples",
                        (true, true) => "Mettre à jour le mouvement",
                        (true, false) => "Créer le mouvement",
                        (false, true) => "Mettre à jour la pose",
                        (false, false) => "Créer la pose",
                    };
                    if ui.add(Button::new(finish).primary()).clicked() {
                        trainer.error = None;
                        actions.push(UiAction::TrainFinish {
                            label: label.clone(),
                            name: trainer
                                .existing
                                .is_none()
                                .then(|| trainer.name.trim().to_string()),
                            two_hands: trainer.two_hands,
                            movement: trainer.movement,
                        });
                    }
                }
            });
            if paused {
                note(
                    ui,
                    "Mudra est en pause : reprends pour enregistrer.",
                    p.accent,
                );
            } else if context.status.hands.is_empty() {
                note(
                    ui,
                    "Aucune main détectée : place-la dans le cadre avant de démarrer.",
                    p.accent,
                );
            } else if trainer.two_hands && context.status.hands.len() < 2 {
                note(
                    ui,
                    "Une seule main détectée : place les deux dans le cadre, sans que l'une \
                     cache l'autre.",
                    p.accent,
                );
            }
        }
        if let Some(error) = &trainer.error {
            ui.label(neu::text(error, 12.5, Weight::Semibold, p.danger));
        }
        note(
            ui,
            "Seuls les points-clés des mains sont enregistrés, jamais d'image : la vignette \
             « Aperçu », à droite, montre ce que Mudra en voit. Aucune action ne se déclenche \
             pendant une prise.",
            p.muted,
        );
    });
    open
}
