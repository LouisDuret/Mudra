//! Grandeurs géométriques calculées sur les points-clés (EF-22, Annexe A).

use crate::Landmarks;

pub const WRIST: usize = 0;
pub const THUMB_CMC: usize = 1;
pub const THUMB_MCP: usize = 2;
pub const THUMB_IP: usize = 3;
pub const THUMB_TIP: usize = 4;
pub const INDEX_MCP: usize = 5;
pub const INDEX_TIP: usize = 8;
pub const MIDDLE_MCP: usize = 9;
pub const MIDDLE_TIP: usize = 12;
pub const RING_MCP: usize = 13;
pub const RING_TIP: usize = 16;
pub const PINKY_MCP: usize = 17;
pub const PINKY_TIP: usize = 20;

/// (MCP, PIP, DIP, bout) pour index, majeur, annulaire, auriculaire.
pub const FINGERS: [[usize; 4]; 4] = [
    [5, 6, 7, 8],
    [9, 10, 11, 12],
    [13, 14, 15, 16],
    [17, 18, 19, 20],
];

pub type V3 = [f32; 3];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn norm(a: V3) -> f32 {
    dot(a, a).sqrt()
}

pub fn dist(a: V3, b: V3) -> f32 {
    norm(sub(a, b))
}

pub fn dist2d(a: V3, b: V3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Angle de flexion en degrés à l'articulation `b` : 0 = segments alignés.
pub fn bend_deg(a: V3, b: V3, c: V3) -> f32 {
    let u = sub(b, a);
    let v = sub(c, b);
    let d = norm(u) * norm(v);
    if d < 1e-12 {
        return 0.0;
    }
    (dot(u, v) / d).clamp(-1.0, 1.0).acos().to_degrees()
}

/// 0 sous `e0`, 1 au-dessus de `e1`, transition lisse entre les deux.
/// Avec `e0 > e1`, la rampe est décroissante.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 - e0).abs() < 1e-12 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Taille de paume de référence : distance poignet → MCP du majeur (3D).
pub fn palm_size(lm: &Landmarks) -> f32 {
    dist(lm[WRIST], lm[MIDDLE_MCP])
}

/// Taille de paume dans le plan image.
pub fn palm_size_2d(lm: &Landmarks) -> f32 {
    dist2d(lm[WRIST], lm[MIDDLE_MCP])
}

/// Centre de la main : moyenne du poignet et des quatre MCP.
pub fn hand_center(lm: &Landmarks) -> [f32; 2] {
    let ids = [WRIST, INDEX_MCP, MIDDLE_MCP, RING_MCP, PINKY_MCP];
    let mut c = [0.0f32; 2];
    for i in ids {
        c[0] += lm[i][0];
        c[1] += lm[i][1];
    }
    [c[0] / ids.len() as f32, c[1] / ids.len() as f32]
}

/// Écart pouce-index normalisé par la taille de paume (EF-22, EF-51).
pub fn pinch_ratio(world: &Landmarks) -> f32 {
    dist(world[THUMB_TIP], world[INDEX_TIP]) / palm_size(world).max(1e-6)
}

/// Flexion cumulée (PIP + DIP) d'un doigt long, en degrés.
pub fn finger_curl_deg(world: &Landmarks, finger: usize) -> f32 {
    let [mcp, pip, dip, tip] = FINGERS[finger];
    bend_deg(world[mcp], world[pip], world[dip]) + bend_deg(world[pip], world[dip], world[tip])
}

/// Angle du poignet dans le plan image (direction poignet → MCP du majeur),
/// en degrés : 0 = main vers le haut, positif dans le sens horaire.
pub fn wrist_angle_deg(image: &Landmarks) -> f32 {
    let dx = image[MIDDLE_MCP][0] - image[WRIST][0];
    let dy = image[MIDDLE_MCP][1] - image[WRIST][1];
    dx.atan2(-dy).to_degrees()
}

/// Constante de temps du lissage de la vitesse, en secondes : six images à
/// 60 i/s, assez pour que le tremblement du suivi s'annule.
const SPEED_SMOOTHING_S: f32 = 0.1;

/// Estime la vitesse du centre de la main en tailles de paume par seconde (EF-33).
/// C'est le vecteur vitesse qui est lissé, pas sa norme : le tremblement du
/// suivi autour d'une main immobile s'annule, un vrai déplacement reste.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpeedEstimator {
    prev: Option<([f32; 2], u64)>,
    velocity: [f32; 2],
}

