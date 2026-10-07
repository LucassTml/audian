//! Shortcut & recording, Microphone & sound.

use audian_common::config::RecordingMode;
use eframe::egui::{self, RichText};

use super::super::theme::{icon, *};
use super::super::widgets as w;
use super::super::SettingsApp;
use crate::hotkey::{self, Hotkey};

#[derive(Clone, Copy, PartialEq)]
enum PausePreset {
    Quick,
    Balanced,
    Patient,
    Custom,
}

fn preset_of(secs: f32) -> PausePreset {
    match (secs * 10.0).round() as i32 {
        12 => PausePreset::Quick,
        20 => PausePreset::Balanced,
        30 => PausePreset::Patient,
        _ => PausePreset::Custom,
    }
}

pub fn shortcut(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Shortcut & recording", "How you start and finish a dictation.");

    w::card(ui, |ui| {
        w::card_title(ui, icon::KEYBOARD, "Dictation shortcut", "Works in every app, even when Audian is in the background.");
        ui.add_space(4.0);
        w::keycaps(ui, &app.shortcut.value(), 17.0);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut app.shortcut.ctrl, "Ctrl");
            ui.checkbox(&mut app.shortcut.alt, "Alt");
            ui.checkbox(&mut app.shortcut.shift, "Shift");
            ui.checkbox(&mut app.shortcut.win, "Win");
            ui.label(RichText::new("+").color(TEXT_FAINT));
            egui::ComboBox::from_id_salt("key").selected_text(app.shortcut.key.clone()).width(120.0).show_ui(ui, |ui| {
                for k in hotkey::selectable_keys() {
                    ui.selectable_value(&mut app.shortcut.key, k.clone(), k);
                }
            });
            let label = if app.shortcut.recording { "Press keys…" } else { "Record" };
            if w::secondary_icon(ui, icon::KEYBOARD, label).clicked() {
                app.shortcut.recording = !app.shortcut.recording;
            }
        });
        if app.shortcut.recording {
            capture_shortcut(app, ui.ctx());
        }
        ui.add_space(2.0);
        match app.check_shortcut() {
            Ok(()) => {
                ui.label(RichText::new(format!("{}  Available", icon::CHECK)).font(super::super::theme::icons(12.5)).color(SUCCESS));
            }
            Err(e) => {
                ui.label(RichText::new(format!("{e} — keeping {}", app.saved.hotkey.shortcut)).size(13.0).color(WARN));
            }
        }
        let s = &app.shortcut;
        if !(s.ctrl || s.alt || s.shift || s.win) && !s.key.starts_with('F') {
            w::hint(ui, "Tip: without a modifier key, that key stops working normally in other apps.");
        }
        if app.shortcut.value().eq_ignore_ascii_case("Ctrl+Shift+V") {
            w::hint(ui, "Note: Ctrl+Shift+V is \"paste as plain text\" in browsers and Office, and Markdown preview in VS Code; Audian takes it over while running.");
        }
    });
    ui.add_space(12.0);

    w::card(ui, |ui| {
        w::card_title(ui, icon::MIC, "Recording style", "");
        w::segmented(
            ui,
            egui::Id::new("rec-mode"),
            &mut app.draft.hotkey.mode,
            &[(RecordingMode::Hybrid, "Hybrid"), (RecordingMode::Hold, "Push-to-talk"), (RecordingMode::Toggle, "Toggle")],
        );
        ui.add_space(4.0);
        w::hint(ui, app.draft.hotkey.mode.description());
        if app.draft.hotkey.mode == RecordingMode::Hybrid {
            w::hint(ui, "Hold the shortcut while you speak and release to finish — or tap it once and just talk: Audian finishes when you pause.");
        }
        w::divider(ui);
        w::row(ui, "Esc cancels", "Discard a dictation that is recording or processing.", |ui| {
            w::toggle(ui, &mut app.draft.hotkey.escape_cancels);
        });
    });
    ui.add_space(12.0);

    w::card(ui, |ui| {
        w::card_title(ui, icon::CLOCK, "Finish automatically", "For hands-free dictation (tap, or Toggle style).");
        w::row(ui, "Stop when I stop talking", "Audian listens for the end of your speech.", |ui| {
            w::toggle(ui, &mut app.draft.auto_stop.enabled);
        });
        ui.add_enabled_ui(app.draft.auto_stop.enabled, |ui| {
            w::divider(ui);
            ui.label(RichText::new("Pause before finishing").size(14.5).color(TEXT));
            w::hint(ui, "How long you can stay quiet before Audian considers you done. Longer = more room to think mid-sentence.");
            let mut preset = preset_of(app.draft.auto_stop.pause_secs);
            if w::segmented(
                ui,
                egui::Id::new("pause-preset"),
                &mut preset,
                &[(PausePreset::Quick, "Quick · 1.2 s"), (PausePreset::Balanced, "Balanced · 2 s"), (PausePreset::Patient, "Patient · 3 s"), (PausePreset::Custom, "Custom")],
            ) {
                app.draft.auto_stop.pause_secs = match preset {
                    PausePreset::Quick => 1.2,
                    PausePreset::Balanced => 2.0,
                    PausePreset::Patient => 3.0,
                    PausePreset::Custom => app.draft.auto_stop.pause_secs + 0.1,
                };
            }
            ui.add(egui::Slider::new(&mut app.draft.auto_stop.pause_secs, 0.6..=6.0).step_by(0.1).suffix(" s"));
            w::divider(ui);
            w::row(
                ui,
                "Smart sentence detection",
                "Checks what you said so far: if the sentence sounds unfinished (\"…and\", \"…because\", \"…porque\"), Audian waits longer. Also makes results appear faster.",
                |ui| {
                    w::toggle(ui, &mut app.draft.auto_stop.smart);
                },
            );
            ui.add_enabled_ui(app.draft.auto_stop.smart, |ui| {
                w::row(ui, "Extra patience for unfinished sentences", "", |ui| {
                    ui.add(egui::Slider::new(&mut app.draft.auto_stop.unfinished_extra_secs, 0.0..=5.0).step_by(0.1).suffix(" s"));
                });
            });
            w::row(ui, "Give up if nothing is said for", "", |ui| {
                ui.add(egui::Slider::new(&mut app.draft.auto_stop.no_speech_secs, 3.0..=30.0).step_by(1.0).suffix(" s"));
            });
            let max = app.draft.auto_stop.pause_secs + if app.draft.auto_stop.smart { app.draft.auto_stop.unfinished_extra_secs } else { 0.0 };
            w::hint(
                ui,
                &format!(
                    "Finishes {:.1} s after a complete sentence{}.",
                    app.draft.auto_stop.pause_secs,
                    if app.draft.auto_stop.smart { format!(", up to {max:.1} s if it sounds unfinished") } else { String::new() }
                ),
            );
        });
    });
    ui.add_space(12.0);

    w::card(ui, |ui| {
        w::card_title(ui, icon::INSERT, "Paste last dictation", "Re-insert your previous result into the current app.");
        w::row(ui, "Shortcut", "Leave empty to disable.", |ui| {
            ui.add(egui::TextEdit::singleline(&mut app.draft.hotkey.paste_last_shortcut).desired_width(180.0).hint_text("e.g. Ctrl+Shift+Alt+V"));
        });
        let paste = app.draft.hotkey.paste_last_shortcut.trim();
        if !paste.is_empty() {
            if let Err(e) = Hotkey::parse(paste) {
                ui.label(RichText::new(e.to_string()).size(13.0).color(WARN));
            }
        }
    });
}

