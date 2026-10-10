//! Onglet « Gestes » : quels gestes déclenchent quelles actions. Chaque
//! modification est écrite tout de suite dans le fichier de configuration, qui
//! reste la source de vérité.

use std::time::Instant;

use egui::{Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui};
use mudra_config::edit::BindingEdit;
use mudra_config::keys::KeyCombo;
use mudra_config::{
    Action, ActivationMode, Binding, Config, HandFilter, KnownGestures, MediaKey, ScreenshotTarget,
    VolumeMode, VolumeOp,
};

use super::preview::{PreviewModel, UiAction};
use super::theme::{self, ACCENT, AMBER, BORDER, CARD, CARD_HOVER, GREEN, MUTED, RED};
use crate::pipeline::Status;

/// Durée de la mise en évidence d'un binding qui vient de se déclencher.
const FLASH_SECONDS: f32 = 0.9;

/// État de l'onglet, conservé par l'application entre deux images.
#[derive(Default)]
pub struct GesturesState {
    pub dry_run: bool,
    pub editor: Option<Editor>,
    /// Dernier refus d'enregistrement (validation, fichier).
    pub error: Option<String>,
    /// Dernier binding déclenché, pour le retour visuel.
    pub flash: Option<(usize, Instant)>,
    pub can_undo: bool,
}

/// Formulaire d'un binding en cours de création ou de modification.
pub struct Editor {
    /// Indice du binding modifié ; `None` pour une création.
    pub target: Option<usize>,
    pub is_static: bool,
    /// Geste réservé à une main (réveil, verrouillage, contrôle continu) :
    /// seul un binding à deux mains est possible.
    pub both_only: bool,
    pub draft: Binding,
    pub capturing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keys,
    Media,
    Volume,
    Screenshot,
    Launch,
    Other,
}

impl Kind {
    const CHOICES: [Kind; 5] = [
        Kind::Keys,
        Kind::Media,
        Kind::Volume,
        Kind::Screenshot,
        Kind::Launch,
    ];

    fn of(action: &Action) -> Self {
        match action {
            Action::Keys { .. } => Kind::Keys,
            Action::Media { .. } => Kind::Media,
            Action::Volume {
                op: VolumeOp::Step | VolumeOp::MuteToggle,
                ..
            } => Kind::Volume,
            Action::Screenshot { .. } => Kind::Screenshot,
            Action::Launch { .. } => Kind::Launch,
            _ => Kind::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Keys => "Raccourci clavier",
            Kind::Media => "Touche multimédia",
            Kind::Volume => "Volume",
            Kind::Screenshot => "Capture d'écran",
            Kind::Launch => "Ouvrir un programme, un fichier ou un site",
            Kind::Other => "Autre (défini dans le fichier)",
        }
    }

    fn default_action(self) -> Action {
        match self {
            Kind::Keys | Kind::Other => Action::Keys {
                combo: String::new(),
            },
            Kind::Media => Action::Media {
                key: MediaKey::PlayPause,
            },
            Kind::Volume => Action::Volume {
                op: VolumeOp::Step,
                delta: 0.05,
                value: None,
                mode: VolumeMode::Api,
            },
            Kind::Screenshot => Action::Screenshot {
                target: ScreenshotTarget::default(),
                dir: "~/Pictures/Mudra".into(),
                clipboard: false,
            },
            Kind::Launch => Action::Launch {
                path: String::new(),
                args: Vec::new(),
                cwd: None,
            },
        }
    }
}

fn hand_label(hand: HandFilter) -> &'static str {
    match hand {
        HandFilter::Any => "Toute main",
        HandFilter::Left => "Main gauche",
        HandFilter::Right => "Main droite",
        HandFilter::Both => "Deux mains",
    }
}

fn media_label(key: MediaKey) -> &'static str {
    match key {
        MediaKey::PlayPause => "Lecture / pause",
        MediaKey::Next => "Piste suivante",
        MediaKey::Prev => "Piste précédente",
        MediaKey::Stop => "Arrêt",
    }
}

