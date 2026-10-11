//! Icônes vectorielles de l'interface : des chemins SVG, aplatis en lignes
//! brisées et dessinés à la taille demandée.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// Un morceau d'icône, dans le repère de sa boîte (`Icon::view`).
pub enum Part {
    /// Chemin tracé, avec son épaisseur.
    Stroke(&'static str, f32),
    /// Chemin convexe rempli.
    Fill(&'static str),
    /// Rectangle arrondi tracé : x, y, largeur, hauteur, rayon ; épaisseur.
    RectStroke([f32; 5], f32),
    RectFill([f32; 5]),
    /// Disque plein : centre et rayon.
    Dot([f32; 3]),
}

pub struct Icon {
    /// Côté de la boîte carrée dans laquelle l'icône est décrite.
    view: f32,
    parts: &'static [Part],
}

const HAND_PATH: &str = "M8 12V5.5a1.5 1.5 0 0 1 3 0V11M11 10.5V4a1.5 1.5 0 0 1 3 0v6.5M14 10.5V5.5a1.5 \
     1.5 0 0 1 3 0V13c0 4-2.5 7-6 7-2.6 0-4.2-1.4-5.5-3.6L4 13.6a1.5 1.5 0 0 1 2.5-1.6L8 14";

pub const HAND: Icon = Icon {
    view: 24.0,
    parts: &[Part::Stroke(HAND_PATH, 1.7)],
};
pub const HAND_THIN: Icon = Icon {
    view: 24.0,
    parts: &[Part::Stroke(HAND_PATH, 1.4)],
};
pub const LOCK: Icon = Icon {
    view: 14.0,
    parts: &[
        Part::RectStroke([2.5, 6.0, 9.0, 6.5, 1.8], 1.4),
        Part::Stroke("M4.5 6V4.5a2.5 2.5 0 0 1 5 0V6", 1.4),
    ],
};
pub const UNLOCK: Icon = Icon {
    view: 14.0,
    parts: &[
        Part::RectStroke([2.5, 6.0, 9.0, 6.5, 1.8], 1.4),
        Part::Stroke("M4.5 6V4.5a2.5 2.5 0 0 1 4.7-1.2", 1.4),
    ],
};
pub const CAMERA: Icon = Icon {
    view: 16.0,
    parts: &[
        Part::RectStroke([1.5, 4.0, 10.0, 8.0, 2.0], 1.4),
        Part::Stroke("m11.5 7 3-2v6l-3-2", 1.4),
    ],
};
pub const CHEVRON: Icon = Icon {
    view: 10.0,
    parts: &[Part::Stroke("M2.5 3.5 5 6l2.5-2.5", 1.4)],
};
pub const MOON: Icon = Icon {
    view: 18.0,
    parts: &[Part::Stroke(
        "M15 11.2A6.5 6.5 0 0 1 6.8 3a6.5 6.5 0 1 0 8.2 8.2Z",
        1.6,
    )],
};
pub const SUN: Icon = Icon {
    view: 18.0,
    parts: &[
        Part::Dot([9.0, 9.0, 3.4]),
        Part::Stroke(
            "M9 1.5v1.8M9 14.7v1.8M1.5 9h1.8M14.7 9h1.8M3.7 3.7l1.3 1.3M13 13l1.3 1.3M3.7 14.3 5 \
             13M13 5l1.3-1.3",
            1.6,
        ),
    ],
};
pub const PAUSE: Icon = Icon {
    view: 12.0,
    parts: &[
        Part::RectFill([2.0, 1.5, 3.0, 9.0, 1.0]),
        Part::RectFill([7.0, 1.5, 3.0, 9.0, 1.0]),
    ],
};
pub const PLAY: Icon = Icon {
    view: 12.0,
    parts: &[Part::Fill("M3 1.8v8.4L10 6 3 1.8Z")],
};
pub const RECORD: Icon = Icon {
    view: 12.0,
    parts: &[Part::Dot([6.0, 6.0, 4.2])],
};
pub const RELOAD: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke("M13 8a5 5 0 1 1-1.5-3.6M13 2.5v2.5h-2.5", 1.6)],
};
pub const FOLDER: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke(
        "M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 1.5h4.5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 \
         1.5h-9A1.5 1.5 0 0 1 2 11.5v-7Z",
        1.5,
    )],
};
pub const FILE: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke("M4 2h6l3 3v9H4V2ZM6.5 8h4M6.5 10.5h4", 1.5)],
};
pub const UNDO: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke(
        "M5.5 4 2.5 7l3 3M3 7h7a3.5 3.5 0 0 1 0 7H8",
        1.5,
    )],
};
pub const PLUS: Icon = Icon {
    view: 12.0,
    parts: &[Part::Stroke("M6 1.5v9M1.5 6h9", 1.8)],
};
pub const CROSS: Icon = Icon {
    view: 12.0,
    parts: &[Part::Stroke("M2.5 2.5l7 7M9.5 2.5l-7 7", 1.7)],
};
pub const PENCIL: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke("M10.5 2.5l3 3L6 13H3v-3l7.5-7.5Z", 1.5)],
};
pub const TRASH: Icon = Icon {
    view: 16.0,
    parts: &[Part::Stroke(
        "M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.7 8.5h5.6l.7-8.5",
        1.5,
    )],
};
/// Flèche vers la gauche ; se tourne pour les autres directions.
pub const ARROW: Icon = Icon {
    view: 18.0,
    parts: &[Part::Stroke("M15 9H4M8 4.5 3.5 9 8 13.5", 1.7)],
};
/// Rotation dans le sens horaire ; se retourne pour l'autre sens.
pub const TURN: Icon = Icon {
    view: 18.0,
    parts: &[Part::Stroke(
        "M14.5 9a5.5 5.5 0 1 1-1.7-4M14.5 2.8v2.9h-2.9",
        1.7,
    )],
};

