//! Mouvements personnalisés appris par l'exemple.
//!
//! Un mouvement perso est un ensemble d'exemples de la trajectoire de la
//! main : chemin de son centre, rapprochement ou éloignement de la caméra, et
//! ouverture des doigts pendant le geste. En direct, la fin de la fenêtre
//! glissante est comparée à chaque exemple, à plusieurs vitesses ; le seuil
//! d'acceptation est calibré sur l'écart des prises entre elles. Ces
//! mouvements s'ajoutent au classifieur dynamique de base (intégré ou ONNX) au
//! lieu de le remplacer : aucun ré-entraînement, aucune dépendance Python.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::builtin::FingerState;
use crate::custom::{Resemblance, confidence, overlay};
use crate::geometry::{hand_center, palm_size_2d};
use crate::window::WindowSample;
use crate::{ClassifierError, DynamicClassifier, Handedness, NONE_LABEL};

/// Version du format de fichier et du descripteur.
pub const FORMAT_VERSION: u32 = 1;
/// Cadence à laquelle les prises sont ré-échantillonnées pour l'apprentissage.
pub const SAMPLE_RATE_HZ: f32 = 60.0;
/// Instants auxquels la trajectoire est relevée, du début à la fin du geste.
const POINTS: usize = 16;
/// Par instant : x, y du centre de la main, puis sa profondeur.
const PATH: usize = 3 * POINTS;
/// Ouverture moyenne des quatre doigts longs, puis du pouce.
const SHAPE: usize = 5;
pub const DESCRIPTOR: usize = PATH + SHAPE;
type Descriptor = [f32; DESCRIPTOR];

/// Poids de la profondeur, lue sur la taille apparente de la paume : main
/// avancée de 30 %, elle compte comme trois quarts de paume de déplacement.
const DEPTH_WEIGHT: f32 = 3.0;
/// Poids de la forme de la main : le même trajet, poing fermé ou main
/// ouverte, s'écarte d'une demi-paume.
const SHAPE_WEIGHT: f32 = 0.5;
/// Vitesse, en tailles de paume par seconde, à partir de laquelle la main
/// « bouge », mesurée sur un dixième de seconde.
const MOVING: f32 = 1.5;
const SPEED_BASE_SECONDS: f32 = 0.1;
/// Un arrêt plus court que cela fait partie du geste (demi-tour d'un va-et-vient).
const PAUSE_SECONDS: f32 = 0.15;
/// Durée d'un mouvement : assez long pour être vu, assez bref pour tenir dans
/// la fenêtre glissante.
const MIN_SECONDS: f32 = 0.2;
pub const MAX_SECONDS: f32 = 1.2;
/// Amplitude minimale, en tailles de paume (écart quadratique moyen au centre
/// de la trajectoire) : en dessous, le geste se confond avec une main qui tremble.
const MIN_RADIUS: f32 = 0.45;
/// Bornes du seuil calibré. Le plafond est une part de l'amplitude du geste :
/// une main immobile est à une amplitude entière de tout exemple.
const MIN_THRESHOLD: f32 = 0.22;
const MAX_THRESHOLD_SHARE: f32 = 0.5;
/// Marge appliquée à l'écart mesuré entre les prises.
const THRESHOLD_MARGIN: f32 = 1.4;
/// Seuil d'un mouvement appris sur une seule prise, en part de son amplitude.
const SINGLE_TAKE_SHARE: f32 = 0.35;
/// Bornes du réglage de sensibilité proposé à l'utilisateur.
pub const SENSITIVITY_RANGE: (f32, f32) = (0.6, 1.3);
/// Vitesses essayées en direct, rapportées à celle de l'exemple.
const SCALES: [f32; 5] = [0.75, 0.87, 1.0, 1.15, 1.33];
/// Le tronçon comparé doit avoir au moins cette part de l'amplitude de l'exemple.
const RADIUS_GATE: f32 = 0.6;
/// Part des prises à partir de laquelle une ressemblance est signalée.
const RESEMBLANCE_SHARE: f32 = 0.3;