fn target_label(target: ScreenshotTarget) -> &'static str {
    match target {
        ScreenshotTarget::Screen => "Tous les écrans",
        ScreenshotTarget::ActiveMonitor => "Écran actif",
        ScreenshotTarget::ActiveWindow => "Fenêtre active",
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Rôle d'un geste qui n'est pas libre pour un binding à une main.
fn reserved_role(config: &Config, gesture: &str) -> Option<String> {
    let a = &config.activation;
    if a.mode == ActivationMode::WakeGesture {
        if gesture == a.wake_gesture {
            return Some("Réservé : arme Mudra".into());
        }
        if gesture == a.lock_gesture {
            return Some("Réservé : verrouille Mudra".into());
        }
    }
    config
        .continuous
        .iter()
        .find(|c| c.enter_gesture == gesture)
        .map(|c| format!("Réservé : contrôle continu « {} »", c.name))
}

/// Confiance du geste s'il est montré en ce moment par une main.
fn live_confidence(status: &Status, gesture: &str, is_static: bool) -> Option<f32> {
    status
        .hands
        .iter()
        .flat_map(|h| {
            if is_static {
                h.top_static.iter()
            } else {
                h.top_dynamic.iter()
            }
        })
        .filter(|(label, p)| label == gesture && *p >= 0.5)
        .map(|(_, p)| *p)
        .reduce(f32::max)
}

/// Raison pour laquelle le brouillon ne peut pas être enregistré.
fn draft_problem(draft: &Binding) -> Option<String> {
    match &draft.action {
        Action::Keys { combo } if combo.trim().is_empty() => {
            Some("Indique un raccourci, par exemple Ctrl+Alt+M.".into())
        }
        Action::Keys { combo } => KeyCombo::parse(combo)
            .err()
            .map(|e| format!("Raccourci non reconnu : {e}")),
        Action::Launch { path, .. } if path.trim().is_empty() => {
            Some("Indique un programme, un fichier ou une adresse web.".into())
        }
        _ => None,
    }
}

/// Raccourci saisi au clavier pendant la capture, s'il y en a un.
fn captured_combo(ui: &Ui) -> Option<Result<String, ()>> {
    ui.input(|input| {
        input.events.iter().find_map(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return None;
            };
            if *key == egui::Key::Escape && modifiers.is_none() {
                return Some(Err(()));
            }
            let mut parts: Vec<&str> = Vec::new();
            if modifiers.ctrl {
                parts.push("Ctrl");
            }
            if modifiers.alt {
                parts.push("Alt");
            }
            if modifiers.shift {
                parts.push("Shift");
            }
            let name = key.name();
            parts.push(name.strip_prefix("Arrow").unwrap_or(name));
            Some(Ok(parts.join("+")))
        })
    })
}

