//! Boîte à outils neumorphique : des surfaces de la couleur du fond, en
//! relief (ombre en bas à droite, reflet en haut à gauche) ou en creux, et
//! les widgets de l'interface bâtis dessus.

use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI, TAU};
use std::ops::RangeInclusive;

use egui::epaint::{Mesh, RectShape};
use egui::{
    Align, Color32, CornerRadius, CursorIcon, Layout, Margin, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, Ui, UiBuilder, Vec2, Widget,
};

use super::icons::{self, Icon, Ink};
use super::theme::{self, EMBER, ON_ACCENT, ORANGE, SUN, Weight, palette};

// ----- Formes -----

fn corner(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8)
}

/// Contour d'un rectangle arrondi, dans le sens horaire depuis le coin haut
/// gauche : chaque point avec sa normale sortante.
fn outline(rect: Rect, radius: f32) -> Vec<(Pos2, Vec2)> {
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let steps = ((r * 0.7).ceil() as usize).clamp(2, 14);
    let corners = [
        (Pos2::new(rect.left() + r, rect.top() + r), PI),
        (Pos2::new(rect.right() - r, rect.top() + r), 1.5 * PI),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), FRAC_PI_2),
    ];
    let mut points = Vec::with_capacity(4 * (steps + 1));
    for (center, from) in corners {
        for step in 0..=steps {
            let angle = from + FRAC_PI_2 * step as f32 / steps as f32;
            let normal = Vec2::new(angle.cos(), angle.sin());
            points.push((center + normal * r, normal));
        }
    }
    points
}

/// Ombre floue d'une surface décalée de `offset`.
fn shadow(rect: Rect, radius: f32, offset: Vec2, blur: f32, color: Color32) -> Shape {
    // Le flou d'egui est linéaire : élargi, il a l'étalement d'un flou gaussien.
    let blur = (blur * 1.6).min(rect.size().min_elem());
    RectShape::filled(rect.translate(offset), corner(radius), color)
        .with_blur_width(blur)
        .into()
}

/// Surface en relief : `depth` est le décalage de l'ombre, en points.
pub fn raised(rect: Rect, radius: f32, depth: f32) -> Shape {
    let p = palette();
    let blur = depth * 2.25;
    Shape::Vec(vec![
        shadow(rect, radius, Vec2::splat(-depth), blur, p.shine),
        shadow(rect, radius, Vec2::splat(depth), blur, p.shade),
        RectShape::filled(rect, corner(radius), p.bg).into(),
    ])
}

/// Ombre intérieure : sombre le long des bords haut et gauche, claire le long
/// des bords bas et droit, sur une bande qui s'efface vers l'intérieur.
fn inner_shadow(rect: Rect, radius: f32, depth: f32, shade: Color32, shine: Color32) -> Shape {
    let points = outline(rect, radius);
    let width = (depth * 2.75)
        .min(radius.max(3.0))
        .min(rect.size().min_elem() / 2.0);
    let mut mesh = Mesh::default();
    for (point, normal) in &points {
        // 1 en haut et à gauche, −1 en bas et à droite, 0 aux deux autres coins.
        let toward_light = (-(normal.x + normal.y) * FRAC_1_SQRT_2 * 1.6).clamp(-1.0, 1.0);
        let color = if toward_light > 0.0 {
            shade.gamma_multiply(toward_light * 0.85)
        } else {
            shine.gamma_multiply(-toward_light * 0.85)
        };
        mesh.colored_vertex(*point + *normal * 0.75, Color32::TRANSPARENT);
        mesh.colored_vertex(*point, color);
        mesh.colored_vertex(*point - *normal * width, Color32::TRANSPARENT);
    }
    let count = points.len() as u32;
    for i in 0..count {
        let (a, b) = (3 * i, 3 * ((i + 1) % count));
        for ring in 0..2 {
            mesh.add_triangle(a + ring, b + ring, a + ring + 1);
            mesh.add_triangle(b + ring, b + ring + 1, a + ring + 1);
        }
    }
    Shape::mesh(mesh)
}

