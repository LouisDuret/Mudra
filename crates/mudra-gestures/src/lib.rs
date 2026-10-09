//! Lissage, classifieurs et moteur de gestes de Mudra (§7.5 à §7.7).
//!
//! Ce crate ne fait aucune entrée/sortie hormis le chargement des classifieurs
//! ONNX : le moteur est une machine à états déterministe à horloge injectée
//! (D-12), testable par rejeu.

pub mod builtin;
pub mod engine;
pub mod geometry;
pub mod one_euro;
pub mod onnx;
pub mod stage;
pub mod window;

pub use engine::{ContinuousValue, Engine, EngineEvent, EngineState, HandObs, PoseProgress};

/// 21 points-clés (Annexe A), `[x, y, z]`.
pub type Landmarks = [[f32; 3]; 21];

pub const NUM_LANDMARKS: usize = 21;

/// Étiquette de la classe « tout le reste », obligatoire dans chaque modèle.
pub const NONE_LABEL: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handedness {
    Left,
    Right,
}

impl Handedness {
    pub fn as_str(self) -> &'static str {
        match self {
            Handedness::Left => "left",
            Handedness::Right => "right",
        }
    }

    pub fn matches(self, filter: mudra_config::HandFilter) -> bool {
        use mudra_config::HandFilter;
        matches!(
            (self, filter),
            (_, HandFilter::Any)
                | (Handedness::Left, HandFilter::Left)
                | (Handedness::Right, HandFilter::Right)
        )
    }
}

/// Une main, telle que vue par les classifieurs.
#[derive(Debug, Clone, Copy)]
pub struct HandSample {
    /// Points image dans l'espace affiché (miroir appliqué), divisés par la
    /// largeur de l'image : repère isotrope, x vers la droite, y vers le bas.
    pub image: Landmarks,
    /// Points monde en mètres, alignés sur les axes de l'image affichée.
    pub world: Landmarks,
    pub handedness: Handedness,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("modèle {path} : {message}")]
    Model { path: String, message: String },
    #[error("inférence du classifieur : {0}")]
    Inference(String),
}

/// Classifieur de poses statiques (EF-20). `out` reçoit une distribution de
/// probabilités alignée sur `labels()`, classe `none` incluse.
pub trait StaticClassifier: Send {
    fn labels(&self) -> &[String];
    fn classify(&mut self, hand: &HandSample, out: &mut [f32]) -> Result<(), ClassifierError>;
    /// Nom affiché dans l'aperçu (« intégré », nom du fichier ONNX…).
    fn description(&self) -> String;
}

/// Classifieur de gestes dynamiques sur fenêtre glissante (EF-21).
pub trait DynamicClassifier: Send {
    fn labels(&self) -> &[String];
    /// Nombre de trames T de la fenêtre.
    fn window_len(&self) -> usize;
    fn sample_rate_hz(&self) -> f32;
    /// `window` contient exactement `window_len()` échantillons, du plus ancien au plus récent.
    fn classify(
        &mut self,
        window: &[window::WindowSample],
        handedness: Handedness,
        out: &mut [f32],
    ) -> Result<(), ClassifierError>;
    fn description(&self) -> String;
}

/// Indice de la plus grande valeur.
pub fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}
