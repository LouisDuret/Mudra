//! Vecteurs dorés des classifieurs (§8.5, §10.1) : les sorties de l'application
//! doivent être identiques à celles calculées en Python au moment de l'export.
//!
//! Les fichiers `tests/golden/*.json` sont écrits par `uv run mudra-train all`.
//! Sans eux (aucun modèle entraîné), les tests sont ignorés avec un message.
//! `MUDRA_GOLDEN_DIR` permet de pointer vers un autre dossier (intégration continue).

use std::path::{Path, PathBuf};

use mudra_gestures::onnx::{OnnxDynamicClassifier, OnnxStaticClassifier};
use mudra_gestures::window::WindowSample;
use mudra_gestures::{DynamicClassifier, HandSample, Handedness, Landmarks, StaticClassifier};
use serde::Deserialize;

const TOLERANCE: f32 = 1e-4;

#[derive(Deserialize)]
struct Golden<C> {
    model: String,
    feature_version: String,
    cases: Vec<C>,
}

#[derive(Deserialize)]
struct StaticCase {
    world: Landmarks,
    handedness: f32,
    probs: Vec<f32>,
}

#[derive(Deserialize)]
struct DynamicCase {
    image: Vec<Landmarks>,
    world: Vec<Landmarks>,
    handedness: f32,
    probs: Vec<f32>,
}

fn golden_dir() -> PathBuf {
    std::env::var_os("MUDRA_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden"))
}

fn load<C: for<'de> Deserialize<'de>>(name: &str) -> Option<(Golden<C>, PathBuf)> {
    let dir = golden_dir();
    let path = dir.join(name);
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "{} absent : test ignoré (entraîne d'abord les modèles)",
            path.display()
        );
        return None;
    };
    let golden: Golden<C> = serde_json::from_str(&text).expect("fichier doré lisible");
    assert_eq!(
        golden.feature_version,
        mudra_gestures::onnx::FEATURE_VERSION
    );
    let model = dir.join(&golden.model);
    Some((golden, model))
}

fn handedness(value: f32) -> Handedness {
    if value > 0.5 {
        Handedness::Right
    } else {
        Handedness::Left
    }
}

fn max_gap(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn static_classifier_matches_python() {
    let Some((golden, model)) = load::<StaticCase>("static_golden.json") else {
        return;
    };
    let mut classifier = OnnxStaticClassifier::load(&model).expect("chargement du modèle statique");
    assert!(classifier.labels().iter().any(|l| l == "none"));
    assert!(!golden.cases.is_empty());
    let mut probs = vec![0.0; classifier.labels().len()];
    for (i, case) in golden.cases.iter().enumerate() {
        // Le modèle statique ne lit que les points monde et la latéralité.
        let sample = HandSample {
            image: case.world,
            world: case.world,
            handedness: handedness(case.handedness),
        };
        classifier.classify(&sample, &mut probs).unwrap();
        let gap = max_gap(&probs, &case.probs);
        assert!(gap < TOLERANCE, "cas {i} : écart {gap}");
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-3);
    }
}

#[test]
fn dynamic_classifier_matches_python() {
    let Some((golden, model)) = load::<DynamicCase>("dynamic_golden.json") else {
        return;
    };
    let mut classifier =
        OnnxDynamicClassifier::load(&model).expect("chargement du modèle dynamique");
    assert_eq!(classifier.window_len(), 32);
    assert_eq!(classifier.sample_rate_hz(), 60.0);
    let mut probs = vec![0.0; classifier.labels().len()];
    for (i, case) in golden.cases.iter().enumerate() {
        let window: Vec<WindowSample> = case
            .image
            .iter()
            .zip(&case.world)
            .map(|(image, world)| WindowSample {
                image: *image,
                world: *world,
            })
            .collect();
        classifier
            .classify(&window, handedness(case.handedness), &mut probs)
            .unwrap();
        let gap = max_gap(&probs, &case.probs);
        assert!(gap < TOLERANCE, "cas {i} : écart {gap}");
    }
}
