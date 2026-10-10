use std::path::Path;

use mudra_config::{Config, KnownGestures};
use proptest::prelude::*;

use super::*;

const FRAME_US: u64 = 16_667;

struct Rig {
    engine: Engine,
    config: Config,
    statics: Vec<String>,
    dynamics: Vec<String>,
    now: u64,
    log: Vec<(u64, EngineEvent)>,
}

impl Rig {
    fn new(config: Config) -> Self {
        let known = KnownGestures::builtin();
        Self {
            engine: Engine::new(&config, &known.static_, &known.dynamic, &known.two_hands),
            config,
            statics: known.static_,
            dynamics: known.dynamic,
            now: 1_000_000,
            log: Vec::new(),
        }
    }

    fn default() -> Self {
        Self::new(
            mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("default.toml"))
                .unwrap(),
        )
    }

    fn probs(labels: &[String], label: &str, p: f32) -> Vec<f32> {
        let mut out = vec![0.0; labels.len()];
        let i = labels.iter().position(|l| l == label).unwrap();
        out[i] = p;
        if label != "none" {
            out[0] = 1.0 - p;
        }
        out
    }

    fn obs<'a>(statics: &'a [f32], dynamics: Option<&'a [f32]>) -> HandObs<'a> {
        HandObs {
            track: 1,
            handedness: Handedness::Right,
            static_probs: statics,
            dynamic_probs: dynamics,
            speed: 0.0,
            center: [0.5, 0.5],
            index_tip: [0.5, 0.4],
            palm: [0.08, 0.14],
            pinch: 1.0,
        }
    }

    fn step(&mut self, hands: &[HandObs<'_>]) -> Vec<EngineEvent> {
        self.now += FRAME_US;
        let mut events = Vec::new();
        self.engine.update(self.now, hands, &mut events);
        self.log
            .extend(events.iter().cloned().map(|e| (self.now, e)));
        events
    }

    /// Maintient une pose pendant `ms` millisecondes et renvoie les événements produits.
    fn hold(&mut self, label: &str, p: f32, ms: u64) -> Vec<EngineEvent> {
        self.hold_with(label, p, ms, |_| {})
    }

    fn hold_with(
        &mut self,
        label: &str,
        p: f32,
        ms: u64,
        tweak: impl Fn(&mut HandObs<'_>),
    ) -> Vec<EngineEvent> {
        let probs = Self::probs(&self.statics.clone(), label, p);
        let mut all = Vec::new();
        for _ in 0..(ms * 1000).div_ceil(FRAME_US) {
            let mut obs = Self::obs(&probs, None);
            tweak(&mut obs);
            all.extend(self.step(&[obs]));
        }
        all
    }

    fn idle(&mut self, ms: u64) -> Vec<EngineEvent> {
        let mut all = Vec::new();
        for _ in 0..(ms * 1000).div_ceil(FRAME_US) {
            all.extend(self.step(&[]));
        }
        all
    }

    fn dynamic(&mut self, label: &str, p: f32, evaluations: usize) -> Vec<EngineEvent> {
        let statics = Self::probs(&self.statics.clone(), "none", 1.0);
        let dynamics = Self::probs(&self.dynamics.clone(), label, p);
        let mut all = Vec::new();
        for _ in 0..evaluations {
            let mut obs = Self::obs(&statics, Some(&dynamics));
            obs.speed = 8.0;
            all.extend(self.step(&[obs]));
            // Une trame sur deux n'est pas évaluée (EF-21).
            let mut obs = Self::obs(&statics, None);
            obs.speed = 8.0;
            all.extend(self.step(&[obs]));
        }
        all
    }

    fn arm(&mut self) {
        self.hold("open_palm", 0.95, 700);
        assert_eq!(self.engine.state(), EngineState::Armed);
        self.hold("none", 1.0, 150);
    }

    fn binding_index(&self, gesture: &str) -> usize {
        self.config
            .bindings
            .iter()
            .position(|b| b.gesture == gesture)
            .unwrap()
    }
}

fn fired(events: &[EngineEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::GestureFired {
                gesture, repeat, ..
            } => Some((gesture.clone(), *repeat)),
            _ => None,
        })
        .collect()
}

