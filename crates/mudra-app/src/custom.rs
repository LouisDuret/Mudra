//! Poses personnalisées côté application : où elles vivent sur le disque,
//! comment leur identifiant est tiré du nom, et l'apprentissage à partir des
//! prises enregistrées par le pipeline.
//!
//! ```text
//! %APPDATA%\Mudra\gestures\<label>.json                    pose apprise
//! %APPDATA%\Mudra\gestures\dataset\static\<label>\*.jsonl  prises (points-clés)
//! ```
//!
//! Le dossier `dataset` a le format de `mudra record` : il reste utilisable
//! par la chaîne d'entraînement Python.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mudra_config::Config;
use mudra_dataset::{Hand, read_session};
use mudra_gestures::custom::{self, CustomPose};
use mudra_gestures::{HandSample, Handedness};

/// Durée d'une prise, en secondes.
pub const TAKE_SECONDS: f32 = 4.0;
/// Nombre de prises demandées pour une nouvelle pose.
pub const TAKES_FOR_NEW_POSE: usize = 3;

/// Dossier des poses perso. `MUDRA_GESTURES_DIR` le déplace (essais, tests).
pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MUDRA_GESTURES_DIR") {
        return PathBuf::from(dir);
    }
    mudra_config::config_dir()
        .map(|d| d.join("gestures"))
        .unwrap_or_else(|_| PathBuf::from("gestures"))
}

/// Racine au format dataset où le pipeline écrit les prises.
pub fn dataset_root() -> PathBuf {
    dir().join("dataset")
}

fn takes_dir(root: &Path, label: &str) -> PathBuf {
    root.join("dataset").join("static").join(label)
}

/// Identifiant snake_case tiré d'un nom libre : « Téléphone ! » → `telephone`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ñ' => 'n',
            'ô' | 'ö' | 'ó' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' => 'y',
            c if c.is_ascii_lowercase() || c.is_ascii_digit() => c,
            _ => '_',
        };
        if c != '_' || !(out.is_empty() || out.ends_with('_')) {
            out.push(c);
        }
    }
    let mut out = out.trim_end_matches('_').to_string();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "pose_");
    }
    out.truncate(40);
    out.trim_end_matches('_').to_string()
}

/// Identifiant libre pour ce nom : suffixé si un geste le porte déjà.
/// `None` si le nom ne contient ni lettre ni chiffre.
pub fn unique_label(name: &str, taken: &[&str]) -> Option<String> {
    let base = slug(name);
    if base.is_empty() {
        return None;
    }
    (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}_{n}")
            }
        })
        .find(|candidate| !taken.contains(&candidate.as_str()))
}

/// Images d'une prise, au format des classifieurs. Si deux mains sont
/// visibles, seule la plus présente compte. Le tri ne se fait pas sur la
/// latéralité : le suivi en change parfois d'une image à l'autre.
fn read_take(path: &Path) -> Result<Vec<HandSample>, String> {
    let (header, records) = read_session(path).map_err(|e| e.to_string())?;
    let mut presence: HashMap<u32, usize> = HashMap::new();
    for r in &records {
        *presence.entry(r.track).or_default() += 1;
    }
    // Le fichier normalise y par la hauteur ; les classifieurs veulent un
    // repère isotrope, tout divisé par la largeur.
    let aspect = header.height as f32 / header.width.max(1) as f32;
    let mut samples: Vec<HandSample> = Vec::with_capacity(records.len());
    let mut last: Option<(u64, usize)> = None;
    for r in records {
        let weight = presence[&r.track];
        let mut image = r.image;
        for p in image.iter_mut() {
            p[1] *= aspect;
        }
        let sample = HandSample {
            image,
            world: r.world,
            handedness: match r.hand {
                Hand::Left => Handedness::Left,
                Hand::Right => Handedness::Right,
            },
        };
        match last {
            // Deuxième main sur la même image : la plus présente l'emporte.
            Some((t, best)) if t == r.t => {
                if weight > best {
                    *samples.last_mut().expect("image précédente") = sample;
                    last = Some((t, weight));
                }
            }
            _ => {
                samples.push(sample);
                last = Some((r.t, weight));
            }
        }
    }
    Ok(samples)
}

fn take_files(root: &Path, label: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(takes_dir(root, label))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
}