/// Sous-chemin aplati : ses points, et s'il se referme.
struct Sub {
    points: Vec<[f32; 2]>,
    closed: bool,
}

/// Lit un nombre à partir de `i` ; `None` si une commande ou la fin suit.
fn number(bytes: &[u8], i: &mut usize) -> Option<f32> {
    while *i < bytes.len() && (bytes[*i] == b' ' || bytes[*i] == b',') {
        *i += 1;
    }
    let start = *i;
    let mut seen_dot = false;
    while *i < bytes.len() {
        match bytes[*i] {
            b'-' | b'+' if *i == start => {}
            b'.' if !seen_dot => seen_dot = true,
            b'0'..=b'9' => {}
            _ => break,
        }
        *i += 1;
    }
    std::str::from_utf8(&bytes[start..*i]).ok()?.parse().ok()
}

fn angle_between(u: [f32; 2], v: [f32; 2]) -> f32 {
    (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1])
}

/// Arc d'ellipse SVG de `from` à `to`, aplati (rotation d'axe nulle : les
/// icônes n'en utilisent pas).
fn arc(
    points: &mut Vec<[f32; 2]>,
    from: [f32; 2],
    radii: [f32; 2],
    large: bool,
    sweep: bool,
    to: [f32; 2],
) {
    let (mut rx, mut ry) = (radii[0].abs(), radii[1].abs());
    if rx < 1e-6 || ry < 1e-6 {
        points.push(to);
        return;
    }
    let (px, py) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let lambda = (px / rx).powi(2) + (py / ry).powi(2);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let denominator = (rx * py).powi(2) + (ry * px).powi(2);
    let numerator = (rx * ry).powi(2) - denominator;
    let mut scale = (numerator / denominator).max(0.0).sqrt();
    if large == sweep {
        scale = -scale;
    }
    let (cxp, cyp) = (scale * rx * py / ry, -scale * ry * px / rx);
    let (cx, cy) = (cxp + (from[0] + to[0]) / 2.0, cyp + (from[1] + to[1]) / 2.0);
    let start = [(px - cxp) / rx, (py - cyp) / ry];
    let end = [(-px - cxp) / rx, (-py - cyp) / ry];
    let theta = angle_between([1.0, 0.0], start);
    let mut delta = angle_between(start, end);
    if !sweep && delta > 0.0 {
        delta -= TAU;
    } else if sweep && delta < 0.0 {
        delta += TAU;
    }
    let steps = ((delta.abs() / (PI / 12.0)).ceil() as usize).max(2);
    for step in 1..=steps {
        let a = theta + delta * step as f32 / steps as f32;
        points.push([cx + rx * a.cos(), cy + ry * a.sin()]);
    }
}