fn capture_shortcut(app: &mut SettingsApp, ctx: &egui::Context) {
    let events = ctx.input(|i| i.events.clone());
    for e in events {
        if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
            if key == egui::Key::Escape {
                app.shortcut.recording = false;
                return;
            }
            let name = match key {
                egui::Key::Minus => "-".to_string(),
                egui::Key::Equals => "=".to_string(),
                egui::Key::OpenBracket => "[".to_string(),
                egui::Key::CloseBracket => "]".to_string(),
                egui::Key::Backslash => "\\".to_string(),
                egui::Key::Semicolon => ";".to_string(),
                egui::Key::Quote => "'".to_string(),
                egui::Key::Comma => ",".to_string(),
                egui::Key::Period => ".".to_string(),
                egui::Key::Slash => "/".to_string(),
                egui::Key::Backtick => "`".to_string(),
                k => k.name().to_string(),
            };
            if let Some(vk) = hotkey::key_from_name(&name) {
                app.shortcut.key = hotkey::key_name(vk);
                app.shortcut.ctrl = modifiers.ctrl;
                app.shortcut.alt = modifiers.alt;
                app.shortcut.shift = modifiers.shift;
                app.shortcut.recording = false;
            }
        }
    }
}

pub fn mic_picker(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let selected = if app.draft.audio.device_id.is_empty() { "System default".to_string() } else { app.draft.audio.device_name.clone() };
    if w::icon_button(ui, "\u{E72C}", "Refresh devices").clicked() {
        app.devices = crate::audio::list_input_devices();
    }
    egui::ComboBox::from_id_salt("mic").selected_text(selected).width(300.0).show_ui(ui, |ui| {
        if ui.selectable_label(app.draft.audio.device_id.is_empty(), "System default").clicked() {
            app.draft.audio.device_id.clear();
            app.draft.audio.device_name.clear();
        }
        for d in &app.devices {
            let label = if d.is_default { format!("{} (default)", d.name) } else { d.name.clone() };
            if ui.selectable_label(app.draft.audio.device_id == d.id, label).clicked() {
                app.draft.audio.device_id = d.id.clone();
                app.draft.audio.device_name = d.name.clone();
            }
        }
    });
}