#[test]
fn starts_locked_and_ignores_bindings() {
    let mut rig = Rig::default();
    assert_eq!(rig.engine.state(), EngineState::Locked);
    for gesture in ["thumbs_up", "v_sign", "rock", "fist", "pinch", "pointing"] {
        assert!(
            rig.hold(gesture, 0.99, 1500).is_empty(),
            "{gesture} a produit un événement en Verrouillé"
        );
    }
    assert!(rig.dynamic("swipe_left", 0.99, 10).is_empty());
    assert_eq!(rig.engine.state(), EngineState::Locked);
}

#[test]
fn wake_gesture_requires_hold() {
    let mut rig = Rig::default();
    assert!(rig.hold("open_palm", 0.95, 500).is_empty());
    assert!(rig.hold("none", 1.0, 100).is_empty());
    assert_eq!(rig.engine.state(), EngineState::Locked);
    let events = rig.hold("open_palm", 0.95, 650);
    assert_eq!(
        events,
        vec![EngineEvent::StateChanged {
            from: EngineState::Locked,
            to: EngineState::Armed,
            reason: Reason::WakeGesture
        }]
    );
}

#[test]
fn wake_below_enter_threshold_never_arms() {
    let mut rig = Rig::default();
    assert!(rig.hold("open_palm", 0.80, 3000).is_empty());
}

#[test]
fn armed_times_out_without_action() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.idle(4100);
    assert_eq!(
        events,
        vec![EngineEvent::StateChanged {
            from: EngineState::Armed,
            to: EngineState::Locked,
            reason: Reason::ArmedTimeout
        }]
    );
}

#[test]
fn static_binding_fires_on_rising_edge_then_repeats() {
    let mut rig = Rig::default();
    rig.arm();
    assert!(
        rig.hold("thumbs_up", 0.95, 250).is_empty(),
        "déclenché avant hold_ms"
    );
    let events = rig.hold("thumbs_up", 0.95, 1000);
    let f = fired(&events);
    assert_eq!(f[0], ("thumbs_up".to_string(), false));
    // Répétition toutes les 250 ms pendant ~950 ms restants.
    let repeats = f.iter().filter(|(_, r)| *r).count();
    assert!((3..=4).contains(&repeats), "{repeats} répétitions");
    match &events[0] {
        EngineEvent::GestureFired {
            binding,
            confidence,
            hand,
            ..
        } => {
            assert_eq!(*binding, rig.binding_index("thumbs_up"));
            assert!((*confidence - 0.95).abs() < 1e-6);
            assert_eq!(*hand, Handedness::Right);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn non_repeating_binding_fires_once_per_episode() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.hold("v_sign", 0.95, 2500);
    assert_eq!(fired(&events), vec![("v_sign".to_string(), false)]);
}

#[test]
fn hysteresis_keeps_pose_between_exit_and_enter() {
    let mut rig = Rig::default();
    rig.arm();
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 400)).len(), 1);
    // Entre static_exit (0,60) et static_enter (0,85) : la pose reste confirmée.
    assert!(rig.hold("v_sign", 0.70, 400).is_empty());
    // De retour au-dessus : pas de nouveau front montant.
    assert!(rig.hold("v_sign", 0.95, 400).is_empty());
    // Relâchée, puis refaite après le cooldown : nouveau déclenchement.
    rig.hold("none", 1.0, 900);
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 400)).len(), 1);
}

#[test]
fn cooldown_delays_refire() {
    let mut rig = Rig::default();
    rig.arm();
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 350)).len(), 1);
    rig.hold("none", 1.0, 150);
    // Refait aussitôt : le cooldown de 800 ms n'est pas écoulé au bout de hold_ms.
    assert!(rig.hold("v_sign", 0.95, 320).is_empty());
    // Toujours maintenue quand le cooldown expire.
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 400)).len(), 1);
}

