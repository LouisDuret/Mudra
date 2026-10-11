//! Fenêtre de Mudra : barre du haut commune aux deux onglets, et onglet
//! « Aperçu » (EF-71) avec l'image caméra, les points-clés, l'état du moteur,
//! les mains, les performances et le journal.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use egui::text::LayoutJob;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, Sense, Stroke, Ui,
    UiBuilder, Vec2,
};
use mudra_capture::CaptureState;
use mudra_config::edit::{ActivationEdit, BindingEdit, ContinuousEdit};
use mudra_config::{Action, ActivationMode, Config, Control, DeviceSelector, KnownGestures, Theme};
use mudra_gestures::EngineState;
use mudra_gestures::custom::CustomPose;
use mudra_gestures::motion::CustomMove;

use super::gestures::GesturesState;
use super::icons::{self, Icon, Ink};
use super::neu::{self, Button, Card, Segments};
use super::theme::{
    self, ORANGE, SCREEN_CHIP, SCREEN_LIVE, SCREEN_MUTED, SCREEN_OK, SCREEN_SOFT, SCREEN_TEXT, SUN,
    Weight, palette,
};
use crate::pipeline::{HandStatus, PreviewFrame, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    State,
    Gesture,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: String,
    pub kind: LogKind,
    pub text: String,
}

/// État de la prise en cours (onglet « Gestes »), alimenté par le pipeline.
#[derive(Default)]
pub struct RecordForm {
    /// Enregistrement en cours.
    pub active: bool,
    /// Dernier message (progression, erreur).
    pub status: Option<(String, bool)>,
    /// Avancement de la capture, entre 0 et 1.
    pub progress: Option<f32>,
    /// Grand message affiché sur l'image : compte à rebours, signal de départ.
    pub banner: Option<(String, Instant)>,
}

/// Onglet affiché dans la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Preview,
    Gestures,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// Onglet « Gestes » : modification d'un binding, écrite dans le fichier.
    EditBinding(BindingEdit),
    /// Onglet « Gestes » : mode et poses d'armement et de verrouillage.
    EditActivation(ActivationEdit),
    /// Onglet « Gestes » : contrôle continu d'indice donné dans la configuration.
    EditContinuous(usize, ContinuousEdit),
    UndoEdit,
    TestAction(Action),
    SetDryRun(bool),
    /// Enregistre une prise de la pose ou du mouvement perso `label`.
    TrainTake {
        label: String,
        movement: bool,
    },
    /// Apprend le geste à partir de ses prises ; `name` pour un nouveau geste.
    TrainFinish {
        label: String,
        name: Option<String>,
        two_hands: bool,
        movement: bool,
    },
    /// Supprime des prises abandonnées.
    DiscardTakes(Vec<PathBuf>),
    SetPoseSensitivity {
        label: String,
        value: f32,
    },
    RenamePose {
        label: String,
        name: String,
    },
    DeletePose(String),
    /// Retire les exemples ajoutés à un geste de base.
    RemoveExamples(String),
    TogglePause,
    Lock,
    CancelRecording,
    ReloadConfig,
    OpenConfigFolder,
    OpenLogs,
    /// La liste des caméras est affichée : à relire si elle date.
    RefreshCameras,
    /// Caméra choisie dans la barre du haut, écrite dans la configuration.
    SelectCamera(DeviceSelector),
    /// Bouton de la barre du haut : passe du thème sombre au clair, ou l'inverse.
    ToggleTheme,
}

pub struct PreviewModel<'a> {
    pub status: &'a Status,
    pub config: &'a Config,
    pub log: &'a VecDeque<LogEntry>,
    pub frame: Option<&'a PreviewFrame>,
    pub texture: Option<&'a egui::TextureHandle>,
    pub pipeline_error: Option<&'a str>,
    /// Caméras du PC, dans l'ordre de leur index ; `None` quand Mudra rejoue
    /// une image fixe au lieu de filmer.
    pub cameras: Option<&'a [String]>,
    pub record: &'a mut RecordForm,
    pub known: &'a KnownGestures,
    /// Poses personnalisées chargées.
    pub custom: &'a [CustomPose],
    /// Mouvements personnalisés chargés.
    pub moves: &'a [CustomMove],
    pub tab: &'a mut Tab,
    pub gestures: &'a mut GesturesState,
}

/// Hauteur de la barre du haut, marge de la fenêtre et écart entre colonnes.
const HEADER: f32 = 84.0;
pub(super) const MARGIN: f32 = 32.0;
pub(super) const GUTTER: f32 = 28.0;
/// Écart vertical entre deux cartes.
pub(super) const STACK: f32 = 22.0;
const SIDE: f32 = 340.0;

/// Liaisons entre points-clés (squelette de la main, Annexe A).
pub(super) const BONES: [(usize, usize); 21] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (0, 5),
    (5, 6),
    (6, 7),
    (7, 8),
    (5, 9),
    (9, 10),
    (10, 11),
    (11, 12),
    (9, 13),
    (13, 14),
    (14, 15),
    (15, 16),
    (13, 17),
    (17, 18),
    (18, 19),
    (19, 20),
    (0, 17),
];

/// Zone de la fenêtre mise en page à part, sans toucher au curseur du parent.
pub(super) fn region<R>(
    ui: &mut Ui,
    name: &str,
    rect: Rect,
    layout: Layout,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut child = ui.new_child(UiBuilder::new().id_salt(name).max_rect(rect).layout(layout));
    add(&mut child)
}

/// Texte qui ne peut pas élargir son panneau : un mot plus long que la ligne
/// (chemin de fichier) est coupé n'importe où au lieu de déborder.
fn wrapped(text: &str, font: FontId, color: Color32, width: f32) -> LayoutJob {
    // Largeur moyenne d'un caractère : un peu plus de la moitié du corps.
    let fits = (width / (font.size * 0.56)).max(8.0) as usize;
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width);
    job.wrap.break_anywhere = text.split_whitespace().any(|w| w.chars().count() > fits);
    job
}

/// Ligne « clé … valeur ».
pub(super) fn key_value(ui: &mut Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.scope(|ui| {
        // Une ligne de texte, pas la hauteur d'un bouton.
        ui.spacing_mut().interact_size.y = 18.0;
        ui.horizontal(|ui| {
            ui.label(neu::text(key, 12.5, Weight::Regular, palette().soft));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(value));
        });
    });
}

