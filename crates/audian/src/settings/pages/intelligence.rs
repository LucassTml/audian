//! Speech recognition, Rewriting and Per-app modes.

use audian_common::catalog::{self, ModelKind};
use audian_common::config::{AppProfile, ProcessingMode, RewriteProvider, SttProvider};
use audian_common::download::State;
use eframe::egui::{self, RichText, vec2};

use super::super::theme::{icon, *};
use super::super::widgets as w;
use super::super::{Page, SettingsApp};
use super::{LANGUAGES, language_name, mode_icon, tile_width, tiles_per_row};

fn installed(app: &SettingsApp, ext: &str) -> Vec<String> {
    let ext = format!(".{ext}");
    app.model_files.iter().map(|(f, _)| f.clone()).filter(|f| f.to_lowercase().ends_with(&ext)).collect()
}

fn model_label(file: &str) -> String {
    catalog::find(file).map(|m| format!("{}  ·  {}", m.name, catalog::format_size(m.size))).unwrap_or_else(|| file.to_string())
}

fn model_combo(ui: &mut egui::Ui, id: &str, value: &mut String, files: &[String]) {
    egui::ComboBox::from_id_salt(id).selected_text(model_label(value)).width(320.0).show_ui(ui, |ui| {
        if files.is_empty() {
            ui.label("No models installed yet");
        }
        for f in files {
            ui.selectable_value(value, f.clone(), model_label(f));
        }
    });
}

const BALANCED_MODEL: &str = "Qwen3.5-2B-Q4_K_M.gguf";
const FAST_MODEL: &str = "Qwen3.5-0.8B-Q4_K_M.gguf";

/// Download button / progress for the selected rewriting model when it is not installed yet.
fn model_download_row(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let Some(m) = catalog::find(&app.draft.processing.local_llm.model) else { return };
    if app.model_installed(m) {
        return;
    }
    let dl = app.download_for(m.file).cloned();
    w::row(ui, &format!("{} is not downloaded", m.name), &format!("{} · needed before it can be used.", catalog::format_size(m.size)), |ui| {
        match dl.map(|d| (d.state(), d)) {
            Some((State::Downloading, d)) => w::progress(ui, d.fraction(), 180.0),
            Some((State::Verifying, _)) => {
                ui.label(RichText::new("Verifying…").size(13.0).color(text_dim()));
            }
            Some((State::Failed(e), _)) => {
                if w::secondary(ui, "Retry").clicked() {
                    app.download(m);
                }
                ui.label(RichText::new(e).size(12.0).color(warn()));
            }
            _ => {
                if w::secondary_icon(ui, icon::DOWNLOAD, "Download").clicked() {
                    app.download(m);
                }
            }
        }
    });
}

/// How long an engine stays loaded after a dictation. Stored as minutes; values set by hand in
/// the config file snap to the nearest choice.
fn keep_control(ui: &mut egui::Ui, id: &str, minutes: &mut u32) {
    const CHOICES: [(u32, &str); 5] = [(0, "After use"), (1, "1 min"), (5, "5 min"), (15, "15 min"), (60, "1 h")];
    let mut k = CHOICES.iter().map(|c| c.0).min_by_key(|c| c.abs_diff(*minutes)).unwrap_or(0);
    if w::segmented(ui, egui::Id::new(id), &mut k, &CHOICES) {
        *minutes = k;
    }
}

