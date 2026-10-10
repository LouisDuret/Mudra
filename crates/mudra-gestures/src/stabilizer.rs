//! Stabilisation temporelle du curseur (EF-51) : une erreur de suivi déplace
//! le bout de l'index d'un coup, le temps d'une image. Une médiane sur trois
//! échantillons écarte ces sauts isolés, puis un passe-bas du premier ordre
//! amortit ce qui reste. Le retard ajouté est d'une image plus la constante
//! de temps.

/// Stabilisateur d'une coordonnée.
#[derive(Debug, Clone, Copy)]
pub struct Stabilizer {
    /// Constante de temps du passe-bas, en secondes ; 0 désactive tout.
    tau: f32,
    /// Les deux échantillons précédents, du plus ancien au plus récent.
    recent: Option<(f32, f32)>,
    smoothed: Option<f32>,
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).min(a.min(b).max(c))
}

impl Stabilizer {
    pub fn new(tau_s: f32) -> Self {
        Self {
            tau: tau_s.max(0.0),
            recent: None,
            smoothed: None,
        }
    }

    /// Filtre `x` ; `dt` est le temps écoulé depuis l'échantillon précédent, en secondes.
    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        if self.tau <= 0.0 {
            return x;
        }
        if !x.is_finite() {
            return self.smoothed.unwrap_or(x);
        }
        let median = match self.recent {
            Some((a, b)) => median3(a, b, x),
            None => x,
        };
        self.recent = Some((self.recent.map_or(x, |(_, b)| b), x));
        let out = match self.smoothed {
            Some(prev) if dt > 0.0 => prev + dt / (self.tau + dt) * (median - prev),
            Some(prev) => prev,
            None => median,
        };
        self.smoothed = Some(out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn disabled_passes_through() {
        let mut s = Stabilizer::new(0.0);
        for x in [0.0, 5.0, -3.0] {
            assert_eq!(s.filter(x, DT), x);
        }
    }

    #[test]
    fn isolated_spike_is_rejected() {
        let mut s = Stabilizer::new(0.05);
        let mut max_dev = 0.0f32;
        for i in 0..120 {
            let x = if i == 60 { 3.0 } else { 1.0 };
            max_dev = max_dev.max((s.filter(x, DT) - 1.0).abs());
        }
        assert!(max_dev < 1e-6, "écart {max_dev}");
    }

    #[test]
    fn step_is_followed_quickly() {
        let mut s = Stabilizer::new(0.05);
        for _ in 0..30 {
            s.filter(0.0, DT);
        }
        let mut y = 0.0;
        // 200 ms après l'échelon : plus de 90 % du chemin.
        for _ in 0..12 {
            y = s.filter(1.0, DT);
        }
        assert!(y > 0.9, "{y}");
    }

    #[test]
    fn steady_motion_lags_by_a_bounded_amount() {
        let mut s = Stabilizer::new(0.05);
        let speed = 2.0; // unités par seconde
        let mut lag = 0.0;
        for i in 0..240 {
            let x = i as f32 * DT * speed;
            lag = x - s.filter(x, DT);
        }
        // Une image de médiane plus la constante de temps.
        assert!((lag - speed * (DT + 0.05)).abs() < 0.01, "retard {lag}");
    }
}
