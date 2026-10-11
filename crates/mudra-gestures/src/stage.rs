//! Étage « gestes » du pipeline (figure 1, étapes ④ à ⑥) : lissage,
//! classifieurs, grandeurs continues et moteur. Sans entrées/sorties : le même
//! code sert en direct et au rejeu de sessions enregistrées (EF-64).

use mudra_config::{Config, Smoothing};

use crate::engine::{Engine, EngineEvent, HandObs, PoseProgress};
use crate::geometry::{
    INDEX_TIP, SpeedEstimator, hand_center, palm_size, palm_size_2d, pinch_ratio,
};
use crate::one_euro::LandmarkSmoother;
use crate::window::{SlidingWindow, WindowSample};
use crate::{
    ClassifierError, DynamicClassifier, HandSample, Handedness, Landmarks, StaticClassifier,
};

/// Une main en entrée de l'étage, telle que fournie par la vision.
#[derive(Debug, Clone, Copy)]
pub struct HandInput {
    pub track: u32,
    pub handedness: Handedness,
    /// Points image en pixels de l'image affichée.
    pub image: Landmarks,
    /// Points monde en mètres.
    pub world: Landmarks,
}

/// État d'une main après l'étage, pour l'aperçu et l'enregistrement.
#[derive(Debug, Clone)]
pub struct HandView {
    pub track: u32,
    pub handedness: Handedness,
    /// Points image lissés, en pixels de l'image affichée.
    pub image: Landmarks,
    /// Points monde lissés.
    pub world: Landmarks,
    pub static_probs: Vec<f32>,
    /// Dernière distribution dynamique évaluée (vide tant que la fenêtre n'est pas pleine).
    pub dynamic_probs: Vec<f32>,
    pub speed: f32,
    pub pinch: f32,
    pub progress: Option<PoseProgress>,
}

struct Slot {
    track: u32,
    image_smoother: LandmarkSmoother,
    world_smoother: LandmarkSmoother,
    speed: SpeedEstimator,
    window: SlidingWindow,
    frames: u32,
    dynamic_fresh: bool,
    seen: bool,
    /// Bout de l'index non lissé : la souris a son propre profil de lissage (§7.5).
    raw_index_tip: [f32; 2],
    /// Main vue par les classifieurs, et sa distribution statique avant lissage.
    sample: HandSample,
    raw_probs: Vec<f32>,
    view: HandView,
}

/// Lissage exponentiel des probabilités statiques : absorbe les creux d'une trame.
const PROB_SMOOTHING: f32 = 0.5;
/// Le classifieur dynamique est évalué toutes les 2 trames (EF-21).
const DYNAMIC_EVERY: u32 = 2;

pub struct GestureStage {
    static_classifier: Box<dyn StaticClassifier>,
    dynamic_classifier: Box<dyn DynamicClassifier>,
    engine: Engine,
    smoothing: Smoothing,
    slots: Vec<Slot>,
    resampled: Vec<WindowSample>,
}

