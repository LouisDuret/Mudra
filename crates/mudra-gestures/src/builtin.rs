//! Classifieurs géométriques intégrés.
//!
//! Ils rendent le prototype utilisable avant tout entraînement : tant que
//! `models/static_gestures.onnx` et `models/dynamic_gestures.onnx` n'existent
//! pas (phase 3 du cahier des charges, données de l'utilisateur), Mudra classe
//! les poses de l'Annexe B par des règles sur les points-clés. Ils exposent la
//! même interface que les modèles ONNX, classe `none` comprise.

use crate::geometry::*;
use crate::window::WindowSample;
use crate::{
    ClassifierError, DynamicClassifier, HandSample, Handedness, Landmarks, StaticClassifier,
};

const STATIC_LABELS: [&str; 10] = [
    "none",
    "open_palm",
    "fist",
    "thumbs_up",
    "thumbs_down",
    "v_sign",
    "rock",
    "three_fingers",
    "pinch",
    "pointing",
];

const DYNAMIC_LABELS: [&str; 5] = [
    "none",
    "swipe_left",
    "swipe_right",
    "swipe_up",
    "swipe_down",
];

/// État de flexion des doigts, chaque valeur dans [0, 1].
///
/// Les seuils sont calés sur des points-clés réels (tests/fixtures/hands) :
/// un doigt tendu a une rectitude ≥ 0,9 ; replié face à la caméra, ≤ 0,6 ;
/// un poing vu de profil (doigts masqués) reste autour de 0,7 à 0,8.
#[derive(Debug, Clone, Copy)]
pub struct FingerState {
    /// Index, majeur, annulaire, auriculaire nettement tendus.
    pub extended: [f32; 4],
    /// Nettement repliés.
    pub down: [f32; 4],
    /// Pas tendus : repliés ou masqués (critère tolérant, pour les pouces levés vus de profil).
    pub folded: [f32; 4],
    /// Pouce nettement écarté de la paume.
    pub thumb_out: f32,
    /// Pouce au moins légèrement dégagé (tolérant, pour la paume ouverte).
    pub thumb_free: f32,
    /// Pouce et index joints.
    pub pinch_closed: f32,
    /// L'index n'est pas replié dans la paume.
    pub index_away: f32,
}

/// Rectitude d'un doigt long : distance poignet → bout rapportée à la longueur
/// de la chaîne poignet → MCP → PIP → DIP → bout. 1 = doigt tendu dans l'axe.
pub fn finger_straightness(world: &Landmarks, finger: usize) -> f32 {
    let [m, p, d, t] = FINGERS[finger];
    let chain = dist(world[WRIST], world[m])
        + dist(world[m], world[p])
        + dist(world[p], world[d])
        + dist(world[d], world[t]);
    dist(world[WRIST], world[t]) / chain.max(1e-6)
}

/// Flexion totale d'un doigt long (MCP + PIP + DIP), en degrés.
pub fn finger_total_bend_deg(world: &Landmarks, finger: usize) -> f32 {
    bend_deg(
        world[WRIST],
        world[FINGERS[finger][0]],
        world[FINGERS[finger][1]],
    ) + finger_curl_deg(world, finger)
}