fn framed<R>(ui: &mut Ui, fill: egui::Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

enum Outcome {
    Open,
    Save,
    Cancel,
}

fn action_fields(ui: &mut Ui, editor: &mut Editor) {
    let capturing = &mut editor.capturing;
    match &mut editor.draft.action {
        Action::Keys { combo } => {
            ui.label(RichText::new("Raccourci").color(MUTED));
            ui.horizontal(|ui| {
                if *capturing {
                    ui.label(
                        RichText::new("Appuie sur le raccourci… (Échap pour annuler)").color(AMBER),
                    );
                    match captured_combo(ui) {
                        Some(Ok(text)) => {
                            *combo = text;
                            *capturing = false;
                        }
                        Some(Err(())) => *capturing = false,
                        None => ui.ctx().request_repaint(),
                    }
                } else {
                    ui.add(
                        egui::TextEdit::singleline(combo)
                            .desired_width(220.0)
                            .hint_text("Ctrl+Alt+M, Win+D…"),
                    );
                    if ui.button("Saisir au clavier").clicked() {
                        *capturing = true;
                    }
                }
            });
            ui.end_row();
            ui.label("");
            ui.label(
                RichText::new("La touche Windows se tape à la main : Win+D, Win+Tab…")
                    .small()
                    .color(MUTED),
            );
            ui.end_row();
        }
        Action::Media { key } => {
            ui.label(RichText::new("Touche").color(MUTED));
            ui.horizontal(|ui| {
                for choice in [
                    MediaKey::PlayPause,
                    MediaKey::Prev,
                    MediaKey::Next,
                    MediaKey::Stop,
                ] {
                    ui.selectable_value(key, choice, media_label(choice));
                }
            });
            ui.end_row();
        }
        Action::Volume { op, delta, .. } => {
            ui.label(RichText::new("Effet").color(MUTED));
            ui.horizontal(|ui| {
                let step = *op == VolumeOp::Step;
                if ui
                    .selectable_label(step && *delta >= 0.0, "Monter")
                    .clicked()
                {
                    *op = VolumeOp::Step;
                    *delta = delta.abs().max(0.01);
                }
                if ui
                    .selectable_label(step && *delta < 0.0, "Baisser")
                    .clicked()
                {
                    *op = VolumeOp::Step;
                    *delta = -delta.abs().max(0.01);
                }
                if ui
                    .selectable_label(*op == VolumeOp::MuteToggle, "Couper / rétablir le son")
                    .clicked()
                {
                    *op = VolumeOp::MuteToggle;
                }
            });
            ui.end_row();
            if *op == VolumeOp::Step {
                ui.label(RichText::new("Pas").color(MUTED));
                let mut percent = (delta.abs() * 100.0).round().max(1.0);
                if ui
                    .add(
                        egui::DragValue::new(&mut percent)
                            .range(1.0..=50.0)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    *delta = (percent / 100.0).copysign(*delta);
                }
                ui.end_row();
            }
        }
        Action::Screenshot {
            target,
            dir,
            clipboard,
        } => {
            ui.label(RichText::new("Zone").color(MUTED));
            ui.horizontal(|ui| {
                for choice in [
                    ScreenshotTarget::ActiveWindow,
                    ScreenshotTarget::ActiveMonitor,
                    ScreenshotTarget::Screen,
                ] {
                    ui.selectable_value(target, choice, target_label(choice));
                }
            });
            ui.end_row();
            ui.label(RichText::new("Dossier").color(MUTED));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(dir).desired_width(300.0));
                if ui.button("Parcourir…").clicked()
                    && let Some(folder) = rfd::FileDialog::new().pick_folder()
                {
                    *dir = folder.display().to_string();
                }
            });
            ui.end_row();
            ui.label("");
            ui.toggle_value(clipboard, "Copier aussi dans le presse-papiers");
            ui.end_row();
        }
        Action::Launch { path, .. } => {
            ui.label(RichText::new("Cible").color(MUTED));
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(path)
                        .desired_width(300.0)
                        .hint_text("notepad.exe, C:\\…\\fichier.pdf, https://…"),
                );
                if ui.button("Parcourir…").clicked()
                    && let Some(file) = rfd::FileDialog::new().pick_file()
                {
                    *path = file.display().to_string();
                }
            });
            ui.end_row();
        }
        other => {
            ui.label(RichText::new("Détail").color(MUTED));
            ui.label(format!(
                "{} — se modifie dans le fichier de configuration.",
                capitalized(&other.summary())
            ));
            ui.end_row();
        }
    }
}

