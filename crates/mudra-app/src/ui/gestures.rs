//! Onglet « Gestes » : quels gestes déclenchent quelles actions. Chaque
//! modification est écrite tout de suite dans le fichier de configuration, qui
//! reste la source de vérité.

use std::time::Instant;

use egui::{Align, Align2, Color32, Layout, Margin, Pos2, Rect, Sense, Ui, UiBuilder, Vec2};
use mudra_capture::CaptureState;
use mudra_config::edit::{ActivationEdit, BindingEdit, ContinuousEdit};
use mudra_config::keys::KeyCombo;
use mudra_config::{
    Action, ActivationMode, Binding, Config, Control, HandFilter, KnownGestures, MediaKey,
    ScreenshotTarget, VolumeMode, VolumeOp,
};
use mudra_gestures::EngineState;
use mudra_gestures::custom::{CustomPose, Resemblance, SENSITIVITY_RANGE};
use mudra_gestures::motion::{self, CustomMove};

use super::icons::{self, Icon};
use super::neu::{self, Button, Card, Segments};
use super::preview::{
    BONES, GUTTER, MARGIN, PreviewModel, RecordForm, STACK, UiAction, key_value, region,
};
use super::theme::{self, ORANGE, Weight, palette};
use super::trainer::{self, Trainer};
use crate::pipeline::{PreviewFrame, Status};

/// Durée de la mise en évidence d'un binding qui vient de se déclencher.
const FLASH_SECONDS: f32 = 0.9;
/// Largeur de la colonne latérale (vignette et résumé).
const SIDE: f32 = 320.0;
/// Écart entre deux cartes compactes.
const GRID: f32 = 18.0;

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
    /// Apprentissage d'une pose perso en cours.
    pub trainer: Option<Trainer>,
    /// Réglage de sensibilité en cours de glissement : (label, valeur).
    pub sensitivity: Option<(String, f32)>,
    /// Renommage en cours : (label, nouveau nom).
    pub renaming: Option<(String, String)>,
    /// Pose dont la suppression attend une confirmation.
    pub confirm_delete: Option<String>,
}

impl GesturesState {
    /// Ouvre le formulaire d'une nouvelle action pour ce geste.
    pub fn open_new_binding(
        &mut self,
        gesture: &str,
        is_static: bool,
        both_only: Option<BothOnly>,
    ) {
        self.error = None;
        self.editor = Some(Editor {
            target: None,
            is_static,
            both_only,
            draft: Binding {
                gesture: gesture.to_string(),
                enabled: true,
                hand: if both_only.is_some() {
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
            advanced: false,
            fresh: true,
        });
    }
}

/// Pourquoi un geste ne peut être associé qu'à deux mains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BothOnly {
    /// Geste réservé à une main : réveil, verrouillage, contrôle continu.
    Reserved,
    /// Pose perso qui ne se fait qu'à deux mains.
    TwoHandPose,
}

/// Formulaire d'un binding en cours de création ou de modification.
pub struct Editor {
    /// Indice du binding modifié ; `None` pour une création.
    pub target: Option<usize>,
    pub is_static: bool,
    /// Seul un binding à deux mains est possible, et pourquoi.
    pub both_only: Option<BothOnly>,
    pub draft: Binding,
    pub capturing: bool,
    /// Les réglages avancés sont dépliés.
    pub advanced: bool,
    /// Le formulaire vient de s'ouvrir : la page défile jusqu'à lui.
    pub fresh: bool,
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

/// Action en deux morceaux : ce qu'elle fait, et sur quoi (touches, chemin).
fn action_parts(action: &Action) -> (String, Option<String>) {
    match action {
        Action::Keys { combo } => ("Touches".into(), Some(combo.clone())),
        Action::Launch { path, .. } => ("Lancer".into(), Some(path.clone())),
        Action::Command { command, .. } => ("Commande".into(), Some(command.clone())),
        other => (capitalized(&other.summary()), None),
    }
}

/// Rôle d'un geste qui n'est pas libre pour un binding à une main, et où ce
/// rôle se change.
fn reserved_role(config: &Config, gesture: &str) -> Option<String> {
    let a = &config.activation;
    if a.mode == ActivationMode::WakeGesture {
        if gesture == a.wake_gesture {
            return Some("Réservé : arme Mudra · se change dans « Activation »".into());
        }
        if gesture == a.lock_gesture {
            return Some("Réservé : verrouille Mudra · se change dans « Activation »".into());
        }
    }
    config
        .continuous
        .iter()
        .find(|c| c.enabled && c.enter_gesture == gesture)
        .map(|c| {
            format!(
                "Réservé : contrôle continu « {} » · se change dans « Contrôles continus »",
                c.name
            )
        })
}

/// Poses qu'un rôle (armement, verrouillage, contrôle continu) peut prendre :
/// celles à une main qui n'en ont pas déjà un, plus `current`.
fn role_choices<'a>(config: &Config, known: &'a KnownGestures, current: &str) -> Vec<&'a String> {
    known
        .static_
        .iter()
        .filter(|g| {
            g.as_str() == current
                || (g.as_str() != "none"
                    && !known.is_two_hands(g)
                    && reserved_role(config, g).is_none())
        })
        .collect()
}

/// Liste déroulante d'une pose ; renvoie la pose choisie si elle change.
fn pose_combo(ui: &mut Ui, width: f32, current: &str, choices: &[&String]) -> Option<String> {
    let mut picked = None;
    neu::dropdown(ui, theme::gesture_name(current), width, |ui| {
        for choice in choices {
            let selected = choice.as_str() == current;
            if neu::menu_item(ui, selected, theme::gesture_name(choice)).clicked() && !selected {
                picked = Some(choice.to_string());
            }
        }
    });
    picked
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

/// Mode d'activation et poses d'armement et de verrouillage.
fn activation_card(
    ui: &mut Ui,
    config: &Config,
    known: &KnownGestures,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    let a = &config.activation;
    Card::LARGE.pad(24.0, 22.0).gap(14.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            neu::accent_dot(ui, 8.0);
            ui.label(neu::caps("ACTIVATION", p.muted));
        });
        ui.label(neu::rich(
            ui.available_width(),
            20.0,
            &[(
                activation_text(config).as_str(),
                theme::font(13.0, Weight::Regular),
                p.text,
            )],
        ));
        let modes = |ui: &mut Ui, actions: &mut Vec<UiAction>| {
            let modes = [ActivationMode::WakeGesture, ActivationMode::AlwaysOn];
            let selected = modes.iter().position(|m| *m == a.mode);
            let picked = Segments::CARD.show_hinted(
                ui,
                &["Geste d'armement", "Toujours actif"],
                &[
                    "Les gestes n'agissent qu'après la pose d'armement.",
                    "Les gestes agissent en permanence : les poses d'armement et de \
                     verrouillage redeviennent libres.",
                ],
                selected,
            );
            if let Some(index) = picked {
                actions.push(UiAction::EditActivation(ActivationEdit::Mode(modes[index])));
            }
        };
        let roles = |ui: &mut Ui, actions: &mut Vec<UiAction>| {
            ui.label(neu::text("Armer avec", 12.5, Weight::Regular, p.muted));
            let choices = role_choices(config, known, &a.wake_gesture);
            if let Some(pose) = pose_combo(ui, 170.0, &a.wake_gesture, &choices) {
                actions.push(UiAction::EditActivation(ActivationEdit::WakeGesture(pose)));
            }
            ui.add_space(10.0);
            ui.label(neu::text(
                "Verrouiller avec",
                12.5,
                Weight::Regular,
                p.muted,
            ));
            let choices = role_choices(config, known, &a.lock_gesture);
            if let Some(pose) = pose_combo(ui, 150.0, &a.lock_gesture, &choices) {
                actions.push(UiAction::EditActivation(ActivationEdit::LockGesture(pose)));
            }
        };
        let wake = a.mode == ActivationMode::WakeGesture;
        // Tout sur une ligne quand la place le permet, sinon les poses dessous.
        let one_line = ui.available_width() >= 760.0;
        ui.horizontal(|ui| {
            ui.set_min_height(42.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            modes(ui, actions);
            if wake && one_line {
                ui.add_space(10.0);
                roles(ui, actions);
            }
        });
        if wake && !one_line {
            ui.horizontal(|ui| {
                ui.set_min_height(38.0);
                ui.spacing_mut().item_spacing.x = 10.0;
                roles(ui, actions);
            });
        }
    });
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

