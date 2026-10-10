//! Capture vidéo de Mudra (§7.1, D-02) : Media Foundation en direct, thread
//! dédié, boîte aux lettres « dernière trame gagne ».

mod mailbox;
mod mf;
mod still;
mod thread;

use std::sync::OnceLock;
use std::time::Instant;

pub use mailbox::{Consumer, Producer, mailbox};
pub use mf::{MfSource, list_device_names, list_devices, promote_current_thread};
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

/// Caméra désignée par sous-chaîne du nom ou par index ; `Auto` prend la
/// première caméra du PC qui s'ouvre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelector {
    Auto,
    Index(u32),
    Name(String),
}

impl DeviceSelector {
    /// Ordre d'essai parmi les caméras `names`, et `true` si la caméra
    /// demandée n'en fait pas partie : toutes sont alors essayées à sa place.
    pub fn candidates(&self, names: &[String]) -> (Vec<usize>, bool) {
        let found = match self {
            DeviceSelector::Auto => return ((0..names.len()).collect(), false),
            DeviceSelector::Index(i) => Some(*i as usize).filter(|i| *i < names.len()),
            DeviceSelector::Name(wanted) => {
                let wanted = wanted.to_lowercase();
                names
                    .iter()
                    .position(|n| n.to_lowercase().contains(&wanted))
            }
        };
        match found {
            Some(i) => (vec![i], false),
            None => ((0..names.len()).collect(), true),
        }
    }

    fn label(&self) -> String {
        match self {
            DeviceSelector::Auto => "auto".into(),
            DeviceSelector::Index(i) => format!("n° {i}"),
            DeviceSelector::Name(n) => n.clone(),
        }
    }
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
    #[error("aucune caméra détectée sur ce PC : branche une webcam")]
    NoDevice,
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
    /// Ce que l'utilisateur doit savoir de l'ouverture (caméra de repli).
    fn notice(&self) -> Option<String> {
        None
    }
}

/// Horloge monotone du processus (QueryPerformanceCounter), en microsecondes.
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    // Décalage d'une seconde : 0 reste disponible comme « jamais ».
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64 + 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_camera_falls_back_to_every_camera() {
        let names = ["Integrated Webcam".to_string(), "USB Camera".to_string()];
        let pick = |selector: DeviceSelector| selector.candidates(&names);
        assert_eq!(pick(DeviceSelector::Auto), (vec![0, 1], false));
        assert_eq!(pick(DeviceSelector::Name("usb".into())), (vec![1], false));
        assert_eq!(pick(DeviceSelector::Index(1)), (vec![1], false));
        // La caméra d'un autre PC, ou un index trop grand : toutes sont essayées.
        assert_eq!(
            pick(DeviceSelector::Name("Logitech StreamCam".into())),
            (vec![0, 1], true)
        );
        assert_eq!(pick(DeviceSelector::Index(4)), (vec![0, 1], true));
        assert_eq!(DeviceSelector::Auto.candidates(&[]), (vec![], false));
    }
}