fn editor_ui(
    ui: &mut Ui,
    editor: &mut Editor,
    config: &Config,
    error: Option<&str>,
    actions: &mut Vec<UiAction>,
) -> Outcome {
    let mut outcome = Outcome::Open;
    framed(ui, CARD_HOVER, |ui| {
        egui::Grid::new("edition-binding")
            .num_columns(2)
            .spacing([14.0, 8.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Main").color(MUTED));
                ui.horizontal(|ui| {
                    let hand = &mut editor.draft.hand;
                    if editor.both_only {
                        *hand = HandFilter::Both;
                        ui.label(hand_label(*hand));
                        ui.label(
                            RichText::new("(seule possibilité : ce geste est réservé à une main)")
                                .small()
                                .color(MUTED),
                        );
                    } else {
                        for choice in [HandFilter::Any, HandFilter::Left, HandFilter::Right] {
                            ui.selectable_value(hand, choice, hand_label(choice));
                        }
                        if editor.is_static {
                            ui.selectable_value(hand, HandFilter::Both, "Deux mains ensemble");
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Action").color(MUTED));
                let mut kind = Kind::of(&editor.draft.action);
                let before = kind;
                egui::ComboBox::from_id_salt("type-action")
                    .selected_text(kind.label())
                    .width(300.0)
                    .show_ui(ui, |ui| {
                        for choice in Kind::CHOICES {
                            ui.selectable_value(&mut kind, choice, choice.label());
                        }
                    });
                if kind != before {
                    editor.draft.action = kind.default_action();
                    editor.capturing = false;
                }
                ui.end_row();

                action_fields(ui, editor);
            });

        egui::CollapsingHeader::new("Réglages avancés")
            .id_salt("avance-binding")
            .show(ui, |ui| {
                let d = &config.defaults;
                let draft = &mut editor.draft;
                egui::Grid::new("avance-grille")
                    .num_columns(2)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        if editor.is_static {
                            ui.label(RichText::new("Maintien avant déclenchement").color(MUTED));
                            let mut hold = draft.hold_ms.unwrap_or(d.hold_ms);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut hold)
                                        .range(50..=5000)
                                        .speed(10)
                                        .suffix(" ms"),
                                )
                                .changed()
                            {
                                draft.hold_ms = Some(hold);
                            }
                            ui.end_row();
                        }
                        ui.label(
                            RichText::new("Délai avant un nouveau déclenchement").color(MUTED),
                        );
                        let mut cooldown = draft.cooldown_ms.unwrap_or(d.cooldown_ms);
                        if ui
                            .add(
                                egui::DragValue::new(&mut cooldown)
                                    .range(0..=10000)
                                    .speed(10)
                                    .suffix(" ms"),
                            )
                            .changed()
                        {
                            draft.cooldown_ms = Some(cooldown);
                        }
                        ui.end_row();
                        if editor.is_static {
                            ui.label(
                                RichText::new("Répéter tant que la pose est tenue").color(MUTED),
                            );
                            ui.horizontal(|ui| {
                                let mut repeat = draft.repeat_ms.is_some();
                                let toggle = if repeat { "Oui" } else { "Non" };
                                if ui.toggle_value(&mut repeat, toggle).changed() {
                                    draft.repeat_ms = repeat.then_some(250);
                                }
                                if let Some(ms) = &mut draft.repeat_ms {
                                    ui.add(
                                        egui::DragValue::new(ms)
                                            .range(50..=5000)
                                            .speed(10)
                                            .prefix("toutes les ")
                                            .suffix(" ms"),
                                    );
                                }
                            });
                            ui.end_row();
                        }
                    });
            });

        // Deux bindings du même geste pour la même main : le second ne partirait jamais.
        let overlaps = |a: HandFilter, b: HandFilter| {
            a == b
                || (a != HandFilter::Both
                    && b != HandFilter::Both
                    && (a == HandFilter::Any || b == HandFilter::Any))
        };
        let duplicate = config.bindings.iter().enumerate().any(|(i, b)| {
            Some(i) != editor.target
                && b.gesture == editor.draft.gesture
                && overlaps(b.hand, editor.draft.hand)
        });
        let problem = draft_problem(&editor.draft).or_else(|| {
            duplicate.then(|| {
                "Ce geste a déjà une action pour cette main : modifie-la, ou choisis une autre main."
                    .to_string()
            })
        });
        if let Some(text) = problem.as_deref().or(error) {
            ui.label(RichText::new(text).small().color(RED));
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let ready = problem.is_none() && Kind::of(&editor.draft.action) != Kind::Other
                || (problem.is_none() && editor.target.is_some());
            if ui
                .add_enabled(ready, egui::Button::new("Enregistrer"))
                .clicked()
            {
                outcome = Outcome::Save;
            }
            if ui
                .add_enabled(problem.is_none(), egui::Button::new("Tester l'action"))
                .on_hover_text("Exécute l'action une fois, tout de suite, sans faire le geste.")
                .clicked()
            {
                actions.push(UiAction::TestAction(editor.draft.action.clone()));
            }
            if ui.button("Annuler").clicked() {
                outcome = Outcome::Cancel;
            }
        });
    });
    outcome
}

struct Section<'a> {
    title: &'a str,
    hint: &'a str,
    labels: &'a [String],
    is_static: bool,
}

