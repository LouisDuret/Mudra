//! Détection de paume BlazePalm (§7.3) : `palm_detection_mediapipe_2023feb.onnx`.

use std::path::Path;

use ort::session::Session;
use ort::value::{TensorRef, ValueType};

use crate::affine::{Affine, iou, sample_affine};
use crate::{Image, VisionError};

pub const PALM_INPUT: usize = 192;
pub const NUM_ANCHORS: usize = 2016;
const NMS_IOU: f32 = 0.3;

/// Une paume détectée, en pixels de l'image affichée.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PalmDetection {
    pub score: f32,
    /// `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// 7 points de paume ; 0 = base de la paume, 2 = base du majeur (Annexe A).
    pub keypoints: [[f32; 2]; 7],
}

/// 2016 ancres SSD : 24×24×2 au pas 8, puis 12×12×6 au pas 16 (centres normalisés).
pub fn generate_anchors() -> Vec<[f32; 2]> {
    let mut anchors = Vec::with_capacity(NUM_ANCHORS);
    for (grid, per_cell) in [(24usize, 2usize), (12, 6)] {
        for y in 0..grid {
            for x in 0..grid {
                let center = [
                    (x as f32 + 0.5) / grid as f32,
                    (y as f32 + 0.5) / grid as f32,
                ];
                for _ in 0..per_cell {
                    anchors.push(center);
                }
            }
        }
    }
    anchors
}

/// Décode les sorties brutes du réseau en détections dans le carré d'entrée
/// (coordonnées en pixels du carré 192×192), sans NMS.
pub fn decode_outputs(
    regressions: &[f32],
    scores: &[f32],
    anchors: &[[f32; 2]],
    threshold: f32,
    out: &mut Vec<PalmDetection>,
) {
    out.clear();
    let size = PALM_INPUT as f32;
    // Seuil appliqué sur le logit : évite la sigmoïde pour les 2016 ancres.
    let logit_threshold = (threshold / (1.0 - threshold)).ln();
    for (i, anchor) in anchors.iter().enumerate() {
        let logit = scores[i];
        if logit < logit_threshold {
            continue;
        }
        let r = &regressions[i * 18..i * 18 + 18];
        let cx = r[0] + anchor[0] * size;
        let cy = r[1] + anchor[1] * size;
        let (w, h) = (r[2], r[3]);
        let mut keypoints = [[0.0; 2]; 7];
        for (k, kp) in keypoints.iter_mut().enumerate() {
            *kp = [
                r[4 + 2 * k] + anchor[0] * size,
                r[5 + 2 * k] + anchor[1] * size,
            ];
        }
        out.push(PalmDetection {
            score: 1.0 / (1.0 + (-logit).exp()),
            bbox: [cx - w * 0.5, cy - h * 0.5, cx + w * 0.5, cy + h * 0.5],
            keypoints,
        });
    }
}

/// NMS pondérée de MediaPipe : les détections qui se recouvrent sont fusionnées
/// par moyenne pondérée par le score, plus stable d'une trame à l'autre.
pub fn weighted_nms(candidates: &mut Vec<PalmDetection>, out: &mut Vec<PalmDetection>) {
    out.clear();
    candidates.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
    while !candidates.is_empty() {
        let top = candidates[0];
        let mut total = 0.0;
        let mut bbox = [0.0f32; 4];
        let mut keypoints = [[0.0f32; 2]; 7];
        candidates.retain(|c| {
            if iou(&top.bbox, &c.bbox) <= NMS_IOU && c.bbox != top.bbox {
                return true;
            }
            total += c.score;
            for (acc, v) in bbox.iter_mut().zip(c.bbox) {
                *acc += v * c.score;
            }
            for (acc, kp) in keypoints.iter_mut().zip(c.keypoints) {
                acc[0] += kp[0] * c.score;
                acc[1] += kp[1] * c.score;
            }
            false
        });
        out.push(PalmDetection {
            score: top.score,
            bbox: bbox.map(|v| v / total),
            keypoints: keypoints.map(|kp| [kp[0] / total, kp[1] / total]),
        });
    }
}

pub struct PalmDetector {
    session: Session,
    input_name: String,
    regressions_name: String,
    scores_name: String,
    input: Vec<f32>,
    anchors: Vec<[f32; 2]>,
    candidates: Vec<PalmDetection>,
}

