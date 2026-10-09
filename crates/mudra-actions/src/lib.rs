//! Exécution des actions Windows de Mudra (§5.5, §7.8, D-16).
//!
//! Les actions s'exécutent sur un thread dédié, alimenté par un canal borné :
//! une action lente ne retarde jamais la vision (EF-41). L'encodage PNG des
//! captures d'écran part sur un second thread, de priorité inférieure.

mod input;
mod screenshot;
mod volume;

use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use mudra_config::keys::{
    KeyCombo, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP,
    VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use mudra_config::{
    Action, InternalOp, MediaKey, MouseOp, MouseTarget, Shell, VolumeMode, VolumeOp, expand_path,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONASTERISK, MB_ICONEXCLAMATION, MB_OK, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::screenshot::Capture;
use crate::volume::Volume;

/// Capacité du canal vision → actions.
const QUEUE: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("injection d'entrée : {0}")]
    Input(String),
    #[error("audio : {0}")]
    Audio(String),
    #[error("lancement de « {path} » : {message}")]
    Launch { path: String, message: String },
    #[error("capture d'écran : {0}")]
    Screenshot(String),
    #[error("commande : {0}")]
    Command(String),
    #[error("raccourci : {0}")]
    Keys(String),
    #[error("l'action « command » est désactivée (general.allow_commands = false)")]
    CommandsDisabled,
}

/// Demande envoyée au thread des actions.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionRequest {
    /// Action d'un binding ; `source` est le geste qui l'a déclenchée.
    Run {
        action: Action,
        source: String,
    },
    /// Entrée dans le contrôle continu du volume : mémorise le volume courant.
    VolumeBegin,
    /// Écart par rapport au volume mémorisé à l'entrée (EF-50).
    VolumeDelta(f32),
    MouseMove {
        x: f32,
        y: f32,
        target: MouseTarget,
    },
    MouseButton {
        down: bool,
    },
    /// Met à jour l'autorisation de l'action `command` (rechargement de la configuration).
    AllowCommands(bool),
}