#[test]
fn fast_hand_confirms_nothing() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.hold_with("v_sign", 0.99, 1500, |o| o.speed = 6.0);
    assert!(events.is_empty());
}

#[test]
fn lock_gesture_locks() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.hold("fist", 0.95, 400);
    assert_eq!(
        events,
        vec![EngineEvent::StateChanged {
            from: EngineState::Armed,
            to: EngineState::Locked,
            reason: Reason::LockGesture
        }]
    );
}

#[test]
fn action_restarts_armed_timer() {
    let mut rig = Rig::default();
    rig.arm();
    rig.idle(3000);
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 400)).len(), 1);
    rig.idle(3000);
    assert_eq!(rig.engine.state(), EngineState::Armed);
    rig.idle(1200);
    assert_eq!(rig.engine.state(), EngineState::Locked);
}

#[test]
fn dynamic_needs_two_evaluations_then_refractory() {
    let mut rig = Rig::default();
    rig.arm();
    assert!(rig.dynamic("swipe_left", 0.95, 1).is_empty());
    let events = rig.dynamic("swipe_left", 0.95, 1);
    assert_eq!(fired(&events), vec![("swipe_left".to_string(), false)]);
    assert!(events.contains(&EngineEvent::FlushDynamicWindow { track: 1 }));
    // Période réfractaire de 600 ms : rien, même si le classifieur insiste.
    assert!(fired(&rig.dynamic("swipe_left", 0.99, 15)).is_empty());
    // Après la période réfractaire et le cooldown du binding.
    rig.idle(500);
    assert_eq!(fired(&rig.dynamic("swipe_left", 0.95, 2)).len(), 1);
}

#[test]
fn dynamic_below_threshold_or_interrupted_does_not_fire() {
    let mut rig = Rig::default();
    rig.arm();
    assert!(rig.dynamic("swipe_up", 0.70, 6).is_empty());
    rig.dynamic("swipe_up", 0.95, 1);
    rig.dynamic("none", 1.0, 1);
    assert!(rig.dynamic("swipe_up", 0.95, 1).is_empty());
}

#[test]
fn dynamic_gesture_suspends_static_poses() {
    let mut rig = Rig::default();
    rig.arm();
    assert_eq!(fired(&rig.dynamic("swipe_right", 0.95, 2)).len(), 1);
    // 300 ms de suspension + 300 ms de maintien : rien avant ~600 ms.
    assert!(rig.hold("v_sign", 0.99, 500).is_empty());
    assert_eq!(fired(&rig.hold("v_sign", 0.99, 300)).len(), 1);
}

#[test]
fn continuous_volume_follows_vertical_displacement() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.hold("pinch", 0.95, 300);
    assert!(events.contains(&EngineEvent::ContinuousStarted { control: 0 }));
    assert_eq!(rig.engine.state(), EngineState::Continuous);
    assert_eq!(rig.engine.active_continuous(), Some(0));

    // Dans la zone morte : aucune mise à jour.
    assert!(
        rig.hold_with("pinch", 0.95, 200, |o| o.center = [0.5, 0.49])
            .is_empty()
    );
    // Main montée de 0,22 : (0,22 − 0,02) × 1,5 = +0,30.
    let events = rig.hold_with("pinch", 0.95, 200, |o| o.center = [0.5, 0.28]);
    assert_eq!(events.len(), 1, "{events:?}");
    match events[0] {
        EngineEvent::ContinuousUpdate {
            value: ContinuousValue::VolumeDelta(v),
            ..
        } => {
            assert!((v - 0.30).abs() < 1e-3, "{v}")
        }
        ref other => panic!("{other:?}"),
    }

    // Pose relâchée plus de 150 ms : retour à Armé.
    let events = rig.hold("none", 1.0, 250);
    assert!(events.contains(&EngineEvent::ContinuousEnded { control: 0 }));
    assert_eq!(rig.engine.state(), EngineState::Armed);
}