/// Surface en creux, de la couleur du fond.
pub fn inset(rect: Rect, radius: f32, depth: f32) -> Shape {
    let p = palette();
    Shape::Vec(vec![
        RectShape::filled(rect, corner(radius), p.bg).into(),
        inner_shadow(rect, radius, depth, p.hollow, p.shine),
    ])
}

/// Cuvette plus sombre que le fond (vignette de l'onglet « Gestes »).
pub fn well(rect: Rect, radius: f32, depth: f32) -> Shape {
    let p = palette();
    Shape::Vec(vec![
        RectShape::filled(rect, corner(radius), p.well).into(),
        inner_shadow(rect, radius, depth, p.well_shade, p.well_shine),
    ])
}

/// Écran sombre de la caméra.
pub fn screen(rect: Rect, radius: f32) -> Shape {
    RectShape::filled(rect, corner(radius), theme::SCREEN).into()
}

/// Ombre intérieure de l'écran, à poser par-dessus l'image.
pub fn screen_shade(rect: Rect, radius: f32) -> Shape {
    inner_shadow(
        rect,
        radius,
        5.0,
        Color32::from_black_alpha(140),
        Color32::TRANSPARENT,
    )
}

/// Image aux coins arrondis.
pub fn image(texture: egui::TextureId, rect: Rect, radius: f32) -> Shape {
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    RectShape::filled(rect, corner(radius), Color32::WHITE)
        .with_texture(texture, uv)
        .into()
}

/// Rectangle arrondi rempli d'un dégradé : en diagonale (du coin haut gauche
/// au coin bas droit) ou de gauche à droite.
pub fn gradient(rect: Rect, radius: f32, from: Color32, to: Color32, diagonal: bool) -> Shape {
    let points = outline(rect, radius);
    let color_at = |p: Pos2| {
        let across = (p.x - rect.left()) / rect.width().max(1e-3);
        let t = if diagonal {
            0.5 + ((p.x - rect.center().x) + (p.y - rect.center().y))
                / (rect.width() + rect.height()).max(1e-3)
        } else {
            across
        };
        from.lerp_to_gamma(to, t.clamp(0.0, 1.0))
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for (point, normal) in &points {
        let inner = *point - *normal * 0.5;
        mesh.colored_vertex(inner, color_at(inner));
        // Bord adouci sur un point.
        mesh.colored_vertex(*point + *normal * 0.5, Color32::TRANSPARENT);
    }
    let count = points.len() as u32;
    for i in 0..count {
        let (a, b) = (1 + 2 * i, 1 + 2 * ((i + 1) % count));
        mesh.add_triangle(0, a, b);
        mesh.add_triangle(a, a + 1, b);
        mesh.add_triangle(b, a + 1, b + 1);
    }
    Shape::mesh(mesh)
}

/// Pastille au dégradé d'accent, avec son ombre portée (segment sélectionné,
/// bouton principal).
fn accent_pill(rect: Rect, radius: f32) -> Shape {
    let p = palette();
    Shape::Vec(vec![
        shadow(rect, radius, Vec2::splat(-2.0), 6.0, p.shine),
        shadow(rect, radius, Vec2::splat(3.0), 8.0, p.glow),
        gradient(rect, radius, SUN, ORANGE, true),
    ])
}

/// Fondu vertical du transparent vers `color` (bas d'une liste qui continue).
pub fn fade(rect: Rect, color: Color32) -> Shape {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.right_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    Shape::mesh(mesh)
}

// ----- Texte -----

pub fn text(text: impl Into<String>, size: f32, weight: Weight, color: Color32) -> RichText {
    RichText::new(text)
        .font(theme::font(size, weight))
        .color(color)
}

pub fn mono(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text).font(theme::mono(size)).color(color)
}

/// Titre de section : petites capitales espacées.
pub fn caps(text: &str, color: Color32) -> RichText {
    RichText::new(text)
        .font(theme::font(11.0, Weight::Bold))
        .extra_letter_spacing(1.5)
        .color(color)
}

