//! Fenêtre d'aperçu et de diagnostic (EF-71) : image caméra, points-clés et
//! région d'intérêt, probabilités, état du moteur, performances et journal.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, Ui, Vec2,
};
use mudra_capture::CaptureState;
use mudra_config::edit::{ActivationEdit, BindingEdit, ContinuousEdit};
use mudra_config::{Action, Config, Control, KnownGestures};
use mudra_gestures::EngineState;
use mudra_gestures::custom::CustomPose;

use super::gestures::GesturesState;
use super::theme::{self, ACCENT, AMBER, BACKGROUND, BORDER, CARD, GREEN, MUTED, PANEL, RED, TEXT};
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

/// État de l'assistant d'enregistrement.
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
    /// Enregistre une prise de la pose perso `label`.
    TrainTake {
        label: String,
    },
    /// Apprend la pose à partir de ses prises ; `name` pour une nouvelle pose.
    TrainFinish {
        label: String,
        name: Option<String>,
        two_hands: bool,
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
}

pub struct PreviewModel<'a> {
    pub status: &'a Status,
    pub config: &'a Config,
    pub log: &'a VecDeque<LogEntry>,
    pub frame: Option<&'a PreviewFrame>,
    pub texture: Option<&'a egui::TextureHandle>,
    pub pipeline_error: Option<&'a str>,
    pub record: &'a mut RecordForm,
    pub known: &'a KnownGestures,
    /// Poses personnalisées chargées.
    pub custom: &'a [CustomPose],
    pub tab: &'a mut Tab,
    pub gestures: &'a mut GesturesState,
}

/// Liaisons entre points-clés (squelette de la main, Annexe A).
const BONES: [(usize, usize); 21] = [
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

fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title.to_uppercase())
                    .small()
                    .color(MUTED)
                    .strong(),
            );
            ui.add_space(2.0);
            add(ui)
        })
        .inner
}

/// Petit texte qui ne peut pas élargir son panneau : un mot plus long que la
/// ligne (chemin de fichier) est coupé n'importe où au lieu de déborder.
fn wrapped(ui: &Ui, text: &str, color: Color32) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, ui.available_width());
    job.wrap.break_anywhere = text.split_whitespace().any(|w| w.chars().count() > 24);
    job
}

fn pill(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.8)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).strong());
        });
}

/// Barre de probabilité avec libellé et pourcentage.
pub(super) fn probability_bar(
    ui: &mut Ui,
    label: &str,
    p: f32,
    color: Color32,
    threshold: Option<f32>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), BACKGROUND);
    let mut fill = rect;
    fill.set_width(rect.width() * p.clamp(0.0, 1.0));
    painter.rect_filled(fill, CornerRadius::same(4), color.gamma_multiply(0.45));
    if let Some(t) = threshold {
        // Repère du seuil d'entrée.
        let x = rect.left() + rect.width() * t;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, MUTED),
        );
    }
    let font = FontId::proportional(12.5);
    painter.text(
        rect.left_center() + Vec2::new(7.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        font.clone(),
        TEXT,
    );
    painter.text(
        rect.right_center() - Vec2::new(7.0, 0.0),
        Align2::RIGHT_CENTER,
        format!("{:.0} %", p * 100.0),
        font,
        TEXT,
    );
}

fn key_value(ui: &mut Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(value));
    });
}

fn top_bar(ui: &mut Ui, model: &PreviewModel<'_>, tab: &mut Tab, actions: &mut Vec<UiAction>) {
    let status = model.status;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mudra").heading().strong());
        ui.add_space(6.0);
        ui.selectable_value(tab, Tab::Preview, "Aperçu");
        ui.selectable_value(tab, Tab::Gestures, "Gestes");
        ui.add_space(6.0);
        if model.gestures.dry_run {
            pill(ui, "mode essai", AMBER);
        }
        if model.pipeline_error.is_some() {
            pill(ui, "Erreur", RED);
        } else {
            pill(
                ui,
                status.state.label_fr(),
                theme::state_color(status.state),
            );
        }
        if let Some(name) = &status.continuous {
            pill(ui, &format!("contrôle « {name} »"), GREEN);
        }
        if status.recording {
            pill(ui, "⏺ enregistrement", RED);
        }
        if status.idle && status.state != EngineState::Paused {
            ui.label(RichText::new("veille : aucune main").color(MUTED));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let paused = status.state == EngineState::Paused;
            let label = if paused {
                "▶  Reprendre"
            } else {
                "⏸  Pause"
            };
            if ui
                .button(label)
                .on_hover_text(format!(
                    "Raccourci global : {}",
                    model.config.activation.pause_hotkey
                ))
                .clicked()
            {
                actions.push(UiAction::TogglePause);
            }
            if matches!(status.state, EngineState::Armed | EngineState::Continuous)
                && ui.button("Verrouiller").clicked()
            {
                actions.push(UiAction::Lock);
            }
            let camera = match (&status.capture.state, &status.capture.format) {
                (CaptureState::Streaming, Some(f)) => format!(
                    "{} · {} {}×{} · {:.0} fps",
                    status.capture.device,
                    f.subtype,
                    status.image_size.0,
                    status.image_size.1,
                    status.capture.measured_fps
                ),
                (CaptureState::Retrying { .. }, _) => "caméra indisponible".to_string(),
                (CaptureState::Opening, _) => "ouverture de la caméra…".to_string(),
                _ => "caméra libérée".to_string(),
            };
            let color = if status.capture.fps_warning {
                AMBER
            } else {
                MUTED
            };
            // Tronqué plutôt que de chevaucher les pastilles d'état quand la place manque.
            ui.add(egui::Label::new(RichText::new(camera).color(color)).truncate());
        });
    });
}