/// Aplatit un chemin SVG (commandes M L H V C A Z, absolues ou relatives).
fn flatten(path: &str) -> Vec<Sub> {
    let bytes = path.as_bytes();
    let mut subs: Vec<Sub> = Vec::new();
    let mut i = 0;
    let mut command = b'M';
    let mut at = [0.0f32; 2];
    let mut start = at;
    loop {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i].is_ascii_alphabetic() {
            command = bytes[i];
            i += 1;
            if command == b'Z' || command == b'z' {
                if let Some(sub) = subs.last_mut() {
                    sub.closed = true;
                }
                at = start;
                continue;
            }
        }
        let relative = command.is_ascii_lowercase();
        let origin = if relative { at } else { [0.0, 0.0] };
        let read = |i: &mut usize| number(bytes, i);
        let mut drawn = true;
        match command.to_ascii_uppercase() {
            b'M' => {
                drawn = false;
                let (Some(x), Some(y)) = (read(&mut i), read(&mut i)) else {
                    break;
                };
                at = [origin[0] + x, origin[1] + y];
                start = at;
                subs.push(Sub {
                    points: vec![at],
                    closed: false,
                });
                // Les paires suivantes sont des segments.
                command = if relative { b'l' } else { b'L' };
            }
            b'L' => {
                let (Some(x), Some(y)) = (read(&mut i), read(&mut i)) else {
                    break;
                };
                at = [origin[0] + x, origin[1] + y];
            }
            b'H' => {
                let Some(x) = read(&mut i) else { break };
                at[0] = origin[0] + x;
            }
            b'V' => {
                let Some(y) = read(&mut i) else { break };
                at[1] = origin[1] + y;
            }
            b'C' => {
                let mut c = [0.0f32; 6];
                let mut complete = true;
                for value in &mut c {
                    match read(&mut i) {
                        Some(v) => *value = v,
                        None => complete = false,
                    }
                }
                if !complete {
                    break;
                }
                let p1 = [origin[0] + c[0], origin[1] + c[1]];
                let p2 = [origin[0] + c[2], origin[1] + c[3]];
                let p3 = [origin[0] + c[4], origin[1] + c[5]];
                if let Some(sub) = subs.last_mut() {
                    for step in 1..12 {
                        let t = step as f32 / 12.0;
                        let u = 1.0 - t;
                        let blend = |k: usize| {
                            u * u * u * at[k]
                                + 3.0 * u * u * t * p1[k]
                                + 3.0 * u * t * t * p2[k]
                                + t * t * t * p3[k]
                        };
                        sub.points.push([blend(0), blend(1)]);
                    }
                }
                at = p3;
            }
            b'A' => {
                let mut a = [0.0f32; 7];
                let mut complete = true;
                for value in &mut a {
                    match read(&mut i) {
                        Some(v) => *value = v,
                        None => complete = false,
                    }
                }
                if !complete {
                    break;
                }
                let to = [origin[0] + a[5], origin[1] + a[6]];
                if let Some(sub) = subs.last_mut() {
                    arc(
                        &mut sub.points,
                        at,
                        [a[0], a[1]],
                        a[3] != 0.0,
                        a[4] != 0.0,
                        to,
                    );
                    // `arc` a déjà posé le point d'arrivée.
                    sub.points.pop();
                }
                at = to;
            }
            _ => break,
        }
        if drawn && let Some(sub) = subs.last_mut() {
            sub.points.push(at);
        }
    }
    subs
}

