//! Audian Setup: a small, self-contained installer (per-user, no admin rights) that carries
//! the application inside it. The same executable is copied next to the app as
//! `uninstall.exe` and handles removal (`--uninstall`).
//!
//! Command line:
//!   audian-setup.exe                       interactive install
//!   audian-setup.exe --quiet [--with-models] [--no-autostart] [--no-launch]
//!   audian-setup.exe --uninstall [--quiet] [--purge]

#![windows_subsystem = "windows"]

mod install;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use audian_common::catalog;
use audian_common::download::{self, Download, State};
use audian_ui::theme::{self, icon, *};
use audian_ui::widgets as w;
use eframe::egui::{self, Color32, Margin, RichText, vec2};

use install::{Options, Progress, StepState};

#[derive(PartialEq)]
enum Screen {
    Welcome,
    Installing,
    Done,
    ConfirmUninstall,
    Uninstalled,
}

struct Setup {
    screen: Screen,
    dir: String,
    autostart: bool,
    desktop: bool,
    models: bool,
    launch: bool,
    purge: bool,
    existing: Option<String>,
    progress: install::Shared,
    downloads: Vec<Download>,
    downloads_started: bool,
    logo: Option<egui::TextureHandle>,
    error: Option<String>,
    started_at: f64,
}

impl Setup {
    fn new(uninstall: bool) -> Setup {
        let existing = install::installed_version();
        let models_missing = catalog::recommended().any(|m| !catalog::is_installed(m));
        Setup {
            screen: if uninstall { Screen::ConfirmUninstall } else { Screen::Welcome },
            dir: existing.as_ref().map(|(_, d)| d.display().to_string()).unwrap_or_else(|| install::default_dir().display().to_string()),
            autostart: true,
            desktop: false,
            models: models_missing,
            launch: true,
            purge: false,
            existing: existing.map(|(v, _)| v),
            progress: Arc::new(Mutex::new(Progress::default())),
            downloads: Vec::new(),
            downloads_started: false,
            logo: None,
            error: None,
            started_at: 0.0,
        }
    }

    fn options(&self) -> Options {
        Options { dir: self.dir.trim().into(), autostart: self.autostart, desktop_shortcut: self.desktop }
    }