// ----- Formulaire d'une action -----

enum Outcome {
    Open,
    Save,
    Cancel,
}

/// Libellé d'un champ de formulaire, centré sur la hauteur de ce champ : la
/// grille place le libellé avant de connaître la hauteur de la ligne.
fn field_label(ui: &mut Ui, text: &str) {
    let p = palette();
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), theme::font(12.5, Weight::Regular), p.muted);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x, 34.0), Sense::hover());
    ui.painter().galley(
        Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        p.muted,
    );
}

fn note(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    ui.label(neu::text(text, 11.5, Weight::Regular, color));
}

fn action_fields(ui: &mut Ui, editor: &mut Editor) {
    let p = palette();
    let capturing = &mut editor.capturing;
    match &mut editor.draft.action {
        Action::Keys { combo } => {
            field_label(ui, "Raccourci");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if *capturing {
                    ui.label(neu::text(
                        "Appuie sur le raccourci… (Échap pour annuler)",
                        12.5,
                        Weight::Semibold,
                        p.accent,
                    ));
                    match captured_combo(ui) {
                        Some(Ok(text)) => {
                            *combo = text;
                            *capturing = false;
                        }
                        Some(Err(())) => *capturing = false,
                        None => ui.ctx().request_repaint(),
                    }
                } else {
                    neu::text_field(ui, combo, 220.0, "Ctrl+Alt+M, Win+D…");
                    if ui.add(Button::new("Saisir au clavier")).clicked() {
                        *capturing = true;
                    }
                }
            });
            ui.end_row();
            ui.label("");
            note(
                ui,
                "La touche Windows se tape à la main : Win+D, Win+Tab…",
                p.muted,
            );
            ui.end_row();
        }
        Action::Media { key } => {
            field_label(ui, "Touche");
            let keys = [
                MediaKey::PlayPause,
                MediaKey::Prev,
                MediaKey::Next,
                MediaKey::Stop,
            ];
            let labels = keys.map(media_label);
            let selected = keys.iter().position(|k| k == key);
            if let Some(index) = Segments::FORM.show(ui, &labels, selected) {
                *key = keys[index];
            }
            ui.end_row();
        }
        Action::Volume { op, delta, .. } => {
            field_label(ui, "Effet");
            let selected = match op {
                VolumeOp::Step if *delta >= 0.0 => Some(0),
                VolumeOp::Step => Some(1),
                VolumeOp::MuteToggle => Some(2),
                _ => None,
            };
            let picked = Segments::FORM.show(
                ui,
                &["Monter", "Baisser", "Couper / rétablir le son"],
                selected,
            );
            match picked {
                Some(0) => {
                    *op = VolumeOp::Step;
                    *delta = delta.abs().max(0.01);
                }
                Some(1) => {
                    *op = VolumeOp::Step;
                    *delta = -delta.abs().max(0.01);
                }
                Some(_) => *op = VolumeOp::MuteToggle,
                None => {}
            }
            ui.end_row();
            if *op == VolumeOp::Step {
                field_label(ui, "Pas");
                let mut percent = (delta.abs() * 100.0).round().max(1.0);
                if neu::number(ui, &mut percent, 1.0..=50.0, 1.0, "", " %", 76.0).changed() {
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
            field_label(ui, "Zone");
            let targets = [
                ScreenshotTarget::ActiveWindow,
                ScreenshotTarget::ActiveMonitor,
                ScreenshotTarget::Screen,
            ];
            let labels = targets.map(target_label);
            let selected = targets.iter().position(|t| t == target);
            if let Some(index) = Segments::FORM.show(ui, &labels, selected) {
                *target = targets[index];
            }
            ui.end_row();
            field_label(ui, "Dossier");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                neu::text_field(ui, dir, 300.0, "");
                if ui.add(Button::new("Parcourir…")).clicked()
                    && let Some(folder) = rfd::FileDialog::new().pick_folder()
                {
                    *dir = folder.display().to_string();
                }
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                neu::switch(ui, clipboard, Vec2::new(40.0, 22.0));
                ui.label(neu::text(
                    "Copier aussi dans le presse-papiers",
                    12.5,
                    Weight::Regular,
                    p.text,
                ));
            });
            ui.end_row();
        }
        Action::Launch { path, .. } => {
            field_label(ui, "Cible");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                neu::text_field(
                    ui,
                    path,
                    300.0,
                    "notepad.exe, C:\\…\\fichier.pdf, https://…",
                );
                if ui.add(Button::new("Parcourir…")).clicked()
                    && let Some(file) = rfd::FileDialog::new().pick_file()
                {
                    *path = file.display().to_string();
                }
            });
            ui.end_row();
        }
        other => {
            field_label(ui, "Détail");
            ui.label(neu::text(
                format!(
                    "{} — se modifie dans le fichier de configuration.",
                    capitalized(&other.summary())
                ),
                12.5,
                Weight::Regular,
                p.text,
            ));
            ui.end_row();
        }
    }
}

fn advanced_fields(ui: &mut Ui, editor: &mut Editor, config: &Config) {
    let p = palette();
    let d = &config.defaults;
    let is_static = editor.is_static;
    let draft = &mut editor.draft;
    egui::Grid::new("avance-grille")
        .num_columns(2)
        .spacing([18.0, 10.0])
        .show(ui, |ui| {
            if is_static {
                field_label(ui, "Maintien avant déclenchement");
                let mut hold = draft.hold_ms.unwrap_or(d.hold_ms);
                if neu::number(ui, &mut hold, 50..=5000, 10.0, "", " ms", 96.0).changed() {
                    draft.hold_ms = Some(hold);
                }
                ui.end_row();
            }
            field_label(ui, "Délai avant un nouveau déclenchement");
            let mut cooldown = draft.cooldown_ms.unwrap_or(d.cooldown_ms);
            if neu::number(ui, &mut cooldown, 0..=10000, 10.0, "", " ms", 96.0).changed() {
                draft.cooldown_ms = Some(cooldown);
            }
            ui.end_row();
            if is_static {
                field_label(ui, "Répéter tant que la pose est tenue");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let mut repeat = draft.repeat_ms.is_some();
                    if neu::switch(ui, &mut repeat, Vec2::new(40.0, 22.0)).changed() {
                        draft.repeat_ms = repeat.then_some(250);
                    }
                    match &mut draft.repeat_ms {
                        Some(ms) => {
                            neu::number(ui, ms, 50..=5000, 10.0, "toutes les ", " ms", 150.0);
                        }
                        None => {
                            ui.label(neu::text("Non", 12.5, Weight::Regular, p.muted));
                        }
                    }
                });
                ui.end_row();
            }
        });
}

