//! Thread de capture (§3.2) : lecture bloquante, dépôt dans la boîte aux
//! lettres, mesure de la fréquence réelle (EF-03) et reprise automatique avec
//! backoff quand la caméra disparaît (EF-05).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{CaptureError, FrameSource, NativeFormat, Producer, promote_current_thread};

/// Fabrique de source, appelée sur le thread de capture à chaque (ré)ouverture.
pub type SourceFactory = Box<dyn FnMut() -> Result<Box<dyn FrameSource>, CaptureError> + Send>;

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureState {
    Opening,
    Streaming,
    /// Caméra débranchée ou occupée : nouvel essai dans `retry_in`.
    Retrying {
        error: String,
        retry_in: Duration,
    },
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub device: String,
    pub format: Option<NativeFormat>,
    /// Fréquence mesurée sur la dernière seconde.
    pub measured_fps: f32,
    /// La fréquence réelle s'écarte de plus de 10 % de la cible (EF-03).
    pub fps_warning: bool,
    pub frames: u64,
}

impl Default for CaptureStatus {
    fn default() -> Self {
        Self {
            state: CaptureState::Opening,
            device: String::new(),
            format: None,
            measured_fps: 0.0,
            fps_warning: false,
            frames: 0,
        }
    }
}

/// Poignée du thread de capture. La détruire arrête la capture et libère la
/// caméra (EF-06).
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<CaptureStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn status(&self) -> CaptureStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Demande l'arrêt et attend la fin du thread : la caméra est alors libérée.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn update(status: &Mutex<CaptureStatus>, f: impl FnOnce(&mut CaptureStatus)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

/// Intervalle avant le prochain essai : court tant que la panne est récente
/// (reprise en moins de 5 s, CA-07), puis jusqu'à 30 s (EF-05).
fn backoff(attempt: u32, outage: Duration) -> Duration {
    let cap = if outage < Duration::from_secs(120) {
        4.0
    } else {
        30.0
    };
    Duration::from_secs_f64((0.5 * 2f64.powi(attempt.min(8) as i32)).min(cap))
}

/// Lance le thread de capture. `target_fps` sert à l'alerte de fréquence.
pub fn spawn(
    mut factory: SourceFactory,
    mut producer: Producer,
    target_fps: u32,
) -> std::io::Result<CaptureHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(CaptureStatus::default()));
    let (stop_flag, shared) = (stop.clone(), status.clone());
    let thread = std::thread::Builder::new().name("mudra-capture".into()).spawn(move || {
        promote_current_thread();
        let mut attempt = 0u32;
        let mut outage_start: Option<Instant> = None;
        while !stop_flag.load(Ordering::Acquire) {
            update(&shared, |s| s.state = CaptureState::Opening);
            let error = match factory() {
                Ok(mut source) => {
                    let (device, format) = source.description();
                    tracing::info!(
                        camera = %device,
                        format = %format!("{} {}×{} @ {:.0} fps", format.subtype, format.width, format.height, format.fps),
                        "capture démarrée"
                    );
                    update(&shared, |s| {
                        s.state = CaptureState::Streaming;
                        s.device = device;
                        s.format = Some(format);
                    });
                    let error = stream(source.as_mut(), &mut producer, &stop_flag, &shared, target_fps);
                    if error.is_some() {
                        // La caméra fonctionnait : la panne commence maintenant.
                        attempt = 0;
                        outage_start = None;
                    }
                    error
                }
                Err(e) => Some(e),
            };
            let Some(error) = error else { break };
            let outage = outage_start.get_or_insert_with(Instant::now).elapsed();
            let retry_in = backoff(attempt, outage);
            attempt += 1;
            tracing::warn!(erreur = %error, nouvel_essai_s = retry_in.as_secs_f32(), "capture interrompue");
            update(&shared, |s| {
                s.state = CaptureState::Retrying { error: error.to_string(), retry_in };
                s.measured_fps = 0.0;
            });
            let deadline = Instant::now() + retry_in;
            while Instant::now() < deadline && !stop_flag.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        update(&shared, |s| {
            s.state = CaptureState::Stopped;
            s.measured_fps = 0.0;
        });
    })?;
    Ok(CaptureHandle {
        stop,
        status,
        thread: Some(thread),
    })
}

