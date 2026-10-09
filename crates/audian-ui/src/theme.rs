//! Visual design system for the Audian window: palette, typography and icon glyphs.
//!
//! Fonts come from Windows itself (Segoe UI Variable, Segoe UI Semibold and Segoe Fluent
//! Icons), so the app looks native without bundling any font files; egui's defaults remain
//! as fallbacks.
//!
//! Colours are read through functions such as [`text`] and [`accent`]: surfaces and text follow
//! the light or dark mode ([`set_light`]), the accent family follows the theme ([`set_theme`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use audian_common::theme::{Palette, Rgb, ThemeId};
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Vec2};

/// Surface, text and control colours of one mode.
pub struct Surfaces {
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub inset: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub danger: Color32,
    /// Resting fill of controls (secondary buttons, switches when off).
    pub control: Color32,
    pub control_hover: Color32,
    pub control_border: Color32,
    /// Behind a selected / hovered navigation item.
    pub selected: Color32,
    pub hovered: Color32,
    /// Danger button: fill, hover fill, border.
    pub danger_fill: Color32,
    pub danger_hover: Color32,
    pub danger_border: Color32,
    /// Keyboard keycaps: face, edge, bottom shadow.
    pub cap: Color32,
    pub cap_edge: Color32,
    pub cap_shadow: Color32,
    /// Multiplier for drop-shadow opacity (shadows are subtler on light surfaces).
    pub shadow: f32,
}

pub const DARK: Surfaces = Surfaces {
    bg: Color32::from_rgb(14, 15, 19),
    sidebar: Color32::from_rgb(18, 19, 24),
    card: Color32::from_rgb(23, 24, 31),
    card_hover: Color32::from_rgb(28, 29, 38),
    inset: Color32::from_rgb(16, 17, 22),
    border: Color32::from_rgb(36, 37, 48),
    text: Color32::from_rgb(236, 236, 242),
    text_dim: Color32::from_rgb(162, 163, 179),
    text_faint: Color32::from_rgb(109, 110, 125),
    success: Color32::from_rgb(63, 208, 139),
    warn: Color32::from_rgb(255, 184, 77),
    danger: Color32::from_rgb(255, 93, 93),
    control: Color32::from_rgb(36, 37, 49),
    control_hover: Color32::from_rgb(46, 47, 62),
    control_border: Color32::from_rgb(62, 63, 82),
    selected: Color32::from_rgb(26, 27, 34),
    hovered: Color32::from_rgb(30, 31, 40),
    danger_fill: Color32::from_rgb(58, 28, 32),
    danger_hover: Color32::from_rgb(80, 34, 40),
    danger_border: Color32::from_rgb(96, 44, 50),
    cap: Color32::from_rgb(40, 41, 54),
    cap_edge: Color32::from_rgb(62, 63, 82),
    cap_shadow: Color32::from_rgb(12, 12, 16),
    shadow: 1.0,
};

pub const LIGHT: Surfaces = Surfaces {
    bg: Color32::from_rgb(244, 242, 237),
    sidebar: Color32::from_rgb(236, 233, 227),
    card: Color32::from_rgb(255, 255, 255),
    card_hover: Color32::from_rgb(250, 249, 246),
    inset: Color32::from_rgb(238, 236, 231),
    border: Color32::from_rgb(224, 220, 212),
    text: Color32::from_rgb(28, 28, 32),
    text_dim: Color32::from_rgb(88, 88, 98),
    text_faint: Color32::from_rgb(132, 131, 140),
    success: Color32::from_rgb(20, 138, 84),
    warn: Color32::from_rgb(178, 102, 0),
    danger: Color32::from_rgb(200, 50, 50),
    control: Color32::from_rgb(234, 231, 225),
    control_hover: Color32::from_rgb(224, 221, 214),
    control_border: Color32::from_rgb(208, 204, 196),
    selected: Color32::from_rgb(225, 222, 215),
    hovered: Color32::from_rgb(230, 227, 221),
    danger_fill: Color32::from_rgb(253, 236, 236),
    danger_hover: Color32::from_rgb(250, 222, 222),
    danger_border: Color32::from_rgb(238, 190, 190),
    cap: Color32::from_rgb(255, 255, 255),
    cap_edge: Color32::from_rgb(212, 208, 200),
    cap_shadow: Color32::from_rgb(204, 200, 192),
    shadow: 0.35,
};

