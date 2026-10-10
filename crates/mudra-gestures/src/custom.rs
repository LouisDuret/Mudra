//! Poses personnalisées apprises par l'exemple.
//!
//! Une pose perso est un ensemble d'exemples de la forme de la main. En
//! direct, la main est comparée aux exemples les plus proches ; le seuil
//! d'acceptation est calibré sur la dispersion des prises entre elles. Ces
//! poses s'ajoutent au classifieur de base (intégré ou ONNX) au lieu de le
//! remplacer : aucun ré-entraînement, aucune dépendance Python.
//!
//! Une pose à deux mains est apprise de la même façon, sur la forme de chaque
//! main et sur leur disposition l'une par rapport à l'autre. Elle n'existe
//! que lorsque deux mains sont visibles.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::{
    INDEX_MCP, MIDDLE_MCP, PINKY_MCP, RING_MCP, THUMB_MCP, THUMB_TIP, V3, WRIST, dist, dist2d, dot,
    hand_center, norm, palm_size, palm_size_2d, sub,
};
use crate::{ClassifierError, HandSample, Handedness, Landmarks, NONE_LABEL, StaticClassifier};

/// Version du format de fichier et des caractéristiques.
pub const FORMAT_VERSION: u32 = 1;
/// Distances entre points de la main, flexions des articulations, puis
/// orientation de la main.
pub const FEATURES: usize = 55;
pub type Features = [f32; FEATURES];
/// Disposition de deux mains : vecteur d'une paume à l'autre, puis écart
/// entre les bouts des doigts homologues.
const RELATION: usize = 7;
/// Main de gauche à l'écran, main de droite, puis leur disposition.
pub const PAIR_FEATURES: usize = 2 * FEATURES + RELATION;
pub type PairFeatures = [f32; PAIR_FEATURES];

/// Poids de l'orientation face à la forme : une main retournée (pouce levé
/// contre pouce baissé) pèse autant qu'un doigt qui change.
const ORIENTATION_WEIGHT: f32 = 0.7;
/// Poids des flexions, déjà largement portées par les distances.
const BEND_WEIGHT: f32 = 0.6;
/// Poids de la hauteur des bouts de doigts au-dessus de la paume : la
/// profondeur est la mesure la plus bruitée du suivi.
const DEPTH_WEIGHT: f32 = 0.5;
/// Poids du vecteur d'une paume à l'autre, et des écarts entre bouts de doigts.
const POSITION_WEIGHT: f32 = 0.7;
const CONTACT_WEIGHT: f32 = 0.5;
/// Écart, en tailles de paume, autour duquel la disposition sature : deux
/// mains qui se touchent ou non diffèrent plus que deux mains éloignées de
/// quatre ou cinq paumes.
const RELATION_REACH: f32 = 3.0;
/// Longueur de la paume rapportée à sa largeur (MCP de l'index à l'auriculaire).
const PALM_ASPECT: f32 = 1.5;
/// La distance à une pose est la moyenne des distances aux plus proches exemples.
const NEIGHBOURS: usize = 3;
const MAX_EXAMPLES_PER_TAKE: usize = 60;
/// Une prise plus courte n'a pas vu la main assez longtemps.
pub const MIN_TAKE_FRAMES: usize = 15;
/// Bornes du seuil calibré. Le plancher absorbe le bruit du suivi ; le
/// plafond reste sous l'écart créé par un doigt qui change (1,4 et plus sur
/// les poses intégrées, contre 0,6 de dispersion sur de vraies prises).
const MIN_THRESHOLD: f32 = 0.4;
const MAX_THRESHOLD: f32 = 0.85;
/// Marge appliquée à la dispersion mesurée entre les prises.
const THRESHOLD_MARGIN: f32 = 1.3;
/// Bornes du réglage de sensibilité proposé à l'utilisateur.
pub const SENSITIVITY_RANGE: (f32, f32) = (0.6, 1.6);
/// Part des images à partir de laquelle une ressemblance est signalée.
const RESEMBLANCE_SHARE: f32 = 0.3;