impl FingerState {
    pub fn from_world(world: &Landmarks) -> Self {
        let palm = palm_size(world).max(1e-6);
        let mut extended = [0.0; 4];
        let mut down = [0.0; 4];
        let mut folded = [0.0; 4];
        for f in 0..4 {
            let straightness = finger_straightness(world, f);
            let bend = finger_total_bend_deg(world, f);
            extended[f] = smoothstep(0.80, 0.90, straightness) * smoothstep(115.0, 95.0, bend);
            down[f] = smoothstep(0.84, 0.72, straightness);
            folded[f] = smoothstep(0.88, 0.80, straightness);
        }
        // Dégagement du pouce : distance de son bout à l'articulation la plus
        // proche des doigts longs. Dans un poing, le pouce repose sur les
        // doigts ; pouce levé ou paume ouverte, il s'en écarte nettement.
        let thumb_clearance = FINGERS
            .iter()
            .flat_map(|f| f[..3].iter())
            .map(|&j| dist(world[THUMB_TIP], world[j]))
            .fold(f32::MAX, f32::min)
            / palm;
        let thumb_bend = bend_deg(world[THUMB_CMC], world[THUMB_MCP], world[THUMB_IP])
            + bend_deg(world[THUMB_MCP], world[THUMB_IP], world[THUMB_TIP]);
        let thumb_straight = smoothstep(95.0, 55.0, thumb_bend);
        Self {
            extended,
            down,
            folded,
            thumb_out: smoothstep(0.32, 0.50, thumb_clearance) * thumb_straight,
            thumb_free: smoothstep(0.20, 0.34, thumb_clearance),
            pinch_closed: smoothstep(0.38, 0.22, pinch_ratio(world)),
            index_away: smoothstep(0.85, 1.05, dist(world[INDEX_TIP], world[WRIST]) / palm),
        }
    }

    /// Mesures brutes derrière l'état des doigts, pour le diagnostic (`mudra image --verbose`).
    pub fn describe(world: &Landmarks) -> String {
        let palm = palm_size(world).max(1e-6);
        let s = Self::from_world(world);
        let fingers: Vec<String> = (0..4)
            .map(|f| {
                format!(
                    "{:.2}/{:.0}°",
                    finger_straightness(world, f),
                    finger_total_bend_deg(world, f)
                )
            })
            .collect();
        let clearance = FINGERS
            .iter()
            .flat_map(|f| f[..3].iter())
            .map(|&j| dist(world[THUMB_TIP], world[j]))
            .fold(f32::MAX, f32::min)
            / palm;
        let thumb_bend = bend_deg(world[THUMB_CMC], world[THUMB_MCP], world[THUMB_IP])
            + bend_deg(world[THUMB_MCP], world[THUMB_IP], world[THUMB_TIP]);
        format!(
            "doigts (rectitude/flexion) [{}] · tendus {:.2?} · repliés {:.2?} · pouce : dégagement {clearance:.2}, flexion {thumb_bend:.0}°, écarté {:.2}, libre {:.2} · pincement {:.2} (fermé {:.2}) · index éloigné {:.2} · écart index-majeur {:.2}",
            fingers.join(", "),
            s.extended,
            s.down,
            s.thumb_out,
            s.thumb_free,
            pinch_ratio(world),
            s.pinch_closed,
            s.index_away,
            dist(world[INDEX_TIP], world[MIDDLE_TIP]) / palm,
        )
    }

    /// Nombre (flou) de doigts longs tendus.
    pub fn extended_count(&self) -> f32 {
        self.extended.iter().sum()
    }
}

/// Classifieur de poses par règles géométriques.
pub struct BuiltinStaticClassifier {
    labels: Vec<String>,
}

