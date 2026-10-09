//! Capture d'écran (§5.5) : capture par xcap sur le thread des actions,
//! encodage PNG sur le thread worker, copie optionnelle dans le presse-papiers.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use mudra_config::ScreenshotTarget;
use windows::Win32::Foundation::{GlobalFree, HANDLE, RECT};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
use xcap::{Monitor, Window};

use crate::ActionError;

/// Pixels RGBA d'une capture.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn shot(e: impl std::fmt::Display) -> ActionError {
    ActionError::Screenshot(e.to_string())
}

fn primary_monitor() -> Result<Monitor, ActionError> {
    let monitors = Monitor::all().map_err(shot)?;
    let primary = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(0);
    monitors
        .into_iter()
        .nth(primary)
        .ok_or_else(|| ActionError::Screenshot("aucun moniteur".into()))
}

/// Moniteur contenant le centre de la fenêtre au premier plan.
fn active_monitor() -> Result<Monitor, ActionError> {
    let mut rect = RECT::default();
    // SAFETY: fenêtre éventuellement nulle, auquel cas GetWindowRect échoue proprement.
    let found = unsafe { GetWindowRect(GetForegroundWindow(), &mut rect) }.is_ok();
    if found
        && let Ok(monitor) =
            Monitor::from_point((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
    {
        return Ok(monitor);
    }
    primary_monitor()
}

/// Capture la cible demandée ; se replie sur le moniteur actif si la fenêtre
/// active n'est pas capturable.
pub fn capture(target: ScreenshotTarget) -> Result<Capture, ActionError> {
    let image = match target {
        ScreenshotTarget::Screen => primary_monitor()?.capture_image().map_err(shot)?,
        ScreenshotTarget::ActiveMonitor => active_monitor()?.capture_image().map_err(shot)?,
        ScreenshotTarget::ActiveWindow => {
            let focused = Window::all()
                .map_err(shot)?
                .into_iter()
                .find(|w| w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(true));
            match focused.map(|w| w.capture_image()) {
                Some(Ok(image)) => image,
                _ => active_monitor()?.capture_image().map_err(shot)?,
            }
        }
    };
    Ok(Capture {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

/// `<dossier>\AAAA-MM-JJ_HH-MM-SS.png`, heure locale ; suffixe si le fichier existe.
pub fn output_path(dir: &Path) -> PathBuf {
    // SAFETY: lecture de l'heure système.
    let t = unsafe { GetLocalTime() };
    let stem = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let mut path = dir.join(format!("{stem}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{stem}_{n}.png"));
    }
    path
}

/// Encode en PNG (compression rapide) et écrit le fichier.
pub fn write_png(capture: &Capture, path: &Path) -> Result<(), ActionError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(shot)?;
    }
    let file = File::create(path).map_err(shot)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), capture.width, capture.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(shot)?;
    writer.write_image_data(&capture.rgba).map_err(shot)?;
    writer.finish().map_err(shot)
}

/// Construit le bloc CF_DIB : en-tête BITMAPINFOHEADER puis lignes BGRA de bas en haut.
fn dib_bytes(capture: &Capture) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: capture.width as i32,
        biHeight: capture.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: capture.width * capture.height * 4,
        ..Default::default()
    };
    let stride = capture.width as usize * 4;
    let mut out = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + capture.rgba.len());
    // SAFETY: BITMAPINFOHEADER est une structure C sans remplissage.
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(
            (&raw const header).cast::<u8>(),
            size_of::<BITMAPINFOHEADER>(),
        )
    });
    for row in capture.rgba.chunks_exact(stride).rev() {
        for px in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

/// Copie l'image dans le presse-papiers au format CF_DIB.
pub fn copy_to_clipboard(capture: &Capture) -> Result<(), ActionError> {
    let clip = |e: windows::core::Error| {
        ActionError::Screenshot(format!("presse-papiers : {}", e.message()))
    };
    let bytes = dib_bytes(capture);
    // SAFETY: le bloc global est rempli avant d'être cédé au presse-papiers ;
    // en cas d'échec de SetClipboardData, il nous appartient toujours et est libéré.
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(clip)?;
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err(ActionError::Screenshot(
                "presse-papiers : mémoire indisponible".into(),
            ));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(handle);
        if let Err(e) = OpenClipboard(None) {
            let _ = GlobalFree(Some(handle));
            return Err(clip(e));
        }
        let result = EmptyClipboard()
            .and_then(|()| SetClipboardData(u32::from(CF_DIB.0), Some(HANDLE(handle.0))));
        let _ = CloseClipboard();
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = GlobalFree(Some(handle));
                Err(clip(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_is_bottom_up_bgra() {
        // 2×2 : rouge, vert / bleu, blanc.
        let capture = Capture {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        let dib = dib_bytes(&capture);
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
        // Première ligne du DIB = dernière ligne de l'image : bleu puis blanc, en BGRA.
        assert_eq!(&dib[40..48], &[255, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&dib[48..56], &[0, 0, 255, 255, 0, 255, 0, 255]);
    }

    /// Test matériel : capture en mémoire uniquement, rien n'est écrit ni affiché.
    #[test]
    #[ignore = "nécessite une session graphique"]
    fn captures_pixels_from_the_screen() {
        for target in [
            ScreenshotTarget::Screen,
            ScreenshotTarget::ActiveMonitor,
            ScreenshotTarget::ActiveWindow,
        ] {
            let shot = capture(target).expect("capture");
            assert!(shot.width >= 64 && shot.height >= 64, "{target:?}");
            assert_eq!(shot.rgba.len(), (shot.width * shot.height * 4) as usize);
        }
    }

    #[test]
    fn png_round_trip() {
        let dir = std::env::temp_dir().join(format!("mudra-shot-test-{}", std::process::id()));
        let capture = Capture {
            width: 3,
            height: 2,
            rgba: (0..24).collect(),
        };
        let path = output_path(&dir);
        assert!(
            path.file_name().unwrap().to_string_lossy().len() >= "2026-10-09_20-15-30.png".len()
        );
        write_png(&capture, &path).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; 24];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(buf, capture.rgba);
        // Un second fichier dans la même seconde ne remplace pas le premier.
        assert_ne!(output_path(&dir), path);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