/// Texte fait de morceaux de styles différents, replié à `width`.
pub fn rich(
    width: f32,
    line_height: f32,
    parts: &[(&str, egui::FontId, Color32)],
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    for (text, font, color) in parts {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: *color,
                line_height: Some(line_height),
                valign: Align::Center,
                ..Default::default()
            },
        );
    }
    job
}

/// Titre de section prêt à être dessiné à la main.
pub fn caps_job(text: &str, color: Color32) -> egui::text::LayoutJob {
    egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: theme::font(11.0, Weight::Bold),
            extra_letter_spacing: 1.5,
            color,
            ..Default::default()
        },
    )
}

/// Largeur d'un texte sur une ligne.
fn text_width(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x
}

// ----- Conteneurs -----

/// Dessine `add` dans un cadre dont le fond, connu une fois le contenu mis
/// en page, est fourni par `paint`. Pour une mise en page de haut en bas ou
/// de gauche à droite.
fn boxed<R>(
    ui: &mut Ui,
    width: f32,
    pad: Margin,
    min_height: f32,
    paint: impl FnOnce(Rect) -> Shape,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let origin = ui.available_rect_before_wrap().min;
    let background = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(
        origin + Vec2::new(pad.left as f32, pad.top as f32),
        Vec2::new(
            (width - pad.left as f32 - pad.right as f32).max(0.0),
            ui.available_height(),
        ),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    let result = add(&mut child);
    let bottom = (child.min_rect().bottom() + pad.bottom as f32).max(origin.y + min_height);
    let rect = Rect::from_min_max(origin, Pos2::new(origin.x + width, bottom));
    ui.painter().set(background, paint(rect));
    ui.allocate_rect(rect, Sense::hover());
    result
}

/// Carte en relief.
#[derive(Debug, Clone, Copy)]
pub struct Card {
    pub radius: f32,
    pub depth: f32,
    pub pad: Vec2,
    /// Espace vertical entre deux éléments du contenu.
    pub gap: f32,
    pub min_height: f32,
}

impl Card {
    pub const LARGE: Card = Card {
        radius: 22.0,
        depth: 8.0,
        pad: Vec2::new(22.0, 20.0),
        gap: 12.0,
        min_height: 0.0,
    };
    pub const COMPACT: Card = Card {
        radius: 20.0,
        depth: 6.0,
        pad: Vec2::new(18.0, 16.0),
        gap: 12.0,
        min_height: 0.0,
    };

    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    pub fn pad(mut self, x: f32, y: f32) -> Self {
        self.pad = Vec2::new(x, y);
        self
    }

    fn margin(self) -> Margin {
        Margin::symmetric(self.pad.x as i8, self.pad.y as i8)
    }

    /// La carte prend toute la largeur disponible.
    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        let width = ui.available_width();
        boxed(
            ui,
            width,
            self.margin(),
            self.min_height,
            |rect| raised(rect, self.radius, self.depth),
            |ui| {
                ui.spacing_mut().item_spacing.y = self.gap;
                add(ui)
            },
        )
    }

    /// `columns` cartes côte à côte, de même largeur et de même hauteur ;
    /// `add` reçoit l'indice de la carte à remplir.
    pub fn row(
        self,
        ui: &mut Ui,
        columns: usize,
        spacing: f32,
        mut add: impl FnMut(&mut Ui, usize),
    ) {
        let columns = columns.max(1);
        let origin = ui.available_rect_before_wrap().min;
        let total = ui.available_width();
        let width = (total - spacing * (columns - 1) as f32) / columns as f32;
        // Tous les fonds d'abord : l'ombre d'une carte ne couvre pas sa voisine.
        let backgrounds: Vec<_> = (0..columns)
            .map(|_| ui.painter().add(Shape::Noop))
            .collect();
        let mut bottom = origin.y + self.min_height;
        for column in 0..columns {
            let left = origin.x + (width + spacing) * column as f32;
            let inner = Rect::from_min_size(
                Pos2::new(left + self.pad.x, origin.y + self.pad.y),
                Vec2::new((width - 2.0 * self.pad.x).max(0.0), ui.available_height()),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .id_salt(("carte", column))
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
            );
            child.spacing_mut().item_spacing.y = self.gap;
            add(&mut child, column);
            bottom = bottom.max(child.min_rect().bottom() + self.pad.y);
        }
        for (column, background) in backgrounds.into_iter().enumerate() {
            let left = origin.x + (width + spacing) * column as f32;
            let rect =
                Rect::from_min_max(Pos2::new(left, origin.y), Pos2::new(left + width, bottom));
            ui.painter()
                .set(background, raised(rect, self.radius, self.depth));
        }
        ui.allocate_rect(
            Rect::from_min_max(origin, Pos2::new(origin.x + total, bottom)),
            Sense::hover(),
        );
    }
}

