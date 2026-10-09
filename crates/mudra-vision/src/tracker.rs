//! Suivi des mains d'une trame à l'autre (EF-10 à EF-15).
//!
//! La région d'intérêt de la trame N+1 est calculée depuis les points-clés de
//! la trame N. La détection de paume ne tourne que si aucune main n'est suivie
//! ou, s'il reste de la place, au plus toutes les K trames.

use std::time::Instant;

use crate::affine::{Roi, bounds_of, iou};
use crate::landmarks::{HandLandmarks, roi_from_landmarks, roi_from_palm};
use crate::palm::PalmDetection;
use crate::session::VisionSessions;
use crate::{Image, Landmarks, VisionError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerConfig {
    /// 1 ou 2 (EF-10).
    pub max_hands: usize,
    pub palm_score_threshold: f32,
    /// Sous ce score de présence, la piste est fermée (§7.4).
    pub presence_threshold: f32,
    /// K : période de re-détection quand moins de `max_hands` mains sont suivies.
    pub redetect_every_n_frames: u32,
    /// Vue « selfie » (EF-07).
    pub mirror: bool,
}

/// Une main suivie, pour une trame (EF-11).
#[derive(Debug, Clone, Copy)]
pub struct Hand {
    /// Identifiant de piste stable tant que la main est suivie.
    pub track_id: u32,
    /// 21 points en pixels de l'image affichée (z relatif, même échelle).
    pub image: Landmarks,
    /// 21 points monde, en mètres.
    pub world: Landmarks,
    /// Latéralité réelle de la main, miroir pris en compte (EF-14).
    pub right: bool,
    /// Confiance de la latéralité, dans [0,5 ; 1].
    pub handedness_score: f32,
    pub presence: f32,
    /// Boîte englobante des points `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    /// Région d'intérêt utilisée pour cette trame.
    pub roi: Roi,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TrackerStats {
    pub palm_ran: bool,
    pub palm_us: u32,
    pub landmarks_us: u32,
    pub landmark_runs: u32,
}

#[derive(Debug, Clone, Copy)]
struct Track {
    id: u32,
    /// Région à utiliser à la prochaine trame.
    roi: Roi,
}

/// Taille minimale d'une main, en pixels (EF-15).
const MIN_HAND_PX: f32 = 20.0;
const MAX_ASPECT: f32 = 6.0;
/// Déplacement maximal du centre entre deux trames, en tailles de région.
const MAX_JUMP: f32 = 0.8;
/// Recouvrement au-delà duquel une paume détectée est une main déjà suivie.
const SAME_HAND_IOU: f32 = 0.25;
/// Recouvrement au-delà duquel deux pistes suivent la même main (§7.4).
const DUPLICATE_IOU: f32 = 0.5;

pub struct HandTracker {
    sessions: VisionSessions,
    config: TrackerConfig,
    tracks: Vec<Track>,
    detections: Vec<PalmDetection>,
    next_id: u32,
    frames_since_detection: u32,
}

impl HandTracker {
    pub fn new(sessions: VisionSessions, config: TrackerConfig) -> Self {
        Self {
            sessions,
            config,
            tracks: Vec::with_capacity(2),
            detections: Vec::with_capacity(8),
            next_id: 1,
            frames_since_detection: u32::MAX / 2,
        }
    }

    pub fn sessions(&self) -> &VisionSessions {
        &self.sessions
    }

    pub fn config(&self) -> &TrackerConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: TrackerConfig) {
        if config.mirror != self.config.mirror {
            self.tracks.clear();
        }
        self.config = config;
        self.tracks.truncate(config.max_hands);
    }

    /// Vrai si au moins une main est suivie : sinon le pipeline passe en veille (EF-13).
    pub fn is_tracking(&self) -> bool {
        !self.tracks.is_empty()
    }

    /// Oublie toutes les pistes (pause, changement de caméra).
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.frames_since_detection = u32::MAX / 2;
    }

    fn hand(&self, id: u32, lm: &HandLandmarks, roi: Roi) -> Hand {
        // Mesuré sur des photos de mains connues (tests/golden_hands.rs) : le score
        // du modèle ONNX désigne la main droite telle qu'elle apparaît dans une
        // image non retournée. En vue selfie, l'image traitée est retournée :
        // la latéralité l'est aussi (EF-14).
        let model_right = lm.handedness > 0.5;
        Hand {
            track_id: id,
            image: lm.image,
            world: lm.world,
            right: model_right != self.config.mirror,
            handedness_score: lm.handedness.max(1.0 - lm.handedness),
            presence: lm.presence,
            bbox: bounds_of(lm.image.iter().map(|p| [p[0], p[1]])),
            roi,
        }
    }

    fn plausible(bbox: &[f32; 4]) -> bool {
        let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
        let (long, short) = (w.max(h), w.min(h));
        w.is_finite()
            && h.is_finite()
            && long >= MIN_HAND_PX
            && short > 0.0
            && long / short <= MAX_ASPECT
    }

    /// Traite une image ; `out` reçoit les mains suivies sur cette trame.
    pub fn process(
        &mut self,
        image: &Image<'_>,
        out: &mut Vec<Hand>,
    ) -> Result<TrackerStats, VisionError> {
        out.clear();
        let mut stats = TrackerStats::default();
        let mirror = self.config.mirror;

        // 1. Mains déjà suivies : points-clés sur la région prédite.
        let mut i = 0;
        while i < self.tracks.len() {
            let Track { id, roi } = self.tracks[i];
            let start = Instant::now();
            let lm = self.sessions.landmarks.run(image, &roi, mirror)?;
            stats.landmarks_us += start.elapsed().as_micros() as u32;
            stats.landmark_runs += 1;

            let hand = self.hand(id, &lm, roi);
            let next = roi_from_landmarks(&lm.image);
            let jump = ((next.cx - roi.cx).powi(2) + (next.cy - roi.cy).powi(2)).sqrt();
            let duplicate = out.iter().any(|h| iou(&h.bbox, &hand.bbox) > DUPLICATE_IOU);
            if lm.presence < self.config.presence_threshold
                || !Self::plausible(&hand.bbox)
                || jump > MAX_JUMP * roi.size
                || duplicate
            {
                self.tracks.remove(i);
                continue;
            }
            self.tracks[i].roi = next;
            out.push(hand);
            i += 1;
        }

        // 2. Détection de paume, seulement quand elle est utile (EF-12).
        self.frames_since_detection = self.frames_since_detection.saturating_add(1);
        let room = self.tracks.len() < self.config.max_hands;
        if room
            && (self.tracks.is_empty()
                || self.frames_since_detection >= self.config.redetect_every_n_frames)
        {
            self.frames_since_detection = 0;
            let start = Instant::now();
            self.sessions.palm.detect(
                image,
                mirror,
                self.config.palm_score_threshold,
                &mut self.detections,
            )?;
            stats.palm_ran = true;
            stats.palm_us = start.elapsed().as_micros() as u32;

            for d in 0..self.detections.len() {
                if self.tracks.len() >= self.config.max_hands {
                    break;
                }
                let roi = roi_from_palm(&self.detections[d]);
                let bounds = roi.bounds();
                if roi.size < MIN_HAND_PX
                    || self
                        .tracks
                        .iter()
                        .any(|t| iou(&t.roi.bounds(), &bounds) > SAME_HAND_IOU)
                {
                    continue;
                }
                let start = Instant::now();
                let lm = self.sessions.landmarks.run(image, &roi, mirror)?;
                stats.landmarks_us += start.elapsed().as_micros() as u32;
                stats.landmark_runs += 1;
                let id = self.next_id;
                let hand = self.hand(id, &lm, roi);
                if lm.presence < self.config.presence_threshold
                    || !Self::plausible(&hand.bbox)
                    || out.iter().any(|h| iou(&h.bbox, &hand.bbox) > DUPLICATE_IOU)
                {
                    continue;
                }
                self.next_id = self.next_id.wrapping_add(1).max(1);
                self.tracks.push(Track {
                    id,
                    roi: roi_from_landmarks(&lm.image),
                });
                out.push(hand);
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausibility_filters_tiny_and_degenerate_boxes() {
        assert!(HandTracker::plausible(&[0.0, 0.0, 100.0, 120.0]));
        assert!(!HandTracker::plausible(&[0.0, 0.0, 10.0, 12.0]));
        assert!(!HandTracker::plausible(&[0.0, 0.0, 300.0, 20.0]));
        assert!(!HandTracker::plausible(&[0.0, 0.0, 100.0, 0.0]));
        assert!(!HandTracker::plausible(&[0.0, 0.0, f32::NAN, 50.0]));
    }
}