#[test]
fn continuous_survives_short_release_and_exits_on_lost_hand() {
    let mut rig = Rig::default();
    rig.arm();
    rig.hold("pinch", 0.95, 300);
    rig.hold("none", 1.0, 100);
    assert_eq!(rig.engine.state(), EngineState::Continuous);
    rig.hold("pinch", 0.95, 100);
    rig.idle(250);
    assert_eq!(rig.engine.state(), EngineState::Continuous);
    let events = rig.idle(100);
    assert!(events.contains(&EngineEvent::StateChanged {
        from: EngineState::Continuous,
        to: EngineState::Armed,
        reason: Reason::HandLost
    }));
}

#[test]
fn mouse_moves_and_clicks_with_hysteresis() {
    let mut rig = Rig::default();
    rig.arm();
    let events = rig.hold("pointing", 0.95, 350);
    assert!(events.contains(&EngineEvent::ContinuousStarted { control: 1 }));

    let events = rig.hold_with("pointing", 0.95, 500, |o| o.index_tip = [0.5, 0.5]);
    match events.last() {
        Some(EngineEvent::ContinuousUpdate {
            value: ContinuousValue::MouseMove { x, y },
            ..
        }) => {
            assert!((x - 0.5).abs() < 0.02 && (y - 0.5).abs() < 0.02, "{x}, {y}");
        }
        other => panic!("{other:?}"),
    }
    // Hors zone active : bord de l'écran.
    let events = rig.hold_with("pointing", 0.95, 1500, |o| o.index_tip = [0.95, 0.05]);
    match events.last() {
        Some(EngineEvent::ContinuousUpdate {
            value: ContinuousValue::MouseMove { x, y },
            ..
        }) => {
            assert!(*x > 0.99 && *y < 0.01, "{x}, {y}");
        }
        other => panic!("{other:?}"),
    }

    let button = |events: &[EngineEvent]| -> Vec<bool> {
        events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::ContinuousUpdate {
                    value: ContinuousValue::MouseButton { down },
                    ..
                } => Some(*down),
                _ => None,
            })
            .collect()
    };
    // 0,30 : entre les deux seuils, pas de clic.
    assert!(button(&rig.hold_with("pointing", 0.95, 100, |o| o.pinch = 0.30)).is_empty());
    // Pincement : la pose « pointing » disparaît mais le mode tient grâce au clic.
    assert_eq!(
        button(&rig.hold_with("pinch", 0.95, 300, |o| o.pinch = 0.15)),
        vec![true]
    );
    assert_eq!(rig.engine.state(), EngineState::Continuous);
    assert!(button(&rig.hold_with("pinch", 0.95, 100, |o| o.pinch = 0.30)).is_empty());
    assert_eq!(
        button(&rig.hold_with("pointing", 0.95, 100, |o| o.pinch = 0.60)),
        vec![false]
    );
}

/// Plus grand écart du curseur après un saut du bout de l'index d'une seule image.
fn cursor_jump_after_glitch(stabilization_ms: u64) -> f32 {
    let mut config =
        mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("default.toml")).unwrap();
    for c in &mut config.continuous {
        if let Control::Mouse {
            stabilization_ms: ms,
            ..
        } = &mut c.control
        {
            *ms = stabilization_ms;
        }
    }
    let mut rig = Rig::new(config);
    rig.arm();
    rig.hold("pointing", 0.95, 350);
    rig.hold_with("pointing", 0.95, 600, |o| o.index_tip = [0.5, 0.5]);
    let mut events = rig.hold_with("pointing", 0.95, 1, |o| o.index_tip = [0.62, 0.5]);
    events.extend(rig.hold_with("pointing", 0.95, 400, |o| o.index_tip = [0.5, 0.5]));
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::ContinuousUpdate {
                value: ContinuousValue::MouseMove { x, .. },
                ..
            } => Some((x - 0.5).abs()),
            _ => None,
        })
        .fold(0.0, f32::max)
}

