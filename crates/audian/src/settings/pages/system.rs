//! History, General, Privacy and About.

use audian_common::config::{InsertMethod, RewriteProvider, SttProvider};
use audian_common::{history, paths};
use eframe::egui::{self, RichText, vec2};

use super::super::theme::{icon, *};
use super::super::widgets as w;
use super::super::SettingsApp;

pub fn history(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "History", "Your recent dictations, stored only on this PC (text only, never audio).");
    if !app.saved.privacy.history {
        w::card(ui, |ui| {
            w::row(ui, "History is off", "Turn it on to keep your recent dictations.", |ui| {
                w::toggle(ui, &mut app.draft.privacy.history);
            });
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::SEARCH).font(super::super::theme::icons(14.0)).color(text_dim()));
        ui.add(egui::TextEdit::singleline(&mut app.history_search).hint_text("Search dictations").desired_width(320.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.confirm_clear {
                if w::danger(ui, "Delete all history").clicked() {
                    history::clear();
                    app.history.clear();
                    app.confirm_clear = false;
                    app.toast("History cleared", true);
                }
                if w::secondary(ui, "Keep").clicked() {
                    app.confirm_clear = false;
                }
            } else if !app.history.is_empty() && w::secondary_icon(ui, icon::DELETE, "Clear").clicked() {
                app.confirm_clear = true;
            }
        });
    });
    ui.add_space(8.0);
    let q = app.history_search.to_lowercase();
    let entries: Vec<(usize, history::Entry)> = app
        .history
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, e)| q.is_empty() || e.text.to_lowercase().contains(&q) || e.app.to_lowercase().contains(&q))
        .take(100)
        .collect();
    if entries.is_empty() {
        w::hint(ui, if app.history.is_empty() { "No dictations yet." } else { "No matches." });
    }
    for (i, e) in entries {
        let expanded = app.expanded_entry == Some(i);
        w::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&e.time).size(12.5).color(text_faint()));
                if !e.app.is_empty() {
                    w::badge(ui, &e.app, accent2());
                }
                w::badge(ui, &e.mode, accent());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::icon_button(ui, icon::COPY, "Copy").clicked() {
                        ui.ctx().copy_text(e.text.clone());
                        app.toast("Copied", true);
                    }
                    let chevron = if expanded { "\u{E70E}" } else { "\u{E70D}" };
                    if w::icon_button(ui, chevron, "Details").clicked() {
                        app.expanded_entry = if expanded { None } else { Some(i) };
                    }
                });
            });
            ui.add(egui::Label::new(RichText::new(&e.text).size(14.5).color(text())).wrap());
            if expanded {
                w::divider(ui);
                ui.label(RichText::new("What you said").size(12.5).color(text_faint()));
                ui.add(egui::Label::new(RichText::new(&e.raw).size(13.5).color(text_dim())).wrap());
                ui.label(
                    RichText::new(format!(
                        "{:.1} s of audio · processed in {:.2} s · {} · language: {}",
                        e.audio_ms as f64 / 1000.0,
                        e.process_ms as f64 / 1000.0,
                        e.provider,
                        e.language
                    ))
                    .size(12.0)
                    .color(text_faint()),
                );
            }
        });
        ui.add_space(8.0);
    }
}

pub fn general(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "General", "Startup, notifications and how text is inserted.");
    w::card(ui, |ui| {
        w::card_title(ui, icon::POWER, "Startup & notifications", "");
        w::row(ui, "Start Audian when I sign in", "", |ui| {
            w::toggle(ui, &mut app.draft.general.start_with_windows);
        });
        w::row(ui, "Notifications", "Show Windows notifications for errors and important events.", |ui| {
            w::toggle(ui, &mut app.draft.general.notifications);
        });
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::INSERT, "Text insertion", "");
        ui.label(RichText::new("Method").size(14.5).color(text()));
        w::segmented(ui, egui::Id::new("ins-method"), &mut app.draft.insertion.method, &[(InsertMethod::Paste, "Paste (recommended)"), (InsertMethod::Type, "Simulate typing")]);
        w::hint(ui, "Pasting is instant and works in editors, browsers, chat apps and terminals. Typing is slower but works where paste is blocked.");
        w::divider(ui);
        w::row(ui, "Restore my clipboard", "Put back what you had copied after pasting. Dictated text is also kept out of clipboard history (Win+V).", |ui| {
            w::toggle(ui, &mut app.draft.insertion.restore_clipboard);
        });
        ui.add_enabled_ui(app.draft.insertion.restore_clipboard, |ui| {
            w::row(ui, "Restore after", "", |ui| {
                ui.add(egui::Slider::new(&mut app.draft.insertion.restore_delay_ms, 100..=3000).suffix(" ms"));
            });
        });
        w::row(ui, "Single line in terminals", "Prevents multi-line results from running commands by accident.", |ui| {
            w::toggle(ui, &mut app.draft.insertion.terminal_single_line);
        });
        w::row(ui, "Add a trailing space", "So consecutive dictations don't run together.", |ui| {
            w::toggle(ui, &mut app.draft.insertion.trailing_space);
        });
    });
}