/// Compte rendu d'exécution, renvoyé à l'application.
#[derive(Debug, Clone, PartialEq)]
pub enum Feedback {
    Done {
        source: String,
        summary: String,
        detail: Option<String>,
        elapsed: Duration,
    },
    Failed {
        source: String,
        summary: String,
        error: String,
    },
    /// Action interne à traiter par l'application (pause, verrouillage, notification).
    Internal {
        op: InternalOp,
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Passage à l'état Armé.
    Armed,
    /// Déclenchement d'une action.
    Fired,
    /// Erreur ou retour à Verrouillé.
    Notice,
}

/// Retour sonore par les sons système (EF-36). Non bloquant.
pub fn play_sound(sound: Sound) {
    let kind = match sound {
        Sound::Armed => MB_ICONASTERISK,
        Sound::Fired => MB_OK,
        Sound::Notice => MB_ICONEXCLAMATION,
    };
    // SAFETY: MessageBeep joue le son de façon asynchrone.
    unsafe {
        let _ = MessageBeep(kind);
    }
}

/// Poignée du thread des actions.
#[derive(Clone)]
pub struct ActionSender {
    tx: Sender<ActionRequest>,
}

impl ActionSender {
    /// Envoi non bloquant. Si le canal est plein, l'action est abandonnée et
    /// journalisée (EF-41) ; renvoie `false`.
    pub fn submit(&self, request: ActionRequest) -> bool {
        match self.tx.try_send(request) {
            Ok(()) => true,
            Err(TrySendError::Full(request)) => {
                tracing::warn!(?request, "canal des actions plein : action abandonnée");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

/// Lance le thread des actions et son worker. Les threads s'arrêtent quand
/// tous les `ActionSender` sont détruits.
pub fn spawn(allow_commands: bool, feedback: Sender<Feedback>) -> std::io::Result<ActionSender> {
    let (tx, rx) = bounded::<ActionRequest>(QUEUE);
    let (png_tx, png_rx) = bounded::<PngJob>(4);
    let worker_feedback = feedback.clone();
    std::thread::Builder::new()
        .name("mudra-worker".into())
        .spawn(move || worker(png_rx, worker_feedback))?;
    std::thread::Builder::new()
        .name("mudra-actions".into())
        .spawn(move || {
            // SAFETY: COM initialisé une fois en MTA pour ce thread (§7.8), libéré à la sortie.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let mut executor = Executor {
                volume: Volume::default(),
                base_volume: None,
                allow_commands,
                png: png_tx,
                feedback,
            };
            for request in rx {
                executor.handle(request);
            }
            drop(executor);
            if com {
                // SAFETY: apparié avec CoInitializeEx ci-dessus.
                unsafe { CoUninitialize() };
            }
        })?;
    Ok(ActionSender { tx })
}

struct PngJob {
    capture: Capture,
    path: std::path::PathBuf,
    source: String,
    summary: String,
    started: Instant,
}

/// Thread worker : tâches longues, priorité en dessous de la normale (§3.2).
fn worker(jobs: Receiver<PngJob>, feedback: Sender<Feedback>) {
    // SAFETY: agit uniquement sur le thread appelant.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    for job in jobs {
        let message = match screenshot::write_png(&job.capture, &job.path) {
            Ok(()) => Feedback::Done {
                source: job.source,
                summary: job.summary,
                detail: Some(job.path.display().to_string()),
                elapsed: job.started.elapsed(),
            },
            Err(e) => Feedback::Failed {
                source: job.source,
                summary: job.summary,
                error: e.to_string(),
            },
        };
        let _ = feedback.try_send(message);
    }
}

struct Executor {
    volume: Volume,
    base_volume: Option<f32>,
    allow_commands: bool,
    png: Sender<PngJob>,
    feedback: Sender<Feedback>,
}

fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

fn launch(path: &str, args: &[String], cwd: Option<&str>) -> Result<(), ActionError> {
    // Les URL sont passées telles quelles ; les chemins voient `~` et `%VAR%` développés (§7.8).
    let file = if path.contains("://") {
        path.to_string()
    } else {
        expand_path(path).to_string_lossy().into_owned()
    };
    let file_w = HSTRING::from(file.as_str());
    let params = args
        .iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    let params_w = HSTRING::from(params.as_str());
    let cwd_w = cwd.map(|c| HSTRING::from(expand_path(c).as_os_str()));
    // SAFETY: chaînes UTF-16 terminées, vivantes pendant l'appel.
    let instance = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file_w,
            if params.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR(params_w.as_ptr())
            },
            cwd_w
                .as_ref()
                .map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr())),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW renvoie une valeur > 32 en cas de succès.
    let code = instance.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        let message = match code {
            2 => "fichier introuvable".to_string(),
            3 => "chemin introuvable".to_string(),
            5 => "accès refusé".to_string(),
            31 => "aucune application associée".to_string(),
            other => format!("ShellExecute a échoué (code {other})"),
        };
        Err(ActionError::Launch {
            path: file,
            message,
        })
    }
}

fn run_command(command: &str, shell: Shell) -> Result<(), ActionError> {
    let mut process = match shell {
        Shell::Cmd => {
            let mut p = std::process::Command::new("cmd.exe");
            p.arg("/C").raw_arg(command);
            p
        }
        Shell::Powershell => {
            let mut p = std::process::Command::new("powershell.exe");
            p.args(["-NoProfile", "-NonInteractive", "-Command", command]);
            p
        }
    };
    // Sans fenêtre (§5.5) ; on n'attend pas la fin du processus.
    process
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map(drop)
        .map_err(|e| ActionError::Command(e.to_string()))
}