fn editor_ui(
    ui: &mut Ui,
    editor: &mut Editor,
    config: &Config,
    error: Option<&str>,
    actions: &mut Vec<UiAction>,
) -> Outcome {
    let p = palette();
    let mut outcome = Outcome::Open;
    let width = ui.available_width();
    if std::mem::take(&mut editor.fresh) {
        ui.scroll_to_cursor(Some(Align::Center));
    }
    neu::hollow(ui, width, 16.0, Margin::symmetric(16, 14), 0.0, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        egui::Grid::new("edition-binding")
            .num_columns(2)
            .spacing([18.0, 10.0])
            .show(ui, |ui| {
                field_label(ui, "Main");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let hand = &mut editor.draft.hand;
                    if let Some(why) = editor.both_only {
                        *hand = HandFilter::Both;
                        neu::chip(ui, hand_label(*hand));
                        let why = match why {
                            BothOnly::Reserved => {
                                "seule possibilité : ce geste est réservé à une main"
                            }
                            BothOnly::TwoHandPose => "cette pose se fait à deux mains",
                        };
                        note(ui, why, p.muted);
                    } else {
                        let mut hands = vec![HandFilter::Any, HandFilter::Left, HandFilter::Right];
                        let mut labels: Vec<&str> = hands.iter().map(|h| hand_label(*h)).collect();
                        if editor.is_static {
                            hands.push(HandFilter::Both);
                            labels.push("Deux mains ensemble");
                        }
                        let selected = hands.iter().position(|h| h == hand);
                        if let Some(index) = Segments::FORM.show(ui, &labels, selected) {
                            *hand = hands[index];
                        }
                    }
                });
                ui.end_row();

                field_label(ui, "Action");
                let kind = Kind::of(&editor.draft.action);
                let mut picked = None;
                neu::dropdown(ui, kind.label(), 320.0, |ui| {
                    for choice in Kind::CHOICES {
                        if neu::menu_item(ui, choice == kind, choice.label()).clicked()
                            && choice != kind
                        {
                            picked = Some(choice);
                        }
                    }
                });
                if let Some(kind) = picked {
                    editor.draft.action = kind.default_action();
                    editor.capturing = false;
                }
                ui.end_row();

                action_fields(ui, editor);
            });

        let toggle = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::click());
                let angle = if editor.advanced {
                    0.0
                } else {
                    -std::f32::consts::FRAC_PI_2
                };
                icons::CHEVRON.paint_turned(ui.painter(), rect, p.muted, angle, false);
                let label = ui.add(
                    egui::Label::new(neu::text(
                        "Réglages avancés",
                        12.5,
                        Weight::Semibold,
                        p.soft,
                    ))
                    .sense(Sense::click()),
                );
                response.clicked() || label.clicked()
            })
            .inner;
        if toggle {
            editor.advanced = !editor.advanced;
        }
        if editor.advanced {
            advanced_fields(ui, editor, config);
        }

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
            note(ui, text, p.danger);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let ready = problem.is_none() && Kind::of(&editor.draft.action) != Kind::Other
                || (problem.is_none() && editor.target.is_some());
            if ui
                .add_enabled(ready, Button::new("Enregistrer").primary())
                .clicked()
            {
                outcome = Outcome::Save;
            }
            if ui
                .add_enabled(
                    problem.is_none(),
                    Button::new("Tester l'action")
                        .icon(&icons::PLAY, 11.0)
                        .icon_color(ORANGE),
                )
                .on_hover_text("Exécute l'action une fois, tout de suite, sans faire le geste.")
                .clicked()
            {
                actions.push(UiAction::TestAction(editor.draft.action.clone()));
            }
            if ui.add(Button::new("Annuler")).clicked() {
                outcome = Outcome::Cancel;
            }
        });
    });
    outcome
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

// ----- Cartes des gestes -----

/// Ce que l'onglet lit sans le modifier.
struct View<'a> {
    config: &'a Config,
    status: &'a Status,
    known: &'a KnownGestures,
    custom: &'a [CustomPose],
    moves: &'a [CustomMove],
}

/// Geste appris par l'exemple, vu par sa carte : pose perso, exemples ajoutés
/// à un geste de base, ou mouvement perso.
struct Learned<'a> {
    label: &'a str,
    name: &'a str,
    sensitivity: f32,
    /// Bornes du réglage de sensibilité.
    range: (f32, f32),
    examples: usize,
    takes: u32,
    resemblances: &'a [Resemblance],
    extends_base: bool,
    two_hands: bool,
    movement: bool,
}

impl<'a> Learned<'a> {
    fn pose(pose: &'a CustomPose) -> Self {
        Self {
            label: &pose.label,
            name: &pose.name,
            sensitivity: pose.sensitivity,
            range: SENSITIVITY_RANGE,
            examples: pose.examples.len(),
            takes: pose.takes,
            resemblances: &pose.resemblances,
            extends_base: pose.extends_base,
            two_hands: pose.two_hands,
            movement: false,
        }
    }

    fn movement(learnt: &'a CustomMove) -> Self {
        Self {
            label: &learnt.label,
            name: &learnt.name,
            sensitivity: learnt.sensitivity,
            range: motion::SENSITIVITY_RANGE,
            examples: learnt.examples.len(),
            takes: learnt.takes(),
            resemblances: &learnt.resemblances,
            extends_base: false,
            two_hands: false,
            movement: true,
        }
    }

    /// « cette pose » ou « ce mouvement », dans une phrase.
    fn this(&self) -> &'static str {
        if self.movement {
            "ce mouvement"
        } else {
            "cette pose"
        }
    }
}

/// Un geste dans sa carte : ce qui décide de ses boutons et de ses actions.
struct Entry<'a> {
    gesture: &'a str,
    is_static: bool,
    /// Pose perso, exemples ajoutés à un geste de base, ou mouvement perso.
    learned: Option<Learned<'a>>,
    role: Option<String>,
    both_only: Option<BothOnly>,
    two_hands: bool,
}

impl<'a> Entry<'a> {
    fn new(view: &View<'a>, gesture: &'a str, is_static: bool) -> Self {
        let learned = if is_static {
            view.custom
                .iter()
                .find(|p| p.label == gesture)
                .map(Learned::pose)
        } else {
            view.moves
                .iter()
                .find(|m| m.label == gesture)
                .map(Learned::movement)
        };
        let role = is_static
            .then(|| reserved_role(view.config, gesture))
            .flatten();
        let two_hands = learned
            .as_ref()
            .is_some_and(|l| l.two_hands && !l.extends_base);
        let both_only = if role.is_some() {
            Some(BothOnly::Reserved)
        } else {
            two_hands.then_some(BothOnly::TwoHandPose)
        };
        Self {
            gesture,
            is_static,
            learned,
            role,
            both_only,
            two_hands,
        }
    }
}

/// Nombre d'actions qui utilisent ce geste dans la configuration.
fn uses(config: &Config, gesture: &str) -> usize {
    config
        .bindings
        .iter()
        .filter(|b| b.gesture == gesture)
        .count()
}

fn associate_button(ui: &mut Ui, entry: &Entry<'_>, compact: bool, state: &mut GesturesState) {
    let label = if entry.role.is_some() {
        "Associer (deux mains)"
    } else {
        "Associer une action"
    };
    let button = if compact {
        Button::new(label)
            .pad(12.0)
            .size(11.5)
            .icon(&icons::PLUS, 10.0)
    } else {
        Button::new(label).icon(&icons::PLUS, 11.0)
    };
    if ui.add(button).clicked() {
        state.open_new_binding(entry.gesture, entry.is_static, entry.both_only);
    }
}

