//! Poses personnalisées apprises par l'exemple.
//!
//! Une pose perso est un ensemble d'exemples de la forme de la main. En
//! direct, la main est comparée aux exemples les plus proches ; le seuil
//! d'acceptation est calibré sur la dispersion des prises entre elles. Ces
//! poses s'ajoutent au classifieur de base (intégré ou ONNX) au lieu de le
//! remplacer : aucun ré-entraînement, aucune dépendance Python.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::{
    INDEX_MCP, MIDDLE_MCP, PINKY_MCP, RING_MCP, THUMB_MCP, THUMB_TIP, V3, WRIST, dist, dot, norm,
    palm_size, sub,
};
use crate::{ClassifierError, HandSample, Handedness, Landmarks, NONE_LABEL, StaticClassifier};

/// Version du format de fichier et des caractéristiques.
pub const FORMAT_VERSION: u32 = 1;
/// Distances entre points de la main, flexions des articulations, puis
/// orientation de la main.
pub const FEATURES: usize = 55;
pub type Features = [f32; FEATURES];

/// Poids de l'orientation face à la forme : une main retournée (pouce levé
/// contre pouce baissé) pèse autant qu'un doigt qui change.
const ORIENTATION_WEIGHT: f32 = 0.7;
/// Poids des flexions, déjà largement portées par les distances.
const BEND_WEIGHT: f32 = 0.6;
/// Poids de la hauteur des bouts de doigts au-dessus de la paume : la
/// profondeur est la mesure la plus bruitée du suivi.
const DEPTH_WEIGHT: f32 = 0.5;
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

fn euclidean(a: &Features, b: &Features) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Moyenne des distances de `f` à ses plus proches exemples.
fn knn_distance<'a>(f: &Features, examples: impl Iterator<Item = &'a Features>) -> f32 {
    let mut best = [f32::INFINITY; NEIGHBOURS];
    for example in examples {
        let mut d = euclidean(f, example);
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
    pub examples: Vec<Vec<f32>>,
}

impl CustomPose {
    /// Apprend une pose à partir de ses prises. Chaque prise est une suite
    /// d'images de la main tenant la pose.
    pub fn train(label: &str, name: &str, takes: &[Vec<HandSample>]) -> Result<Self, TrainError> {
        if takes.is_empty() {
            return Err(TrainError::NoTakes);
        }
        let mut per_take: Vec<Vec<Features>> = Vec::with_capacity(takes.len());
        let mut readings: Vec<Vec<[Features; 2]>> = Vec::with_capacity(takes.len());
        for (i, take) in takes.iter().enumerate() {
            if take.len() < MIN_TAKE_FRAMES {
                return Err(TrainError::TakeTooShort {
                    take: i + 1,
                    frames: take.len(),
                });
            }
            let kept = take.len().min(MAX_EXAMPLES_PER_TAKE);
            let kept: Vec<&HandSample> = (0..kept).map(|k| &take[k * take.len() / kept]).collect();
            per_take.push(
                kept.iter()
                    .map(|s| features(&s.world, s.handedness))
                    .collect(),
            );
            readings.push(kept.iter().map(|s| both_readings(&s.world)).collect());
        }
        Ok(Self {
            version: FORMAT_VERSION,
            label: label.to_string(),
            name: name.to_string(),
            sensitivity: 1.0,
            threshold: calibrate(&per_take, &readings),
            takes: takes.len() as u32,
            resemblances: Vec::new(),
            examples: per_take.into_iter().flatten().map(|f| f.to_vec()).collect(),
        })
    }

    fn matcher(&self) -> Option<Matcher> {
        let examples: Vec<Features> = self
            .examples
            .iter()
            .filter_map(|e| Features::try_from(e.as_slice()).ok())
            .collect();
        (!examples.is_empty() && examples.len() == self.examples.len()).then(|| Matcher {
            threshold: self.threshold
                * self
                    .sensitivity
                    .clamp(SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.1),
            examples,
        })
    }

    /// Confiance de la pose pour une main, entre 0 et 1.
    pub fn confidence(&self, hand: &HandSample) -> f32 {
        self.matcher().map_or(0.0, |m| m.confidence(&hand.world))
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
            let Some(matcher) = other.matcher() else {
                continue;
            };
            let hits = samples
                .iter()
                .filter(|s| matcher.confidence(&s.world) >= enter)
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
}

/// Seuil d'acceptation : dispersion des prises entre elles, avec une marge.
/// Avec une seule prise, rien ne mesure la variation d'une fois sur l'autre :
/// le plancher s'applique.
fn calibrate(takes: &[Vec<Features>], readings: &[Vec<[Features; 2]>]) -> f32 {
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
            for [right, left] in take {
                spread.push(knn_distance(right, others()).min(knn_distance(left, others())));
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

struct Matcher {
    threshold: f32,
    examples: Vec<Features>,
}

impl Matcher {
    fn confidence(&self, world: &Landmarks) -> f32 {
        let [right, left] = both_readings(world);
        let distance = knn_distance(&right, self.examples.iter())
            .min(knn_distance(&left, self.examples.iter()));
        confidence(distance, self.threshold)
    }
}

/// Classifieur de base complété par les poses personnalisées. Ses labels sont
/// ceux de la base suivis de ceux des poses.
pub struct CustomStaticClassifier {
    base: Box<dyn StaticClassifier>,
    matchers: Vec<Matcher>,
    labels: Vec<String>,
    base_len: usize,
    scores: Vec<f32>,
}

impl CustomStaticClassifier {
    /// Les poses illisibles ou dont le label existe déjà sont ignorées.
    pub fn new(base: Box<dyn StaticClassifier>, poses: &[CustomPose]) -> Self {
        let mut labels = base.labels().to_vec();
        let base_len = labels.len();
        let mut matchers = Vec::new();
        for pose in poses {
            if labels.contains(&pose.label) {
                continue;
            }
            if let Some(matcher) = pose.matcher() {
                labels.push(pose.label.clone());
                matchers.push(matcher);
            }
        }
        Self {
            base,
            scores: vec![0.0; matchers.len()],
            matchers,
            labels,
            base_len,
        }
    }
}

impl StaticClassifier for CustomStaticClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn classify(&mut self, hand: &HandSample, out: &mut [f32]) -> Result<(), ClassifierError> {
        let (base_out, custom_out) = out.split_at_mut(self.base_len);
        self.base.classify(hand, base_out)?;
        if self.matchers.is_empty() {
            return Ok(());
        }
        let mut best = 0.0f32;
        let mut sharp_sum = 0.0;
        for (score, matcher) in self.scores.iter_mut().zip(&self.matchers) {
            *score = matcher.confidence(&hand.world);
            best = best.max(*score);
            sharp_sum += score.powi(4);
        }
        // La meilleure pose perso prend sa confiance sur la distribution de
        // base ; les autres poses perso se partagent cette même masse.
        for p in base_out.iter_mut() {
            *p *= 1.0 - best;
        }
        for (p, score) in custom_out.iter_mut().zip(&self.scores) {
            *p = if sharp_sum > 0.0 {
                best * score.powi(4) / sharp_sum
            } else {
                0.0
            };
        }
        Ok(())
    }

    fn description(&self) -> String {
        match self.matchers.len() {
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
                } else if pose.matcher().is_none() {
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

    #[test]
    fn a_pose_named_like_a_base_gesture_is_ignored() {
        let pose = CustomPose::train("fist", "Faux poing", &takes(PHONE, 12)).unwrap();
        let classifier = classifier(&[pose]);
        assert_eq!(classifier.labels(), BuiltinStaticClassifier::new().labels());
    }
}