// ----- Barre du haut -----

/// Pastille d'état du moteur : icône, nom, et ce qu'il attend.
fn state_pill(ui: &mut Ui, model: &PreviewModel<'_>) {
    let p = palette();
    let status = model.status;
    let (icon, ink, name, name_color, detail): (&Icon, Color32, &str, Color32, Option<String>) =
        if model.pipeline_error.is_some() {
            (&icons::CROSS, p.danger, "Erreur", p.danger, None)
        } else {
            let name = status.state.label_fr();
            match status.state {
                EngineState::Paused => (
                    &icons::PAUSE,
                    p.muted,
                    name,
                    p.text,
                    Some("caméra libérée".to_string()),
                ),
                EngineState::Locked => (
                    &icons::LOCK,
                    p.muted,
                    name,
                    p.text,
                    status.idle.then(|| "veille : aucune main".to_string()),
                ),
                EngineState::Armed => (
                    &icons::UNLOCK,
                    ORANGE,
                    name,
                    p.accent,
                    status
                        .armed_remaining_ms
                        .map(|ms| format!("encore {:.0} s", (ms as f32 / 1000.0).ceil())),
                ),
                EngineState::Continuous => (
                    &icons::HAND,
                    p.ok,
                    name,
                    p.ok,
                    status
                        .continuous
                        .as_ref()
                        .map(|c| format!("contrôle « {c} »")),
                ),
            }
        };
    let painter = ui.painter();
    let name = painter.layout_no_wrap(name.to_owned(), theme::font(12.5, Weight::Bold), name_color);
    let detail =
        detail.map(|d| painter.layout_no_wrap(d, theme::font(12.0, Weight::Regular), p.muted));
    let width = 10.0
        + 24.0
        + 10.0
        + name.size().x
        + detail.as_ref().map_or(0.0, |d| 10.0 + d.size().x)
        + 14.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
    let painter = ui.painter();
    painter.add(neu::inset(rect, 18.0, 2.0));
    let disc = Rect::from_center_size(
        Pos2::new(rect.left() + 22.0, rect.center().y),
        Vec2::splat(24.0),
    );
    painter.add(neu::raised(disc, 12.0, 2.0));
    icon.paint_at(painter, disc.center(), 12.0, ink);
    let mut x = disc.right() + 10.0;
    let baseline = |height: f32| rect.center().y - height / 2.0;
    let name_width = name.size().x;
    painter.galley(Pos2::new(x, baseline(name.size().y)), name, name_color);
    x += name_width + 10.0;
    if let Some(detail) = detail {
        painter.galley(Pos2::new(x, baseline(detail.size().y)), detail, p.muted);
    }
}

/// Caméra en service ; ouvre la liste des caméras du PC quand il y en a une.
fn camera_chip(ui: &mut Ui, model: &PreviewModel<'_>, actions: &mut Vec<UiAction>) {
    let p = palette();
    let status = model.status;
    let (name, format) = match (&status.capture.state, &status.capture.format) {
        (CaptureState::Streaming, Some(f)) => (
            status.capture.device.clone(),
            Some(format!(
                "{} {}×{} · {:.0} fps",
                f.subtype, status.image_size.0, status.image_size.1, status.capture.measured_fps
            )),
        ),
        (CaptureState::Retrying { .. }, _) => ("Caméra indisponible".to_string(), None),
        (CaptureState::Opening, _) => ("Ouverture de la caméra…".to_string(), None),
        _ => ("Caméra libérée".to_string(), None),
    };
    let format_color = if status.capture.fps_warning {
        p.accent
    } else {
        p.muted
    };
    let painter = ui.painter();
    let name = painter.layout_no_wrap(name, theme::font(12.5, Weight::Semibold), p.text);
    let format = format.map(|f| painter.layout_no_wrap(f, theme::mono(11.0), format_color));
    let menu = model.cameras.is_some();
    let fixed = 14.0 + 16.0 + 12.0 + if menu { 12.0 + 10.0 } else { 0.0 } + 14.0;
    // Faute de place, le format disparaît, puis le nom se tronque, puis la
    // pastille elle-même : elle ne chevauche jamais l'état du moteur.
    let available = ui.available_width();
    if available < fixed + 48.0 {
        return;
    }
    let format =
        format.filter(|f| fixed + name.size().x.min(140.0) + 12.0 + f.size().x <= available);
    let format_width = format.as_ref().map_or(0.0, |f| 12.0 + f.size().x);
    let name_width = name.size().x.min(available - fixed - format_width);
    let width = fixed + name_width + format_width;
    let sense = if menu { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 40.0), sense);
    let painter = ui.painter();
    let depth = if menu && response.hovered() { 5.0 } else { 4.0 };
    painter.add(neu::raised(rect, 14.0, depth));
    let middle = rect.center().y;
    let mut x = rect.left() + 14.0;
    icons::CAMERA.paint_at(painter, Pos2::new(x + 8.0, middle), 16.0, p.muted);
    x += 16.0 + 12.0;
    let clip = Rect::from_min_max(
        Pos2::new(x, rect.top()),
        Pos2::new(x + name_width, rect.bottom()),
    );
    painter
        .with_clip_rect(clip.intersect(painter.clip_rect()))
        .galley(Pos2::new(x, middle - name.size().y / 2.0), name, p.text);
    x += name_width;
    if let Some(format) = format {
        x += 12.0;
        let format_width = format.size().x;
        painter.galley(
            Pos2::new(x, middle - format.size().y / 2.0),
            format,
            format_color,
        );
        x += format_width;
    }
    let Some(cameras) = model.cameras else {
        return;
    };
    icons::CHEVRON.paint_at(painter, Pos2::new(x + 12.0 + 5.0, middle), 10.0, p.muted);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let device = &model.config.camera.device;
    let chosen = match device {
        DeviceSelector::Auto => None,
        DeviceSelector::Index(i) => Some(*i as usize).filter(|i| *i < cameras.len()),
        DeviceSelector::Name(wanted) => {
            let wanted = wanted.to_lowercase();
            cameras
                .iter()
                .position(|n| n.to_lowercase().contains(&wanted))
        }
    };
    egui::Popup::menu(&response)
        .gap(8.0)
        .width(320.0)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            actions.push(UiAction::RefreshCameras);
            neu::menu_title(ui, "CAMÉRA");
            if neu::menu_item(ui, *device == DeviceSelector::Auto, "Automatique")
                .on_hover_text("La première caméra du PC qui fonctionne.")
                .clicked()
                && *device != DeviceSelector::Auto
            {
                actions.push(UiAction::SelectCamera(DeviceSelector::Auto));
            }
            for (index, name) in cameras.iter().enumerate() {
                let selected = chosen == Some(index);
                if neu::menu_item(ui, selected, name).clicked() && !selected {
                    // Deux caméras du même nom ne se distinguent que par leur index.
                    let twins = cameras.iter().filter(|n| *n == name).count() > 1;
                    actions.push(UiAction::SelectCamera(if twins {
                        DeviceSelector::Index(index as u32)
                    } else {
                        DeviceSelector::Name(name.clone())
                    }));
                }
            }
            let note = if cameras.is_empty() {
                Some("Aucune caméra détectée.".to_string())
            } else if chosen.is_none() && *device != DeviceSelector::Auto {
                let wanted = match device {
                    DeviceSelector::Name(name) => format!("« {name} »"),
                    _ => "La caméra choisie".to_string(),
                };
                Some(format!(
                    "{wanted} n'est pas branchée : Mudra prend la première disponible."
                ))
            } else {
                None
            };
            if let Some(note) = note {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(10.0);
                    ui.label(neu::text(note, 11.5, Weight::Regular, p.accent));
                });
            }
        });
}

