//! Source de test : rejoue une image JPEG fixe à cadence constante. Permet de
//! faire tourner tout le pipeline (aperçu, bench) sans caméra.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{CaptureError, Frame, FrameSource, NativeFormat, PixelFormat, now_us};

pub struct StillSource {
    name: String,
    jpeg: Vec<u8>,
    period: Duration,
    next: Instant,
    seq: u64,
    fps: u32,
}

impl StillSource {
    pub fn open(path: &Path, fps: u32) -> Result<Self, CaptureError> {
        let jpeg = std::fs::read(path)
            .map_err(|e| CaptureError::Other(format!("{} : {e}", path.display())))?;
        if jpeg.len() < 4 || jpeg[..2] != [0xFF, 0xD8] {
            return Err(CaptureError::Other(format!(
                "{} n'est pas un fichier JPEG",
                path.display()
            )));
        }
        let fps = fps.clamp(1, 240);
        Ok(Self {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            jpeg,
            period: Duration::from_secs_f64(1.0 / f64::from(fps)),
            next: Instant::now(),
            seq: 0,
            fps,
        })
    }
}

impl FrameSource for StillSource {
    fn next_frame(&mut self, frame: &mut Frame) -> Result<(), CaptureError> {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        self.next = self.next.max(now) + self.period;
        self.seq += 1;
        frame.data.clear();
        frame.data.extend_from_slice(&self.jpeg);
        frame.format = PixelFormat::Mjpeg;
        frame.width = 0;
        frame.height = 0;
        frame.timestamp_us = now_us();
        frame.device_timestamp = 0;
        frame.seq = self.seq;
        Ok(())
    }

    fn description(&self) -> (String, NativeFormat) {
        (
            format!("image fixe {}", self.name),
            NativeFormat {
                subtype: "MJPG".into(),
                pixel_format: Some(PixelFormat::Mjpeg),
                width: 0,
                height: 0,
                fps: self.fps as f32,
            },
        )
    }
}