#[test]
fn mouse_ignores_single_frame_tracking_glitch() {
    // Sans stabilisation, le curseur part avec le saut (20 % de l'écran ici).
    assert!(cursor_jump_after_glitch(0) > 0.03);
    assert!(cursor_jump_after_glitch(50) < 0.001);
}

#[test]
fn leaving_mouse_mode_releases_button() {
    let mut rig = Rig::default();
    rig.arm();
    rig.hold("pointing", 0.95, 350);
    rig.hold_with("pinch", 0.95, 100, |o| o.pinch = 0.1);
    let events = rig.idle(400);
    let up = events
        .iter()
        .position(|e| {
            matches!(
                e,
                EngineEvent::ContinuousUpdate {
                    value: ContinuousValue::MouseButton { down: false },
                    ..
                }
            )
        })
        .expect("bouton relâché");
    let ended = events
        .iter()
        .position(|e| matches!(e, EngineEvent::ContinuousEnded { .. }))
        .unwrap();
    assert!(up < ended);
}

#[test]
fn pause_and_resume() {
    let mut rig = Rig::default();
    rig.arm();
    let mut events = Vec::new();
    rig.engine.pause(&mut events);
    assert_eq!(rig.engine.state(), EngineState::Paused);
    assert!(rig.hold("thumbs_up", 0.99, 1000).is_empty());
    assert!(rig.hold("open_palm", 0.99, 1000).is_empty());
    events.clear();
    rig.engine.resume(&mut events);
    assert_eq!(
        events,
        vec![EngineEvent::StateChanged {
            from: EngineState::Paused,
            to: EngineState::Locked,
            reason: Reason::Resume
        }]
    );
}

#[test]
fn always_on_mode_never_locks() {
    let mut config = Config::default();
    config.activation.mode = ActivationMode::AlwaysOn;
    config.bindings = mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("d"))
        .unwrap()
        .bindings;
    let mut rig = Rig::new(config);
    assert_eq!(rig.engine.state(), EngineState::Armed);
    rig.idle(10_000);
    assert_eq!(rig.engine.state(), EngineState::Armed);
    assert_eq!(fired(&rig.hold("v_sign", 0.95, 400)).len(), 1);
    rig.hold("none", 1.0, 200);
    rig.hold("fist", 0.99, 1000);
    assert_eq!(rig.engine.state(), EngineState::Armed);
}

/// Config par défaut, plus un binding à deux mains sur `gesture`.
fn rig_with_pair(gesture: &str) -> (Rig, usize) {
    let mut config =
        mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("d")).unwrap();
    config.bindings.retain(|b| b.hand != HandFilter::Both);
    let mut pair = config.bindings[0].clone();
    pair.gesture = gesture.into();
    pair.hand = HandFilter::Both;
    pair.hold_ms = Some(300);
    pair.repeat_ms = None;
    config.bindings.push(pair);
    let index = config.bindings.len() - 1;
    (Rig::new(config), index)
}

impl Rig {
    /// Les deux mains tiennent la même pose pendant `ms` millisecondes.
    fn hold_both(&mut self, label: &str, ms: u64) -> Vec<EngineEvent> {
        let probs = Self::probs(&self.statics.clone(), label, 0.97);
        let mut all = Vec::new();
        for _ in 0..(ms * 1000).div_ceil(FRAME_US) {
            let right = Self::obs(&probs, None);
            let mut left = Self::obs(&probs, None);
            left.track = 2;
            left.handedness = Handedness::Left;
            all.extend(self.step(&[right, left]));
        }
        all
    }
}

fn fired_bindings(events: &[EngineEvent]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::GestureFired { binding, .. } => Some(*binding),
            _ => None,
        })
        .collect()
}

#[test]
fn two_open_palms_fire_the_two_hand_binding_once() {
    // La paume ouverte est le geste de réveil : seule, elle ne fait rien une fois armé.
    let (mut rig, pair) = rig_with_pair("open_palm");
    rig.arm();
    assert!(rig.hold("open_palm", 0.97, 800).is_empty());
    rig.hold("none", 1.0, 200);
    assert!(fired_bindings(&rig.hold_both("open_palm", 200)).is_empty());
    assert_eq!(
        fired_bindings(&rig.hold_both("open_palm", 1200)),
        vec![pair]
    );
    assert_eq!(rig.engine.state(), EngineState::Armed);
}