pub fn mic_test(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let running = app.mic_test.is_some();
        let clicked = if running { w::secondary_icon(ui, icon::STOP, "Stop test") } else { w::secondary_icon(ui, icon::PLAY, "Test microphone") }.clicked();
        if clicked {
            app.toggle_mic_test();
        }
        if let Some(t) = &app.mic_test {
            if let Some(err) = t.error() {
                ui.label(RichText::new(err).size(13.0).color(WARN));
            } else {
                ui.vertical(|ui| {
                    w::level_meter(ui, t.level(), app.draft.audio.silence_threshold, 300.0);
                    let state = if t.peak() < app.draft.audio.silence_threshold { "speak to test" } else { "signal OK" };
                    ui.label(RichText::new(format!("{}  ·  {state}", t.device())).size(12.0).color(TEXT_DIM));
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
        }
    });
}

pub fn audio(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Microphone & sound", "Input device, sensitivity and audio cues.");
    w::card(ui, |ui| {
        w::card_title(ui, icon::MIC, "Microphone", "The microphone is only open while you dictate.");
        w::row(ui, "Input device", "", |ui| mic_picker(app, ui));
        w::divider(ui);
        mic_test(app, ui);
        w::divider(ui);
        w::row(ui, "Input gain", "Boost quiet microphones.", |ui| {
            ui.add(egui::Slider::new(&mut app.draft.audio.gain, 0.5..=4.0).step_by(0.1).suffix("×"));
        });
        w::row(ui, "Voice detection sensitivity", "Higher picks up softer speech; lower ignores more background noise.", |ui| {
            ui.add(egui::Slider::new(&mut app.draft.audio.vad_sensitivity, 0.0..=1.0).step_by(0.05));
        });
        w::row(ui, "Silence threshold", "Recordings quieter than this (yellow mark on the meter) are discarded.", |ui| {
            ui.add(egui::Slider::new(&mut app.draft.audio.silence_threshold, 0.001..=0.05).logarithmic(true));
        });
        w::row(ui, "Maximum length", "", |ui| {
            ui.add(egui::Slider::new(&mut app.draft.audio.max_duration_secs, 10..=1800).suffix(" s"));
        });
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::VOLUME, "Sound cues", "Soft chimes when listening starts and stops.");
        w::row(ui, "Play sounds", "", |ui| {
            w::toggle(ui, &mut app.draft.general.sounds);
        });
        ui.add_enabled_ui(app.draft.general.sounds, |ui| {
            w::row(ui, "Volume", "", |ui| {
                if w::icon_button(ui, icon::PLAY, "Preview").clicked() {
                    crate::sound::play(crate::sound::Cue::Start, app.draft.general.sound_volume);
                }
                ui.add(egui::Slider::new(&mut app.draft.general.sound_volume, 0.05..=1.0).show_value(false));
            });
        });
    });
}