impl Default for BuiltinStaticClassifier {
    fn default() -> Self {
        Self {
            labels: STATIC_LABELS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl BuiltinStaticClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scores bruts par pose (indice aligné sur les labels, `none` à 0).
    pub fn scores(hand: &HandSample) -> [f32; 10] {
        let s = FingerState::from_world(&hand.world);
        let [ei, em, er, ep] = s.extended;
        let [_, dm, dr, dp] = s.down;
        let all_down: f32 = s.down.iter().product();
        // Poing : doigts serrés, plus strict que « replié » pour ne pas
        // confondre une main détendue, doigts à demi fléchis, avec un poing.
        let all_tight: f32 = (0..4)
            .map(|f| smoothstep(0.72, 0.62, finger_straightness(&hand.world, f)))
            .product();
        let all_folded: f32 = s.folded.iter().product();

        // Direction du pouce dans le plan image : y vers le bas.
        let tx = hand.image[THUMB_TIP][0] - hand.image[THUMB_MCP][0];
        let ty = hand.image[THUMB_TIP][1] - hand.image[THUMB_MCP][1];
        let tn = (tx * tx + ty * ty).sqrt().max(1e-9);
        let thumb_visible = smoothstep(0.25, 0.40, tn / palm_size_2d(&hand.image).max(1e-9));
        let thumb_up = smoothstep(0.55, 0.85, -ty / tn) * thumb_visible;
        let thumb_down = smoothstep(0.55, 0.85, ty / tn) * thumb_visible;

        let palm = palm_size(&hand.world).max(1e-6);
        let v_spread = smoothstep(
            0.22,
            0.38,
            dist(hand.world[INDEX_TIP], hand.world[MIDDLE_TIP]) / palm,
        );
        let not_pinched = 1.0 - s.pinch_closed;

        let mut out = [0.0; 10];
        out[1] = ei * em * er * ep * s.thumb_free * not_pinched; // open_palm
        out[2] = all_tight * (1.0 - s.thumb_out); // fist
        // Pouce levé ou baissé : le pouce décide ; les autres doigts doivent
        // seulement ne pas être tendus (ils sont souvent masqués, main de profil).
        out[3] = all_folded * s.thumb_out * thumb_up; // thumbs_up
        out[4] = all_folded * s.thumb_out * thumb_down; // thumbs_down
        out[5] = ei * em * dr * dp * v_spread; // v_sign
        out[6] = ei * dm * dr * ep; // rock
        out[7] = ei * em * er * dp; // three_fingers
        out[8] = s.pinch_closed * s.index_away * (1.0 - all_down); // pinch
        out[9] = ei * dm * dr * dp * not_pinched; // pointing
        out
    }
}

impl StaticClassifier for BuiltinStaticClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn classify(&mut self, hand: &HandSample, out: &mut [f32]) -> Result<(), ClassifierError> {
        let scores = Self::scores(hand);
        normalize(&scores, out);
        Ok(())
    }

    fn description(&self) -> String {
        "géométrique intégré".into()
    }
}

/// Transforme des scores par classe en distribution : la classe `none`
/// (indice 0) reçoit ce qui manque au meilleur score.
fn normalize(scores: &[f32], out: &mut [f32]) {
    let best = scores[1..].iter().copied().fold(0.0f32, f32::max);
    let none = 1.0 - best;
    let sum: f32 = scores[1..].iter().sum::<f32>() + none;
    out[0] = none / sum;
    for (o, s) in out[1..].iter_mut().zip(&scores[1..]) {
        *o = s / sum;
    }
}

/// Détecte les balayages (paume ouverte) sur la trajectoire du centre de la main.
pub struct BuiltinDynamicClassifier {
    labels: Vec<String>,
}

impl Default for BuiltinDynamicClassifier {
    fn default() -> Self {
        Self {
            labels: DYNAMIC_LABELS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl BuiltinDynamicClassifier {
    pub const WINDOW: usize = 32;
    pub const SAMPLE_RATE_HZ: f32 = 60.0;
    /// Le balayage est cherché sur les ~0,33 dernière seconde.
    const SPAN: usize = 20;

    pub fn new() -> Self {
        Self::default()
    }
}

impl DynamicClassifier for BuiltinDynamicClassifier {
    fn labels(&self) -> &[String] {
        &self.labels
    }

    fn window_len(&self) -> usize {
        Self::WINDOW
    }

    fn sample_rate_hz(&self) -> f32 {
        Self::SAMPLE_RATE_HZ
    }

    fn classify(
        &mut self,
        window: &[WindowSample],
        _handedness: Handedness,
        out: &mut [f32],
    ) -> Result<(), ClassifierError> {
        let mut scores = [0.0f32; 5];
        if window.len() >= Self::SPAN {
            let span = &window[window.len() - Self::SPAN..];
            let palm = span.iter().map(|s| palm_size_2d(&s.image)).sum::<f32>() / span.len() as f32;
            let palm = palm.max(1e-6);
            let first = hand_center(&span[0].image);
            let last = hand_center(&span[span.len() - 1].image);
            let net = [(last[0] - first[0]) / palm, (last[1] - first[1]) / palm];

            // Longueur parcourue : un balayage est un mouvement franc, sans aller-retour.
            let mut path = 0.0;
            let mut prev = first;
            for s in &span[1..] {
                let c = hand_center(&s.image);
                path += ((c[0] - prev[0]).powi(2) + (c[1] - prev[1]).powi(2)).sqrt() / palm;
                prev = c;
            }
            let net_len = (net[0] * net[0] + net[1] * net[1]).sqrt();
            let straight = smoothstep(0.70, 0.88, net_len / path.max(1e-6));

            // Main ouverte pendant le geste (Annexe B), échantillonnée sur 4 trames.
            let open = (0..4)
                .map(|k| {
                    FingerState::from_world(&span[k * (span.len() - 1) / 3].world).extended_count()
                })
                .sum::<f32>()
                / 4.0;
            let open = smoothstep(2.2, 3.2, open);

            let amplitude = |main: f32, other: f32| {
                smoothstep(1.1, 1.7, main) * smoothstep(1.4, 2.4, main / other.abs().max(1e-3))
            };
            let common = straight * open;
            scores[1] = amplitude(-net[0], net[1]) * common; // swipe_left
            scores[2] = amplitude(net[0], net[1]) * common; // swipe_right
            scores[3] = amplitude(-net[1], net[0]) * common; // swipe_up
            scores[4] = amplitude(net[1], net[0]) * common; // swipe_down
        }
        normalize(&scores, out);
        Ok(())
    }

    fn description(&self) -> String {
        "géométrique intégré".into()
    }
}

/// Mains synthétiques pour les tests de ce crate et le rejeu sans caméra.
pub mod synthetic {
    use crate::{HandSample, Handedness, Landmarks};

    /// Flexion d'un doigt long : 0 = tendu, 1 = replié.
    #[derive(Debug, Clone, Copy)]
    pub struct Pose {
        pub fingers: [f32; 4],
        /// 0 = pouce replié sur la paume, 1 = pouce écarté.
        pub thumb: f32,
        /// Rapproche le bout du pouce et de l'index.
        pub pinch: bool,
        /// Rotation de la main dans le plan image, en degrés (sens horaire).
        pub roll_deg: f32,
    }

    impl Pose {
        pub const OPEN_PALM: Pose = Pose {
            fingers: [0.0; 4],
            thumb: 1.0,
            pinch: false,
            roll_deg: 0.0,
        };
        pub const FIST: Pose = Pose {
            fingers: [1.0; 4],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        /// Poing, pouce écarté : le pouce du modèle est vertical une fois la main tournée de 51°.
        pub const THUMBS_UP: Pose = Pose {
            fingers: [1.0; 4],
            thumb: 1.0,
            pinch: false,
            roll_deg: 51.0,
        };
        pub const THUMBS_DOWN: Pose = Pose {
            fingers: [1.0; 4],
            thumb: 1.0,
            pinch: false,
            roll_deg: -129.0,
        };
        pub const V_SIGN: Pose = Pose {
            fingers: [0.0, 0.0, 1.0, 1.0],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        pub const ROCK: Pose = Pose {
            fingers: [0.0, 1.0, 1.0, 0.0],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        pub const THREE: Pose = Pose {
            fingers: [0.0, 0.0, 0.0, 1.0],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        pub const POINTING: Pose = Pose {
            fingers: [0.0, 1.0, 1.0, 1.0],
            thumb: 0.0,
            pinch: false,
            roll_deg: 0.0,
        };
        pub const PINCH: Pose = Pose {
            fingers: [0.45, 0.1, 0.1, 0.1],
            thumb: 1.0,
            pinch: true,
            roll_deg: 0.0,
        };
        /// Main détendue, doigts à demi fléchis : ne doit correspondre à aucune pose.
        pub const RELAXED: Pose = Pose {
            fingers: [0.5; 4],
            thumb: 0.3,
            pinch: false,
            roll_deg: 0.0,
        };
    }

    fn rot2(p: [f32; 3], deg: f32) -> [f32; 3] {
        let (s, c) = deg.to_radians().sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    }

    /// Main droite schématique vue de face (paume vers la caméra, image miroir) :
    /// poignet à l'origine, doigts vers le haut (y négatif), en mètres.
    pub fn world(pose: Pose) -> Landmarks {
        let mut lm = [[0.0f32; 3]; 21];
        // Bases des doigts longs : index à gauche dans l'image miroir d'une main droite.
        let bases = [
            [-0.025, -0.085],
            [-0.004, -0.090],
            [0.016, -0.085],
            [0.034, -0.075],
        ];
        let lengths = [
            [0.040, 0.025, 0.020],
            [0.045, 0.028, 0.022],
            [0.041, 0.026, 0.021],
            [0.032, 0.019, 0.018],
        ];
        let splay: [f32; 4] = [-8.0, 0.0, 7.0, 15.0];
        for f in 0..4 {
            let mcp = [bases[f][0], bases[f][1], 0.0];
            lm[5 + 4 * f] = mcp;
            // Chaque phalange tourne vers la paume (z négatif) à mesure que le doigt se replie.
            let curl = pose.fingers[f];
            let joint_bend = [curl * 80.0, curl * 100.0, curl * 70.0];
            let mut angle = 0.0f32;
            let mut p = mcp;
            for j in 0..3 {
                angle += joint_bend[j];
                let (s, c) = angle.to_radians().sin_cos();
                let (ss, sc) = splay[f].to_radians().sin_cos();
                let len = lengths[f][j];
                p = [p[0] + len * c * ss, p[1] - len * c * sc, p[2] - len * s];
                lm[6 + 4 * f + j] = p;
            }
        }
        // Pouce : écarté sur le côté de la main, ou au repos contre le flanc de l'index.
        let t = pose.thumb;
        let lerp = |a: [f32; 3], b: [f32; 3]| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let (index_mcp, index_pip) = (lm[5], lm[6]);
        let rest_ip = [
            index_mcp[0] - 0.016,
            index_mcp[1] + 0.012,
            index_mcp[2] - 0.006,
        ];
        let rest_tip = [
            index_pip[0] - 0.012,
            index_pip[1] + 0.008,
            index_pip[2] - 0.004,
        ];
        lm[1] = [-0.025, -0.020, -0.005];
        lm[2] = lerp([-0.036, -0.048, -0.012], [-0.050, -0.040, -0.008]);
        lm[3] = lerp(rest_ip, [-0.072, -0.058, -0.010]);
        lm[4] = lerp(rest_tip, [-0.092, -0.074, -0.012]);
        if pose.pinch {
            let tip = lm[8];
            lm[3] = [tip[0] - 0.012, tip[1] + 0.028, tip[2] + 0.004];
            lm[4] = [tip[0] - 0.004, tip[1] + 0.006, tip[2] - 0.002];
        }
        for p in lm.iter_mut() {
            *p = rot2(*p, pose.roll_deg);
        }
        lm
    }

    /// Main synthétique centrée en `center` (repère image isotrope), de taille de
    /// paume `palm` dans ce repère.
    pub fn hand(pose: Pose, center: [f32; 2], palm: f32) -> HandSample {
        let world = world(pose);
        let scale = palm / crate::geometry::palm_size(&world);
        let mut image = world;
        for p in image.iter_mut() {
            *p = [
                center[0] + p[0] * scale,
                center[1] + p[1] * scale,
                p[2] * scale,
            ];
        }
        HandSample {
            image,
            world,
            handedness: Handedness::Right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic::{Pose, hand};
    use super::*;
    use crate::argmax;

    fn classify(pose: Pose) -> (String, f32) {
        let mut c = BuiltinStaticClassifier::new();
        let mut out = [0.0; 10];
        c.classify(&hand(pose, [0.5, 0.4], 0.08), &mut out).unwrap();
        let sum: f32 = out.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-4,
            "distribution non normalisée : {sum}"
        );
        let best = argmax(&out);
        (c.labels()[best].clone(), out[best])
    }

    #[test]
    fn synthetic_poses_are_recognised() {
        let cases = [
            (Pose::OPEN_PALM, "open_palm"),
            (Pose::FIST, "fist"),
            (Pose::THUMBS_UP, "thumbs_up"),
            (Pose::THUMBS_DOWN, "thumbs_down"),
            (Pose::V_SIGN, "v_sign"),
            (Pose::ROCK, "rock"),
            (Pose::THREE, "three_fingers"),
            (Pose::POINTING, "pointing"),
            (Pose::PINCH, "pinch"),
        ];
        for (pose, expected) in cases {
            let (label, p) = classify(pose);
            assert_eq!(label, expected, "pose {pose:?} (p = {p})");
            assert!(
                p >= 0.85,
                "{expected} : confiance {p} sous le seuil d'entrée"
            );
        }
    }

    #[test]
    fn relaxed_hand_is_none() {
        let (label, p) = classify(Pose::RELAXED);
        assert_eq!(label, "none");
        assert!(p > 0.6, "{p}");
    }

    #[test]
    fn sideways_thumb_is_neither_up_nor_down() {
        // À -39°, le pouce du modèle synthétique est horizontal.
        let mut pose = Pose::THUMBS_UP;
        pose.roll_deg = -39.0;
        let (label, _) = classify(pose);
        assert!(label != "thumbs_up" && label != "thumbs_down", "{label}");
    }

    fn swipe(dx: f32, dy: f32, pose: Pose) -> (String, f32) {
        let palm = 0.08;
        let mut window = Vec::new();
        for i in 0..BuiltinDynamicClassifier::WINDOW {
            // Immobile, puis déplacement sur les 20 dernières trames.
            let t = (i as f32 - 12.0).max(0.0) / 19.0;
            let h = hand(pose, [0.5 + dx * palm * t, 0.5 + dy * palm * t], palm);
            window.push(WindowSample {
                image: h.image,
                world: h.world,
            });
        }
        let mut c = BuiltinDynamicClassifier::new();
        let mut out = [0.0; 5];
        c.classify(&window, Handedness::Right, &mut out).unwrap();
        let best = argmax(&out);
        (c.labels()[best].clone(), out[best])
    }

    #[test]
    fn swipes_in_four_directions() {
        for (dx, dy, expected) in [
            (-2.5, 0.1, "swipe_left"),
            (2.5, -0.1, "swipe_right"),
            (0.2, -2.5, "swipe_up"),
            (0.0, 2.5, "swipe_down"),
        ] {
            let (label, p) = swipe(dx, dy, Pose::OPEN_PALM);
            assert_eq!(label, expected);
            assert!(p >= 0.8, "{expected} : {p}");
        }
    }

    #[test]
    fn small_diagonal_or_closed_hand_moves_are_none() {
        assert_eq!(swipe(0.6, 0.0, Pose::OPEN_PALM).0, "none");
        assert_eq!(swipe(2.0, 2.0, Pose::OPEN_PALM).0, "none");
        assert_eq!(swipe(2.5, 0.0, Pose::FIST).0, "none");
        assert_eq!(swipe(0.0, 0.0, Pose::OPEN_PALM).0, "none");
    }
}
