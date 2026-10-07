//! Model manager: download, select and remove local models.

use audian_common::catalog::{self, ModelInfo, ModelKind};
use audian_common::download::State;
use audian_common::paths;
use eframe::egui::{self, RichText};

use super::super::theme::{icon, *};
use super::super::widgets as w;
use super::super::SettingsApp;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Models", "Everything runs on your CPU — download only what you need.");
    ui.horizontal(|ui| {
        let missing: Vec<&'static ModelInfo> = catalog::recommended().filter(|m| !app.model_installed(m)).collect();
        if !missing.is_empty() {
            let total: u64 = missing.iter().map(|m| m.size).sum();
            if w::primary_icon(ui, icon::DOWNLOAD, &format!("Download recommended ({})", catalog::format_size(total))).clicked() {
                for m in missing {
                    app.download(m);
                }
            }
        } else {
            w::badge(ui, "Recommended models installed", SUCCESS);
        }
        if w::secondary_icon(ui, icon::FOLDER, "Open folder").clicked() {
            super::super::open_path(&paths::models_dir());
        }
    });
    ui.add_space(12.0);
    for (kind, title, subtitle, ic) in [
        (ModelKind::Speech, "Speech recognition", "Whisper models (whisper.cpp). Larger = more accurate, slower.", icon::SPEECH),
        (ModelKind::Rewrite, "Rewriting", "Small instruction-tuned language models (GGUF, llama.cpp).", icon::SPARKLE),
    ] {
        w::card(ui, |ui| {
            w::card_title(ui, ic, title, subtitle);
            let models: Vec<&'static ModelInfo> = catalog::MODELS.iter().filter(|m| m.kind == kind).collect();
            for (i, m) in models.iter().enumerate() {
                if i > 0 {
                    w::divider(ui);
                }
                model_row(app, ui, m);
            }
            let custom = custom_models(app, kind);
            for file in custom {
                w::divider(ui);
                custom_row(app, ui, kind, &file);
            }
        });
        ui.add_space(12.0);
    }
    w::hint(
        ui,
        "RAM is the engine's memory while loaded; engines start only when you dictate and exit after the idle time set on the Speech recognition and Rewriting pages. No GPU/VRAM is used.",
    );
}

fn in_use(app: &SettingsApp, m: &ModelInfo) -> bool {
    match m.kind {
        ModelKind::Speech => app.draft.transcription.whisper.model.eq_ignore_ascii_case(m.file),
        ModelKind::Rewrite => app.draft.processing.local_llm.model.eq_ignore_ascii_case(m.file),
    }
}

fn select(app: &mut SettingsApp, kind: ModelKind, file: &str) {
    match kind {
        ModelKind::Speech => app.draft.transcription.whisper.model = file.to_string(),
        ModelKind::Rewrite => app.draft.processing.local_llm.model = file.to_string(),
    }
}

fn model_row(app: &mut SettingsApp, ui: &mut egui::Ui, m: &'static ModelInfo) {
    let installed = app.model_installed(m);
    let using = in_use(app, m);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(m.name).font(super::super::theme::semibold(15.0)).color(TEXT));
                if m.recommended {
                    w::badge(ui, "Recommended", accent());
                }
                if using && installed {
                    w::badge(ui, "In use", SUCCESS);
                }
            });
            ui.label(RichText::new(m.summary).size(13.0).color(TEXT_DIM));
            ui.label(RichText::new(format!("{}  ·  ~{} MB RAM  ·  {}", catalog::format_size(m.size), m.ram_mb, m.speed)).size(12.0).color(TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let dl = app.download_for(m.file).cloned();
            match dl.map(|d| (d.state(), d)) {
                Some((State::Downloading, d)) => {
                    if w::icon_button(ui, icon::CLOSE, "Cancel (resumable)").clicked() {
                        d.cancel();
                    }
                    ui.vertical(|ui| {
                        w::progress(ui, d.fraction(), 180.0);
                        ui.label(RichText::new(format!("{} of {}", catalog::format_size(d.received()), catalog::format_size(d.total))).size(12.0).color(TEXT_DIM));
                    });
                }
                Some((State::Verifying, _)) => {
                    ui.label(RichText::new("Verifying…").size(13.0).color(TEXT_DIM));
                }
                Some((State::Failed(e), _)) if !installed => {
                    if w::secondary(ui, "Retry").clicked() {
                        app.download(m);
                    }
                    ui.label(RichText::new(e).size(12.0).color(WARN));
                }
                _ if installed => {
                    if !using {
                        if w::icon_button(ui, icon::DELETE, "Delete").clicked() {
                            let _ = std::fs::remove_file(paths::models_dir().join(m.file));
                            app.refresh_model_files();
                        }
                        if w::secondary(ui, "Use").clicked() {
                            select(app, m.kind, m.file);
                        }
                    }
                }
                _ => {
                    let partial = app.model_file_size(&format!("{}.part", m.file)).unwrap_or(0);
                    let label = if partial > 0 { "Resume" } else { "Download" };
                    if w::secondary_icon(ui, icon::DOWNLOAD, label).clicked() {
                        app.download(m);
                    }
                }
            }
        });
    });
}

fn custom_models(app: &SettingsApp, kind: ModelKind) -> Vec<String> {
    let ext = match kind {
        ModelKind::Speech => ".bin",
        ModelKind::Rewrite => ".gguf",
    };
    app.model_files
        .iter()
        .map(|(f, _)| f.clone())
        .filter(|f| f.to_lowercase().ends_with(ext) && catalog::find(f).is_none())
        .collect()
}

fn custom_row(app: &mut SettingsApp, ui: &mut egui::Ui, kind: ModelKind, file: &str) {
    let using = match kind {
        ModelKind::Speech => app.draft.transcription.whisper.model.eq_ignore_ascii_case(file),
        ModelKind::Rewrite => app.draft.processing.local_llm.model.eq_ignore_ascii_case(file),
    };
    let size = app.model_file_size(file).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(file).font(super::super::theme::semibold(14.5)).color(TEXT));
                w::badge(ui, "Custom", TEXT_DIM);
                if using {
                    w::badge(ui, "In use", SUCCESS);
                }
            });
            ui.label(RichText::new(format!("{}  ·  added manually", catalog::format_size(size))).size(12.0).color(TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !using && w::secondary(ui, "Use").clicked() {
                select(app, kind, file);
            }
        });
    });
}