pub fn speech(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Speech recognition", "Turns your voice into text — always on this computer.");
    w::card(ui, |ui| {
        w::card_title(ui, icon::SPEECH, "Engine", "");
        let per_row = 2;
        let tw = tile_width(ui, per_row);
        let engines = [
            (SttProvider::Parakeet, "NVIDIA Parakeet", icon::SPARKLE, "Recommended. Fast and accurate on any PC. 25 European languages."),
            (SttProvider::WhisperLocal, "Whisper", icon::GLOBE, "OpenAI Whisper. 99 languages and custom vocabulary."),
        ];
        for chunk in engines.chunks(per_row) {
            ui.horizontal(|ui| {
                for &(p, title, ic, desc) in chunk {
                    if w::choice_tile(ui, vec2(tw, 92.0), ic, title, desc, app.draft.transcription.provider == p).clicked() {
                        app.draft.transcription.provider = p;
                    }
                }
            });
        }
        ui.add_space(4.0);
        match app.draft.transcription.provider {
            SttProvider::WhisperLocal => whisper_options(app, ui),
            SttProvider::Parakeet => parakeet_options(app, ui),
        }
    });
    ui.add_space(12.0);
    language_card(app, ui);
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::TEXT, "Custom vocabulary", "Names, products and jargon to spell correctly — one per line.");
        if app.draft.transcription.provider == SttProvider::Parakeet {
            w::hint(ui, "Used by Whisper only; the rewriting step still sees these words in your text.");
        }
        ui.add(egui::TextEdit::multiline(&mut app.vocabulary_text).desired_rows(4).desired_width(f32::INFINITY).hint_text("Audian\nKubernetes\nMaria Fernandes"));
    });
}

fn local_badges(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        w::badge(ui, "Private", success());
        w::badge(ui, "Offline", success());
    });
}

fn whisper_options(app: &mut SettingsApp, ui: &mut egui::Ui) {
    local_badges(ui);
    w::divider(ui);
    let files = installed(app, "bin");
    w::row(ui, "Model", "Accuracy vs. speed. Download more under Models.", |ui| {
        if ui.link("Manage models").clicked() {
            app.go(Page::Models);
        }
        model_combo(ui, "sttmodel", &mut app.draft.transcription.whisper.model, &files);
    });
    if let Some(m) = catalog::find(&app.draft.transcription.whisper.model) {
        w::hint(ui, &format!("{} — {} · ~{} MB RAM while loaded · {}", m.name, m.summary, m.ram_mb, m.speed));
    }
    w::divider(ui);
    keep_and_threads(ui, "keep-stt", &mut app.draft.transcription.whisper.keep_loaded_minutes, &mut app.draft.transcription.whisper.threads);
}

fn parakeet_options(app: &mut SettingsApp, ui: &mut egui::Ui) {
    local_badges(ui);
    w::divider(ui);
    match catalog::find(&app.draft.transcription.parakeet.model) {
        Some(m) => super::models::model_row(app, ui, m),
        None => w::hint(ui, &format!("Custom model folder: {}", app.draft.transcription.parakeet.model)),
    }
    w::hint(
        ui,
        "Detects the language by itself among 25 European languages, including English, Portuguese, Spanish, French, German and Italian. Model by NVIDIA (CC-BY-4.0).",
    );
    w::divider(ui);
    keep_and_threads(ui, "keep-pk", &mut app.draft.transcription.parakeet.keep_loaded_minutes, &mut app.draft.transcription.parakeet.threads);
}

fn keep_and_threads(ui: &mut egui::Ui, id: &str, keep_minutes: &mut u32, threads: &mut u32) {
    ui.label(RichText::new("Keep the model in memory").size(14.5).color(text()));
    w::hint(ui, "The engine loads when you press the shortcut (while you speak) and exits after this much idle time, giving all its memory back. \"After use\" frees it as soon as the text is inserted.");
    keep_control(ui, id, keep_minutes);
    w::row(ui, "CPU threads", "0 = automatic (half your logical cores, max 8).", |ui| {
        ui.add(egui::DragValue::new(threads).range(0..=64));
    });
}

