//! Visual design system for the Audian window: palette, typography and icon glyphs.
//!
//! Fonts come from Windows itself (Segoe UI Variable, Segoe UI Semibold and Segoe Fluent
//! Icons), so the app looks native without bundling any font files; egui's defaults remain
//! as fallbacks.
//!
//! Surfaces and text are constants; the accent family comes from the selected theme
//! ([`set_theme`]) and is read through functions such as [`accent`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use audian_common::theme::{Palette, Rgb, ThemeId};
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Vec2};

pub const BG: Color32 = Color32::from_rgb(14, 15, 19);
pub const SIDEBAR: Color32 = Color32::from_rgb(18, 19, 24);
pub const CARD: Color32 = Color32::from_rgb(23, 24, 31);
pub const CARD_HOVER: Color32 = Color32::from_rgb(28, 29, 38);
pub const INSET: Color32 = Color32::from_rgb(16, 17, 22);
pub const BORDER: Color32 = Color32::from_rgb(36, 37, 48);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 242);
pub const TEXT_DIM: Color32 = Color32::from_rgb(162, 163, 179);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(109, 110, 125);
pub const SUCCESS: Color32 = Color32::from_rgb(63, 208, 139);
pub const WARN: Color32 = Color32::from_rgb(255, 184, 77);
pub const DANGER: Color32 = Color32::from_rgb(255, 93, 93);

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

/// Main accent: buttons, switches, selection, icons.
pub fn accent() -> Color32 {
    rgb(palette().accent)
}

pub fn accent_hover() -> Color32 {
    rgb(palette().accent_hover)
}

/// Text and icons on an accent fill.
pub fn on_accent() -> Color32 {
    rgb(palette().on_accent)
}

/// Second accent, for gradients and secondary highlights.
pub fn accent2() -> Color32 {
    rgb(palette().accent_2)
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
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.extreme_bg_color = INSET;
    visuals.faint_bg_color = CARD_HOVER;
    visuals.selection.bg_fill = accent.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, accent);
    visuals.hyperlink_color = accent;
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.override_text_color = Some(TEXT);
    let w = &mut visuals.widgets;
    for (state, fill, stroke) in [
        (&mut w.noninteractive, CARD, BORDER),
        (&mut w.inactive, Color32::from_rgb(34, 35, 45), Color32::from_rgb(44, 45, 58)),
        (&mut w.hovered, Color32::from_rgb(42, 43, 56), Color32::from_rgb(70, 70, 92)),
        (&mut w.active, Color32::from_rgb(48, 48, 64), accent),
        (&mut w.open, Color32::from_rgb(40, 41, 54), accent),
    ] {
        state.corner_radius = CornerRadius::same(8);
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
    }
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.fg_stroke = Stroke::new(1.5, TEXT);
    ctx.set_visuals(visuals);
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}