pub fn privacy(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Privacy", "What stays on this PC — and what doesn't.");
    let cloud = app.draft.processing.enabled && app.draft.processing.provider == RewriteProvider::Antigravity;
    w::card(ui, |ui| {
        w::card_title(ui, icon::SHIELD, "Data flow", "");
        for (ic, title, desc, local) in [
            (icon::MIC, "Microphone audio", "Captured only while you dictate, kept in memory, discarded after transcription.", true),
            (
                icon::SPEECH,
                "Speech recognition",
                match app.draft.transcription.provider {
                    SttProvider::WhisperLocal => "Whisper runs on this computer.",
                    SttProvider::Parakeet => "NVIDIA Parakeet runs on this computer.",
                },
                true,
            ),
            (
                icon::SPARKLE,
                "Rewriting",
                if cloud { "Your transcribed text is sent via Antigravity (cloud)." } else { "Performed on this computer." },
                !cloud,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).font(super::super::theme::icons(16.0)).color(text_dim()));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(title).size(14.5).color(text()));
                    ui.label(RichText::new(desc).size(12.5).color(text_dim()));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if local { w::badge(ui, "On this PC", success()) } else { w::badge(ui, "Cloud", warn()) }
                });
            });
            ui.add_space(4.0);
        }
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::LOCK, "Stored data", "");
        w::row(ui, "Dictation history", "Text of recent dictations, for the History page and \"paste last\".", |ui| {
            w::toggle(ui, &mut app.draft.privacy.history);
        });
        ui.add_enabled_ui(app.draft.privacy.history, |ui| {
            w::row(ui, "Keep the last", "", |ui| {
                ui.add(egui::Slider::new(&mut app.draft.privacy.history_limit, 10..=1000).suffix(" entries"));
            });
        });
        w::row(ui, "Keep audio recordings", "Saves a WAV of each dictation. Off by default.", |ui| {
            if w::icon_button(ui, icon::FOLDER, "Open recordings folder").clicked() {
                super::super::open_path(&paths::recordings_dir());
            }
            w::toggle(ui, &mut app.draft.privacy.save_recordings);
        });
        w::row(ui, "Log transcripts", "Writes dictated text to the debug log, for troubleshooting.", |ui| {
            if w::icon_button(ui, icon::FOLDER, "Open logs folder").clicked() {
                super::super::open_path(&paths::logs_dir());
            }
            w::toggle(ui, &mut app.draft.privacy.log_transcripts);
        });
        w::row(ui, "Usage statistics", "Word and dictation counts shown on Home.", |ui| {
            if w::secondary(ui, "Reset").clicked() {
                history::reset_stats();
                app.stats = history::load_stats();
                app.toast("Statistics reset", true);
            }
        });
    });
}

pub fn about(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::card(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(logo) = &app.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(72.0, 72.0)));
            }
            ui.vertical(|ui| {
                ui.label(RichText::new("Audian").font(super::super::theme::semibold(30.0)).color(text()));
                ui.label(RichText::new(format!("Version {}  ·  Voice dictation for Windows", audian_common::APP_VERSION)).size(14.0).color(text_dim()));
                ui.label(RichText::new("Speak naturally — get polished text wherever you type.").size(13.0).color(text_faint()));
            });
        });
    });
    ui.add_space(12.0);
    super::home::resources_card(app, ui);
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::FOLDER, "Files", "");
        for (label, path) in [
            ("Settings", paths::config_file()),
            ("Models", paths::models_dir()),
            ("Logs", paths::logs_dir()),
            ("History", paths::history_file()),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(80.0, 20.0), egui::Label::new(RichText::new(label).size(14.0).color(text())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::icon_button(ui, icon::FOLDER, "Open").clicked() {
                        super::super::open_path(&path);
                    }
                    ui.add(egui::Label::new(RichText::new(super::display_path(&path)).size(12.0).color(text_faint())).truncate())
                        .on_hover_text(path.display().to_string());
                });
            });
        }
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::INFO, "Built with", "");
        w::hint(
            ui,
            "whisper.cpp & llama.cpp (ggml, MIT) · ONNX Runtime (MIT) · OpenAI Whisper models (MIT) · NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0) · Qwen models (Apache 2.0) · egui, tiny-skia, cpal and the windows crate. Audian is MIT licensed.",
        );
    });
}
