//! Home dashboard: status, statistics, quick toggles, live resource usage, recent dictations.

use audian_common::config::ProcessingMode;
use eframe::egui::{self, Align2, Color32, RichText, UiBuilder, vec2};

use super::super::theme::{self, icon, *};
use super::super::widgets as w;
use super::super::{Page, SettingsApp};

fn greeting() -> &'static str {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    match t.wHour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    // Hero banner.
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 172.0), egui::Sense::hover());
    app.paint_hero(ui, rect, 0.52);
    let mut hero = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(28.0, 22.0))));
    hero.spacing_mut().item_spacing.y = 6.0;
    hero.horizontal(|ui| {
        if app.daemon_running {
            w::status_dot(ui, theme::DARK.success, true);
            ui.label(RichText::new("Ready").size(13.0).color(Color32::from_rgb(200, 240, 220)));
        } else {
            w::status_dot(ui, theme::DARK.warn, false);
            ui.label(RichText::new("Audian is not running").size(13.0).color(Color32::from_rgb(255, 225, 180)));
        }
    });
    hero.label(RichText::new(greeting()).font(theme::semibold(26.0)).color(Color32::WHITE));
    hero.horizontal(|ui| {
        ui.label(RichText::new("Press").size(14.5).color(Color32::from_rgb(214, 214, 236)));
        w::keycaps(ui, &app.saved.hotkey.shortcut, 13.0);
        ui.label(RichText::new("anywhere and start talking.").size(14.5).color(Color32::from_rgb(214, 214, 236)));
    });
    if !app.daemon_running {
        hero.add_space(2.0);
        if w::primary_icon(&mut hero, icon::POWER, "Start Audian").clicked() {
            super::super::start_daemon();
        }
    }
    ui.add_space(14.0);

    // Statistics.
    let stats = app.stats.clone();
    let avg_ms = {
        let recent: Vec<u64> = app.history.iter().take(30).map(|e| e.process_ms).filter(|&m| m > 0).collect();
        if recent.is_empty() { None } else { Some(recent.iter().sum::<u64>() as f64 / recent.len() as f64) }
    };
    let tw = super::tile_width(ui, 4);
    ui.horizontal(|ui| {
        w::stat_tile(ui, tw, icon::TEXT, &format_count(stats.words), "words dictated", accent());
        w::stat_tile(ui, tw, icon::CLOCK, &format_minutes(stats.minutes_saved()), "saved vs typing", success());
        w::stat_tile(ui, tw, icon::MIC, &format_count(stats.dictations), "dictations", accent2());
        w::stat_tile(
            ui,
            tw,
            icon::SPARKLE,
            &avg_ms.map(|m| format!("{:.1} s", m / 1000.0)).unwrap_or_else(|| "—".into()),
            "avg. processing",
            warn(),
        );
    });
    ui.add_space(14.0);

    // Quick settings + live resources: side by side when there is room, stacked otherwise.
    if ui.available_width() >= 700.0 {
        let half = super::tile_width(ui, 2);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(half, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| quick_settings(app, ui));
            ui.allocate_ui_with_layout(vec2(half, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| resources_card(app, ui));
        });
    } else {
        quick_settings(app, ui);
        ui.add_space(12.0);
        resources_card(app, ui);
    }
    ui.add_space(14.0);

    // Recent dictations.
    w::card(ui, |ui| {
        ui.horizontal(|ui| {
            w::card_title(ui, icon::HISTORY, "Recent dictations", "");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui.link("View all").clicked() {
                    app.go(Page::History);
                }
            });
        });
        if !app.saved.privacy.history {
            w::hint(ui, "History is turned off (Privacy).");
        } else if app.history.is_empty() {
            w::hint(ui, "Nothing yet. Press your shortcut in any text field and say something.");
        }
        let entries: Vec<_> = app.history.iter().take(4).cloned().collect();
        for (i, e) in entries.iter().enumerate() {
            if i > 0 {
                w::divider(ui);
            }
            ui.horizontal(|ui| {
                let text_w = (ui.available_width() - 48.0).max(120.0);
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(format!("{}  ·  {}  ·  {}", e.time, if e.app.is_empty() { "—" } else { &e.app }, e.mode)).size(12.0).color(text_faint()));
                    let text: String = e.text.chars().take(180).collect();
                    ui.add(egui::Label::new(RichText::new(text).size(14.0).color(theme::text())).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::icon_button(ui, icon::COPY, "Copy").clicked() {
                        ui.ctx().copy_text(e.text.clone());
                        app.toast("Copied", true);
                    }
                });
            });
        }
    });
}