/// Apprend (ou réapprend) la pose `label` à partir de toutes ses prises et
/// l'écrit dans `root`. Le nom et la sensibilité d'une pose existante sont
/// conservés si `name` est `None`.
pub fn train_in(
    root: &Path,
    label: &str,
    name: Option<&str>,
    config: &Config,
) -> Result<CustomPose, String> {
    let files = take_files(root, label);
    let mut takes = Vec::with_capacity(files.len());
    for file in &files {
        takes.push(read_take(file)?);
    }
    let (existing, _) = custom::load_dir(root);
    let previous = existing.iter().find(|p| p.label == label);
    let name = name
        .or(previous.map(|p| p.name.as_str()))
        .unwrap_or(label)
        .to_string();
    let mut pose = CustomPose::train(label, &name, &takes).map_err(|e| e.to_string())?;
    if let Some(previous) = previous {
        pose.sensitivity = previous.sensitivity;
    }
    let samples: Vec<HandSample> = takes.into_iter().flatten().collect();
    let (mut base, _, _) = crate::pipeline::load_base_classifiers(config);
    pose.find_resemblances(
        &samples,
        base.as_mut(),
        &existing,
        config.defaults.static_enter,
    );
    custom::save(root, &pose).map_err(|e| format!("écriture de la pose : {e}"))?;
    Ok(pose)
}

/// Modifie une pose déjà apprise (nom, sensibilité) sans la réapprendre.
pub fn update_in(
    root: &Path,
    label: &str,
    edit: impl FnOnce(&mut CustomPose),
) -> Result<(), String> {
    let (poses, _) = custom::load_dir(root);
    let mut pose = poses
        .into_iter()
        .find(|p| p.label == label)
        .ok_or_else(|| format!("pose « {label} » introuvable"))?;
    edit(&mut pose);
    custom::save(root, &pose).map_err(|e| format!("écriture de la pose : {e}"))
}

/// Supprime une pose et ses prises.
pub fn delete_in(root: &Path, label: &str) -> Result<(), String> {
    custom::remove(root, label).map_err(|e| format!("suppression de la pose : {e}"))?;
    match std::fs::remove_dir_all(takes_dir(root, label)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("suppression des prises : {e}"))
        }
        _ => Ok(()),
    }
}