impl GestureStage {
    pub fn new(
        config: &Config,
        static_classifier: Box<dyn StaticClassifier>,
        dynamic_classifier: Box<dyn DynamicClassifier>,
    ) -> Self {
        let engine = Engine::new(
            config,
            static_classifier.labels(),
            dynamic_classifier.labels(),
            static_classifier.two_hand_labels(),
        );
        let resampled = Vec::with_capacity(dynamic_classifier.window_len());
        Self {
            static_classifier,
            dynamic_classifier,
            engine,
            smoothing: config.smoothing,
            slots: Vec::with_capacity(4),
            resampled,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    pub fn static_labels(&self) -> &[String] {
        self.static_classifier.labels()
    }

    pub fn dynamic_labels(&self) -> &[String] {
        self.dynamic_classifier.labels()
    }

    pub fn classifier_descriptions(&self) -> (String, String) {
        (
            self.static_classifier.description(),
            self.dynamic_classifier.description(),
        )
    }

    /// Applique une configuration rechargée à chaud (mêmes classifieurs).
    pub fn set_config(&mut self, config: &Config, events: &mut Vec<EngineEvent>) {
        self.smoothing = config.smoothing;
        for slot in &mut self.slots {
            slot.image_smoother.set_params(config.smoothing);
            slot.world_smoother.set_params(config.smoothing);
        }
        self.engine.set_config(
            config,
            self.static_classifier.labels(),
            self.dynamic_classifier.labels(),
            self.static_classifier.two_hand_labels(),
            events,
        );
    }

    /// Remplace les classifieurs à chaud (poses perso ajoutées ou retirées).
    /// Les labels peuvent changer : les pistes repartent de zéro.
    pub fn set_classifiers(
        &mut self,
        config: &Config,
        static_classifier: Box<dyn StaticClassifier>,
        dynamic_classifier: Box<dyn DynamicClassifier>,
        events: &mut Vec<EngineEvent>,
    ) {
        self.resampled = Vec::with_capacity(dynamic_classifier.window_len());
        self.static_classifier = static_classifier;
        self.dynamic_classifier = dynamic_classifier;
        self.slots.clear();
        self.set_config(config, events);
    }

    /// Oublie toutes les pistes (pause, reprise, changement de caméra).
    pub fn reset_tracks(&mut self) {
        self.slots.clear();
    }

    /// Mains de la dernière trame traitée.
    pub fn hands(&self) -> impl Iterator<Item = &HandView> {
        self.slots.iter().filter(|s| s.seen).map(|s| &s.view)
    }

    fn slot_index(&mut self, track: u32, handedness: Handedness) -> usize {
        if let Some(i) = self.slots.iter().position(|s| s.track == track) {
            return i;
        }
        let empty = [[0.0; 3]; 21];
        self.slots.push(Slot {
            track,
            image_smoother: LandmarkSmoother::new(self.smoothing),
            world_smoother: LandmarkSmoother::new(self.smoothing),
            speed: SpeedEstimator::default(),
            window: SlidingWindow::new(
                self.dynamic_classifier.window_len(),
                self.dynamic_classifier.sample_rate_hz(),
            ),
            frames: 0,
            dynamic_fresh: false,
            seen: false,
            raw_index_tip: [0.0; 2],
            sample: HandSample {
                image: empty,
                world: empty,
                handedness,
            },
            raw_probs: vec![0.0; self.static_classifier.labels().len()],
            view: HandView {
                track,
                handedness,
                image: empty,
                world: empty,
                static_probs: vec![0.0; self.static_classifier.labels().len()],
                dynamic_probs: Vec::with_capacity(self.dynamic_classifier.labels().len()),
                speed: 0.0,
                pinch: 1.0,
                progress: None,
            },
        });
        self.slots.len() - 1
    }

    /// Traite une trame : `width`/`height` sont ceux de l'image affichée.
    /// Les événements du moteur sont ajoutés à `events`.
    pub fn process(
        &mut self,
        now_us: u64,
        width: f32,
        height: f32,
        hands: &[HandInput],
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), ClassifierError> {
        for slot in &mut self.slots {
            slot.seen = false;
        }
        for hand in hands {
            let i = self.slot_index(hand.track, hand.handedness);
            let slot = &mut self.slots[i];
            slot.seen = true;
            slot.frames += 1;
            slot.raw_index_tip = [hand.image[INDEX_TIP][0], hand.image[INDEX_TIP][1]];
            let view = &mut slot.view;
            view.handedness = hand.handedness;

            // ④ Lissage One Euro, en unités de taille de paume.
            view.image = slot
                .image_smoother
                .smooth(&hand.image, palm_size_2d(&hand.image), now_us);
            view.world = slot
                .world_smoother
                .smooth(&hand.world, palm_size(&hand.world), now_us);

            // Repère isotrope pour les classifieurs : tout est divisé par la largeur.
            let mut sample = HandSample {
                image: view.image,
                world: view.world,
                handedness: hand.handedness,
            };
            for p in sample.image.iter_mut() {
                *p = [p[0] / width, p[1] / width, p[2] / width];
            }

            // ⑤ Classifieur statique à chaque trame.
            slot.sample = sample;
            self.static_classifier
                .classify(&sample, &mut slot.raw_probs)?;

            // ⑤ Classifieur dynamique sur fenêtre glissante, une trame sur deux.
            slot.window.push(
                now_us,
                WindowSample {
                    image: sample.image,
                    world: sample.world,
                },
            );
            slot.dynamic_fresh = false;
            if slot.frames.is_multiple_of(DYNAMIC_EVERY)
                && slot.window.resample(
                    self.dynamic_classifier.min_window_len(),
                    &mut self.resampled,
                )
            {
                let n = self.dynamic_classifier.labels().len();
                view.dynamic_probs.resize(n, 0.0);
                self.dynamic_classifier.classify(
                    &self.resampled,
                    hand.handedness,
                    &mut view.dynamic_probs,
                )?;
                slot.dynamic_fresh = true;
            }

            // Grandeurs géométriques (EF-22).
            let center = hand_center(&sample.image);
            view.speed = slot
                .speed
                .update(center, palm_size_2d(&sample.image), now_us);
            view.pinch = pinch_ratio(&view.world);
        }
        self.slots.retain(|s| s.seen);

        // ⑤ Poses à deux mains, puis lissage des probabilités.
        if let [a, b] = self.slots.as_mut_slice() {
            self.static_classifier
                .classify_pair([&a.sample, &b.sample], [&mut a.raw_probs, &mut b.raw_probs])?;
        }
        for slot in &mut self.slots {
            if slot.frames == 1 {
                slot.view.static_probs.copy_from_slice(&slot.raw_probs);
            } else {
                for (p, raw) in slot.view.static_probs.iter_mut().zip(&slot.raw_probs) {
                    *p += PROB_SMOOTHING * (raw - *p);
                }
            }
        }

        // ⑥ Moteur de gestes.
        let empty: &[f32] = &[];
        let placeholder = HandObs {
            track: 0,
            handedness: Handedness::Right,
            static_probs: empty,
            dynamic_probs: None,
            speed: 0.0,
            center: [0.0; 2],
            index_tip: [0.0; 2],
            palm: [1.0; 2],
            pinch: 1.0,
        };
        let mut observations = [placeholder; 4];
        let count = self.slots.len().min(observations.len());
        for (obs, slot) in observations.iter_mut().zip(&self.slots) {
            let v = &slot.view;
            let center = hand_center(&v.image);
            let palm = palm_size_2d(&v.image);
            *obs = HandObs {
                track: v.track,
                handedness: v.handedness,
                static_probs: &v.static_probs,
                dynamic_probs: slot.dynamic_fresh.then_some(v.dynamic_probs.as_slice()),
                speed: v.speed,
                center: [center[0] / width, center[1] / height],
                index_tip: [
                    slot.raw_index_tip[0] / width,
                    slot.raw_index_tip[1] / height,
                ],
                palm: [palm / width, palm / height],
                pinch: v.pinch,
            };
        }
        let first_event = events.len();
        self.engine.update(now_us, &observations[..count], events);

        for event in &events[first_event..] {
            if let EngineEvent::FlushDynamicWindow { track } = event
                && let Some(slot) = self.slots.iter_mut().find(|s| s.track == *track)
            {
                slot.window.clear();
                slot.view.dynamic_probs.clear();
            }
        }
        for slot in &mut self.slots {
            slot.view.progress =
                self.engine
                    .pose_progress(slot.track, slot.view.handedness, now_us);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::builtin::synthetic::{Pose, hand};
    use crate::builtin::{BuiltinDynamicClassifier, BuiltinStaticClassifier};
    use crate::engine::{ContinuousValue, EngineState};

    const W: f32 = 1280.0;
    const H: f32 = 720.0;
    const FRAME_US: u64 = 16_667;

    struct Rig {
        stage: GestureStage,
        now: u64,
        config: Config,
    }

    impl Rig {
        fn new() -> Self {
            let config =
                mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("d")).unwrap();
            let stage = GestureStage::new(
                &config,
                Box::new(BuiltinStaticClassifier::new()),
                Box::new(BuiltinDynamicClassifier::new()),
            );
            Self {
                stage,
                now: 1_000_000,
                config,
            }
        }

        /// Maintient une pose synthétique ; le centre va de `from` à `to` (pixels).
        fn show(&mut self, pose: Pose, from: [f32; 2], to: [f32; 2], ms: u64) -> Vec<EngineEvent> {
            let frames = (ms * 1000).div_ceil(FRAME_US);
            let mut events = Vec::new();
            for i in 0..frames {
                self.now += FRAME_US;
                let t = (i + 1) as f32 / frames as f32;
                let center = [
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ];
                // La main synthétique est en repère isotrope : on la met en pixels.
                let mut h = hand(pose, [center[0] / W, center[1] / W], 100.0 / W);
                for p in h.image.iter_mut() {
                    *p = [p[0] * W, p[1] * W, p[2] * W];
                }
                let input = HandInput {
                    track: 7,
                    handedness: Handedness::Right,
                    image: h.image,
                    world: h.world,
                };
                self.stage
                    .process(self.now, W, H, &[input], &mut events)
                    .unwrap();
            }
            events
        }

        fn nothing(&mut self, ms: u64) -> Vec<EngineEvent> {
            let mut events = Vec::new();
            for _ in 0..(ms * 1000).div_ceil(FRAME_US) {
                self.now += FRAME_US;
                self.stage
                    .process(self.now, W, H, &[], &mut events)
                    .unwrap();
            }
            events
        }

        fn fired(&self, events: &[EngineEvent]) -> Vec<String> {
            events
                .iter()
                .filter_map(|e| match e {
                    EngineEvent::GestureFired { binding, .. } => {
                        Some(self.config.bindings[*binding].gesture.clone())
                    }
                    _ => None,
                })
                .collect()
        }
    }

    const CENTER: [f32; 2] = [640.0, 400.0];

    #[test]
    fn demo_scenario_wake_then_gestures() {
        let mut rig = Rig::new();
        // Verrouillé : un pouce levé ne fait rien.
        assert!(rig.show(Pose::THUMBS_UP, CENTER, CENTER, 800).is_empty());
        rig.show(Pose::RELAXED, CENTER, CENTER, 200);
        // Réveil : paume ouverte maintenue 600 ms.
        rig.show(Pose::OPEN_PALM, CENTER, CENTER, 750);
        assert_eq!(rig.stage.engine().state(), EngineState::Armed);
        // Pouce levé : volume +5 %, répété.
        let events = rig.show(Pose::THUMBS_UP, CENTER, CENTER, 900);
        let fired = rig.fired(&events);
        assert!(
            fired.len() >= 2 && fired.iter().all(|g| g == "thumbs_up"),
            "{fired:?}"
        );
        // V : capture d'écran, une seule fois.
        let events = rig.show(Pose::V_SIGN, CENTER, CENTER, 900);
        assert_eq!(rig.fired(&events), vec!["v_sign"]);
        // Rock : lance un programme (maintien de 500 ms).
        let events = rig.show(Pose::ROCK, CENTER, CENTER, 800);
        assert_eq!(rig.fired(&events), vec!["rock"]);
        // Poing : verrouillage.
        rig.show(Pose::FIST, CENTER, CENTER, 500);
        assert_eq!(rig.stage.engine().state(), EngineState::Locked);
    }

    #[test]
    fn swipe_fires_media_binding_once() {
        let mut rig = Rig::new();
        rig.show(Pose::OPEN_PALM, CENTER, CENTER, 750);
        assert_eq!(rig.stage.engine().state(), EngineState::Armed);
        // Balayage vers la droite de 3 tailles de paume en 250 ms, puis arrêt.
        let mut events = rig.show(Pose::OPEN_PALM, CENTER, [940.0, 400.0], 250);
        events.extend(rig.show(Pose::OPEN_PALM, [940.0, 400.0], [940.0, 400.0], 500));
        assert_eq!(rig.fired(&events), vec!["swipe_right"], "{events:?}");
        assert!(events.contains(&EngineEvent::FlushDynamicWindow { track: 7 }));
    }

    #[test]
    fn slow_drift_is_not_a_swipe() {
        let mut rig = Rig::new();
        rig.show(Pose::OPEN_PALM, CENTER, CENTER, 750);
        let events = rig.show(Pose::OPEN_PALM, CENTER, [940.0, 400.0], 3000);
        assert!(rig.fired(&events).is_empty());
    }

    #[test]
    fn pinch_controls_volume_then_releases() {
        let mut rig = Rig::new();
        rig.show(Pose::OPEN_PALM, CENTER, CENTER, 750);
        rig.show(Pose::PINCH, CENTER, CENTER, 400);
        assert_eq!(rig.stage.engine().state(), EngineState::Continuous);
        // La main monte de 144 px (20 % de la hauteur) : (0,20 − 0,02) × 1,5 = +27 %.
        let events = rig.show(Pose::PINCH, CENTER, [640.0, 256.0], 600);
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                EngineEvent::ContinuousUpdate {
                    value: ContinuousValue::VolumeDelta(v),
                    ..
                } => Some(*v),
                _ => None,
            })
            .expect("mise à jour du volume");
        assert!((last - 0.27).abs() < 0.03, "{last}");
        rig.show(Pose::OPEN_PALM, [640.0, 256.0], [640.0, 256.0], 300);
        assert_eq!(rig.stage.engine().state(), EngineState::Armed);
    }

    #[test]
    fn tracks_are_forgotten_when_the_hand_leaves() {
        let mut rig = Rig::new();
        rig.show(Pose::OPEN_PALM, CENTER, CENTER, 100);
        assert_eq!(rig.stage.hands().count(), 1);
        let view = rig.stage.hands().next().unwrap();
        assert_eq!(view.track, 7);
        assert!((view.static_probs.iter().sum::<f32>() - 1.0).abs() < 1e-3);
        rig.nothing(50);
        assert_eq!(rig.stage.hands().count(), 0);
    }
}
