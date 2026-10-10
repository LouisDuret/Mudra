//! Thread vision (§3.2, figure 1) : consomme la boîte aux lettres, décode,
//! suit les mains, classe, fait tourner le moteur et envoie les actions.
//!
//! Le même pipeline sert à l'application, à `mudra record` et à `mudra bench`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, bounded};
use mudra_actions::{ActionRequest, ActionSender, Sound, play_sound};
use mudra_capture::{
    CaptureConfig, CaptureHandle, CaptureState, CaptureStatus, Consumer, FrameSource, MfSource,
    SourceFactory, StillSource, mailbox, now_us, promote_current_thread,
};
use mudra_config::{Action, Config, Control, InternalOp, MouseTarget, resolve_model_path};
use mudra_dataset::{FrameRecord, SessionHeader, SessionKind, SessionWriter};
use mudra_gestures::builtin::{BuiltinDynamicClassifier, BuiltinStaticClassifier};
use mudra_gestures::custom::CustomStaticClassifier;
use mudra_gestures::engine::Reason;
use mudra_gestures::onnx::{OnnxDynamicClassifier, OnnxStaticClassifier};
use mudra_gestures::stage::{GestureStage, HandInput, HandView};
use mudra_gestures::{
    ContinuousValue, DynamicClassifier, EngineEvent, EngineState, Handedness, StaticClassifier,
};
use mudra_vision::{
    FrameDecoder, Hand, HandTracker, Provider, SourceFormat, TrackerConfig, VisionSessions,
};

use crate::metrics::{CpuMeter, Metrics, Quantiles, Stage, process_memory_bytes};

/// D'où viennent les trames.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Camera,
    /// Image JPEG fixe rejouée en boucle (tests, démonstration sans caméra).
    Still(PathBuf),
}

/// Réveille le thread principal quand il y a du nouveau à afficher.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

