//! Poses personnalisées côté application : où elles vivent sur le disque,
//! comment leur identifiant est tiré du nom, et l'apprentissage à partir des
//! prises enregistrées par le pipeline.
//!
//! ```text
//! %APPDATA%\Mudra\gestures\<label>.json                    pose apprise
//! %APPDATA%\Mudra\gestures\dataset\static\<label>\*.jsonl  prises (points-clés)
//! ```
//!
//! Les exemples ajoutés à un geste de base (`open_palm`, `fist`…) vivent au
//! même endroit, sous le label de ce geste.
//!
//! Le dossier `dataset` a le format de `mudra record` : il reste utilisable
//! par la chaîne d'entraînement Python.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mudra_config::Config;
use mudra_dataset::{FrameRecord, Hand, read_session};
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

/// Une main enregistrée, au format des classifieurs. Le fichier normalise y
/// par la hauteur ; les classifieurs veulent un repère isotrope, tout divisé
/// par la largeur : `aspect` est la hauteur de l'image rapportée à sa largeur.
fn sample(record: &FrameRecord, aspect: f32) -> HandSample {
    let mut image = record.image;
    for p in image.iter_mut() {
        p[1] *= aspect;
    }
    HandSample {
        image,
        world: record.world,
        handedness: match record.hand {
            Hand::Left => Handedness::Left,
            Hand::Right => Handedness::Right,
        },
    }
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
    let aspect = header.height as f32 / header.width.max(1) as f32;
    let mut samples: Vec<HandSample> = Vec::with_capacity(records.len());
    let mut last: Option<(u64, usize)> = None;
    for r in records {
        let weight = presence[&r.track];
        let sample = sample(&r, aspect);
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

/// Images d'une prise à deux mains : celles où les deux mains sont visibles.
pub fn read_pair_take(path: &Path) -> Result<Vec<[HandSample; 2]>, String> {
    let (header, records) = read_session(path).map_err(|e| e.to_string())?;
    let aspect = header.height as f32 / header.width.max(1) as f32;
    // Les mains d'une même image se suivent, avec le même horodatage.
    Ok(records
        .chunk_by(|a, b| a.t == b.t)
        .filter_map(|frame| match frame {
            [a, b] => Some([sample(a, aspect), sample(b, aspect)]),
            _ => None,
        })
        .collect())
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
/// conservés si `name` est `None`. `two_hands` vaut pour une nouvelle pose :
/// une pose existante garde son nombre de mains. Si `label` est un geste du
/// classifieur de base, les prises deviennent des exemples de ce geste.
pub fn train_in(
    root: &Path,
    label: &str,
    name: Option<&str>,
    two_hands: bool,
    config: &Config,
) -> Result<CustomPose, String> {
    let files = take_files(root, label);
    let (existing, _) = custom::load_dir(root);
    let previous = existing.iter().find(|p| p.label == label);
    let name = name
        .or(previous.map(|p| p.name.as_str()))
        .unwrap_or(label)
        .to_string();
    let enter = config.defaults.static_enter;
    let (mut base, _, _) = crate::pipeline::load_base_classifiers(config);
    let extends_base = base.labels().iter().any(|l| l == label);
    let mut pose = if !extends_base && previous.map_or(two_hands, |p| p.two_hands) {
        let mut takes = Vec::with_capacity(files.len());
        for file in &files {
            takes.push(read_pair_take(file)?);
        }
        let mut pose = CustomPose::train_pair(label, &name, &takes).map_err(|e| e.to_string())?;
        let samples: Vec<[HandSample; 2]> = takes.into_iter().flatten().collect();
        pose.find_pair_resemblances(&samples, &existing, enter);
        pose
    } else {
        let mut takes = Vec::with_capacity(files.len());
        for file in &files {
            takes.push(read_take(file)?);
        }
        let mut pose = CustomPose::train(label, &name, &takes).map_err(|e| e.to_string())?;
        let samples: Vec<HandSample> = takes.into_iter().flatten().collect();
        pose.extends_base = extends_base;
        pose.find_resemblances(&samples, base.as_mut(), &existing, enter);
        pose
    };
    if let Some(previous) = previous {
        pose.sensitivity = previous.sensitivity;
    }
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

    /// Ouvre une prise comme le ferait le pipeline : image 1280 × 720.
    fn session(root: &Path, label: &str) -> SessionWriter {
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
        SessionWriter::create(&root.join("dataset"), header).unwrap()
    }

    /// Une main de l'image `frame`, y normalisé par la hauteur.
    fn record(frame: usize, track: u32, sample: &HandSample) -> FrameRecord {
        let mut image = sample.image;
        for p in image.iter_mut() {
            p[1] *= 1280.0 / 720.0;
        }
        FrameRecord {
            t: frame as u64 * 16_667,
            track,
            hand: match sample.handedness {
                Handedness::Left => Hand::Left,
                Handedness::Right => Hand::Right,
            },
            hand_score: 0.99,
            presence: 0.99,
            image,
            world: sample.world,
            clip: None,
        }
    }

    /// Main un peu différente à chaque image, comme une vraie main qui tient la pose.
    fn wobbly(pose: Pose, roll: f32, frame: usize, wrist: [f32; 2]) -> HandSample {
        let wobble = (frame as f32 * 0.7).sin();
        hand(
            Pose {
                roll_deg: roll + 2.0 * wobble,
                ..pose
            },
            wrist,
            0.1 + 0.005 * wobble,
        )
    }

    fn write_take(root: &Path, label: &str, pose: Pose, roll: f32, frames: usize) -> PathBuf {
        let mut writer = session(root, label);
        for i in 0..frames {
            let sample = wobbly(pose, roll, i, [0.5, 0.3]);
            writer.write(&record(i, 1, &sample)).unwrap();
        }
        writer.finish().unwrap()
    }

    /// Prise à deux mains : poing à gauche de l'écran, « téléphone » à droite.
    /// La main de droite n'est vue que sur les `both` premières images.
    fn write_pair_take(root: &Path, label: &str, roll: f32, frames: usize, both: usize) -> PathBuf {
        let mut writer = session(root, label);
        for i in 0..frames {
            let mut left = wobbly(Pose::FIST, -roll, i, [0.35, 0.3]);
            for (image, world) in left.image.iter_mut().zip(left.world.iter_mut()) {
                image[0] = 0.7 - image[0];
                world[0] = -world[0];
            }
            left.handedness = Handedness::Left;
            writer.write(&record(i, 1, &left)).unwrap();
            if i < both {
                let right = wobbly(PHONE, roll, i, [0.6, 0.3]);
                writer.write(&record(i, 2, &right)).unwrap();
            }
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
        let pose = train_in(&root, "telephone", Some("Téléphone"), false, &config).unwrap();
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
        let pose = train_in(&root, "telephone", None, false, &config).unwrap();
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
        let error = train_in(&root, "telephone", Some("Téléphone"), false, &config()).unwrap_err();
        assert!(error.contains("4 images"), "{error}");
        assert!(custom::load_dir(&root).0.is_empty());

        discard_takes(&[short]);
        assert_eq!(take_files(&root, "telephone").len(), 1);
        assert!(train_in(&root, "telephone", Some("Téléphone"), false, &config()).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_copy_of_a_builtin_pose_is_flagged() {
        let root = temp_root("copy");
        for roll in [-5.0, 0.0, 5.0] {
            write_take(&root, "mon_v", Pose::V_SIGN, roll, 120);
        }
        let pose = train_in(&root, "mon_v", Some("Mon V"), false, &config()).unwrap();
        assert_eq!(pose.resemblances.len(), 1, "{:?}", pose.resemblances);
        assert_eq!(pose.resemblances[0].label, "v_sign");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_hand_pose_is_trained_from_the_frames_showing_both_hands() {
        let root = temp_root("pair");
        let config = config();
        for roll in [-6.0, 0.0, 6.0] {
            write_pair_take(&root, "duo", roll, 120, 100);
        }
        let take = take_files(&root, "duo").remove(0);
        assert_eq!(read_pair_take(&take).unwrap().len(), 100);
        assert_eq!(read_take(&take).unwrap().len(), 120);

        let pose = train_in(&root, "duo", Some("Duo"), true, &config).unwrap();
        assert!(pose.two_hands);
        assert_eq!((pose.name.as_str(), pose.takes), ("Duo", 3));
        assert!(pose.resemblances.is_empty(), "{:?}", pose.resemblances);

        // Relue du disque, la pose est connue comme pose à deux mains.
        let (poses, notices) = custom::load_dir(&root);
        assert!(notices.is_empty(), "{notices:?}");
        let (base, _, _) = crate::pipeline::load_base_classifiers(&config);
        let classifier = CustomStaticClassifier::new(base, &poses);
        assert_eq!(classifier.two_hand_labels(), ["duo".to_string()]);

        // Une prise de plus : la pose reste à deux mains, quoi que dise l'appelant.
        write_pair_take(&root, "duo", 10.0, 120, 120);
        let pose = train_in(&root, "duo", None, false, &config).unwrap();
        assert!(pose.two_hands);
        assert_eq!((pose.name.as_str(), pose.takes), ("Duo", 4));

        // Une prise où la seconde main n'apparaît presque pas est refusée.
        write_pair_take(&root, "duo", 0.0, 120, 6);
        let error = train_in(&root, "duo", None, true, &config).unwrap_err();
        assert!(
            error.contains("deux mains") && error.contains("6 images"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn takes_of_a_base_gesture_become_its_examples() {
        let root = temp_root("base");
        let config = config();
        let (mut base, _, _) = crate::pipeline::load_base_classifiers(&config);
        let labels = base.labels().to_vec();
        let fist = labels.iter().position(|l| l == "fist").unwrap();
        let sample = hand(
            Pose {
                roll_deg: 4.0,
                ..PHONE
            },
            [0.4, 0.4],
            0.12,
        );
        let mut probs = vec![0.0; labels.len()];
        base.classify(&sample, &mut probs).unwrap();
        assert!(probs[fist] < 0.3, "{probs:?}");

        // Ma façon de fermer le poing, que la base ne reconnaît pas.
        for roll in [-8.0, 0.0, 8.0] {
            write_take(&root, "fist", PHONE, roll, 120);
        }
        let pose = train_in(&root, "fist", None, true, &config).unwrap();
        assert!(pose.extends_base && !pose.two_hands);
        assert_eq!(pose.takes, 3);
        assert!(pose.resemblances.is_empty(), "{:?}", pose.resemblances);

        // Relus du disque, les exemples alimentent le poing sans créer de pose.
        let (poses, notices) = custom::load_dir(&root);
        assert!(notices.is_empty(), "{notices:?}");
        let mut classifier = CustomStaticClassifier::new(base, &poses);
        assert_eq!(classifier.labels(), labels);
        classifier.classify(&sample, &mut probs).unwrap();
        assert!(probs[fist] >= config.defaults.static_enter, "{probs:?}");

        // Réglage, prise de plus, puis retrait : la base redevient seule.
        update_in(&root, "fist", |p| p.sensitivity = 0.8).unwrap();
        write_take(&root, "fist", PHONE, 12.0, 120);
        let pose = train_in(&root, "fist", None, false, &config).unwrap();
        assert!(pose.extends_base);
        assert_eq!((pose.takes, pose.sensitivity), (4, 0.8));
        delete_in(&root, "fist").unwrap();
        assert!(custom::load_dir(&root).0.is_empty());
        assert!(take_files(&root, "fist").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// De bout en bout sur une photo à deux mains, modèles de vision compris :
    /// prises, apprentissage, puis reconnaissance en direct. Dans un dossier
    /// jetable : `MUDRA_GESTURES_DIR=… cargo test --release -p mudra-app
    /// two_real_hands -- --ignored --nocapture`.
    #[test]
    #[ignore = "a besoin des modèles de vision et de MUDRA_GESTURES_DIR vers un dossier jetable"]
    fn two_real_hands_are_learnt_and_recognised_by_the_pipeline() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use crate::pipeline::{
            self, Command, Options, PipelineEvent, RecordEvent, RecordKind, RecordRequest, Source,
        };

        assert!(
            std::env::var_os("MUDRA_GESTURES_DIR").is_some(),
            "MUDRA_GESTURES_DIR doit désigner un dossier jetable"
        );
        let _ = std::fs::remove_dir_all(dir());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut config = config();
        for model in [&mut config.models.palm, &mut config.models.landmarks] {
            *model = repo.join(&*model).display().to_string();
        }
        let config = Arc::new(config);
        let pipeline = pipeline::start(Options {
            config: config.clone(),
            source: Source::Still(repo.join("tests/fixtures/hands/woman_hands.jpg")),
            actions: None,
            notify: None,
            start_paused: false,
        })
        .unwrap();

        // L'en-tête d'une prise porte la taille des images : attendre les premières.
        let two_hands_by = |deadline: Instant| {
            while pipeline.status().hands.len() < 2 {
                assert!(
                    Instant::now() < deadline,
                    "deux mains attendues sur la photo"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        two_hands_by(Instant::now() + Duration::from_secs(10));

        for take in 1..=3 {
            pipeline.send(Command::Record(RecordRequest {
                label: "duo".into(),
                kind: RecordKind::Static { seconds: 1.0 },
                dataset_root: dataset_root(),
            }));
            loop {
                match pipeline.events.recv_timeout(Duration::from_secs(15)) {
                    Ok(PipelineEvent::Recording(RecordEvent::Done { path, frames })) => {
                        let pairs = read_pair_take(&path).unwrap().len();
                        println!("prise {take} : {frames} mains, {pairs} images à deux mains");
                        break;
                    }
                    Ok(PipelineEvent::Recording(RecordEvent::Failed(message))) => {
                        panic!("prise {take} : {message}")
                    }
                    Ok(_) => {}
                    Err(e) => panic!("prise {take} : {e}"),
                }
            }
        }
        let pose = train_in(&dir(), "duo", Some("Duo"), true, &config).unwrap();
        println!(
            "seuil {:.3} · {} exemples",
            pose.threshold,
            pose.examples.len()
        );
        pipeline.send(Command::ReloadClassifiers);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let hands = pipeline.status().hands;
            let seen = hands.len() == 2
                && hands.iter().all(|h| {
                    h.top_static
                        .first()
                        .is_some_and(|(label, p)| label == "duo" && *p >= 0.85)
                });
            if seen {
                println!("reconnue : {:?}", hands[0].top_static);
                break;
            }
            assert!(Instant::now() < deadline, "pose non reconnue : {hands:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(pipeline);
        let _ = std::fs::remove_dir_all(dir());
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