fn language_card(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::card(ui, |ui| {
        w::card_title(ui, icon::GLOBE, "Language", "");
        if app.draft.transcription.provider == SttProvider::Parakeet {
            w::hint(ui, "Parakeet detects the language by itself; your languages below help label it for rewriting.");
        }
        w::row(ui, "Spoken language", "", |ui| {
            let current = if app.draft.transcription.language == "auto" { "Auto-detect".to_string() } else { language_name(&app.draft.transcription.language) };
            egui::ComboBox::from_id_salt("lang").selected_text(current).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut app.draft.transcription.language, "auto".to_string(), "Auto-detect");
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut app.draft.transcription.language, code.to_string(), *name);
                }
            });
        });
        if app.draft.transcription.language == "auto" {
            ui.label(RichText::new("Languages you speak").size(14.5).color(text()));
            w::hint(ui, "Auto-detect only chooses among these — faster, and avoids mix-ups like Portuguese → Galician on short phrases. None selected = any language.");
            ui.horizontal_wrapped(|ui| {
                for (code, name) in LANGUAGES {
                    let on = app.draft.transcription.auto_languages.iter().any(|l| l == code);
                    let resp = ui.add(egui::Button::selectable(on, RichText::new(*name).size(13.0)).corner_radius(14.0).min_size(vec2(0.0, 28.0)));
                    if resp.clicked() {
                        app.draft.transcription.auto_languages.retain(|l| l != code);
                        if !on {
                            app.draft.transcription.auto_languages.push(code.to_string());
                        }
                    }
                }
            });
        }
    });
}