fn gesture_card(
    ui: &mut Ui,
    section: &Section<'_>,
    gesture: &str,
    config: &Config,
    status: &Status,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let role = section
        .is_static
        .then(|| reserved_role(config, gesture))
        .flatten();
    let live = live_confidence(status, gesture, section.is_static);
    framed(ui, CARD, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(theme::gesture_name(gesture)).strong());
            if let Some(p) = live {
                ui.label(
                    RichText::new(format!("détecté · {:.0} %", p * 100.0))
                        .small()
                        .color(GREEN),
                );
            }
            if let Some(role) = &role {
                ui.label(RichText::new(role).small().color(MUTED));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if role.is_some() {
                    "+ Associer (deux mains)"
                } else {
                    "+ Associer une action"
                };
                if ui.button(label).clicked() {
                    state.error = None;
                    state.editor = Some(Editor {
                        target: None,
                        is_static: section.is_static,
                        both_only: role.is_some(),
                        draft: Binding {
                            gesture: gesture.to_string(),
                            enabled: true,
                            hand: if role.is_some() {
                                HandFilter::Both
                            } else {
                                HandFilter::Any
                            },
                            hold_ms: None,
                            cooldown_ms: None,
                            repeat_ms: None,
                            static_enter: None,
                            static_exit: None,
                            dynamic_threshold: None,
                            action: Kind::Keys.default_action(),
                        },
                        capturing: false,
                    });
                }
            });
        });

        let mut any = false;
        for (index, binding) in config.bindings.iter().enumerate() {
            if binding.gesture != gesture {
                continue;
            }
            any = true;
            let editing = state
                .editor
                .as_ref()
                .is_some_and(|e| e.target == Some(index));
            let fired = state
                .flash
                .is_some_and(|(i, at)| i == index && at.elapsed().as_secs_f32() < FLASH_SECONDS);
            if fired {
                ui.ctx().request_repaint();
            }
            ui.horizontal(|ui| {
                let mut enabled = binding.enabled;
                let toggle = if enabled { "Actif" } else { "Inactif" };
                if ui
                    .toggle_value(&mut enabled, toggle)
                    .on_hover_text("Activer ou désactiver sans supprimer")
                    .changed()
                {
                    actions.push(UiAction::EditBinding(BindingEdit::SetEnabled(
                        index, enabled,
                    )));
                }
                let ignored = role.is_some() && binding.hand != HandFilter::Both;
                let color = if fired {
                    GREEN
                } else if !binding.enabled || ignored {
                    MUTED
                } else {
                    theme::TEXT
                };
                ui.label(RichText::new(hand_label(binding.hand)).color(MUTED));
                ui.label(RichText::new(capitalized(&binding.action.summary())).color(color));
                if fired {
                    ui.label(RichText::new("déclenché").small().color(GREEN));
                } else if ignored {
                    ui.label(RichText::new("ignoré : geste réservé").small().color(AMBER));
                } else if !binding.enabled {
                    ui.label(RichText::new("désactivé").small().color(MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Supprimer").clicked() {
                        actions.push(UiAction::EditBinding(BindingEdit::Remove(index)));
                        state.editor = None;
                    }
                    if ui
                        .button("Tester")
                        .on_hover_text("Exécute l'action une fois, sans faire le geste.")
                        .clicked()
                    {
                        actions.push(UiAction::TestAction(binding.action.clone()));
                    }
                    if ui
                        .add_enabled(!editing, egui::Button::new("Modifier"))
                        .clicked()
                    {
                        state.error = None;
                        state.editor = Some(Editor {
                            target: Some(index),
                            is_static: section.is_static,
                            both_only: role.is_some(),
                            draft: binding.clone(),
                            capturing: false,
                        });
                    }
                });
            });
            if editing {
                edit_in_place(ui, config, state, actions);
            }
        }
        if !any && role.is_none() {
            ui.label(RichText::new("Aucune action.").small().color(MUTED));
        }
        let creating = state
            .editor
            .as_ref()
            .is_some_and(|e| e.target.is_none() && e.draft.gesture == gesture);
        if creating {
            edit_in_place(ui, config, state, actions);
        }
    });
}