/// Les trois boutons ronds d'une action : supprimer, tester, modifier (posés
/// de droite à gauche).
fn binding_buttons(
    ui: &mut Ui,
    side: f32,
    index: usize,
    binding: &Binding,
    entry: &Entry<'_>,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    let icon = side * 0.41;
    let editing = state
        .editor
        .as_ref()
        .is_some_and(|e| e.target == Some(index));
    if neu::icon_button(ui, &icons::TRASH, side, side / 2.0, icon, p.danger)
        .on_hover_text("Supprimer l'action")
        .clicked()
    {
        actions.push(UiAction::EditBinding(BindingEdit::Remove(index)));
        state.editor = None;
    }
    if neu::icon_button(ui, &icons::PLAY, side, side / 2.0, icon * 0.86, ORANGE)
        .on_hover_text("Tester : exécute l'action une fois, sans faire le geste.")
        .clicked()
    {
        actions.push(UiAction::TestAction(binding.action.clone()));
    }
    let edit = ui
        .add_enabled_ui(!editing, |ui| {
            neu::icon_button(ui, &icons::PENCIL, side, side / 2.0, icon, p.text)
        })
        .inner
        .on_hover_text("Modifier l'action");
    if edit.clicked() {
        state.error = None;
        state.editor = Some(Editor {
            target: Some(index),
            is_static: entry.is_static,
            both_only: entry.both_only,
            draft: binding.clone(),
            capturing: false,
            advanced: false,
            fresh: true,
        });
    }
}

/// Mention à droite d'une action : vient de partir, ignorée, désactivée.
fn binding_tag(
    binding: &Binding,
    entry: &Entry<'_>,
    fired: bool,
) -> Option<(&'static str, Color32)> {
    let p = palette();
    let ignored = entry.both_only.is_some() && binding.hand != HandFilter::Both;
    if fired {
        Some(("déclenché", p.ok))
    } else if ignored && entry.two_hands {
        Some(("ignoré : pose à deux mains", p.accent))
    } else if ignored {
        Some(("ignoré : geste réservé", p.accent))
    } else {
        None
    }
}

/// Actions du geste, une ligne en creux chacune. `inline` : le formulaire de
/// modification s'ouvre sous la ligne (cartes pleine largeur).
fn binding_rows(
    ui: &mut Ui,
    view: &View<'_>,
    entry: &Entry<'_>,
    compact: bool,
    inline: bool,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    for (index, binding) in view.config.bindings.iter().enumerate() {
        if binding.gesture != entry.gesture {
            continue;
        }
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
        let tag = binding_tag(binding, entry, fired);
        let width = ui.available_width();
        let (radius, pad, switch) = if compact {
            (
                14.0,
                Margin {
                    left: 12,
                    right: 8,
                    top: 8,
                    bottom: 8,
                },
                Vec2::new(40.0, 22.0),
            )
        } else {
            (
                16.0,
                Margin {
                    left: 14,
                    right: 10,
                    top: 10,
                    bottom: 10,
                },
                Vec2::new(44.0, 24.0),
            )
        };
        neu::hollow(ui, width, radius, pad, 0.0, |ui| {
            ui.horizontal(|ui| {
                // La hauteur des boutons ronds, connue d'avance : tout se centre dessus.
                ui.set_min_height(if compact { 30.0 } else { 34.0 });
                ui.spacing_mut().item_spacing.x = if compact { 10.0 } else { 14.0 };
                let mut enabled = binding.enabled;
                if neu::switch(ui, &mut enabled, switch)
                    .on_hover_text("Activer ou désactiver sans supprimer")
                    .changed()
                {
                    actions.push(UiAction::EditBinding(BindingEdit::SetEnabled(
                        index, enabled,
                    )));
                }
                let dim = !binding.enabled || tag.is_some_and(|(_, c)| c == p.accent);
                let ink = if fired {
                    p.ok
                } else if dim {
                    p.muted
                } else {
                    p.text
                };
                if !compact {
                    let (label, color) = if binding.enabled {
                        ("Actif", p.accent)
                    } else {
                        ("Inactif", p.muted)
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(44.0, 24.0),
                        Layout::left_to_right(Align::Center),
                        |ui| ui.label(neu::text(label, 12.0, Weight::Bold, color)),
                    );
                    neu::chip(ui, hand_label(binding.hand));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = if compact { 8.0 } else { 10.0 };
                    let side = if compact { 30.0 } else { 34.0 };
                    binding_buttons(ui, side, index, binding, entry, state, actions);
                    if !compact && let Some((text, color)) = tag {
                        ui.label(neu::text(text, 11.0, Weight::Semibold, color));
                    }
                    if compact {
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let summary = capitalized(&binding.action.summary());
                            ui.add(
                                egui::Label::new(neu::text(summary, 12.5, Weight::Bold, ink))
                                    .truncate(),
                            );
                            let (text, color) = match tag {
                                Some((text, color)) => (text, color),
                                None => (hand_label(binding.hand), p.muted),
                            };
                            ui.add(
                                egui::Label::new(neu::text(text, 11.0, Weight::Regular, color))
                                    .truncate(),
                            );
                        });
                    } else {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let (verb, target) = action_parts(&binding.action);
                            ui.label(neu::text(verb, 12.5, Weight::Bold, ink));
                            if let Some(target) = target {
                                ui.add(
                                    egui::Label::new(neu::mono(target, 11.5, p.soft)).truncate(),
                                );
                            }
                        });
                    }
                });
            });
        });
        if editing && inline {
            edit_in_place(ui, view.config, state, actions);
        }
    }
    let creating = state
        .editor
        .as_ref()
        .is_some_and(|e| e.target.is_none() && e.draft.gesture == entry.gesture);
    if creating && inline {
        edit_in_place(ui, view.config, state, actions);
    }
}

