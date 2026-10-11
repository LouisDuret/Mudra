//! Thèmes de Mudra : palette neumorphique sombre (par défaut) et claire,
//! polices, et styles des widgets egui restés d'origine.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use mudra_config::Theme;
use mudra_gestures::EngineState;

/// Couleurs d'un thème. Les surfaces ont toutes la couleur du fond : le
/// relief vient de l'ombre (`shade`) et du reflet (`shine`).
pub struct Palette {
    pub bg: Color32,
    pub text: Color32,
    /// Texte secondaire.
    pub soft: Color32,
    pub muted: Color32,
    /// Ombre et reflet des surfaces en relief.
    pub shade: Color32,
    pub shine: Color32,
    /// Ombre des creux.
    pub hollow: Color32,
    /// Texte et icônes d'accent, lisibles sur le fond.
    pub accent: Color32,
    pub danger: Color32,
    pub ok: Color32,
    /// Bouton des interrupteurs et son ombre portée.
    pub knob: Color32,
    pub knob_shade: Color32,
    /// Curseur de sensibilité.
    pub handle: Color32,
    pub handle_shade: Color32,
    /// Cuvette de la vignette de l'onglet « Gestes ».
    pub well: Color32,
    pub well_shade: Color32,
    pub well_shine: Color32,
    /// Rail de la jauge circulaire.
    pub track: Color32,
    /// Main en pointillé de la vignette vide.
    pub ghost: Color32,
    /// Pastille éteinte.
    pub dot: Color32,
    /// Ombre portée d'un segment sélectionné.
    pub glow: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x26, 0x2A, 0x31),
    text: Color32::from_rgb(0xE8, 0xEA, 0xEE),
    soft: Color32::from_rgb(0xC3, 0xC8, 0xD1),
    muted: Color32::from_rgb(0x9A, 0xA0, 0xAC),
    shade: Color32::from_rgb(0x16, 0x18, 0x1D),
    shine: Color32::from_rgb(0x32, 0x37, 0x41),
    hollow: Color32::from_rgb(0x16, 0x18, 0x1D),
    accent: Color32::from_rgb(0xF5, 0xB5, 0x2E),
    danger: Color32::from_rgb(0xFF, 0x7A, 0x6B),
    ok: Color32::from_rgb(0x5B, 0xD4, 0x8A),
    knob: Color32::from_rgb(0xE8, 0xEA, 0xEE),
    knob_shade: Color32::from_rgba_premultiplied(0, 0, 0, 140),
    handle: Color32::from_rgb(0x3A, 0x3F, 0x49),
    handle_shade: Color32::from_rgb(0x14, 0x16, 0x1A),
    well: Color32::from_rgb(0x1E, 0x21, 0x27),
    well_shade: Color32::from_rgb(0x14, 0x16, 0x1A),
    well_shine: Color32::from_rgb(0x2C, 0x30, 0x39),
    track: Color32::from_rgb(0x1A, 0x1C, 0x21),
    ghost: Color32::from_rgb(0x5E, 0x64, 0x70),
    dot: Color32::from_rgb(0x16, 0x18, 0x1D),
    glow: Color32::from_rgba_premultiplied(0, 0, 0, 115),
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xE6, 0xE9, 0xEF),
    text: Color32::from_rgb(0x2B, 0x2F, 0x38),
    soft: Color32::from_rgb(0x4A, 0x50, 0x5C),
    muted: Color32::from_rgb(0x6B, 0x71, 0x80),
    shade: Color32::from_rgb(0xC3, 0xC8, 0xD2),
    shine: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hollow: Color32::from_rgb(0xC8, 0xCD, 0xD6),
    accent: Color32::from_rgb(0xB4, 0x53, 0x09),
    danger: Color32::from_rgb(0xB4, 0x23, 0x18),
    ok: Color32::from_rgb(0x2F, 0x9E, 0x62),
    knob: Color32::from_rgb(0xF2, 0xF4, 0xF7),
    // rgba(120, 128, 145, 0.45), prémultiplié.
    knob_shade: Color32::from_rgba_premultiplied(54, 58, 65, 115),
    handle: Color32::from_rgb(0xEE, 0xF0, 0xF4),
    handle_shade: Color32::from_rgb(0xBC, 0xC2, 0xCC),
    well: Color32::from_rgb(0xDD, 0xE1, 0xE8),
    well_shade: Color32::from_rgb(0xBF, 0xC5, 0xCF),
    well_shine: Color32::from_rgb(0xFA, 0xFB, 0xFC),
    track: Color32::from_rgb(0xD3, 0xD7, 0xDE),
    ghost: Color32::from_rgb(0x9A, 0xA1, 0xAE),
    dot: Color32::from_rgb(0xC3, 0xC8, 0xD2),
    // rgba(214, 110, 20, 0.35), prémultiplié.
    glow: Color32::from_rgba_premultiplied(75, 39, 7, 89),
};