/// Encadré en creux de la largeur donnée.
pub fn hollow<R>(
    ui: &mut Ui,
    width: f32,
    radius: f32,
    pad: Margin,
    min_height: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    boxed(
        ui,
        width,
        pad,
        min_height,
        |rect| inset(rect, radius, 3.0),
        add,
    )
}

// ----- Boutons -----

/// Bouton en relief, avec une icône facultative devant son texte.
pub struct Button<'a> {
    text: &'a str,
    icon: Option<&'static Icon>,
    icon_size: f32,
    color: Option<Color32>,
    icon_color: Option<Color32>,
    height: f32,
    radius: f32,
    pad: f32,
    size: f32,
    weight: Weight,
    depth: f32,
    primary: bool,
}

impl<'a> Button<'a> {
    /// Bouton courant des cartes : 32 points de haut.
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            icon: None,
            icon_size: 11.0,
            color: None,
            icon_color: None,
            height: 32.0,
            radius: 11.0,
            pad: 14.0,
            size: 12.0,
            weight: Weight::Semibold,
            depth: 3.0,
            primary: false,
        }
    }

    /// Bouton d'en-tête de section ou de page : 38 points de haut.
    pub fn large(text: &'a str) -> Self {
        Self {
            height: 38.0,
            radius: 13.0,
            pad: 16.0,
            size: 12.5,
            depth: 4.0,
            icon_size: 14.0,
            ..Self::new(text)
        }
    }

    pub fn height(mut self, height: f32, radius: f32) -> Self {
        self.height = height;
        self.radius = radius;
        self
    }

    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn bold(mut self) -> Self {
        self.weight = Weight::Bold;
        self
    }

    pub fn icon(mut self, icon: &'static Icon, size: f32) -> Self {
        self.icon = Some(icon);
        self.icon_size = size;
        self
    }

    /// Couleur du texte, et de l'icône si elle n'a pas la sienne.
    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }

    pub fn icon_color(mut self, color: Color32) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Bouton principal : rempli du dégradé d'accent.
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self.weight = Weight::Bold;
        self
    }
}

impl Button<'_> {
    fn icon_gap(&self) -> f32 {
        if self.icon.is_some() && !self.text.is_empty() {
            self.icon_size * 0.6
        } else {
            0.0
        }
    }

    fn icon_width(&self) -> f32 {
        if self.icon.is_some() {
            self.icon_size
        } else {
            0.0
        }
    }

    /// Largeur que le bouton occupera.
    pub fn width(&self, ui: &Ui) -> f32 {
        let font = theme::font(self.size, self.weight);
        2.0 * self.pad + self.icon_width() + self.icon_gap() + text_width(ui, self.text, font)
    }
}