impl SpeedEstimator {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `center` et `palm` sont dans la même unité (repère image isotrope).
    pub fn update(&mut self, center: [f32; 2], palm: f32, now_us: u64) -> f32 {
        if let Some((prev, t)) = self.prev
            && now_us > t
        {
            let dt = (now_us - t) as f32 * 1e-6;
            let scale = 1.0 / (palm.max(1e-6) * dt);
            let a = (dt / SPEED_SMOOTHING_S).min(1.0);
            for axis in 0..2 {
                let instant = (center[axis] - prev[axis]) * scale;
                self.velocity[axis] += a * (instant - self.velocity[axis]);
            }
        }
        self.prev = Some((center, now_us));
        self.velocity[0].hypot(self.velocity[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bend_of_straight_and_right_angle() {
        assert!(bend_deg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]) < 1e-3);
        let a = bend_deg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]);
        assert!((a - 90.0).abs() < 1e-3);
        let a = bend_deg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        assert!((a - 180.0).abs() < 1e-3);
    }

    #[test]
    fn speed_ignores_tracking_jitter_but_follows_a_real_move() {
        // Paume de 120 px, 60 i/s. Le centre d'une main immobile mal suivie
        // saute de plusieurs pixels à chaque image.
        const PALM: f32 = 120.0;
        let mut seed = 7u32;
        let mut jitter = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) * 9.0
        };
        let mut still = SpeedEstimator::default();
        let mut worst = 0.0f32;
        let mut travelled = 0.0;
        let mut prev = [640.0, 360.0];
        for frame in 0..600u64 {
            let center = [640.0 + jitter(), 360.0 + jitter()];
            travelled += (center[0] - prev[0]).hypot(center[1] - prev[1]);
            prev = center;
            let speed = still.update(center, PALM, frame * 16_667);
            if frame > 12 {
                worst = worst.max(speed);
            }
        }
        // Image par image, ce tremblement vaut plus de 2,5 paumes par seconde.
        assert!(travelled / PALM / 10.0 > 2.5, "{travelled}");
        assert!(worst < 2.0, "{worst}");

        // La même main qui part à 6 paumes par seconde est vue en mouvement
        // en moins de 100 ms, et sa vitesse est bien mesurée ensuite.
        let mut moving = SpeedEstimator::default();
        let mut reported = Vec::new();
        for frame in 0..30u64 {
            let x = 200.0 + 6.0 * PALM * frame as f32 / 60.0;
            let center = [x + jitter(), 360.0 + jitter()];
            reported.push(moving.update(center, PALM, frame * 16_667));
        }
        assert!(reported[6] > 2.5, "{reported:?}");
        assert!((reported[29] - 6.0).abs() < 1.5, "{reported:?}");
    }

    #[test]
    fn smoothstep_both_directions() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(smoothstep(80.0, 40.0, 30.0), 1.0);
        assert_eq!(smoothstep(80.0, 40.0, 90.0), 0.0);
    }

    #[test]
    fn speed_in_palm_units() {
        let mut s = SpeedEstimator::default();
        let mut v = 0.0;
        // 0,1 unité toutes les 1/60 s, paume de 0,1 : 60 paumes/s.
        for i in 0..60 {
            v = s.update([i as f32 * 0.1, 0.0], 0.1, i * 16_667);
        }
        assert!((v - 60.0).abs() < 1.0, "{v}");
    }

    #[test]
    fn wrist_angle_reference() {
        let mut lm = [[0.0f32; 3]; 21];
        lm[WRIST] = [0.5, 0.8, 0.0];
        lm[MIDDLE_MCP] = [0.5, 0.6, 0.0];
        assert!(wrist_angle_deg(&lm).abs() < 1e-3);
        lm[MIDDLE_MCP] = [0.7, 0.8, 0.0];
        assert!((wrist_angle_deg(&lm) - 90.0).abs() < 1e-3);
    }
}