#[test]
fn a_held_two_hand_pose_fires_once_whatever_the_tracking_does() {
    let (mut rig, pair) = rig_with_pair("v_sign");
    rig.arm();
    let steady = Rig::probs(&rig.statics.clone(), "v_sign", 0.97);
    // Entre les seuils de sortie et d'entrée : la pose est toujours tenue.
    let weak = Rig::probs(&rig.statics.clone(), "v_sign", 0.7);
    let none = Rig::probs(&rig.statics.clone(), "none", 1.0);
    let mut fired = Vec::new();
    // Trois secondes de pose tenue. Une fois le geste parti, la main gauche,
    // mal suivie, voit sa confiance fléchir, s'effondrer un instant, puis
    // paraît bouger d'un coup.
    for frame in 0..180 {
        let right = Rig::obs(&steady, None);
        let probs = match frame % 45 {
            20..=24 => &weak,
            30..=32 => &none,
            _ => &steady,
        };
        let mut left = Rig::obs(probs, None);
        left.track = 2;
        left.handedness = Handedness::Left;
        if frame % 45 == 40 {
            left.speed = 6.0;
        }
        fired.extend(fired_bindings(&rig.step(&[right, left])));
    }
    assert_eq!(fired, vec![pair]);

    // Relâchée puis reformée, la pose est un nouveau geste.
    rig.hold("none", 1.0, 900);
    assert_eq!(fired_bindings(&rig.hold_both("v_sign", 600)), vec![pair]);
}

#[test]
fn two_hand_binding_takes_priority_over_single_hand_binding() {
    let (mut rig, pair) = rig_with_pair("v_sign");
    let single = rig.binding_index("v_sign");
    rig.arm();
    // Les deux mains ensemble : seul le binding à deux mains part.
    assert_eq!(fired_bindings(&rig.hold_both("v_sign", 1000)), vec![pair]);
    // Une main se retire : l'autre, déjà consommée, ne déclenche rien.
    assert!(rig.hold("v_sign", 0.97, 1000).is_empty());
    // Une main seule, plus tard : le binding à une main fonctionne toujours.
    rig.hold("none", 1.0, 300);
    assert_eq!(
        fired_bindings(&rig.hold("v_sign", 0.97, 1000)),
        vec![single]
    );
}

#[test]
fn two_hands_with_different_poses_do_not_pair() {
    let (mut rig, pair) = rig_with_pair("open_palm");
    rig.arm();
    let palm = Rig::probs(&rig.statics.clone(), "open_palm", 0.97);
    let none = Rig::probs(&rig.statics.clone(), "none", 1.0);
    for _ in 0..90 {
        let right = Rig::obs(&palm, None);
        let mut left = Rig::obs(&none, None);
        left.track = 2;
        left.handedness = Handedness::Left;
        let events = rig.step(&[right, left]);
        assert!(!fired_bindings(&events).contains(&pair));
    }
}

#[test]
fn a_pose_freed_from_its_role_takes_ordinary_bindings() {
    let mut config =
        mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("d")).unwrap();
    // L'armement passe au rock, le contrôle du volume est désactivé : la
    // paume ouverte et le pincement redeviennent des poses ordinaires.
    config.activation.wake_gesture = "rock".into();
    config.bindings.retain(|b| b.gesture != "rock");
    let volume = config
        .continuous
        .iter()
        .position(|c| c.enter_gesture == "pinch")
        .unwrap();
    config.continuous[volume].enabled = false;
    for gesture in ["open_palm", "pinch"] {
        let mut binding = config.bindings[0].clone();
        binding.gesture = gesture.into();
        binding.hand = HandFilter::Any;
        binding.repeat_ms = None;
        config.bindings.push(binding);
    }
    let mut rig = Rig::new(config);
    assert!(rig.hold("open_palm", 0.97, 900).is_empty());
    assert_eq!(rig.engine.state(), EngineState::Locked);
    rig.hold("rock", 0.97, 700);
    assert_eq!(rig.engine.state(), EngineState::Armed);
    rig.hold("none", 1.0, 150);

    let events = rig.hold("open_palm", 0.97, 600);
    assert_eq!(fired(&events), vec![("open_palm".to_string(), false)]);
    rig.hold("none", 1.0, 150);
    let events = rig.hold("pinch", 0.97, 600);
    assert_eq!(fired(&events), vec![("pinch".to_string(), false)]);
    assert_eq!(rig.engine.state(), EngineState::Armed);
}

