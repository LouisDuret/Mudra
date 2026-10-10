//! Moteur de gestes (§7.7, EF-30 à EF-34) : machine à états déterministe, sans
//! entrées/sorties, à horloge injectée (D-12).
//!
//! États : Pause → Verrouillé → Armé ⇄ Continu (figure 2 du cahier des charges).

use mudra_config::{ActivationMode, Config, Control, HandFilter};

use crate::one_euro::OneEuro;
use crate::{Handedness, NONE_LABEL, argmax};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineState {
    /// Caméra libérée, rien n'est reconnu.
    Paused,
    /// Seul le geste de réveil est reconnu.
    Locked,
    /// Tous les bindings sont actifs.
    Armed,
    /// Un contrôle continu est piloté par la main.
    Continuous,
}

impl EngineState {
    pub fn label_fr(self) -> &'static str {
        match self {
            EngineState::Paused => "Pause",
            EngineState::Locked => "Verrouillé",
            EngineState::Armed => "Armé",
            EngineState::Continuous => "Continu",
        }
    }
}

/// Événement déclencheur d'une transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Pause,
    Resume,
    WakeGesture,
    LockGesture,
    LockCommand,
    ArmedTimeout,
    ContinuousEnter,
    PoseReleased,
    HandLost,
    ConfigReload,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContinuousValue {
    /// Variation de volume depuis l'entrée dans le mode, dans [-1, 1].
    VolumeDelta(f32),
    /// Position du curseur dans la zone active, chaque axe dans [0, 1].
    MouseMove { x: f32, y: f32 },
    /// Bouton gauche enfoncé / relâché (pincement, EF-51).
    MouseButton { down: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    StateChanged {
        from: EngineState,
        to: EngineState,
        reason: Reason,
    },
    /// Un binding est déclenché : `binding` indexe `config.bindings`.
    GestureFired {
        binding: usize,
        gesture: String,
        confidence: f32,
        hand: Handedness,
        /// Répétition d'une pose maintenue (`repeat_ms`).
        repeat: bool,
    },
    /// `control` indexe `config.continuous`.
    ContinuousStarted {
        control: usize,
    },
    ContinuousUpdate {
        control: usize,
        value: ContinuousValue,
    },
    ContinuousEnded {
        control: usize,
    },
    /// La fenêtre dynamique de cette piste doit être vidée (EF-32).
    FlushDynamicWindow {
        track: u32,
    },
}

/// Observation d'une main pour une trame.
#[derive(Debug, Clone, Copy)]
pub struct HandObs<'a> {
    pub track: u32,
    pub handedness: Handedness,
    /// Distribution du classifieur statique, alignée sur ses labels.
    pub static_probs: &'a [f32],
    /// Distribution du classifieur dynamique, seulement aux trames évaluées.
    pub dynamic_probs: Option<&'a [f32]>,
    /// Vitesse du centre de la main, en tailles de paume par seconde.
    pub speed: f32,
    /// Centre de la main, normalisé (x / largeur, y / hauteur), image affichée.
    pub center: [f32; 2],
    /// Bout de l'index, même repère.
    pub index_tip: [f32; 2],
    /// Taille de paume en fraction de la largeur et de la hauteur de l'image.
    pub palm: [f32; 2],
    /// Écart pouce-index divisé par la taille de paume.
    pub pinch: f32,
}

/// Avancement de la pose en cours sur une piste (pour l'aperçu).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseProgress {
    /// Indice du label statique.
    pub label: usize,
    /// Fraction du maintien requis, dans [0, 1].
    pub progress: f32,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy)]
struct PoseEpisode {
    label: usize,
    start_us: u64,
    below_since: Option<u64>,
    fired: bool,
    last_fire_us: u64,
    /// Consommé par un geste à deux mains : ne déclenche plus rien seul.
    paired: bool,
}

/// Même pose tenue par les deux mains, liée par un binding `hand = "both"` (EF-24).
#[derive(Debug, Clone, Copy)]
struct PairEpisode {
    slot: usize,
    start_us: u64,
    fired: bool,
    last_fire_us: u64,
}