/// Réglages d'un geste appris : reconnaissance en direct, sensibilité,
/// exemples, nom, suppression. Les exemples ajoutés à un geste de base ont
/// les mêmes, sauf le nom ; les retirer laisse le geste et ses actions.
fn learned_controls(
    ui: &mut Ui,
    learned: &Learned<'_>,
    view: &View<'_>,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    let label = learned.label;
    let live = view
        .status
        .hands
        .iter()
        .flat_map(|h| {
            if learned.movement {
                h.top_dynamic.iter()
            } else {
                h.top_static.iter()
            }
        })
        .filter(|(l, _)| l == label)
        .map(|(_, p)| *p)
        .fold(0.0, f32::max);
    let trigger = if learned.movement {
        view.config.defaults.dynamic_threshold
    } else {
        view.config.defaults.static_enter
    };

    let mut value = state
        .sensitivity
        .as_ref()
        .filter(|(l, _)| l == label)
        .map_or(learned.sensitivity, |(_, v)| *v);
    let total = ui.available_width();
    let stacked = total < 560.0;
    let right = if stacked { total } else { 300.0 };
    let left = if stacked { total } else { total - right - 28.0 };
    let recognition = |ui: &mut Ui| {
        ui.set_width(left);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            ui.label(neu::text(
                "Reconnaissance en direct",
                12.0,
                Weight::Semibold,
                p.text,
            ));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let stats = if learned.movement {
                    format!("{:.0} % · {} prises", live * 100.0, learned.takes)
                } else {
                    format!(
                        "{:.0} % · {} exemples · {} prises",
                        live * 100.0,
                        learned.examples,
                        learned.takes
                    )
                };
                ui.add(egui::Label::new(neu::mono(stats, 11.5, p.muted)).truncate());
            });
        });
        let tint = (live >= trigger).then_some(p.ok);
        neu::bar(ui, live, 12.0, tint, Some(trigger));
    };
    let mut slider = None;
    let mut sensitivity = |ui: &mut Ui, slider: &mut Option<egui::Response>| {
        ui.set_width(right);
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.label(neu::text("Sensibilité", 12.0, Weight::Semibold, p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(neu::text(
                    "stricte — tolérante",
                    11.0,
                    Weight::Regular,
                    p.muted,
                ));
            });
        });
        let hint = if learned.movement {
            "Stricte : le mouvement doit être refait fidèlement. Tolérante : il est reconnu \
             plus facilement, au risque de se déclencher à tort."
        } else {
            "Stricte : la pose doit être refaite fidèlement. Tolérante : elle est reconnue \
             plus facilement, au risque de se déclencher à tort."
        };
        *slider = Some(
            neu::slider(ui, &mut value, learned.range.0..=learned.range.1, right)
                .on_hover_text(hint),
        );
    };
    if stacked {
        ui.vertical(recognition);
        ui.vertical(|ui| sensitivity(ui, &mut slider));
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            ui.vertical(recognition);
            ui.vertical(|ui| sensitivity(ui, &mut slider));
        });
    }
    if let Some(slider) = slider {
        if slider.changed() {
            state.sensitivity = Some((label.to_string(), value));
        }
        // Écrit une fois le curseur relâché, pas à chaque pixel.
        if (slider.drag_stopped() || (slider.changed() && !slider.dragged()))
            && (value - learned.sensitivity).abs() > 1e-3
        {
            actions.push(UiAction::SetPoseSensitivity {
                label: label.to_string(),
                value,
            });
        }
    }

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
        let more = if learned.movement {
            "Une prise de plus, pour un mouvement mal reconnu."
        } else {
            "Une prise de plus, pour une pose mal reconnue."
        };
        if ui
            .add_enabled(state.trainer.is_none(), Button::new("Ajouter des exemples"))
            .on_hover_text(more)
            .clicked()
        {
            state.trainer = Some(if learned.movement {
                Trainer::more_movement_examples(label, learned.name)
            } else if learned.extends_base {
                Trainer::base_examples(label, theme::gesture_name(label), false)
            } else {
                Trainer::more_examples(label, learned.name, learned.two_hands)
            });
        }
        if learned.extends_base {
            if ui.add(Button::new("Retirer mes exemples")).clicked() {
                state.confirm_delete = Some(label.to_string());
            }
        } else {
            if ui.add(Button::new("Renommer")).clicked() {
                state.renaming = Some((label.to_string(), learned.name.to_string()));
            }
            let delete = if learned.movement {
                "Supprimer le mouvement"
            } else {
                "Supprimer la pose"
            };
            if ui.add(Button::new(delete).color(p.danger)).clicked() {
                state.confirm_delete = Some(label.to_string());
            }
        }
    });
    if let Some((target, name)) = &mut state.renaming
        && target == label
    {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            field_label(ui, "Nouveau nom");
            neu::text_field(ui, name, 220.0, "");
            let valid = !name.trim().is_empty();
            if ui
                .add_enabled(valid, Button::new("Valider").primary())
                .clicked()
            {
                actions.push(UiAction::RenamePose {
                    label: label.to_string(),
                    name: name.trim().to_string(),
                });
                close = true;
            }
            if ui.add(Button::new("Annuler")).clicked() {
                close = true;
            }
        });
        if close {
            state.renaming = None;
        }
    }
    if state.confirm_delete.as_deref() == Some(label) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
            let this = learned.this();
            let text = match uses(view.config, label) {
                _ if learned.extends_base => {
                    "Retirer tes exemples ? Le geste sera reconnu comme avant.".to_string()
                }
                0 => format!("Supprimer {this} et ses exemples ?"),
                1 => format!("Supprimer {this}, ses exemples et son action ?"),
                n => format!("Supprimer {this}, ses exemples et ses {n} actions ?"),
            };
            ui.label(neu::text(text, 12.5, Weight::Semibold, p.accent));
            if learned.extends_base {
                if ui.add(Button::new("Retirer").color(p.danger)).clicked() {
                    actions.push(UiAction::RemoveExamples(label.to_string()));
                    state.confirm_delete = None;
                }
            } else if ui
                .add(Button::new("Supprimer définitivement").color(p.danger))
                .clicked()
            {
                actions.push(UiAction::DeletePose(label.to_string()));
                state.confirm_delete = None;
                state.editor = None;
            }
            if ui.add(Button::new("Garder")).clicked() {
                state.confirm_delete = None;
            }
        });
    }
    for resemblance in learned.resemblances {
        let other = theme::gesture_name(&resemblance.label);
        let share = resemblance.share * 100.0;
        let text = if learned.movement {
            format!(
                "Ressemble à « {other} » ({share:.0} % des prises). Si les deux ont une \
                 action, c'est le geste reconnu le premier qui part : fais ce mouvement \
                 main fermée, ou choisis-en un plus distinct."
            )
        } else if learned.extends_base {
            format!(
                "Ressemble à « {other} » ({share:.0} % des images des prises). Quand les \
                 deux se confondent, ce sont tes exemples qui l'emportent : resserre la \
                 sensibilité ou retire-les."
            )
        } else {
            format!(
                "Ressemble à « {other} » ({share:.0} % des images des prises). Quand les \
                 deux se confondent, c'est cette pose perso qui l'emporte : resserre la \
                 sensibilité ou choisis une pose plus distincte."
            )
        };
        note(ui, text, p.accent);
    }
}

/// Carte pleine largeur d'un geste appris : pose perso, geste de base
/// complété, ou mouvement perso.
fn learned_card(
    ui: &mut Ui,
    view: &View<'_>,
    entry: &Entry<'_>,
    learned: &Learned<'_>,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    Card::LARGE.gap(16.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(neu::text(
                theme::gesture_name(entry.gesture),
                16.0,
                Weight::Bold,
                p.text,
            ));
            let badge = if learned.extends_base {
                "+ tes exemples"
            } else if entry.two_hands {
                "perso · deux mains"
            } else {
                "perso"
            };
            neu::badge(ui, badge, p.accent, 22.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                associate_button(ui, entry, false, state);
                if let Some(role) = &entry.role {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(neu::text(role, 11.5, Weight::Regular, p.muted))
                                .truncate(),
                        );
                    });
                }
            });
        });
        learned_controls(ui, learned, view, state, actions);
        binding_rows(ui, view, entry, false, true, state, actions);
    });
}

/// Flèche ou rotation qui illustre un mouvement.
fn movement_icon(gesture: &str) -> (&'static Icon, f32, bool) {
    use std::f32::consts::{FRAC_PI_2, PI};
    match gesture {
        "swipe_right" => (&icons::ARROW, PI, false),
        "swipe_up" => (&icons::ARROW, FRAC_PI_2, false),
        "swipe_down" => (&icons::ARROW, -FRAC_PI_2, false),
        "rotate_cw" => (&icons::TURN, 0.0, false),
        "rotate_ccw" => (&icons::TURN, 0.0, true),
        _ => (&icons::ARROW, 0.0, false),
    }
}

/// Contenu d'une carte compacte : geste de base sans exemples, ou mouvement.
fn compact_card(
    ui: &mut Ui,
    view: &View<'_>,
    entry: &Entry<'_>,
    inline: bool,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.spacing_mut().item_spacing.x = 10.0;
        if !entry.is_static {
            let (icon, angle, flip) = movement_icon(entry.gesture);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            icon.paint_turned(ui.painter(), rect, p.accent, angle, flip);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            associate_button(ui, entry, true, state);
            if entry.is_static
                && neu::icon_button(ui, &icons::CAMERA, 32.0, 11.0, 14.0, p.text)
                    .on_hover_text(
                        "Ajouter des exemples : montre à Mudra ta façon de faire ce geste, \
                         en trois courtes prises. Il le reconnaîtra plus facilement.",
                    )
                    .clicked()
                && state.trainer.is_none()
            {
                let name = theme::gesture_name(entry.gesture);
                state.trainer = Some(Trainer::base_examples(entry.gesture, name, true));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                // Geste montré en ce moment : son nom passe au vert, suivi de
                // sa confiance s'il reste de la place avant les boutons.
                let live = live_confidence(view.status, entry.gesture, entry.is_static);
                let name = ui.add(
                    egui::Label::new(neu::text(
                        theme::gesture_name(entry.gesture),
                        15.0,
                        Weight::Bold,
                        if live.is_some() { p.ok } else { p.text },
                    ))
                    .truncate(),
                );
                if live.is_some() {
                    name.on_hover_text("Détecté en ce moment");
                }
                if entry.role.is_some() && ui.available_width() >= 56.0 {
                    neu::badge(ui, "réservé", p.muted, 20.0);
                }
                if let Some(confidence) = live
                    && ui.available_width() >= 40.0
                {
                    ui.label(neu::mono(
                        format!("{:.0} %", confidence * 100.0),
                        11.0,
                        p.ok,
                    ));
                }
            });
        });
    });
    if let Some(role) = &entry.role {
        let width = ui.available_width();
        neu::hollow(ui, width, 14.0, Margin::symmetric(14, 10), 0.0, |ui| {
            ui.label(neu::rich(
                ui.available_width(),
                18.0,
                &[(
                    format!("{role}.").as_str(),
                    theme::font(12.0, Weight::Regular),
                    p.soft,
                )],
            ));
        });
    }
    binding_rows(ui, view, entry, true, inline, state, actions);
}

