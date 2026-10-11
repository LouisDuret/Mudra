//! Fenêtre glissante par piste pour les gestes dynamiques (EF-21) : tampon des
//! dernières trames, ré-échantillonné à fréquence fixe d'après les horodatages.

use std::collections::VecDeque;

use crate::Landmarks;

#[derive(Debug, Clone, Copy)]
pub struct WindowSample {
    /// Points image, repère isotrope (voir [`crate::HandSample::image`]).
    pub image: Landmarks,
    pub world: Landmarks,
}

/// Un trou plus long que ce délai entre deux trames invalide l'historique.
const MAX_GAP_US: u64 = 150_000;

#[derive(Debug, Clone)]
pub struct SlidingWindow {
    samples: VecDeque<(u64, WindowSample)>,
    window_len: usize,
    period_us: f64,
}

impl SlidingWindow {
    pub fn new(window_len: usize, sample_rate_hz: f32) -> Self {
        Self {
            samples: VecDeque::with_capacity(256),
            window_len: window_len.max(2),
            period_us: 1e6 / f64::from(sample_rate_hz.max(1.0)),
        }
    }

    pub fn window_len(&self) -> usize {
        self.window_len
    }

    /// Vide la fenêtre (perte de piste, geste déclenché : EF-32).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    fn span_us(&self) -> u64 {
        ((self.window_len - 1) as f64 * self.period_us) as u64
    }

    pub fn push(&mut self, t_us: u64, sample: WindowSample) {
        if let Some(&(last, _)) = self.samples.back() {
            if t_us <= last {
                return;
            }
            if t_us - last > MAX_GAP_US {
                self.samples.clear();
            }
        }
        // Garde un échantillon antérieur au début de la fenêtre pour l'interpolation.
        let keep_from = t_us.saturating_sub(self.span_us() + MAX_GAP_US);
        while self.samples.len() > 2 && self.samples[1].0 < keep_from {
            self.samples.pop_front();
        }
        if self.samples.len() == self.samples.capacity() {
            self.samples.pop_front();
        }
        self.samples.push_back((t_us, sample));
    }

    /// Ré-échantillonne les derniers instants dans `out` : `window_len` si
    /// l'historique les couvre, sinon ceux qu'il couvre. Renvoie `false` s'il
    /// y en a moins de `min_len`.
    pub fn resample(&self, min_len: usize, out: &mut Vec<WindowSample>) -> bool {
        out.clear();
        let (Some(&(first, _)), Some(&(last, _))) = (self.samples.front(), self.samples.back())
        else {
            return false;
        };
        // Tolérance d'une demi-période sur le début de la fenêtre.
        let covered = ((last - first) as f64 / self.period_us + 0.5).floor() as usize + 1;
        let len = covered.min(self.window_len);
        if len < min_len.clamp(2, self.window_len) {
            return false;
        }
        let start = last as f64 - (len - 1) as f64 * self.period_us;
        let mut cursor = 0;
        for i in 0..len {
            let t = start + i as f64 * self.period_us;
            while cursor + 1 < self.samples.len() && (self.samples[cursor + 1].0 as f64) < t {
                cursor += 1;
            }
            let (t0, a) = &self.samples[cursor];
            let sample = match self.samples.get(cursor + 1) {
                Some((t1, b)) if t > *t0 as f64 => {
                    let w = ((t - *t0 as f64) / (*t1 - *t0) as f64).clamp(0.0, 1.0) as f32;
                    lerp(a, b, w)
                }
                _ => *a,
            };
            out.push(sample);
        }
        true
    }
}

pub(crate) fn lerp(a: &WindowSample, b: &WindowSample, w: f32) -> WindowSample {
    let mut out = *a;
    for i in 0..a.image.len() {
        for c in 0..3 {
            out.image[i][c] = a.image[i][c] + (b.image[i][c] - a.image[i][c]) * w;
            out.world[i][c] = a.world[i][c] + (b.world[i][c] - a.world[i][c]) * w;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(x: f32) -> WindowSample {
        WindowSample {
            image: [[x, 0.0, 0.0]; 21],
            world: [[0.0; 3]; 21],
        }
    }

    #[test]
    fn not_ready_until_window_is_covered() {
        let mut w = SlidingWindow::new(32, 60.0);
        let mut out = Vec::new();
        for i in 0..31u64 {
            w.push(i * 16_667, sample(i as f32));
            assert!(!w.resample(32, &mut out), "trame {i}");
        }
        w.push(31 * 16_667, sample(31.0));
        assert!(w.resample(32, &mut out));
        assert_eq!(out.len(), 32);
        assert!((out[0].image[0][0] - 0.0).abs() < 0.05);
        assert!((out[31].image[0][0] - 31.0).abs() < 1e-4);
    }

    #[test]
    fn resamples_30_fps_to_60_hz() {
        let mut w = SlidingWindow::new(32, 60.0);
        // x = temps en secondes, capturé à 30 fps.
        for i in 0..40u64 {
            let t = i * 33_333;
            w.push(t, sample(t as f32 * 1e-6));
        }
        let mut out = Vec::new();
        assert!(w.resample(32, &mut out));
        let last_t = 39.0 * 33_333.0 * 1e-6;
        for (i, s) in out.iter().enumerate() {
            let expected = last_t - (31 - i) as f32 / 60.0;
            assert!((s.image[0][0] - expected).abs() < 1e-3, "échantillon {i}");
        }
    }

    #[test]
    fn gap_invalidates_history() {
        let mut w = SlidingWindow::new(8, 60.0);
        for i in 0..20u64 {
            w.push(i * 16_667, sample(0.0));
        }
        let mut out = Vec::new();
        assert!(w.resample(8, &mut out));
        w.push(20 * 16_667 + 400_000, sample(0.0));
        assert!(!w.resample(8, &mut out));
    }

    #[test]
    fn a_partial_window_is_served_from_its_minimum_length() {
        // Fenêtre de 64 instants, utilisable dès 32.
        let mut w = SlidingWindow::new(64, 60.0);
        let mut out = Vec::new();
        for i in 0..40u64 {
            w.push(i * 16_667, sample(i as f32));
            assert_eq!(w.resample(32, &mut out), i >= 31, "trame {i}");
        }
        // Les 40 instants couverts, le plus récent en dernier.
        assert_eq!(out.len(), 40);
        assert!((out[0].image[0][0] - 0.0).abs() < 0.05);
        assert!((out[39].image[0][0] - 39.0).abs() < 1e-4);
        for i in 40..100u64 {
            w.push(i * 16_667, sample(i as f32));
        }
        assert!(w.resample(32, &mut out));
        assert_eq!(out.len(), 64);
        assert!((out[63].image[0][0] - 99.0).abs() < 1e-4);
        assert!((out[0].image[0][0] - 36.0).abs() < 0.05);
    }

    #[test]
    fn memory_stays_bounded() {
        let mut w = SlidingWindow::new(32, 60.0);
        for i in 0..100_000u64 {
            w.push(i * 8_333, sample(0.0));
        }
        assert!(w.samples.len() <= 256);
        assert_eq!(w.samples.capacity(), 256);
    }
}