fn engine_card(ui: &mut Ui, model: &PreviewModel<'_>) {
    let status = model.status;
    card(ui, "Moteur de gestes", |ui| {
        let hint = match status.state {
            EngineState::Paused => {
                "Caméra libérée. Reprends par le menu ou le raccourci.".to_string()
            }
            EngineState::Locked => format!(
                "Montre « {} » pendant {} ms pour armer.",
                theme::gesture_name(&model.config.activation.wake_gesture),
                model.config.activation.wake_hold_ms
            ),
            EngineState::Armed => format!(
                "Tous les gestes sont actifs. « {} » verrouille.",
                theme::gesture_name(&model.config.activation.lock_gesture)
            ),
            EngineState::Continuous => {
                "Un contrôle continu suit ta main. Relâche la pose pour sortir.".to_string()
            }
        };
        ui.label(RichText::new(hint).color(MUTED));
        if let Some(ms) = status.armed_remaining_ms {
            let total = model.config.activation.armed_duration_ms.max(1) as f32;
            ui.add(
                egui::ProgressBar::new((ms as f32 / total).clamp(0.0, 1.0))
                    .desired_height(6.0)
                    .fill(ACCENT),
            );
            ui.label(
                RichText::new(format!("verrouillage dans {:.1} s", ms as f32 / 1000.0))
                    .small()
                    .color(MUTED),
            );
        }
    });
}

fn hand_card(ui: &mut Ui, hand: &HandStatus, model: &PreviewModel<'_>) {
    let side = if hand.right {
        "Main droite"
    } else {
        "Main gauche"
    };
    card(ui, side, |ui| {
        let enter = model.config.defaults.static_enter;
        for (i, (label, p)) in hand.top_static.iter().enumerate() {
            let color = if label == "none" {
                theme::GRAY
            } else if i == 0 && *p >= enter {
                GREEN
            } else {
                ACCENT
            };
            probability_bar(
                ui,
                theme::gesture_name(label),
                *p,
                color,
                (i == 0).then_some(enter),
            );
        }
        if let Some((label, progress, confirmed)) = &hand.progress {
            ui.horizontal(|ui| {
                let text = if *confirmed {
                    "confirmée"
                } else {
                    "maintien…"
                };
                ui.label(
                    RichText::new(format!("{} : {text}", theme::gesture_name(label)))
                        .small()
                        .color(MUTED),
                );
                ui.add(
                    egui::ProgressBar::new(*progress)
                        .desired_height(6.0)
                        .fill(if *confirmed { GREEN } else { AMBER }),
                );
            });
        }
        if !hand.top_dynamic.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("DYNAMIQUE").small().color(MUTED).strong());
            let threshold = model.config.defaults.dynamic_threshold;
            for (i, (label, p)) in hand.top_dynamic.iter().enumerate() {
                let color = if label == "none" { theme::GRAY } else { AMBER };
                probability_bar(
                    ui,
                    theme::gesture_name(label),
                    *p,
                    color,
                    (i == 0).then_some(threshold),
                );
            }
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new(format!(
                "vitesse {:.1} paume/s · pincement {:.2}",
                hand.speed, hand.pinch
            ))
            .small()
            .color(MUTED),
        );
    });
}