/// Geste dont le formulaire d'action est ouvert.
fn edited_gesture<'a>(state: &'a GesturesState, config: &'a Config) -> Option<&'a str> {
    let editor = state.editor.as_ref()?;
    match editor.target {
        Some(index) => config.bindings.get(index).map(|b| b.gesture.as_str()),
        None => Some(editor.draft.gesture.as_str()),
    }
}

/// Formulaire d'une carte compacte : trop large pour elle, il s'ouvre dans sa
/// propre carte, sous la rangée.
fn editor_card(
    ui: &mut Ui,
    view: &View<'_>,
    gesture: &str,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    let creating = state.editor.as_ref().is_some_and(|e| e.target.is_none());
    Card::COMPACT.show(ui, |ui| {
        let name = theme::gesture_name(gesture);
        let title = if creating {
            format!("Nouvelle action pour « {name} »")
        } else {
            format!("Action de « {name} »")
        };
        ui.label(neu::text(title, 14.0, Weight::Bold, p.text));
        edit_in_place(ui, view.config, state, actions);
    });
}

fn section_header(ui: &mut Ui, title: &str, hint: &str, button: impl FnOnce(&mut Ui)) {
    let p = palette();
    ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.label(neu::caps(title, p.accent));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            button(ui);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(neu::text(hint, 12.5, Weight::Regular, p.muted)).truncate(),
                );
            });
        });
    });
}

fn accent_button(label: &str) -> Button<'_> {
    Button::new(label)
        .height(36.0, 12.0)
        .pad(16.0)
        .size(12.5)
        .bold()
        .color(palette().accent)
        .icon(&icons::PLUS, 12.0)
}

/// Grille des cartes compactes, sur une ou deux colonnes.
fn compact_grid(
    ui: &mut Ui,
    view: &View<'_>,
    gestures: &[&String],
    is_static: bool,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let columns = if ui.available_width() >= 700.0 { 2 } else { 1 };
    let cell = (ui.available_width() - GRID * (columns - 1) as f32) / columns as f32;
    for row in gestures.chunks(columns) {
        if row.len() == columns {
            Card::COMPACT.row(ui, columns, GRID, |ui, column| {
                let entry = Entry::new(view, row[column], is_static);
                compact_card(ui, view, &entry, false, state, actions);
            });
        } else {
            // Rangée incomplète : la carte garde la largeur d'une colonne.
            ui.scope(|ui| {
                ui.set_max_width(cell);
                Card::COMPACT.show(ui, |ui| {
                    let entry = Entry::new(view, row[0], is_static);
                    compact_card(ui, view, &entry, false, state, actions);
                });
            });
        }
        let edited = edited_gesture(state, view.config).map(str::to_owned);
        if let Some(gesture) = edited
            && row.iter().any(|g| **g == gesture)
        {
            editor_card(ui, view, &gesture, state, actions);
        }
    }
}

fn continuous_card(ui: &mut Ui, view: &View<'_>, actions: &mut Vec<UiAction>) {
    let p = palette();
    let config = view.config;
    Card::LARGE.gap(10.0).show(ui, |ui| {
        for (index, control) in config.continuous.iter().enumerate() {
            let active = view.status.continuous.as_deref() == Some(control.name.as_str());
            let width = ui.available_width();
            let pad = Margin {
                left: 14,
                right: 10,
                top: 8,
                bottom: 8,
            };
            neu::hollow(ui, width, 16.0, pad, 0.0, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(38.0);
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let mut enabled = control.enabled;
                    if neu::switch(ui, &mut enabled, Vec2::new(44.0, 24.0))
                        .on_hover_text(
                            "Désactivé, le contrôle garde ses réglages et sa pose redevient libre.",
                        )
                        .changed()
                    {
                        actions.push(UiAction::EditContinuous(
                            index,
                            ContinuousEdit::SetEnabled(enabled),
                        ));
                    }
                    let (label, color) = if control.enabled {
                        ("Actif", p.accent)
                    } else {
                        ("Inactif", p.muted)
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(44.0, 24.0),
                        Layout::left_to_right(Align::Center),
                        |ui| ui.label(neu::text(label, 12.0, Weight::Bold, color)),
                    );
                    let mut choices = role_choices(config, view.known, &control.enter_gesture);
                    // La pose du clic ne peut pas aussi ouvrir le contrôle.
                    if let Control::Mouse {
                        click: Some(click), ..
                    } = &control.control
                    {
                        choices.retain(|pose| *pose != click);
                    }
                    if let Some(pose) = pose_combo(ui, 170.0, &control.enter_gesture, &choices) {
                        actions.push(UiAction::EditContinuous(
                            index,
                            ContinuousEdit::EnterGesture(pose),
                        ));
                    }
                    ui.label(neu::text("maintenu", 12.5, Weight::Regular, p.muted));
                    let color = if control.enabled { p.text } else { p.muted };
                    ui.label(neu::text(
                        format!("contrôle « {} »", control.name),
                        12.5,
                        Weight::Bold,
                        color,
                    ));
                    if active {
                        neu::badge(ui, "en cours", p.ok, 20.0);
                    }
                });
            });
        }
    });
}

// ----- Colonne latérale -----

/// Main en pointillé de la vignette vide, dans une boîte de 160 points.
const GHOST_HAND: [[[f32; 2]; 5]; 5] = [
    [
        [80.0, 140.0],
        [56.0, 112.0],
        [44.0, 88.0],
        [38.0, 66.0],
        [34.0, 46.0],
    ],
    [
        [80.0, 140.0],
        [70.0, 96.0],
        [66.0, 64.0],
        [64.0, 40.0],
        [63.0, 20.0],
    ],
    [
        [80.0, 140.0],
        [86.0, 94.0],
        [90.0, 62.0],
        [92.0, 38.0],
        [93.0, 18.0],
    ],
    [
        [80.0, 140.0],
        [100.0, 98.0],
        [108.0, 70.0],
        [112.0, 50.0],
        [114.0, 32.0],
    ],
    [
        [80.0, 140.0],
        [112.0, 108.0],
        [124.0, 90.0],
        [132.0, 76.0],
        [138.0, 64.0],
    ],
];