impl Executor {
    fn handle(&mut self, request: ActionRequest) {
        match request {
            ActionRequest::Run { action, source } => {
                let summary = action.summary();
                let started = Instant::now();
                match self.run(&action, &source, started) {
                    Ok(Outcome::Done(detail)) => {
                        let elapsed = started.elapsed();
                        tracing::debug!(geste = %source, action = %summary, ms = elapsed.as_secs_f32() * 1e3, "action exécutée");
                        self.report(Feedback::Done {
                            source,
                            summary,
                            detail,
                            elapsed,
                        });
                    }
                    // Le worker ou l'application rendront compte.
                    Ok(Outcome::Deferred) => {}
                    Err(e) => {
                        tracing::warn!(geste = %source, action = %summary, erreur = %e, "action en échec");
                        self.report(Feedback::Failed {
                            source,
                            summary,
                            error: e.to_string(),
                        });
                    }
                }
            }
            ActionRequest::VolumeBegin => self.base_volume = self.volume.get().ok(),
            ActionRequest::VolumeDelta(delta) => {
                if let Some(base) = self.base_volume
                    && let Err(e) = self.volume.set(base + delta)
                {
                    tracing::warn!(erreur = %e, "volume continu");
                }
            }
            ActionRequest::MouseMove { x, y, target } => {
                let _ = input::mouse_move(x, y, target);
            }
            ActionRequest::MouseButton { down } => {
                let _ = input::mouse_left(down);
            }
            ActionRequest::AllowCommands(allow) => self.allow_commands = allow,
        }
    }

    fn report(&self, feedback: Feedback) {
        let _ = self.feedback.try_send(feedback);
    }

    fn run(
        &mut self,
        action: &Action,
        source: &str,
        started: Instant,
    ) -> Result<Outcome, ActionError> {
        match action {
            Action::Launch { path, args, cwd } => {
                launch(path, args, cwd.as_deref()).map(|()| Outcome::Done(None))
            }
            Action::Volume {
                op,
                delta,
                value,
                mode,
            } => self.run_volume(*op, *delta, *value, *mode),
            Action::Media { key } => input::send_vk(match key {
                MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
                MediaKey::Next => VK_MEDIA_NEXT_TRACK,
                MediaKey::Prev => VK_MEDIA_PREV_TRACK,
                MediaKey::Stop => VK_MEDIA_STOP,
            })
            .map(|()| Outcome::Done(None)),
            Action::Keys { combo } => {
                let combo = KeyCombo::parse(combo).map_err(|e| ActionError::Keys(e.0))?;
                input::send_combo(&combo).map(|()| Outcome::Done(None))
            }
            Action::Screenshot {
                target,
                dir,
                clipboard,
            } => {
                let capture = screenshot::capture(*target)?;
                if *clipboard && let Err(e) = screenshot::copy_to_clipboard(&capture) {
                    tracing::warn!(erreur = %e, "copie dans le presse-papiers");
                }
                let job = PngJob {
                    path: screenshot::output_path(&expand_path(dir)),
                    capture,
                    source: source.to_string(),
                    summary: action.summary(),
                    started,
                };
                self.png
                    .try_send(job)
                    .map(|()| Outcome::Deferred)
                    .map_err(|_| {
                        ActionError::Screenshot("encodage saturé : capture abandonnée".into())
                    })
            }
            Action::Snip => {
                let combo = KeyCombo::parse("Win+Shift+S").map_err(|e| ActionError::Keys(e.0))?;
                input::send_combo(&combo).map(|()| Outcome::Done(None))
            }
            Action::Mouse { op, amount } => match op {
                MouseOp::LeftClick => input::mouse_left_click(),
                MouseOp::RightClick => input::mouse_right_click(),
                MouseOp::Scroll => input::mouse_scroll(*amount),
            }
            .map(|()| Outcome::Done(None)),
            Action::Command { command, shell } => {
                if !self.allow_commands {
                    return Err(ActionError::CommandsDisabled);
                }
                run_command(command, *shell).map(|()| Outcome::Done(None))
            }
            Action::Sequence { steps } => {
                for step in steps {
                    if let Outcome::Done(_) = self.run(&step.action, source, started)? {
                        // Les étapes différées rendent compte elles-mêmes.
                    }
                    if step.delay_ms > 0 {
                        std::thread::sleep(Duration::from_millis(step.delay_ms));
                    }
                }
                Ok(Outcome::Done(None))
            }
            Action::Internal { op, message } => {
                self.report(Feedback::Internal {
                    op: *op,
                    message: message.clone(),
                });
                Ok(Outcome::Deferred)
            }
        }
    }