fn scaled(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn unit(a: V3) -> V3 {
    scaled(a, 1.0 / norm(a).max(1e-6))
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Bout et base de chaque doigt, pouce compris.
const TIPS: [usize; 5] = [THUMB_TIP, 8, 12, 16, 20];
const BASES: [usize; 5] = [THUMB_MCP, INDEX_MCP, MIDDLE_MCP, RING_MCP, PINKY_MCP];

/// Flexion en `b` de la chaîne a-b-c : 0 tendue, 1 repliée sur elle-même.
fn bend(a: V3, b: V3, c: V3) -> f32 {
    let (u, v) = (sub(a, b), sub(c, b));
    (1.0 + dot(u, v) / (norm(u) * norm(v)).max(1e-9)) / 2.0
}

/// Caractéristiques d'une main. La forme est décrite par des distances entre
/// points et des flexions, en tailles de paume : contrairement à des
/// coordonnées, elles ne dépendent pas d'un repère estimé sur quelques points
/// bruités. L'orientation face à la caméra vient ensuite. Une main gauche est
/// ramenée à une main droite par symétrie.
pub fn features(world: &Landmarks, handedness: Handedness) -> Features {
    mirrored_features(world, handedness == Handedness::Left)
}

/// Les deux lectures possibles d'une main, droite puis gauche. Le suivi se
/// trompe parfois de main sur une pose fermée, et une pose apprise d'une main
/// doit valoir pour l'autre : la comparaison garde la lecture la plus proche.
fn both_readings(world: &Landmarks) -> [Features; 2] {
    [
        mirrored_features(world, false),
        mirrored_features(world, true),
    ]
}

fn mirrored_features(world: &Landmarks, mirrored: bool) -> Features {
    let flip = if mirrored { -1.0 } else { 1.0 };
    let point = |i: usize| [world[i][0] * flip, world[i][1], world[i][2]];
    let wrist = point(WRIST);
    let up = unit(sub(point(MIDDLE_MCP), wrist));
    let across = sub(point(INDEX_MCP), point(PINKY_MCP));
    let across = unit(sub(across, scaled(up, dot(across, up))));
    let normal = cross(up, across);
    let scale = 1.0 / palm_size(world).max(1e-6);
    let mut centre = [0.0; 3];
    for i in [WRIST, INDEX_MCP, MIDDLE_MCP, RING_MCP, PINKY_MCP] {
        let p = point(i);
        for c in 0..3 {
            centre[c] += p[c] / 5.0;
        }
    }

    let mut out = [0.0; FEATURES];
    let mut n = 0;
    let mut push = |value: f32| {
        out[n] = value;
        n += 1;
    };
    for (tip, base) in TIPS.into_iter().zip(BASES) {
        push(dist(point(tip), wrist) * scale);
        push(dist(point(tip), point(base)) * scale);
        push(dist(point(tip), centre) * scale);
    }
    for (i, a) in TIPS.iter().enumerate() {
        for b in &TIPS[i + 1..] {
            push(dist(point(*a), point(*b)) * scale);
        }
    }
    for base in &BASES[1..] {
        push(dist(point(THUMB_TIP), point(*base)) * scale);
    }
    for finger in 0..5 {
        let first = 1 + 4 * finger;
        let chain = [
            wrist,
            point(first),
            point(first + 1),
            point(first + 2),
            point(first + 3),
        ];
        for joint in 1..4 {
            push(bend(chain[joint - 1], chain[joint], chain[joint + 1]) * BEND_WEIGHT);
        }
    }
    for tip in TIPS {
        push(dot(sub(point(tip), centre), normal) * scale * DEPTH_WEIGHT);
    }
    for value in up.into_iter().chain(normal) {
        push(value * ORIENTATION_WEIGHT);
    }
    debug_assert_eq!(n, FEATURES);
    out
}

/// Taille de la main dans l'image : la longueur de la paume, ou sa largeur
/// quand la main pointe vers la caméra et que la longueur s'écrase.
fn image_scale(image: &Landmarks) -> f32 {
    palm_size_2d(image).max(PALM_ASPECT * dist2d(image[INDEX_MCP], image[PINKY_MCP]))
}

fn saturated(d: f32) -> f32 {
    d * RELATION_REACH / (RELATION_REACH + d)
}

/// Caractéristiques de deux mains, `left` étant celle de gauche à l'écran.
/// Chaque main est lue selon son rang et non selon la latéralité annoncée,
/// que le suivi inverse parfois. La disposition se mesure dans l'image, en
/// tailles de paume : les points monde de chaque main ont leur propre origine.
fn pair_features(left: &HandSample, right: &HandSample) -> PairFeatures {
    let mut out = [0.0; PAIR_FEATURES];
    out[..FEATURES].copy_from_slice(&mirrored_features(&left.world, true));
    out[FEATURES..2 * FEATURES].copy_from_slice(&mirrored_features(&right.world, false));
    let scale = 2.0 / (image_scale(&left.image) + image_scale(&right.image)).max(1e-6);
    let (from, to) = (hand_center(&left.image), hand_center(&right.image));
    let offset = [(to[0] - from[0]) * scale, (to[1] - from[1]) * scale];
    let reach = offset[0].hypot(offset[1]).max(1e-6);
    let relation = &mut out[2 * FEATURES..];
    for (slot, value) in relation.iter_mut().zip(offset) {
        *slot = value / reach * saturated(reach) * POSITION_WEIGHT;
    }
    for (slot, tip) in relation[2..].iter_mut().zip(TIPS) {
        *slot = saturated(dist2d(left.image[tip], right.image[tip]) * scale) * CONTACT_WEIGHT;
    }
    out
}

/// Les deux lectures d'une paire : mains rangées de gauche à droite à
/// l'écran, puis dans l'ordre inverse. Deux mains l'une au-dessus de l'autre
/// changent de rang d'une image à l'autre : la comparaison garde la lecture
/// la plus proche.
fn pair_readings(hands: [&HandSample; 2]) -> [PairFeatures; 2] {
    let [a, b] = hands;
    let (left, right) = if hand_center(&a.image)[0] <= hand_center(&b.image)[0] {
        (a, b)
    } else {
        (b, a)
    };
    [pair_features(left, right), pair_features(right, left)]
}

fn euclidean(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Caractéristiques comparables entre elles : celles d'une main, ou de deux.
trait Shape: Copy + AsRef<[f32]> + for<'a> TryFrom<&'a [f32]> {
    fn distance(&self, other: &Self) -> f32;
}

impl Shape for Features {
    fn distance(&self, other: &Self) -> f32 {
        euclidean(self, other)
    }
}

impl Shape for PairFeatures {
    /// Écart de la partie qui s'éloigne le plus, entre chaque main et leur
    /// disposition : chaque main doit ressembler autant que pour une pose à
    /// une main, sans que le bruit des deux s'additionne.
    fn distance(&self, other: &Self) -> f32 {
        let part = |from: usize, to: usize| euclidean(&self[from..to], &other[from..to]);
        part(0, FEATURES)
            .max(part(FEATURES, 2 * FEATURES))
            .max(part(2 * FEATURES, PAIR_FEATURES))
    }
}

/// Moyenne des distances de `f` à ses plus proches exemples.
fn knn_distance<'a, F: Shape + 'a>(f: &F, examples: impl Iterator<Item = &'a F>) -> f32 {
    let mut best = [f32::INFINITY; NEIGHBOURS];
    for example in examples {
        let mut d = f.distance(example);
        for slot in &mut best {
            if d < *slot {
                std::mem::swap(&mut d, slot);
            }
        }
    }
    let found: Vec<f32> = best.into_iter().filter(|d| d.is_finite()).collect();
    if found.is_empty() {
        f32::INFINITY
    } else {
        found.iter().sum::<f32>() / found.len() as f32
    }
}

/// Confiance d'une pose à la distance `distance` : 0,9 au seuil, puis chute
/// rapide. La zone entre les seuils d'entrée et de sortie du moteur donne
/// l'hystérésis.
fn confidence(distance: f32, threshold: f32) -> f32 {
    let r = distance / threshold.max(1e-6);
    1.0 / (1.0 + 0.11 * r.powi(6))
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TrainError {
    #[error("aucune prise enregistrée")]
    NoTakes,
    #[error("prise {take} : la main n'a été vue que sur {frames} images, recommence-la")]
    TakeTooShort { take: usize, frames: usize },
    #[error(
        "prise {take} : les deux mains n'ont été vues ensemble que sur {frames} images, recommence-la"
    )]
    PairTakeTooShort { take: usize, frames: usize },
}

/// Ressemblance mesurée à l'apprentissage avec un autre geste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resemblance {
    pub label: String,
    /// Part des images de la pose que l'autre geste reconnaît aussi.
    pub share: f32,
}

/// Une pose personnalisée, telle qu'elle est enregistrée sur le disque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomPose {
    pub version: u32,
    /// Identifiant snake_case : label du classifieur et nom dans la configuration.
    pub label: String,
    /// Nom affiché.
    pub name: String,
    /// Multiplicateur du seuil : moins de 1 est plus strict, plus de 1 plus tolérant.
    pub sensitivity: f32,
    /// Distance d'acceptation calibrée sur les exemples.
    pub threshold: f32,
    /// Nombre de prises dont viennent les exemples.
    pub takes: u32,
    #[serde(default)]
    pub resemblances: Vec<Resemblance>,
    /// Pose qui ne se fait qu'à deux mains : ses exemples décrivent les deux.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub two_hands: bool,
    pub examples: Vec<Vec<f32>>,
}