/// The home banner stays dark in both modes; this is the colour it is blended from.
pub const BANNER_BASE: Color32 = Color32::from_rgb(14, 15, 19);

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

/// Switches between the light and dark surfaces. Call [`apply_visuals`] afterwards.
pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT_MODE.load(Ordering::Relaxed)
}

pub fn surfaces() -> &'static Surfaces {
    if is_light() { &LIGHT } else { &DARK }
}

pub fn bg() -> Color32 {
    surfaces().bg
}
pub fn sidebar() -> Color32 {
    surfaces().sidebar
}
pub fn card() -> Color32 {
    surfaces().card
}
pub fn card_hover() -> Color32 {
    surfaces().card_hover
}
pub fn inset() -> Color32 {
    surfaces().inset
}
pub fn border() -> Color32 {
    surfaces().border
}
pub fn text() -> Color32 {
    surfaces().text
}
pub fn text_dim() -> Color32 {
    surfaces().text_dim
}
pub fn text_faint() -> Color32 {
    surfaces().text_faint
}
pub fn success() -> Color32 {
    surfaces().success
}
pub fn warn() -> Color32 {
    surfaces().warn
}
pub fn danger() -> Color32 {
    surfaces().danger
}

/// A black shadow of the given opacity (0–255 on dark), scaled down on light surfaces.
pub fn shadow(alpha: u8) -> Color32 {
    Color32::from_black_alpha((alpha as f32 * surfaces().shadow) as u8)
}

static THEME: AtomicU8 = AtomicU8::new(0);

/// Selects the accent family. Call [`apply_visuals`] afterwards to update egui's own widgets.
pub fn set_theme(theme: ThemeId) {
    THEME.store(theme as u8, Ordering::Relaxed);
}

pub fn current_theme() -> ThemeId {
    ThemeId::from_index(THEME.load(Ordering::Relaxed))
}

pub fn palette() -> &'static Palette {
    current_theme().palette()
}

