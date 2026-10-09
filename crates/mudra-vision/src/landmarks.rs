//! Points-clés de la main (§7.4) : `handpose_estimation_mediapipe_2023feb.onnx`.

use std::path::Path;

use ort::session::Session;
use ort::value::{TensorRef, ValueType};

use crate::affine::{Roi, sample_affine};
use crate::palm::PalmDetection;
use crate::{Image, Landmarks, NUM_LANDMARKS, VisionError};

pub const LANDMARK_INPUT: usize = 224;

// Région depuis la paume : constantes de l'implémentation de référence opencv_zoo.
const PALM_SHIFT_Y: f32 = -0.4;
const PALM_SCALE: f32 = 3.0;
// Région depuis les points-clés (suivi), §7.4.
const TRACK_SHIFT_Y: f32 = -0.1;
const TRACK_SCALE: f32 = 1.65;

/// Sortie du modèle pour une région, reprojetée dans l'image affichée.
#[derive(Debug, Clone, Copy)]
pub struct HandLandmarks {
    /// Points image en pixels de l'image affichée ; z relatif, à la même échelle.
    pub image: Landmarks,
    /// Points monde en mètres, alignés sur les axes de l'image affichée.
    pub world: Landmarks,
    /// Score de présence d'une main dans la région.
    pub presence: f32,
    /// Score de latéralité : > 0,5 = main droite, pour une image de type selfie.
    pub handedness: f32,
}

/// Région d'intérêt déduite d'une détection de paume : rotation alignant la
/// base de la paume (point 0) sur la base du majeur (point 2) vers le haut.
pub fn roi_from_palm(palm: &PalmDetection) -> Roi {
    let [p0, p2] = [palm.keypoints[0], palm.keypoints[2]];
    let rotation = Roi::rotation_from_direction(p2[0] - p0[0], p2[1] - p0[1]);
    Roi::around_points(
        palm.keypoints.iter().copied(),
        rotation,
        PALM_SHIFT_Y,
        PALM_SCALE,
    )
}

/// Région de la trame suivante, déduite des points-clés de la trame courante :
/// c'est le suivi (EF-12). Rotation du poignet (0) vers le MCP du majeur (9).
pub fn roi_from_landmarks(image: &Landmarks) -> Roi {
    let [w, m] = [image[0], image[9]];
    let rotation = Roi::rotation_from_direction(m[0] - w[0], m[1] - w[1]);
    Roi::around_points(
        image.iter().map(|p| [p[0], p[1]]),
        rotation,
        TRACK_SHIFT_Y,
        TRACK_SCALE,
    )
}

pub struct LandmarkModel {
    session: Session,
    input_name: String,
    points_name: String,
    world_name: String,
    presence_name: String,
    handedness_name: String,
    input: Vec<f32>,
    checked: bool,
}

