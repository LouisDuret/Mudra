//! Filtre One Euro (D-11) : passe-bas adaptatif, stable au repos, réactif en mouvement.

use mudra_config::Smoothing;

use crate::{Landmarks, NUM_LANDMARKS};

fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

/// Filtre One Euro sur un scalaire.
#[derive(Debug, Clone, Copy)]
pub struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    prev: Option<(f32, f32)>,
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32, d_cutoff: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff,
            prev: None,
        }
    }

    pub fn set_params(&mut self, min_cutoff: f32, beta: f32, d_cutoff: f32) {
        self.min_cutoff = min_cutoff;
        self.beta = beta;
        self.d_cutoff = d_cutoff;
    }

    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Filtre `x` ; `dt` est le temps écoulé depuis l'échantillon précédent, en secondes.
    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        let Some((x_prev, dx_prev)) = self.prev else {
            self.prev = Some((x, 0.0));
            return x;
        };
        if dt <= 0.0 || !x.is_finite() {
            return x_prev;
        }
        let dx = (x - x_prev) / dt;
        let dx_hat = dx_prev + alpha(self.d_cutoff, dt) * (dx - dx_prev);
        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let x_hat = x_prev + alpha(cutoff, dt) * (x - x_prev);
        self.prev = Some((x_hat, dx_hat));
        x_hat
    }
}

/// Lisse les 21 points d'une main, coordonnée par coordonnée, en unités de
/// taille de paume (§7.5) : `scale` est la taille de paume dans l'unité des points.
#[derive(Debug, Clone)]
pub struct LandmarkSmoother {
    filters: [[OneEuro; 3]; NUM_LANDMARKS],
    last_us: Option<u64>,
}

impl LandmarkSmoother {
    pub fn new(params: Smoothing) -> Self {
        Self {
            filters: [[OneEuro::new(params.min_cutoff, params.beta, params.d_cutoff); 3];
                NUM_LANDMARKS],
            last_us: None,
        }
    }

    pub fn set_params(&mut self, params: Smoothing) {
        for f in self.filters.iter_mut().flatten() {
            f.set_params(params.min_cutoff, params.beta, params.d_cutoff);
        }
    }

    /// À appeler à la perte de piste.
    pub fn reset(&mut self) {
        self.filters.iter_mut().flatten().for_each(OneEuro::reset);
        self.last_us = None;
    }

    pub fn smooth(&mut self, points: &Landmarks, scale: f32, now_us: u64) -> Landmarks {
        let dt = match self.last_us {
            Some(last) if now_us > last => (now_us - last) as f32 * 1e-6,
            Some(_) => return *points,
            None => 1.0 / 60.0,
        };
        // Un trou de plus de 250 ms : l'historique n'a plus de sens.
        if dt > 0.25 {
            self.filters.iter_mut().flatten().for_each(OneEuro::reset);
        }
        self.last_us = Some(now_us);
        let scale = scale.max(1e-6);
        let mut out = *points;
        for (i, p) in points.iter().enumerate() {
            for (c, v) in p.iter().enumerate() {
                out[i][c] = self.filters[i][c].filter(v / scale, dt) * scale;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_passes_through() {
        let mut f = OneEuro::new(1.0, 0.05, 1.0);
        assert_eq!(f.filter(3.5, 1.0 / 60.0), 3.5);
    }

    #[test]
    fn constant_signal_is_unchanged() {
        let mut f = OneEuro::new(1.0, 0.05, 1.0);
        for _ in 0..100 {
            assert!((f.filter(2.0, 1.0 / 60.0) - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn jitter_is_attenuated_at_rest() {
        let mut f = OneEuro::new(1.0, 0.0, 1.0);
        let mut max_dev = 0.0f32;
        for i in 0..600 {
            let noise = if i % 2 == 0 { 0.01 } else { -0.01 };
            let y = f.filter(1.0 + noise, 1.0 / 60.0);
            if i > 60 {
                max_dev = max_dev.max((y - 1.0).abs());
            }
        }
        assert!(max_dev < 0.003, "bruit résiduel {max_dev}");
    }

    #[test]
    fn fast_motion_lags_less_with_beta() {
        let run = |beta: f32| {
            let mut f = OneEuro::new(1.0, beta, 1.0);
            let mut y = 0.0;
            for i in 0..30 {
                y = f.filter(i as f32 * 0.5, 1.0 / 60.0);
            }
            14.5 - y
        };
        assert!(run(1.0) < run(0.0) * 0.5);
    }

    #[test]
    fn smoother_resets_after_gap() {
        let mut s = LandmarkSmoother::new(Smoothing::default());
        let a = [[0.0; 3]; NUM_LANDMARKS];
        let b = [[1.0; 3]; NUM_LANDMARKS];
        s.smooth(&a, 1.0, 0);
        let out = s.smooth(&b, 1.0, 1_000_000);
        assert_eq!(out, b);
    }
}
