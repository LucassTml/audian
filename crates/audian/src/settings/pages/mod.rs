//! Page contents of the Audian window.

mod appearance;
mod dictation;
mod home;
mod intelligence;
mod models;
mod system;
mod welcome;

use audian_common::config::ProcessingMode;
use eframe::egui;

use super::theme::icon;
use super::{Page, SettingsApp};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    match app.page {
        Page::Welcome => welcome::show(app, ui),
        Page::Home => home::show(app, ui),
        Page::Shortcut => dictation::shortcut(app, ui),
        Page::Audio => dictation::audio(app, ui),
        Page::Speech => intelligence::speech(app, ui),
        Page::Rewriting => intelligence::rewriting(app, ui),
        Page::Profiles => intelligence::profiles(app, ui),
        Page::Models => models::show(app, ui),
        Page::History => system::history(app, ui),
        Page::General => system::general(app, ui),
        Page::Appearance => appearance::show(app, ui),
        Page::Privacy => system::privacy(app, ui),
        Page::About => system::about(app, ui),
    }
}

pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("pt", "Portuguese"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("tr", "Turkish"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
];

pub fn language_name(code: &str) -> String {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, n)| n.to_string()).unwrap_or_else(|| code.to_string())
}

pub fn mode_icon(mode: ProcessingMode) -> &'static str {
    match mode {
        ProcessingMode::Natural => icon::CHAT,
        ProcessingMode::AiPrompt => icon::ROBOT,
        ProcessingMode::Professional => icon::WORK,
        ProcessingMode::Document => icon::DOC,
        ProcessingMode::Literal => icon::QUOTE,
        ProcessingMode::Custom => icon::EDIT,
    }
}

/// A path with the profile folders shown as `%APPDATA%` / `%LOCALAPPDATA%`: shorter, and the
/// same on every PC.
pub fn display_path(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    for var in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let base = base.to_string_lossy().to_string();
            if !base.is_empty() && s.to_lowercase().starts_with(&base.to_lowercase()) {
                return format!("%{var}%{}", &s[base.len()..]);
            }
        }
    }
    s
}

/// How many choice tiles fit per row: 3, or 2 in a narrow window.
pub fn tiles_per_row(ui: &egui::Ui) -> usize {
    if ui.available_width() >= 640.0 { 3 } else { 2 }
}

/// Lays out `n` equally sized tiles per row and returns their width.
pub fn tile_width(ui: &egui::Ui, per_row: usize) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    ((ui.available_width() - gap * (per_row as f32 - 1.0)) / per_row as f32).floor()
}