#[test]
fn hand_filter_is_respected() {
    let mut config =
        mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("d")).unwrap();
    let i = config
        .bindings
        .iter()
        .position(|b| b.gesture == "v_sign")
        .unwrap();
    config.bindings[i].hand = HandFilter::Left;
    let mut rig = Rig::new(config);
    rig.arm();
    assert!(rig.hold("v_sign", 0.99, 1000).is_empty());
    rig.hold("none", 1.0, 200);
    let events = rig.hold_with("v_sign", 0.99, 500, |o| o.handedness = Handedness::Left);
    assert_eq!(fired(&events).len(), 1);
}

#[test]
fn pose_progress_reports_hold_fraction() {
    let mut rig = Rig::default();
    rig.hold("open_palm", 0.95, 300);
    let p = rig
        .engine
        .pose_progress(1, Handedness::Right, rig.now)
        .unwrap();
    assert_eq!(rig.statics[p.label], "open_palm");
    assert!(!p.confirmed);
    assert!((p.progress - 0.5).abs() < 0.1, "{}", p.progress);
    rig.hold("open_palm", 0.95, 400);
    assert!(
        rig.engine
            .pose_progress(1, Handedness::Right, rig.now)
            .unwrap()
            .confirmed
    );
}

// ---------------------------------------------------------------------------
// Tests de propriétés (§7.7) : aucune action en Verrouillé, cooldown toujours
// respecté, aucune transition sans événement déclencheur.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Step {
    dt_us: u64,
    hand: Option<StepHand>,
}

#[derive(Debug, Clone)]
struct StepHand {
    track: u32,
    right: bool,
    static_label: usize,
    static_p: f32,
    dynamic: Option<(usize, f32)>,
    speed: f32,
    y: f32,
    pinch: f32,
}

fn step_strategy() -> impl Strategy<Value = Step> {
    let hand = (
        1u32..3,
        any::<bool>(),
        0usize..10,
        prop_oneof![Just(0.99f32), Just(0.9), 0.0f32..1.0],
        proptest::option::of((0usize..5, prop_oneof![Just(0.95f32), 0.0f32..1.0])),
        prop_oneof![4 => Just(0.0f32), 1 => 0.0f32..10.0],
        0.0f32..1.0,
        0.0f32..1.0,
    )
        .prop_map(
            |(track, right, static_label, static_p, dynamic, speed, y, pinch)| StepHand {
                track,
                right,
                static_label,
                static_p,
                dynamic,
                speed,
                y,
                pinch,
            },
        );
    (
        prop_oneof![4 => Just(FRAME_US), 1 => 1_000u64..400_000],
        proptest::option::weighted(0.9, hand),
    )
        .prop_map(|(dt_us, hand)| Step { dt_us, hand })
}

