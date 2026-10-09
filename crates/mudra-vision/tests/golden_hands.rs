//! Tests dorés de la vision sur des photos de mains (tests/fixtures/hands).
//!
//! Ils exigent les modèles ONNX (scripts/fetch-models.ps1) ; en leur absence,
//! les tests sont ignorés avec un message. Les photos ne sont pas des vues
//! « selfie » : elles sont traitées sans miroir, et la latéralité attendue est
//! celle de la personne photographiée (EF-14).

use std::path::{Path, PathBuf};

use mudra_vision::{
    FrameDecoder, Hand, HandTracker, Provider, SourceFormat, TrackerConfig, VisionSessions,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tracker(max_hands: usize, mirror: bool) -> Option<HandTracker> {
    let models = root().join("models");
    let palm = models.join("palm_detection_mediapipe_2023feb.onnx");
    let landmarks = models.join("handpose_estimation_mediapipe_2023feb.onnx");
    if !palm.is_file() || !landmarks.is_file() {
        eprintln!("modèles absents : test ignoré (lance scripts/fetch-models.ps1)");
        return None;
    }
    let sessions =
        VisionSessions::load(&palm, &landmarks, Provider::Cpu).expect("chargement des modèles");
    Some(HandTracker::new(
        sessions,
        TrackerConfig {
            max_hands,
            palm_score_threshold: 0.5,
            presence_threshold: 0.6,
            redetect_every_n_frames: 10,
            mirror,
        },
    ))
}

/// Mains vues à chaque passage, et taille de l'image.
type Run = (Vec<Vec<Hand>>, (usize, usize));

/// Traite `frames` fois la même photo et renvoie les mains de chaque passage.
fn run(name: &str, max_hands: usize, mirror: bool, frames: usize) -> Option<Run> {
    let mut tracker = tracker(max_hands, mirror)?;
    let bytes =
        std::fs::read(root().join("tests/fixtures/hands").join(name)).expect("photo de test");
    let mut decoder = FrameDecoder::new();
    let image = decoder
        .decode(SourceFormat::Mjpeg, &bytes, 0, 0)
        .expect("décodage JPEG");
    let size = (image.width, image.height);
    let mut all = Vec::new();
    let mut hands = Vec::new();
    for _ in 0..frames {
        tracker.process(&image, &mut hands).expect("inférence");
        all.push(hands.clone());
    }
    Some((all, size))
}

fn describe(name: &str, hands: &[Hand]) {
    for h in hands {
        eprintln!(
            "{name}: piste {} {} ({:.2}) présence {:.2} boîte [{:.0}, {:.0}, {:.0}, {:.0}] poignet ({:.0}, {:.0}) index ({:.0}, {:.0})",
            h.track_id,
            if h.right { "droite" } else { "gauche" },
            h.handedness_score,
            h.presence,
            h.bbox[0],
            h.bbox[1],
            h.bbox[2],
            h.bbox[3],
            h.image[0][0],
            h.image[0][1],
            h.image[8][0],
            h.image[8][1],
        );
    }
}

fn max_drift(a: &Hand, b: &Hand) -> f32 {
    a.image
        .iter()
        .zip(&b.image)
        .map(|(p, q)| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt())
        .fold(0.0, f32::max)
}

#[test]
fn victory_right_hand_is_found_and_tracked() {
    let Some((frames, _)) = run("victory.jpg", 1, false, 8) else {
        return;
    };
    describe("victory", &frames[0]);
    describe("victory (suivi)", &frames[7]);
    for (i, hands) in frames.iter().enumerate() {
        assert_eq!(hands.len(), 1, "trame {i}");
        assert!(
            hands[0].presence > 0.8,
            "trame {i} : présence {}",
            hands[0].presence
        );
        assert_eq!(
            hands[0].track_id, frames[0][0].track_id,
            "la piste doit rester stable"
        );
    }
    let h = &frames[7][0];
    // EF-14 : main droite connue (bras venant de la gauche de l'image, paume vers l'objectif).
    assert!(h.right, "latéralité");
    // Index et majeur levés, bien séparés ; paume vers l'objectif, donc index à droite.
    assert!(
        (h.image[8][0] - 175.0).abs() < 15.0 && (h.image[8][1] - 166.0).abs() < 15.0,
        "bout de l'index"
    );
    assert!(
        (h.image[12][0] - 97.0).abs() < 15.0 && (h.image[12][1] - 160.0).abs() < 15.0,
        "bout du majeur"
    );
    // Annulaire et auriculaire repliés : leurs bouts restent près de la paume.
    assert!(h.image[16][1] > 260.0 && h.image[20][1] > 260.0);
    // D'une trame à l'autre, les bouts des doigts ne bougent pas (le poignet,
    // flou et hors champ sur cette photo, est moins contraint).
    for hands in &frames[2..] {
        for tip in [8, 12] {
            let (p, q) = (hands[0].image[tip], h.image[tip]);
            assert!(
                ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() < 8.0,
                "bout {tip} instable"
            );
        }
    }
}

#[test]
fn pointing_left_hand() {
    let Some((frames, _)) = run("pointing_up.jpg", 1, false, 6) else {
        return;
    };
    describe("pointing_up", &frames[5]);
    let h = &frames[5][0];
    assert!(!h.right, "main gauche de la personne");
    // Le suivi sur une image fixe converge : la région prédite redonne les mêmes points.
    assert!(
        max_drift(&frames[4][0], h) < 2.0,
        "dérive {}",
        max_drift(&frames[4][0], h)
    );
    assert!(max_drift(&frames[0][0], h) < 8.0, "écart détection / suivi");
    assert!(
        (h.image[8][0] - 298.0).abs() < 20.0 && (h.image[8][1] - 32.0).abs() < 20.0,
        "bout de l'index"
    );
    // Les points monde sont en mètres : une paume fait entre 6 et 12 cm.
    let d = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let palm = d(h.world[0], h.world[9]);
    assert!((0.06..0.12).contains(&palm), "paume de {palm} m");
    // Repère monde aligné sur l'image : l'index pointe vers le haut (y négatif).
    assert!(h.world[8][1] < h.world[5][1] - 0.04);
}

#[test]
fn mirror_flips_coordinates_but_not_handedness() {
    let Some((plain, (width, _))) = run("pointing_up.jpg", 1, false, 6) else {
        return;
    };
    let Some((mirrored, _)) = run("pointing_up.jpg", 1, true, 6) else {
        return;
    };
    let (a, b) = (&plain[5][0], &mirrored[5][0]);
    describe("pointing_up (miroir)", &mirrored[5]);
    // En vue selfie, les coordonnées sont celles de l'image retournée.
    assert!((b.image[8][0] - (width as f32 - a.image[8][0])).abs() < 6.0);
    assert!((b.image[8][1] - a.image[8][1]).abs() < 6.0);
    // Le miroir n'est qu'un mode d'affichage : la latéralité annoncée reste
    // celle de la personne filmée (EF-14).
    assert_eq!(a.right, b.right);
    assert!(!b.right);
}

#[test]
fn thumbs_and_two_hands() {
    let Some((frames, _)) = run("thumbs_up.jpg", 1, false, 5) else {
        return;
    };
    describe("thumbs_up", &frames[4]);
    let h = &frames[4][0];
    assert!(h.right, "main droite (épaule droite vers l'objectif)");
    assert!(h.image[4][1] < h.image[2][1] - 20.0, "pouce vers le haut");

    let Some((frames, _)) = run("thumbs_down.jpg", 1, false, 5) else {
        return;
    };
    describe("thumbs_down", &frames[4]);
    let h = &frames[4][0];
    assert!(h.right, "main droite (bras venant de la gauche de l'image)");
    assert!(h.image[4][1] > h.image[2][1] + 60.0, "pouce vers le bas");

    let Some((frames, _)) = run("woman_hands.jpg", 2, false, 5) else {
        return;
    };
    describe("woman_hands", &frames[4]);
    assert_eq!(frames[4].len(), 2, "deux mains attendues");
    assert_ne!(frames[4][0].track_id, frames[4][1].track_id);
    // Main du haut : bras droit ; main du bas : bras gauche.
    let (top, bottom) = if frames[4][0].bbox[1] < frames[4][1].bbox[1] {
        (0, 1)
    } else {
        (1, 0)
    };
    assert!(
        frames[4][top].right && !frames[4][bottom].right,
        "latéralité des deux mains"
    );
    // Avec max_hands = 1, une seule main est suivie (EF-10).
    let Some((frames, _)) = run("woman_hands.jpg", 1, false, 3) else {
        return;
    };
    assert_eq!(frames[2].len(), 1);
}

#[test]
fn empty_image_has_no_hand() {
    let Some(mut tracker) = tracker(2, true) else {
        return;
    };
    let data = vec![40u8; 640 * 360 * 3];
    let image = mudra_vision::Image::new(&data, 640, 360);
    let mut hands = Vec::new();
    for _ in 0..3 {
        let stats = tracker.process(&image, &mut hands).unwrap();
        assert!(
            stats.palm_ran,
            "sans main suivie, la détection de paume tourne"
        );
        assert!(hands.is_empty());
        assert!(!tracker.is_tracking());
    }
}

/// Bout en bout vision → classifieur de poses intégré, sur des gestes réels.
#[test]
fn builtin_classifier_recognises_real_gestures() {
    use mudra_gestures::builtin::BuiltinStaticClassifier;
    use mudra_gestures::{HandSample, Handedness, StaticClassifier, argmax};

    let mut classifier = BuiltinStaticClassifier::new();
    for (photo, expected) in [
        ("thumbs_up.jpg", "thumbs_up"),
        ("thumbs_down.jpg", "thumbs_down"),
        ("victory.jpg", "v_sign"),
        ("pointing_up.jpg", "pointing"),
    ] {
        // Comme en usage réel : vue selfie.
        let Some((frames, (width, _))) = run(photo, 1, true, 6) else {
            return;
        };
        let hand = &frames[5][0];
        let mut image = hand.image;
        for p in image.iter_mut() {
            *p = [
                p[0] / width as f32,
                p[1] / width as f32,
                p[2] / width as f32,
            ];
        }
        let sample = HandSample {
            image,
            world: hand.world,
            handedness: if hand.right {
                Handedness::Right
            } else {
                Handedness::Left
            },
        };
        let mut probs = vec![0.0; classifier.labels().len()];
        classifier.classify(&sample, &mut probs).unwrap();
        let best = argmax(&probs);
        eprintln!("{photo}: {} {:.2}", classifier.labels()[best], probs[best]);
        assert_eq!(classifier.labels()[best], expected, "{photo}");
        assert!(
            probs[best] >= 0.85,
            "{photo} : {} sous le seuil d'entrée",
            probs[best]
        );
    }
}