impl Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = palette();
        let font = theme::font(self.size, self.weight);
        let galley = ui
            .painter()
            .layout_no_wrap(self.text.to_owned(), font, Color32::PLACEHOLDER);
        let gap = self.icon_gap();
        let width = 2.0 * self.pad + self.icon_width() + gap + galley.size().x;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, self.height), Sense::click());
        if !ui.is_rect_visible(rect.expand(24.0)) {
            return response;
        }
        let enabled = ui.is_enabled();
        let pressed = enabled && response.is_pointer_button_down_on();
        let hovered = enabled && response.hovered();
        let painter = ui.painter();
        let color = if self.primary {
            ON_ACCENT
        } else {
            self.color.unwrap_or(p.text)
        };
        if self.primary {
            painter.add(accent_pill(rect, self.radius));
            if pressed {
                painter.add(inner_shadow(
                    rect,
                    self.radius,
                    2.0,
                    EMBER,
                    Color32::TRANSPARENT,
                ));
            }
        } else if pressed {
            painter.add(inset(rect, self.radius, self.depth * 0.8));
        } else {
            let depth = match (enabled, hovered) {
                (false, _) => self.depth * 0.5,
                (true, true) => self.depth + 1.0,
                (true, false) => self.depth,
            };
            painter.add(raised(rect, self.radius, depth));
        }
        let mut x = rect.left() + self.pad;
        if let Some(icon) = self.icon {
            let center = Pos2::new(x + self.icon_size / 2.0, rect.center().y);
            icon.paint_at(
                painter,
                center,
                self.icon_size,
                self.icon_color.unwrap_or(color),
            );
            x += self.icon_size + gap;
        }
        let pos = Pos2::new(x, rect.center().y - galley.size().y / 2.0);
        painter.galley(pos, galley, color);
        response.on_hover_cursor(CursorIcon::PointingHand)
    }
}

/// Bouton carré ou rond qui ne porte qu'une icône.
pub fn icon_button(
    ui: &mut Ui,
    icon: &'static Icon,
    side: f32,
    radius: f32,
    icon_size: f32,
    ink: impl Into<Ink>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    if ui.is_rect_visible(rect.expand(24.0)) {
        let enabled = ui.is_enabled();
        let painter = ui.painter();
        if enabled && response.is_pointer_button_down_on() {
            painter.add(inset(rect, radius, 2.4));
        } else {
            let depth = match (enabled, response.hovered()) {
                (false, _) => 1.5,
                (true, true) => 4.0,
                (true, false) => 3.0,
            };
            painter.add(raised(rect, radius, depth));
        }
        icon.paint_at(painter, rect.center(), icon_size, ink);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Proportions d'un sélecteur à segments.
#[derive(Debug, Clone, Copy)]
pub struct Segments {
    pub height: f32,
    pub pad: f32,
    pub radius: f32,
    pub size: f32,
}

impl Segments {
    /// Onglets de la barre du haut.
    pub const TABS: Segments = Segments {
        height: 34.0,
        pad: 18.0,
        radius: 12.0,
        size: 13.0,
    };
    /// Choix d'une carte.
    pub const CARD: Segments = Segments {
        height: 32.0,
        pad: 16.0,
        radius: 11.0,
        size: 12.5,
    };
    /// Choix d'un formulaire.
    pub const FORM: Segments = Segments {
        height: 28.0,
        pad: 12.0,
        radius: 9.0,
        size: 12.0,
    };

    /// Dessine les segments ; renvoie l'indice de celui qu'on vient de choisir.
    pub fn show(self, ui: &mut Ui, options: &[&str], selected: Option<usize>) -> Option<usize> {
        self.show_hinted(ui, options, &[], selected)
    }

    /// Comme [`Self::show`], avec une infobulle par segment.
    pub fn show_hinted(
        self,
        ui: &mut Ui,
        options: &[&str],
        hints: &[&str],
        selected: Option<usize>,
    ) -> Option<usize> {
        const TRACK_PAD: f32 = 5.0;
        const SPACING: f32 = 4.0;
        let p = palette();
        // Largeur mesurée en gras : choisir un segment ne déplace pas les autres.
        let widths: Vec<f32> = options
            .iter()
            .map(|o| text_width(ui, o, theme::font(self.size, Weight::Bold)) + 2.0 * self.pad)
            .collect();
        let total = widths.iter().sum::<f32>()
            + SPACING * options.len().saturating_sub(1) as f32
            + 2.0 * TRACK_PAD;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(total, self.height + 2.0 * TRACK_PAD),
            Sense::hover(),
        );
        ui.painter()
            .add(inset(rect, self.radius + TRACK_PAD - 1.0, 3.0));
        let mut picked = None;
        let mut left = rect.left() + TRACK_PAD;
        for (index, (option, width)) in options.iter().zip(widths).enumerate() {
            let item = Rect::from_min_size(
                Pos2::new(left, rect.top() + TRACK_PAD),
                Vec2::new(width, self.height),
            );
            left += width + SPACING;
            let on = selected == Some(index);
            let mut answer = ui
                .interact(item, response.id.with(index), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            if let Some(hint) = hints.get(index) {
                answer = answer.on_hover_text(*hint);
            }
            if answer.clicked() && !on {
                picked = Some(index);
            }
            let painter = ui.painter();
            let (color, weight) = if on {
                painter.add(accent_pill(item, self.radius));
                (ON_ACCENT, Weight::Bold)
            } else if answer.hovered() && ui.is_enabled() {
                (p.text, Weight::Semibold)
            } else {
                (p.soft, Weight::Semibold)
            };
            painter.text(
                item.center(),
                egui::Align2::CENTER_CENTER,
                option,
                theme::font(self.size, weight),
                color,
            );
        }
        picked
    }
}

/// Interrupteur ; `size` vaut 48×26, 44×24 ou 40×22 selon l'endroit.
pub fn switch(ui: &mut Ui, on: &mut bool, size: Vec2) -> Response {
    let p = palette();
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(response.id, *on);
        let radius = rect.height() / 2.0;
        let painter = ui.painter();
        if t > 0.5 {
            painter.add(gradient(rect, radius, SUN, ORANGE, true));
            painter.add(inner_shadow(rect, radius, 2.0, EMBER, Color32::TRANSPARENT));
        } else {
            painter.add(inset(rect, radius, 2.0));
        }
        let knob = rect.height() - 6.0;
        let travel = rect.width() - 6.0 - knob;
        let center = Pos2::new(rect.left() + 3.0 + knob / 2.0 + travel * t, rect.center().y);
        let disc = Rect::from_center_size(center, Vec2::splat(knob));
        painter.add(shadow(
            disc,
            knob / 2.0,
            Vec2::splat(2.0),
            4.0,
            p.knob_shade,
        ));
        painter.circle_filled(center, knob / 2.0, p.knob);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ----- Étiquettes -----

/// Étiquette en creux (« perso », « réservé »).
pub fn badge(ui: &mut Ui, label: &str, color: Color32, height: f32) -> Response {
    let size = if height >= 22.0 { 11.0 } else { 10.5 };
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::font(size, Weight::Bold), color);
    let pad = height * 0.45;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 2.0 * pad, height),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.add(inset(rect, height / 2.0, 2.0));
    painter.galley(
        Pos2::new(rect.left() + pad, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response
}

/// Étiquette en relief (main d'un binding).
pub fn chip(ui: &mut Ui, label: &str) -> Response {
    let p = palette();
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::font(11.5, Weight::Semibold),
        p.soft,
    );
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 24.0), Sense::hover());
    let painter = ui.painter();
    painter.add(raised(rect, 12.0, 2.0));
    painter.galley(
        Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        p.soft,
    );
    response
}