/// Boucle de lecture. Renvoie `None` sur demande d'arrêt, l'erreur sinon.
fn stream(
    source: &mut dyn FrameSource,
    producer: &mut Producer,
    stop: &AtomicBool,
    status: &Mutex<CaptureStatus>,
    target_fps: u32,
) -> Option<CaptureError> {
    let mut window_start = Instant::now();
    let mut window_frames = 0u32;
    let mut total = 0u64;
    let mut off_target_seconds = 0u32;
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        if let Err(e) = source.next_frame(producer.buffer()) {
            return Some(e);
        }
        producer.publish();
        window_frames += 1;
        total += 1;
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let fps = window_frames as f32 / elapsed.as_secs_f32();
            let off = target_fps > 0 && (fps - target_fps as f32).abs() > 0.1 * target_fps as f32;
            off_target_seconds = if off { off_target_seconds + 1 } else { 0 };
            // Alerte après 3 secondes consécutives hors cible, pas sur un à-coup.
            let warning = off_target_seconds >= 3;
            if off_target_seconds == 3 {
                tracing::warn!(
                    mesure = fps,
                    cible = target_fps,
                    "fréquence de capture hors cible (éclairage ?)"
                );
            }
            update(status, |s| {
                s.measured_fps = fps;
                s.fps_warning = warning;
                s.frames = total;
            });
            window_start = Instant::now();
            window_frames = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, PixelFormat, mailbox};
    use std::sync::atomic::AtomicU32;

    struct Fake {
        remaining: u32,
    }

    impl FrameSource for Fake {
        fn next_frame(&mut self, frame: &mut Frame) -> Result<(), CaptureError> {
            if self.remaining == 0 {
                return Err(CaptureError::Disconnected);
            }
            self.remaining -= 1;
            std::thread::sleep(Duration::from_millis(1));
            frame.seq += 1;
            frame.data.clear();
            frame.data.push(1);
            Ok(())
        }

        fn description(&self) -> (String, NativeFormat) {
            (
                "fausse caméra".into(),
                NativeFormat {
                    subtype: "MJPG".into(),
                    pixel_format: Some(PixelFormat::Mjpeg),
                    width: 4,
                    height: 4,
                    fps: 60.0,
                },
            )
        }
    }

    #[test]
    fn backoff_is_short_at_first_then_capped() {
        assert_eq!(backoff(0, Duration::ZERO), Duration::from_millis(500));
        assert_eq!(backoff(1, Duration::ZERO), Duration::from_secs(1));
        assert_eq!(backoff(6, Duration::from_secs(20)), Duration::from_secs(4));
        assert_eq!(
            backoff(6, Duration::from_secs(300)),
            Duration::from_secs(30)
        );
        assert_eq!(
            backoff(50, Duration::from_secs(300)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn reopens_after_disconnection_and_stops_cleanly() {
        let opens = Arc::new(AtomicU32::new(0));
        let counter = opens.clone();
        let factory: SourceFactory = Box::new(move || {
            // Premier essai : caméra occupée ; ensuite 20 trames puis débranchement.
            match counter.fetch_add(1, Ordering::SeqCst) {
                0 => Err(CaptureError::DeviceNotFound("occupée".into())),
                _ => Ok(Box::new(Fake { remaining: 20 })),
            }
        });
        let (producer, mut consumer) = mailbox();
        let mut handle = spawn(factory, producer, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut received = 0;
        while opens.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            if consumer.recv_timeout(Duration::from_millis(20)).is_some() {
                received += 1;
            }
        }
        assert!(
            opens.load(Ordering::SeqCst) >= 3,
            "la source doit être rouverte après chaque panne"
        );
        assert!(received > 0);
        handle.stop();
        assert_eq!(handle.status().state, CaptureState::Stopped);
    }
}
