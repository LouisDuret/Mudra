//! Classifieurs ONNX entraînés par `training/` (§7.6, D-08 à D-10).
//!
//! L'extraction des features fait partie du graphe (D-09) : Rust ne fournit que
//! les points-clés lissés et la latéralité. Les labels et les paramètres de
//! fenêtre sont lus dans les `metadata_props` du modèle (D-10).

use std::path::Path;

use ort::session::Session;
use ort::value::TensorRef;

use crate::window::WindowSample;
use crate::{
    ClassifierError, DynamicClassifier, HandSample, Handedness, NONE_LABEL, StaticClassifier,
};

/// Version du format d'entrée attendue par ce binaire.
pub const FEATURE_VERSION: &str = "1";

struct Loaded {
    session: Session,
    labels: Vec<String>,
    name: String,
    window: Option<(usize, f32)>,
}

fn load(path: &Path, dynamic: bool) -> Result<Loaded, ClassifierError> {
    let display = path.display().to_string();
    let model_err = |message: String| ClassifierError::Model {
        path: display.clone(),
        message,
    };
    let ort_err = |e: ort::Error| model_err(e.to_string());

    // D-08 : EP CPU, un seul thread intra-op.
    let session = Session::builder()
        .map_err(ort_err)?
        .with_intra_threads(1)
        .map_err(|e| model_err(e.to_string()))?
        .commit_from_file(path)
        .map_err(ort_err)?;

    let metadata = session.metadata().map_err(ort_err)?;
    let custom = |key: &str| {
        metadata
            .custom(key)
            .ok_or_else(|| model_err(format!("métadonnée « {key} » absente (D-10)")))
    };
    let labels: Vec<String> = serde_json::from_str(&custom("labels")?)
        .map_err(|e| model_err(format!("métadonnée « labels » illisible : {e}")))?;
    if !labels.iter().any(|l| l == NONE_LABEL) {
        return Err(model_err("la classe « none » est obligatoire".into()));
    }
    let version = custom("feature_version")?;
    if version != FEATURE_VERSION {
        return Err(model_err(format!(
            "feature_version = {version}, ce binaire attend {FEATURE_VERSION} : ré-entraîne le modèle"
        )));
    }
    let window = if dynamic {
        let t: usize = custom("window_T")?
            .parse()
            .map_err(|_| model_err("window_T invalide".into()))?;
        let hz: f32 = custom("sample_rate_hz")?
            .parse()
            .map_err(|_| model_err("sample_rate_hz invalide".into()))?;
        if !(2..=512).contains(&t) || !(1.0..=240.0).contains(&hz) {
            return Err(model_err(format!("fenêtre T = {t} à {hz} Hz hors bornes")));
        }
        Some((t, hz))
    } else {
        None
    };
    drop(metadata);

    let expected: &[&str] = if dynamic {
        &["image", "world", "handedness"]
    } else {
        &["world", "handedness"]
    };
    for name in expected {
        if !session.inputs().iter().any(|i| i.name() == *name) {
            return Err(model_err(format!("entrée « {name} » absente")));
        }
    }
    if !session.outputs().iter().any(|o| o.name() == "probs") {
        return Err(model_err("sortie « probs » absente".into()));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(display);
    Ok(Loaded {
        session,
        labels,
        name,
        window,
    })
}

fn copy_probs(
    outputs: &ort::session::SessionOutputs<'_>,
    out: &mut [f32],
) -> Result<(), ClassifierError> {
    let (_, probs) = outputs["probs"]
        .try_extract_tensor::<f32>()
        .map_err(|e| ClassifierError::Inference(e.to_string()))?;
    if probs.len() != out.len() {
        return Err(ClassifierError::Inference(format!(
            "le modèle renvoie {} probabilités pour {} labels",
            probs.len(),
            out.len()
        )));
    }
    out.copy_from_slice(probs);
    Ok(())
}

fn handedness_value(h: Handedness) -> [f32; 1] {
    match h {
        Handedness::Left => [0.0],
        Handedness::Right => [1.0],
    }
}

fn inference(e: ort::Error) -> ClassifierError {
    ClassifierError::Inference(e.to_string())
}

/// Modèle statique : `world [1,21,3]`, `handedness [1,1]` → `probs [1,K]`.
pub struct OnnxStaticClassifier {
    session: Session,
    labels: Vec<String>,
    name: String,
}

impl OnnxStaticClassifier {
    pub fn load(path: &Path) -> Result<Self, ClassifierError> {
        let Loaded {
            session,
            labels,
            name,
            ..
        } = load(path, false)?;
        Ok(Self {
            session,
            labels,
            name,
        })
    }
}

impl StaticClassifier for OnnxStaticClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn classify(&mut self, hand: &HandSample, out: &mut [f32]) -> Result<(), ClassifierError> {
        let handedness = handedness_value(hand.handedness);
        let world = TensorRef::from_array_view(([1usize, 21, 3], hand.world.as_flattened()))
            .map_err(inference)?;
        let handedness =
            TensorRef::from_array_view(([1usize, 1], &handedness[..])).map_err(inference)?;
        let outputs = self
            .session
            .run(ort::inputs!["world" => world, "handedness" => handedness])
            .map_err(inference)?;
        copy_probs(&outputs, out)
    }

    fn description(&self) -> String {
        self.name.clone()
    }
}

/// Modèle dynamique : `image [1,T,21,3]`, `world [1,T,21,3]`, `handedness [1,1]` → `probs [1,K]`.
pub struct OnnxDynamicClassifier {
    session: Session,
    labels: Vec<String>,
    name: String,
    window_len: usize,
    sample_rate_hz: f32,
    image: Vec<f32>,
    world: Vec<f32>,
}

impl OnnxDynamicClassifier {
    pub fn load(path: &Path) -> Result<Self, ClassifierError> {
        let Loaded {
            session,
            labels,
            name,
            window,
        } = load(path, true)?;
        let (window_len, sample_rate_hz) = window.unwrap_or((32, 60.0));
        Ok(Self {
            session,
            labels,
            name,
            window_len,
            sample_rate_hz,
            image: vec![0.0; window_len * 63],
            world: vec![0.0; window_len * 63],
        })
    }
}

impl DynamicClassifier for OnnxDynamicClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn window_len(&self) -> usize {
        self.window_len
    }

    fn sample_rate_hz(&self) -> f32 {
        self.sample_rate_hz
    }

    fn classify(
        &mut self,
        window: &[WindowSample],
        handedness: Handedness,
        out: &mut [f32],
    ) -> Result<(), ClassifierError> {
        if window.len() != self.window_len {
            return Err(ClassifierError::Inference(format!(
                "fenêtre de {} trames, {} attendues",
                window.len(),
                self.window_len
            )));
        }
        for (i, s) in window.iter().enumerate() {
            self.image[i * 63..(i + 1) * 63].copy_from_slice(s.image.as_flattened());
            self.world[i * 63..(i + 1) * 63].copy_from_slice(s.world.as_flattened());
        }
        let handedness = handedness_value(handedness);
        let shape = [1usize, self.window_len, 21, 3];
        let image =
            TensorRef::from_array_view((shape, self.image.as_slice())).map_err(inference)?;
        let world =
            TensorRef::from_array_view((shape, self.world.as_slice())).map_err(inference)?;
        let handedness =
            TensorRef::from_array_view(([1usize, 1], &handedness[..])).map_err(inference)?;
        let outputs = self
            .session
            .run(ort::inputs!["image" => image, "world" => world, "handedness" => handedness])
            .map_err(inference)?;
        copy_probs(&outputs, out)
    }

    fn description(&self) -> String {
        self.name.clone()
    }
}