fn top_bar(ui: &mut Ui, model: &PreviewModel<'_>, tab: &mut Tab, actions: &mut Vec<UiAction>) {
    let p = palette();
    let status = model.status;
    ui.spacing_mut().item_spacing.x = 0.0;

    let (logo, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
    ui.painter().add(neu::raised(logo, 22.0, 5.0));
    icons::HAND.paint_at(
        ui.painter(),
        logo.center(),
        22.0,
        Ink::Gradient(SUN, ORANGE),
    );
    ui.add_space(12.0);
    ui.label(neu::text("Mudra", 20.0, Weight::Bold, p.text));
    ui.add_space(22.0);

    let selected = match *tab {
        Tab::Preview => 0,
        Tab::Gestures => 1,
    };
    if let Some(picked) = Segments::TABS.show(ui, &["Aperçu", "Gestes"], Some(selected)) {
        *tab = if picked == 0 {
            Tab::Preview
        } else {
            Tab::Gestures
        };
    }
    ui.add_space(22.0);
    state_pill(ui, model);
    if model.gestures.dry_run {
        ui.add_space(8.0);
        neu::badge(ui, "mode essai", p.accent, 22.0).on_hover_text(
            "Les gestes sont reconnus et signalés, mais aucune action n'est exécutée.",
        );
    }
    if status.recording {
        ui.add_space(8.0);
        neu::badge(ui, "enregistrement", p.danger, 22.0);
    }

    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let paused = status.state == EngineState::Paused;
        let (label, icon) = if paused {
            ("Reprendre", &icons::PLAY)
        } else {
            ("Pause", &icons::PAUSE)
        };
        let pause = Button::new(label)
            .height(40.0, 14.0)
            .pad(18.0)
            .size(13.0)
            .bold()
            .icon(icon, 12.0);
        if ui
            .add(pause)
            .on_hover_text(format!(
                "Raccourci global : {}",
                model.config.activation.pause_hotkey
            ))
            .clicked()
        {
            actions.push(UiAction::TogglePause);
        }
        ui.add_space(14.0);
        let (icon, ink, hint) = match theme::current() {
            Theme::Dark => (&icons::SUN, SUN, "Passer au thème clair"),
            Theme::Light => (&icons::MOON, p.text, "Passer au thème sombre"),
        };
        if neu::icon_button(ui, icon, 40.0, 20.0, 18.0, ink)
            .on_hover_text(hint)
            .clicked()
        {
            actions.push(UiAction::ToggleTheme);
        }
        ui.add_space(14.0);
        if matches!(status.state, EngineState::Armed | EngineState::Continuous) {
            let lock = Button::new("Verrouiller")
                .height(40.0, 14.0)
                .pad(16.0)
                .size(13.0)
                .bold()
                .icon(&icons::LOCK, 13.0);
            if ui.add(lock).clicked() {
                actions.push(UiAction::Lock);
            }
            ui.add_space(14.0);
        }
        camera_chip(ui, model, actions);
    });
}

// ----- Image caméra -----

/// Ce que l'écran de la caméra affiche.
struct CameraView<'a> {
    status: &'a Status,
    config: &'a Config,
    frame: Option<&'a PreviewFrame>,
    texture: Option<&'a egui::TextureHandle>,
    pipeline_error: Option<&'a str>,
    /// Compte à rebours ou signal de départ d'une prise.
    banner: Option<&'a (String, Instant)>,
}

/// Étiquette posée sur l'image ; renvoie son rectangle, pour en aligner une autre.
fn screen_chip(
    painter: &egui::Painter,
    anchor: Pos2,
    align: Align2,
    dot: Option<Color32>,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let dot_width = if dot.is_some() { 14.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + dot_width + 22.0, 26.0);
    let rect = align.anchor_size(anchor, size);
    painter.rect_filled(rect, CornerRadius::same(13), SCREEN_CHIP);
    let mut x = rect.left() + 11.0;
    if let Some(dot) = dot {
        painter.circle_filled(Pos2::new(x + 3.5, rect.center().y), 3.5, dot);
        x += dot_width;
    }
    painter.galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    rect
}

fn screen_message(painter: &egui::Painter, rect: Rect, title: &str, detail: &str, color: Color32) {
    let center = rect.center();
    icons::HAND_THIN.paint_at(painter, center - Vec2::new(0.0, 34.0), 40.0, SCREEN_MUTED);
    painter.text(
        center + Vec2::new(0.0, 4.0),
        Align2::CENTER_CENTER,
        title,
        theme::font(13.0, Weight::Semibold),
        color,
    );
    if !detail.is_empty() {
        let galley = painter.layout(
            detail.to_owned(),
            theme::font(12.0, Weight::Regular),
            SCREEN_MUTED,
            rect.width() - 80.0,
        );
        painter.galley(
            Pos2::new(center.x - galley.size().x / 2.0, center.y + 18.0),
            galley,
            SCREEN_MUTED,
        );
    }
}

