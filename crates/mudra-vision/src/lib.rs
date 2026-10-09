//! Vision de Mudra (§7.2 à §7.4) : décodage, recadrage affine, détection de
//! paume, points-clés et suivi des mains.
//!
//! Toutes les coordonnées publiques sont exprimées dans l'**image affichée** :
//! l'image source après miroir horizontal éventuel (vue « selfie », EF-07). Le
//! miroir n'est jamais appliqué par copie : il est replié dans la matrice de
//! l'échantillonneur affine.

pub mod affine;
pub mod decode;
pub mod landmarks;
pub mod palm;
pub mod session;
pub mod tracker;

pub use affine::{Affine, Roi};
pub use decode::{FrameDecoder, SourceFormat};
pub use session::{Provider, VisionSessions};
pub use tracker::{Hand, HandTracker, TrackerConfig, TrackerStats};

pub const NUM_LANDMARKS: usize = 21;

/// 21 points `[x, y, z]`.
pub type Landmarks = [[f32; 3]; NUM_LANDMARKS];

/// Image RGB8 entrelacée, lignes contiguës.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
}

impl<'a> Image<'a> {
    pub fn new(data: &'a [u8], width: usize, height: usize) -> Self {
        assert!(data.len() >= width * height * 3, "tampon RGB trop court");
        Self {
            data,
            width,
            height,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    #[error("décodage {format} : {message}")]
    Decode {
        format: &'static str,
        message: String,
    },
    #[error("modèle {path} : {message}")]
    Model { path: String, message: String },
    #[error("inférence {stage} : {message}")]
    Inference {
        stage: &'static str,
        message: String,
    },
}