    fn start_install(&mut self) {
        *self.progress.lock().unwrap() = Progress { steps: install::steps(), ..Default::default() };
        let opts = self.options();
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let result = install::install(&opts, &progress);
            let mut p = progress.lock().unwrap();
            p.finished = true;
            if let Err(e) = result {
                p.error = Some(format!("{e:#}"));
            }
        });
        self.screen = Screen::Installing;
    }

    fn header(&self, ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(64.0, 64.0)));
            }
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(title).font(theme::semibold(26.0)).color(text()));
                ui.label(RichText::new(subtitle).size(14.0).color(text_dim()));
            });
        });
        ui.add_space(18.0);
    }

    fn welcome(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (title, subtitle) = match &self.existing {
            Some(v) if v == audian_common::APP_VERSION => ("Reinstall Audian".to_string(), format!("Version {v} is already installed.")),
            Some(v) => (format!("Update Audian to {}", audian_common::APP_VERSION), format!("Version {v} is installed — your settings are kept.")),
            None => (format!("Install Audian {}", audian_common::APP_VERSION), "Voice dictation for Windows — speak, get polished text wherever you type.".to_string()),
        };
        self.header(ui, &title, &subtitle);
        w::card(ui, |ui| {
            w::row(ui, "Install location", "Per-user install — no administrator rights needed.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.dir).desired_width(320.0));
            });
            w::divider(ui);
            w::row(ui, "Start Audian when I sign in", "Runs quietly in the system tray.", |ui| {
                w::toggle(ui, &mut self.autostart);
            });
            w::row(ui, "Desktop shortcut", "", |ui| {
                w::toggle(ui, &mut self.desktop);
            });
            let missing: Vec<_> = catalog::recommended().filter(|m| !catalog::is_installed(m)).collect();
            if !missing.is_empty() {
                let size: u64 = missing.iter().map(|m| m.size).sum();
                w::row(
                    ui,
                    &format!("Download AI models now ({})", catalog::format_size(size)),
                    "Speech recognition + rewriting models. Or download them later from the app.",
                    |ui| {
                        w::toggle(ui, &mut self.models);
                    },
                );
            } else {
                w::row(ui, "AI models", "Already installed on this PC.", |ui| w::badge(ui, "Ready", success()));
            }
        });
        ui.add_space(10.0);
        w::hint(ui, "Audian runs speech recognition and rewriting on your own CPU. Nothing you say is sent anywhere unless you enable a cloud provider.");
        if !install::has_payload() {
            ui.label(RichText::new("This installer was built without the application files.").color(warn()));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.existing.is_some() { "Update" } else { "Install" };
                    if ui.add_enabled_ui(install::has_payload(), |ui| w::primary_icon(ui, icon::DOWNLOAD, label)).inner.clicked() {
                        self.started_at = ctx.input(|i| i.time);
                        self.start_install();
                    }
                    if w::secondary(ui, "Cancel").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if self.existing.is_some() && ui.link("Uninstall instead").clicked() {
                        self.screen = Screen::ConfirmUninstall;
                    }
                });
            });
        });
    }

    fn installing(&mut self, ui: &mut egui::Ui) {
        self.header(ui, "Installing Audian", "This only takes a moment.");
        let (steps, finished, error) = {
            let p = self.progress.lock().unwrap();
            (p.steps.clone(), p.finished, p.error.clone())
        };
        if finished && error.is_none() && self.models && !self.downloads_started {
            self.downloads_started = true;
            for m in catalog::recommended().filter(|m| !catalog::is_installed(m)) {
                self.downloads.push(download::start(m));
            }
        }
        w::card(ui, |ui| {
            for (label, state) in &steps {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                    let p = ui.painter();
                    match state {
                        StepState::Done => {
                            p.circle_filled(r.center(), 10.0, success().linear_multiply(0.2));
                            p.text(r.center(), egui::Align2::CENTER_CENTER, icon::CHECK, theme::icons(11.0), success());
                        }
                        StepState::Running => {
                            let t = ui.input(|i| i.time) as f32;
                            p.circle_stroke(r.center(), 8.0, egui::Stroke::new(2.0, border()));
                            let a0 = t * 5.0;
                            let pts: Vec<egui::Pos2> = (0..12).map(|k| {
                                let a = a0 + k as f32 * 0.13;
                                r.center() + vec2(a.cos(), a.sin()) * 8.0
                            }).collect();
                            p.add(egui::Shape::line(pts, egui::Stroke::new(2.0, accent())));
                        }
                        StepState::Pending => {
                            p.circle_stroke(r.center(), 8.0, egui::Stroke::new(1.5, border()));
                        }
                    }
                    let color = if *state == StepState::Pending { text_faint() } else { text() };
                    ui.label(RichText::new(label).size(14.5).color(color));
                });
            }
            for d in &self.downloads {
                ui.add_space(6.0);
                let name = catalog::find(d.file).map(|m| m.name).unwrap_or(d.file);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Downloading {name}")).size(14.0).color(text()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match d.state() {
                        State::Done => w::badge(ui, "Done", success()),
                        State::Verifying => w::badge(ui, "Verifying", accent()),
                        State::Failed(e) => {
                            ui.label(RichText::new(e).size(12.0).color(warn()));
                        }
                        State::Cancelled => w::badge(ui, "Skipped", text_dim()),
                        State::Downloading => {
                            ui.label(RichText::new(format!("{} / {}", catalog::format_size(d.received()), catalog::format_size(d.total))).size(12.5).color(text_dim()));
                        }
                    });
                });
                w::progress(ui, d.fraction(), ui.available_width());
            }
        });
        if let Some(e) = &error {
            ui.add_space(10.0);
            ui.label(RichText::new(format!("Installation failed: {e}")).color(warn()));
            if w::secondary(ui, "Try again").clicked() {
                self.start_install();
            }
            return;
        }
        let downloading = self.downloads.iter().any(|d| d.is_active());
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if downloading && w::secondary(ui, "Skip downloads (finish later in the app)").clicked() {
                        for d in &self.downloads {
                            d.cancel();
                        }
                    }
                });
            });
        });
        if finished && !downloading {
            self.screen = Screen::Done;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(60));
    }

    fn done(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.header(ui, "Audian is ready", "Installed for your account.");
        w::card(ui, |ui| {
            let shortcut = audian_common::config::Config::load().hotkey.shortcut;
            ui.label(RichText::new("How to dictate").font(theme::semibold(16.0)).color(text()));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Press").size(14.5).color(text_dim()));
                w::keycaps(ui, &shortcut, 13.0);
                ui.label(RichText::new("in any text field, speak, then release — or tap and simply pause.").size(14.5).color(text_dim()));
            });
            ui.add_space(4.0);
            w::hint(ui, "Audian lives in the system tray. Left-click its icon to open the dashboard; right-click for quick options.");
            if self.downloads.iter().any(|d| matches!(d.state(), State::Failed(_) | State::Cancelled)) || (!self.models && catalog::recommended().any(|m| !catalog::is_installed(m))) {
                ui.add_space(4.0);
                ui.label(RichText::new("Models still need to be downloaded — Audian will guide you on first launch.").size(13.0).color(warn()));
            }
        });
        ui.add_space(10.0);
        ui.checkbox(&mut self.launch, "Launch Audian now");
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::primary(ui, "Finish").clicked() {
                    if self.launch {
                        install::launch(&self.options().dir);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn confirm_uninstall(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.header(ui, "Uninstall Audian", "Removes the app, its shortcuts and its startup entry.");
        w::card(ui, |ui| {
            w::row(
                ui,
                "Also remove my data",
                &format!("Settings, history, logs and downloaded models ({}).", catalog::format_size(install::user_data_size())),
                |ui| {
                    w::toggle(ui, &mut self.purge);
                },
            );
        });
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(warn()));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::danger(ui, "Uninstall").clicked() {
                    match install::uninstall(self.purge) {
                        Ok(()) => self.screen = Screen::Uninstalled,
                        Err(e) => self.error = Some(format!("{e:#}")),
                    }
                }
                if w::secondary(ui, "Cancel").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn uninstalled(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.header(ui, "Audian was removed", if self.purge { "Your data was deleted too." } else { "Your settings and models were kept in case you reinstall." });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::primary(ui, "Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.logo.is_none() {
            let pm = audian_art::draw_logo(128);
            let img = egui::ColorImage::from_rgba_unmultiplied([128, 128], &audian_art::to_rgba(&pm));
            self.logo = Some(ctx.load_texture("logo", img, egui::TextureOptions::LINEAR));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg()).inner_margin(Margin::symmetric(34, 30)))
            .show(ui, |ui| match self.screen {
                Screen::Welcome => self.welcome(ui, &ctx),
                Screen::Installing => self.installing(ui),
                Screen::Done => self.done(ui, &ctx),
                Screen::ConfirmUninstall => self.confirm_uninstall(ui, &ctx),
                Screen::Uninstalled => self.uninstalled(ui, &ctx),
            });
        let _ = Color32::WHITE;
    }
}

