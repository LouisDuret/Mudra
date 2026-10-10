//! Validation sémantique de la configuration (§7.9). La même fonction sert au
//! chargement, au rechargement à chaud et à `mudra check-config`.

use std::fmt;

use crate::keys::KeyCombo;
use crate::schema::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    /// Champ concerné, par exemple `bindings[2].gesture`.
    pub field: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Warning => "avertissement",
            Level::Error => "erreur",
        };
        write!(f, "{level} · {} : {}", self.field, self.message)
    }
}

/// Gestes connus des classifieurs chargés (D-10 : lus dans les métadonnées des
/// modèles, ou fournis par les classifieurs intégrés).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownGestures {
    pub static_: Vec<String>,
    pub dynamic: Vec<String>,
    /// Poses de `static_` qui ne se font qu'à deux mains.
    pub two_hands: Vec<String>,
}

impl KnownGestures {
    /// Jeu de gestes des classifieurs géométriques intégrés (Annexe B).
    pub fn builtin() -> Self {
        let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        Self {
            static_: owned(&[
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
            ]),
            dynamic: owned(&[
                "none",
                "swipe_left",
                "swipe_right",
                "swipe_up",
                "swipe_down",
            ]),
            two_hands: Vec::new(),
        }
    }

    pub fn is_static(&self, gesture: &str) -> bool {
        gesture != "none" && self.static_.iter().any(|g| g == gesture)
    }

    pub fn is_dynamic(&self, gesture: &str) -> bool {
        gesture != "none" && self.dynamic.iter().any(|g| g == gesture)
    }

    pub fn is_two_hands(&self, gesture: &str) -> bool {
        self.two_hands.iter().any(|g| g == gesture)
    }
}

struct Checker<'a> {
    issues: Vec<Issue>,
    known: &'a KnownGestures,
}

impl Checker<'_> {
    fn error(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue {
            level: Level::Error,
            field: field.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue {
            level: Level::Warning,
            field: field.into(),
            message: message.into(),
        });
    }

    fn range(&mut self, field: &str, value: f32, min: f32, max: f32) {
        if !(value >= min && value <= max) {
            self.error(field, format!("{value} hors bornes [{min}, {max}]"));
        }
    }

    fn static_gesture(&mut self, field: &str, gesture: &str) {
        if !self.known.is_static(gesture) {
            self.error(
                field,
                format!(
                    "pose « {gesture} » inconnue du classifieur statique (connues : {})",
                    self.known.static_.join(", ")
                ),
            );
        }
    }

    fn action(&mut self, field: &str, action: &Action, allow_commands: bool, depth: usize) {
        match action {
            Action::Launch { path, .. } => {
                if path.trim().is_empty() {
                    self.error(format!("{field}.path"), "chemin vide");
                }
            }
            Action::Volume {
                op, delta, value, ..
            } => match op {
                VolumeOp::Step => {
                    if *delta == 0.0 || delta.abs() > 1.0 {
                        self.error(
                            format!("{field}.delta"),
                            "attendu un pas non nul dans [-1, 1]",
                        );
                    }
                }
                VolumeOp::Set => match value {
                    Some(v) if (0.0..=1.0).contains(v) => {}
                    _ => self.error(format!("{field}.value"), "attendu une valeur dans [0, 1]"),
                },
                VolumeOp::MuteToggle => {}
            },
            Action::Keys { combo } => {
                if let Err(e) = KeyCombo::parse(combo) {
                    self.error(format!("{field}.combo"), e.0);
                }
            }
            Action::Screenshot { dir, .. } => {
                if dir.trim().is_empty() {
                    self.error(format!("{field}.dir"), "dossier vide");
                }
            }
            Action::Mouse { op, amount } => {
                if *op == MouseOp::Scroll && *amount == 0 {
                    self.error(format!("{field}.amount"), "défilement nul");
                }
            }
            Action::Command { command, .. } => {
                if command.trim().is_empty() {
                    self.error(format!("{field}.command"), "commande vide");
                }
                if !allow_commands {
                    self.error(
                        field,
                        "l'action « command » exige general.allow_commands = true (ENF-22)",
                    );
                }
            }
            Action::Sequence { steps } => {
                if steps.is_empty() {
                    self.error(format!("{field}.steps"), "séquence vide");
                }
                if depth >= 4 {
                    self.error(field, "séquences imbriquées trop profondément");
                    return;
                }
                for (i, step) in steps.iter().enumerate() {
                    self.action(
                        &format!("{field}.steps[{i}].action"),
                        &step.action,
                        allow_commands,
                        depth + 1,
                    );
                }
            }
            Action::Internal { op, message } => {
                if *op == InternalOp::Notify && message.as_deref().unwrap_or("").is_empty() {
                    self.error(format!("{field}.message"), "message de notification vide");
                }
            }
            Action::Media { .. } | Action::Snip => {}
        }
    }
}