/// Pastille ronde.
pub fn dot(ui: &mut Ui, diameter: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter / 2.0, color);
    response
}

/// Pastille au dégradé d'accent.
pub fn accent_dot(ui: &mut Ui, diameter: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    ui.painter()
        .add(gradient(rect, diameter / 2.0, SUN, ORANGE, true));
    response
}

// ----- Jauges -----

/// Barre de progression dans le rectangle donné : rail en creux, remplissage
/// en dégradé ; `mark` place un repère (seuil) sur le rail.
pub fn bar_at(ui: &Ui, rect: Rect, fraction: f32, tint: Option<Color32>, mark: Option<f32>) {
    let p = palette();
    let painter = ui.painter();
    let radius = rect.height() / 2.0;
    painter.add(inset(rect, radius, 2.0));
    let width = (rect.width() * fraction.clamp(0.0, 1.0)).max(rect.height());
    let fill = Rect::from_min_size(rect.min, Vec2::new(width, rect.height()));
    let (from, to) = match tint {
        Some(color) => (color, color),
        None => (ORANGE, SUN),
    };
    painter.add(gradient(fill, radius, from, to, false));
    if let Some(mark) = mark {
        let x = rect.left() + rect.width() * mark.clamp(0.0, 1.0);
        painter.line_segment(
            [
                Pos2::new(x, rect.top() - 2.0),
                Pos2::new(x, rect.bottom() + 2.0),
            ],
            Stroke::new(1.0, p.muted),
        );
    }
}