/// Couleur des tracés sur l'image, selon l'état du moteur.
fn skeleton_color(state: EngineState) -> Color32 {
    match state {
        EngineState::Armed => SUN,
        EngineState::Continuous => SCREEN_OK,
        EngineState::Locked | EngineState::Paused => SCREEN_TEXT,
    }
}

/// Dessine la caméra et ses tracés dans l'écran `screen`.
fn camera_view(ui: &Ui, screen: Rect, model: &CameraView<'_>) {
    const RADIUS: f32 = 18.0;
    let painter = ui.painter_at(screen);
    painter.add(neu::screen(screen, RADIUS));
    let status = model.status;

    let frame = match (model.frame, model.texture) {
        (Some(frame), Some(texture))
            if model.pipeline_error.is_none() && status.state != EngineState::Paused =>
        {
            Some((frame, texture))
        }
        _ => None,
    };
    let Some((frame, texture)) = frame else {
        if let Some(error) = model.pipeline_error {
            screen_message(
                &painter,
                screen,
                "Mudra ne peut pas démarrer",
                error,
                theme::SCREEN_LIVE,
            );
        } else if status.state == EngineState::Paused {
            let hotkey = &model.config.activation.pause_hotkey;
            screen_message(
                &painter,
                screen,
                "En pause",
                &format!("Caméra libérée · {hotkey} pour reprendre"),
                SCREEN_SOFT,
            );
        } else if let CaptureState::Retrying { error, .. } = &status.capture.state {
            screen_message(&painter, screen, "Caméra indisponible", error, SUN);
        } else {
            screen_message(
                &painter,
                screen,
                "En attente de la caméra…",
                "",
                SCREEN_MUTED,
            );
        }
        painter.add(neu::screen_shade(screen, RADIUS));
        corner_marks(&painter, screen);
        return;
    };

    // Image ajustée à l'écran en conservant ses proportions.
    let aspect = frame.width as f32 / frame.height.max(1) as f32;
    let mut size = Vec2::new(screen.width(), screen.width() / aspect);
    if size.y > screen.height() {
        size = Vec2::new(screen.height() * aspect, screen.height());
    }
    let rect = Rect::from_center_size(screen.center(), size);
    painter.add(neu::image(texture.id(), rect, RADIUS));
    painter.add(neu::screen_shade(screen, RADIUS));
    let to_screen = |p: [f32; 2]| {
        Pos2::new(
            rect.left() + p[0] / frame.source_size.0 * rect.width(),
            rect.top() + p[1] / frame.source_size.1 * rect.height(),
        )
    };

    // Zone active de la souris, pendant le contrôle correspondant (EF-51).
    for control in &model.config.continuous {
        if let Control::Mouse {
            active_zone: [x0, y0, x1, y1],
            ..
        } = control.control
            && status.continuous.as_deref() == Some(control.name.as_str())
        {
            let zone = Rect::from_min_max(
                Pos2::new(
                    rect.left() + x0 * rect.width(),
                    rect.top() + y0 * rect.height(),
                ),
                Pos2::new(
                    rect.left() + x1 * rect.width(),
                    rect.top() + y1 * rect.height(),
                ),
            );
            painter.rect_stroke(
                zone,
                CornerRadius::same(6),
                Stroke::new(1.5, SCREEN_OK),
                egui::StrokeKind::Inside,
            );
        }
    }

    let color = skeleton_color(status.state);
    for hand in &frame.hands {
        // Région d'intérêt du modèle de points-clés.
        let roi: Vec<Pos2> = hand.roi.iter().map(|p| to_screen(*p)).collect();
        for i in 0..4 {
            painter.line_segment(
                [roi[i], roi[(i + 1) % 4]],
                Stroke::new(1.0, SCREEN_MUTED.gamma_multiply(0.7)),
            );
        }
        let points: Vec<Pos2> = hand.landmarks.iter().map(|p| to_screen(*p)).collect();
        for (a, b) in BONES {
            painter.line_segment(
                [points[a], points[b]],
                Stroke::new(2.0, color.gamma_multiply(0.85)),
            );
        }
        for (i, p) in points.iter().enumerate() {
            let tip = matches!(i, 4 | 8 | 12 | 16 | 20);
            painter.circle_filled(
                *p,
                if tip { 4.0 } else { 3.0 },
                if tip { Color32::WHITE } else { color },
            );
        }
        // Étiquette au-dessus de la main.
        let top = points.iter().fold(points[0], |acc, p| {
            Pos2::new(acc.x.min(p.x), acc.y.min(p.y))
        });
        let side = if hand.right { "Droite" } else { "Gauche" };
        let text = match &hand.label {
            Some((label, p)) => {
                format!("{side} · {} {:.0} %", theme::gesture_name(label), p * 100.0)
            }
            None => side.to_string(),
        };
        let anchor = Pos2::new(top.x, (top.y - 10.0).max(rect.top() + 54.0));
        screen_chip(
            &painter,
            anchor,
            Align2::LEFT_BOTTOM,
            None,
            &text,
            theme::font(12.0, Weight::Semibold),
            SCREEN_TEXT,
        );
    }

    corner_marks(&painter, screen);
    let live = screen_chip(
        &painter,
        screen.left_top() + Vec2::splat(18.0),
        Align2::LEFT_TOP,
        Some(SCREEN_LIVE),
        "EN DIRECT",
        theme::font(11.5, Weight::Bold),
        SCREEN_TEXT,
    );
    screen_chip(
        &painter,
        live.right_top() + Vec2::new(8.0, 0.0),
        Align2::LEFT_TOP,
        None,
        &format!(
            "{}×{} · {:.0} fps",
            status.image_size.0, status.image_size.1, status.capture.measured_fps
        ),
        theme::mono(11.0),
        SCREEN_SOFT,
    );
    let hands = match frame.hands.len() {
        0 => "Aucune main détectée".to_string(),
        1 => "1 main suivie".to_string(),
        n => format!("{n} mains suivies"),
    };
    screen_chip(
        &painter,
        screen.right_bottom() - Vec2::splat(18.0),
        Align2::RIGHT_BOTTOM,
        None,
        &hands,
        theme::font(11.5, Weight::Regular),
        SCREEN_SOFT,
    );
    if status.capture.fps_warning {
        screen_chip(
            &painter,
            screen.left_bottom() + Vec2::new(18.0, -18.0),
            Align2::LEFT_BOTTOM,
            Some(SUN),
            &format!(
                "{:.0} fps au lieu de {:.0} : manque de lumière ?",
                status.capture.measured_fps, status.capture.target_fps
            ),
            theme::font(11.5, Weight::Semibold),
            SUN,
        );
    }

    // Compte à rebours et signal de départ de l'enregistrement.
    if let Some((text, since)) = model.banner
        && since.elapsed().as_secs_f32() < 1.2
    {
        let radius = (rect.height() * 0.24).min(44.0);
        let center = Pos2::new(rect.center().x, rect.top() + radius * 1.6);
        painter.circle_filled(center, radius, SCREEN_CHIP);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            text,
            theme::font(radius, Weight::Bold),
            SCREEN_LIVE,
        );
    }
}