/// Dégradé d'accent, commun aux deux thèmes, et texte posé dessus.
pub const SUN: Color32 = Color32::from_rgb(0xF5, 0xB5, 0x2E);
pub const ORANGE: Color32 = Color32::from_rgb(0xE8, 0x77, 0x1A);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x2A, 0x16, 0x06);
/// Ombre intérieure d'un interrupteur allumé : rgba(150, 70, 0, 0.35).
pub const EMBER: Color32 = Color32::from_rgba_premultiplied(53, 25, 0, 89);

/// Écran de la caméra : sombre dans les deux thèmes, comme l'image qu'il porte.
pub const SCREEN: Color32 = Color32::from_rgb(0x1C, 0x1F, 0x25);
/// Étiquette posée sur l'image : rgba(20, 22, 27, 0.72).
pub const SCREEN_CHIP: Color32 = Color32::from_rgba_premultiplied(14, 16, 19, 184);
pub const SCREEN_TEXT: Color32 = Color32::from_rgb(0xF2, 0xF4, 0xF7);
pub const SCREEN_SOFT: Color32 = Color32::from_rgb(0xD9, 0xDC, 0xE2);
pub const SCREEN_MUTED: Color32 = Color32::from_rgb(0x8E, 0x95, 0xA3);
pub const SCREEN_LIVE: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);
pub const SCREEN_OK: Color32 = Color32::from_rgb(0x5B, 0xD4, 0x8A);

/// Couleurs fixes de l'icône de notification et du halo du curseur, posés
/// sur le bureau et non sur une fenêtre de Mudra.
pub const TRAY_PAUSED: Color32 = Color32::from_rgb(0x6B, 0x74, 0x80);
pub const TRAY_LOCKED: Color32 = Color32::from_rgb(0xE6, 0xEA, 0xF0);
pub const TRAY_ARMED: Color32 = Color32::from_rgb(0xF5, 0xB5, 0x2E);
pub const TRAY_ERROR: Color32 = Color32::from_rgb(0xF2, 0x61, 0x6A);
pub const GREEN: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);

static LIGHT_THEME: AtomicBool = AtomicBool::new(false);

/// Palette du thème en service.
pub fn palette() -> &'static Palette {
    if LIGHT_THEME.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn current() -> Theme {
    if LIGHT_THEME.load(Ordering::Relaxed) {
        Theme::Light
    } else {
        Theme::Dark
    }
}

/// Change de thème ; chaque fenêtre ouverte doit ensuite repasser par [`apply`].
pub fn set(theme: Theme) {
    LIGHT_THEME.store(theme == Theme::Light, Ordering::Relaxed);
}

/// Couleur associée à un état du moteur.
pub fn state_color(state: EngineState) -> Color32 {
    let p = palette();
    match state {
        EngineState::Paused | EngineState::Locked => p.muted,
        EngineState::Armed => ORANGE,
        EngineState::Continuous => p.ok,
    }
}

/// Graisses disponibles ; egui n'en a pas, chacune est une famille de police.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Semibold,
    Bold,
}

const SEMIBOLD: &str = "semibold";
const BOLD: &str = "bold";

pub fn font(size: f32, weight: Weight) -> FontId {
    let family = match weight {
        Weight::Regular => FontFamily::Proportional,
        Weight::Semibold => FontFamily::Name(SEMIBOLD.into()),
        Weight::Bold => FontFamily::Name(BOLD.into()),
    };
    FontId::new(size, family)
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Polices des maquettes, compilées dans le binaire (voir `assets/fonts/NOTICE`).
static MANROPE: &[u8] = include_bytes!("../../assets/fonts/Manrope.ttf");
static JETBRAINS_MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono.ttf");

/// Graisses de Manrope pour le texte courant, les libellés et les titres :
/// les maquettes vont de 400 à 800.
const REGULAR_WEIGHT: f32 = 450.0;
const SEMIBOLD_WEIGHT: f32 = 650.0;
const BOLD_WEIGHT: f32 = 800.0;

/// Manrope en trois graisses et JetBrains Mono, devant Segoe UI (pour les
/// écritures que Manrope n'a pas) puis les polices d'egui, qui gardent les
/// symboles.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let mut embed = |name: &str, bytes: &'static [u8], weight: f32| {
        let tweak = egui::FontTweak {
            coords: egui::epaint::text::VariationCoords::new([(b"wght", weight)]),
            ..Default::default()
        };
        let data = egui::FontData::from_static(bytes).tweak(tweak);
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    embed("regular", MANROPE, REGULAR_WEIGHT);
    embed(SEMIBOLD, MANROPE, SEMIBOLD_WEIGHT);
    embed(BOLD, MANROPE, BOLD_WEIGHT);
    embed("mono", JETBRAINS_MONO, REGULAR_WEIGHT);

    let system = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
        .join("segoeui.ttf");
    let fallback = std::fs::read(system).ok().map(|bytes| {
        fonts.font_data.insert(
            "systeme".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        "systeme".to_owned()
    });
    let base = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let stack = |first: &str| -> Vec<String> {
        std::iter::once(first.to_owned())
            .chain(fallback.clone())
            .chain(base.iter().cloned())
            .collect()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, stack("regular"));
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), stack(SEMIBOLD));
    fonts
        .families
        .insert(FontFamily::Name(BOLD.into()), stack(BOLD));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "mono".to_owned());
    fonts
}

