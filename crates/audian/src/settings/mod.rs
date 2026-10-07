//! The Audian window (settings, dashboard, history, models), run as its own process
//! (`audian.exe --settings [page]`) so the tray daemon never keeps a GUI toolkit or GPU
//! context in memory. Changes are saved automatically and the daemon is told to reload.

mod monitor;
mod pages;
pub use audian_ui::{theme, widgets};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audian_common::config::{AudioConfig, Config, IndicatorStyle, ThemeId};
use audian_common::download::{self, Download};
use audian_common::{catalog, history};
use eframe::egui::{self, Align2, Color32, Margin, RichText, Sense, TextureHandle, pos2, vec2};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, RegisterWindowMessageW, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::{HSTRING, PCWSTR};

use crate::app::{CONFIG_CHANGED_MESSAGE, WINDOW_CLASS};
use crate::audio::{self, DeviceInfo, Recorder};
use crate::hotkey::{self, Hotkey};
use crate::win::WideStr;
use theme::{icon, *};

const TITLE: &str = "Audian";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Welcome,
    Home,
    Shortcut,
    Audio,
    Speech,
    Rewriting,
    Profiles,
    Models,
    History,
    General,
    Appearance,
    Privacy,
    About,
}

impl Page {
    fn from_arg(s: &str) -> Option<Page> {
        Some(match s.to_ascii_lowercase().as_str() {
            "welcome" => Page::Welcome,
            "home" => Page::Home,
            "shortcut" => Page::Shortcut,
            "audio" => Page::Audio,
            "speech" => Page::Speech,
            "rewriting" => Page::Rewriting,
            "profiles" => Page::Profiles,
            "models" => Page::Models,
            "history" => Page::History,
            "general" | "insertion" => Page::General,
            "appearance" | "theme" => Page::Appearance,
            "privacy" => Page::Privacy,
            "about" => Page::About,
            _ => return None,
        })
    }
}

/// Sidebar structure: (group label, [(page, icon, label)]).
const NAV: &[(&str, &[(Page, &str, &str)])] = &[
    ("", &[(Page::Home, icon::HOME, "Home")]),
    ("DICTATION", &[(Page::Shortcut, icon::KEYBOARD, "Shortcut & recording"), (Page::Audio, icon::MIC, "Microphone & sound")]),
    (
        "INTELLIGENCE",
        &[
            (Page::Speech, icon::SPEECH, "Speech recognition"),
            (Page::Rewriting, icon::SPARKLE, "Rewriting"),
            (Page::Profiles, icon::APPS, "Per-app modes"),
            (Page::Models, icon::DOWNLOAD, "Models"),
        ],
    ),
    (
        "APP",
        &[
            (Page::History, icon::HISTORY, "History"),
            (Page::General, icon::SETTINGS, "General"),
            (Page::Appearance, icon::PALETTE, "Appearance"),
            (Page::Privacy, icon::SHIELD, "Privacy"),
            (Page::About, icon::INFO, "About"),
        ],
    ),
];

/// Live microphone level test, run on its own thread (WASAPI/COM must not share the GUI thread).
pub struct MicTest {
    level: Arc<AtomicU32>,
    peak: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    device: Arc<Mutex<String>>,
}