impl CustomPose {
    /// Apprend une pose à partir de ses prises. Chaque prise est une suite
    /// d'images de la main tenant la pose.
    pub fn train(label: &str, name: &str, takes: &[Vec<HandSample>]) -> Result<Self, TrainError> {
        Self::learn(label, name, false, takes, |s| {
            (features(&s.world, s.handedness), both_readings(&s.world))
        })
    }

    /// Apprend une pose à deux mains. Chaque prise est une suite d'images où
    /// les deux mains sont visibles.
    pub fn train_pair(
        label: &str,
        name: &str,
        takes: &[Vec<[HandSample; 2]>],
    ) -> Result<Self, TrainError> {
        Self::learn(label, name, true, takes, |[a, b]| {
            let readings = pair_readings([a, b]);
            (readings[0], readings)
        })
    }

    /// `read` donne, pour une image, l'exemple à garder et ses lectures possibles.
    fn learn<S, F: Shape>(
        label: &str,
        name: &str,
        two_hands: bool,
        takes: &[Vec<S>],
        read: impl Fn(&S) -> (F, [F; 2]),
    ) -> Result<Self, TrainError> {
        if takes.is_empty() {
            return Err(TrainError::NoTakes);
        }
        let mut per_take: Vec<Vec<F>> = Vec::with_capacity(takes.len());
        let mut readings: Vec<Vec<[F; 2]>> = Vec::with_capacity(takes.len());
        for (i, take) in takes.iter().enumerate() {
            if take.len() < MIN_TAKE_FRAMES {
                let (take, frames) = (i + 1, take.len());
                return Err(if two_hands {
                    TrainError::PairTakeTooShort { take, frames }
                } else {
                    TrainError::TakeTooShort { take, frames }
                });
            }
            let kept = take.len().min(MAX_EXAMPLES_PER_TAKE);
            let (examples, read): (Vec<F>, Vec<[F; 2]>) = (0..kept)
                .map(|k| read(&take[k * take.len() / kept]))
                .unzip();
            per_take.push(examples);
            readings.push(read);
        }
        Ok(Self {
            version: FORMAT_VERSION,
            label: label.to_string(),
            name: name.to_string(),
            sensitivity: 1.0,
            threshold: calibrate(&per_take, &readings),
            takes: takes.len() as u32,
            resemblances: Vec::new(),
            two_hands,
            examples: per_take
                .into_iter()
                .flatten()
                .map(|f| f.as_ref().to_vec())
                .collect(),
        })
    }

    fn matcher<F: Shape>(&self) -> Option<Matcher<F>> {
        let examples: Vec<F> = self
            .examples
            .iter()
            .filter_map(|e| F::try_from(e.as_slice()).ok())
            .collect();
        (!examples.is_empty() && examples.len() == self.examples.len()).then(|| Matcher {
            threshold: self.threshold
                * self
                    .sensitivity
                    .clamp(SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.1),
            examples,
        })
    }

    fn single(&self) -> Option<Matcher<Features>> {
        (!self.two_hands).then(|| self.matcher()).flatten()
    }

    fn pair(&self) -> Option<Matcher<PairFeatures>> {
        self.two_hands.then(|| self.matcher()).flatten()
    }

    /// Confiance de la pose pour une main, entre 0 et 1. Toujours nulle pour
    /// une pose à deux mains.
    pub fn confidence(&self, hand: &HandSample) -> f32 {
        self.single()
            .map_or(0.0, |m| m.confidence(&both_readings(&hand.world)))
    }

    /// Confiance d'une pose à deux mains pour ces deux mains.
    pub fn pair_confidence(&self, hands: [&HandSample; 2]) -> f32 {
        self.pair()
            .map_or(0.0, |m| m.confidence(&pair_readings(hands)))
    }