/// Barre de progression sur toute la largeur disponible.
pub fn bar(ui: &mut Ui, fraction: f32, height: f32, tint: Option<Color32>, mark: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    bar_at(ui, rect, fraction, tint, mark);
}

/// Curseur : rail en creux, partie parcourue en dégradé, poignée en relief.
pub fn slider(ui: &mut Ui, value: &mut f32, range: RangeInclusive<f32>, width: f32) -> Response {
    const KNOB: f32 = 22.0;
    const RAIL: f32 = 12.0;
    let p = palette();
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, KNOB), Sense::click_and_drag());
    let (low, high) = (*range.start(), *range.end());
    let travel = (rect.width() - KNOB).max(1.0);
    if let Some(pointer) = response.interact_pointer_pos() {
        let t = ((pointer.x - rect.left() - KNOB / 2.0) / travel).clamp(0.0, 1.0);
        let next = low + (high - low) * t;
        if (next - *value).abs() > f32::EPSILON {
            *value = next;
            response.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let t = ((*value - low) / (high - low).max(f32::EPSILON)).clamp(0.0, 1.0);
        let rail = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), RAIL));
        let center = Pos2::new(rect.left() + KNOB / 2.0 + travel * t, rect.center().y);
        let painter = ui.painter();
        painter.add(inset(rail, RAIL / 2.0, 2.0));
        let fill = Rect::from_min_max(rail.min, Pos2::new(center.x, rail.bottom()));
        if fill.width() >= RAIL {
            painter.add(gradient(fill, RAIL / 2.0, ORANGE, SUN, false));
        }
        let knob = Rect::from_center_size(center, Vec2::splat(KNOB));
        painter.add(shadow(knob, KNOB / 2.0, Vec2::splat(-2.0), 5.0, p.shine));
        painter.add(shadow(
            knob,
            KNOB / 2.0,
            Vec2::splat(3.0),
            6.0,
            p.handle_shade,
        ));
        painter.circle_filled(center, KNOB / 2.0, p.handle);
    }
    response.on_hover_cursor(CursorIcon::ResizeHorizontal)
}

/// Jauge circulaire en creux, avec une icône au centre.
pub fn gauge(ui: &mut Ui, fraction: f32, color: Color32, icon: &'static Icon, ink: Color32) {
    const SIDE: f32 = 52.0;
    const RING: f32 = 16.0;
    let p = palette();
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(SIDE), Sense::hover());
    let painter = ui.painter();
    painter.add(inset(rect, SIDE / 2.0, 3.0));
    let center = rect.center();
    painter.circle_stroke(center, RING, Stroke::new(4.0, p.track));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.004 {
        let steps = ((fraction * 64.0).ceil() as usize).max(2);
        let at = |i: usize| {
            let angle = -FRAC_PI_2 + TAU * fraction * i as f32 / steps as f32;
            center + Vec2::new(angle.cos(), angle.sin()) * RING
        };
        let points: Vec<Pos2> = (0..=steps).map(at).collect();
        // Extrémités arrondies.
        painter.circle_filled(points[0], 2.0, color);
        painter.circle_filled(points[steps], 2.0, color);
        painter.add(Shape::line(points, Stroke::new(4.0, color)));
    }
    icon.paint_at(painter, center, 15.0, ink);
}

// ----- Saisie -----

/// Champ de texte en creux.
pub fn text_field(ui: &mut Ui, text: &mut String, width: f32, hint: &str) -> Response {
    let p = palette();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
    ui.painter().add(inset(rect, 11.0, 2.5));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.add(
        egui::TextEdit::singleline(text)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .desired_width(f32::INFINITY)
            .font(theme::font(12.5, Weight::Semibold))
            .text_color(p.text)
            .hint_text(text_hint(hint)),
    )
}