#[derive(Debug, Clone, Copy)]
struct TrackState {
    track: u32,
    pose: Option<PoseEpisode>,
    dynamic_candidate: Option<(usize, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureKind {
    Static(usize),
    Dynamic(usize),
}

#[derive(Debug, Clone)]
struct ResolvedBinding {
    index: usize,
    gesture: String,
    kind: GestureKind,
    hand: HandFilter,
    hold_us: u64,
    cooldown_us: u64,
    repeat_us: Option<u64>,
    enter: f32,
    exit: f32,
    dynamic_threshold: f32,
}

#[derive(Debug, Clone)]
struct ResolvedContinuous {
    index: usize,
    label: usize,
    hand: HandFilter,
    hold_us: u64,
    control: Control,
    click_label: Option<usize>,
}

#[derive(Debug, Clone)]
struct Resolved {
    mode: ActivationMode,
    wake: Option<usize>,
    lock: Option<usize>,
    wake_hold_us: u64,
    armed_duration_us: u64,
    enter: f32,
    exit: f32,
    hold_us: u64,
    release_us: u64,
    max_pose_speed: f32,
    refractory_us: u64,
    suppress_us: u64,
    continuous_release_us: u64,
    hand_lost_us: u64,
    static_none: Option<usize>,
    dynamic_none: Option<usize>,
    static_len: usize,
    dynamic_len: usize,
    bindings: Vec<ResolvedBinding>,
    continuous: Vec<ResolvedContinuous>,
}

impl Resolved {
    fn new(config: &Config, static_labels: &[String], dynamic_labels: &[String]) -> Self {
        let find = |labels: &[String], name: &str| labels.iter().position(|l| l == name);
        let d = &config.defaults;
        let a = &config.activation;
        let wake_mode = a.mode == ActivationMode::WakeGesture;
        let bindings = config
            .bindings
            .iter()
            .enumerate()
            .filter_map(|(index, b)| {
                // Le geste de réveil et le geste de verrouillage sont réservés.
                // À deux mains, ils restent utilisables (deux paumes ouvertes, EF-24).
                if wake_mode
                    && b.hand != HandFilter::Both
                    && (b.gesture == a.wake_gesture || b.gesture == a.lock_gesture)
                {
                    return None;
                }
                if b.gesture == NONE_LABEL || !b.enabled {
                    return None;
                }
                let kind = find(static_labels, &b.gesture)
                    .map(GestureKind::Static)
                    .or_else(|| find(dynamic_labels, &b.gesture).map(GestureKind::Dynamic))?;
                Some(ResolvedBinding {
                    index,
                    gesture: b.gesture.clone(),
                    kind,
                    hand: b.hand,
                    hold_us: b.hold_ms.unwrap_or(d.hold_ms) * 1000,
                    cooldown_us: b.cooldown_ms.unwrap_or(d.cooldown_ms) * 1000,
                    repeat_us: b.repeat_ms.map(|r| r * 1000),
                    enter: b.static_enter.unwrap_or(d.static_enter),
                    exit: b.static_exit.unwrap_or(d.static_exit),
                    dynamic_threshold: b.dynamic_threshold.unwrap_or(d.dynamic_threshold),
                })
            })
            .collect();
        let continuous = config
            .continuous
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                let click_label = match &c.control {
                    Control::Mouse {
                        click: Some(click), ..
                    } => find(static_labels, click),
                    _ => None,
                };
                Some(ResolvedContinuous {
                    index,
                    label: find(static_labels, &c.enter_gesture)?,
                    hand: c.hand,
                    hold_us: c.enter_hold_ms * 1000,
                    control: c.control.clone(),
                    click_label,
                })
            })
            .collect();
        Self {
            mode: a.mode,
            wake: find(static_labels, &a.wake_gesture),
            lock: find(static_labels, &a.lock_gesture),
            wake_hold_us: a.wake_hold_ms * 1000,
            armed_duration_us: a.armed_duration_ms * 1000,
            enter: d.static_enter,
            exit: d.static_exit,
            hold_us: d.hold_ms * 1000,
            release_us: d.release_ms * 1000,
            max_pose_speed: d.max_pose_speed,
            refractory_us: d.dynamic_refractory_ms * 1000,
            suppress_us: d.dynamic_suppress_ms * 1000,
            continuous_release_us: d.continuous_release_ms * 1000,
            hand_lost_us: d.hand_lost_ms * 1000,
            static_none: find(static_labels, NONE_LABEL),
            dynamic_none: find(dynamic_labels, NONE_LABEL),
            static_len: static_labels.len(),
            dynamic_len: dynamic_labels.len(),
            bindings,
            continuous,
        }
    }