/// Vignette : le squelette des mains suivies, sans l'image de la caméra.
fn skeleton_view(
    ui: &Ui,
    rect: Rect,
    status: &Status,
    frame: Option<&PreviewFrame>,
    caption: Option<&str>,
    banner: Option<&(String, Instant)>,
) {
    let p = palette();
    let painter = ui.painter_at(rect);
    painter.add(neu::well(rect, 18.0, 5.0));
    let hands = frame.filter(|f| !f.hands.is_empty());
    if let Some(frame) = hands {
        // Même cadrage que l'image de la caméra, ajusté à la cuvette.
        let area = rect.shrink(10.0);
        let aspect = frame.source_size.0 / frame.source_size.1.max(1.0);
        let mut size = Vec2::new(area.width(), area.width() / aspect);
        if size.y > area.height() {
            size = Vec2::new(area.height() * aspect, area.height());
        }
        let view = Rect::from_center_size(area.center(), size);
        let color = match status.state {
            EngineState::Armed => ORANGE,
            EngineState::Continuous => p.ok,
            EngineState::Locked | EngineState::Paused => p.soft,
        };
        for hand in &frame.hands {
            let points: Vec<Pos2> = hand
                .landmarks
                .iter()
                .map(|l| {
                    Pos2::new(
                        view.left() + l[0] / frame.source_size.0 * view.width(),
                        view.top() + l[1] / frame.source_size.1 * view.height(),
                    )
                })
                .collect();
            for (a, b) in BONES {
                painter.line_segment([points[a], points[b]], egui::Stroke::new(2.0, color));
            }
            for (i, point) in points.iter().enumerate() {
                let tip = matches!(i, 4 | 8 | 12 | 16 | 20);
                painter.circle_filled(
                    *point,
                    if tip { 3.2 } else { 2.2 },
                    if tip { p.text } else { color },
                );
            }
        }
    } else {
        let origin = rect.center() - Vec2::splat(80.0);
        for finger in GHOST_HAND {
            let path: Vec<Pos2> = finger
                .iter()
                .map(|q| origin + Vec2::new(q[0], q[1]))
                .collect();
            painter.extend(egui::Shape::dotted_line(&path, p.ghost, 7.0, 1.0));
        }
    }
    if let Some(caption) = caption {
        painter.text(
            Pos2::new(rect.center().x, rect.bottom() - 14.0),
            Align2::CENTER_BOTTOM,
            caption,
            theme::font(12.0, Weight::Semibold),
            p.muted,
        );
    }
    // Compte à rebours et signal de départ d'une prise.
    if let Some((text, since)) = banner
        && since.elapsed().as_secs_f32() < 1.2
    {
        let center = Pos2::new(rect.center().x, rect.top() + 46.0);
        painter.add(neu::raised(
            Rect::from_center_size(center, Vec2::splat(64.0)),
            32.0,
            4.0,
        ));
        painter.text(
            center,
            Align2::CENTER_CENTER,
            text,
            theme::font(30.0, Weight::Bold),
            p.danger,
        );
    }
}

fn side_card() -> Card {
    Card {
        radius: 24.0,
        depth: 8.0,
        pad: Vec2::splat(18.0),
        gap: 14.0,
        min_height: 0.0,
    }
}

fn preview_card(
    ui: &mut Ui,
    view: &View<'_>,
    frame: Option<&PreviewFrame>,
    pipeline_error: Option<&str>,
    record: &RecordForm,
) {
    let p = palette();
    let status = view.status;
    let paused = status.state == EngineState::Paused;
    let (dot, state) = if pipeline_error.is_some() {
        (p.danger, "Mudra arrêté")
    } else if paused {
        (p.muted, "En pause")
    } else {
        match status.capture.state {
            CaptureState::Streaming => (p.ok, "Caméra active"),
            CaptureState::Retrying { .. } => (p.danger, "Caméra indisponible"),
            _ => (p.accent, "Ouverture de la caméra…"),
        }
    };
    side_card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(neu::caps("APERÇU", p.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(neu::text(state, 11.5, Weight::Semibold, p.soft));
                neu::dot(ui, 7.0, dot);
            });
        });
        let (well, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 212.0), Sense::hover());
        let streaming = !paused && pipeline_error.is_none() && frame.is_some();
        let hands = frame.map_or(0, |f| f.hands.len());
        let caption = if record.active {
            None
        } else if !streaming {
            Some(state)
        } else if hands == 0 {
            Some("Aucune main détectée")
        } else {
            None
        };
        skeleton_view(
            ui,
            well,
            status,
            frame.filter(|_| streaming),
            caption,
            record.banner.as_ref(),
        );
        if record.active {
            // Avancement de la prise, au pied de la cuvette.
            let rail = Rect::from_min_max(
                Pos2::new(well.left() + 16.0, well.bottom() - 18.0),
                Pos2::new(well.right() - 16.0, well.bottom() - 10.0),
            );
            neu::bar_at(ui, rail, record.progress.unwrap_or(0.0), None, None);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let width = (ui.available_width() - 12.0) / 2.0;
            let enter = view.config.defaults.static_enter;
            let pose = status
                .hands
                .iter()
                .filter_map(|h| h.top_static.first())
                .filter(|(label, p)| label != "none" && *p >= enter)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(label, _)| theme::gesture_name(label));
            neu::hollow(ui, width, 16.0, Margin::symmetric(14, 12), 0.0, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(neu::text("Pose reconnue", 11.0, Weight::Regular, p.muted));
                ui.add(
                    egui::Label::new(neu::text(pose.unwrap_or("—"), 15.0, Weight::Bold, p.text))
                        .truncate(),
                );
            });
            neu::hollow(ui, width, 16.0, Margin::symmetric(14, 12), 0.0, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(neu::text("Flux", 11.0, Weight::Regular, p.muted));
                let fps = if paused {
                    "—".to_string()
                } else {
                    format!("{:.0} fps", status.capture.measured_fps)
                };
                ui.label(neu::mono(fps, 14.0, p.text));
            });
        });
    });
}

fn summary_card(ui: &mut Ui, view: &View<'_>) {
    let p = palette();
    let poses = view.known.static_.iter().filter(|g| g.as_str() != "none");
    let reserved = poses
        .clone()
        .filter(|g| reserved_role(view.config, g).is_some())
        .count();
    let active = view.config.bindings.iter().filter(|b| b.enabled).count();
    side_card().gap(12.0).show(ui, |ui| {
        ui.label(neu::caps("RÉSUMÉ", p.muted));
        let count = |n: usize| neu::mono(n.to_string(), 12.5, p.text);
        key_value(ui, "Poses", count(poses.count()));
        key_value(ui, "Actions actives", count(active));
        key_value(ui, "Réservées par Mudra", count(reserved));
    });
}

// ----- Onglet -----

