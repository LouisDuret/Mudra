//! Structures sérialisables de la configuration (Annexe C du cahier des charges).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub camera: Camera,
    pub inference: Inference,
    pub models: Models,
    pub smoothing: Smoothing,
    pub activation: Activation,
    pub defaults: Defaults,
    pub bindings: Vec<Binding>,
    pub continuous: Vec<Continuous>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub log_level: String,
    /// Active l'action `command` (ENF-22).
    pub allow_commands: bool,
    pub sounds: bool,
    pub overlay: bool,
    pub autostart: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            allow_commands: false,
            sounds: true,
            overlay: true,
            autostart: false,
        }
    }
}

/// Caméra désignée par sous-chaîne du nom ou par index.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DeviceSelector {
    Index(u32),
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Mjpeg,
    Nv12,
    Yuy2,
}

/// `"auto"` ou une valeur manuelle (échelle log2 des secondes, ex. `-6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    Auto,
    Manual(i32),
}

impl<'de> Deserialize<'de> for Exposure {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i32),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(v) => Ok(Exposure::Manual(v)),
            Raw::Text(s) if s.eq_ignore_ascii_case("auto") => Ok(Exposure::Auto),
            Raw::Text(s) => Err(serde::de::Error::custom(format!(
                "exposure : « {s} » invalide, attendu \"auto\" ou un entier"
            ))),
        }
    }
}