    fn static_binding(&self, label: usize, hand: Handedness) -> Option<usize> {
        self.bindings
            .iter()
            .position(|b| b.kind == GestureKind::Static(label) && hand.matches(b.hand))
    }

    /// Binding à deux mains (`hand = "both"`) d'une pose statique.
    fn pair_binding(&self, label: usize) -> Option<usize> {
        self.bindings
            .iter()
            .position(|b| b.kind == GestureKind::Static(label) && b.hand == HandFilter::Both)
    }

    fn dynamic_binding(&self, label: usize, hand: Handedness) -> Option<usize> {
        self.bindings
            .iter()
            .position(|b| b.kind == GestureKind::Dynamic(label) && hand.matches(b.hand))
    }

    fn continuous_for(&self, label: usize, hand: Handedness) -> Option<usize> {
        self.continuous
            .iter()
            .position(|c| c.label == label && hand.matches(c.hand))
    }

    /// Seuils (entrée, sortie) d'une pose : ceux de son binding s'il en a un.
    fn thresholds(&self, label: usize, hand: Handedness) -> (f32, f32) {
        match self.static_binding(label, hand) {
            Some(b) => (self.bindings[b].enter, self.bindings[b].exit),
            None => (self.enter, self.exit),
        }
    }
}

#[derive(Debug, Clone)]
struct ActiveContinuous {
    /// Position dans `Resolved::continuous`.
    slot: usize,
    track: u32,
    entry_center: [f32; 2],
    release_since: Option<u64>,
    lost_since: Option<u64>,
    button_down: bool,
    last_volume: f32,
    filter_x: OneEuro,
    filter_y: OneEuro,
    last_us: u64,
}

/// Seuils de pincement du clic souris (EF-51), en fractions de la taille de paume.
const CLICK_CLOSE: f32 = 0.25;
const CLICK_OPEN: f32 = 0.35;

pub struct Engine {
    cfg: Resolved,
    state: EngineState,
    tracks: Vec<TrackState>,
    armed_deadline_us: u64,
    last_fire_us: Vec<Option<u64>>,
    refractory_until_us: u64,
    suppress_until_us: u64,
    continuous: Option<ActiveContinuous>,
    pair: Option<PairEpisode>,
}

impl Engine {
    /// Crée un moteur à l'état de repos : Verrouillé, ou Armé en mode `always_on`.
    pub fn new(config: &Config, static_labels: &[String], dynamic_labels: &[String]) -> Self {
        let cfg = Resolved::new(config, static_labels, dynamic_labels);
        let state = Self::rest_state(&cfg);
        Self {
            last_fire_us: vec![None; cfg.bindings.len()],
            cfg,
            state,
            tracks: Vec::with_capacity(4),
            armed_deadline_us: u64::MAX,
            refractory_until_us: 0,
            suppress_until_us: 0,
            continuous: None,
            pair: None,
        }
    }

    fn rest_state(cfg: &Resolved) -> EngineState {
        match cfg.mode {
            ActivationMode::WakeGesture => EngineState::Locked,
            ActivationMode::AlwaysOn => EngineState::Armed,
        }
    }

    pub fn state(&self) -> EngineState {
        self.state
    }