/// Valide la configuration contre les gestes connus des classifieurs.
pub fn validate(config: &Config, known: &KnownGestures) -> Vec<Issue> {
    let mut c = Checker {
        issues: Vec::new(),
        known,
    };

    if !["trace", "debug", "info", "warn", "error"].contains(&config.general.log_level.as_str()) {
        c.error(
            "general.log_level",
            format!("niveau « {} » inconnu", config.general.log_level),
        );
    }

    let cam = &config.camera;
    if !(160..=3840).contains(&cam.width) || !(120..=2160).contains(&cam.height) {
        c.error(
            "camera",
            format!("résolution {}×{} hors bornes", cam.width, cam.height),
        );
    }
    if !(1..=240).contains(&cam.fps) {
        c.error("camera.fps", format!("{} hors bornes [1, 240]", cam.fps));
    }
    if let DeviceSelector::Name(name) = &cam.device
        && name.trim().is_empty()
    {
        c.error("camera.device", "nom de caméra vide");
    }

    let inf = &config.inference;
    if !(1..=2).contains(&inf.max_hands) {
        c.error("inference.max_hands", "attendu 1 ou 2 (EF-10)");
    }
    c.range(
        "inference.palm_score_threshold",
        inf.palm_score_threshold,
        0.05,
        0.99,
    );
    c.range(
        "inference.presence_threshold",
        inf.presence_threshold,
        0.05,
        0.99,
    );
    if inf.redetect_every_n_frames == 0 {
        c.error("inference.redetect_every_n_frames", "doit être ≥ 1");
    }
    if !(1..=60).contains(&inf.idle_detection_hz) {
        c.error(
            "inference.idle_detection_hz",
            "attendu une fréquence dans [1, 60] Hz",
        );
    }
    if matches!(
        inf.execution_provider,
        ExecutionProvider::Cuda | ExecutionProvider::Tensorrt
    ) {
        c.warn(
            "inference.execution_provider",
            "CUDA/TensorRT ne sont pas compilés dans ce prototype : repli sur DirectML puis CPU",
        );
    }

    let s = &config.smoothing;
    c.range("smoothing.min_cutoff", s.min_cutoff, 0.001, 100.0);
    c.range("smoothing.beta", s.beta, 0.0, 100.0);
    c.range("smoothing.d_cutoff", s.d_cutoff, 0.001, 100.0);

    let d = &config.defaults;
    c.range("defaults.static_enter", d.static_enter, 0.0, 1.0);
    c.range("defaults.static_exit", d.static_exit, 0.0, 1.0);
    if d.static_exit > d.static_enter {
        c.error(
            "defaults.static_exit",
            "doit être ≤ static_enter (hystérésis)",
        );
    }
    c.range("defaults.dynamic_threshold", d.dynamic_threshold, 0.0, 1.0);
    c.range("defaults.max_pose_speed", d.max_pose_speed, 0.1, 100.0);

    let a = &config.activation;
    c.static_gesture("activation.wake_gesture", &a.wake_gesture);
    c.static_gesture("activation.lock_gesture", &a.lock_gesture);
    if a.wake_gesture == a.lock_gesture {
        c.error("activation.lock_gesture", "identique au geste de réveil");
    }
    if a.armed_duration_ms < 500 {
        c.error("activation.armed_duration_ms", "doit être ≥ 500 ms");
    }
    match KeyCombo::parse(&a.pause_hotkey) {
        Err(e) => c.error("activation.pause_hotkey", e.0),
        Ok(combo) if combo.keys.iter().all(|k| !k.modifier) => {
            c.warn(
                "activation.pause_hotkey",
                "raccourci global sans modificateur",
            );
        }
        Ok(_) => {}
    }

    for (i, b) in config.bindings.iter().enumerate() {
        let field = format!("bindings[{i}]");
        let is_static = known.is_static(&b.gesture);
        let is_dynamic = known.is_dynamic(&b.gesture);
        if !is_static && !is_dynamic {
            c.error(
                format!("{field}.gesture"),
                format!(
                    "geste « {} » inconnu des modèles (statiques : {} ; dynamiques : {})",
                    b.gesture,
                    known.static_.join(", "),
                    known.dynamic.join(", ")
                ),
            );
        }
        let both = b.hand == HandFilter::Both;
        if both && is_dynamic {
            c.error(
                format!("{field}.hand"),
                "« both » ne s'applique qu'aux poses statiques",
            );
        }
        if both && config.inference.max_hands < 2 {
            c.warn(
                format!("{field}.hand"),
                "jamais déclenché tant que inference.max_hands = 1",
            );
        }
        // À deux mains, le geste de réveil ou de verrouillage reste utilisable.
        if is_static && !both && (b.gesture == a.wake_gesture || b.gesture == a.lock_gesture) {
            c.warn(
                format!("{field}.gesture"),
                format!(
                    "« {} » est déjà le geste de réveil ou de verrouillage : binding ignoré",
                    b.gesture
                ),
            );
        }
        if !both && known.is_two_hands(&b.gesture) {
            c.warn(
                format!("{field}.hand"),
                format!(
                    "« {} » est une pose à deux mains : binding ignoré sans hand = \"both\"",
                    b.gesture
                ),
            );
        }
        if is_dynamic && b.repeat_ms.is_some() {
            c.warn(
                format!("{field}.repeat_ms"),
                "sans effet sur un geste dynamique",
            );
        }
        if let Some(r) = b.repeat_ms
            && r < 50
        {
            c.error(format!("{field}.repeat_ms"), "doit être ≥ 50 ms");
        }
        for (name, v) in [
            ("static_enter", b.static_enter),
            ("static_exit", b.static_exit),
            ("dynamic_threshold", b.dynamic_threshold),
        ] {
            if let Some(v) = v {
                c.range(&format!("{field}.{name}"), v, 0.0, 1.0);
            }
        }
        c.action(
            &format!("{field}.action"),
            &b.action,
            config.general.allow_commands,
            0,
        );
    }

    for (i, k) in config.continuous.iter().enumerate() {
        let field = format!("continuous[{i}]");
        if k.hand == HandFilter::Both {
            c.error(
                format!("{field}.hand"),
                "« both » n'est pas pris en charge pour un contrôle continu",
            );
        }
        c.static_gesture(&format!("{field}.enter_gesture"), &k.enter_gesture);
        if known.is_two_hands(&k.enter_gesture) {
            c.error(
                format!("{field}.enter_gesture"),
                "une pose à deux mains ne peut pas piloter un contrôle continu",
            );
        }
        if config.continuous[..i]
            .iter()
            .any(|o| o.enter_gesture == k.enter_gesture && o.hand == k.hand)
        {
            c.error(
                format!("{field}.enter_gesture"),
                "pose d'entrée déjà utilisée par un autre contrôle continu",
            );
        }
        if config.bindings.iter().any(|b| b.gesture == k.enter_gesture) {
            c.warn(
                format!("{field}.enter_gesture"),
                "un binding utilise la même pose : le contrôle continu est prioritaire",
            );
        }
        match &k.control {
            Control::Volume {
                gain, dead_zone, ..
            } => {
                c.range(&format!("{field}.control.gain"), *gain, 0.05, 20.0);
                c.range(&format!("{field}.control.dead_zone"), *dead_zone, 0.0, 0.5);
            }
            Control::Mouse {
                active_zone,
                click,
                min_cutoff,
                beta,
                stabilization_ms,
                ..
            } => {
                let [x0, y0, x1, y1] = *active_zone;
                let ordered = x0 < x1 && y0 < y1;
                let inside = active_zone.iter().all(|v| (0.0..=1.0).contains(v));
                if !ordered || !inside {
                    c.error(
                        format!("{field}.control.active_zone"),
                        "attendu [x0, y0, x1, y1] croissants dans [0, 1]",
                    );
                }
                if let Some(click) = click {
                    c.static_gesture(&format!("{field}.control.click"), click);
                }
                c.range(
                    &format!("{field}.control.min_cutoff"),
                    *min_cutoff,
                    0.001,
                    100.0,
                );
                c.range(&format!("{field}.control.beta"), *beta, 0.0, 100.0);
                c.range(
                    &format!("{field}.control.stabilization_ms"),
                    *stabilization_ms as f32,
                    0.0,
                    500.0,
                );
            }
        }
    }

    c.issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn check(toml: &str) -> Vec<Issue> {
        let config = crate::parse(toml, Path::new("test.toml")).unwrap();
        validate(&config, &KnownGestures::builtin())
    }

    #[test]
    fn both_hands_only_for_static_bindings() {
        let pair = |gesture: &str| {
            format!(
                "{}\n[[bindings]]\ngesture = \"{gesture}\"\nhand = \"both\"\naction = {{ type = \"media\", key = \"next\" }}\n",
                crate::DEFAULT_CONFIG_TOML
            )
        };
        let on_hand = |toml: &str| check(toml).iter().any(|i| i.field.ends_with(".hand"));
        assert!(!on_hand(&pair("thumbs_up")));
        assert!(on_hand(&pair("swipe_left")));
    }

    #[test]
    fn two_hand_pose_wants_a_two_hand_binding() {
        let mut known = KnownGestures::builtin();
        known.static_.push("coeur".into());
        known.two_hands.push("coeur".into());
        let issues = |toml: &str| {
            let config = crate::parse(toml, Path::new("test.toml")).unwrap();
            validate(&config, &known)
        };
        let binding = |hand: &str| {
            format!(
                "[inference]\nmax_hands = 2\n[[bindings]]\ngesture = \"coeur\"\nhand = \"{hand}\"\naction = {{ type = \"snip\" }}\n"
            )
        };
        let on_hand = |toml: &str| -> Vec<Level> {
            let on_hand = |i: &Issue| i.field == "bindings[0].hand";
            issues(toml)
                .iter()
                .filter(|i| on_hand(i))
                .map(|i| i.level)
                .collect()
        };
        assert!(on_hand(&binding("both")).is_empty());
        assert_eq!(on_hand(&binding("any")), vec![Level::Warning]);
        let continuous = "[[continuous]]\nname = \"v\"\nenter_gesture = \"coeur\"\ncontrol = { type = \"volume\" }\n";
        assert!(has_error(
            &issues(continuous),
            "continuous[0].enter_gesture"
        ));
    }

    fn has_error(issues: &[Issue], field: &str) -> bool {
        issues
            .iter()
            .any(|i| i.level == Level::Error && i.field == field)
    }

    #[test]
    fn unknown_gesture_is_an_explicit_error() {
        let issues = check("[[bindings]]\ngesture = \"moonwalk\"\naction = { type = \"snip\" }\n");
        assert!(has_error(&issues, "bindings[0].gesture"));
        assert!(issues[0].message.contains("moonwalk"));
    }

    #[test]
    fn command_requires_opt_in() {
        let toml = "[[bindings]]\ngesture = \"rock\"\naction = { type = \"command\", command = \"dir\" }\n";
        assert!(has_error(&check(toml), "bindings[0].action"));
        let ok = format!("[general]\nallow_commands = true\n{toml}");
        assert!(!check(&ok).iter().any(|i| i.level == Level::Error));
    }

    #[test]
    fn bad_hotkey_and_bounds() {
        let issues = check(
            "[activation]\npause_hotkey = \"Ctrl+Zorglub\"\n[defaults]\nstatic_enter = 1.5\n[inference]\nmax_hands = 3\n",
        );
        assert!(has_error(&issues, "activation.pause_hotkey"));
        assert!(has_error(&issues, "defaults.static_enter"));
        assert!(has_error(&issues, "inference.max_hands"));
    }

    #[test]
    fn hysteresis_must_be_ordered() {
        let issues = check("[defaults]\nstatic_enter = 0.5\nstatic_exit = 0.7\n");
        assert!(has_error(&issues, "defaults.static_exit"));
    }

    #[test]
    fn sequence_actions_are_checked_recursively() {
        let toml = r#"
[[bindings]]
gesture = "rock"
[bindings.action]
type = "sequence"
steps = [
  { action = { type = "keys", combo = "Win+D" }, delay_ms = 200 },
  { action = { type = "keys", combo = "Nope+Q" } },
]
"#;
        assert!(has_error(
            &check(toml),
            "bindings[0].action.steps[1].action.combo"
        ));
    }

    #[test]
    fn mouse_zone_must_be_ordered() {
        let toml = "[[continuous]]\nname = \"m\"\nenter_gesture = \"pointing\"\n[continuous.control]\ntype = \"mouse\"\nactive_zone = [0.8, 0.2, 0.2, 0.8]\n";
        assert!(has_error(&check(toml), "continuous[0].control.active_zone"));
    }
}