pub struct Options {
    pub config: Arc<Config>,
    pub source: Source,
    /// `None` : les gestes sont reconnus mais aucune action n'est exécutée.
    pub actions: Option<ActionSender>,
    pub notify: Option<Notify>,
    pub start_paused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordKind {
    /// Capture continue pendant `seconds`.
    Static { seconds: f32 },
    /// `reps` répétitions guidées, un clip de 1,0 s chacune.
    Dynamic { reps: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordRequest {
    pub label: String,
    pub kind: RecordKind,
    pub dataset_root: PathBuf,
}

#[derive(Debug)]
pub enum Command {
    TogglePause,
    Lock,
    /// Nouvelle configuration (bindings, seuils, lissage) sans redémarrage.
    Reload(Arc<Config>),
    /// Fenêtre d'aperçu ouverte ou fermée (EF-72).
    SetPreview(bool),
    /// Poses perso modifiées sur le disque : recharge les classifieurs.
    ReloadClassifiers,
    Record(RecordRequest),
    CancelRecording,
    ResetMetrics,
    /// Mode essai : les gestes sont reconnus et affichés, rien n'est exécuté.
    SetDryRun(bool),
    /// Exécute une action une fois, sans geste (bouton « Tester »).
    TestAction(Action),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordEvent {
    /// Compte à rebours avant la capture ; `clip` = (numéro, total) en mode dynamique.
    Countdown {
        seconds_left: u32,
        clip: Option<(u32, u32)>,
    },
    /// La capture commence : c'est le moment de faire le geste.
    Go {
        clip: Option<(u32, u32)>,
    },
    Progress {
        frames: u64,
        fraction: f32,
    },
    Done {
        path: PathBuf,
        frames: u64,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    State {
        from: EngineState,
        to: EngineState,
        reason: Reason,
    },
    Gesture {
        /// Indice du binding dans `Config::bindings`.
        binding: usize,
        gesture: String,
        action: String,
        confidence: f32,
        right: bool,
        repeat: bool,
        latency_ms: f32,
    },
    Continuous {
        name: String,
        active: bool,
    },
    Notice {
        error: bool,
        message: String,
    },
    Recording(RecordEvent),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HandStatus {
    pub track: u32,
    pub right: bool,
    /// Trois poses les plus probables.
    pub top_static: Vec<(String, f32)>,
    pub top_dynamic: Vec<(String, f32)>,
    /// Pose en cours de maintien : (label, avancement, confirmée).
    pub progress: Option<(String, f32, bool)>,
    pub speed: f32,
    pub pinch: f32,
}

/// Instantané léger de l'état du pipeline, publié une dizaine de fois par seconde.
#[derive(Debug, Clone)]
pub struct Status {
    pub state: EngineState,
    pub capture: CaptureStatus,
    pub provider: &'static str,
    pub static_classifier: String,
    pub dynamic_classifier: String,
    pub processed_fps: f32,
    pub dropped: u64,
    pub dropped_percent: f32,
    pub latencies: Vec<Quantiles>,
    pub hands: Vec<HandStatus>,
    pub armed_remaining_ms: Option<u64>,
    pub continuous: Option<String>,
    /// Mode veille : aucune main suivie, détection à fréquence réduite (EF-13).
    pub idle: bool,
    pub image_size: (u32, u32),
    pub cpu_percent: f32,
    pub memory_mb: f32,
    pub recording: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: EngineState::Paused,
            capture: CaptureStatus::default(),
            provider: "",
            static_classifier: String::new(),
            dynamic_classifier: String::new(),
            processed_fps: 0.0,
            dropped: 0,
            dropped_percent: 0.0,
            latencies: Vec::new(),
            hands: Vec::new(),
            armed_remaining_ms: None,
            continuous: None,
            idle: false,
            image_size: (0, 0),
            cpu_percent: 0.0,
            memory_mb: 0.0,
            recording: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewHand {
    /// Points lissés, en pixels de l'image source affichée.
    pub landmarks: [[f32; 2]; 21],
    pub roi: [[f32; 2]; 4],
    pub right: bool,
    pub label: Option<(String, f32)>,
}

/// Image et tracés pour l'aperçu. N'est produit que fenêtre ouverte (EF-72).
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub width: usize,
    pub height: usize,
    /// RGB8, déjà retourné si la vue selfie est active.
    pub rgb: Vec<u8>,
    /// Taille de l'image source, repère des `landmarks`.
    pub source_size: (f32, f32),
    pub hands: Vec<PreviewHand>,
}

#[derive(Default)]
pub struct Shared {
    pub status: Mutex<Status>,
    pub preview: Mutex<Option<PreviewFrame>>,
    /// Quantiles cumulés depuis le démarrage (ou la dernière remise à zéro), pour `bench`.
    pub cumulative: Mutex<Vec<Quantiles>>,
}

pub struct Pipeline {
    commands: Sender<Command>,
    pub events: Receiver<PipelineEvent>,
    pub shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Pipeline {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // Fermer le canal de commandes arrête le thread, qui libère la caméra.
        let (dead, _) = bounded(0);
        self.commands = dead;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Charge les classifieurs configurés, complétés par les poses perso.
pub fn load_classifiers(
    config: &Config,
) -> (
    Box<dyn StaticClassifier>,
    Box<dyn DynamicClassifier>,
    Vec<String>,
) {
    let (static_classifier, dynamic_classifier, mut notices) = load_base_classifiers(config);
    let (poses, custom_notices) = mudra_gestures::custom::load_dir(&crate::custom::dir());
    notices.extend(custom_notices);
    let static_classifier: Box<dyn StaticClassifier> = if poses.is_empty() {
        static_classifier
    } else {
        Box::new(CustomStaticClassifier::new(static_classifier, &poses))
    };
    (static_classifier, dynamic_classifier, notices)
}

/// Charge les classifieurs de base ; à défaut de fichier ONNX, les
/// classifieurs géométriques intégrés prennent le relais.
pub fn load_base_classifiers(
    config: &Config,
) -> (
    Box<dyn StaticClassifier>,
    Box<dyn DynamicClassifier>,
    Vec<String>,
) {
    let mut notices = Vec::new();
    let mut resolve = |raw: &str, what: &str| -> Option<PathBuf> {
        if raw.eq_ignore_ascii_case("builtin") {
            return None;
        }
        let path = resolve_model_path(raw);
        if path.is_file() {
            Some(path)
        } else {
            notices.push(format!(
                "{what} : {raw} absent, classifieur géométrique intégré utilisé"
            ));
            None
        }
    };
    let static_path = resolve(&config.models.static_, "poses");
    let dynamic_path = resolve(&config.models.dynamic, "gestes dynamiques");
    let static_classifier: Box<dyn StaticClassifier> =
        match static_path.map(|p| OnnxStaticClassifier::load(&p)) {
            Some(Ok(c)) => Box::new(c),
            Some(Err(e)) => {
                notices.push(format!("{e} — classifieur intégré utilisé"));
                Box::new(BuiltinStaticClassifier::new())
            }
            None => Box::new(BuiltinStaticClassifier::new()),
        };
    let dynamic_classifier: Box<dyn DynamicClassifier> =
        match dynamic_path.map(|p| OnnxDynamicClassifier::load(&p)) {
            Some(Ok(c)) => Box::new(c),
            Some(Err(e)) => {
                notices.push(format!("{e} — classifieur intégré utilisé"));
                Box::new(BuiltinDynamicClassifier::new())
            }
            None => Box::new(BuiltinDynamicClassifier::new()),
        };
    (static_classifier, dynamic_classifier, notices)
}

pub fn provider_for(config: &Config) -> Provider {
    match config.inference.execution_provider {
        mudra_config::ExecutionProvider::Cpu => Provider::Cpu,
        // CUDA et TensorRT ne sont pas compilés dans ce prototype : DirectML, puis CPU.
        _ => Provider::DirectMl,
    }
}

pub fn load_vision(config: &Config) -> Result<VisionSessions> {
    let palm = resolve_model_path(&config.models.palm);
    let landmarks = resolve_model_path(&config.models.landmarks);
    VisionSessions::load(&palm, &landmarks, provider_for(config))
        .context("chargement des modèles de vision")
}

pub fn tracker_config(config: &Config) -> TrackerConfig {
    TrackerConfig {
        max_hands: config.inference.max_hands.clamp(1, 2) as usize,
        palm_score_threshold: config.inference.palm_score_threshold,
        presence_threshold: config.inference.presence_threshold,
        redetect_every_n_frames: config.inference.redetect_every_n_frames,
        mirror: config.camera.mirror,
    }
}

pub fn capture_config(config: &Config) -> CaptureConfig {
    use mudra_config::{DeviceSelector, Exposure, PixelFormat};
    CaptureConfig {
        device: match &config.camera.device {
            DeviceSelector::Index(i) => mudra_capture::DeviceSelector::Index(*i),
            DeviceSelector::Name(n) => mudra_capture::DeviceSelector::Name(n.clone()),
        },
        format: match config.camera.format {
            PixelFormat::Mjpeg => mudra_capture::PixelFormat::Mjpeg,
            PixelFormat::Nv12 => mudra_capture::PixelFormat::Nv12,
            PixelFormat::Yuy2 => mudra_capture::PixelFormat::Yuy2,
        },
        width: config.camera.width,
        height: config.camera.height,
        fps: config.camera.fps,
        exposure: match config.camera.exposure {
            Exposure::Auto => None,
            Exposure::Manual(v) => Some(v),
        },
        auto_exposure_priority: config.camera.auto_exposure_priority,
    }
}

/// Démarre le pipeline. Les modèles sont chargés ici, sur le thread appelant :
/// une erreur de modèle est renvoyée tout de suite, avec un message clair.
pub fn start(options: Options) -> Result<Pipeline> {
    let sessions = load_vision(&options.config)?;
    let provider = sessions.provider;
    let (static_classifier, dynamic_classifier, notices) = load_classifiers(&options.config);
    let stage = GestureStage::new(&options.config, static_classifier, dynamic_classifier);
    let tracker = HandTracker::new(sessions, tracker_config(&options.config));

    let (command_tx, command_rx) = bounded(64);
    let (event_tx, event_rx) = bounded(512);
    let shared = Arc::new(Shared::default());
    let worker_shared = shared.clone();
    let thread = std::thread::Builder::new()
        .name("mudra-vision".into())
        .spawn(move || {
            let mut worker = Worker::new(
                options,
                tracker,
                stage,
                provider,
                command_rx,
                event_tx,
                worker_shared,
            );
            for notice in notices {
                tracing::info!("{notice}");
                worker.emit(PipelineEvent::Notice {
                    error: false,
                    message: notice,
                });
            }
            worker.run();
        })
        .context("création du thread vision")?;
    Ok(Pipeline {
        commands: command_tx,
        events: event_rx,
        shared,
        thread: Some(thread),
    })
}

enum RecordPhase {
    Countdown { until_us: u64, announced: u32 },
    Capturing { since_us: u64, until_us: u64 },
    Rest { until_us: u64 },
}

struct Recorder {
    writer: SessionWriter,
    kind: RecordKind,
    phase: RecordPhase,
    clip: u32,
    last_progress_us: u64,
}

const STATIC_COUNTDOWN_US: u64 = 3_000_000;
const DYNAMIC_COUNTDOWN_US: u64 = 2_000_000;
const CLIP_US: u64 = 1_000_000;
const REST_US: u64 = 600_000;

struct Worker {
    config: Arc<Config>,
    source: Source,
    actions: Option<ActionSender>,
    notify: Option<Notify>,
    tracker: HandTracker,
    stage: GestureStage,
    provider: Provider,
    commands: Receiver<Command>,
    events: Sender<PipelineEvent>,
    shared: Arc<Shared>,

    capture: Option<CaptureHandle>,
    consumer: Option<Consumer>,
    decoder: FrameDecoder,
    hands: Vec<Hand>,
    inputs: Vec<HandInput>,
    engine_events: Vec<EngineEvent>,
    metrics: Metrics,
    cpu: CpuMeter,
    recorder: Option<Recorder>,

    paused: bool,
    dry_run: bool,
    preview: bool,
    clock_us: u64,
    last_idle_detection_us: u64,
    last_status: Instant,
    last_metrics_reset: Instant,
    last_preview: Instant,
    last_capture_state: Option<CaptureState>,
    fps_warned: bool,
    fallback_notified: bool,
    processed_in_window: u32,
    processed_fps: f32,
    fps_window: Instant,
    image_size: (u32, u32),
    /// Trames reçues et trames abandonnées pendant qu'une main est suivie.
    /// En veille, les trames sont sautées volontairement : elles ne comptent pas (ENF-01).
    frames_tracking: u64,
    dropped_tracking: u64,
    dropped_seen: u64,
}

impl Worker {
    fn new(
        options: Options,
        tracker: HandTracker,
        stage: GestureStage,
        provider: Provider,
        commands: Receiver<Command>,
        events: Sender<PipelineEvent>,
        shared: Arc<Shared>,
    ) -> Self {
        let now = Instant::now();
        Self {
            paused: options.start_paused,
            dry_run: false,
            config: options.config,
            source: options.source,
            actions: options.actions,
            notify: options.notify,
            tracker,
            stage,
            provider,
            commands,
            events,
            shared,
            capture: None,
            consumer: None,
            decoder: FrameDecoder::new(),
            hands: Vec::with_capacity(2),
            inputs: Vec::with_capacity(2),
            engine_events: Vec::with_capacity(16),
            metrics: Metrics::default(),
            cpu: CpuMeter::default(),
            recorder: None,
            preview: false,
            clock_us: 0,
            last_idle_detection_us: 0,
            last_status: now,
            last_metrics_reset: now,
            last_preview: now,
            last_capture_state: None,
            fps_warned: false,
            fallback_notified: false,
            processed_in_window: 0,
            processed_fps: 0.0,
            fps_window: now,
            image_size: (0, 0),
            frames_tracking: 0,
            dropped_tracking: 0,
            dropped_seen: 0,
        }
    }

    /// Aucune action n'est exécutée en mode essai ni pendant un enregistrement.
    fn silent(&self) -> bool {
        self.dry_run || self.recorder.is_some()
    }

    fn emit(&self, event: PipelineEvent) {
        if self.events.try_send(event).is_ok()
            && let Some(notify) = &self.notify
        {
            notify();
        }
    }

    /// Horloge monotone du moteur : jamais en arrière, même si une trame
    /// horodatée avant un tic de veille arrive après lui.
    fn tick_clock(&mut self, t_us: u64) -> u64 {
        self.clock_us = self.clock_us.max(t_us);
        self.clock_us
    }

    fn start_capture(&mut self) {
        let (producer, consumer) = mailbox();
        let factory: SourceFactory = match &self.source {
            Source::Camera => {
                let config = capture_config(&self.config);
                Box::new(move || {
                    MfSource::open(&config).map(|s| Box::new(s) as Box<dyn FrameSource>)
                })
            }
            Source::Still(path) => {
                let (path, fps) = (path.clone(), self.config.camera.fps);
                Box::new(move || {
                    StillSource::open(&path, fps).map(|s| Box::new(s) as Box<dyn FrameSource>)
                })
            }
        };
        match mudra_capture::spawn(factory, producer, self.config.camera.fps) {
            Ok(handle) => {
                self.capture = Some(handle);
                self.consumer = Some(consumer);
                self.dropped_seen = 0;
            }
            Err(e) => self.emit(PipelineEvent::Notice {
                error: true,
                message: format!("thread de capture : {e}"),
            }),
        }
    }

    /// Arrête la capture : la caméra est libérée, le voyant s'éteint (EF-06).
    fn stop_capture(&mut self) {
        self.capture = None;
        self.consumer = None;
        self.last_capture_state = None;
        self.tracker.reset();
        self.stage.reset_tracks();
    }

    fn set_paused(&mut self, paused: bool) {
        if paused == self.paused && (paused || self.capture.is_some()) {
            return;
        }
        self.paused = paused;
        self.engine_events.clear();
        if paused {
            self.stage.engine_mut().pause(&mut self.engine_events);
            self.stop_capture();
            self.cancel_recording("pause");
        } else {
            self.stage.engine_mut().resume(&mut self.engine_events);
            self.start_capture();
        }
        self.dispatch(now_us());
        self.publish_status(true);
    }

    fn run(&mut self) {
        promote_current_thread();
        self.engine_events.clear();
        if self.paused {
            self.stage.engine_mut().pause(&mut self.engine_events);
        } else {
            self.start_capture();
        }
        self.dispatch(now_us());
        self.publish_status(true);

        loop {
            // Commandes : sans attente quand on traite des trames.
            loop {
                let command = if self.paused {
                    match self.commands.recv_timeout(Duration::from_millis(250)) {
                        Ok(c) => Some(c),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match self.commands.try_recv() {
                        Ok(c) => Some(c),
                        Err(crossbeam_channel::TryRecvError::Empty) => None,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                    }
                };
                match command {
                    Some(c) => self.handle_command(c),
                    None => break,
                }
            }
            if self.paused {
                self.publish_status(false);
                continue;
            }
            self.step();
            self.watch_capture();
            self.publish_status(false);
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::TogglePause => self.set_paused(!self.paused),
            Command::Lock => {
                self.engine_events.clear();
                self.stage.engine_mut().lock(&mut self.engine_events);
                self.dispatch(now_us());
            }
            Command::Reload(config) => {
                self.engine_events.clear();
                self.stage.set_config(&config, &mut self.engine_events);
                self.tracker.set_config(tracker_config(&config));
                if let Some(actions) = &self.actions {
                    actions.submit(ActionRequest::AllowCommands(config.general.allow_commands));
                }
                self.config = config;
                self.dispatch(now_us());
                self.publish_status(true);
            }
            Command::SetPreview(open) => {
                self.preview = open;
                if !open && let Ok(mut slot) = self.shared.preview.lock() {
                    *slot = None;
                }
            }
            Command::ReloadClassifiers => {
                let (static_classifier, dynamic_classifier, notices) =
                    load_classifiers(&self.config);
                for notice in notices {
                    tracing::info!("{notice}");
                }
                self.engine_events.clear();
                self.stage.set_classifiers(
                    &self.config,
                    static_classifier,
                    dynamic_classifier,
                    &mut self.engine_events,
                );
                self.dispatch(now_us());
                self.publish_status(true);
            }
            Command::Record(request) => self.start_recording(request),
            Command::CancelRecording => self.cancel_recording("annulé"),
            Command::SetDryRun(on) => self.dry_run = on,
            Command::TestAction(action) => self.run_action(action, "essai"),
            Command::ResetMetrics => {
                self.metrics.reset_all();
                self.frames_tracking = 0;
                self.dropped_tracking = 0;
            }
        }
    }

    /// Traite au plus une trame.
    fn step(&mut self) {
        let Some(consumer) = self.consumer.as_mut() else {
            std::thread::sleep(Duration::from_millis(50));
            return;
        };
        let dropped_now = consumer.dropped();
        let Some(frame) = consumer.recv_timeout(Duration::from_millis(100)) else {
            // Pas de trame : le moteur doit quand même voir passer le temps.
            self.advance_without_hands();
            return;
        };
        let frame_us = frame.timestamp_us;
        if self.tracker.is_tracking() {
            self.frames_tracking += 1;
            self.dropped_tracking += dropped_now.saturating_sub(self.dropped_seen);
        }
        self.dropped_seen = dropped_now;

        // Mode veille (EF-13) : sans main suivie, une trame sur N seulement est
        // décodée et passée au détecteur de paume.
        let idle_period = 1_000_000 / u64::from(self.config.inference.idle_detection_hz.max(1));
        if !self.tracker.is_tracking()
            && self.recorder.is_none()
            && frame_us.saturating_sub(self.last_idle_detection_us) < idle_period
        {
            self.advance_without_hands();
            return;
        }
        self.last_idle_detection_us = frame_us;

        // ① Décodage.
        let format = match frame.format {
            mudra_capture::PixelFormat::Mjpeg => SourceFormat::Mjpeg,
            mudra_capture::PixelFormat::Nv12 => SourceFormat::Nv12,
            mudra_capture::PixelFormat::Yuy2 => SourceFormat::Yuy2,
        };
        let t0 = Instant::now();
        let image = match self.decoder.decode(
            format,
            &frame.data,
            frame.width as usize,
            frame.height as usize,
        ) {
            Ok(image) => image,
            Err(e) => {
                tracing::debug!(erreur = %e, "trame illisible ignorée");
                return;
            }
        };
        self.metrics
            .record(Stage::Decode, t0.elapsed().as_micros() as u64);
        let (width, height) = (image.width as f32, image.height as f32);
        self.image_size = (image.width as u32, image.height as u32);

        // ② ③ Paume (si nécessaire) et points-clés.
        let stats = match self.tracker.process(&image, &mut self.hands) {
            Ok(stats) => stats,
            Err(e) => {
                tracing::error!(erreur = %e, "inférence de vision");
                self.emit(PipelineEvent::Notice {
                    error: true,
                    message: e.to_string(),
                });
                std::thread::sleep(Duration::from_millis(200));
                return;
            }
        };
        if stats.palm_ran {
            self.metrics.record(Stage::Palm, u64::from(stats.palm_us));
        }
        if stats.landmark_runs > 0 {
            self.metrics
                .record(Stage::Landmarks, u64::from(stats.landmarks_us));
        }

        // ④ ⑤ ⑥ Lissage, classifieurs, moteur.
        let t1 = Instant::now();
        self.inputs.clear();
        self.inputs.extend(self.hands.iter().map(|h| HandInput {
            track: h.track_id,
            handedness: if h.right {
                Handedness::Right
            } else {
                Handedness::Left
            },
            image: h.image,
            world: h.world,
        }));
        let now = self.tick_clock(frame_us);
        self.engine_events.clear();
        if let Err(e) =
            self.stage
                .process(now, width, height, &self.inputs, &mut self.engine_events)
        {
            tracing::error!(erreur = %e, "classifieur");
            self.emit(PipelineEvent::Notice {
                error: true,
                message: e.to_string(),
            });
        }
        self.metrics
            .record(Stage::Gestures, t1.elapsed().as_micros() as u64);
        self.metrics
            .record(Stage::Total, now_us().saturating_sub(frame_us));
        self.processed_in_window += 1;

        self.record_frame(now, width, height);
        self.dispatch(frame_us);
        self.publish_preview();
    }

    fn advance_without_hands(&mut self) {
        // Le moteur ne voit « aucune main » que si le suivi n'en a vraiment plus.
        if self.tracker.is_tracking() {
            return;
        }
        let now = self.tick_clock(now_us());
        self.engine_events.clear();
        if let Err(e) = self
            .stage
            .process(now, 1.0, 1.0, &[], &mut self.engine_events)
        {
            tracing::error!(erreur = %e, "moteur");
        }
        self.tick_recorder(now);
        self.dispatch(now);
    }

    /// Transforme les événements du moteur en actions et en événements d'interface.
    fn dispatch(&mut self, frame_us: u64) {
        if self.engine_events.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.engine_events);
        let recording = self.recorder.is_some();
        for event in &events {
            match event {
                EngineEvent::StateChanged { from, to, reason } => {
                    tracing::info!(de = from.label_fr(), vers = to.label_fr(), raison = ?reason, "état du moteur");
                    if self.config.general.sounds && !recording {
                        match to {
                            EngineState::Armed if *from == EngineState::Locked => {
                                play_sound(Sound::Armed)
                            }
                            EngineState::Locked if *from == EngineState::Armed => {
                                play_sound(Sound::Notice)
                            }
                            _ => {}
                        }
                    }
                    self.emit(PipelineEvent::State {
                        from: *from,
                        to: *to,
                        reason: *reason,
                    });
                }
                EngineEvent::GestureFired {
                    binding,
                    gesture,
                    confidence,
                    hand,
                    repeat,
                } => {
                    // Pendant un enregistrement, les gestes ne déclenchent rien.
                    if recording {
                        continue;
                    }
                    let Some(action) = self.config.bindings.get(*binding).map(|b| b.action.clone())
                    else {
                        continue;
                    };
                    let summary = action.summary();
                    let latency_ms = now_us().saturating_sub(frame_us) as f32 / 1000.0;
                    // EF-42 : chaque déclenchement est journalisé.
                    tracing::info!(
                        geste = %gesture,
                        confiance = confidence,
                        action = %summary,
                        latence_ms = latency_ms,
                        repetition = repeat,
                        essai = self.dry_run,
                        "geste déclenché"
                    );
                    let summary = if self.dry_run {
                        format!("{summary} (essai, non exécutée)")
                    } else if self.recorder.is_some() {
                        format!("{summary} (enregistrement en cours, non exécutée)")
                    } else {
                        self.run_action(action, gesture);
                        summary
                    };
                    if self.config.general.sounds && !repeat {
                        play_sound(Sound::Fired);
                    }
                    self.emit(PipelineEvent::Gesture {
                        binding: *binding,
                        gesture: gesture.clone(),
                        action: summary,
                        confidence: *confidence,
                        right: *hand == Handedness::Right,
                        repeat: *repeat,
                        latency_ms,
                    });
                }
                EngineEvent::ContinuousStarted { control } => {
                    let Some(c) = self.config.continuous.get(*control) else {
                        continue;
                    };
                    if matches!(c.control, Control::Volume { .. })
                        && !self.silent()
                        && let Some(actions) = &self.actions
                    {
                        actions.submit(ActionRequest::VolumeBegin);
                    }
                    self.emit(PipelineEvent::Continuous {
                        name: c.name.clone(),
                        active: true,
                    });
                }
                EngineEvent::ContinuousUpdate { control, value } => {
                    let (Some(actions), Some(c)) =
                        (&self.actions, self.config.continuous.get(*control))
                    else {
                        continue;
                    };
                    if self.silent() {
                        continue;
                    }
                    let target = match &c.control {
                        Control::Mouse { target, .. } => *target,
                        Control::Volume { .. } => MouseTarget::Primary,
                    };
                    actions.submit(match value {
                        ContinuousValue::VolumeDelta(delta) => ActionRequest::VolumeDelta(*delta),
                        ContinuousValue::MouseMove { x, y } => ActionRequest::MouseMove {
                            x: *x,
                            y: *y,
                            target,
                        },
                        ContinuousValue::MouseButton { down } => {
                            ActionRequest::MouseButton { down: *down }
                        }
                    });
                }
                EngineEvent::ContinuousEnded { control } => {
                    if let Some(c) = self.config.continuous.get(*control) {
                        self.emit(PipelineEvent::Continuous {
                            name: c.name.clone(),
                            active: false,
                        });
                    }
                }
                EngineEvent::FlushDynamicWindow { .. } => {}
            }
        }
        // Rend le tampon pour éviter une réallocation à la trame suivante.
        self.engine_events = events;
        self.engine_events.clear();
    }

    fn run_action(&mut self, action: Action, gesture: &str) {
        match action {
            // Les actions internes pilotent le moteur lui-même.
            Action::Internal {
                op: InternalOp::TogglePause,
                ..
            } => self.set_paused(!self.paused),
            Action::Internal {
                op: InternalOp::Lock,
                ..
            } => {
                let mut events = Vec::new();
                self.stage.engine_mut().lock(&mut events);
                for e in events {
                    if let EngineEvent::StateChanged { from, to, reason } = e {
                        self.emit(PipelineEvent::State { from, to, reason });
                    }
                }
            }
            Action::Internal {
                op: InternalOp::Notify,
                message,
            } => {
                self.emit(PipelineEvent::Notice {
                    error: false,
                    message: message.unwrap_or_default(),
                });
            }
            action => {
                if let Some(actions) = &self.actions {
                    actions.submit(ActionRequest::Run {
                        action,
                        source: gesture.to_string(),
                    });
                }
            }
        }
    }

    // ----- Enregistrement de données (EF-60) -----

    fn start_recording(&mut self, request: RecordRequest) {
        if self.paused {
            self.emit(PipelineEvent::Recording(RecordEvent::Failed(
                "Mudra est en pause".into(),
            )));
            return;
        }
        let capture = self
            .capture
            .as_ref()
            .map(CaptureHandle::status)
            .unwrap_or_default();
        let (clips, countdown) = match &request.kind {
            RecordKind::Static { .. } => (None, STATIC_COUNTDOWN_US),
            RecordKind::Dynamic { reps } => (Some(*reps), DYNAMIC_COUNTDOWN_US),
        };
        let header = SessionHeader {
            format_version: mudra_dataset::FORMAT_VERSION,
            kind: if clips.is_some() {
                SessionKind::Dynamic
            } else {
                SessionKind::Static
            },
            label: request.label.clone(),
            camera: capture.device,
            // Taille réelle des images traitées : les points sont normalisés par elle.
            width: if self.image_size.0 > 0 {
                self.image_size.0
            } else {
                self.config.camera.width
            },
            height: if self.image_size.1 > 0 {
                self.image_size.1
            } else {
                self.config.camera.height
            },
            fps: capture
                .format
                .map_or(self.config.camera.fps as f32, |f| f.fps),
            mirror: self.config.camera.mirror,
            created_at: String::new(),
            clips,
            clip_seconds: clips.map(|_| CLIP_US as f32 * 1e-6),
        };
        match SessionWriter::create(&request.dataset_root, header) {
            Ok(writer) => {
                let now = self.tick_clock(now_us());
                self.recorder = Some(Recorder {
                    writer,
                    kind: request.kind,
                    phase: RecordPhase::Countdown {
                        until_us: now + countdown,
                        announced: u32::MAX,
                    },
                    clip: 1,
                    last_progress_us: 0,
                });
            }
            Err(e) => self.emit(PipelineEvent::Recording(RecordEvent::Failed(e.to_string()))),
        }
    }

    fn cancel_recording(&mut self, why: &str) {
        if let Some(recorder) = self.recorder.take() {
            let path = recorder.writer.path().to_path_buf();
            drop(recorder);
            let _ = std::fs::remove_file(path);
            self.emit(PipelineEvent::Recording(RecordEvent::Failed(format!(
                "enregistrement interrompu ({why})"
            ))));
        }
    }

    /// Fait avancer le compte à rebours et les phases de l'enregistrement.
    fn tick_recorder(&mut self, now: u64) {
        let Some(recorder) = self.recorder.as_mut() else {
            return;
        };
        let clip_info = match recorder.kind {
            RecordKind::Dynamic { reps } => Some((recorder.clip, reps)),
            RecordKind::Static { .. } => None,
        };
        let mut event = None;
        let mut finished = false;
        match &mut recorder.phase {
            RecordPhase::Countdown {
                until_us,
                announced,
            } => {
                if now >= *until_us {
                    let duration = match recorder.kind {
                        RecordKind::Static { seconds } => (seconds.max(0.5) * 1e6) as u64,
                        RecordKind::Dynamic { .. } => CLIP_US,
                    };
                    recorder.phase = RecordPhase::Capturing {
                        since_us: now,
                        until_us: now + duration,
                    };
                    // Bip : c'est le signal de départ (EF-60).
                    play_sound(Sound::Armed);
                    event = Some(RecordEvent::Go { clip: clip_info });
                } else {
                    let left = ((*until_us - now) / 1_000_000) as u32 + 1;
                    if left != *announced {
                        *announced = left;
                        event = Some(RecordEvent::Countdown {
                            seconds_left: left,
                            clip: clip_info,
                        });
                    }
                }
            }
            RecordPhase::Capturing { since_us, until_us } => {
                if now >= *until_us {
                    match recorder.kind {
                        RecordKind::Dynamic { reps } if recorder.clip < reps => {
                            recorder.clip += 1;
                            recorder.phase = RecordPhase::Rest {
                                until_us: now + REST_US,
                            };
                        }
                        _ => finished = true,
                    }
                } else if now.saturating_sub(recorder.last_progress_us) >= 500_000 {
                    recorder.last_progress_us = now;
                    let fraction = match recorder.kind {
                        RecordKind::Static { .. } => {
                            (now - *since_us) as f32 / (*until_us - *since_us).max(1) as f32
                        }
                        RecordKind::Dynamic { reps } => {
                            (recorder.clip - 1) as f32 / reps.max(1) as f32
                        }
                    };
                    event = Some(RecordEvent::Progress {
                        frames: recorder.writer.frames(),
                        fraction,
                    });
                }
            }
            RecordPhase::Rest { until_us } => {
                if now >= *until_us {
                    recorder.phase = RecordPhase::Countdown {
                        until_us: now + DYNAMIC_COUNTDOWN_US,
                        announced: u32::MAX,
                    };
                }
            }
        }
        if let Some(event) = event {
            self.emit(PipelineEvent::Recording(event));
        }
        if finished && let Some(recorder) = self.recorder.take() {
            let frames = recorder.writer.frames();
            let event = match recorder.writer.finish() {
                Ok(path) => {
                    tracing::info!(fichier = %path.display(), trames = frames, "session enregistrée");
                    RecordEvent::Done { path, frames }
                }
                Err(e) => RecordEvent::Failed(e.to_string()),
            };
            self.emit(PipelineEvent::Recording(event));
        }
    }

    fn record_frame(&mut self, now: u64, width: f32, height: f32) {
        self.tick_recorder(now);
        let Some(recorder) = self.recorder.as_mut() else {
            return;
        };
        if !matches!(recorder.phase, RecordPhase::Capturing { .. }) {
            return;
        }
        let clip = matches!(recorder.kind, RecordKind::Dynamic { .. }).then_some(recorder.clip);
        let mut failure = None;
        for view in self.stage.hands() {
            let Some(hand) = self.hands.iter().find(|h| h.track_id == view.track) else {
                continue;
            };
            let mut image = view.image;
            for p in image.iter_mut() {
                *p = [p[0] / width, p[1] / height, p[2] / width];
            }
            let record = FrameRecord {
                t: now,
                track: view.track,
                hand: match view.handedness {
                    Handedness::Left => mudra_dataset::Hand::Left,
                    Handedness::Right => mudra_dataset::Hand::Right,
                },
                hand_score: hand.handedness_score,
                presence: hand.presence,
                image,
                world: view.world,
                clip,
            };
            if let Err(e) = recorder.writer.write(&record) {
                failure = Some(e.to_string());
                break;
            }
        }
        if let Some(message) = failure {
            self.recorder = None;
            self.emit(PipelineEvent::Recording(RecordEvent::Failed(message)));
        }
    }

    // ----- Publication vers l'interface -----

    /// Signale les changements d'état de la capture (EF-02, EF-03, EF-05).
    fn watch_capture(&mut self) {
        let Some(capture) = &self.capture else { return };
        let status = capture.status();
        if self.last_capture_state.as_ref() != Some(&status.state) {
            match &status.state {
                CaptureState::Retrying { error, retry_in } => self.emit(PipelineEvent::Notice {
                    error: true,
                    message: format!(
                        "Caméra indisponible : {error} — nouvel essai dans {:.0} s",
                        retry_in.as_secs_f32()
                    ),
                }),
                CaptureState::Streaming => {
                    if let Some(f) = &status.format {
                        let c = &self.config.camera;
                        let exact = self.source != Source::Camera
                            || (f.width == c.width
                                && f.height == c.height
                                && (f.fps - c.fps as f32).abs() < 0.5);
                        if !exact && !self.fallback_notified {
                            self.fallback_notified = true;
                            self.emit(PipelineEvent::Notice {
                                error: false,
                                message: format!(
                                    "Format demandé indisponible : repli sur {} {}×{} à {:.0} fps",
                                    f.subtype, f.width, f.height, f.fps
                                ),
                            });
                        }
                    }
                    if self.last_capture_state.is_some() {
                        self.emit(PipelineEvent::Notice {
                            error: false,
                            message: format!("Caméra « {} » connectée", status.device),
                        });
                    }
                }
                _ => {}
            }
            self.last_capture_state = Some(status.state.clone());
        }
        if status.fps_warning != self.fps_warned {
            self.fps_warned = status.fps_warning;
            if status.fps_warning {
                self.emit(PipelineEvent::Notice {
                    error: false,
                    message: format!(
                        "La caméra ne tient que {:.0} fps au lieu de {} (faible lumière ?)",
                        status.measured_fps, self.config.camera.fps
                    ),
                });
            }
        }
    }

    fn top3(labels: &[String], probs: &[f32]) -> Vec<(String, f32)> {
        let mut order: Vec<usize> = (0..probs.len().min(labels.len())).collect();
        order.sort_unstable_by(|a, b| probs[*b].total_cmp(&probs[*a]));
        order
            .into_iter()
            .take(3)
            .map(|i| (labels[i].clone(), probs[i]))
            .collect()
    }

    fn hand_status(&self, view: &HandView) -> HandStatus {
        let statics = self.stage.static_labels();
        HandStatus {
            track: view.track,
            right: view.handedness == Handedness::Right,
            top_static: Self::top3(statics, &view.static_probs),
            top_dynamic: Self::top3(self.stage.dynamic_labels(), &view.dynamic_probs),
            progress: view
                .progress
                .map(|p| (statics[p.label].clone(), p.progress, p.confirmed)),
            speed: view.speed,
            pinch: view.pinch,
        }
    }

    fn publish_status(&mut self, force: bool) {
        if !force && self.last_status.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_status = Instant::now();
        let window = self.fps_window.elapsed();
        if window >= Duration::from_secs(1) {
            self.processed_fps = self.processed_in_window as f32 / window.as_secs_f32();
            self.processed_in_window = 0;
            self.fps_window = Instant::now();
        }
        let engine = self.stage.engine();
        let dropped = self.dropped_tracking;
        let (static_classifier, dynamic_classifier) = self.stage.classifier_descriptions();
        let mut status = Status {
            state: engine.state(),
            capture: self
                .capture
                .as_ref()
                .map(CaptureHandle::status)
                .unwrap_or_else(|| CaptureStatus {
                    state: CaptureState::Stopped,
                    ..CaptureStatus::default()
                }),
            provider: self.provider.as_str(),
            static_classifier,
            dynamic_classifier,
            processed_fps: if self.paused { 0.0 } else { self.processed_fps },
            dropped,
            dropped_percent: 100.0 * dropped as f32
                / (self.frames_tracking + dropped).max(1) as f32,
            latencies: self.metrics.recent(),
            hands: self.stage.hands().map(|v| self.hand_status(v)).collect(),
            armed_remaining_ms: engine.armed_remaining_us(self.clock_us).map(|us| us / 1000),
            continuous: engine
                .active_continuous()
                .and_then(|i| self.config.continuous.get(i))
                .map(|c| c.name.clone()),
            idle: !self.paused && !self.tracker.is_tracking(),
            image_size: self.image_size,
            cpu_percent: 0.0,
            memory_mb: process_memory_bytes() as f32 / (1024.0 * 1024.0),
            recording: self.recorder.is_some(),
        };
        if let Ok(mut cumulative) = self.shared.cumulative.lock() {
            *cumulative = self.metrics.cumulative();
        }
        if let Ok(mut slot) = self.shared.status.lock() {
            // Le pourcentage CPU est mesuré sur une seconde glissante.
            status.cpu_percent = slot.cpu_percent;
            if self.last_metrics_reset.elapsed() >= Duration::from_secs(1) {
                status.cpu_percent = self.cpu.sample();
            }
            *slot = status;
        }
        if self.last_metrics_reset.elapsed() >= Duration::from_secs(1) {
            self.last_metrics_reset = Instant::now();
            // La fenêtre de latences couvre les 5 dernières secondes environ.
            if self.metrics.recent().last().is_some_and(|q| q.count > 300) {
                self.metrics.reset_window();
            }
        }
        if self.preview
            && let Some(notify) = &self.notify
        {
            notify();
        }
    }

    /// Copie l'image vers l'aperçu, au plus 30 fois par seconde (EF-71), et
    /// seulement fenêtre ouverte (EF-72).
    fn publish_preview(&mut self) {
        if !self.preview || self.last_preview.elapsed() < Duration::from_millis(33) {
            return;
        }
        self.last_preview = Instant::now();
        let image = self.decoder.image();
        // Sous-échantillonnage 2× des grandes images : l'aperçu n'a pas besoin de plus.
        let step = if image.width > 960 { 2 } else { 1 };
        let (w, h) = (image.width / step, image.height / step);
        let mirror = self.config.camera.mirror;
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            let row = &image.data[y * step * image.width * 3..];
            let out = &mut rgb[y * w * 3..(y + 1) * w * 3];
            for x in 0..w {
                let sx = if mirror {
                    image.width - 1 - x * step
                } else {
                    x * step
                };
                out[x * 3..x * 3 + 3].copy_from_slice(&row[sx * 3..sx * 3 + 3]);
            }
        }
        let statics = self.stage.static_labels();
        let hands = self
            .stage
            .hands()
            .map(|view| {
                let mut landmarks = [[0.0; 2]; 21];
                for (o, p) in landmarks.iter_mut().zip(&view.image) {
                    *o = [p[0], p[1]];
                }
                let roi = self
                    .hands
                    .iter()
                    .find(|h| h.track_id == view.track)
                    .map_or([[0.0; 2]; 4], |h| h.roi.corners());
                let best = mudra_gestures::argmax(&view.static_probs);
                PreviewHand {
                    landmarks,
                    roi,
                    right: view.handedness == Handedness::Right,
                    label: (statics[best] != mudra_gestures::NONE_LABEL)
                        .then(|| (statics[best].clone(), view.static_probs[best])),
                }
            })
            .collect();
        let frame = PreviewFrame {
            width: w,
            height: h,
            rgb,
            source_size: (image.width as f32, image.height as f32),
            hands,
        };
        if let Ok(mut slot) = self.shared.preview.try_lock() {
            *slot = Some(frame);
        }
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}