impl LandmarkModel {
    pub fn new(session: Session, path: &Path) -> Result<Self, VisionError> {
        let err = |message: String| VisionError::Model {
            path: path.display().to_string(),
            message,
        };
        let input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| err("aucune entrée".into()))?;
        // Sorties identifiées par leur forme (§7.4). Dans chaque paire de même
        // forme, l'ordre du graphe est : points image puis points monde,
        // présence puis latéralité (ordre de mp_handpose.py dans opencv_zoo).
        let with_len = |len: i64| -> Vec<String> {
            session
                .outputs()
                .iter()
                .filter(|o| match o.dtype() {
                    ValueType::Tensor { shape, .. } => shape.len() == 2 && shape[1] == len,
                    _ => false,
                })
                .map(|o| o.name().to_string())
                .collect()
        };
        let points = with_len(63);
        let scalars = with_len(1);
        if points.len() != 2 || scalars.len() != 2 {
            let names: Vec<_> = session
                .outputs()
                .iter()
                .map(|o| o.name().to_string())
                .collect();
            return Err(err(format!("sorties inattendues : {names:?}")));
        }
        tracing::debug!(points = ?points, scalaires = ?scalars, "sorties du modèle de points-clés");
        Ok(Self {
            session,
            input_name,
            points_name: points[0].clone(),
            world_name: points[1].clone(),
            presence_name: scalars[0].clone(),
            handedness_name: scalars[1].clone(),
            input: vec![0.0; LANDMARK_INPUT * LANDMARK_INPUT * 3],
            checked: false,
        })
    }

    /// Exécute le modèle sur la région `roi` de l'image.
    pub fn run(
        &mut self,
        image: &Image<'_>,
        roi: &Roi,
        mirror: bool,
    ) -> Result<HandLandmarks, VisionError> {
        let inference = |e: ort::Error| VisionError::Inference {
            stage: "points-clés",
            message: e.to_string(),
        };
        let crop = roi.to_affine(LANDMARK_INPUT);
        let transform = if mirror {
            crop.then_mirror(image.width)
        } else {
            crop
        };
        sample_affine(
            image,
            &transform,
            LANDMARK_INPUT,
            LANDMARK_INPUT,
            &mut self.input,
        );

        let tensor = TensorRef::from_array_view((
            [1usize, LANDMARK_INPUT, LANDMARK_INPUT, 3],
            self.input.as_slice(),
        ))
        .map_err(inference)?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(inference)?;
        let extract = |name: &str| {
            outputs[name]
                .try_extract_tensor::<f32>()
                .map(|(_, d)| d)
                .map_err(inference)
        };
        let mut points = extract(&self.points_name)?;
        let mut world = extract(&self.world_name)?;
        let presence = extract(&self.presence_name)?;
        let handedness = extract(&self.handedness_name)?;
        if points.len() != 63 || world.len() != 63 || presence.is_empty() || handedness.is_empty() {
            return Err(VisionError::Inference {
                stage: "points-clés",
                message: "forme de sortie inattendue".into(),
            });
        }
        let (presence, handedness) = (presence[0], handedness[0]);

        // Garde-fou : les points image sont en pixels du recadrage (jusqu'à 224),
        // les points monde en mètres (quelques centimètres). Si l'ordre des
        // sorties du graphe était inversé, on le détecte et on le corrige.
        let magnitude = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let swapped = magnitude(points) < 2.0 && magnitude(world) > 2.0;
        if swapped {
            std::mem::swap(&mut points, &mut world);
        }

        let size = LANDMARK_INPUT as f32;
        let z_scale = roi.size / size;
        let (sin, cos) = roi.rotation.sin_cos();
        let mut result = HandLandmarks {
            image: [[0.0; 3]; NUM_LANDMARKS],
            world: [[0.0; 3]; NUM_LANDMARKS],
            presence,
            handedness,
        };
        for i in 0..NUM_LANDMARKS {
            let p = &points[i * 3..i * 3 + 3];
            let [x, y] = roi.crop_to_image(p[0], p[1], size);
            result.image[i] = [x, y, p[2] * z_scale];
            // Les points monde sortent dans le repère du recadrage redressé :
            // on les ramène sur les axes de l'image.
            let w = &world[i * 3..i * 3 + 3];
            result.world[i] = [w[0] * cos - w[1] * sin, w[0] * sin + w[1] * cos, w[2]];
        }
        drop(outputs);
        if swapped && !self.checked {
            tracing::warn!(
                "sorties points image / points monde inversées dans le graphe : corrigé"
            );
            std::mem::swap(&mut self.points_name, &mut self.world_name);
        }
        self.checked = true;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palm_roi_is_upright_shifted_and_enlarged() {
        // Paume verticale : base en bas, base du majeur en haut.
        let mut keypoints = [[0.0f32; 2]; 7];
        keypoints[0] = [100.0, 200.0];
        keypoints[2] = [100.0, 120.0];
        keypoints[1] = [70.0, 130.0];
        keypoints[3] = [125.0, 130.0];
        keypoints[4] = [140.0, 145.0];
        keypoints[5] = [60.0, 180.0];
        keypoints[6] = [60.0, 160.0];
        let palm = PalmDetection {
            score: 0.9,
            bbox: [60.0, 120.0, 140.0, 200.0],
            keypoints,
        };
        let roi = roi_from_palm(&palm);
        assert!(roi.rotation.abs() < 1e-6);
        // Boîte des points : x 60..140, y 120..200 → centre (100, 160), décalé de −0,4 × 80.
        assert!((roi.cx - 100.0).abs() < 1e-3);
        assert!((roi.cy - 128.0).abs() < 1e-3);
        assert!((roi.size - 240.0).abs() < 1e-3);
    }

    #[test]
    fn palm_roi_follows_hand_rotation() {
        // Même paume tournée de 90° dans le sens horaire : les doigts pointent à droite.
        let rot = |p: [f32; 2]| [300.0 - (p[1] - 160.0), 300.0 + (p[0] - 100.0)];
        let mut keypoints = [[0.0f32; 2]; 7];
        let upright = [
            [100.0, 200.0],
            [70.0, 130.0],
            [100.0, 120.0],
            [125.0, 130.0],
            [140.0, 145.0],
            [60.0, 180.0],
            [60.0, 160.0],
        ];
        for (k, p) in keypoints.iter_mut().zip(upright) {
            *k = rot(p);
        }
        let palm = PalmDetection {
            score: 0.9,
            bbox: [0.0; 4],
            keypoints,
        };
        let roi = roi_from_palm(&palm);
        assert!((roi.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!((roi.size - 240.0).abs() < 1e-2);
        // Le centre est décalé vers les doigts, donc vers la droite.
        assert!((roi.cx - 332.0).abs() < 1e-2, "{}", roi.cx);
        assert!((roi.cy - 300.0).abs() < 1e-2, "{}", roi.cy);
    }

    #[test]
    fn tracking_roi_covers_all_landmarks() {
        let mut lm = [[0.0f32; 3]; NUM_LANDMARKS];
        for (i, p) in lm.iter_mut().enumerate() {
            *p = [
                200.0 + (i % 5) as f32 * 20.0,
                300.0 - (i / 5) as f32 * 30.0,
                0.0,
            ];
        }
        lm[0] = [240.0, 330.0, 0.0];
        lm[9] = [240.0, 240.0, 0.0];
        let roi = roi_from_landmarks(&lm);
        assert!(roi.rotation.abs() < 1e-6);
        let [x0, y0, x1, y1] = roi.bounds();
        for p in &lm {
            assert!(p[0] > x0 && p[0] < x1 && p[1] > y0 && p[1] < y1);
        }
    }
}