    /// Temps restant avant le retour à Verrouillé, si l'état est Armé avec délai.
    pub fn armed_remaining_us(&self, now_us: u64) -> Option<u64> {
        (self.state == EngineState::Armed && self.cfg.mode == ActivationMode::WakeGesture)
            .then(|| self.armed_deadline_us.saturating_sub(now_us))
    }

    /// Indice (dans `config.continuous`) du contrôle continu actif.
    pub fn active_continuous(&self) -> Option<usize> {
        self.continuous
            .as_ref()
            .map(|c| self.cfg.continuous[c.slot].index)
    }

    /// Avancement de la pose en cours sur une piste.
    pub fn pose_progress(
        &self,
        track: u32,
        handedness: Handedness,
        now_us: u64,
    ) -> Option<PoseProgress> {
        let ep = self.tracks.iter().find(|t| t.track == track)?.pose?;
        let required = self
            .required_hold(ep.label, handedness)
            .unwrap_or(self.cfg.hold_us)
            .max(1);
        let held = now_us.saturating_sub(ep.start_us);
        Some(PoseProgress {
            label: ep.label,
            progress: if ep.fired {
                1.0
            } else {
                (held as f32 / required as f32).min(1.0)
            },
            confirmed: ep.fired,
        })
    }

    /// Maintien requis pour le consommateur de cette pose dans l'état courant.
    fn required_hold(&self, label: usize, hand: Handedness) -> Option<u64> {
        let wake_mode = self.cfg.mode == ActivationMode::WakeGesture;
        match self.state {
            EngineState::Locked => (Some(label) == self.cfg.wake).then_some(self.cfg.wake_hold_us),
            EngineState::Armed => {
                if wake_mode && Some(label) == self.cfg.lock {
                    Some(self.cfg.hold_us)
                } else if let Some(c) = self.cfg.continuous_for(label, hand) {
                    Some(self.cfg.continuous[c].hold_us)
                } else {
                    self.cfg
                        .static_binding(label, hand)
                        .map(|b| self.cfg.bindings[b].hold_us)
                }
            }
            _ => None,
        }
    }

    /// Applique une nouvelle configuration (rechargement à chaud).
    pub fn set_config(
        &mut self,
        config: &Config,
        static_labels: &[String],
        dynamic_labels: &[String],
        events: &mut Vec<EngineEvent>,
    ) {
        self.end_continuous(events);
        self.cfg = Resolved::new(config, static_labels, dynamic_labels);
        self.last_fire_us = vec![None; self.cfg.bindings.len()];
        self.tracks.clear();
        self.pair = None;
        if self.state != EngineState::Paused {
            let rest = Self::rest_state(&self.cfg);
            self.transition(rest, Reason::ConfigReload, events);
        }
    }

    /// Met en pause (raccourci global, menu) depuis n'importe quel état.
    pub fn pause(&mut self, events: &mut Vec<EngineEvent>) {
        self.end_continuous(events);
        self.tracks.clear();
        self.transition(EngineState::Paused, Reason::Pause, events);
    }

    /// Reprend après une pause.
    pub fn resume(&mut self, events: &mut Vec<EngineEvent>) {
        if self.state == EngineState::Paused {
            self.tracks.clear();
            let rest = Self::rest_state(&self.cfg);
            self.transition(rest, Reason::Resume, events);
        }
    }

    /// Verrouille immédiatement (action interne `lock`).
    pub fn lock(&mut self, events: &mut Vec<EngineEvent>) {
        if matches!(self.state, EngineState::Armed | EngineState::Continuous)
            && self.cfg.mode == ActivationMode::WakeGesture
        {
            self.end_continuous(events);
            self.transition(EngineState::Locked, Reason::LockCommand, events);
        }
    }

    fn transition(&mut self, to: EngineState, reason: Reason, events: &mut Vec<EngineEvent>) {
        if self.state == to {
            return;
        }
        events.push(EngineEvent::StateChanged {
            from: self.state,
            to,
            reason,
        });
        self.state = to;
        if to != EngineState::Armed {
            self.armed_deadline_us = u64::MAX;
        }
    }