/// Chemins déjà aplatis, par texte de chemin.
type Flattened = Mutex<HashMap<&'static str, Arc<Vec<Sub>>>>;

fn flattened(path: &'static str) -> Arc<Vec<Sub>> {
    static CACHE: OnceLock<Flattened> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let Ok(mut cache) = cache.lock() else {
        return Arc::new(flatten(path));
    };
    cache
        .entry(path)
        .or_insert_with(|| Arc::new(flatten(path)))
        .clone()
}

/// Couleur du trait : unie, ou dégradée du coin haut gauche au coin bas droit.
#[derive(Clone, Copy)]
pub enum Ink {
    Solid(Color32),
    Gradient(Color32, Color32),
}

impl From<Color32> for Ink {
    fn from(color: Color32) -> Self {
        Ink::Solid(color)
    }
}

impl Icon {
    /// Dessine l'icône dans `rect` (carré), d'une seule couleur.
    pub fn paint(&self, painter: &Painter, rect: Rect, ink: impl Into<Ink>) {
        self.paint_turned(painter, rect, ink, 0.0, false);
    }

    /// Dessine l'icône centrée sur `center`, avec le côté donné.
    pub fn paint_at(&self, painter: &Painter, center: Pos2, size: f32, ink: impl Into<Ink>) {
        self.paint(
            painter,
            Rect::from_center_size(center, Vec2::splat(size)),
            ink,
        );
    }