fn text_hint(hint: &str) -> RichText {
    text(hint, 12.5, Weight::Regular, palette().muted)
}

/// Nombre à glisser ou à saisir, dans un champ en creux.
pub fn number<N: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: RangeInclusive<N>,
    speed: f64,
    prefix: &str,
    suffix: &str,
    width: f32,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    ui.painter().add(inset(rect, 10.0, 2.5));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 0.0)))
            .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
    );
    let visuals = child.visuals_mut();
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    for state in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        state.weak_bg_fill = Color32::TRANSPARENT;
        state.bg_fill = Color32::TRANSPARENT;
        state.bg_stroke = Stroke::NONE;
        state.expansion = 0.0;
    }
    child.add(
        egui::DragValue::new(value)
            .range(range)
            .speed(speed)
            .prefix(prefix)
            .suffix(suffix),
    )
}

/// Liste déroulante en creux. `menu` dessine les choix avec [`menu_item`].
pub fn dropdown(ui: &mut Ui, label: &str, width: f32, menu: impl FnOnce(&mut Ui)) -> Response {
    let p = palette();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::click());
    let painter = ui.painter();
    painter.add(inset(rect, 13.0, 3.0));
    let text_clip = Rect::from_min_max(
        rect.min + Vec2::new(14.0, 0.0),
        rect.max - Vec2::new(30.0, 0.0),
    );
    painter
        .with_clip_rect(text_clip.intersect(painter.clip_rect()))
        .text(
            text_clip.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            theme::font(12.5, Weight::Semibold),
            p.text,
        );
    icons::CHEVRON.paint_at(
        painter,
        Pos2::new(rect.right() - 19.0, rect.center().y),
        10.0,
        p.muted,
    );
    egui::Popup::menu(&response)
        .gap(6.0)
        .width(width.max(180.0))
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            menu(ui);
        });
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Ligne d'un menu déroulant ; cliquer ferme le menu.
pub fn menu_item(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let p = palette();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, corner(9.0), p.well);
    }
    let (color, weight) = if selected {
        (p.accent, Weight::Bold)
    } else {
        (p.text, Weight::Semibold)
    };
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .text(
            rect.left_center() + Vec2::new(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            theme::font(12.5, weight),
            color,
        );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Titre muet d'un menu déroulant.
pub fn menu_title(ui: &mut Ui, label: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(caps(label, palette().muted));
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_walks_clockwise_along_the_rounded_rectangle() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 40.0));
        let points = outline(rect, 12.0);
        // Du milieu du bord gauche (haut du coin) au bord haut, puis à droite.
        assert!((points[0].0 - Pos2::new(10.0, 32.0)).length() < 1e-3);
        assert!((points[0].1 - Vec2::new(-1.0, 0.0)).length() < 1e-3);
        for (point, normal) in &points {
            assert!(rect.expand(1e-3).contains(*point), "{point:?}");
            assert!((normal.length() - 1.0).abs() < 1e-3);
        }
        // Aire signée positive en repère écran (y vers le bas) : sens horaire.
        let area: f32 = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|((a, _), (b, _))| a.x * b.y - b.x * a.y)
            .sum();
        assert!(area > 0.0);
        // Un rayon plus grand que la demi-hauteur donne une pilule, pas un pli.
        for (point, _) in outline(rect, 500.0) {
            assert!(rect.expand(1e-3).contains(point));
        }
    }

    #[test]
    fn meshes_are_valid_for_any_size() {
        for size in [
            Vec2::new(300.0, 120.0),
            Vec2::new(22.0, 22.0),
            Vec2::new(4.0, 2.0),
        ] {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            for shape in [
                inner_shadow(rect, 14.0, 3.0, Color32::BLACK, Color32::WHITE),
                gradient(rect, 11.0, SUN, ORANGE, true),
                gradient(rect, 6.0, ORANGE, SUN, false),
                fade(rect, Color32::BLACK),
            ] {
                let Shape::Mesh(mesh) = shape else {
                    panic!("un maillage est attendu");
                };
                assert!(mesh.is_valid());
                assert!(!mesh.indices.is_empty());
            }
        }
    }
}