impl MicTest {
    fn start(cfg: AudioConfig) -> MicTest {
        let t = MicTest {
            level: Arc::new(AtomicU32::new(0)),
            peak: Arc::new(AtomicU32::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            device: Arc::new(Mutex::new(String::new())),
        };
        let (level, peak, stop, error, device) = (t.level.clone(), t.peak.clone(), t.stop.clone(), t.error.clone(), t.device.clone());
        std::thread::spawn(move || {
            let recorder = match Recorder::start(&cfg, |_| {}) {
                Ok(r) => r,
                Err(e) => {
                    *error.lock().unwrap() = Some(e.to_string());
                    return;
                }
            };
            *device.lock().unwrap() = recorder.device_name.clone();
            let meter = recorder.meter();
            let mut smoothed = 0.0f32;
            while !stop.load(Ordering::Relaxed) {
                let l = meter.take();
                smoothed = if l > smoothed { l } else { smoothed * 0.82 + l * 0.18 };
                level.store(smoothed.to_bits(), Ordering::Relaxed);
                if l > f32::from_bits(peak.load(Ordering::Relaxed)) {
                    peak.store(l.to_bits(), Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            drop(recorder.stop());
        });
        t
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    pub fn device(&self) -> String {
        self.device.lock().map(|d| d.clone()).unwrap_or_default()
    }
}

impl Drop for MicTest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct ShortcutEditor {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
    pub recording: bool,
}

impl ShortcutEditor {
    pub fn from(s: &str) -> ShortcutEditor {
        let hk = Hotkey::parse(s).ok();
        ShortcutEditor {
            ctrl: hk.is_some_and(|h| h.ctrl),
            alt: hk.is_some_and(|h| h.alt),
            shift: hk.is_some_and(|h| h.shift),
            win: hk.is_some_and(|h| h.win),
            key: hk.map(|h| hotkey::key_name(h.vk)).unwrap_or_else(|| "Space".into()),
            recording: false,
        }
    }

    pub fn value(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

pub struct SettingsApp {
    pub saved: Config,
    pub draft: Config,
    pub page: Page,
    page_changed_at: f64,
    reset_scroll: bool,
    dirty_since: Option<f64>,
    pub devices: Vec<DeviceInfo>,
    pub mic_test: Option<MicTest>,
    pub shortcut: ShortcutEditor,
    pub shortcut_check: Option<(String, Result<(), String>)>,
    pub vocabulary_text: String,
    pub downloads: Vec<Download>,
    pub history: Vec<history::Entry>,
    pub stats: history::Stats,
    pub history_search: String,
    pub expanded_entry: Option<usize>,
    pub confirm_clear: bool,
    pub monitor: monitor::Monitor,
    pub logo: Option<TextureHandle>,
    logo_theme: Option<ThemeId>,
    hero: Option<(ThemeId, TextureHandle)>,
    toast: Option<(String, bool, f64)>,
    pub agy_path: Option<std::path::PathBuf>,
    pub agy_models: Arc<Mutex<Option<Vec<String>>>>,
    pub daemon_running: bool,
    daemon_checked_at: f64,
    pub now: f64,
    /// Files in the models folder (name, size), refreshed every few seconds rather than
    /// listing the folder on every frame.
    pub model_files: Vec<(String, u64)>,
    model_files_at: f64,
    /// Recording-indicator previews for the Appearance page, keyed by what they depend on.
    pub indicator_preview: Option<((ThemeId, IndicatorStyle, bool), Vec<TextureHandle>)>,
}

fn daemon_running() -> bool {
    unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()).is_ok() }
}

pub fn notify_daemon() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) else { return false };
        let name = WideStr::new(CONFIG_CHANGED_MESSAGE);
        let msg = RegisterWindowMessageW(name.pcwstr());
        PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)).is_ok()
    }
}