fn quick_settings(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::card(ui, |ui| {
        w::card_title(ui, icon::SETTINGS, "Quick settings", "");
        w::row(ui, "AI rewriting", "", |ui| {
            w::toggle(ui, &mut app.draft.processing.enabled);
        });
        w::row(ui, "Auto-stop when I pause", "", |ui| {
            w::toggle(ui, &mut app.draft.auto_stop.enabled);
        });
        w::row(ui, "Per-app modes", "", |ui| {
            w::toggle(ui, &mut app.draft.profiles.enabled);
        });
        w::row(ui, "Sound cues", "", |ui| {
            w::toggle(ui, &mut app.draft.general.sounds);
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Default mode").size(12.5).color(text_dim()));
        w::segmented(
            ui,
            egui::Id::new("home-mode"),
            &mut app.draft.processing.mode,
            &[
                (ProcessingMode::Natural, "Natural"),
                (ProcessingMode::AiPrompt, "Prompt"),
                (ProcessingMode::Professional, "Pro"),
                (ProcessingMode::Document, "Doc"),
                (ProcessingMode::Literal, "Literal"),
            ],
        );
    });

}

pub fn resources_card(app: &mut SettingsApp, ui: &mut egui::Ui) {
    app.monitor.tick();
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(1500));
    let snap = &app.monitor.snapshot;
    let speech_note = keep_note(crate::stt::keep_loaded_minutes(&app.saved));
    let keep_llm = app.saved.processing.local_llm.keep_loaded_minutes;
    w::card(ui, |ui| {
        w::card_title(ui, icon::CHIP, "Resource usage", "Live — engines only run while needed");
        for (name, stat, idle) in [
            ("Audian (tray)", &snap.tray, "not running"),
            ("Speech engine", &snap.speech, "asleep"),
            ("Rewriting engine", &snap.rewrite, "asleep"),
        ] {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, if stat.running { success() } else { text_faint() });
                ui.label(RichText::new(name).size(14.0).color(text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if stat.running {
                        ui.label(RichText::new(format!("{:.0} MB  ·  {:.1}% CPU", stat.memory_mb, stat.cpu_percent)).size(13.0).color(text_dim()));
                    } else {
                        ui.label(RichText::new(idle).size(13.0).color(text_faint()));
                    }
                });
            });
        }
        ui.add_space(2.0);
        w::hint(ui, &format!("Engines wake when you dictate. Speech: {}. Rewriting: {}.", speech_note, keep_note(keep_llm)));
        let total: f64 = [&snap.tray, &snap.speech, &snap.rewrite].iter().map(|s| s.memory_mb).sum();
        ui.label(
            RichText::new(format!(
                "Total {:.0} MB of {:.0} GB RAM  ·  GPU: not used (CPU only)",
                total, snap.system_ram_gb
            ))
            .size(12.0)
            .color(text_faint()),
        );
    });
    let _ = Align2::LEFT_CENTER;
}

fn keep_note(minutes: u32) -> String {
    if minutes == 0 { "unloads after each use".into() } else { format!("unloads after {minutes} min idle") }
}

fn format_count(n: u64) -> String {
    if n >= 10_000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

fn format_minutes(m: f64) -> String {
    if m >= 90.0 { format!("{:.1} h", m / 60.0) } else { format!("{:.0} min", m) }
}