/// Repères d'angle du cadre.
fn corner_marks(painter: &egui::Painter, screen: Rect) {
    const INSET: f32 = 24.0;
    const LEG: f32 = 32.0;
    let frame = screen.shrink(INSET);
    let stroke = Stroke::new(2.0, SUN.gamma_multiply(0.6));
    for (corner, dx, dy) in [
        (frame.left_top(), 1.0, 1.0),
        (frame.right_top(), -1.0, 1.0),
        (frame.right_bottom(), -1.0, -1.0),
        (frame.left_bottom(), 1.0, -1.0),
    ] {
        painter.add(egui::Shape::line(
            vec![
                corner + Vec2::new(0.0, LEG * dy),
                corner,
                corner + Vec2::new(LEG * dx, 0.0),
            ],
            stroke,
        ));
    }
}

// ----- Cartes de l'aperçu -----

/// Texte fait de morceaux de styles différents, replié à la largeur disponible.
fn rich_line(ui: &Ui, parts: &[(&str, FontId, Color32)]) -> LayoutJob {
    neu::rich(ui.available_width(), 19.5, parts)
}

fn engine_card(ui: &mut Ui, model: &PreviewModel<'_>) {
    let p = palette();
    let status = model.status;
    let activation = &model.config.activation;
    ui.label(neu::caps("MOTEUR DE GESTES", p.muted));
    let wake = theme::gesture_name(&activation.wake_gesture);
    let lock = theme::gesture_name(&activation.lock_gesture);
    // Avancement du maintien de la pose d'armement, main levée.
    let waking = status
        .hands
        .iter()
        .filter_map(|h| h.progress.as_ref())
        .filter(|(label, _, _)| *label == activation.wake_gesture)
        .map(|(_, progress, _)| *progress)
        .fold(0.0, f32::max);
    let (fraction, color, icon, ink): (f32, Color32, &Icon, Color32) = match status.state {
        EngineState::Paused => (0.0, ORANGE, &icons::PAUSE, p.muted),
        EngineState::Locked => (waking, ORANGE, &icons::LOCK, p.muted),
        EngineState::Armed => (
            status.armed_remaining_ms.map_or(1.0, |ms| {
                ms as f32 / activation.armed_duration_ms.max(1) as f32
            }),
            ORANGE,
            &icons::UNLOCK,
            p.accent,
        ),
        EngineState::Continuous => (1.0, p.ok, &icons::HAND, p.ok),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        neu::gauge(ui, fraction, color, icon, ink);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let body = theme::font(13.0, Weight::Regular);
            let strong = theme::font(13.0, Weight::Bold);
            let code = theme::mono(12.0);
            let wake_quoted = format!("« {wake} »");
            let lock_quoted = format!("« {lock} »");
            let hold = format!("{} ms", activation.wake_hold_ms);
            let always_on = activation.mode == ActivationMode::AlwaysOn;
            let parts: Vec<(&str, FontId, Color32)> = match status.state {
                EngineState::Paused => vec![
                    (
                        "Caméra libérée. Reprends par le menu ou le raccourci ",
                        body.clone(),
                        p.text,
                    ),
                    (activation.pause_hotkey.as_str(), code.clone(), p.text),
                    (".", body.clone(), p.text),
                ],
                EngineState::Locked => vec![
                    ("Montre ", body.clone(), p.text),
                    (wake_quoted.as_str(), strong.clone(), p.text),
                    (" pendant ", body.clone(), p.text),
                    (hold.as_str(), code.clone(), p.text),
                    (" pour armer.", body.clone(), p.text),
                ],
                EngineState::Armed if always_on => {
                    vec![("Les gestes agissent en permanence.", body.clone(), p.text)]
                }
                EngineState::Armed => vec![
                    ("Tous les gestes sont actifs. ", body.clone(), p.text),
                    (lock_quoted.as_str(), strong.clone(), p.text),
                    (" verrouille.", body.clone(), p.text),
                ],
                EngineState::Continuous => vec![(
                    "Un contrôle continu suit ta main. Relâche la pose pour sortir.",
                    body.clone(),
                    p.text,
                )],
            };
            ui.label(rich_line(ui, &parts));
            if let Some(ms) = status.armed_remaining_ms {
                ui.label(neu::mono(
                    format!("verrouillage dans {:.1} s", ms as f32 / 1000.0),
                    11.0,
                    p.muted,
                ));
            }
        });
    });
}

/// Ligne d'une probabilité : nom, pourcentage, et barre fine dessous.
fn probability_row(ui: &mut Ui, label: &str, p: f32, tint: Option<Color32>, mark: Option<f32>) {
    let pal = palette();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 21.0), Sense::hover());
    let painter = ui.painter();
    let clip = Rect::from_min_max(rect.min, rect.max - Vec2::new(40.0, 0.0));
    painter
        .with_clip_rect(clip.intersect(painter.clip_rect()))
        .text(
            rect.left_top(),
            Align2::LEFT_TOP,
            label,
            theme::font(11.5, Weight::Semibold),
            pal.text,
        );
    painter.text(
        rect.right_top() + Vec2::new(0.0, 1.0),
        Align2::RIGHT_TOP,
        format!("{:.0} %", p * 100.0),
        theme::mono(10.5),
        pal.muted,
    );
    let rail = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - 5.0),
        rect.right_bottom(),
    );
    neu::bar_at(ui, rail, p, tint, mark);
}

