//! First-run welcome: models, microphone, shortcut.

use audian_common::catalog;
use audian_common::download::State;
use eframe::egui::{self, Color32, RichText, UiBuilder, vec2};

use super::super::theme::{self, icon, *};
use super::super::widgets as w;
use super::super::{Page, SettingsApp};

fn step(ui: &mut egui::Ui, n: u32, done: bool, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
        let p = ui.painter();
        if done {
            p.circle_filled(r.center(), 14.0, SUCCESS.linear_multiply(0.2));
            p.text(r.center(), egui::Align2::CENTER_CENTER, icon::CHECK, theme::icons(13.0), SUCCESS);
        } else {
            p.circle_filled(r.center(), 14.0, accent().linear_multiply(0.2));
            p.text(r.center(), egui::Align2::CENTER_CENTER, n.to_string(), theme::semibold(14.0), accent());
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(title).font(theme::semibold(16.0)).color(TEXT));
            ui.label(RichText::new(subtitle).size(12.5).color(TEXT_DIM));
        });
    });
    ui.add_space(6.0);
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 150.0), egui::Sense::hover());
    app.paint_hero(ui, rect, 0.64);
    let mut hero = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(28.0, 24.0))));
    hero.horizontal(|ui| {
        if let Some(logo) = &app.logo {
            ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(64.0, 64.0)));
        }
        let text_w = (rect.width() * 0.60 - 100.0).max(240.0);
        ui.allocate_ui_with_layout(vec2(text_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.label(RichText::new("Welcome to Audian").font(theme::semibold(28.0)).color(Color32::WHITE));
            ui.add(
                egui::Label::new(RichText::new("Speak naturally — Audian writes what you meant, wherever you type. It all runs on this PC.").size(14.5).color(Color32::from_rgb(214, 214, 236)))
                    .wrap(),
            );
        });
    });
    ui.add_space(16.0);

    let recommended: Vec<&'static catalog::ModelInfo> = catalog::recommended().collect();
    let models_ready = recommended.iter().all(|m| app.model_installed(m));
    w::card(ui, |ui| {
        step(ui, 1, models_ready, "Get the AI models", "Speech recognition + rewriting. Downloaded once, then everything works offline.");
        for m in &recommended {
            ui.horizontal(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new(m.name).size(14.0).color(TEXT));
                ui.label(RichText::new(format!("{} · ~{} MB RAM", catalog::format_size(m.size), m.ram_mb)).size(12.5).color(TEXT_FAINT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if app.model_installed(m) {
                        w::badge(ui, "Ready", SUCCESS);
                    } else if let Some(d) = app.download_for(m.file).cloned() {
                        match d.state() {
                            State::Downloading => w::progress(ui, d.fraction(), 200.0),
                            State::Verifying => {
                                ui.label(RichText::new("Verifying…").size(12.5).color(TEXT_DIM));
                            }
                            State::Failed(e) => {
                                ui.label(RichText::new(e).size(12.0).color(WARN));
                            }
                            _ => {}
                        }
                    }
                });
            });
        }
        if !models_ready {
            let active = app.downloads.iter().any(|d| d.is_active());
            ui.horizontal(|ui| {
                ui.add_space(40.0);
                if !active {
                    let total: u64 = recommended.iter().filter(|m| !app.model_installed(m)).map(|m| m.size).sum();
                    if w::primary_icon(ui, icon::DOWNLOAD, &format!("Download ({})", catalog::format_size(total))).clicked() {
                        for m in &recommended {
                            if !app.model_installed(m) {
                                app.download(m);
                            }
                        }
                    }
                    ui.label(RichText::new("From Hugging Face, verified by checksum.").size(12.5).color(TEXT_FAINT));
                } else {
                    ui.label(RichText::new("Downloading… you can continue setting up meanwhile.").size(12.5).color(TEXT_DIM));
                }
            });
        }
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        let tested = app.mic_test.as_ref().is_some_and(|t| t.peak() >= app.draft.audio.silence_threshold);
        step(ui, 2, tested, "Check your microphone", "Pick the mic you want to use and say something.");
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            super::dictation::mic_picker(app, ui);
        });
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            super::dictation::mic_test(app, ui);
        });
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        step(ui, 3, true, "Your shortcut", "Change it any time on the Shortcut page.");
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            w::keycaps(ui, &app.saved.hotkey.shortcut, 15.0);
        });
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            w::hint(ui, "Hold it while you talk and release — or tap it once, talk, and simply pause: Audian finishes on its own. Press Esc to cancel.");
        });
    });
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        let label = if models_ready { "Start using Audian" } else { "Continue — finish downloads later" };
        if w::primary(ui, label).clicked() {
            app.draft.general.welcome_done = true;
            app.mic_test = None;
            if !app.daemon_running {
                super::super::start_daemon();
            }
            app.go(Page::Home);
        }
    });
}