fn edit_in_place(
    ui: &mut Ui,
    config: &Config,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let Some(mut editor) = state.editor.take() else {
        return;
    };
    match editor_ui(ui, &mut editor, config, state.error.as_deref(), actions) {
        Outcome::Open => state.editor = Some(editor),
        Outcome::Cancel => state.error = None,
        Outcome::Save => {
            let edit = match editor.target {
                Some(index) => BindingEdit::Replace(index, editor.draft.clone()),
                None => BindingEdit::Add(editor.draft.clone()),
            };
            actions.push(UiAction::EditBinding(edit));
            // Le formulaire reste ouvert : l'application le ferme si
            // l'enregistrement réussit, et affiche l'erreur sinon.
            state.editor = Some(editor);
        }
    }
}

fn activation_text(config: &Config) -> String {
    let a = &config.activation;
    match a.mode {
        ActivationMode::WakeGesture => format!(
            "Les gestes n'agissent qu'une fois Mudra armé. Armer : « {} » pendant {} s. \
             Verrouiller : « {} ». Sans geste, Mudra se reverrouille après {:.0} s.",
            theme::gesture_name(&a.wake_gesture),
            format!("{:.1}", a.wake_hold_ms as f32 / 1000.0).replace('.', ","),
            theme::gesture_name(&a.lock_gesture),
            a.armed_duration_ms as f32 / 1000.0,
        ),
        _ => "Les gestes agissent en permanence (pas de geste d'armement).".into(),
    }
}

pub fn draw(ui: &mut Ui, model: &mut PreviewModel<'_>, actions: &mut Vec<UiAction>) {
    let config: &Config = model.config;
    let status: &Status = model.status;
    let known: &KnownGestures = model.known;
    let state: &mut GesturesState = model.gestures;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(920.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Gestes et actions").heading().strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .toggle_value(&mut state.dry_run, "Mode essai")
                        .on_hover_text(
                            "Les gestes sont reconnus et signalés ici, mais aucune action \
                             n'est exécutée. Pratique pendant les réglages.",
                        )
                        .changed()
                    {
                        actions.push(UiAction::SetDryRun(state.dry_run));
                    }
                    if ui
                        .add_enabled(
                            state.can_undo,
                            egui::Button::new("Annuler la dernière modification"),
                        )
                        .clicked()
                    {
                        actions.push(UiAction::UndoEdit);
                        state.editor = None;
                    }
                });
            });
            ui.label(
                RichText::new(
                    "Chaque modification est enregistrée tout de suite dans config.toml et \
                     appliquée sans redémarrer.",
                )
                .small()
                .color(MUTED),
            );
            if state.editor.is_none()
                && let Some(error) = &state.error
            {
                ui.label(RichText::new(error).color(RED));
            }
            ui.add_space(6.0);
            framed(ui, CARD, |ui| {
                ui.label(RichText::new("ACTIVATION").small().color(MUTED).strong());
                ui.label(activation_text(config));
            });

            let sections = [
                Section {
                    title: "POSES",
                    hint: "Une forme de main tenue un instant.",
                    labels: &known.static_,
                    is_static: true,
                },
                Section {
                    title: "MOUVEMENTS",
                    hint: "Un geste bref de la main.",
                    labels: &known.dynamic,
                    is_static: false,
                },
            ];
            for section in &sections {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(section.title).small().color(ACCENT).strong());
                    ui.label(RichText::new(section.hint).small().color(MUTED));
                });
                for gesture in section.labels.iter().filter(|g| g.as_str() != "none") {
                    gesture_card(ui, section, gesture, config, status, state, actions);
                }
            }

            if !config.continuous.is_empty() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("CONTRÔLES CONTINUS")
                            .small()
                            .color(ACCENT)
                            .strong(),
                    );
                    ui.label(
                        RichText::new("Réglables dans le fichier de configuration.")
                            .small()
                            .color(MUTED),
                    );
                });
                framed(ui, CARD, |ui| {
                    for control in &config.continuous {
                        let active = status.continuous.as_deref() == Some(control.name.as_str());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(theme::gesture_name(&control.enter_gesture)).strong(),
                            );
                            ui.label(RichText::new("maintenu").color(MUTED));
                            ui.label(format!("contrôle « {} »", control.name));
                            if active {
                                ui.label(RichText::new("actif").small().color(GREEN));
                            }
                        });
                    }
                });
            }
            ui.add_space(12.0);
        });
}
