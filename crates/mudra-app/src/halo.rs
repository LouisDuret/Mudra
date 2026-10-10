//! Halo du curseur (EF-51) : lueur verte centrée sur le pointeur tant que
//! l'index pilote la souris. C'est une fenêtre superposée à alpha par pixel,
//! toujours au premier plan, traversée par les clics et jamais activée. Elle
//! vit sur son propre thread pour suivre le curseur sans dépendre du rendu de
//! l'aperçu.

use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetMessageW, HWND_TOPMOST,
    KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use crate::ui::theme::GREEN;

/// Rayon du halo, en pixels logiques.
const RADIUS: f32 = 44.0;
/// Opacité au centre.
const PEAK_ALPHA: f32 = 0.6;
/// Période de suivi du curseur, en millisecondes.
const FOLLOW_MS: u32 = 10;
/// Message d'affichage (`wparam` non nul) ou de masquage.
const WM_HALO_VISIBLE: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;

pub struct Halo {
    /// `HWND` de la fenêtre, qui appartient au thread du halo.
    hwnd: isize,
    visible: bool,
    thread: Option<JoinHandle<()>>,
}

impl Halo {
    /// Crée la fenêtre, masquée. `None` si Windows la refuse.
    pub fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("mudra-halo".into())
            .spawn(move || run(&tx))
            .ok()?;
        match rx.recv() {
            Ok(Some(hwnd)) => Some(Self {
                hwnd,
                visible: false,
                thread: Some(thread),
            }),
            _ => {
                tracing::warn!("halo du curseur indisponible");
                let _ = thread.join();
                None
            }
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if visible != self.visible {
            self.visible = visible;
            self.post(WM_HALO_VISIBLE, usize::from(visible));
        }
    }

    fn post(&self, message: u32, wparam: usize) {
        // SAFETY: PostMessageW accepte un handle depuis n'importe quel thread
        // et échoue proprement si la fenêtre n'existe plus.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                message,
                WPARAM(wparam),
                LPARAM(0),
            );
        }
    }
}

impl Drop for Halo {
    fn drop(&mut self) {
        self.post(WM_CLOSE, 0);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Image du halo : `size` × `size` pixels `0xAARRGGBB` prémultipliés, opaque
/// à `PEAK_ALPHA` au centre et transparente au bord.
fn halo_pixels(size: usize) -> Vec<u32> {
    let center = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0;
    let mut pixels = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let d2 =
                ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)) / (radius * radius);
            let alpha = PEAK_ALPHA * (1.0 - d2).max(0.0).powi(2);
            let channel = |c: u8| (f32::from(c) * alpha).round() as u32;
            pixels.push(
                ((alpha * 255.0).round() as u32) << 24
                    | channel(GREEN.r()) << 16
                    | channel(GREEN.g()) << 8
                    | channel(GREEN.b()),
            );
        }
    }
    pixels
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: appelée par Windows avec les arguments de la fenêtre du halo.
    unsafe {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

/// Crée la fenêtre du halo et y dessine la lueur. Renvoie la fenêtre et son
/// demi-côté en pixels.
unsafe fn create_window() -> windows::core::Result<(HWND, i32)> {
    // SAFETY: appels Win32 sur des objets créés ici ; le DC et le bitmap sont
    // libérés une fois l'image remise à la fenêtre, qui en garde une copie.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("MudraCursorHalo"),
            ..Default::default()
        };
        // Un échec ici (classe déjà enregistrée) se voit à la création.
        RegisterClassExW(&class);
        let half = (RADIUS * GetDpiForSystem() as f32 / 96.0).round() as i32;
        let size = half * 2;
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.lpszClassName,
            w!("Mudra"),
            WS_POPUP,
            0,
            0,
            size,
            size,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let pixels = halo_pixels(size as usize);
        std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixels.len()).copy_from_slice(&pixels);
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let drawn = UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&SIZE { cx: size, cy: size }),
            Some(dc),
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        drawn?;
        Ok((hwnd, half))
    }
}

/// Centre le halo sur le curseur et le garde au-dessus des autres fenêtres.
unsafe fn follow(hwnd: HWND, half: i32, last: &mut Option<(i32, i32)>) {
    // SAFETY: fenêtre vivante de ce thread.
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() || *last == Some((cursor.x, cursor.y)) {
            return;
        }
        *last = Some((cursor.x, cursor.y));
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            cursor.x - half,
            cursor.y - half,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

fn run(ready: &mpsc::Sender<Option<isize>>) {
    // SAFETY: la fenêtre est créée, utilisée et détruite sur ce seul thread.
    unsafe {
        let (hwnd, half) = match create_window() {
            Ok(window) => window,
            Err(e) => {
                tracing::warn!(erreur = %e, "création du halo du curseur");
                let _ = ready.send(None);
                return;
            }
        };
        let _ = ready.send(Some(hwnd.0 as isize));
        let mut last = None;
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            match message.message {
                WM_HALO_VISIBLE if message.wParam.0 != 0 => {
                    last = None;
                    follow(hwnd, half, &mut last);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    SetTimer(Some(hwnd), TIMER_ID, FOLLOW_MS, None);
                }
                WM_HALO_VISIBLE => {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                WM_TIMER => follow(hwnd, half, &mut last),
                _ => {
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo_fades_from_centre_to_edge() {
        let size = 72;
        let pixels = halo_pixels(size);
        let alpha = |x: usize, y: usize| pixels[y * size + x] >> 24;
        assert!(alpha(size / 2, size / 2) > 140);
        assert!(alpha(size / 2, size / 4) < alpha(size / 2, size / 2));
        for (x, y) in [(0, 0), (size - 1, 0), (0, size / 2), (size / 2, size - 1)] {
            assert!(alpha(x, y) <= 1, "bord ({x}, {y})");
        }
        // Prémultiplié : aucune composante ne dépasse l'alpha.
        for p in &pixels {
            let a = p >> 24;
            assert!((p >> 16 & 0xFF) <= a && (p >> 8 & 0xFF) <= a && (p & 0xFF) <= a);
        }
    }

    /// Contrôle visuel : `cargo test --release -p mudra-app halo -- --ignored`.
    #[test]
    #[ignore = "affiche le halo à l'écran pendant quelques secondes"]
    fn show_halo_for_a_while() {
        let mut halo = Halo::spawn().expect("fenêtre du halo");
        halo.set_visible(true);
        std::thread::sleep(std::time::Duration::from_secs(6));
        halo.set_visible(false);
    }
}