pub fn open_path(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(if path.extension().is_some() { path.parent().unwrap_or(path) } else { path });
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

pub fn start_daemon() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

fn parse_lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Hero banner background in the theme's colours. Rendered once, small, and stretched to the
/// banner (gradients scale cleanly), so resizing the window never re-renders it.
fn hero_image() -> egui::ColorImage {
    use tiny_skia::*;
    let (w, h) = (320u32, 96u32);
    let mut pm = Pixmap::new(w, h).unwrap();
    let (fw, fh) = (w as f32, h as f32);
    let pal = theme::palette();
    let mix = |c: (u8, u8, u8), t: f32| {
        let m = theme::lerp_color(BG, theme::rgb(c), t);
        Color::from_rgba8(m.r(), m.g(), m.b(), 255)
    };
    let rect = Rect::from_xywh(0.0, 0.0, fw, fh).unwrap();
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    if let Some(shader) = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(fw, fh),
        vec![GradientStop::new(0.0, mix(pal.accent, 0.24)), GradientStop::new(0.55, mix(pal.accent, 0.12)), GradientStop::new(1.0, mix(pal.accent_2, 0.13))],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        paint.shader = shader;
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
    let (r, g, b) = pal.accent;
    if let Some(shader) = RadialGradient::new(
        Point::from_xy(fw * 0.12, 0.0),
        0.0,
        Point::from_xy(fw * 0.12, 0.0),
        fh * 1.6,
        vec![GradientStop::new(0.0, Color::from_rgba8(r, g, b, 60)), GradientStop::new(1.0, Color::from_rgba8(r, g, b, 0))],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        paint.shader = shader;
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &audian_art::to_rgba(&pm))
}

/// The models folder's files (name, size).
fn list_model_files() -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = std::fs::read_dir(audian_common::paths::models_dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?.len())))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

impl SettingsApp {
    fn new(cfg: Config, page: Page) -> SettingsApp {
        SettingsApp {
            shortcut: ShortcutEditor::from(&cfg.hotkey.shortcut),
            vocabulary_text: cfg.transcription.vocabulary.join("\n"),
            saved: cfg.clone(),
            draft: cfg,
            page,
            page_changed_at: 0.0,
            reset_scroll: false,
            dirty_since: None,
            devices: audio::list_input_devices(),
            mic_test: None,
            shortcut_check: None,
            downloads: Vec::new(),
            history: history::load(),
            stats: history::load_stats(),
            history_search: String::new(),
            expanded_entry: None,
            confirm_clear: false,
            monitor: monitor::Monitor::new(),
            logo: None,
            logo_theme: None,
            hero: None,
            toast: None,
            agy_path: crate::text::antigravity_path(""),
            agy_models: Arc::new(Mutex::new(None)),
            daemon_running: daemon_running(),
            daemon_checked_at: 0.0,
            now: 0.0,
            model_files: list_model_files(),
            model_files_at: 0.0,
            indicator_preview: None,
        }
    }

    pub fn go(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.page_changed_at = self.now;
            self.reset_scroll = true;
            if page != Page::Audio && page != Page::Welcome {
                self.mic_test = None;
            }
            if page == Page::History || page == Page::Home {
                self.history = history::load();
                self.stats = history::load_stats();
            }
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some((text.into(), ok, self.now));
    }

    pub fn toggle_mic_test(&mut self) {
        self.mic_test = match self.mic_test.take() {
            Some(_) => None,
            None => Some(MicTest::start(self.draft.audio.clone())),
        };
    }

    pub fn download(&mut self, model: &'static catalog::ModelInfo) {
        if !self.downloads.iter().any(|d| d.file == model.file && d.is_active()) {
            self.downloads.retain(|d| d.file != model.file);
            self.downloads.push(download::start(model));
        }
    }

    pub fn download_for(&self, file: &str) -> Option<&Download> {
        self.downloads.iter().find(|d| d.file == file)
    }

    /// Paints the hero banner (gradient background and a decorative waveform starting at
    /// `wave_from`, a fraction of the width) into `rect`.
    pub fn paint_hero(&mut self, ui: &egui::Ui, rect: egui::Rect, wave_from: f32) {
        let theme = theme::current_theme();
        if self.hero.as_ref().map(|(k, _)| *k) != Some(theme) {
            self.hero = Some((theme, ui.ctx().load_texture("hero", hero_image(), egui::TextureOptions::LINEAR)));
        }
        let tex = &self.hero.as_ref().unwrap().1;
        egui::Image::new((tex.id(), rect.size())).corner_radius(18).paint_at(ui, rect);
        let p = ui.painter();
        let n = 46;
        let x0 = rect.left() + rect.width() * wave_from;
        let step = (rect.right() - x0 - 24.0) / n as f32;
        for i in 0..n {
            let t = i as f32 / n as f32;
            let v = (0.5 + 0.5 * (t * 17.0).sin() * (t * 5.3 + 1.0).cos()).abs() * (1.0 - (t * 2.0 - 1.0).powi(2) * 0.6);
            let bh = 8.0 + v * rect.height() * 0.62;
            let bar = egui::Rect::from_min_size(pos2(x0 + i as f32 * step, rect.center().y - bh / 2.0), vec2(step * 0.45, bh));
            p.rect_filled(bar, step * 0.22, Color32::from_white_alpha((18.0 + 30.0 * v) as u8));
        }
    }