/// Hauteur d'une tuile de main : repliée, ou dépliée quand une main est suivie.
const TILE_FOLDED: f32 = 52.0;
const TILE_OPEN: f32 = 146.0;

fn hand_tile(
    ui: &mut Ui,
    width: f32,
    side: &str,
    hand: Option<&HandStatus>,
    model: &PreviewModel<'_>,
    open: bool,
) {
    let p = palette();
    let height = if open { TILE_OPEN } else { TILE_FOLDED };
    let pad = if open {
        Margin::symmetric(12, 10)
    } else {
        Margin::symmetric(12, 0)
    };
    neu::hollow(ui, width, 16.0, pad, height, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let Some(hand) = hand else {
            // Tuile éteinte : pastille et côté, centrés quand elle est repliée.
            let height = if open { 16.0 } else { TILE_FOLDED };
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
            let painter = ui.painter();
            let galley = painter.layout_no_wrap(
                side.to_owned(),
                theme::font(12.0, Weight::Semibold),
                p.muted,
            );
            let left = if open {
                rect.left()
            } else {
                rect.center().x - (16.0 + galley.size().x) / 2.0
            };
            painter.circle_filled(Pos2::new(left + 4.0, rect.center().y), 4.0, p.dot);
            painter.galley(
                Pos2::new(left + 16.0, rect.center().y - galley.size().y / 2.0),
                galley,
                p.muted,
            );
            return;
        };
        let enter = model.config.defaults.static_enter;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            neu::accent_dot(ui, 8.0);
            ui.label(neu::text(side, 12.0, Weight::Semibold, p.text));
            if let Some((_, progress, confirmed)) = &hand.progress {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = if *confirmed {
                        ("confirmée".to_string(), p.ok)
                    } else {
                        (format!("maintien {:.0} %", progress * 100.0), p.accent)
                    };
                    ui.label(neu::text(text, 10.5, Weight::Semibold, color));
                });
            }
        });
        ui.add_space(2.0);
        for (i, (label, probability)) in hand.top_static.iter().enumerate() {
            let tint = if label == "none" {
                Some(p.muted)
            } else if i == 0 && *probability >= enter {
                Some(p.ok)
            } else {
                None
            };
            probability_row(
                ui,
                theme::gesture_name(label),
                *probability,
                tint,
                (i == 0).then_some(enter),
            );
        }
        // Le mouvement le plus probable, s'il y en a un qui se dessine.
        if let Some((label, probability)) = hand
            .top_dynamic
            .iter()
            .find(|(label, p)| label != "none" && *p >= 0.05)
        {
            probability_row(
                ui,
                theme::gesture_name(label),
                *probability,
                None,
                Some(model.config.defaults.dynamic_threshold),
            );
        }
        ui.add_space(1.0);
        ui.add(
            egui::Label::new(neu::mono(
                format!("{:.1} paume/s · pince {:.2}", hand.speed, hand.pinch),
                10.0,
                p.muted,
            ))
            .truncate(),
        );
    });
}

/// Main à afficher dans la tuile de gauche, puis dans celle de droite. Deux
/// mains vues du même côté gardent chacune leur tuile.
fn hand_slots(hands: &[HandStatus]) -> [Option<&HandStatus>; 2] {
    let lefts: Vec<&HandStatus> = hands.iter().filter(|h| !h.right).collect();
    let rights: Vec<&HandStatus> = hands.iter().filter(|h| h.right).collect();
    [
        lefts.first().copied().or_else(|| rights.get(1).copied()),
        rights.first().copied().or_else(|| lefts.get(1).copied()),
    ]
}

fn hands_card(ui: &mut Ui, model: &PreviewModel<'_>) {
    let p = palette();
    ui.label(neu::caps("MAINS", p.muted));
    let hands = &model.status.hands;
    let slots = hand_slots(hands);
    let open = !hands.is_empty();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let width = (ui.available_width() - 12.0) / 2.0;
        hand_tile(ui, width, "Gauche", slots[0], model, open);
        hand_tile(ui, width, "Droite", slots[1], model, open);
    });
    let text = match (model.status.state, hands.len()) {
        (EngineState::Paused, _) => "Mudra est en pause.".to_string(),
        (_, 0) => "Aucune main dans le champ.".to_string(),
        (_, 1) => "1 main dans le champ.".to_string(),
        (_, n) => format!("{n} mains dans le champ."),
    };
    ui.label(neu::text(text, 12.5, Weight::Regular, p.soft));
}

fn config_card(ui: &mut Ui, model: &PreviewModel<'_>, actions: &mut Vec<UiAction>) {
    let p = palette();
    Card::LARGE.show(ui, |ui| {
        let buttons = [
            (
                Button::large("Recharger")
                    .pad(14.0)
                    .bold()
                    .icon(&icons::RELOAD, 14.0)
                    .icon_color(ORANGE),
                UiAction::ReloadConfig,
            ),
            (
                Button::large("Dossier")
                    .pad(14.0)
                    .bold()
                    .icon(&icons::FOLDER, 14.0),
                UiAction::OpenConfigFolder,
            ),
            (
                Button::large("Journaux")
                    .pad(14.0)
                    .bold()
                    .icon(&icons::FILE, 14.0),
                UiAction::OpenLogs,
            ),
        ];
        let buttons_width: f32 = buttons.iter().map(|(b, _)| b.width(ui)).sum::<f32>() + 10.0 * 2.0;
        let text_width = (ui.available_width() - buttons_width - 24.0).max(120.0);
        let summary = format!(
            "{} bindings · {} contrôles continus · poses : {} · dynamiques : {}",
            model.config.bindings.len(),
            model.config.continuous.len(),
            model.status.static_classifier,
            model.status.dynamic_classifier
        );
        let mut job = wrapped(
            &summary,
            theme::font(12.5, Weight::Regular),
            p.soft,
            text_width,
        );
        job.sections[0].format.line_height = Some(19.0);
        let galley = ui.painter().layout_job(job);
        let height = (13.0 + 8.0 + galley.size().y).max(38.0);
        let (row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let painter = ui.painter();
        let top = row.center().y - (13.0 + 8.0 + galley.size().y) / 2.0;
        painter.galley(
            Pos2::new(row.left(), top),
            painter.layout_job(neu::caps_job("CONFIGURATION", p.muted)),
            p.muted,
        );
        painter.galley(Pos2::new(row.left(), top + 13.0 + 8.0), galley, p.soft);
        region(
            ui,
            "configuration-boutons",
            row,
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (button, action) in buttons.into_iter().rev() {
                    if ui.add(button).clicked() {
                        actions.push(action);
                    }
                }
            },
        );
    });
}

