//! Interface de Mudra (§7.10) : thème sombre, fenêtres egui créées et détruites
//! à la demande dans l'unique boucle d'événements winit (D-14), icônes de la
//! zone de notification.

pub mod gestures;
pub mod overlay;
pub mod preview;
pub mod theme;
pub mod trainer;

use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::{Context, Result};
use egui::ViewportId;
use egui_wgpu::winit::Painter;
use mudra_gestures::EngineState;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId};

/// Une fenêtre egui rendue par wgpu. La détruire libère la surface et le
/// périphérique graphique : fenêtre fermée, aucun coût de rendu (EF-72).
pub struct EguiWindow {
    window: Arc<Window>,
    ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
}

impl EguiWindow {
    pub fn new(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("création de la fenêtre")?,
        );
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut painter = pollster::block_on(Painter::new(
            ctx.clone(),
            egui_wgpu::WgpuConfiguration::default(),
            false,
            egui_wgpu::RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(|e| anyhow::anyhow!("initialisation du rendu : {e}"))?;
        let state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            painter.max_texture_side(),
        );
        Ok(Self {
            window,
            ctx,
            state,
            painter,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Transmet un événement winit à egui. Renvoie `true` s'il faut redessiner.
    pub fn on_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(size) = event
            && let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        {
            self.painter.on_window_resized(ViewportId::ROOT, w, h);
        }
        self.state.on_window_event(&self.window, event).repaint
    }

    /// Exécute une passe egui et la présente.
    pub fn paint(&mut self, clear: egui::Color32, mut ui: impl FnMut(&mut egui::Ui)) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let input = self.state.take_egui_input(&self.window);
        let mut output = self.ctx.run_ui(input, |root| ui(root));
        self.state
            .handle_platform_output(&self.window, output.platform_output);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let c = clear.to_normalized_gamma_f32();
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            c,
            &primitives,
            &mut output.textures_delta,
            Vec::new(),
            &self.window,
        );
        // egui demande parfois une passe de plus (animations, mise en page).
        let again = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        if again {
            self.window.request_redraw();
        }
    }
}

/// Catégorie d'icône de la zone de notification (§7.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayKind {
    Paused,
    Locked,
    Armed,
    Continuous,
    Error,
}

impl TrayKind {
    pub fn of(state: EngineState) -> Self {
        match state {
            EngineState::Paused => TrayKind::Paused,
            EngineState::Locked => TrayKind::Locked,
            EngineState::Armed => TrayKind::Armed,
            EngineState::Continuous => TrayKind::Continuous,
        }
    }

    pub const ALL: [TrayKind; 5] = [
        TrayKind::Paused,
        TrayKind::Locked,
        TrayKind::Armed,
        TrayKind::Continuous,
        TrayKind::Error,
    ];
}

/// Icône 32×32 RGBA générée : gris = Pause, contour = Verrouillé, bleu = Armé,
/// vert = Continu, rouge = erreur.
pub fn tray_icon_rgba(kind: TrayKind) -> Vec<u8> {
    const SIZE: usize = 32;
    let color = match kind {
        TrayKind::Paused => theme::GRAY,
        TrayKind::Locked => theme::TEXT,
        TrayKind::Armed => theme::ACCENT,
        TrayKind::Continuous => theme::GREEN,
        TrayKind::Error => theme::RED,
    };
    let [r, g, b, _] = color.to_array();
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // Bord adouci sur un pixel.
            let disc = (13.5 - d).clamp(0.0, 1.0);
            let alpha = match kind {
                // Verrouillé : anneau et point central.
                TrayKind::Locked => {
                    let ring = disc * (d - 9.5).clamp(0.0, 1.0);
                    ring.max((4.0 - d).clamp(0.0, 1.0))
                }
                // Pause : deux barres verticales dans le disque.
                TrayKind::Paused => {
                    let dx = (x as f32 - center).abs();
                    let dy = (y as f32 - center).abs();
                    let bar = dy < 6.0 && (2.5..=5.5).contains(&dx);
                    if bar { 0.0 } else { disc }
                }
                _ => disc,
            };
            let i = (y * SIZE + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_distinct_32x32_rgba() {
        let icons: Vec<Vec<u8>> = TrayKind::ALL.iter().map(|k| tray_icon_rgba(*k)).collect();
        for icon in &icons {
            assert_eq!(icon.len(), 32 * 32 * 4);
            // Coins transparents, matière au milieu de l'anneau.
            assert_eq!(icon[3], 0);
            assert!(icon[(4 * 32 + 16) * 4 + 3] > 200);
        }
        for i in 0..icons.len() {
            for j in i + 1..icons.len() {
                assert_ne!(icons[i], icons[j]);
            }
        }
    }
}