    /// True if a catalog model's file is present with the expected size.
    pub fn model_installed(&self, m: &catalog::ModelInfo) -> bool {
        self.model_files.iter().any(|(f, size)| f.eq_ignore_ascii_case(m.file) && *size == m.size)
    }

    /// Size of a file in the models folder, if present.
    pub fn model_file_size(&self, file: &str) -> Option<u64> {
        self.model_files.iter().find(|(f, _)| f.eq_ignore_ascii_case(file)).map(|(_, s)| *s)
    }

    /// Re-reads the models folder on the next frame (after a delete, download, ...).
    pub fn refresh_model_files(&mut self) {
        self.model_files_at = f64::NEG_INFINITY;
    }

    /// Validates the shortcut being edited (parse + availability), caching the result.
    pub fn check_shortcut(&mut self) -> Result<(), String> {
        let current = self.shortcut.value();
        if let Some((s, r)) = &self.shortcut_check {
            if *s == current {
                return r.clone();
            }
        }
        let r = if current.eq_ignore_ascii_case(&self.saved.hotkey.shortcut) && self.daemon_running {
            Ok(())
        } else {
            Hotkey::parse(&current).map_err(|e| e.to_string()).and_then(|hk| hotkey::probe(&hk).map_err(|e| e.to_string()))
        };
        self.shortcut_check = Some((current, r.clone()));
        r
    }

    /// The configuration as it should be saved (editor state folded in).
    fn effective(&mut self) -> Config {
        let mut cfg = self.draft.clone();
        cfg.transcription.vocabulary = parse_lines(&self.vocabulary_text);
        // Only adopt a shortcut that parses and is available; otherwise keep the old one.
        if self.check_shortcut().is_ok() {
            cfg.hotkey.shortcut = self.shortcut.value();
        } else {
            cfg.hotkey.shortcut = self.saved.hotkey.shortcut.clone();
        }
        cfg
    }