pub fn rgb((r, g, b): Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Main accent: buttons, switches, selection, icons. On light surfaces each theme uses its
/// darker "ink" variant (Ivory's off-white would vanish on white).
pub fn accent() -> Color32 {
    if is_light() { rgb(palette().ink) } else { rgb(palette().accent) }
}

pub fn accent_hover() -> Color32 {
    if is_light() { lerp_color(rgb(palette().ink), Color32::BLACK, 0.15) } else { rgb(palette().accent_hover) }
}

/// Text and icons on an accent fill.
pub fn on_accent() -> Color32 {
    if is_light() { Color32::WHITE } else { rgb(palette().on_accent) }
}

/// Second accent, for gradients and secondary highlights.
pub fn accent2() -> Color32 {
    if is_light() { rgb(palette().ink_2) } else { rgb(palette().accent_2) }
}

/// `base` tinted towards the accent, for selected surfaces.
pub fn tint(base: Color32, amount: f32) -> Color32 {
    lerp_color(base, accent(), amount)
}

/// Segoe Fluent Icons code points.
pub mod icon {
    pub const HOME: &str = "\u{E80F}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const KEYBOARD: &str = "\u{E765}";
    pub const MIC: &str = "\u{E720}";
    pub const SPEECH: &str = "\u{E8D6}";
    pub const EDIT: &str = "\u{E70F}";
    pub const APPS: &str = "\u{E71D}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const HISTORY: &str = "\u{E81C}";
    pub const LOCK: &str = "\u{E72E}";
    pub const INFO: &str = "\u{E946}";
    pub const CHECK: &str = "\u{E73E}";
    pub const CLOSE: &str = "\u{E711}";
    pub const COPY: &str = "\u{E8C8}";
    pub const DELETE: &str = "\u{E74D}";
    pub const SEARCH: &str = "\u{E721}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const CLOUD: &str = "\u{E753}";
    pub const SHIELD: &str = "\u{EA18}";
    pub const CHAT: &str = "\u{E8BD}";
    pub const ROBOT: &str = "\u{E99A}";
    pub const WORK: &str = "\u{E821}";
    pub const DOC: &str = "\u{E8A5}";
    pub const QUOTE: &str = "\u{E9B2}";
    pub const SPARKLE: &str = "\u{E945}";
    pub const PLAY: &str = "\u{E768}";
    pub const STOP: &str = "\u{E71A}";
    pub const FOLDER: &str = "\u{E838}";
    pub const ADD: &str = "\u{E710}";
    pub const CLOCK: &str = "\u{E916}";
    pub const TEXT: &str = "\u{E8D2}";
    pub const INSERT: &str = "\u{E7C3}";
    pub const VOLUME: &str = "\u{E767}";
    pub const GLOBE: &str = "\u{E774}";
    pub const CHIP: &str = "\u{E950}";
    pub const POWER: &str = "\u{E7E8}";
    pub const PALETTE: &str = "\u{E790}";
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn icons(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

fn system_font(file: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
    std::fs::read(dir.join(file)).ok()
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, files: &[&str]| -> bool {
        for f in files {
            if let Some(bytes) = system_font(f) {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let has_body = add("segoe", &["SegUIVar.ttf", "segoeui.ttf"]);
    let has_semibold = add("segoe-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let has_icons = add("fluent-icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);

    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if has_body {
        proportional.insert(0, "segoe".into());
    }
    let fallback = proportional.clone();
    let mut semibold = if has_semibold { vec!["segoe-semibold".to_string()] } else { Vec::new() };
    semibold.extend(fallback.iter().cloned());
    fonts.families.insert(FontFamily::Name("semibold".into()), semibold);
    let mut icon_family = if has_icons { vec!["fluent-icons".to_string()] } else { Vec::new() };
    icon_family.extend(fallback);
    fonts.families.insert(FontFamily::Name("icons".into()), icon_family);
    ctx.set_fonts(fonts);
    apply_visuals(ctx);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.interact_size = Vec2::new(40.0, 30.0);
        s.spacing.slider_width = 220.0;
        s.spacing.combo_width = 220.0;
        s.animation_time = 0.16;
        for (text_style, size) in [
            (egui::TextStyle::Body, 14.5),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 12.5),
            (egui::TextStyle::Monospace, 13.5),
            (egui::TextStyle::Heading, 26.0),
        ] {
            if let Some(f) = s.text_styles.get_mut(&text_style) {
                f.size = size;
            }
        }
    });
}

/// egui's own widget colours (text selection, combo boxes, sliders...) for the current theme.
pub fn apply_visuals(ctx: &egui::Context) {
    let accent = accent();
    let c = surfaces();
    let mut visuals = if is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    visuals.panel_fill = c.bg;
    visuals.window_fill = c.card;
    visuals.extreme_bg_color = c.inset;
    visuals.faint_bg_color = c.card_hover;
    visuals.selection.bg_fill = accent.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, accent);
    visuals.hyperlink_color = accent;
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.window_stroke = Stroke::new(1.0, c.border);
    visuals.override_text_color = Some(c.text);
    let w = &mut visuals.widgets;
    for (state, fill, stroke) in [
        (&mut w.noninteractive, c.card, c.border),
        (&mut w.inactive, c.control, c.control_border),
        (&mut w.hovered, c.control_hover, lerp_color(c.control_border, c.text_faint, 0.4)),
        (&mut w.active, c.control_hover, accent),
        (&mut w.open, c.control, accent),
    ] {
        state.corner_radius = CornerRadius::same(8);
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
    }
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.text_dim);
    w.inactive.fg_stroke = Stroke::new(1.0, c.text);
    w.hovered.fg_stroke = Stroke::new(1.5, c.text);
    w.active.fg_stroke = Stroke::new(1.5, c.text);
    visuals.popup_shadow.color = shadow(90);
    visuals.window_shadow.color = shadow(90);
    // Same visuals whichever theme egui thinks is active; the native title bar follows the mode.
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
    ctx.set_theme(if is_light() { egui::Theme::Light } else { egui::Theme::Dark });
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}