/// Répète chaque pas plusieurs trames pour que les maintiens aient une chance d'aboutir.
fn run(steps: &[(Step, u8)], mut check: impl FnMut(u64, EngineState, &EngineEvent, &Rig)) {
    let mut rig = Rig::default();
    for (step, repeat) in steps {
        for _ in 0..*repeat {
            rig.now += step.dt_us;
            let mut events = Vec::new();
            let before = rig.engine.state();
            match &step.hand {
                Some(h) => {
                    let mut statics = vec![0.0; 10];
                    statics[h.static_label] = h.static_p;
                    if h.static_label != 0 {
                        statics[0] = 1.0 - h.static_p;
                    }
                    let dynamics = h.dynamic.map(|(label, p)| {
                        let mut d = vec![0.0; 5];
                        d[label] = p;
                        d
                    });
                    let obs = HandObs {
                        track: h.track,
                        handedness: if h.right {
                            Handedness::Right
                        } else {
                            Handedness::Left
                        },
                        static_probs: &statics,
                        dynamic_probs: dynamics.as_deref(),
                        speed: h.speed,
                        center: [0.5, h.y],
                        index_tip: [0.5, h.y],
                        palm: [0.08, 0.14],
                        pinch: h.pinch,
                    };
                    rig.engine.update(rig.now, &[obs], &mut events);
                }
                None => rig.engine.update(rig.now, &[], &mut events),
            }
            let mut state = before;
            for e in &events {
                check(rig.now, state, e, &rig);
                if let EngineEvent::StateChanged { to, .. } = e {
                    state = *to;
                }
            }
            assert_eq!(
                state,
                rig.engine.state(),
                "les StateChanged ne reflètent pas l'état réel"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn no_action_while_locked(steps in proptest::collection::vec((step_strategy(), 1u8..45), 1..60)) {
        run(&steps, |_, state, event, _| {
            match event {
                EngineEvent::GestureFired { .. } => {
                    assert_eq!(state, EngineState::Armed, "{event:?} émis hors de l'état Armé");
                }
                EngineEvent::ContinuousStarted { .. } | EngineEvent::ContinuousUpdate { .. } => {
                    assert_eq!(state, EngineState::Continuous, "{event:?} émis hors de l'état Continu");
                }
                _ => {}
            }
        });
    }

    #[test]
    fn cooldown_is_always_respected(steps in proptest::collection::vec((step_strategy(), 1u8..45), 1..60)) {
        let mut last: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
        run(&steps, |now, _, event, rig| {
            if let EngineEvent::GestureFired { binding, repeat: false, .. } = event {
                let cooldown = rig.config.bindings[*binding].cooldown_ms.unwrap_or(rig.config.defaults.cooldown_ms) * 1000;
                if let Some(prev) = last.insert(*binding, now) {
                    assert!(now - prev >= cooldown, "binding {binding} redéclenché après {} µs", now - prev);
                }
            }
        });
    }

    #[test]
    fn transitions_are_chained_and_triggered(steps in proptest::collection::vec((step_strategy(), 1u8..45), 1..60)) {
        run(&steps, |_, state, event, _| {
            if let EngineEvent::StateChanged { from, to, reason } = event {
                assert_eq!(*from, state, "transition depuis un état qui n'est pas l'état courant");
                assert_ne!(from, to);
                let legal = matches!(
                    (from, to, reason),
                    (EngineState::Locked, EngineState::Armed, Reason::WakeGesture)
                        | (EngineState::Armed, EngineState::Locked, Reason::LockGesture | Reason::ArmedTimeout)
                        | (EngineState::Armed, EngineState::Continuous, Reason::ContinuousEnter)
                        | (EngineState::Continuous, EngineState::Armed, Reason::PoseReleased | Reason::HandLost)
                );
                assert!(legal, "transition illégale {event:?}");
            }
        });
    }

    #[test]
    fn none_only_input_never_leaves_locked(dts in proptest::collection::vec(1_000u64..500_000, 1..400)) {
        let mut rig = Rig::default();
        let statics = Rig::probs(&rig.statics.clone(), "none", 1.0);
        let dynamics = Rig::probs(&rig.dynamics.clone(), "none", 1.0);
        for dt in dts {
            rig.now += dt;
            let mut events = Vec::new();
            rig.engine.update(rig.now, &[Rig::obs(&statics, Some(&dynamics))], &mut events);
            prop_assert!(events.is_empty());
            prop_assert_eq!(rig.engine.state(), EngineState::Locked);
        }
    }
}