    /// Comme [`Self::paint`], l'icône tournée de `angle` radians (sens
    /// horaire) autour de son centre, et retournée de gauche à droite si `flip`.
    pub fn paint_turned(
        &self,
        painter: &Painter,
        rect: Rect,
        ink: impl Into<Ink>,
        angle: f32,
        flip: bool,
    ) {
        let ink = ink.into();
        let scale = rect.width() / self.view;
        let half = self.view / 2.0;
        let (sin, cos) = angle.sin_cos();
        let place = |p: [f32; 2]| {
            let mut x = p[0] - half;
            let y = p[1] - half;
            if flip {
                x = -x;
            }
            Pos2::new(
                rect.center().x + (x * cos - y * sin) * scale,
                rect.center().y + (x * sin + y * cos) * scale,
            )
        };
        let solid = match ink {
            Ink::Solid(color) => color,
            Ink::Gradient(_, to) => to,
        };
        let stroke = |width: f32| -> egui::epaint::PathStroke {
            match ink {
                Ink::Solid(color) => Stroke::new(width * scale, color).into(),
                Ink::Gradient(from, to) => {
                    egui::epaint::PathStroke::new_uv(width * scale, move |_, p| {
                        let t = ((p.x - rect.left()) + (p.y - rect.top()))
                            / (rect.width() + rect.height());
                        from.lerp_to_gamma(to, t.clamp(0.0, 1.0))
                    })
                }
            }
        };
        for part in self.parts {
            match part {
                Part::Stroke(path, width) => {
                    for sub in flattened(path).iter() {
                        let points: Vec<Pos2> = sub.points.iter().map(|p| place(*p)).collect();
                        painter.add(if sub.closed {
                            Shape::closed_line(points, stroke(*width))
                        } else {
                            Shape::line(points, stroke(*width))
                        });
                    }
                }
                Part::Fill(path) => {
                    for sub in flattened(path).iter() {
                        let points: Vec<Pos2> = sub.points.iter().map(|p| place(*p)).collect();
                        painter.add(Shape::convex_polygon(points, solid, Stroke::NONE));
                    }
                }
                Part::RectStroke([x, y, w, h, r], width) => {
                    let (a, b) = (place([*x, *y]), place([x + w, y + h]));
                    painter.rect_stroke(
                        Rect::from_two_pos(a, b),
                        CornerRadius::same((r * scale).round() as u8),
                        Stroke::new(width * scale, solid),
                        StrokeKind::Middle,
                    );
                }
                Part::RectFill([x, y, w, h, r]) => {
                    let (a, b) = (place([*x, *y]), place([x + w, y + h]));
                    painter.rect_filled(
                        Rect::from_two_pos(a, b),
                        CornerRadius::same((r * scale).round() as u8),
                        solid,
                    );
                }
                Part::Dot([x, y, r]) => {
                    painter.circle_filled(place([*x, *y]), r * scale, solid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn relative_and_implicit_commands_follow_the_pen() {
        // Chevron : « 5 6 » puis « l2.5-2.5 », sans répéter la commande.
        let subs = flatten("M2.5 3.5 5 6l2.5-2.5");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].points, vec![[2.5, 3.5], [5.0, 6.0], [7.5, 3.5]]);
        assert!(!subs[0].closed);
        // Plus : deux traits, chacun ouvert par un M.
        let subs = flatten("M6 1.5v9M1.5 6h9");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].points, vec![[6.0, 1.5], [6.0, 10.5]]);
        assert_eq!(subs[1].points, vec![[1.5, 6.0], [10.5, 6.0]]);
        // Nombres collés : « .7 8.5 » et « l.7-8.5 ».
        let subs = flatten("M4.5 4.5l.7 8.5h5.6l.7-8.5");
        assert!(near(subs[0].points[1], [5.2, 13.0]));
        assert!(near(*subs[0].points.last().unwrap(), [11.5, 4.5]));
    }

    #[test]
    fn arcs_stay_on_their_circle_and_reach_their_end() {
        // Anse du cadenas : demi-cercle de rayon 2,5 centré en (7 ; 4,5).
        let subs = flatten("M4.5 6V4.5a2.5 2.5 0 0 1 5 0V6");
        let points = &subs[0].points;
        assert!(near(*points.last().unwrap(), [9.5, 6.0]));
        let on_arc = &points[1..points.len() - 1];
        assert!(on_arc.len() > 6);
        for p in on_arc {
            let d = ((p[0] - 7.0).powi(2) + (p[1] - 4.5).powi(2)).sqrt();
            assert!((d - 2.5).abs() < 1e-3, "{p:?}");
            assert!(p[1] <= 4.5 + 1e-3, "l'anse passe par le haut : {p:?}");
        }
        // Lune : chemin fermé, grand arc compris.
        let subs = flatten("M15 11.2A6.5 6.5 0 0 1 6.8 3a6.5 6.5 0 1 0 8.2 8.2Z");
        assert!(subs[0].closed);
        assert!(near(*subs[0].points.last().unwrap(), [15.0, 11.2]));
    }

    #[test]
    fn every_icon_path_parses_inside_its_box() {
        for icon in [
            &HAND, &HAND_THIN, &LOCK, &UNLOCK, &CAMERA, &CHEVRON, &MOON, &SUN, &PAUSE, &PLAY,
            &RECORD, &RELOAD, &FOLDER, &FILE, &UNDO, &PLUS, &CROSS, &PENCIL, &TRASH, &ARROW, &TURN,
        ] {
            for part in icon.parts {
                let (Part::Stroke(path, _) | Part::Fill(path)) = part else {
                    continue;
                };
                let subs = flatten(path);
                assert!(!subs.is_empty(), "{path}");
                for sub in &subs {
                    assert!(sub.points.len() >= 2, "{path}");
                    for p in &sub.points {
                        assert!(
                            p.iter().all(|v| (-0.5..=icon.view + 0.5).contains(v)),
                            "{path} sort de sa boîte : {p:?}"
                        );
                    }
                }
            }
        }
    }
}