/// Polices et espacements : une fois par fenêtre, à sa création.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.interaction.selectable_labels = false;
        style.text_styles = [
            (TextStyle::Heading, font(20.0, Weight::Bold)),
            (TextStyle::Body, font(13.0, Weight::Regular)),
            (TextStyle::Button, font(12.5, Weight::Semibold)),
            (TextStyle::Small, font(11.5, Weight::Regular)),
            (TextStyle::Monospace, mono(11.5)),
        ]
        .into();
    });
    apply(ctx);
}

/// Couleurs du thème en service, pour les widgets egui d'origine (champs de
/// saisie, listes déroulantes, infobulles, barres de défilement).
pub fn apply(ctx: &egui::Context) {
    let p = palette();
    let light = current() == Theme::Light;
    let mut visuals = if light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    visuals.override_text_color = Some(p.text);
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.bg;
    visuals.extreme_bg_color = p.well;
    visuals.faint_bg_color = p.well;
    visuals.code_bg_color = p.well;
    visuals.window_stroke = Stroke::new(1.0, p.shine);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.menu_corner_radius = CornerRadius::same(14);
    visuals.popup_shadow = egui::Shadow {
        offset: [6, 8],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(if light { 50 } else { 120 }),
    };
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = ORANGE.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.warn_fg_color = p.accent;
    visuals.error_fg_color = p.danger;

    let radius = CornerRadius::same(9);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = p.bg;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.shade);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    widgets.noninteractive.corner_radius = radius;
    widgets.inactive.bg_fill = p.well;
    widgets.inactive.weak_bg_fill = p.well;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0, p.soft);
    widgets.inactive.corner_radius = radius;
    widgets.hovered.bg_fill = p.well;
    widgets.hovered.weak_bg_fill = p.well;
    widgets.hovered.bg_stroke = Stroke::new(1.0, ORANGE.gamma_multiply(0.7));
    widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    widgets.hovered.corner_radius = radius;
    widgets.active.bg_fill = p.well;
    widgets.active.weak_bg_fill = p.well;
    widgets.active.bg_stroke = Stroke::new(1.0, ORANGE);
    widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    widgets.active.corner_radius = radius;
    widgets.open.bg_fill = p.well;
    widgets.open.weak_bg_fill = p.well;
    widgets.open.corner_radius = radius;

    let theme = if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, visuals);
}

/// Nom français d'un geste connu ; l'identifiant brut sinon.
pub fn gesture_name(id: &str) -> &str {
    match id {
        "none" => "aucun",
        "open_palm" => "Paume ouverte",
        "fist" => "Poing",
        "thumbs_up" => "Pouce levé",
        "thumbs_down" => "Pouce baissé",
        "v_sign" => "V",
        "rock" => "Rock",
        "three_fingers" => "Trois doigts",
        "pinch" => "Pincement",
        "pointing" => "Index tendu",
        "swipe_left" => "Balayage gauche",
        "swipe_right" => "Balayage droite",
        "swipe_up" => "Balayage haut",
        "swipe_down" => "Balayage bas",
        "rotate_cw" => "Rotation horaire",
        "rotate_ccw" => "Rotation anti-horaire",
        other => CUSTOM_NAMES
            .lock()
            .ok()
            .and_then(|names| {
                names
                    .iter()
                    .find(|(label, _)| label == other)
                    .map(|(_, n)| *n)
            })
            .unwrap_or(other),
    }
}

/// Noms affichés des poses perso. Ils sont lus de partout dans l'interface,
/// d'où une table globale ; un nom reste alloué pour la durée du programme.
static CUSTOM_NAMES: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

/// Remplace la table des noms des poses perso (label, nom).
pub fn set_custom_names<'a>(names: impl Iterator<Item = (&'a str, &'a str)>) {
    let Ok(mut table) = CUSTOM_NAMES.lock() else {
        return;
    };
    let next = names
        .map(|(label, name)| {
            let kept = table
                .iter()
                .find(|(l, n)| l == label && *n == name)
                .map(|(_, n)| *n);
            (
                label.to_string(),
                kept.unwrap_or_else(|| Box::leak(name.to_string().into_boxed_str())),
            )
        })
        .collect();
    *table = next;
}