    fn run_volume(
        &mut self,
        op: VolumeOp,
        delta: f32,
        value: Option<f32>,
        mode: VolumeMode,
    ) -> Result<Outcome, ActionError> {
        let percent = |v: f32| Some(format!("{:.0} %", v * 100.0));
        match (op, mode) {
            (VolumeOp::Step, VolumeMode::Api) => {
                self.volume.step(delta).map(|v| Outcome::Done(percent(v)))
            }
            (VolumeOp::Step, VolumeMode::Keys) => {
                // Une pression de touche vaut 2 % : on en envoie autant que nécessaire.
                let presses = (delta.abs() / 0.02).round().max(1.0) as u32;
                let vk = if delta >= 0.0 {
                    VK_VOLUME_UP
                } else {
                    VK_VOLUME_DOWN
                };
                for _ in 0..presses {
                    input::send_vk(vk)?;
                }
                Ok(Outcome::Done(None))
            }
            (VolumeOp::Set, _) => self
                .volume
                .set(value.unwrap_or(0.0))
                .map(|v| Outcome::Done(percent(v))),
            (VolumeOp::MuteToggle, VolumeMode::Api) => self.volume.toggle_mute().map(|muted| {
                Outcome::Done(Some(if muted {
                    "son coupé".into()
                } else {
                    "son rétabli".into()
                }))
            }),
            (VolumeOp::MuteToggle, VolumeMode::Keys) => {
                input::send_vk(VK_VOLUME_MUTE).map(|()| Outcome::Done(None))
            }
        }
    }
}

enum Outcome {
    /// Terminé, avec un détail optionnel à afficher.
    Done(Option<String>),
    /// Le compte rendu viendra d'ailleurs (worker PNG, application).
    Deferred,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_when_needed() {
        assert_eq!(quote_arg("--flag"), "--flag");
        assert_eq!(quote_arg("mon fichier.txt"), "\"mon fichier.txt\"");
        assert_eq!(quote_arg("dit \"oui\""), "\"dit \\\"oui\\\"\"");
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn full_queue_drops_instead_of_blocking() {
        let (tx, _rx) = bounded(2);
        let sender = ActionSender { tx };
        let request = ActionRequest::MouseButton { down: true };
        assert!(sender.submit(request.clone()));
        assert!(sender.submit(request.clone()));
        let started = Instant::now();
        assert!(!sender.submit(request));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn commands_are_refused_unless_enabled() {
        let (feedback_tx, feedback_rx) = bounded(8);
        let sender = spawn(false, feedback_tx).unwrap();
        let action = Action::Command {
            command: "exit 0".into(),
            shell: Shell::Cmd,
        };
        sender.submit(ActionRequest::Run {
            action: action.clone(),
            source: "rock".into(),
        });
        match feedback_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Feedback::Failed { error, source, .. } => {
                assert!(error.contains("allow_commands"), "{error}");
                assert_eq!(source, "rock");
            }
            other => panic!("{other:?}"),
        }
        // Une fois autorisée, la commande s'exécute (sans fenêtre).
        sender.submit(ActionRequest::AllowCommands(true));
        sender.submit(ActionRequest::Run {
            action,
            source: "rock".into(),
        });
        assert!(matches!(
            feedback_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Feedback::Done { .. }
        ));
    }

    #[test]
    fn internal_actions_are_forwarded_and_launch_errors_reported() {
        let (feedback_tx, feedback_rx) = bounded(8);
        let sender = spawn(false, feedback_tx).unwrap();
        sender.submit(ActionRequest::Run {
            action: Action::Internal {
                op: InternalOp::Lock,
                message: None,
            },
            source: "fist".into(),
        });
        assert_eq!(
            feedback_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Feedback::Internal {
                op: InternalOp::Lock,
                message: None
            }
        );
        sender.submit(ActionRequest::Run {
            action: Action::Launch {
                path: r"C:\mudra\introuvable-xyz.exe".into(),
                args: vec![],
                cwd: None,
            },
            source: "rock".into(),
        });
        match feedback_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Feedback::Failed { error, .. } => assert!(error.contains("introuvable"), "{error}"),
            other => panic!("{other:?}"),
        }
    }
}