pub fn rewriting(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Rewriting", "Turns what you said into what you meant.");
    w::card(ui, |ui| {
        w::row(ui, "Rewrite with AI", "Off: insert the raw transcription with minimal cleanup.", |ui| {
            w::toggle(ui, &mut app.draft.processing.enabled);
        });
    });
    ui.add_space(12.0);
    ui.add_enabled_ui(app.draft.processing.enabled, |ui| {
        w::card(ui, |ui| {
            w::card_title(ui, icon::SPARKLE, "Default mode", "Per-app modes can override this automatically.");
            let per_row = tiles_per_row(ui);
            let tw = tile_width(ui, per_row);
            for chunk in ProcessingMode::ALL.chunks(per_row) {
                ui.horizontal(|ui| {
                    for &mode in chunk {
                        if w::choice_tile(ui, vec2(tw, 92.0), mode_icon(mode), mode.label(), mode.description(), app.draft.processing.mode == mode).clicked() {
                            app.draft.processing.mode = mode;
                        }
                    }
                });
            }
        });
        ui.add_space(12.0);
        w::card(ui, |ui| {
            w::card_title(ui, icon::CHIP, "AI provider", "");
            let per_row = tiles_per_row(ui);
            let tw = tile_width(ui, per_row);
            let providers = [
                (RewriteProvider::LocalLlm, icon::LOCK, "Small model on this PC. Private and offline."),
                (RewriteProvider::Antigravity, icon::CLOUD, "Your Antigravity account. Best quality, needs internet."),
                (RewriteProvider::Rules, icon::EDIT, "No AI: removes fillers and fixes spacing. Instant."),
            ];
            for chunk in providers.chunks(per_row) {
                ui.horizontal(|ui| {
                    for &(p, ic, desc) in chunk {
                        if w::choice_tile(ui, vec2(tw, 92.0), ic, p.label(), desc, app.draft.processing.provider == p).clicked() {
                            app.draft.processing.provider = p;
                        }
                    }
                });
            }
            ui.add_space(4.0);
            match app.draft.processing.provider {
                RewriteProvider::LocalLlm => {
                    ui.horizontal(|ui| {
                        w::badge(ui, "Private", success());
                        w::badge(ui, "Offline", success());
                    });
                    ui.label(RichText::new("Model").size(14.5).color(text()));
                    let tw = tile_width(ui, 2);
                    ui.horizontal(|ui| {
                        for &(file, title, ic, desc) in &[
                            (BALANCED_MODEL, "Balanced", icon::SPARKLE, "Qwen 3.5 2B. Best results, including spoken corrections."),
                            (FAST_MODEL, "Fast", icon::CLOCK, "Qwen 3.5 0.8B. About 1.5× faster on the CPU, but misses most spoken corrections."),
                        ] {
                            if w::choice_tile(ui, vec2(tw, 92.0), ic, title, desc, app.draft.processing.local_llm.model == file).clicked() {
                                app.draft.processing.local_llm.model = file.into();
                            }
                        }
                    });
                    model_download_row(app, ui);
                    let files = installed(app, "gguf");
                    w::row(ui, "Other models", "", |ui| {
                        if ui.link("Manage models").clicked() {
                            app.go(Page::Models);
                        }
                        model_combo(ui, "llmmodel", &mut app.draft.processing.local_llm.model, &files);
                    });
                    if let Some(m) = catalog::find(&app.draft.processing.local_llm.model) {
                        w::hint(ui, &format!("{} · ~{} MB RAM while loaded · {}", m.summary, m.ram_mb, m.speed));
                    }
                    let gpu_note = match crate::text::local_llm::gpu_status() {
                        None => {
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                            "Looking for a graphics card…".to_string()
                        }
                        Some(Some(gpu)) => format!("Runs on your {} — several times faster than the CPU. Falls back to the CPU if the card has a problem.", gpu.name),
                        Some(None) => "No dedicated graphics card found, so the CPU is used. (Built-in graphics are slower than the CPU for this.)".to_string(),
                    };
                    w::row(ui, "Use the graphics card", &gpu_note, |ui| {
                        w::toggle(ui, &mut app.draft.processing.local_llm.gpu);
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Keep the model in memory").size(14.5).color(text()));
                    w::hint(ui, "Loads while you speak; its prompt is restored from a disk cache. \"After use\" frees its memory as soon as the text is inserted.");
                    keep_control(ui, "keep-llm", &mut app.draft.processing.local_llm.keep_loaded_minutes);
                    w::row(ui, "CPU threads", "0 = automatic.", |ui| {
                        ui.add(egui::DragValue::new(&mut app.draft.processing.local_llm.threads).range(0..=64));
                    });
                }
                RewriteProvider::Antigravity => {
                    ui.horizontal(|ui| {
                        w::badge(ui, "Cloud", warn());
                        ui.label(RichText::new("Your transcribed text — never audio — is sent through the Antigravity account signed in on this PC.").size(12.5).color(warn()));
                    });
                    w::row(ui, "Antigravity CLI", "", |ui| match &app.agy_path {
                        Some(p) => {
                            ui.label(RichText::new(p.display().to_string()).size(12.5).color(text_dim()));
                        }
                        None => {
                            ui.label(RichText::new("not found — install Antigravity").size(12.5).color(warn()));
                        }
                    });
                    w::row(ui, "Model", "Fast \"flash\" models with low effort respond quickest.", |ui| {
                        let models = app.agy_models.lock().unwrap().clone();
                        match models {
                            Some(list) if !list.is_empty() => {
                                egui::ComboBox::from_id_salt("agymodel").selected_text(app.draft.processing.antigravity.model.clone()).width(260.0).show_ui(ui, |ui| {
                                    for m in list {
                                        ui.selectable_value(&mut app.draft.processing.antigravity.model, m.clone(), m);
                                    }
                                });
                            }
                            _ => {
                                if ui.link("List models").clicked() {
                                    fetch_agy_models(app);
                                }
                                ui.add(egui::TextEdit::singleline(&mut app.draft.processing.antigravity.model).desired_width(220.0));
                            }
                        }
                    });
                    w::row(ui, "Timeout", "", |ui| {
                        ui.add(egui::Slider::new(&mut app.draft.processing.antigravity.timeout_secs, 5..=120).suffix(" s"));
                    });
                }
                RewriteProvider::Rules => {
                    ui.horizontal(|ui| {
                        w::badge(ui, "Instant", success());
                        w::badge(ui, "Offline", success());
                    });
                }
            }
            w::divider(ui);
            w::row(ui, "Fallback", "If the AI fails, insert a basic cleanup instead of nothing.", |ui| {
                w::toggle(ui, &mut app.draft.processing.fallback_to_rules);
            });
        });
        ui.add_space(12.0);
        w::card(ui, |ui| {
            w::card_title(ui, icon::GLOBE, "Output language", "Speak in one language, write in another.");
            w::row(ui, "Write the result in", "Translation needs an AI provider.", |ui| {
                let current = if app.draft.processing.output_language == "same" { "Same as spoken".to_string() } else { language_name(&app.draft.processing.output_language) };
                egui::ComboBox::from_id_salt("outlang").selected_text(current).width(220.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.draft.processing.output_language, "same".to_string(), "Same as spoken");
                    for (code, name) in LANGUAGES {
                        ui.selectable_value(&mut app.draft.processing.output_language, code.to_string(), *name);
                    }
                });
            });
        });
        ui.add_space(12.0);
        w::card(ui, |ui| {
            w::card_title(ui, icon::EDIT, "Custom instructions", "Added to every AI rewrite.");
            ui.add(
                egui::TextEdit::multiline(&mut app.draft.processing.custom_instructions)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("e.g. Always organize my spoken instructions into concise technical prompts. Keep code names and technical terminology unchanged."),
            );
        });
    });
}