    /// Mesure les ressemblances de la pose avec les gestes du classifieur de
    /// base et avec les autres poses perso, sur les images des prises.
    pub fn find_resemblances(
        &mut self,
        samples: &[HandSample],
        base: &mut dyn StaticClassifier,
        others: &[CustomPose],
        enter: f32,
    ) {
        self.resemblances.clear();
        if samples.is_empty() {
            return;
        }
        let total = samples.len() as f32;
        let labels = base.labels().to_vec();
        let mut probs = vec![0.0; labels.len()];
        let mut counts = vec![0u32; labels.len()];
        for sample in samples {
            if base.classify(sample, &mut probs).is_ok() {
                let top = crate::argmax(&probs);
                if labels[top] != NONE_LABEL && probs[top] >= enter {
                    counts[top] += 1;
                }
            }
        }
        for (label, count) in labels.iter().zip(counts) {
            let share = count as f32 / total;
            if share >= RESEMBLANCE_SHARE {
                self.resemblances.push(Resemblance {
                    label: label.clone(),
                    share,
                });
            }
        }
        for other in others.iter().filter(|o| o.label != self.label) {
            let Some(matcher) = other.single() else {
                continue;
            };
            let hits = samples
                .iter()
                .filter(|s| matcher.confidence(&both_readings(&s.world)) >= enter)
                .count();
            let share = hits as f32 / total;
            if share >= RESEMBLANCE_SHARE {
                self.resemblances.push(Resemblance {
                    label: other.label.clone(),
                    share,
                });
            }
        }
    }

    /// Mesure les ressemblances d'une pose à deux mains avec les autres poses
    /// à deux mains, sur les images des prises.
    pub fn find_pair_resemblances(
        &mut self,
        samples: &[[HandSample; 2]],
        others: &[CustomPose],
        enter: f32,
    ) {
        self.resemblances.clear();
        if samples.is_empty() {
            return;
        }
        for other in others.iter().filter(|o| o.label != self.label) {
            let Some(matcher) = other.pair() else {
                continue;
            };
            let hits = samples
                .iter()
                .filter(|[a, b]| matcher.confidence(&pair_readings([a, b])) >= enter)
                .count();
            let share = hits as f32 / samples.len() as f32;
            if share >= RESEMBLANCE_SHARE {
                self.resemblances.push(Resemblance {
                    label: other.label.clone(),
                    share,
                });
            }
        }
    }
}

/// Seuil d'acceptation : dispersion des prises entre elles, avec une marge.
/// Avec une seule prise, rien ne mesure la variation d'une fois sur l'autre :
/// le plancher s'applique.
fn calibrate<F: Shape>(takes: &[Vec<F>], readings: &[Vec<[F; 2]>]) -> f32 {
    let mut spread: Vec<f32> = Vec::new();
    if takes.len() >= 2 {
        for (i, take) in readings.iter().enumerate() {
            let others = || {
                takes
                    .iter()
                    .enumerate()
                    .filter(move |(j, _)| *j != i)
                    .flat_map(|(_, t)| t.iter())
            };
            for [first, second] in take {
                spread.push(knn_distance(first, others()).min(knn_distance(second, others())));
            }
        }
    }
    spread.retain(|d| d.is_finite());
    if spread.is_empty() {
        return MIN_THRESHOLD;
    }
    spread.sort_unstable_by(f32::total_cmp);
    let p90 = spread[(spread.len() * 9 / 10).min(spread.len() - 1)];
    (p90 * THRESHOLD_MARGIN).clamp(MIN_THRESHOLD, MAX_THRESHOLD)
}

struct Matcher<F> {
    threshold: f32,
    examples: Vec<F>,
}

impl<F: Shape> Matcher<F> {
    /// Confiance de la lecture la plus proche des exemples.
    fn confidence(&self, readings: &[F; 2]) -> f32 {
        let distance = knn_distance(&readings[0], self.examples.iter())
            .min(knn_distance(&readings[1], self.examples.iter()));
        confidence(distance, self.threshold)
    }
}

/// La meilleure pose de `scores` prend sa confiance sur `out[..from]` ; les
/// autres poses du groupe se partagent cette même masse, à partir de `from`.
fn overlay(out: &mut [f32], from: usize, scores: &[f32]) {
    let best = scores.iter().copied().fold(0.0, f32::max);
    let sharp_sum: f32 = scores.iter().map(|s| s.powi(4)).sum();
    let (rest, group) = out.split_at_mut(from);
    for p in rest {
        *p *= 1.0 - best;
    }
    for (p, score) in group.iter_mut().zip(scores) {
        *p = if sharp_sum > 0.0 {
            best * score.powi(4) / sharp_sum
        } else {
            0.0
        };
    }
}

/// Classifieur de base complété par les poses personnalisées. Ses labels sont
/// ceux de la base, puis ceux des poses à une main, puis ceux des poses à
/// deux mains.
pub struct CustomStaticClassifier {
    base: Box<dyn StaticClassifier>,
    matchers: Vec<Matcher<Features>>,
    pairs: Vec<Matcher<PairFeatures>>,
    labels: Vec<String>,
    base_len: usize,
    /// Indice du premier label à deux mains.
    pairs_from: usize,
    scores: Vec<f32>,
    pair_scores: Vec<f32>,
}

impl CustomStaticClassifier {
    /// Les poses illisibles ou dont le label existe déjà sont ignorées.
    pub fn new(base: Box<dyn StaticClassifier>, poses: &[CustomPose]) -> Self {
        let mut labels = base.labels().to_vec();
        let base_len = labels.len();
        let mut matchers = Vec::new();
        let mut pairs = Vec::new();
        let mut pair_labels: Vec<String> = Vec::new();
        for pose in poses {
            if labels.contains(&pose.label) || pair_labels.contains(&pose.label) {
                continue;
            }
            if let Some(matcher) = pose.single() {
                labels.push(pose.label.clone());
                matchers.push(matcher);
            } else if let Some(matcher) = pose.pair() {
                pair_labels.push(pose.label.clone());
                pairs.push(matcher);
            }
        }
        let pairs_from = labels.len();
        labels.extend(pair_labels);
        Self {
            base,
            scores: vec![0.0; matchers.len()],
            pair_scores: vec![0.0; pairs.len()],
            matchers,
            pairs,
            labels,
            base_len,
            pairs_from,
        }
    }
}

