//! Échantillonneur affine unique (§7.2) : sert au letterbox 192×192 de la paume
//! et au recadrage 224×224 des points-clés. Écrit directement un tenseur f32
//! NHWC normalisé dans [0, 1], interpolation bilinéaire.

use crate::Image;

/// Transformation affine 2×3 : `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub m: [f32; 6],
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    };

    pub fn apply(&self, x: f32, y: f32) -> [f32; 2] {
        let m = &self.m;
        [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]]
    }

    /// Compose avec le miroir horizontal d'une image de largeur `width` :
    /// le résultat envoie vers l'image source non retournée (EF-07).
    pub fn then_mirror(mut self, width: usize) -> Self {
        let w = width as f32 - 1.0;
        self.m[0] = -self.m[0];
        self.m[1] = -self.m[1];
        self.m[2] = w - self.m[2];
        self
    }

    /// Letterbox centré : pixels d'un carré `size` → pixels de l'image affichée.
    pub fn letterbox(width: usize, height: usize, size: usize) -> Self {
        let scale = size as f32 / width.max(height) as f32;
        let pad_x = (size as f32 - width as f32 * scale) * 0.5;
        let pad_y = (size as f32 - height as f32 * scale) * 0.5;
        let inv = 1.0 / scale;
        // Centres de pixels alignés : x_src + 0,5 = (x_dst + 0,5 − pad) / échelle.
        Affine {
            m: [
                inv,
                0.0,
                (0.5 - pad_x) * inv - 0.5,
                0.0,
                inv,
                (0.5 - pad_y) * inv - 0.5,
            ],
        }
    }
}

/// Région d'intérêt carrée et orientée, en pixels de l'image affichée.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    pub cx: f32,
    pub cy: f32,
    pub size: f32,
    /// Rotation en radians : 0 quand la main pointe vers le haut de l'image,
    /// positive dans le sens horaire.
    pub rotation: f32,
}

impl Roi {
    /// Rotation qui amène la direction `(dx, dy)` (poignet → doigts) vers le haut du recadrage.
    pub fn rotation_from_direction(dx: f32, dy: f32) -> f32 {
        dx.atan2(-dy)
    }

    /// Axes du recadrage exprimés dans l'image : (x du recadrage, y du recadrage).
    pub fn axes(&self) -> ([f32; 2], [f32; 2]) {
        let (s, c) = self.rotation.sin_cos();
        ([c, s], [-s, c])
    }

    /// Coordonnées continues du recadrage (`[0, crop]`) → image affichée.
    pub fn crop_to_image(&self, u: f32, v: f32, crop: f32) -> [f32; 2] {
        let (ex, ey) = self.axes();
        let lx = (u / crop - 0.5) * self.size;
        let ly = (v / crop - 0.5) * self.size;
        [
            self.cx + lx * ex[0] + ly * ey[0],
            self.cy + lx * ex[1] + ly * ey[1],
        ]
    }

    /// Matrice indices de pixels du recadrage `crop × crop` → pixels de l'image affichée.
    pub fn to_affine(&self, crop: usize) -> Affine {
        let (ex, ey) = self.axes();
        let k = self.size / crop as f32;
        // Le centre du pixel d'indice u est à la coordonnée continue u + 0,5.
        let origin = self.crop_to_image(0.5, 0.5, crop as f32);
        Affine {
            m: [
                ex[0] * k,
                ey[0] * k,
                origin[0] - 0.5,
                ex[1] * k,
                ey[1] * k,
                origin[1] - 0.5,
            ],
        }
    }

    /// Boîte englobante alignée sur les axes `[x0, y0, x1, y1]`.
    pub fn bounds(&self) -> [f32; 4] {
        let corners = [
            self.crop_to_image(0.0, 0.0, 1.0),
            self.crop_to_image(1.0, 0.0, 1.0),
            self.crop_to_image(0.0, 1.0, 1.0),
            self.crop_to_image(1.0, 1.0, 1.0),
        ];
        bounds_of(corners.iter().copied())
    }