fn performance_card(ui: &mut Ui, status: &Status) {
    card(ui, "Performances", |ui| {
        let dropped = if status.dropped_percent < 1.0 {
            TEXT
        } else {
            RED
        };
        key_value(
            ui,
            "Capture",
            format!("{:.0} fps", status.capture.measured_fps),
        );
        key_value(ui, "Traitement", format!("{:.0} fps", status.processed_fps));
        key_value(
            ui,
            "Trames abandonnées",
            RichText::new(format!(
                "{} ({:.2} %)",
                status.dropped, status.dropped_percent
            ))
            .color(dropped),
        );
        key_value(ui, "Inférence", status.provider);
        key_value(
            ui,
            "Processus",
            format!(
                "CPU {:.1} % · {:.0} Mo",
                status.cpu_percent, status.memory_mb
            ),
        );
        ui.add_space(4.0);
        egui::Grid::new("latences")
            .num_columns(4)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                for header in ["étage (ms)", "p50", "p95", "p99"] {
                    ui.label(RichText::new(header).small().color(MUTED));
                }
                ui.end_row();
                for q in &status.latencies {
                    if q.count == 0 {
                        continue;
                    }
                    ui.label(RichText::new(q.stage.label()).small());
                    ui.label(RichText::new(format!("{:.2}", q.p50)).small().monospace());
                    let over = q.p95 > q.stage.budget_ms();
                    ui.label(
                        RichText::new(format!("{:.2}", q.p95))
                            .small()
                            .monospace()
                            .color(if over { AMBER } else { TEXT }),
                    )
                    .on_hover_text(format!("budget p95 : {:.1} ms", q.stage.budget_ms()));
                    ui.label(RichText::new(format!("{:.2}", q.p99)).small().monospace());
                    ui.end_row();
                }
            });
    });
}

fn log_card(ui: &mut Ui, log: &VecDeque<LogEntry>) {
    card(ui, "Journal", |ui| {
        if log.is_empty() {
            ui.label(RichText::new("Aucun événement pour l'instant.").color(MUTED));
        }
        for entry in log.iter().rev().take(40) {
            let color = match entry.kind {
                LogKind::Info => MUTED,
                LogKind::State => ACCENT,
                LogKind::Gesture => GREEN,
                LogKind::Error => RED,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&entry.time).small().monospace().color(MUTED));
                ui.label(wrapped(ui, &entry.text, color));
            });
        }
    });
}

fn centered_message(ui: &Ui, rect: Rect, title: &str, detail: &str, color: Color32) {
    let painter = ui.painter();
    painter.text(
        rect.center() - Vec2::new(0.0, 14.0),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(22.0),
        color,
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 16.0),
        Align2::CENTER_CENTER,
        detail,
        FontId::proportional(13.5),
        MUTED,
    );
}

/// Ce que l'image caméra affiche ; sert aussi à la vignette de l'onglet « Gestes ».
pub(super) struct CameraView<'a> {
    pub status: &'a Status,
    pub config: &'a Config,
    pub frame: Option<&'a PreviewFrame>,
    pub texture: Option<&'a egui::TextureHandle>,
    pub pipeline_error: Option<&'a str>,
    /// Compte à rebours ou signal de départ d'une prise.
    pub banner: Option<&'a (String, Instant)>,
}