fn euclidean(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Amplitude de la trajectoire d'un descripteur, en tailles de paume.
fn radius(descriptor: &[f32]) -> f32 {
    descriptor[..PATH].iter().map(|v| v * v).sum::<f32>().sqrt()
}

/// Mesures de la main sur une suite d'instants régulièrement espacés.
#[derive(Default)]
struct Measures {
    centers: Vec<[f32; 2]>,
    palms: Vec<f32>,
    shapes: Vec<[f32; SHAPE]>,
}

impl Measures {
    fn fill(&mut self, samples: &[WindowSample]) {
        self.centers.clear();
        self.palms.clear();
        self.shapes.clear();
        for sample in samples {
            self.centers.push(hand_center(&sample.image));
            self.palms.push(palm_size_2d(&sample.image).max(1e-6));
            let fingers = FingerState::from_world(&sample.world);
            let [a, b, c, d] = fingers.extended;
            self.shapes.push([a, b, c, d, fingers.thumb_out]);
        }
    }

    fn len(&self) -> usize {
        self.centers.len()
    }

    /// Descripteur du tronçon allant de l'instant `from` à l'instant `to`
    /// (indices, éventuellement fractionnaires). La trajectoire est ramenée à
    /// son centre et à la taille de la paume ; chaque terme est pondéré pour
    /// qu'une distance euclidienne entre descripteurs soit un écart moyen, en
    /// tailles de paume.
    fn describe(&self, from: f32, to: f32) -> Descriptor {
        let last = self.len().saturating_sub(1);
        let mut points = [[0.0f32; 3]; POINTS];
        let mut shape = [0.0f32; SHAPE];
        for (i, point) in points.iter_mut().enumerate() {
            let at = (from + (to - from) * i as f32 / (POINTS - 1) as f32).clamp(0.0, last as f32);
            let low = at.floor() as usize;
            let high = (low + 1).min(last);
            let w = at - low as f32;
            let mix = |a: f32, b: f32| a + (b - a) * w;
            *point = [
                mix(self.centers[low][0], self.centers[high][0]),
                mix(self.centers[low][1], self.centers[high][1]),
                mix(self.palms[low], self.palms[high]),
            ];
            let nearest = if w < 0.5 { low } else { high };
            for (sum, value) in shape.iter_mut().zip(self.shapes[nearest]) {
                *sum += value;
            }
        }
        let mean = |k: usize| points.iter().map(|p| p[k]).sum::<f32>() / POINTS as f32;
        let (cx, cy, palm) = (mean(0), mean(1), mean(2).max(1e-6));
        let per_point = 1.0 / (POINTS as f32).sqrt();
        let mut out = [0.0f32; DESCRIPTOR];
        for (i, point) in points.iter().enumerate() {
            out[3 * i] = (point[0] - cx) / palm * per_point;
            out[3 * i + 1] = (point[1] - cy) / palm * per_point;
            out[3 * i + 2] = DEPTH_WEIGHT * (point[2] / palm).max(1e-6).ln() * per_point;
        }
        let per_finger = SHAPE_WEIGHT / (SHAPE as f32).sqrt() / POINTS as f32;
        for (k, sum) in shape.iter().enumerate() {
            out[PATH + k] = sum * per_finger;
        }
        out
    }

    /// Tronçon où la main bouge : premier et dernier instants du geste le
    /// plus ample de la suite, cadencée à `rate` hertz.
    fn segment(&self, rate: f32) -> Option<(usize, usize)> {
        let base = ((SPEED_BASE_SECONDS * rate).round() as usize).max(1);
        let pause = (PAUSE_SECONDS * rate).round() as usize;
        // Vitesse sur un dixième de seconde : le bruit d'une image à l'autre
        // ne passe pas pour un mouvement.
        let speed = |i: usize| -> f32 {
            if i < base {
                return 0.0;
            }
            let j = i - base;
            let dx = (self.centers[i][0] - self.centers[j][0]) / self.palms[i];
            let dy = (self.centers[i][1] - self.centers[j][1]) / self.palms[i];
            let dz = DEPTH_WEIGHT * (self.palms[i] / self.palms[j]).ln();
            (dx * dx + dy * dy + dz * dz).sqrt() * rate / base as f32
        };
        let mut best: Option<(f32, usize, usize)> = None;
        let mut run: Option<(usize, usize, f32)> = None;
        for i in 0..=self.len() {
            let v = if i < self.len() { speed(i) } else { 0.0 };
            if v > MOVING {
                let (start, _, travelled) = run.unwrap_or((i, i, 0.0));
                run = Some((start, i, travelled + v / rate));
            } else if let Some((start, end, travelled)) = run
                && (i == self.len() || i - end > pause)
            {
                if best.is_none_or(|(most, _, _)| travelled > most) {
                    best = Some((travelled, start, end));
                }
                run = None;
            }
        }
        // La vitesse à l'instant i décrit le dixième de seconde qui le précède.
        best.map(|(_, start, end)| (start.saturating_sub(base), end))
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MoveError {
    #[error("aucune prise enregistrée")]
    NoTakes,
    #[error("prise {take} : aucun mouvement net n'a été vu, recommence-la")]
    NoMovement { take: usize },
    #[error("prise {take} : le mouvement dure plus de {MAX_SECONDS} s, fais-le plus bref")]
    TooLong { take: usize },
    #[error("prise {take} : le mouvement est trop petit, fais-le plus ample")]
    TooSmall { take: usize },
}

/// Un exemple de mouvement : sa durée, et sa trajectoire ramenée à
/// [`DESCRIPTOR`] nombres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveExample {
    pub duration_ms: u32,
    pub descriptor: Vec<f32>,
}

/// Un mouvement personnalisé, tel qu'il est enregistré sur le disque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMove {
    pub version: u32,
    /// Identifiant snake_case : label du classifieur et nom dans la configuration.
    pub label: String,
    /// Nom affiché.
    pub name: String,
    /// Multiplicateur du seuil : moins de 1 est plus strict, plus de 1 plus tolérant.
    pub sensitivity: f32,
    /// Distance d'acceptation calibrée sur les exemples.
    pub threshold: f32,
    #[serde(default)]
    pub resemblances: Vec<Resemblance>,
    /// Un exemple par prise.
    pub examples: Vec<MoveExample>,
}

/// Exemple tiré d'une prise cadencée à [`SAMPLE_RATE_HZ`] : le geste le plus
/// ample qu'elle contient.
fn example_of(take: &[WindowSample], number: usize) -> Result<MoveExample, MoveError> {
    let mut measures = Measures::default();
    measures.fill(take);
    let (start, end) = measures
        .segment(SAMPLE_RATE_HZ)
        .ok_or(MoveError::NoMovement { take: number })?;
    let seconds = (end - start) as f32 / SAMPLE_RATE_HZ;
    if seconds < MIN_SECONDS {
        return Err(MoveError::NoMovement { take: number });
    }
    let descriptor = measures.describe(start as f32, end as f32);
    let long = seconds > MAX_SECONDS;
    if radius(&descriptor) < MIN_RADIUS {
        // Une main qui dérive longtemps sans aller nulle part n'a rien fait.
        return Err(if long {
            MoveError::NoMovement { take: number }
        } else {
            MoveError::TooSmall { take: number }
        });
    }
    if long {
        return Err(MoveError::TooLong { take: number });
    }
    Ok(MoveExample {
        duration_ms: (seconds * 1000.0).round() as u32,
        descriptor: descriptor.to_vec(),
    })
}

/// Vérifie qu'une prise contient un mouvement exploitable.
pub fn check_take(take: &[WindowSample]) -> Result<(), MoveError> {
    example_of(take, 1).map(|_| ())
}

/// Ramène une prise horodatée (microsecondes) à [`SAMPLE_RATE_HZ`], par
/// interpolation entre les images voisines.
pub fn uniform(frames: &[(u64, WindowSample)]) -> Vec<WindowSample> {
    let (Some(&(first, _)), Some(&(last, _))) = (frames.first(), frames.last()) else {
        return Vec::new();
    };
    let period = 1e6 / f64::from(SAMPLE_RATE_HZ);
    let count = ((last - first) as f64 / period).floor() as usize + 1;
    let mut out = Vec::with_capacity(count);
    let mut cursor = 0;
    for i in 0..count {
        let t = first as f64 + i as f64 * period;
        while cursor + 1 < frames.len() && (frames[cursor + 1].0 as f64) <= t {
            cursor += 1;
        }
        let (t0, a) = &frames[cursor];
        out.push(match frames.get(cursor + 1) {
            Some((t1, b)) if t1 > t0 => {
                let w = ((t - *t0 as f64) / (*t1 - *t0) as f64).clamp(0.0, 1.0) as f32;
                crate::window::lerp(a, b, w)
            }
            _ => *a,
        });
    }
    out
}

impl CustomMove {
    /// Apprend un mouvement à partir de ses prises, cadencées à
    /// [`SAMPLE_RATE_HZ`]. Chaque prise contient le geste fait une fois.
    pub fn train(label: &str, name: &str, takes: &[Vec<WindowSample>]) -> Result<Self, MoveError> {
        if takes.is_empty() {
            return Err(MoveError::NoTakes);
        }
        let examples = takes
            .iter()
            .enumerate()
            .map(|(i, take)| example_of(take, i + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let threshold = calibrate(&examples);
        Ok(Self {
            version: FORMAT_VERSION,
            label: label.to_string(),
            name: name.to_string(),
            sensitivity: 1.0,
            threshold,
            resemblances: Vec::new(),
            examples,
        })
    }

    /// Nombre de prises dont viennent les exemples.
    pub fn takes(&self) -> u32 {
        self.examples.len() as u32
    }

    fn well_formed(&self) -> bool {
        !self.examples.is_empty()
            && self
                .examples
                .iter()
                .all(|e| e.descriptor.len() == DESCRIPTOR && e.duration_ms > 0)
    }

    /// Confiance du mouvement pour un tronçon déjà décrit.
    fn confidence(&self, descriptor: &[f32]) -> f32 {
        let distance = self
            .examples
            .iter()
            .map(|e| euclidean(descriptor, &e.descriptor))
            .fold(f32::INFINITY, f32::min);
        confidence(distance, self.threshold * self.sensitivity)
    }

    /// Cherche les gestes qui partiraient sur ces mêmes prises : un geste de
    /// `base` reconnu en cours de route (le mouvement commence comme lui), ou
    /// un autre mouvement perso. `threshold` est le seuil de déclenchement.
    pub fn find_resemblances(
        &mut self,
        takes: &[Vec<WindowSample>],
        base: &mut dyn DynamicClassifier,
        others: &[CustomMove],
        threshold: f32,
    ) {
        let labels = base.labels().to_vec();
        let window = base.window_len();
        let mut hits = vec![0usize; labels.len()];
        let mut other_hits = vec![0usize; others.len()];
        let mut probs = vec![0.0f32; labels.len()];
        let mut measures = Measures::default();
        // Une base à une autre cadence ne peut pas rejouer ces prises.
        let replayable = (base.sample_rate_hz() - SAMPLE_RATE_HZ).abs() < 0.5;
        for take in takes {
            measures.fill(take);
            let Some((start, end)) = measures.segment(SAMPLE_RATE_HZ) else {
                continue;
            };
            let mut seen = vec![false; labels.len()];
            if replayable {
                for last in (start + 1..=end).filter(|last| last + 1 >= window) {
                    let slice = &take[last + 1 - window..=last];
                    if base.classify(slice, Handedness::Right, &mut probs).is_err() {
                        continue;
                    }
                    let top = crate::argmax(&probs);
                    if labels[top] != NONE_LABEL && probs[top] >= threshold {
                        seen[top] = true;
                    }
                }
            }
            for (hit, seen) in hits.iter_mut().zip(seen) {
                *hit += usize::from(seen);
            }
            let descriptor = measures.describe(start as f32, end as f32);
            for (hit, other) in other_hits.iter_mut().zip(others) {
                let alike = other.label != self.label
                    && other.well_formed()
                    && other.confidence(&descriptor) >= threshold;
                *hit += usize::from(alike);
            }
        }
        let total = takes.len().max(1) as f32;
        self.resemblances = labels
            .iter()
            .zip(hits)
            .chain(others.iter().map(|o| &o.label).zip(other_hits))
            .map(|(label, hits)| Resemblance {
                label: label.clone(),
                share: hits as f32 / total,
            })
            .filter(|r| r.share >= RESEMBLANCE_SHARE)
            .collect();
    }
}

/// Seuil d'acceptation : écart de chaque prise à sa plus proche voisine, avec
/// une marge, sans dépasser une part de l'amplitude du plus petit exemple.
fn calibrate(examples: &[MoveExample]) -> f32 {
    let smallest = examples
        .iter()
        .map(|e| radius(&e.descriptor))
        .fold(f32::INFINITY, f32::min);
    let ceiling = (MAX_THRESHOLD_SHARE * smallest).max(MIN_THRESHOLD);
    if examples.len() < 2 {
        // Rien ne mesure la variation d'une fois sur l'autre.
        return (SINGLE_TAKE_SHARE * smallest).clamp(MIN_THRESHOLD, ceiling);
    }
    let mut spread: Vec<f32> = examples
        .iter()
        .enumerate()
        .map(|(i, example)| {
            examples
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| euclidean(&example.descriptor, &other.descriptor))
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    spread.sort_unstable_by(f32::total_cmp);
    let p90 = spread[(spread.len() * 9 / 10).min(spread.len() - 1)];
    (p90 * THRESHOLD_MARGIN).clamp(MIN_THRESHOLD, ceiling)
}

/// Un mouvement prêt à être comparé en direct.
struct Template {
    threshold: f32,
    /// Durée en échantillons de la fenêtre, amplitude et descripteur de chaque exemple.
    examples: Vec<(f32, f32, Descriptor)>,
}

/// Classifieur dynamique de base complété par les mouvements personnalisés.
/// Ses labels sont ceux de la base, puis ceux des mouvements. La fenêtre
/// s'allonge pour contenir le plus long mouvement, sans retarder la base :
/// elle reçoit toujours ses `window_len()` derniers échantillons.
pub struct CustomDynamicClassifier {
    base: Box<dyn DynamicClassifier>,
    templates: Vec<Template>,
    labels: Vec<String>,
    base_len: usize,
    window: usize,
    measures: Measures,
    scores: Vec<f32>,
}

impl CustomDynamicClassifier {
    pub fn new(base: Box<dyn DynamicClassifier>, moves: &[CustomMove]) -> Self {
        let rate = base.sample_rate_hz();
        let mut labels = base.labels().to_vec();
        let base_len = labels.len();
        let mut templates = Vec::new();
        let mut longest = 0.0f32;
        for custom in moves {
            // Un label déjà pris ou des exemples illisibles : mouvement ignoré.
            if labels.contains(&custom.label) || !custom.well_formed() {
                continue;
            }
            let examples: Vec<(f32, f32, Descriptor)> = custom
                .examples
                .iter()
                .filter_map(|e| {
                    let descriptor = Descriptor::try_from(e.descriptor.as_slice()).ok()?;
                    let samples = e.duration_ms as f32 * 1e-3 * rate;
                    Some((samples, radius(&descriptor), descriptor))
                })
                .collect();
            longest = examples.iter().map(|e| e.0).fold(longest, f32::max);
            labels.push(custom.label.clone());
            templates.push(Template {
                threshold: custom.threshold * custom.sensitivity,
                examples,
            });
        }
        let fastest = SCALES.iter().copied().fold(1.0, f32::max);
        let window = base
            .window_len()
            .max((longest * fastest).ceil() as usize + 1);
        Self {
            scores: vec![0.0; templates.len()],
            base,
            templates,
            labels,
            base_len,
            window,
            measures: Measures::default(),
        }
    }

    /// Nombre de mouvements personnalisés pris en compte.
    pub fn custom_count(&self) -> usize {
        self.templates.len()
    }
}

impl DynamicClassifier for CustomDynamicClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn window_len(&self) -> usize {
        self.window
    }

    fn min_window_len(&self) -> usize {
        self.base.window_len()
    }

    fn sample_rate_hz(&self) -> f32 {
        self.base.sample_rate_hz()
    }

    fn classify(
        &mut self,
        window: &[WindowSample],
        handedness: Handedness,
        out: &mut [f32],
    ) -> Result<(), ClassifierError> {
        out.fill(0.0);
        let recent = window.len().saturating_sub(self.base.window_len());
        self.base
            .classify(&window[recent..], handedness, &mut out[..self.base_len])?;
        self.measures.fill(window);
        let last = self.measures.len().saturating_sub(1) as f32;
        for (score, template) in self.scores.iter_mut().zip(&self.templates) {
            let mut nearest = f32::INFINITY;
            for (samples, amplitude, example) in &template.examples {
                for scale in SCALES {
                    let from = last - samples * scale;
                    if from < 0.0 {
                        continue;
                    }
                    let descriptor = self.measures.describe(from, last);
                    if radius(&descriptor) >= RADIUS_GATE * amplitude {
                        nearest = nearest.min(euclidean(&descriptor, example));
                    }
                }
            }
            *score = if nearest.is_finite() {
                confidence(nearest, template.threshold)
            } else {
                0.0
            };
        }
        overlay(out, &self.scores, self.base_len..self.labels.len());
        Ok(())
    }

    fn description(&self) -> String {
        let base = self.base.description();
        match self.templates.len() {
            0 => base,
            1 => format!("{base} + 1 mouvement perso"),
            n => format!("{base} + {n} mouvements perso"),
        }
    }
}

fn move_path(dir: &Path, label: &str) -> PathBuf {
    dir.join(format!("{label}.json"))
}

/// Lit tous les mouvements de `dir`, triés par label. Un fichier illisible
/// donne un message et n'empêche pas de charger les autres.
pub fn load_dir(dir: &Path) -> (Vec<CustomMove>, Vec<String>) {
    let mut moves = Vec::new();
    let mut notices = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (moves, notices);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<CustomMove>(&text).map_err(|e| e.to_string()))
            .and_then(|custom| {
                if custom.version != FORMAT_VERSION {
                    Err(format!("version {} non prise en charge", custom.version))
                } else if !custom.well_formed() {
                    Err("exemples absents ou mal formés".into())
                } else {
                    Ok(custom)
                }
            });
        match parsed {
            Ok(custom) => moves.push(custom),
            Err(e) => notices.push(format!("mouvement perso {} ignoré : {e}", path.display())),
        }
    }
    (moves, notices)
}

/// Écrit le mouvement dans `dir/<label>.json`, par fichier temporaire puis renommage.
pub fn save(dir: &Path, custom: &CustomMove) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = move_path(dir, &custom.label);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(custom)?)?;
    std::fs::rename(&tmp, &path)
}