fn quiet_install(args: &[String]) -> i32 {
    let opts = Options {
        dir: install::installed_version().map(|(_, d)| d).unwrap_or_else(install::default_dir),
        autostart: !args.iter().any(|a| a == "--no-autostart"),
        desktop_shortcut: args.iter().any(|a| a == "--desktop-shortcut"),
    };
    let progress: install::Shared = Arc::new(Mutex::new(Progress { steps: install::steps(), ..Default::default() }));
    if let Err(e) = install::install(&opts, &progress) {
        log::error!("install failed: {e:#}");
        return 1;
    }
    if args.iter().any(|a| a == "--with-models") {
        let downloads: Vec<Download> = catalog::recommended().filter(|m| !catalog::is_installed(m)).map(download::start).collect();
        while downloads.iter().any(|d| d.is_active()) {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if !args.iter().any(|a| a == "--no-launch") {
        install::launch(&opts.dir);
    }
    0
}

fn main() {
    audian_common::logging::init("setup");
    let args: Vec<String> = std::env::args().collect();
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let quiet = args.iter().any(|a| a == "--quiet");
    if quiet {
        let code = if uninstall {
            match install::uninstall(args.iter().any(|a| a == "--purge")) {
                Ok(()) => 0,
                Err(e) => {
                    log::error!("uninstall failed: {e:#}");
                    1
                }
            }
        } else {
            quiet_install(&args)
        };
        std::process::exit(code);
    }
    let icon = {
        let pm = audian_art::draw_logo(64);
        egui::IconData { rgba: audian_art::to_rgba(&pm), width: 64, height: 64 }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(if uninstall { "Uninstall Audian" } else { "Audian Setup" })
            .with_inner_size([760.0, 560.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(icon),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Audian Setup",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(Setup::new(uninstall)))
        }),
    );
}