fn performance_card(ui: &mut Ui, status: &Status) {
    let p = palette();
    Card::LARGE.pad(20.0, 20.0).gap(10.0).show(ui, |ui| {
        ui.label(neu::caps("PERFORMANCES", p.muted));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let width = (ui.available_width() - 10.0) / 2.0;
            for (label, fps) in [
                ("Capture", status.capture.measured_fps),
                ("Traitement", status.processed_fps),
            ] {
                neu::hollow(ui, width, 14.0, Margin::symmetric(12, 10), 0.0, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(neu::text(label, 11.0, Weight::Regular, p.muted));
                    let value = format!("{fps:.0} ");
                    ui.label(rich_line(
                        ui,
                        &[
                            (value.as_str(), theme::mono(16.0), p.text),
                            ("fps", theme::mono(11.0), p.muted),
                        ],
                    ));
                });
            }
        });
        let dropped = if status.dropped_percent < 1.0 {
            p.text
        } else {
            p.danger
        };
        key_value(
            ui,
            "Trames abandonnées",
            neu::mono(
                format!("{} ({:.2} %)", status.dropped, status.dropped_percent),
                12.0,
                dropped,
            ),
        );
        key_value(ui, "Inférence", neu::mono(status.provider, 12.0, p.text));
        key_value(
            ui,
            "Processus",
            neu::mono(
                format!(
                    "CPU {:.1} % · {:.0} Mo",
                    status.cpu_percent, status.memory_mb
                ),
                12.0,
                p.text,
            ),
        );
        let width = ui.available_width();
        neu::hollow(ui, width, 14.0, Margin::symmetric(12, 10), 0.0, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            const COLUMN: f32 = 46.0;
            let cells = |ui: &mut Ui,
                         height: f32,
                         name: (&str, FontId, Color32),
                         values: [(String, FontId, Color32); 3]| {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
                let painter = ui.painter();
                let clip = Rect::from_min_max(
                    rect.min,
                    Pos2::new(rect.right() - 3.0 * COLUMN - 4.0, rect.bottom()),
                );
                painter
                    .with_clip_rect(clip.intersect(painter.clip_rect()))
                    .text(
                        rect.left_center(),
                        Align2::LEFT_CENTER,
                        name.0,
                        name.1,
                        name.2,
                    );
                for (i, (text, font, color)) in values.into_iter().enumerate() {
                    let x = rect.right() - COLUMN * (2 - i) as f32;
                    painter.text(
                        Pos2::new(x, rect.center().y),
                        Align2::RIGHT_CENTER,
                        text,
                        font,
                        color,
                    );
                }
                response
            };
            let head = theme::font(10.5, Weight::Regular);
            cells(
                ui,
                18.0,
                ("Étage (ms)", head.clone(), p.muted),
                [
                    ("p50".to_string(), head.clone(), p.muted),
                    ("p95".to_string(), head.clone(), p.muted),
                    ("p99".to_string(), head.clone(), p.muted),
                ],
            );
            let shown: Vec<_> = status.latencies.iter().filter(|q| q.count > 0).collect();
            for (i, q) in shown.iter().enumerate() {
                // La dernière ligne est le total, de la trame à la décision.
                let weight = if i + 1 == shown.len() && shown.len() > 1 {
                    Weight::Bold
                } else {
                    Weight::Regular
                };
                let over = q.p95 > q.stage.budget_ms();
                let number = theme::mono(11.0);
                cells(
                    ui,
                    22.0,
                    (q.stage.label(), theme::font(11.5, weight), p.text),
                    [
                        (format!("{:.2}", q.p50), number.clone(), p.text),
                        (
                            format!("{:.2}", q.p95),
                            number.clone(),
                            if over { p.accent } else { p.text },
                        ),
                        (format!("{:.2}", q.p99), number.clone(), p.text),
                    ],
                )
                .on_hover_text(format!("budget p95 : {:.1} ms", q.stage.budget_ms()));
            }
            if shown.is_empty() {
                ui.label(neu::text(
                    "Pas encore de mesure.",
                    11.5,
                    Weight::Regular,
                    p.muted,
                ));
            }
        });
    });
}

/// Journal : occupe toute la hauteur restante de la colonne.
fn log_card(ui: &mut Ui, log: &VecDeque<LogEntry>) {
    let p = palette();
    let size = Vec2::new(ui.available_width(), ui.available_height().max(140.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().add(neu::raised(rect, 22.0, 8.0));
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(20.0, 20.0),
        rect.max - Vec2::new(20.0, 4.0),
    );
    region(ui, "journal", inner, Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.horizontal(|ui| {
            ui.label(neu::caps("JOURNAL", p.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(neu::text(
                    "plus récent en haut",
                    11.0,
                    Weight::Regular,
                    p.muted,
                ));
            });
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 6.0);
                if log.is_empty() {
                    ui.label(neu::text(
                        "Aucun événement pour l'instant.",
                        12.0,
                        Weight::Regular,
                        p.muted,
                    ));
                }
                for entry in log.iter().rev().take(60) {
                    let (dot, color, weight) = match entry.kind {
                        LogKind::Info => (p.dot, p.muted, Weight::Regular),
                        LogKind::State => (ORANGE, p.text, Weight::Bold),
                        LogKind::Gesture => (p.ok, p.text, Weight::Bold),
                        LogKind::Error => (p.danger, p.danger, Weight::Semibold),
                    };
                    ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                        ui.label(neu::mono(&entry.time, 11.0, p.muted));
                        let (mark, _) =
                            ui.allocate_exact_size(Vec2::new(6.0, 16.0), Sense::hover());
                        ui.painter().circle_filled(
                            Pos2::new(mark.center().x, mark.top() + 8.0),
                            3.0,
                            dot,
                        );
                        let job = wrapped(
                            &entry.text,
                            theme::font(12.0, weight),
                            color,
                            ui.available_width(),
                        );
                        ui.add(egui::Label::new(job).wrap());
                    });
                }
                ui.add_space(40.0);
            });
    });
    // La liste s'efface vers le bas de la carte.
    let fade = Rect::from_min_max(
        Pos2::new(inner.left(), rect.bottom() - 52.0),
        Pos2::new(inner.right(), rect.bottom() - 4.0),
    );
    ui.painter().add(neu::fade(fade, p.bg));
}