    /// Coins du carré dans l'image, dans l'ordre haut-gauche, haut-droit, bas-droit, bas-gauche.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        [
            self.crop_to_image(0.0, 0.0, 1.0),
            self.crop_to_image(1.0, 0.0, 1.0),
            self.crop_to_image(1.0, 1.0, 1.0),
            self.crop_to_image(0.0, 1.0, 1.0),
        ]
    }

    /// Région carrée couvrant un nuage de points, alignée sur `rotation`,
    /// décalée de `shift_y` hauteurs le long de l'axe de la main et agrandie de `scale`.
    pub fn around_points(
        points: impl Iterator<Item = [f32; 2]>,
        rotation: f32,
        shift_y: f32,
        scale: f32,
    ) -> Roi {
        let base = Roi {
            cx: 0.0,
            cy: 0.0,
            size: 1.0,
            rotation,
        };
        let (ex, ey) = base.axes();
        // Boîte englobante dans le repère tourné.
        let [x0, y0, x1, y1] =
            bounds_of(points.map(|p| [p[0] * ex[0] + p[1] * ex[1], p[0] * ey[0] + p[1] * ey[1]]));
        let (w, h) = (x1 - x0, y1 - y0);
        let mx = (x0 + x1) * 0.5;
        let my = (y0 + y1) * 0.5 + shift_y * h;
        Roi {
            cx: mx * ex[0] + my * ey[0],
            cy: mx * ex[1] + my * ey[1],
            size: w.max(h) * scale,
            rotation,
        }
    }
}