impl StaticClassifier for CustomStaticClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn two_hand_labels(&self) -> &[String] {
        &self.labels[self.pairs_from..]
    }

    fn classify(&mut self, hand: &HandSample, out: &mut [f32]) -> Result<(), ClassifierError> {
        let (base_out, custom_out) = out.split_at_mut(self.base_len);
        self.base.classify(hand, base_out)?;
        // Une main seule ne tient aucune pose à deux mains.
        custom_out.fill(0.0);
        if self.matchers.is_empty() {
            return Ok(());
        }
        let readings = both_readings(&hand.world);
        for (score, matcher) in self.scores.iter_mut().zip(&self.matchers) {
            *score = matcher.confidence(&readings);
        }
        overlay(out, self.base_len, &self.scores);
        Ok(())
    }

    fn classify_pair(
        &mut self,
        hands: [&HandSample; 2],
        out: [&mut [f32]; 2],
    ) -> Result<(), ClassifierError> {
        if self.pairs.is_empty() {
            return Ok(());
        }
        let readings = pair_readings(hands);
        for (score, matcher) in self.pair_scores.iter_mut().zip(&self.pairs) {
            *score = matcher.confidence(&readings);
        }
        // La pose à deux mains l'emporte sur ce que chaque main montre seule.
        for out in out {
            overlay(out, self.pairs_from, &self.pair_scores);
        }
        Ok(())
    }

    fn description(&self) -> String {
        match self.matchers.len() + self.pairs.len() {
            0 => self.base.description(),
            1 => format!("{} + 1 pose perso", self.base.description()),
            n => format!("{} + {n} poses perso", self.base.description()),
        }
    }
}

fn pose_path(dir: &Path, label: &str) -> PathBuf {
    dir.join(format!("{label}.json"))
}

/// Lit toutes les poses de `dir`, triées par label. Un fichier illisible
/// donne un message et n'empêche pas de charger les autres.
pub fn load_dir(dir: &Path) -> (Vec<CustomPose>, Vec<String>) {
    let mut poses = Vec::new();
    let mut notices = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (poses, notices);
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
            .and_then(|text| serde_json::from_str::<CustomPose>(&text).map_err(|e| e.to_string()))
            .and_then(|pose| {
                if pose.version != FORMAT_VERSION {
                    Err(format!("version {} non prise en charge", pose.version))
                } else if pose.single().is_none() && pose.pair().is_none() {
                    Err("exemples absents ou mal formés".into())
                } else {
                    Ok(pose)
                }
            });
        match parsed {
            Ok(pose) => poses.push(pose),
            Err(e) => notices.push(format!("pose perso {} ignorée : {e}", path.display())),
        }
    }
    (poses, notices)
}

/// Écrit la pose dans `dir/<label>.json`, par fichier temporaire puis renommage.
pub fn save(dir: &Path, pose: &CustomPose) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = pose_path(dir, &pose.label);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(pose)?)?;
    std::fs::rename(&tmp, &path)
}