    fn end_continuous(&mut self, events: &mut Vec<EngineEvent>) {
        if let Some(c) = self.continuous.take() {
            let control = self.cfg.continuous[c.slot].index;
            if c.button_down {
                events.push(EngineEvent::ContinuousUpdate {
                    control,
                    value: ContinuousValue::MouseButton { down: false },
                });
            }
            events.push(EngineEvent::ContinuousEnded { control });
        }
    }

    /// Traite une trame : `hands` contient les mains suivies à l'instant `now_us`.
    pub fn update(&mut self, now_us: u64, hands: &[HandObs<'_>], events: &mut Vec<EngineEvent>) {
        if self.state == EngineState::Paused {
            return;
        }
        self.tracks
            .retain(|t| hands.iter().any(|h| h.track == t.track));
        for h in hands {
            if !self.tracks.iter().any(|t| t.track == h.track) {
                self.tracks.push(TrackState {
                    track: h.track,
                    pose: None,
                    dynamic_candidate: None,
                });
            }
        }

        if self.state == EngineState::Continuous {
            self.update_continuous(now_us, hands, events);
            return;
        }

        for h in hands {
            if h.static_probs.len() == self.cfg.static_len {
                self.update_pose(now_us, h, events);
            }
            if self.state == EngineState::Continuous {
                break;
            }
            if self.state == EngineState::Armed {
                self.update_dynamic(now_us, h, events);
            }
        }

        if self.state == EngineState::Armed {
            self.update_pair(now_us, hands, events);
        } else {
            self.pair = None;
        }

        if self.state == EngineState::Armed
            && self.cfg.mode == ActivationMode::WakeGesture
            && now_us >= self.armed_deadline_us
        {
            self.transition(EngineState::Locked, Reason::ArmedTimeout, events);
        }
    }

    fn cooldown_ok(&self, slot: usize, now_us: u64) -> bool {
        match self.last_fire_us[slot] {
            Some(last) => now_us.saturating_sub(last) >= self.cfg.bindings[slot].cooldown_us,
            None => true,
        }
    }

    fn fire(
        &mut self,
        slot: usize,
        now_us: u64,
        confidence: f32,
        hand: Handedness,
        repeat: bool,
        events: &mut Vec<EngineEvent>,
    ) {
        let b = &self.cfg.bindings[slot];
        events.push(EngineEvent::GestureFired {
            binding: b.index,
            gesture: b.gesture.clone(),
            confidence,
            hand,
            repeat,
        });
        if !repeat {
            self.last_fire_us[slot] = Some(now_us);
        }
        // Le délai d'armement est relancé à chaque action (EF-30).
        self.armed_deadline_us = now_us + self.cfg.armed_duration_us;
    }

    /// Geste à deux mains : les deux mains tiennent la même pose, liée par un
    /// binding `hand = "both"`. Maintien, cooldown et répétition comme à une main.
    fn update_pair(&mut self, now_us: u64, hands: &[HandObs<'_>], events: &mut Vec<EngineEvent>) {
        let common = match hands {
            [a, b] => {
                let pose = |h: &HandObs<'_>| {
                    self.tracks
                        .iter()
                        .find(|t| t.track == h.track)
                        .and_then(|t| t.pose)
                        .filter(|e| e.below_since.is_none())
                };
                match (pose(a), pose(b)) {
                    (Some(x), Some(y)) if x.label == y.label => {
                        let p =
                            |h: &HandObs<'_>| h.static_probs.get(x.label).copied().unwrap_or(0.0);
                        self.cfg
                            .pair_binding(x.label)
                            .map(|slot| (slot, p(a).min(p(b)), a.handedness))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let Some((slot, confidence, hand)) = common else {
            self.pair = None;
            return;
        };
        let mut pair = match self.pair {
            Some(p) if p.slot == slot => p,
            _ => PairEpisode {
                slot,
                start_us: now_us,
                fired: false,
                last_fire_us: 0,
            },
        };
        let (hold_us, repeat_us) = {
            let b = &self.cfg.bindings[slot];
            (b.hold_us, b.repeat_us)
        };
        if !pair.fired {
            if now_us - pair.start_us >= hold_us && self.cooldown_ok(slot, now_us) {
                pair.fired = true;
                pair.last_fire_us = now_us;
                for t in &mut self.tracks {
                    if let Some(e) = &mut t.pose {
                        e.paired = true;
                    }
                }
                self.fire(slot, now_us, confidence, hand, false, events);
            }
        } else if let Some(repeat) = repeat_us
            && now_us - pair.last_fire_us >= repeat
        {
            pair.last_fire_us = now_us;
            self.fire(slot, now_us, confidence, hand, true, events);
        }
        self.pair = Some(pair);
    }

    fn update_pose(&mut self, now_us: u64, h: &HandObs<'_>, events: &mut Vec<EngineEvent>) {
        let Some(ti) = self.tracks.iter().position(|t| t.track == h.track) else {
            return;
        };
        let probs = h.static_probs;
        let top = argmax(probs);
        let moving = h.speed > self.cfg.max_pose_speed;
        let blocked = moving || now_us < self.suppress_until_us;
        let top_is_pose = Some(top) != self.cfg.static_none
            && probs[top] >= self.cfg.thresholds(top, h.handedness).0;

        // 1. Cycle de vie de l'épisode : maintien strict avant confirmation,
        //    hystérésis ensuite (EF-31).
        let mut episode = self.tracks[ti].pose;
        if let Some(e) = &mut episode {
            let p = probs[e.label];
            let (enter, exit) = self.cfg.thresholds(e.label, h.handedness);
            let ended = if !e.fired {
                p < enter || blocked
            } else if top != e.label && top_is_pose {
                // Une autre pose est reconnue avec confiance : enchaînement direct.
                true
            } else if p < exit {
                let since = *e.below_since.get_or_insert(now_us);
                now_us - since >= self.cfg.release_us
            } else {
                e.below_since = None;
                false
            };
            if ended {
                episode = None;
            }
        }
        if episode.is_none() && top_is_pose && !blocked {
            episode = Some(PoseEpisode {
                label: top,
                start_us: now_us,
                below_since: None,
                fired: false,
                last_fire_us: 0,
                paired: false,
            });
        }

        // 2. Consommateur de la pose selon l'état.
        // L'autre main tient la même pose et un binding à deux mains existe :
        // il a priorité sur ce que la pose ferait à une main (EF-24).
        let pairing = episode.is_some_and(|e| {
            e.paired
                || (self.cfg.pair_binding(e.label).is_some()
                    && self
                        .tracks
                        .iter()
                        .any(|t| t.track != h.track && t.pose.is_some_and(|p| p.label == e.label)))
        });
        if let Some(e) = &mut episode {
            let held = now_us - e.start_us;
            let confidence = probs[e.label];
            let wake_mode = self.cfg.mode == ActivationMode::WakeGesture;
            match self.state {
                EngineState::Locked => {
                    if Some(e.label) == self.cfg.wake && !e.fired && held >= self.cfg.wake_hold_us {
                        e.fired = true;
                        e.last_fire_us = now_us;
                        self.transition(EngineState::Armed, Reason::WakeGesture, events);
                        self.armed_deadline_us = now_us + self.cfg.armed_duration_us;
                    }
                }
                EngineState::Armed => {
                    if pairing {
                        // Laissé à `update_pair`.
                    } else if wake_mode && Some(e.label) == self.cfg.lock {
                        if !e.fired && held >= self.cfg.hold_us {
                            e.fired = true;
                            self.transition(EngineState::Locked, Reason::LockGesture, events);
                        }
                    } else if wake_mode && Some(e.label) == self.cfg.wake {
                        // Réservé : la paume ouverte ne déclenche rien une fois armé.
                    } else if let Some(slot) = self.cfg.continuous_for(e.label, h.handedness) {
                        if !e.fired && held >= self.cfg.continuous[slot].hold_us {
                            e.fired = true;
                            self.enter_continuous(slot, now_us, h, events);
                        }
                    } else if let Some(slot) = self.cfg.static_binding(e.label, h.handedness) {
                        let b = &self.cfg.bindings[slot];
                        if !e.fired {
                            if held >= b.hold_us && self.cooldown_ok(slot, now_us) {
                                e.fired = true;
                                e.last_fire_us = now_us;
                                self.fire(slot, now_us, confidence, h.handedness, false, events);
                            }
                        } else if let Some(repeat) = b.repeat_us
                            && e.below_since.is_none()
                            && now_us - e.last_fire_us >= repeat
                        {
                            e.last_fire_us = now_us;
                            self.fire(slot, now_us, confidence, h.handedness, true, events);
                        }
                    }
                }
                EngineState::Paused | EngineState::Continuous => {}
            }
        }
        self.tracks[ti].pose = episode;
    }

    fn update_dynamic(&mut self, now_us: u64, h: &HandObs<'_>, events: &mut Vec<EngineEvent>) {
        let Some(probs) = h.dynamic_probs else {
            return;
        };
        let Some(ti) = self.tracks.iter().position(|t| t.track == h.track) else {
            return;
        };
        if probs.len() != self.cfg.dynamic_len || now_us < self.refractory_until_us {
            self.tracks[ti].dynamic_candidate = None;
            return;
        }
        let top = argmax(probs);
        let slot = (Some(top) != self.cfg.dynamic_none)
            .then(|| self.cfg.dynamic_binding(top, h.handedness))
            .flatten()
            .filter(|&slot| probs[top] >= self.cfg.bindings[slot].dynamic_threshold);
        let Some(slot) = slot else {
            self.tracks[ti].dynamic_candidate = None;
            return;
        };
        let count = match self.tracks[ti].dynamic_candidate {
            Some((label, n)) if label == top => n.saturating_add(1),
            _ => 1,
        };
        // Deux évaluations consécutives au-dessus du seuil (EF-32).
        if count >= 2 && self.cooldown_ok(slot, now_us) {
            self.fire(slot, now_us, probs[top], h.handedness, false, events);
            self.refractory_until_us = now_us + self.cfg.refractory_us;
            // Arbitrage : les poses statiques sont suspendues (EF-33).
            self.suppress_until_us = now_us + self.cfg.suppress_us;
            events.push(EngineEvent::FlushDynamicWindow { track: h.track });
            self.tracks[ti].dynamic_candidate = None;
            if self.tracks[ti].pose.is_some_and(|e| !e.fired) {
                self.tracks[ti].pose = None;
            }
        } else {
            self.tracks[ti].dynamic_candidate = Some((top, count));
        }
    }

    fn enter_continuous(
        &mut self,
        slot: usize,
        now_us: u64,
        h: &HandObs<'_>,
        events: &mut Vec<EngineEvent>,
    ) {
        let (min_cutoff, beta) = match &self.cfg.continuous[slot].control {
            Control::Mouse {
                min_cutoff, beta, ..
            } => (*min_cutoff, *beta),
            Control::Volume { .. } => (1.0, 0.0),
        };
        self.continuous = Some(ActiveContinuous {
            slot,
            track: h.track,
            entry_center: h.center,
            release_since: None,
            lost_since: None,
            button_down: false,
            last_volume: 0.0,
            filter_x: OneEuro::new(min_cutoff, beta, 1.0),
            filter_y: OneEuro::new(min_cutoff, beta, 1.0),
            last_us: now_us,
        });
        self.transition(EngineState::Continuous, Reason::ContinuousEnter, events);
        events.push(EngineEvent::ContinuousStarted {
            control: self.cfg.continuous[slot].index,
        });
    }

    fn exit_continuous(&mut self, now_us: u64, reason: Reason, events: &mut Vec<EngineEvent>) {
        self.end_continuous(events);
        for t in &mut self.tracks {
            t.pose = None;
            t.dynamic_candidate = None;
        }
        self.transition(EngineState::Armed, reason, events);
        self.armed_deadline_us = now_us + self.cfg.armed_duration_us;
    }

    fn update_continuous(
        &mut self,
        now_us: u64,
        hands: &[HandObs<'_>],
        events: &mut Vec<EngineEvent>,
    ) {
        for t in &mut self.tracks {
            t.pose = None;
            t.dynamic_candidate = None;
        }
        let Some(mut active) = self.continuous.take() else {
            self.exit_continuous(now_us, Reason::HandLost, events);
            return;
        };
        let spec = &self.cfg.continuous[active.slot];
        let control = spec.index;

        // Si la piste a changé d'identifiant (re-détection) et qu'il ne reste
        // qu'une main, c'est la même main.
        let only = match hands {
            [only] => Some(only),
            _ => None,
        };
        let hand = hands.iter().find(|h| h.track == active.track).or(only);
        let Some(h) = hand else {
            let since = *active.lost_since.get_or_insert(now_us);
            self.continuous = Some(active);
            if now_us - since > self.cfg.hand_lost_us {
                self.exit_continuous(now_us, Reason::HandLost, events);
            }
            return;
        };
        active.track = h.track;
        active.lost_since = None;

        // Clic par pincement, avec hystérésis (EF-51).
        let mut click_pose = false;
        if let Control::Mouse { click: Some(_), .. } = &spec.control {
            let down = if active.button_down {
                h.pinch < CLICK_OPEN
            } else {
                h.pinch < CLICK_CLOSE
            };
            if down != active.button_down {
                active.button_down = down;
                events.push(EngineEvent::ContinuousUpdate {
                    control,
                    value: ContinuousValue::MouseButton { down },
                });
            }
            click_pose = down
                || spec.click_label.is_some_and(|l| {
                    h.static_probs.get(l).copied().unwrap_or(0.0) >= self.cfg.exit
                });
        }

        let p_enter = h.static_probs.get(spec.label).copied().unwrap_or(0.0);
        if p_enter >= self.cfg.thresholds(spec.label, h.handedness).1 || click_pose {
            active.release_since = None;
        } else {
            let since = *active.release_since.get_or_insert(now_us);
            if now_us - since > self.cfg.continuous_release_us {
                self.continuous = Some(active);
                self.exit_continuous(now_us, Reason::PoseReleased, events);
                return;
            }
        }

        let dt = now_us.saturating_sub(active.last_us) as f32 * 1e-6;
        active.last_us = now_us;
        match &spec.control {
            Control::Volume {
                gain, dead_zone, ..
            } => {
                // y image vers le bas : monter la main augmente le volume.
                let dy = active.entry_center[1] - h.center[1];
                let dy = if dy.abs() <= *dead_zone {
                    0.0
                } else {
                    dy - dead_zone.copysign(dy)
                };
                let value = (gain * dy).clamp(-1.0, 1.0);
                if (value - active.last_volume).abs() >= 0.005 {
                    active.last_volume = value;
                    events.push(EngineEvent::ContinuousUpdate {
                        control,
                        value: ContinuousValue::VolumeDelta(value),
                    });
                }
            }
            Control::Mouse { active_zone, .. } => {
                let [x0, y0, x1, y1] = *active_zone;
                // Filtrage en unités de taille de paume, comme les points-clés.
                let sx = (x1 - x0) / h.palm[0].max(1e-6);
                let sy = (y1 - y0) / h.palm[1].max(1e-6);
                let u = ((h.index_tip[0] - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let v = ((h.index_tip[1] - y0) / (y1 - y0)).clamp(0.0, 1.0);
                let x = active.filter_x.filter(u * sx, dt) / sx;
                let y = active.filter_y.filter(v * sy, dt) / sy;
                events.push(EngineEvent::ContinuousUpdate {
                    control,
                    value: ContinuousValue::MouseMove {
                        x: x.clamp(0.0, 1.0),
                        y: y.clamp(0.0, 1.0),
                    },
                });
            }
        }
        self.continuous = Some(active);
    }
}

#[cfg(test)]
mod tests;