fn preview_tab(ui: &mut Ui, body: Rect, model: &mut PreviewModel<'_>, actions: &mut Vec<UiAction>) {
    let content = Rect::from_min_max(
        body.min + Vec2::new(MARGIN, 8.0),
        body.max - Vec2::splat(MARGIN),
    );
    let side = Rect::from_min_max(
        Pos2::new(content.right() - SIDE, content.top()),
        content.max,
    );
    let main = Rect::from_min_max(
        content.min,
        Pos2::new(side.left() - GUTTER, content.bottom()),
    );

    let main_column = |ui: &mut Ui, actions: &mut Vec<UiAction>| {
        ui.spacing_mut().item_spacing.y = STACK;
        let width = ui.available_width();
        // Moteur et mains côte à côte, ou l'un sous l'autre si la place manque.
        let side_by_side = width >= 720.0;
        // Place gardée sous l'image pour les cartes, tuiles des mains
        // dépliées : l'image ne change pas de taille quand une main arrive.
        let hands = 94.0 + TILE_OPEN;
        let below = if side_by_side {
            hands + 98.0 + 2.0 * STACK
        } else {
            118.0 + hands + 98.0 + 3.0 * STACK
        };
        let ideal = (width - 24.0) * 9.0 / 16.0 + 24.0;
        let height = ideal.min(main.height() - below).max(260.0);
        let (card, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter().add(neu::raised(card, 26.0, 8.0));
        camera_view(
            ui,
            card.shrink(12.0),
            &CameraView {
                status: model.status,
                config: model.config,
                frame: model.frame,
                texture: model.texture,
                pipeline_error: model.pipeline_error,
                banner: model.record.banner.as_ref(),
            },
        );
        if side_by_side {
            Card::LARGE.row(ui, 2, STACK, |ui, column| {
                if column == 0 {
                    engine_card(ui, model);
                } else {
                    hands_card(ui, model);
                }
            });
        } else {
            Card::LARGE.show(ui, |ui| engine_card(ui, model));
            Card::LARGE.show(ui, |ui| hands_card(ui, model));
        }
        config_card(ui, model, actions);
    };
    // La colonne défile si la fenêtre est trop basse pour elle. Sa zone va du
    // bord de la fenêtre à la colonne latérale : les ombres ne sont pas rognées.
    let scroll = Rect::from_min_max(body.min, Pos2::new(side.left() - 4.0, body.bottom()));
    region(
        ui,
        "principal",
        scroll,
        Layout::top_down(Align::Min),
        |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let origin = ui.available_rect_before_wrap().min;
                    let mut column = ui.new_child(
                        UiBuilder::new()
                            .id_salt("principal-colonne")
                            .max_rect(Rect::from_min_size(
                                Pos2::new(main.left(), origin.y + 8.0),
                                Vec2::new(main.width(), f32::INFINITY),
                            ))
                            .layout(Layout::top_down(Align::Min)),
                    );
                    main_column(&mut column, actions);
                    let bottom = column.min_rect().bottom() + MARGIN;
                    ui.allocate_rect(
                        Rect::from_min_max(origin, Pos2::new(scroll.right(), bottom)),
                        Sense::hover(),
                    );
                });
        },
    );

    region(ui, "lateral", side, Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = STACK;
        performance_card(ui, model.status);
        log_card(ui, model.log);
    });
}

/// Dessine toute la fenêtre et renvoie les actions demandées.
pub fn draw(ui: &mut Ui, model: &mut PreviewModel<'_>) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let mut tab = *model.tab;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(palette().bg))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let header = Rect::from_min_size(full.min, Vec2::new(full.width(), HEADER));
            let body = Rect::from_min_max(Pos2::new(full.left(), header.bottom()), full.max);
            match tab {
                Tab::Preview => preview_tab(ui, body, model, &mut actions),
                Tab::Gestures => super::gestures::draw(ui, body, model, &mut actions),
            }
            region(
                ui,
                "barre",
                header.shrink2(Vec2::new(MARGIN, 0.0)),
                Layout::left_to_right(Align::Center),
                |ui| top_bar(ui, model, &mut tab, &mut actions),
            );
        });
    *model.tab = tab;
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(track: u32, right: bool) -> HandStatus {
        HandStatus {
            track,
            right,
            top_static: Vec::new(),
            top_dynamic: Vec::new(),
            progress: None,
            speed: 0.0,
            pinch: 0.0,
        }
    }

    #[test]
    fn each_hand_gets_a_tile_even_when_both_look_alike() {
        let track = |slot: Option<&HandStatus>| slot.map(|h| h.track);
        let slots = |hands: &[HandStatus]| {
            let [left, right] = hand_slots(hands);
            [track(left), track(right)]
        };
        assert_eq!(slots(&[]), [None, None]);
        assert_eq!(slots(&[hand(1, true)]), [None, Some(1)]);
        assert_eq!(slots(&[hand(1, false)]), [Some(1), None]);
        assert_eq!(slots(&[hand(1, true), hand(2, false)]), [Some(2), Some(1)]);
        // Deux mains annoncées du même côté : la seconde prend l'autre tuile.
        assert_eq!(slots(&[hand(1, true), hand(2, true)]), [Some(2), Some(1)]);
        assert_eq!(slots(&[hand(1, false), hand(2, false)]), [Some(1), Some(2)]);
    }
}