/// Retire le fichier de la pose ; une pose déjà absente n'est pas une erreur.
pub fn remove(dir: &Path, label: &str) -> std::io::Result<()> {
    match std::fs::remove_file(pose_path(dir, label)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::argmax;
    use crate::builtin::BuiltinStaticClassifier;
    use crate::builtin::synthetic::{Pose, hand};

    /// « Téléphone » : pouce et auriculaire tendus, absent des poses intégrées.
    const PHONE: Pose = Pose {
        fingers: [1.0, 1.0, 1.0, 0.0],
        thumb: 1.0,
        pinch: false,
        roll_deg: 0.0,
    };

    struct Noise(u32);

    impl Noise {
        /// Valeur pseudo-aléatoire dans [-1, 1].
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        }
    }

    /// Une image de la pose, telle qu'une vraie main la tiendrait : doigts
    /// pas tout à fait identiques, main un peu inclinée, bruit du suivi.
    fn noisy(pose: Pose, roll: f32, noise: &mut Noise) -> HandSample {
        let mut pose = pose;
        for f in &mut pose.fingers {
            *f = (*f + 0.06 * noise.next()).clamp(0.0, 1.0);
        }
        pose.thumb = (pose.thumb + 0.06 * noise.next()).clamp(0.0, 1.0);
        pose.roll_deg = roll + 3.0 * noise.next();
        let mut sample = hand(pose, [0.5, 0.5], 0.1);
        for p in sample.world.iter_mut() {
            for c in p.iter_mut() {
                *c += 0.0015 * noise.next();
            }
        }
        sample
    }

    /// Trois prises, la main inclinée différemment à chaque fois.
    fn takes(pose: Pose, seed: u32) -> Vec<Vec<HandSample>> {
        let mut noise = Noise(seed);
        [-8.0, 0.0, 9.0]
            .into_iter()
            .map(|roll| (0..80).map(|_| noisy(pose, roll, &mut noise)).collect())
            .collect()
    }

    fn classifier(poses: &[CustomPose]) -> CustomStaticClassifier {
        CustomStaticClassifier::new(Box::new(BuiltinStaticClassifier::new()), poses)
    }

    fn top(classifier: &mut CustomStaticClassifier, sample: &HandSample) -> (String, f32) {
        let mut probs = vec![0.0; classifier.labels().len()];
        classifier.classify(sample, &mut probs).unwrap();
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{probs:?}");
        let i = argmax(&probs);
        (classifier.labels()[i].clone(), probs[i])
    }

    #[test]
    fn features_ignore_position_size_and_handedness() {
        let a = hand(PHONE, [0.2, 0.3], 0.05);
        let mut b = hand(PHONE, [0.7, 0.6], 0.2);
        for p in b.world.iter_mut() {
            *p = [p[0] * 1.7 + 0.3, p[1] * 1.7 - 0.1, p[2] * 1.7 + 0.05];
        }
        let fa = features(&a.world, a.handedness);
        assert!(euclidean(&fa, &features(&b.world, b.handedness)) < 1e-3);
        // La main gauche est l'image miroir de la droite.
        let mut left = a;
        for p in left.world.iter_mut() {
            p[0] = -p[0];
        }
        assert!(euclidean(&fa, &features(&left.world, Handedness::Left)) < 1e-3);
    }

    #[test]
    fn learned_pose_is_recognised_and_builtin_poses_are_kept() {
        let pose = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 1)).unwrap();
        assert!((MIN_THRESHOLD..=MAX_THRESHOLD).contains(&pose.threshold));
        let mut classifier = classifier(std::slice::from_ref(&pose));
        assert_eq!(classifier.labels().last().unwrap(), "telephone");

        // Nouvelles images de la pose, jamais vues à l'apprentissage.
        let mut noise = Noise(99);
        for roll in [-10.0, -4.0, 3.0, 11.0] {
            let (label, p) = top(&mut classifier, &noisy(PHONE, roll, &mut noise));
            assert_eq!(label, "telephone", "inclinaison {roll}");
            assert!(p >= 0.85, "inclinaison {roll} : {p}");
        }

        // Les poses intégrées gardent leur classe.
        let mut plain = BuiltinStaticClassifier::new();
        for builtin in [
            Pose::OPEN_PALM,
            Pose::FIST,
            Pose::THUMBS_UP,
            Pose::V_SIGN,
            Pose::ROCK,
            Pose::THREE,
            Pose::POINTING,
            Pose::PINCH,
            Pose::RELAXED,
        ] {
            let sample = hand(builtin, [0.5, 0.5], 0.1);
            let mut expected = vec![0.0; plain.labels().len()];
            plain.classify(&sample, &mut expected).unwrap();
            let (label, _) = top(&mut classifier, &sample);
            assert_eq!(label, plain.labels()[argmax(&expected)], "{builtin:?}");
            assert!(pose.confidence(&sample) < 0.3, "{builtin:?}");
        }
    }

    #[test]
    fn orientation_is_part_of_the_pose() {
        let pose = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 2)).unwrap();
        let upside_down = hand(
            Pose {
                roll_deg: 180.0,
                ..PHONE
            },
            [0.5, 0.5],
            0.1,
        );
        assert!(pose.confidence(&upside_down) < 0.3);
    }

    #[test]
    fn sensitivity_widens_or_narrows_the_pose() {
        let mut pose = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 3)).unwrap();
        // Auriculaire à moitié replié : à la limite de la pose.
        let sloppy = hand(
            Pose {
                fingers: [1.0, 1.0, 1.0, 0.35],
                ..PHONE
            },
            [0.5, 0.5],
            0.1,
        );
        pose.sensitivity = SENSITIVITY_RANGE.0;
        let strict = pose.confidence(&sloppy);
        pose.sensitivity = SENSITIVITY_RANGE.1;
        let tolerant = pose.confidence(&sloppy);
        assert!(tolerant > strict + 0.2, "{strict} puis {tolerant}");
    }

    #[test]
    fn short_take_is_refused() {
        let mut all = takes(PHONE, 4);
        all[1].truncate(5);
        assert_eq!(
            CustomPose::train("telephone", "Téléphone", &all),
            Err(TrainError::TakeTooShort { take: 2, frames: 5 })
        );
        assert_eq!(
            CustomPose::train("telephone", "Téléphone", &[]),
            Err(TrainError::NoTakes)
        );
    }

    #[test]
    fn resemblance_with_a_builtin_pose_is_reported() {
        let all = takes(Pose::V_SIGN, 5);
        let samples: Vec<HandSample> = all.iter().flatten().copied().collect();
        let mut copy = CustomPose::train("mon_v", "Mon V", &all).unwrap();
        let mut base = BuiltinStaticClassifier::new();
        copy.find_resemblances(&samples, &mut base, &[], 0.85);
        assert_eq!(copy.resemblances.len(), 1, "{:?}", copy.resemblances);
        assert_eq!(copy.resemblances[0].label, "v_sign");

        // Une pose distincte ne ressemble à rien, sauf à sa propre copie.
        let all = takes(PHONE, 6);
        let samples: Vec<HandSample> = all.iter().flatten().copied().collect();
        let mut phone = CustomPose::train("telephone", "Téléphone", &all).unwrap();
        phone.find_resemblances(&samples, &mut base, std::slice::from_ref(&copy), 0.85);
        assert!(phone.resemblances.is_empty(), "{:?}", phone.resemblances);
        let twin = CustomPose::train("allo", "Allô", &takes(PHONE, 7)).unwrap();
        phone.find_resemblances(&samples, &mut base, &[copy, twin], 0.85);
        assert_eq!(phone.resemblances.len(), 1);
        assert_eq!(phone.resemblances[0].label, "allo");
    }

    #[test]
    fn two_custom_poses_do_not_steal_each_other() {
        let phone = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 8)).unwrap();
        let four = Pose {
            fingers: [0.0; 4],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        let four_pose = CustomPose::train("quatre", "Quatre", &takes(four, 9)).unwrap();
        let mut classifier = classifier(&[phone, four_pose]);
        let mut noise = Noise(10);
        assert_eq!(
            top(&mut classifier, &noisy(PHONE, 2.0, &mut noise)).0,
            "telephone"
        );
        assert_eq!(
            top(&mut classifier, &noisy(four, -3.0, &mut noise)).0,
            "quatre"
        );
    }

    #[test]
    fn poses_survive_a_round_trip_and_bad_files_are_reported() {
        let dir = std::env::temp_dir().join(format!("mudra-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pose = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 11)).unwrap();
        save(&dir, &pose).unwrap();
        std::fs::write(dir.join("abime.json"), "{ pas du json").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignoré").unwrap();
        let (poses, notices) = load_dir(&dir);
        assert_eq!(poses, vec![pose]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("abime.json"));

        remove(&dir, "telephone").unwrap();
        remove(&dir, "telephone").unwrap();
        assert!(load_dir(&dir).0.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Déplace une main synthétique (poignet en 0,5 ; 0,5) et, pour une main
    /// gauche, la retourne.
    fn placed(mut sample: HandSample, wrist: [f32; 2], left: bool) -> HandSample {
        for (image, world) in sample.image.iter_mut().zip(sample.world.iter_mut()) {
            if left {
                image[0] = 1.0 - image[0];
                world[0] = -world[0];
            }
            image[0] += wrist[0] - 0.5;
            image[1] += wrist[1] - 0.5;
        }
        if left {
            sample.handedness = Handedness::Left;
        }
        sample
    }

    /// Une image d'une pose à deux mains : `left` à gauche de l'écran, `right`
    /// décalée de `gap` tailles de paume.
    fn noisy_pair(
        left: Pose,
        right: Pose,
        gap: [f32; 2],
        roll: f32,
        noise: &mut Noise,
    ) -> [HandSample; 2] {
        let at = [0.4 + 0.004 * noise.next(), 0.5 + 0.004 * noise.next()];
        let other = [at[0] + 0.1 * gap[0], at[1] + 0.1 * gap[1]];
        [
            placed(noisy(left, -roll, noise), at, true),
            placed(noisy(right, roll, noise), other, false),
        ]
    }

    /// Trois prises : mains inclinées et écartées différemment à chaque fois.
    fn pair_takes(left: Pose, right: Pose, gap: [f32; 2], seed: u32) -> Vec<Vec<[HandSample; 2]>> {
        let mut noise = Noise(seed);
        [(-8.0, -0.15), (0.0, 0.0), (9.0, 0.15)]
            .into_iter()
            .map(|(roll, wider)| {
                let gap = [gap[0] + wider, gap[1]];
                (0..80)
                    .map(|_| noisy_pair(left, right, gap, roll, &mut noise))
                    .collect()
            })
            .collect()
    }

    /// Classe deux mains vues ensemble, comme le fait l'étage de gestes.
    fn pair_top(
        classifier: &mut CustomStaticClassifier,
        hands: [&HandSample; 2],
    ) -> [(String, f32); 2] {
        let n = classifier.labels().len();
        let (mut a, mut b) = (vec![0.0; n], vec![0.0; n]);
        classifier.classify(hands[0], &mut a).unwrap();
        classifier.classify(hands[1], &mut b).unwrap();
        classifier.classify_pair(hands, [&mut a, &mut b]).unwrap();
        [a, b].map(|probs| {
            assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{probs:?}");
            let i = argmax(&probs);
            (classifier.labels()[i].clone(), probs[i])
        })
    }

    const SIDE_BY_SIDE: [f32; 2] = [2.5, 0.0];

    #[test]
    fn two_hand_pose_is_recognised_only_with_both_hands() {
        let takes = pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 20);
        let pose = CustomPose::train_pair("duo", "Duo", &takes).unwrap();
        assert!(pose.two_hands);
        assert!((MIN_THRESHOLD..=MAX_THRESHOLD).contains(&pose.threshold));
        let phone =
            CustomPose::train("telephone", "Téléphone", &super::tests::takes(PHONE, 21)).unwrap();
        // Les poses à deux mains viennent après celles à une main.
        let mut classifier = classifier(&[pose.clone(), phone]);
        assert_eq!(classifier.labels().last().unwrap(), "duo");
        assert_eq!(classifier.two_hand_labels(), ["duo".to_string()]);

        let mut noise = Noise(22);
        for roll in [-10.0, -3.0, 4.0, 11.0] {
            let [left, right] = noisy_pair(Pose::FIST, PHONE, [2.6, 0.1], roll, &mut noise);
            for (label, p) in pair_top(&mut classifier, [&left, &right]) {
                assert_eq!(label, "duo", "inclinaison {roll}");
                assert!(p >= 0.85, "inclinaison {roll} : {p}");
            }
            // Ni l'ordre des mains ni la latéralité annoncée ne comptent.
            let (mut wrong_left, mut wrong_right) = (left, right);
            wrong_left.handedness = Handedness::Right;
            wrong_right.handedness = Handedness::Left;
            for (label, p) in pair_top(&mut classifier, [&wrong_right, &wrong_left]) {
                assert_eq!(label, "duo", "inclinaison {roll}");
                assert!(p >= 0.85, "inclinaison {roll} : {p}");
            }

            // Une main seule garde sa lecture à une main.
            assert_eq!(top(&mut classifier, &right).0, "telephone");
            assert_eq!(top(&mut classifier, &left).0, "fist");
            assert_eq!(pose.confidence(&right), 0.0);
        }
    }

    #[test]
    fn two_hand_pose_depends_on_each_hand_and_on_their_layout() {
        let takes = pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 23);
        let pose = CustomPose::train_pair("duo", "Duo", &takes).unwrap();
        let mut noise = Noise(24);
        let mut confidence = |left: Pose, right: Pose, gap: [f32; 2]| {
            let [a, b] = noisy_pair(left, right, gap, 0.0, &mut noise);
            pose.pair_confidence([&a, &b])
        };
        assert!(confidence(Pose::FIST, PHONE, SIDE_BY_SIDE) >= 0.85);
        // Une main change de forme.
        assert!(confidence(Pose::FIST, Pose::OPEN_PALM, SIDE_BY_SIDE) < 0.3);
        assert!(confidence(Pose::V_SIGN, PHONE, SIDE_BY_SIDE) < 0.3);
        // Les deux formes échangent leur côté : c'est une autre pose.
        assert!(confidence(PHONE, Pose::FIST, SIDE_BY_SIDE) < 0.3);
        // Mêmes formes, autre disposition.
        assert!(confidence(Pose::FIST, PHONE, [0.4, 2.5]) < 0.3);
        assert!(confidence(Pose::FIST, PHONE, [6.0, 0.0]) < 0.5);
    }

    #[test]
    fn stacked_hands_are_recognised_whichever_is_leftmost() {
        // La main de droite est au-dessus de l'autre, à peine décalée.
        let above = |dx: f32| [dx, -2.2];
        let mut noise = Noise(25);
        let takes: Vec<Vec<[HandSample; 2]>> = [-6.0, 0.0, 7.0]
            .into_iter()
            .map(|roll| {
                (0..80)
                    .map(|_| noisy_pair(Pose::OPEN_PALM, Pose::FIST, above(0.15), roll, &mut noise))
                    .collect()
            })
            .collect();
        let pose = CustomPose::train_pair("etage", "Étage", &takes).unwrap();
        for dx in [0.15, 0.0, -0.15] {
            let [a, b] = noisy_pair(Pose::OPEN_PALM, Pose::FIST, above(dx), 2.0, &mut noise);
            assert!(pose.pair_confidence([&a, &b]) >= 0.85, "décalage {dx}");
        }
        // Le poing en dessous : autre pose.
        let [a, b] = noisy_pair(Pose::OPEN_PALM, Pose::FIST, [0.0, 2.2], 2.0, &mut noise);
        assert!(pose.pair_confidence([&a, &b]) < 0.3);
    }

    #[test]
    fn two_hand_poses_do_not_steal_each_other_and_twins_are_reported() {
        let duo_takes = pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 26);
        let mut duo = CustomPose::train_pair("duo", "Duo", &duo_takes).unwrap();
        let palms = CustomPose::train_pair(
            "paumes",
            "Paumes",
            &pair_takes(PHONE, PHONE, SIDE_BY_SIDE, 27),
        )
        .unwrap();
        let twin = CustomPose::train_pair(
            "jumeau",
            "Jumeau",
            &pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 28),
        )
        .unwrap();
        let phone = CustomPose::train("telephone", "Téléphone", &takes(PHONE, 29)).unwrap();

        let mut classifier = classifier(&[duo.clone(), palms.clone()]);
        let mut noise = Noise(30);
        let [a, b] = noisy_pair(Pose::FIST, PHONE, SIDE_BY_SIDE, 3.0, &mut noise);
        assert_eq!(pair_top(&mut classifier, [&a, &b])[0].0, "duo");
        let [a, b] = noisy_pair(PHONE, PHONE, SIDE_BY_SIDE, -3.0, &mut noise);
        assert_eq!(pair_top(&mut classifier, [&a, &b])[1].0, "paumes");

        // Seule une autre pose à deux mains peut ressembler à celle-ci.
        let samples: Vec<[HandSample; 2]> = duo_takes.into_iter().flatten().collect();
        duo.find_pair_resemblances(&samples, &[palms, phone, twin], 0.85);
        assert_eq!(duo.resemblances.len(), 1, "{:?}", duo.resemblances);
        assert_eq!(duo.resemblances[0].label, "jumeau");
    }

    #[test]
    fn short_two_hand_take_is_refused_and_files_keep_the_number_of_hands() {
        let mut all = pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 31);
        let pose = CustomPose::train_pair("duo", "Duo", &all).unwrap();
        all[2].truncate(7);
        assert_eq!(
            CustomPose::train_pair("duo", "Duo", &all),
            Err(TrainError::PairTakeTooShort { take: 3, frames: 7 })
        );

        let dir = std::env::temp_dir().join(format!("mudra-custom-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, &pose).unwrap();
        // Exemples d'une main annoncés comme ceux de deux : illisible.
        let mut broken = CustomPose::train("faux", "Faux", &takes(PHONE, 32)).unwrap();
        broken.two_hands = true;
        save(&dir, &broken).unwrap();
        let (poses, notices) = load_dir(&dir);
        assert_eq!(poses, vec![pose]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("faux.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_hand_pose_fires_its_two_hand_binding_through_the_engine() {
        use crate::builtin::BuiltinDynamicClassifier;
        use crate::engine::EngineEvent;
        use crate::stage::{GestureStage, HandInput};

        const W: f32 = 1280.0;
        let pose = CustomPose::train_pair(
            "duo",
            "Duo",
            &pair_takes(Pose::FIST, PHONE, SIDE_BY_SIDE, 33),
        )
        .unwrap();
        let mut config = mudra_config::parse(
            mudra_config::DEFAULT_CONFIG_TOML,
            std::path::Path::new("default.toml"),
        )
        .unwrap();
        config.activation.mode = mudra_config::ActivationMode::AlwaysOn;
        let mut both = config.bindings[0].clone();
        both.gesture = "duo".into();
        both.hand = mudra_config::HandFilter::Both;
        both.hold_ms = Some(300);
        both.repeat_ms = None;
        // Une action à une main sur une pose à deux mains est ignorée.
        let mut single = both.clone();
        single.hand = mudra_config::HandFilter::Any;
        config.bindings = vec![single, both];
        config.continuous.clear();
        let mut stage = GestureStage::new(
            &config,
            Box::new(classifier(&[pose])),
            Box::new(BuiltinDynamicClassifier::new()),
        );

        let mut noise = Noise(34);
        let mut now = 1_000_000;
        let mut show = |stage: &mut GestureStage, hands: usize, ms: u64| {
            let mut fired = Vec::new();
            for _ in 0..(ms * 1000).div_ceil(16_667) {
                now += 16_667;
                let pair = noisy_pair(Pose::FIST, PHONE, SIDE_BY_SIDE, 1.0, &mut noise);
                let inputs: Vec<HandInput> = pair
                    .iter()
                    .take(hands)
                    .enumerate()
                    .map(|(i, h)| HandInput {
                        track: 1 + i as u32,
                        handedness: h.handedness,
                        // Repère isotrope → pixels.
                        image: h.image.map(|p| p.map(|c| c * W)),
                        world: h.world,
                    })
                    .collect();
                let mut events = Vec::new();
                stage.process(now, W, 720.0, &inputs, &mut events).unwrap();
                fired.extend(events.into_iter().filter_map(|e| match e {
                    EngineEvent::GestureFired { binding, .. } => Some(binding),
                    _ => None,
                }));
            }
            fired
        };
        // Une main seule ne déclenche rien ; les deux, une fois, l'action à deux mains.
        assert!(show(&mut stage, 1, 800).is_empty());
        assert_eq!(show(&mut stage, 2, 1000), vec![1]);
        let label = stage.static_labels().len() - 1;
        for view in stage.hands() {
            assert_eq!(argmax(&view.static_probs), label);
        }
    }

    #[test]
    fn a_pose_named_like_a_base_gesture_is_ignored() {
        let pose = CustomPose::train("fist", "Faux poing", &takes(PHONE, 12)).unwrap();
        let classifier = classifier(&[pose]);
        assert_eq!(classifier.labels(), BuiltinStaticClassifier::new().labels());
    }
}