/// Dessine la caméra et ses tracés dans toute la place disponible.
pub(super) fn camera_view(ui: &mut Ui, model: &CameraView<'_>) {
    let available = ui.available_rect_before_wrap();
    ui.allocate_rect(available, Sense::hover());
    let painter = ui.painter_at(available);
    painter.rect_filled(available, CornerRadius::same(10), BACKGROUND);
    let status = model.status;

    if let Some(error) = model.pipeline_error {
        centered_message(ui, available, "Mudra ne peut pas démarrer", error, RED);
        return;
    }
    if status.state == EngineState::Paused {
        let hotkey = &model.config.activation.pause_hotkey;
        centered_message(
            ui,
            available,
            "En pause",
            &format!("Caméra libérée · {hotkey} pour reprendre"),
            MUTED,
        );
        return;
    }
    let (Some(frame), Some(texture)) = (model.frame, model.texture) else {
        match &status.capture.state {
            CaptureState::Retrying { error, .. } => {
                centered_message(ui, available, "Caméra indisponible", error, AMBER)
            }
            _ => centered_message(ui, available, "En attente de la caméra…", "", MUTED),
        }
        return;
    };

    // Image ajustée au cadre en conservant ses proportions.
    let aspect = frame.width as f32 / frame.height.max(1) as f32;
    let mut size = Vec2::new(available.width(), available.width() / aspect);
    if size.y > available.height() {
        size = Vec2::new(available.height() * aspect, available.height());
    }
    let rect = Rect::from_center_size(available.center(), size);
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(texture.id(), rect, uv, Color32::WHITE);
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
                CornerRadius::same(4),
                Stroke::new(1.5, GREEN),
                egui::StrokeKind::Inside,
            );
        }
    }

    let color = theme::state_color(status.state);
    let color = if status.state == EngineState::Locked {
        TEXT
    } else {
        color
    };
    for hand in &frame.hands {
        // Région d'intérêt du modèle de points-clés.
        let roi: Vec<Pos2> = hand.roi.iter().map(|p| to_screen(*p)).collect();
        for i in 0..4 {
            painter.line_segment(
                [roi[i], roi[(i + 1) % 4]],
                Stroke::new(1.0, MUTED.gamma_multiply(0.7)),
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
        let anchor = Pos2::new(top.x, (top.y - 12.0).max(rect.top() + 12.0));
        let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), TEXT);
        let background = Rect::from_min_size(
            anchor - Vec2::new(6.0, 10.0),
            galley.size() + Vec2::new(12.0, 6.0),
        );
        painter.rect_filled(background, CornerRadius::same(5), PANEL.gamma_multiply(0.9));
        painter.galley(background.min + Vec2::new(6.0, 3.0), galley, TEXT);
    }

    // Compte à rebours et signal de départ de l'enregistrement.
    if let Some((text, since)) = model.banner
        && since.elapsed().as_secs_f32() < 1.2
    {
        // Proportionné à l'image : la vignette de l'onglet « Gestes » est petite.
        let radius = (rect.height() * 0.24).min(44.0);
        let center = Pos2::new(rect.center().x, rect.top() + radius * 1.6);
        painter.circle_filled(center, radius, PANEL.gamma_multiply(0.85));
        painter.text(
            center,
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(radius),
            RED,
        );
    }
    if status.capture.fps_warning {
        painter.text(
            rect.left_bottom() + Vec2::new(10.0, -10.0),
            Align2::LEFT_BOTTOM,
            format!(
                "⚠ {:.0} fps au lieu de {} : manque de lumière ?",
                status.capture.measured_fps, model.config.camera.fps
            ),
            FontId::proportional(13.0),
            AMBER,
        );
    }
}

/// Dessine toute la fenêtre d'aperçu et renvoie les actions demandées.
pub fn draw(ui: &mut Ui, model: &mut PreviewModel<'_>) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let mut tab = *model.tab;

    egui::Panel::top("barre")
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(Margin::symmetric(14, 9)),
        )
        .show(ui, |ui| top_bar(ui, model, &mut tab, &mut actions));
    *model.tab = tab;

    if tab == Tab::Gestures {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 6)),
            )
            .show(ui, |ui| super::gestures::draw(ui, model, &mut actions));
        return actions;
    }

    egui::Panel::right("diagnostic")
        .exact_size(340.0)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(Margin::same(10)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    engine_card(ui, model);
                    if model.status.hands.is_empty() && model.status.state != EngineState::Paused {
                        card(ui, "Mains", |ui| {
                            ui.label(RichText::new("Aucune main dans le champ.").color(MUTED));
                        });
                    }
                    for hand in &model.status.hands {
                        hand_card(ui, hand, model);
                    }
                    performance_card(ui, model.status);
                    log_card(ui, model.log);
                    card(ui, "Configuration", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Recharger").clicked() {
                                actions.push(UiAction::ReloadConfig);
                            }
                            if ui.button("Dossier").clicked() {
                                actions.push(UiAction::OpenConfigFolder);
                            }
                            if ui.button("Journaux").clicked() {
                                actions.push(UiAction::OpenLogs);
                            }
                        });
                        ui.label(
                        RichText::new(format!(
                            "{} bindings · {} contrôles continus · poses : {} · dynamiques : {}",
                            model.config.bindings.len(),
                            model.config.continuous.len(),
                            model.status.static_classifier,
                            model.status.dynamic_classifier
                        ))
                        .small()
                        .color(MUTED),
                    );
                    });
                });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin {
            left: 10,
            right: 0,
            top: 0,
            bottom: 10,
        }))
        .show(ui, |ui| {
            camera_view(
                ui,
                &CameraView {
                    status: model.status,
                    config: model.config,
                    frame: model.frame,
                    texture: model.texture,
                    pipeline_error: model.pipeline_error,
                    banner: model.record.banner.as_ref(),
                },
            )
        });

    actions
}