/// Retire le fichier du mouvement ; un mouvement déjà absent n'est pas une erreur.
pub fn remove(dir: &Path, label: &str) -> std::io::Result<()> {
    match std::fs::remove_file(move_path(dir, label)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Trajectoires synthétiques pour les tests de ce crate et de l'application.
pub mod synthetic {
    use std::f32::consts::TAU;

    use crate::builtin::synthetic::{Pose, hand};
    use crate::window::WindowSample;

    /// Taille de paume des mains synthétiques, en repère image isotrope.
    pub const PALM: f32 = 0.08;

    /// Décalage du centre de la main, en tailles de paume, à l'avancement `t`
    /// du geste (0 à 1).
    pub type Trajectory = fn(f32) -> [f32; 2];

    /// Cercle d'une paume et quart de rayon, parcouru une fois.
    pub fn circle(t: f32) -> [f32; 2] {
        let a = TAU * t;
        [1.25 * (a.cos() - 1.0), 1.25 * a.sin()]
    }

    /// Va-et-vient horizontal de deux paumes et demie.
    pub fn wave(t: f32) -> [f32; 2] {
        [2.5 * (1.0 - (2.0 * t - 1.0).abs()), 0.0]
    }

    /// Descente franche de trois paumes.
    pub fn drop(t: f32) -> [f32; 2] {
        [0.0, 3.0 * t]
    }

    /// Suite cadencée à 60 Hz : main immobile `before` secondes, geste de
    /// `seconds` secondes agrandi de `size`, puis main immobile `after`
    /// secondes. `wobble` déplace légèrement chaque image, comme un suivi réel.
    pub fn play(
        trajectory: Trajectory,
        pose: Pose,
        seconds: f32,
        size: f32,
        (before, after): (f32, f32),
        wobble: f32,
    ) -> Vec<WindowSample> {
        let rate = super::SAMPLE_RATE_HZ;
        let moving = (seconds * rate).round() as usize;
        let lead = (before * rate).round() as usize;
        let total = lead + moving + (after * rate).round() as usize;
        (0..total)
            .map(|i| {
                let t = (i.saturating_sub(lead) as f32 / moving.max(1) as f32).min(1.0);
                // Départ et arrêt progressifs, comme une vraie main.
                let eased = t * t * (3.0 - 2.0 * t);
                let [dx, dy] = trajectory(eased);
                let noise = wobble * (i as f32 * 1.7).sin();
                let center = [
                    0.5 + (dx * size + noise) * PALM,
                    0.5 + (dy * size - noise) * PALM,
                ];
                let h = hand(pose, center, PALM);
                WindowSample {
                    image: h.image,
                    world: h.world,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic::{Trajectory, circle, drop, play, wave};
    use super::*;
    use crate::builtin::BuiltinDynamicClassifier;
    use crate::builtin::synthetic::Pose;
    use crate::window::SlidingWindow;

    /// Trois prises du même geste, chacune un peu différente.
    fn takes(trajectory: Trajectory, pose: Pose) -> Vec<Vec<WindowSample>> {
        [(0.55, 0.92), (0.60, 1.0), (0.68, 1.1)]
            .into_iter()
            .map(|(seconds, size)| play(trajectory, pose, seconds, size, (0.5, 0.6), 0.03))
            .collect()
    }

    fn classifier(moves: &[CustomMove]) -> CustomDynamicClassifier {
        CustomDynamicClassifier::new(Box::new(BuiltinDynamicClassifier::new()), moves)
    }

    /// Rejoue une suite comme l'étage « gestes » : fenêtre glissante, une
    /// évaluation toutes les deux images. Renvoie, par label, la plus haute
    /// probabilité tenue sur deux évaluations de suite : ce qu'il faut au
    /// moteur pour déclencher.
    fn replay(classifier: &mut CustomDynamicClassifier, frames: &[WindowSample]) -> Vec<f32> {
        let mut window = SlidingWindow::new(classifier.window_len(), classifier.sample_rate_hz());
        let labels = classifier.labels().len();
        let mut held = vec![0.0f32; labels];
        let mut previous = vec![0.0f32; labels];
        let mut probs = vec![0.0f32; labels];
        let mut resampled = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            window.push(1_000_000 + i as u64 * 16_667, *frame);
            if i % 2 == 1 && window.resample(classifier.min_window_len(), &mut resampled) {
                classifier
                    .classify(&resampled, Handedness::Right, &mut probs)
                    .unwrap();
                assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{probs:?}");
                for k in 0..labels {
                    held[k] = held[k].max(probs[k].min(previous[k]));
                }
                previous.copy_from_slice(&probs);
            }
        }
        held
    }

    fn held(classifier: &mut CustomDynamicClassifier, frames: &[WindowSample], label: &str) -> f32 {
        let index = classifier.labels().iter().position(|l| l == label).unwrap();
        replay(classifier, frames)[index]
    }

    #[test]
    fn a_learnt_movement_is_recognised_at_other_speeds_and_sizes() {
        let circle_move =
            CustomMove::train("cercle", "Cercle", &takes(circle, Pose::FIST)).unwrap();
        assert_eq!(circle_move.takes(), 3);
        assert!(
            (MIN_THRESHOLD..=0.7).contains(&circle_move.threshold),
            "{}",
            circle_move.threshold
        );
        let mut c = classifier(std::slice::from_ref(&circle_move));
        assert_eq!(c.labels().last().unwrap(), "cercle");
        assert!(c.window_len() > c.min_window_len());
        assert_eq!(c.description(), "géométrique intégré + 1 mouvement perso");
        // Ni les durées ni les tailles des prises.
        for (seconds, size) in [(0.5, 1.05), (0.63, 0.95), (0.75, 1.0)] {
            let frames = play(circle, Pose::FIST, seconds, size, (0.7, 0.4), 0.03);
            let p = held(&mut c, &frames, "cercle");
            assert!(p >= 0.85, "{seconds} s × {size} : {p}");
        }
    }

    #[test]
    fn stillness_and_other_movements_are_not_the_learnt_one() {
        let circle_move =
            CustomMove::train("cercle", "Cercle", &takes(circle, Pose::FIST)).unwrap();
        // Au plus tolérant.
        let tolerant = CustomMove {
            sensitivity: SENSITIVITY_RANGE.1,
            ..circle_move
        };
        let mut c = classifier(&[tolerant]);
        let still = play(circle, Pose::FIST, 0.6, 0.0, (1.0, 1.0), 0.05);
        assert!(held(&mut c, &still, "cercle") < 0.5);
        for other in [wave as Trajectory, drop] {
            let frames = play(other, Pose::FIST, 0.6, 1.0, (0.7, 0.4), 0.03);
            let p = held(&mut c, &frames, "cercle");
            assert!(p < 0.5, "{p}");
        }
        // Le même cercle, main ouverte : la forme de la main compte.
        let open = play(circle, Pose::OPEN_PALM, 0.6, 1.0, (0.7, 0.4), 0.03);
        assert!(held(&mut c, &open, "cercle") < 0.8);
    }

    #[test]
    fn two_movements_and_the_base_gestures_live_side_by_side() {
        let moves = [
            CustomMove::train("cercle", "Cercle", &takes(circle, Pose::FIST)).unwrap(),
            CustomMove::train("coucou", "Coucou", &takes(wave, Pose::FIST)).unwrap(),
        ];
        let mut c = classifier(&moves);
        let frames = play(wave, Pose::FIST, 0.6, 1.0, (0.7, 0.4), 0.03);
        let probs = replay(&mut c, &frames);
        let at = |label: &str| probs[c.labels().iter().position(|l| l == label).unwrap()];
        assert!(at("coucou") >= 0.85, "{probs:?}");
        assert!(at("cercle") < 0.5, "{probs:?}");
        // Un balayage main ouverte reste un balayage de la base.
        let swipe = play(drop, Pose::OPEN_PALM, 0.3, 1.0, (0.7, 0.3), 0.0);
        let probs = replay(&mut c, &swipe);
        let at = |label: &str| probs[c.labels().iter().position(|l| l == label).unwrap()];
        assert!(at("swipe_down") >= 0.8, "{probs:?}");
        assert!(at("cercle") < 0.5 && at("coucou") < 0.5, "{probs:?}");
    }

    #[test]
    fn takes_without_a_clear_movement_are_refused() {
        let still = play(circle, Pose::FIST, 0.6, 0.0, (0.5, 0.5), 0.05);
        assert_eq!(check_take(&still), Err(MoveError::NoMovement { take: 1 }));
        let tiny = play(circle, Pose::FIST, 0.5, 0.25, (0.5, 0.5), 0.0);
        assert!(matches!(
            check_take(&tiny),
            Err(MoveError::NoMovement { .. } | MoveError::TooSmall { .. })
        ));
        let slow = play(circle, Pose::FIST, 2.2, 2.0, (0.3, 0.3), 0.0);
        assert_eq!(check_take(&slow), Err(MoveError::TooLong { take: 1 }));
        assert_eq!(
            CustomMove::train("x", "X", &[]).unwrap_err(),
            MoveError::NoTakes
        );
        // La deuxième prise est en cause, et le dit.
        let good = play(circle, Pose::FIST, 0.6, 1.0, (0.5, 0.5), 0.03);
        assert_eq!(
            CustomMove::train("x", "X", &[good.clone(), still]).unwrap_err(),
            MoveError::NoMovement { take: 2 }
        );
        assert!(check_take(&good).is_ok());
    }

    #[test]
    fn a_movement_that_starts_like_a_swipe_says_so() {
        // Descente main ouverte : c'est le balayage vers le bas de la base.
        let down = takes(drop, Pose::OPEN_PALM);
        let mut custom = CustomMove::train("chute", "Chute", &down).unwrap();
        let circle_move =
            CustomMove::train("cercle", "Cercle", &takes(circle, Pose::FIST)).unwrap();
        let mut base = BuiltinDynamicClassifier::new();
        custom.find_resemblances(&down, &mut base, std::slice::from_ref(&circle_move), 0.8);
        let labels: Vec<&str> = custom
            .resemblances
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["swipe_down"]);
        // Un cercle poing fermé ne ressemble à rien de connu.
        let rounds = takes(circle, Pose::FIST);
        let mut other = CustomMove::train("rond", "Rond", &rounds).unwrap();
        other.find_resemblances(&rounds, &mut base, &[], 0.8);
        assert!(other.resemblances.is_empty(), "{:?}", other.resemblances);
        // Le même cercle sous un autre nom : les deux mouvements se confondent.
        other.find_resemblances(&rounds, &mut base, &[circle_move], 0.8);
        assert_eq!(other.resemblances[0].label, "cercle");
    }

    #[test]
    fn a_learnt_movement_fires_its_binding_through_the_engine() {
        use crate::engine::{EngineEvent, EngineState};
        use crate::stage::{GestureStage, HandInput};

        const W: f32 = 1280.0;
        let text = format!(
            "{}
[[bindings]]
gesture = \"cercle\"
action = {{ type = \"keys\", combo = \"Ctrl+Alt+C\" }}
",
            mudra_config::DEFAULT_CONFIG_TOML
        );
        let config = mudra_config::parse(&text, Path::new("essai.toml")).unwrap();
        let custom =
            CustomMove::train("cercle", "Cercle", &takes(circle, Pose::OPEN_PALM)).unwrap();
        let mut stage = GestureStage::new(
            &config,
            Box::new(crate::builtin::BuiltinStaticClassifier::new()),
            Box::new(classifier(&[custom])),
        );
        let mut now = 1_000_000;
        let mut feed = |stage: &mut GestureStage, frames: &[WindowSample]| -> Vec<String> {
            let mut events = Vec::new();
            for frame in frames {
                now += 16_667;
                // Les points de la prise sont en repère isotrope : en pixels ici.
                let mut image = frame.image;
                for p in image.iter_mut() {
                    *p = [p[0] * W, p[1] * W, p[2] * W];
                }
                let input = HandInput {
                    track: 3,
                    handedness: Handedness::Right,
                    image,
                    world: frame.world,
                };
                stage.process(now, W, 720.0, &[input], &mut events).unwrap();
            }
            events
                .iter()
                .filter_map(|e| match e {
                    EngineEvent::GestureFired { binding, .. } => {
                        Some(config.bindings[*binding].gesture.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        // Paume ouverte tenue une seconde : Mudra s'arme ; puis le cercle.
        let frames = play(circle, Pose::OPEN_PALM, 0.6, 1.0, (1.0, 0.5), 0.02);
        assert_eq!(feed(&mut stage, &frames), vec!["cercle"]);
        assert_eq!(stage.engine().state(), EngineState::Armed);
        // Poing immobile ensuite : rien d'autre ne part.
        let frames = play(wave, Pose::FIST, 0.6, 0.0, (0.2, 0.2), 0.02);
        assert!(feed(&mut stage, &frames).is_empty());
    }

    #[test]
    fn irregular_takes_are_brought_to_a_steady_rate() {
        // Prise à 30 images par seconde : x avance d'une unité par seconde.
        let frames: Vec<(u64, WindowSample)> = (0..31u64)
            .map(|i| {
                let t = i * 33_333;
                let sample = WindowSample {
                    image: [[t as f32 * 1e-6, 0.0, 0.0]; 21],
                    world: [[0.0; 3]; 21],
                };
                (5_000_000 + t, sample)
            })
            .collect();
        let steady = uniform(&frames);
        assert_eq!(steady.len(), 60);
        for (i, sample) in steady.iter().enumerate() {
            let expected = i as f32 / SAMPLE_RATE_HZ;
            assert!(
                (sample.image[0][0] - expected).abs() < 1e-3,
                "échantillon {i}"
            );
        }
        assert!(uniform(&[]).is_empty());
    }

    #[test]
    fn movements_survive_a_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("mudra-mouvements-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let custom = CustomMove::train("cercle", "Cercle", &takes(circle, Pose::FIST)).unwrap();
        save(&dir, &custom).unwrap();
        std::fs::write(dir.join("casse.json"), "{").unwrap();
        let (moves, notices) = load_dir(&dir);
        assert_eq!(moves, vec![custom]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        remove(&dir, "cercle").unwrap();
        remove(&dir, "cercle").unwrap();
        assert!(load_dir(&dir).0.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