fn fetch_agy_models(app: &SettingsApp) {
    let Some(path) = app.agy_path.clone() else { return };
    let slot = app.agy_models.clone();
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new(path).arg("models").creation_flags(crate::helper::CREATE_NO_WINDOW).output();
        let list = out
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.split_whitespace().next().map(str::to_string))
                    .filter(|m| !m.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        *slot.lock().unwrap() = Some(list);
    });
}

fn join(v: &[String]) -> String {
    v.join(", ")
}

fn split(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

pub fn profiles(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Per-app modes", "Pick the right style automatically for the app you're dictating into.");
    w::card(ui, |ui| {
        w::row(ui, "Use per-app modes", "Matched by the app's executable name or a word in its window title (useful for websites like ChatGPT in a browser).", |ui| {
            w::toggle(ui, &mut app.draft.profiles.enabled);
        });
    });
    ui.add_space(12.0);
    let mut remove = None;
    ui.add_enabled_ui(app.draft.profiles.enabled, |ui| {
        for (i, p) in app.draft.profiles.rules.iter_mut().enumerate() {
            w::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(mode_icon(p.mode)).font(super::super::theme::icons(17.0)).color(accent()));
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(200.0).font(super::super::theme::semibold(15.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::icon_button(ui, icon::DELETE, "Remove").clicked() {
                            remove = Some(i);
                        }
                        w::toggle(ui, &mut p.enabled);
                        egui::ComboBox::from_id_salt(("pmode", i)).selected_text(p.mode.label()).width(150.0).show_ui(ui, |ui| {
                            for m in ProcessingMode::ALL {
                                ui.selectable_value(&mut p.mode, m, m.label());
                            }
                        });
                    });
                });
                let mut apps = join(&p.apps);
                w::row(ui, "Apps", "Executable names, comma separated", |ui| {
                    if ui.add(egui::TextEdit::singleline(&mut apps).desired_width(340.0)).changed() {
                        p.apps = split(&apps);
                    }
                });
                let mut titles = join(&p.title_keywords);
                w::row(ui, "Window title contains", "Comma separated", |ui| {
                    if ui.add(egui::TextEdit::singleline(&mut titles).desired_width(340.0)).changed() {
                        p.title_keywords = split(&titles);
                    }
                });
            });
            ui.add_space(10.0);
        }
    });
    if let Some(i) = remove {
        app.draft.profiles.rules.remove(i);
    }
    ui.horizontal(|ui| {
        if w::secondary_icon(ui, icon::ADD, "Add profile").clicked() {
            app.draft.profiles.rules.push(AppProfile::default());
        }
        let recent: Vec<String> = {
            let mut seen = Vec::new();
            for e in &app.history {
                if !e.app.is_empty() && !seen.contains(&e.app) {
                    seen.push(e.app.clone());
                }
            }
            seen.into_iter().take(6).collect()
        };
        if !recent.is_empty() {
            ui.label(RichText::new(format!("Recently used: {}", recent.iter().map(|a| format!("{a}.exe")).collect::<Vec<_>>().join(", "))).size(12.5).color(text_faint()));
        }
    });
    let _ = ModelKind::Speech;
}