/// Supprime les prises d'une pose abandonnée avant d'avoir été apprise.
pub fn discard_takes(files: &[PathBuf]) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
    if let Some(dir) = files.first().and_then(|f| f.parent()) {
        // Ne retire le dossier que s'il est vide.
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use mudra_dataset::{FrameRecord, SessionHeader, SessionKind, SessionWriter};
    use mudra_gestures::StaticClassifier;
    use mudra_gestures::builtin::synthetic::{Pose, hand};
    use mudra_gestures::custom::CustomStaticClassifier;

    use super::*;

    const PHONE: Pose = Pose {
        fingers: [1.0, 1.0, 1.0, 0.0],
        thumb: 1.0,
        pinch: false,
        roll_deg: 0.0,
    };

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mudra-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn config() -> Config {
        mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("default.toml")).unwrap()
    }

    /// Écrit une prise comme le ferait le pipeline : image 1280 × 720, y
    /// normalisé par la hauteur.
    fn write_take(root: &Path, label: &str, pose: Pose, roll: f32, frames: usize) -> PathBuf {
        let header = SessionHeader {
            format_version: mudra_dataset::FORMAT_VERSION,
            kind: SessionKind::Static,
            label: label.into(),
            camera: "test".into(),
            width: 1280,
            height: 720,
            fps: 60.0,
            mirror: true,
            created_at: String::new(),
            clips: None,
            clip_seconds: None,
        };
        let mut writer = SessionWriter::create(&root.join("dataset"), header).unwrap();
        for i in 0..frames {
            let wobble = (i as f32 * 0.7).sin();
            let sample = hand(
                Pose {
                    roll_deg: roll + 2.0 * wobble,
                    ..pose
                },
                [0.5, 0.3],
                0.1 + 0.005 * wobble,
            );
            let mut image = sample.image;
            for p in image.iter_mut() {
                p[1] *= 1280.0 / 720.0;
            }
            writer
                .write(&FrameRecord {
                    t: i as u64 * 16_667,
                    track: 1,
                    hand: Hand::Right,
                    hand_score: 0.99,
                    presence: 0.99,
                    image,
                    world: sample.world,
                    clip: None,
                })
                .unwrap();
        }
        writer.finish().unwrap()
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Téléphone !"), "telephone");
        assert_eq!(slug("  Pouce + auriculaire "), "pouce_auriculaire");
        assert_eq!(slug("3 doigts"), "pose_3_doigts");
        assert_eq!(slug("???"), "");
        assert!(mudra_dataset::validate_label(&slug("Ça marche à 100 %")).is_ok());
        assert!(mudra_dataset::validate_label(&slug(&"é".repeat(80))).is_ok());
    }

    #[test]
    fn labels_avoid_existing_gestures() {
        assert_eq!(unique_label("Poing", &["fist"]).as_deref(), Some("poing"));
        assert_eq!(unique_label("Fist", &["fist"]).as_deref(), Some("fist_2"));
        assert_eq!(
            unique_label("Fist", &["fist", "fist_2"]).as_deref(),
            Some("fist_3")
        );
        assert_eq!(unique_label("!!", &[]), None);
    }

    #[test]
    fn pose_is_trained_from_recorded_takes_then_edited_and_deleted() {
        let root = temp_root("train");
        let config = config();
        for roll in [-8.0, 0.0, 8.0] {
            write_take(&root, "telephone", PHONE, roll, 120);
        }
        let pose = train_in(&root, "telephone", Some("Téléphone"), &config).unwrap();
        assert_eq!((pose.name.as_str(), pose.takes), ("Téléphone", 3));
        assert!(pose.resemblances.is_empty(), "{:?}", pose.resemblances);

        // La pose relue du disque reconnaît une main jamais vue.
        let (poses, notices) = custom::load_dir(&root);
        assert!(notices.is_empty(), "{notices:?}");
        let (base, _, _) = crate::pipeline::load_base_classifiers(&config);
        let mut classifier = CustomStaticClassifier::new(base, &poses);
        let mut probs = vec![0.0; classifier.labels().len()];
        let sample = hand(
            Pose {
                roll_deg: 4.0,
                ..PHONE
            },
            [0.4, 0.4],
            0.12,
        );
        classifier.classify(&sample, &mut probs).unwrap();
        assert!(*probs.last().unwrap() >= config.defaults.static_enter);

        // Réglage puis nouvelle prise : le réglage et le nom survivent.
        update_in(&root, "telephone", |p| p.sensitivity = 1.3).unwrap();
        write_take(&root, "telephone", PHONE, 12.0, 120);
        let pose = train_in(&root, "telephone", None, &config).unwrap();
        assert_eq!(
            (pose.name.as_str(), pose.takes, pose.sensitivity),
            ("Téléphone", 4, 1.3)
        );

        delete_in(&root, "telephone").unwrap();
        assert!(custom::load_dir(&root).0.is_empty());
        assert!(take_files(&root, "telephone").is_empty());
        delete_in(&root, "telephone").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_take_without_the_hand_is_refused_and_can_be_discarded() {
        let root = temp_root("short");
        write_take(&root, "telephone", PHONE, 0.0, 120);
        let short = write_take(&root, "telephone", PHONE, 5.0, 4);
        let error = train_in(&root, "telephone", Some("Téléphone"), &config()).unwrap_err();
        assert!(error.contains("4 images"), "{error}");
        assert!(custom::load_dir(&root).0.is_empty());

        discard_takes(&[short]);
        assert_eq!(take_files(&root, "telephone").len(), 1);
        assert!(train_in(&root, "telephone", Some("Téléphone"), &config()).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_copy_of_a_builtin_pose_is_flagged() {
        let root = temp_root("copy");
        for roll in [-5.0, 0.0, 5.0] {
            write_take(&root, "mon_v", Pose::V_SIGN, roll, 120);
        }
        let pose = train_in(&root, "mon_v", Some("Mon V"), &config()).unwrap();
        assert_eq!(pose.resemblances.len(), 1, "{:?}", pose.resemblances);
        assert_eq!(pose.resemblances[0].label, "v_sign");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Mesure sur de vraies prises, si le dépôt en contient (dossier ignoré
    /// par git) : `cargo test --release -p mudra-app real_takes -- --ignored --nocapture`.
    #[test]
    #[ignore = "a besoin de dataset/static/thumbs_up enregistré avec une vraie main"]
    fn real_takes_generalise_to_another_session() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dataset/static/thumbs_up");
        let mut sessions: Vec<PathBuf> = std::fs::read_dir(&source)
            .expect("dossier de sessions")
            .flatten()
            .map(|e| e.path())
            .collect();
        sessions.sort();
        assert!(sessions.len() >= 2, "au moins deux sessions");
        let held_out = read_take(&sessions.pop().unwrap()).unwrap();
        // Quatre secondes de chaque session restante, comme les prises de l'interface.
        let takes: Vec<Vec<HandSample>> = sessions
            .iter()
            .map(|s| read_take(s).unwrap().into_iter().take(240).collect())
            .collect();
        let pose = CustomPose::train("pouce", "Pouce", &takes).unwrap();
        // Le suivi hésite entre main gauche et main droite sur cette pose.
        let rights = held_out
            .iter()
            .filter(|s| s.handedness == Handedness::Right)
            .count();
        println!(
            "session inconnue : {rights} images « droite » sur {}",
            held_out.len()
        );
        let accepted = held_out
            .iter()
            .filter(|s| pose.confidence(s) >= 0.85)
            .count();
        let share = accepted as f32 / held_out.len() as f32;
        println!(
            "seuil {:.3} · {} exemples · session inconnue acceptée à {:.0} %",
            pose.threshold,
            pose.examples.len(),
            share * 100.0
        );
        for other in [Pose::OPEN_PALM, Pose::FIST, Pose::V_SIGN, Pose::POINTING] {
            println!(
                "  main synthétique {other:?} : {:.2}",
                pose.confidence(&hand(other, [0.5, 0.5], 0.1))
            );
        }
        assert!(share > 0.7, "{share}");
    }
}