fn title_row(ui: &mut Ui, state: &mut GesturesState, actions: &mut Vec<UiAction>) {
    let p = palette();
    let undo = Button::large("Annuler la dernière modification").icon(&icons::UNDO, 14.0);
    let mode = "Mode essai";
    let mode_width = ui
        .painter()
        .layout_no_wrap(mode.to_owned(), theme::font(12.5, Weight::Semibold), p.text)
        .size()
        .x;
    let right_width = undo.width(ui) + 16.0 + mode_width + 10.0 + 48.0;
    let origin = ui.available_rect_before_wrap().min;
    let total = ui.available_width();
    let left_width = (total - right_width - 24.0).max(200.0);
    let mut left = ui.new_child(
        UiBuilder::new()
            .id_salt("titre")
            .max_rect(Rect::from_min_size(
                origin,
                Vec2::new(left_width, f32::INFINITY),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    left.spacing_mut().item_spacing.y = 6.0;
    left.label(neu::text("Gestes et actions", 26.0, Weight::Bold, p.text));
    left.label(neu::rich(
        left_width,
        18.0,
        &[
            (
                "Chaque modification est enregistrée tout de suite dans ",
                theme::font(12.5, Weight::Regular),
                p.muted,
            ),
            ("config.toml", theme::mono(11.5), p.text),
            (
                " et appliquée sans redémarrer.",
                theme::font(12.5, Weight::Regular),
                p.muted,
            ),
        ],
    ));
    let bottom = left.min_rect().bottom().max(origin.y + 38.0);
    let row = Rect::from_min_max(origin, Pos2::new(origin.x + total, bottom));
    let strip = Rect::from_min_max(Pos2::new(row.left(), row.bottom() - 38.0), row.max);
    region(
        ui,
        "titre-boutons",
        strip,
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if neu::switch(ui, &mut state.dry_run, Vec2::new(48.0, 26.0))
                .on_hover_text(
                    "Les gestes sont reconnus et signalés ici, mais aucune action n'est \
                     exécutée. Pratique pendant les réglages.",
                )
                .changed()
            {
                actions.push(UiAction::SetDryRun(state.dry_run));
            }
            ui.add_space(10.0);
            ui.label(neu::text(mode, 12.5, Weight::Semibold, p.text));
            ui.add_space(16.0);
            if ui.add_enabled(state.can_undo, undo).clicked() {
                actions.push(UiAction::UndoEdit);
                state.editor = None;
            }
        },
    );
    ui.allocate_rect(row, Sense::hover());
}

/// Séance d'apprentissage en cours, dans la section qui la concerne.
fn training_session(
    ui: &mut Ui,
    context: &trainer::Context<'_>,
    movement: bool,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    if state
        .trainer
        .as_ref()
        .is_some_and(|t| t.movement == movement)
        && let Some(mut session) = state.trainer.take()
        && trainer::draw(ui, &mut session, context, actions)
    {
        state.trainer = Some(session);
    }
}

fn content(
    ui: &mut Ui,
    view: &View<'_>,
    record: &RecordForm,
    pipeline_error: Option<&str>,
    state: &mut GesturesState,
    actions: &mut Vec<UiAction>,
) {
    let p = palette();
    ui.spacing_mut().item_spacing.y = STACK;
    title_row(ui, state, actions);
    if state.editor.is_none()
        && let Some(error) = &state.error
    {
        ui.label(neu::text(error, 12.5, Weight::Semibold, p.danger));
    }
    activation_card(ui, view.config, view.known, actions);

    // Poses.
    section_header(ui, "POSES", "Une forme de main tenue un instant.", |ui| {
        if ui
            .add_enabled(state.trainer.is_none(), accent_button("Nouvelle pose"))
            .on_hover_text(
                "Apprends à Mudra une pose à toi, à une ou deux mains, en trois courtes prises.",
            )
            .clicked()
        {
            state.trainer = Some(Trainer::new_pose());
        }
    });
    let context = trainer::Context {
        status: view.status,
        config: view.config,
        pipeline_error,
        record,
        known: view.known,
    };
    training_session(ui, &context, false, state, actions);
    let poses: Vec<&String> = view
        .known
        .static_
        .iter()
        .filter(|g| g.as_str() != "none")
        .collect();
    let example_of = |g: &&String| view.custom.iter().find(|p| p.label == **g);
    // Les poses perso d'abord, puis les gestes de base complétés : ce sont
    // celles qu'on règle.
    let personal = poses
        .iter()
        .filter(|g| example_of(g).is_some_and(|p| !p.extends_base));
    let completed = poses
        .iter()
        .filter(|g| example_of(g).is_some_and(|p| p.extends_base));
    for gesture in personal.chain(completed) {
        let entry = Entry::new(view, gesture, true);
        if let Some(learned) = &entry.learned {
            learned_card(ui, view, &entry, learned, state, actions);
        }
    }
    let plain: Vec<&String> = poses
        .iter()
        .filter(|g| example_of(g).is_none())
        .copied()
        .collect();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = GRID;
        compact_grid(ui, view, &plain, true, state, actions);
    });

    // Mouvements.
    section_header(ui, "MOUVEMENTS", "Un geste bref de la main.", |ui| {
        if ui
            .add_enabled(state.trainer.is_none(), accent_button("Nouveau mouvement"))
            .on_hover_text(
                "Apprends à Mudra un mouvement à toi (un cercle, un va-et-vient…), en trois \
                 courtes prises.",
            )
            .clicked()
        {
            state.trainer = Some(Trainer::new_movement());
        }
    });
    training_session(ui, &context, true, state, actions);
    let movements = view.known.dynamic.iter().filter(|g| g.as_str() != "none");
    // Les mouvements perso d'abord : ce sont ceux qu'on règle.
    for gesture in movements.clone() {
        let entry = Entry::new(view, gesture, false);
        if let Some(learned) = &entry.learned {
            learned_card(ui, view, &entry, learned, state, actions);
        }
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = GRID;
        for gesture in movements {
            let entry = Entry::new(view, gesture, false);
            if entry.learned.is_none() {
                Card::COMPACT.show(ui, |ui| {
                    compact_card(ui, view, &entry, true, state, actions)
                });
            }
        }
    });

    if !view.config.continuous.is_empty() {
        section_header(
            ui,
            "CONTRÔLES CONTINUS",
            "La pose d'entrée se choisit ici ; les autres réglages restent dans le fichier \
             de configuration.",
            |_| {},
        );
        continuous_card(ui, view, actions);
    }
}

pub fn draw(ui: &mut Ui, body: Rect, model: &mut PreviewModel<'_>, actions: &mut Vec<UiAction>) {
    let view = View {
        config: model.config,
        status: model.status,
        known: model.known,
        custom: model.custom,
        moves: model.moves,
    };
    let record = &*model.record;
    let frame = model.frame;
    let pipeline_error = model.pipeline_error;
    let state: &mut GesturesState = model.gestures;

    let side = Rect::from_min_max(
        Pos2::new(body.right() - MARGIN - SIDE, body.top() + 14.0),
        body.max - Vec2::splat(MARGIN),
    );
    // La zone qui défile va du bord de la fenêtre à la colonne latérale : les
    // ombres des cartes ne sont pas rognées sur les côtés.
    let scroll = Rect::from_min_max(body.min, Pos2::new(side.left() - 4.0, body.bottom()));
    region(ui, "gestes", scroll, Layout::top_down(Align::Min), |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let origin = ui.available_rect_before_wrap().min;
                let total = ui.available_width();
                let width = total - MARGIN - (GUTTER - 4.0);
                let mut column = ui.new_child(
                    UiBuilder::new()
                        .id_salt("gestes-colonne")
                        .max_rect(Rect::from_min_size(
                            origin + Vec2::new(MARGIN, 8.0),
                            Vec2::new(width, f32::INFINITY),
                        ))
                        .layout(Layout::top_down(Align::Min)),
                );
                content(&mut column, &view, record, pipeline_error, state, actions);
                let bottom = column.min_rect().bottom() + MARGIN;
                ui.allocate_rect(
                    Rect::from_min_max(origin, Pos2::new(origin.x + total, bottom)),
                    Sense::hover(),
                );
            });
    });
    region(
        ui,
        "gestes-lateral",
        side,
        Layout::top_down(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = STACK;
            preview_card(ui, &view, frame, pipeline_error, record);
            summary_card(ui, &view);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_split_into_what_they_do_and_what_they_act_on() {
        let keys = Action::Keys {
            combo: "Win+Tab".into(),
        };
        assert_eq!(
            action_parts(&keys),
            ("Touches".to_string(), Some("Win+Tab".to_string()))
        );
        let launch = Action::Launch {
            path: r"C:\Program Files\Audacity\Audacity.exe".into(),
            args: Vec::new(),
            cwd: None,
        };
        assert_eq!(
            action_parts(&launch),
            (
                "Lancer".to_string(),
                Some(r"C:\Program Files\Audacity\Audacity.exe".to_string())
            )
        );
        let volume = Action::Volume {
            op: VolumeOp::Step,
            delta: 0.05,
            value: None,
            mode: VolumeMode::Api,
        };
        assert_eq!(action_parts(&volume), ("Volume +5 %".to_string(), None));
    }
}