impl Serialize for Exposure {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Exposure::Auto => s.serialize_str("auto"),
            Exposure::Manual(v) => s.serialize_i32(*v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Camera {
    pub device: DeviceSelector,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub mirror: bool,
    pub exposure: Exposure,
    pub auto_exposure_priority: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            device: DeviceSelector::Index(0),
            format: PixelFormat::Mjpeg,
            width: 1280,
            height: 720,
            fps: 60,
            mirror: true,
            exposure: Exposure::Auto,
            auto_exposure_priority: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionProvider {
    Directml,
    Cpu,
    Cuda,
    Tensorrt,
}

impl ExecutionProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Directml => "directml",
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Tensorrt => "tensorrt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Inference {
    pub execution_provider: ExecutionProvider,
    pub max_hands: u32,
    pub palm_score_threshold: f32,
    pub presence_threshold: f32,
    pub redetect_every_n_frames: u32,
    pub idle_detection_hz: u32,
}

impl Default for Inference {
    fn default() -> Self {
        Self {
            // Défaut fixé par le benchmark B-01 (docs/adr/0001-fournisseur-execution.md).
            execution_provider: ExecutionProvider::Cpu,
            max_hands: 1,
            palm_score_threshold: 0.5,
            presence_threshold: 0.6,
            redetect_every_n_frames: 10,
            idle_detection_hz: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Models {
    pub palm: String,
    pub landmarks: String,
    /// Classifieur de poses ; fichier absent ou `"builtin"` → classifieur géométrique intégré.
    #[serde(rename = "static")]
    pub static_: String,
    /// Classifieur de gestes dynamiques ; même repli que `static`.
    pub dynamic: String,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            palm: "models/palm_detection_mediapipe_2023feb.onnx".into(),
            landmarks: "models/handpose_estimation_mediapipe_2023feb.onnx".into(),
            static_: "models/static_gestures.onnx".into(),
            dynamic: "models/dynamic_gestures.onnx".into(),
        }
    }
}

/// Paramètres du filtre One Euro (D-11).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Smoothing {
    pub min_cutoff: f32,
    pub beta: f32,
    pub d_cutoff: f32,
}

impl Default for Smoothing {
    fn default() -> Self {
        // beta calibré pour des vitesses en tailles de paume par seconde (docs/adr/0002).
        Self {
            min_cutoff: 1.0,
            beta: 1.5,
            d_cutoff: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    WakeGesture,
    AlwaysOn,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Activation {
    pub mode: ActivationMode,
    pub wake_gesture: String,
    pub wake_hold_ms: u64,
    pub lock_gesture: String,
    pub armed_duration_ms: u64,
    pub pause_hotkey: String,
}

impl Default for Activation {
    fn default() -> Self {
        Self {
            mode: ActivationMode::WakeGesture,
            wake_gesture: "open_palm".into(),
            wake_hold_ms: 600,
            lock_gesture: "fist".into(),
            armed_duration_ms: 4000,
            pause_hotkey: "Ctrl+Alt+G".into(),
        }
    }
}

/// Valeurs par défaut des bindings (EF-31 à EF-34).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub static_enter: f32,
    pub static_exit: f32,
    pub hold_ms: u64,
    pub cooldown_ms: u64,
    pub dynamic_threshold: f32,
    pub dynamic_refractory_ms: u64,
    /// Durée sous `static_exit` avant de considérer la pose relâchée (hystérésis).
    pub release_ms: u64,
    /// Vitesse (tailles de paume par seconde) au-delà de laquelle aucune pose n'est confirmée.
    pub max_pose_speed: f32,
    /// Suspension des poses statiques après un geste dynamique (EF-33).
    pub dynamic_suppress_ms: u64,
    /// Sortie du mode continu : pose d'entrée relâchée plus longtemps que ce délai.
    pub continuous_release_ms: u64,
    /// Sortie du mode continu : main perdue plus longtemps que ce délai.
    pub hand_lost_ms: u64,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            static_enter: 0.85,
            static_exit: 0.60,
            hold_ms: 300,
            cooldown_ms: 800,
            dynamic_threshold: 0.80,
            dynamic_refractory_ms: 600,
            release_ms: 100,
            max_pose_speed: 2.5,
            dynamic_suppress_ms: 300,
            continuous_release_ms: 150,
            hand_lost_ms: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandFilter {
    Left,
    Right,
    #[default]
    Any,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub gesture: String,
    #[serde(default)]
    pub hand: HandFilter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_ms: Option<u64>,
    /// Répétition tant que la pose est maintenue (EF-31).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_enter: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_exit: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_threshold: Option<f32>,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeOp {
    Step,
    Set,
    MuteToggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeMode {
    /// `IAudioEndpointVolume` : précis et silencieux.
    #[default]
    Api,
    /// Touches `VK_VOLUME_*` : affiche l'OSD Windows.
    Keys,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Prev,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotTarget {
    Screen,
    #[default]
    ActiveMonitor,
    ActiveWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseOp {
    LeftClick,
    RightClick,
    Scroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    #[default]
    Cmd,
    Powershell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InternalOp {
    TogglePause,
    Lock,
    Notify,
}

fn default_screenshot_dir() -> String {
    "~/Pictures/Mudra".into()
}

/// Action Windows associée à un geste (§5.5).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Launch {
        path: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Volume {
        op: VolumeOp,
        #[serde(default)]
        delta: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<f32>,
        #[serde(default)]
        mode: VolumeMode,
    },
    Media {
        key: MediaKey,
    },
    Keys {
        combo: String,
    },
    Screenshot {
        #[serde(default)]
        target: ScreenshotTarget,
        #[serde(default = "default_screenshot_dir")]
        dir: String,
        #[serde(default)]
        clipboard: bool,
    },
    Snip,
    Mouse {
        op: MouseOp,
        /// Crans de molette pour `scroll` (positif = vers le haut).
        #[serde(default)]
        amount: i32,
    },
    Command {
        command: String,
        #[serde(default)]
        shell: Shell,
    },
    Sequence {
        steps: Vec<SequenceStep>,
    },
    Internal {
        op: InternalOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

impl Action {
    /// Libellé court pour les journaux et l'interface.
    pub fn summary(&self) -> String {
        match self {
            Action::Launch { path, .. } => format!("lancer {path}"),
            Action::Volume {
                op: VolumeOp::Step,
                delta,
                ..
            } => {
                format!("volume {:+.0} %", delta * 100.0)
            }
            Action::Volume {
                op: VolumeOp::Set,
                value,
                ..
            } => {
                format!("volume = {:.0} %", value.unwrap_or(0.0) * 100.0)
            }
            Action::Volume {
                op: VolumeOp::MuteToggle,
                ..
            } => "couper / rétablir le son".into(),
            Action::Media { key } => match key {
                MediaKey::PlayPause => "lecture / pause".into(),
                MediaKey::Next => "piste suivante".into(),
                MediaKey::Prev => "piste précédente".into(),
                MediaKey::Stop => "stop".into(),
            },
            Action::Keys { combo } => format!("touches {combo}"),
            Action::Screenshot { target, .. } => match target {
                ScreenshotTarget::Screen => "capture de l'écran".into(),
                ScreenshotTarget::ActiveMonitor => "capture du moniteur actif".into(),
                ScreenshotTarget::ActiveWindow => "capture de la fenêtre active".into(),
            },
            Action::Snip => "outil Capture d'écran".into(),
            Action::Mouse { op, amount } => match op {
                MouseOp::LeftClick => "clic gauche".into(),
                MouseOp::RightClick => "clic droit".into(),
                MouseOp::Scroll => format!("défilement {amount:+}"),
            },
            Action::Command { command, .. } => format!("commande {command}"),
            Action::Sequence { steps } => format!("séquence de {} actions", steps.len()),
            Action::Internal { op, message } => match op {
                InternalOp::TogglePause => "pause / reprise".into(),
                InternalOp::Lock => "verrouiller".into(),
                InternalOp::Notify => {
                    format!("notification : {}", message.as_deref().unwrap_or(""))
                }
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceStep {
    pub action: Action,
    /// Délai avant l'étape suivante.
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_enter_hold_ms() -> u64 {
    250
}

/// Contrôle continu (§5.6).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Continuous {
    pub name: String,
    pub enter_gesture: String,
    #[serde(default = "default_enter_hold_ms")]
    pub enter_hold_ms: u64,
    #[serde(default)]
    pub hand: HandFilter,
    pub control: Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeSource {
    #[default]
    VerticalDisplacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseTarget {
    #[default]
    Primary,
    VirtualDesktop,
}

fn default_gain() -> f32 {
    1.5
}
fn default_dead_zone() -> f32 {
    0.02
}
fn default_active_zone() -> [f32; 4] {
    [0.2, 0.2, 0.8, 0.8]
}
fn default_mouse_min_cutoff() -> f32 {
    0.8
}
fn default_mouse_beta() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    /// EF-50 : le volume suit le déplacement vertical de la main depuis l'entrée.
    Volume {
        #[serde(default)]
        source: VolumeSource,
        #[serde(default = "default_gain")]
        gain: f32,
        #[serde(default = "default_dead_zone")]
        dead_zone: f32,
    },
    /// EF-51 : le curseur suit l'index, clic par pincement.
    Mouse {
        #[serde(default = "default_active_zone")]
        active_zone: [f32; 4],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        click: Option<String>,
        #[serde(default = "default_mouse_min_cutoff")]
        min_cutoff: f32,
        #[serde(default = "default_mouse_beta")]
        beta: f32,
        #[serde(default)]
        target: MouseTarget,
    },
}