impl PalmDetector {
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
        // Les sorties sont identifiées par leur forme, jamais par leur index (§7.3).
        let by_last_dim = |last: i64| {
            session
                .outputs()
                .iter()
                .find(|o| match o.dtype() {
                    ValueType::Tensor { shape, .. } => {
                        shape.len() == 3 && shape[1] == NUM_ANCHORS as i64 && shape[2] == last
                    }
                    _ => false,
                })
                .map(|o| o.name().to_string())
                .ok_or_else(|| {
                    err(format!(
                        "sortie de forme [1, {NUM_ANCHORS}, {last}] absente"
                    ))
                })
        };
        let regressions_name = by_last_dim(18)?;
        let scores_name = by_last_dim(1)?;
        Ok(Self {
            session,
            input_name,
            regressions_name,
            scores_name,
            input: vec![0.0; PALM_INPUT * PALM_INPUT * 3],
            anchors: generate_anchors(),
            candidates: Vec::with_capacity(64),
        })
    }

    /// Détecte les paumes de l'image ; `out` reçoit les détections en pixels
    /// de l'image affichée, triées par score décroissant.
    pub fn detect(
        &mut self,
        image: &Image<'_>,
        mirror: bool,
        threshold: f32,
        out: &mut Vec<PalmDetection>,
    ) -> Result<(), VisionError> {
        let inference = |e: ort::Error| VisionError::Inference {
            stage: "paume",
            message: e.to_string(),
        };
        let letterbox = Affine::letterbox(image.width, image.height, PALM_INPUT);
        let transform = if mirror {
            letterbox.then_mirror(image.width)
        } else {
            letterbox
        };
        sample_affine(image, &transform, PALM_INPUT, PALM_INPUT, &mut self.input);

        let tensor = TensorRef::from_array_view((
            [1usize, PALM_INPUT, PALM_INPUT, 3],
            self.input.as_slice(),
        ))
        .map_err(inference)?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(inference)?;
        let (_, regressions) = outputs[self.regressions_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        let (_, scores) = outputs[self.scores_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        if regressions.len() != NUM_ANCHORS * 18 || scores.len() != NUM_ANCHORS {
            return Err(VisionError::Inference {
                stage: "paume",
                message: "forme de sortie inattendue".into(),
            });
        }
        decode_outputs(
            regressions,
            scores,
            &self.anchors,
            threshold,
            &mut self.candidates,
        );
        drop(outputs);
        weighted_nms(&mut self.candidates, out);

        // Carré d'entrée → image affichée (le letterbox seul, sans le miroir).
        let to_image = |p: [f32; 2]| {
            let q = letterbox.apply(p[0] - 0.5, p[1] - 0.5);
            [q[0] + 0.5, q[1] + 0.5]
        };
        for d in out.iter_mut() {
            let a = to_image([d.bbox[0], d.bbox[1]]);
            let b = to_image([d.bbox[2], d.bbox[3]]);
            d.bbox = [a[0], a[1], b[0], b[1]];
            d.keypoints = d.keypoints.map(to_image);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_reference_layout() {
        let a = generate_anchors();
        assert_eq!(a.len(), NUM_ANCHORS);
        // Pas 8 : grille 24×24, 2 ancres par cellule, parcours ligne par ligne.
        assert_eq!(a[0], [0.5 / 24.0, 0.5 / 24.0]);
        assert_eq!(a[1], a[0]);
        assert_eq!(a[2], [1.5 / 24.0, 0.5 / 24.0]);
        assert_eq!(a[48], [0.5 / 24.0, 1.5 / 24.0]);
        // Pas 16 : grille 12×12, 6 ancres par cellule.
        assert_eq!(a[1152], [0.5 / 12.0, 0.5 / 12.0]);
        assert_eq!(a[1157], a[1152]);
        assert_eq!(a[1158], [1.5 / 12.0, 0.5 / 12.0]);
        assert_eq!(a[2015], [11.5 / 12.0, 11.5 / 12.0]);
    }

    #[test]
    fn decodes_box_and_keypoints_relative_to_anchor() {
        let anchors = generate_anchors();
        let mut regressions = vec![0.0; NUM_ANCHORS * 18];
        let mut scores = vec![-10.0; NUM_ANCHORS];
        let i = 1152 + 6 * (12 * 5 + 3); // cellule (3, 5) du pas 16
        scores[i] = 3.0;
        regressions[i * 18..i * 18 + 6].copy_from_slice(&[4.0, -2.0, 40.0, 60.0, 10.0, 20.0]);
        let mut out = Vec::new();
        decode_outputs(&regressions, &scores, &anchors, 0.5, &mut out);
        assert_eq!(out.len(), 1);
        let d = out[0];
        let (ax, ay) = (3.5 / 12.0 * 192.0, 5.5 / 12.0 * 192.0);
        assert!((d.score - 0.9526).abs() < 1e-3);
        assert!((d.bbox[0] - (ax + 4.0 - 20.0)).abs() < 1e-4);
        assert!((d.bbox[3] - (ay - 2.0 + 30.0)).abs() < 1e-4);
        assert!((d.keypoints[0][0] - (ax + 10.0)).abs() < 1e-4);
        assert!((d.keypoints[0][1] - (ay + 20.0)).abs() < 1e-4);
        assert!((d.keypoints[1][0] - ax).abs() < 1e-4);
    }

    fn det(score: f32, x: f32) -> PalmDetection {
        PalmDetection {
            score,
            bbox: [x, 0.0, x + 10.0, 10.0],
            keypoints: [[x, 0.0]; 7],
        }
    }

    #[test]
    fn weighted_nms_merges_overlaps_and_keeps_distinct() {
        let mut candidates = vec![
            det(0.6, 1.0),
            det(0.9, 0.0),
            det(0.8, 100.0),
            det(0.3, 101.0),
        ];
        let mut out = Vec::new();
        weighted_nms(&mut candidates, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0].score - 0.9).abs() < 1e-6);
        // Moyenne pondérée : (0·0,9 + 1·0,6) / 1,5 = 0,4.
        assert!((out[0].bbox[0] - 0.4).abs() < 1e-5);
        assert!((out[0].keypoints[3][0] - 0.4).abs() < 1e-5);
        assert!((out[1].score - 0.8).abs() < 1e-6);
        assert!(out[1].bbox[0] > 100.0 && out[1].bbox[0] < 100.5);
    }

    #[test]
    fn nms_of_nothing_is_nothing() {
        let mut out = vec![det(0.5, 0.0)];
        weighted_nms(&mut Vec::new(), &mut out);
        assert!(out.is_empty());
    }
}