    /// Auto-save: persists changes ~0.6 s after the last edit and notifies the daemon.
    fn autosave(&mut self) {
        let cfg = self.effective();
        if cfg == self.saved {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert(self.now);
        if self.now - since < 0.6 {
            return;
        }
        self.dirty_since = None;
        if let Err(e) = cfg.save() {
            self.toast(format!("Could not save: {e:#}"), false);
            return;
        }
        if cfg.general.start_with_windows != self.saved.general.start_with_windows {
            if let Err(e) = crate::autostart::set(cfg.general.start_with_windows) {
                self.toast(format!("Autostart: {e}"), false);
            }
        }
        if !cfg.privacy.history && self.saved.privacy.history {
            history::clear();
            self.history.clear();
        }
        self.saved = cfg;
        notify_daemon();
        self.toast("Saved", true);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(34.0, 34.0)));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("Audian").font(theme::semibold(19.0)).color(TEXT));
                ui.label(RichText::new(format!("Version {}", audian_common::APP_VERSION)).size(11.5).color(TEXT_FAINT));
            });
        });
        ui.add_space(18.0);

        // Item geometry is fixed, so the selection indicator can be drawn first and animated.
        let item_h = 34.0;
        let gap = 2.0;
        let group_gap = 27.0;
        let start = ui.cursor().min;
        let mut y = start.y;
        let mut target_y = None;
        let mut layout = Vec::new();
        for (gi, (group, items)) in NAV.iter().enumerate() {
            if gi > 0 {
                y += if group.is_empty() { 10.0 } else { group_gap };
            }
            for (page, icon, label) in items.iter() {
                if *page == self.page || (self.page == Page::Welcome && *page == Page::Home) {
                    target_y = Some(y);
                }
                layout.push((y, *page, *icon, *label, *group));
                y += item_h + gap;
            }
        }
        let width = ui.available_width();
        if let Some(ty) = target_y {
            let ay = ui.ctx().animate_value_with_time(egui::Id::new("nav-sel"), ty, 0.2);
            let r = egui::Rect::from_min_size(pos2(start.x, ay), vec2(width, item_h));
            ui.painter().rect_filled(r, 10.0, theme::tint(Color32::from_rgb(26, 27, 34), 0.1));
            ui.painter().rect_filled(egui::Rect::from_min_size(pos2(r.left(), r.top() + 9.0), vec2(3.0, item_h - 18.0)), 2.0, accent());
        }
        let mut last_group = "";
        for (iy, page, ic, label, group) in layout {
            if group != last_group && !group.is_empty() {
                ui.painter().text(pos2(start.x + 10.0, iy - 13.0), Align2::LEFT_CENTER, group, theme::semibold(10.5), TEXT_FAINT);
            }
            last_group = group;
            let rect = egui::Rect::from_min_size(pos2(start.x, iy), vec2(width, item_h));
            let resp = ui.interact(rect, egui::Id::new(("nav", label)), Sense::click());
            let selected = page == self.page || (self.page == Page::Welcome && page == Page::Home);
            let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered() && !selected, 0.12);
            if hover > 0.0 {
                ui.painter().rect_filled(rect, 10.0, Color32::from_rgb(30, 31, 40).linear_multiply(hover));
            }
            let color = if selected { TEXT } else { lerp_color(TEXT_DIM, TEXT, hover) };
            ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, ic, theme::icons(15.0), if selected { accent() } else { color });
            ui.painter().text(pos2(rect.left() + 44.0, rect.center().y), Align2::LEFT_CENTER, label, theme::body(14.0), color);
            if resp.clicked() {
                self.go(page);
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        ui.allocate_space(vec2(width, y - start.y));

        // Status at the bottom.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.daemon_running {
                    widgets::status_dot(ui, SUCCESS, true);
                    ui.label(RichText::new("Running in the tray").size(12.5).color(TEXT_DIM));
                } else {
                    widgets::status_dot(ui, WARN, false);
                    ui.label(RichText::new("Not running").size(12.5).color(TEXT_DIM));
                    if ui.link(RichText::new("Start").size(12.5)).clicked() {
                        start_daemon();
                    }
                }
            });
        });
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, ok, at)) = self.toast.clone() else { return };
        let age = (self.now - at) as f32;
        if age > 2.4 {
            self.toast = None;
            return;
        }
        let appear = ease_out(age / 0.18);
        let vanish = ((age - 2.0) / 0.4).clamp(0.0, 1.0);
        let alpha = appear * (1.0 - vanish);
        let screen = ctx.content_rect();
        let font = theme::semibold(13.0);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toast")));
        let galley = painter.layout_no_wrap(text.clone(), font, TEXT);
        let size = vec2(galley.size().x + 52.0, 38.0);
        let pos = pos2(screen.right() - size.x - 24.0, screen.bottom() - size.y - 24.0 + (1.0 - appear) * 12.0);
        let rect = egui::Rect::from_min_size(pos, size);
        painter.rect_filled(rect.translate(vec2(0.0, 4.0)), 12.0, Color32::from_black_alpha((50.0 * alpha) as u8));
        painter.rect_filled(rect, 12.0, Color32::from_rgb(36, 37, 50).linear_multiply(alpha));
        painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0, BORDER.linear_multiply(alpha)), egui::StrokeKind::Inside);
        let (glyph, color) = if ok { (icon::CHECK, SUCCESS) } else { (icon::WARNING, WARN) };
        painter.text(pos2(rect.left() + 18.0, rect.center().y), Align2::CENTER_CENTER, glyph, theme::icons(13.0), color.linear_multiply(alpha));
        painter.galley(pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, TEXT.linear_multiply(alpha));
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl eframe::App for SettingsApp {
    /// Opaque background colour, so areas not painted yet (e.g. while resizing) never flash
    /// darker than the window.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        // Theme changes apply live: egui's widget colours, the logo and cached artwork.
        if self.draft.appearance.theme != theme::current_theme() {
            theme::set_theme(self.draft.appearance.theme);
            theme::apply_visuals(&ctx);
        }
        let theme = theme::current_theme();
        if self.logo_theme != Some(theme) {
            let pal = theme.palette();
            let colors = audian_art::LogoColors { bg_top: pal.logo_bg.0, bg_bottom: pal.logo_bg.1, glow: pal.logo_glow, mark: pal.logo_mark };
            let pm = audian_art::draw_logo_with(96, &colors);
            let img = egui::ColorImage::from_rgba_unmultiplied([96, 96], &audian_art::to_rgba(&pm));
            self.logo = Some(ctx.load_texture("logo", img, egui::TextureOptions::LINEAR));
            self.logo_theme = Some(theme);
        }
        let downloading = self.downloads.iter().any(|d| d.is_active());
        if self.now - self.model_files_at > if downloading { 1.0 } else { 3.0 } {
            self.model_files = list_model_files();
            self.model_files_at = self.now;
        }
        if self.now - self.daemon_checked_at > 2.0 {
            self.daemon_checked_at = self.now;
            self.daemon_running = daemon_running();
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        if self.downloads.iter().any(|d| d.is_active()) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(246.0)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin { left: 14, right: 14, top: 18, bottom: 14 }))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin { left: 34, right: 30, top: 26, bottom: 10 }))
            .show(ui, |ui| {
                // Page transition: a short fade + slide. It starts from a partly visible page
                // rather than an empty one, which looked like a dark flash on slow frames.
                let t = ((self.now - self.page_changed_at) as f32 / 0.18).clamp(0.0, 1.0);
                if t < 1.0 {
                    ctx.request_repaint();
                }
                let e = ease_out(t);
                ui.multiply_opacity(0.45 + 0.55 * e);
                let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if std::mem::take(&mut self.reset_scroll) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    ui.add_space((1.0 - e) * 10.0);
                    ui.set_max_width(ui.available_width().min(880.0));
                    pages::show(self, ui);
                    ui.add_space(24.0);
                });
            });

        self.autosave();
        if self.dirty_since.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        self.draw_toast(&ctx);
    }
}

fn window_icon() -> egui::IconData {
    let pm = audian_art::draw_logo(64);
    egui::IconData { rgba: audian_art::to_rgba(&pm), width: 64, height: 64 }
}

pub fn run(page: Option<String>) -> anyhow::Result<()> {
    unsafe {
        let name = WideStr::new("Local\\Audian.Settings");
        let mutex = CreateMutexW(None, false, name.pcwstr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = FindWindowW(PCWSTR::null(), &HSTRING::from(TITLE)) {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return Ok(());
        }
        // Keep the mutex for the life of the process.
        std::mem::forget(mutex);
    }
    let cfg = Config::load();
    let page = page.as_deref().and_then(Page::from_arg).unwrap_or(Page::Home);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1080.0, 740.0])
            .with_min_inner_size([900.0, 660.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            theme::set_theme(cfg.appearance.theme);
            theme::install(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new(cfg, page)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("could not open the Audian window: {e}"))
}
