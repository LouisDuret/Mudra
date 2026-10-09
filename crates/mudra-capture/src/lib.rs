//! Capture vidéo de Mudra (§7.1, D-02) : Media Foundation en direct, thread
//! dédié, boîte aux lettres « dernière trame gagne ».

mod mailbox;
mod mf;
mod still;
mod thread;

use std::sync::OnceLock;
use std::time::Instant;

pub use mailbox::{Consumer, Producer, mailbox};
pub use mf::{MfSource, list_devices, promote_current_thread};
pub use still::StillSource;
pub use thread::{CaptureHandle, CaptureState, CaptureStatus, SourceFactory, spawn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PixelFormat {
    #[default]
    Mjpeg,
    Nv12,
    Yuy2,
}

impl PixelFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            PixelFormat::Mjpeg => "MJPG",
            PixelFormat::Nv12 => "NV12",
            PixelFormat::Yuy2 => "YUY2",
        }
    }
}

/// Un format natif de caméra (EF-01).
#[derive(Debug, Clone, PartialEq)]
pub struct NativeFormat {
    /// Sous-type : `MJPG`, `NV12`, `YUY2` ou le FourCC brut.
    pub subtype: String,
    /// Format exploitable par Mudra, s'il l'est.
    pub pixel_format: Option<PixelFormat>,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub index: u32,
    pub name: String,
    pub formats: Vec<NativeFormat>,
}

/// Caméra désignée par sous-chaîne du nom ou par index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelector {
    Index(u32),
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    pub device: DeviceSelector,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// `None` = exposition automatique ; sinon valeur manuelle (log2 secondes).
    pub exposure: Option<i32>,
    /// Priorité d'exposition automatique : désactivée, la caméra tient sa
    /// fréquence en faible lumière (EF-04).
    pub auto_exposure_priority: bool,
}

/// Trame brute issue de la caméra. Le tampon est réutilisé d'une trame à l'autre.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub data: Vec<u8>,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    /// Horodatage QPC à la réception, en microsecondes (voir [`now_us`]).
    pub timestamp_us: u64,
    /// Horodatage Media Foundation (unités de 100 ns), conservé pour diagnostic.
    pub device_timestamp: i64,
    pub seq: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("aucune caméra ne correspond à « {0} »")]
    DeviceNotFound(String),
    #[error("la caméra « {0} » n'expose aucun format exploitable (MJPG, NV12, YUY2)")]
    NoUsableFormat(String),
    #[error("Media Foundation : {context} ({source})")]
    Platform {
        context: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("caméra déconnectée ou flux interrompu")]
    Disconnected,
    #[error("{0}")]
    Other(String),
}

/// Source de trames (§7.1). `next_frame` bloque jusqu'à la trame suivante et
/// remplit `frame` en réutilisant son tampon.
pub trait FrameSource {
    fn next_frame(&mut self, frame: &mut Frame) -> Result<(), CaptureError>;
    /// Nom de la source et format réellement ouvert.
    fn description(&self) -> (String, NativeFormat);
}

/// Horloge monotone du processus (QueryPerformanceCounter), en microsecondes.
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    // Décalage d'une seconde : 0 reste disponible comme « jamais ».
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1_000_000
}