pub fn bounds_of(points: impl Iterator<Item = [f32; 2]>) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in points {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

/// Intersection sur union de deux boîtes `[x0, y0, x1, y1]`.
pub fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Remplit `out` (tenseur NHWC `out_h × out_w × 3`, valeurs dans [0, 1]) en
/// échantillonnant `src` : `transform` envoie un indice de pixel de sortie vers
/// une position dans l'image source. Hors de l'image : noir.
pub fn sample_affine(
    src: &Image<'_>,
    transform: &Affine,
    out_w: usize,
    out_h: usize,
    out: &mut [f32],
) {
    assert_eq!(out.len(), out_w * out_h * 3, "taille du tenseur de sortie");
    const INV: f32 = 1.0 / 255.0;
    let m = &transform.m;
    let stride = src.width * 3;
    let max_x = src.width as f32 - 1.0;
    let max_y = src.height as f32 - 1.0;
    for (row, line) in out.chunks_exact_mut(out_w * 3).enumerate() {
        // Coordonnées incrémentales le long de la ligne.
        let mut sx = m[1] * row as f32 + m[2];
        let mut sy = m[4] * row as f32 + m[5];
        for px in line.as_chunks_mut::<3>().0 {
            if sx >= 0.0 && sy >= 0.0 && sx < max_x && sy < max_y {
                let x0 = sx as usize;
                let y0 = sy as usize;
                let fx = sx - x0 as f32;
                let fy = sy - y0 as f32;
                let i = y0 * stride + x0 * 3;
                let top = &src.data[i..i + 6];
                let bottom = &src.data[i + stride..i + stride + 6];
                let w00 = (1.0 - fx) * (1.0 - fy) * INV;
                let w10 = fx * (1.0 - fy) * INV;
                let w01 = (1.0 - fx) * fy * INV;
                let w11 = fx * fy * INV;
                for c in 0..3 {
                    px[c] = f32::from(top[c]) * w00
                        + f32::from(top[c + 3]) * w10
                        + f32::from(bottom[c]) * w01
                        + f32::from(bottom[c + 3]) * w11;
                }
            } else {
                px.fill(0.0);
            }
            sx += m[0];
            sy += m[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Vec<u8> {
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                data[(y * w + x) * 3] = x as u8;
                data[(y * w + x) * 3 + 1] = y as u8;
                data[(y * w + x) * 3 + 2] = 255;
            }
        }
        data
    }

    #[test]
    fn identity_copies_pixels() {
        let data = gradient(16, 12);
        let img = Image::new(&data, 16, 12);
        let mut out = vec![0.0; 8 * 8 * 3];
        sample_affine(&img, &Affine::IDENTITY, 8, 8, &mut out);
        assert!((out[(3 * 8 + 5) * 3] - 5.0 / 255.0).abs() < 1e-6);
        assert!((out[(3 * 8 + 5) * 3 + 1] - 3.0 / 255.0).abs() < 1e-6);
        assert!((out[(3 * 8 + 5) * 3 + 2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mirror_flips_horizontally_without_copy() {
        let data = gradient(16, 12);
        let img = Image::new(&data, 16, 12);
        let mut out = vec![0.0; 8 * 8 * 3];
        sample_affine(&img, &Affine::IDENTITY.then_mirror(16), 8, 8, &mut out);
        // Le pixel affiché x = 2 vient du pixel source x = 13.
        assert!((out[2 * 3] - 13.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn bilinear_interpolates() {
        let data = gradient(16, 12);
        let img = Image::new(&data, 16, 12);
        let shifted = Affine {
            m: [1.0, 0.0, 0.5, 0.0, 1.0, 0.25],
        };
        let mut out = vec![0.0; 4 * 4 * 3];
        sample_affine(&img, &shifted, 4, 4, &mut out);
        assert!((out[0] - 0.5 / 255.0).abs() < 1e-6);
        assert!((out[1] - 0.25 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn outside_is_black() {
        let data = gradient(16, 12);
        let img = Image::new(&data, 16, 12);
        let far = Affine {
            m: [1.0, 0.0, 100.0, 0.0, 1.0, -50.0],
        };
        let mut out = vec![1.0; 4 * 4 * 3];
        sample_affine(&img, &far, 4, 4, &mut out);
        assert!(out.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn letterbox_is_centered() {
        let t = Affine::letterbox(1280, 720, 192);
        // Le centre du carré tombe au centre de l'image.
        let c = t.apply(95.5, 95.5);
        assert!(
            (c[0] - 639.5).abs() < 1e-2 && (c[1] - 359.5).abs() < 1e-2,
            "{c:?}"
        );
        // La première ligne utile est à (192 − 108) / 2 = 42.
        assert!(t.apply(0.0, 41.0)[1] < 0.0);
        assert!(t.apply(0.0, 42.0)[1] >= 0.0);
    }

    #[test]
    fn roi_rotation_convention() {
        assert!(Roi::rotation_from_direction(0.0, -1.0).abs() < 1e-6);
        let right = Roi::rotation_from_direction(1.0, 0.0);
        assert!((right - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        // Main pointant vers la droite : le haut du recadrage regarde à droite dans l'image.
        let roi = Roi {
            cx: 100.0,
            cy: 100.0,
            size: 50.0,
            rotation: right,
        };
        let top = roi.crop_to_image(112.0, 0.0, 224.0);
        assert!(
            (top[0] - 125.0).abs() < 1e-3 && (top[1] - 100.0).abs() < 1e-3,
            "{top:?}"
        );
    }

    #[test]
    fn roi_affine_matches_continuous_mapping() {
        let roi = Roi {
            cx: 320.0,
            cy: 240.0,
            size: 180.0,
            rotation: 0.6,
        };
        let a = roi.to_affine(224);
        for (u, v) in [(0usize, 0usize), (100, 37), (223, 223)] {
            let from_affine = a.apply(u as f32, v as f32);
            let continuous = roi.crop_to_image(u as f32 + 0.5, v as f32 + 0.5, 224.0);
            assert!((from_affine[0] + 0.5 - continuous[0]).abs() < 1e-2);
            assert!((from_affine[1] + 0.5 - continuous[1]).abs() < 1e-2);
        }
    }

    #[test]
    fn roi_around_points_is_square_and_shifted() {
        let pts = [[10.0, 20.0], [30.0, 20.0], [10.0, 60.0], [30.0, 60.0]];
        let roi = Roi::around_points(pts.into_iter(), 0.0, -0.5, 2.0);
        assert!((roi.cx - 20.0).abs() < 1e-4);
        assert!((roi.cy - 20.0).abs() < 1e-4, "{}", roi.cy);
        assert!((roi.size - 80.0).abs() < 1e-4);
    }

    #[test]
    fn iou_basics() {
        assert_eq!(iou(&[0.0, 0.0, 1.0, 1.0], &[2.0, 2.0, 3.0, 3.0]), 0.0);
        assert!((iou(&[0.0, 0.0, 2.0, 2.0], &[0.0, 0.0, 2.0, 2.0]) - 1.0).abs() < 1e-6);
        assert!((iou(&[0.0, 0.0, 2.0, 2.0], &[1.0, 0.0, 3.0, 2.0]) - 1.0 / 3.0).abs() < 1e-6);
    }
}
