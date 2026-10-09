//! The background daemon: owns the hidden message window, tray icon, global shortcuts,
//! overlay and the dictation state machine. Everything here runs on the UI thread; heavy
//! work (model warm-up, previews, transcription, rewriting) runs on worker threads that
//! report back through a channel plus a wake-up message.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audian_common::config::{Config, ProcessingMode, RecordingMode, RewriteProvider, SttProvider};
use audian_common::{history, paths};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::audio::{self, AudioError, Recorder};
use crate::autostart;
use crate::endpoint::{self, Decision, Preview};
use crate::hotkey::{self, Hotkey};
use crate::inject::{InsertOutcome, Injector, Target};
use crate::overlay::{Overlay, Phase as OverlayPhase};
use crate::pipeline::{self, Engines, Job, Outcome, PipelineError, Precomputed, Stage};
use crate::sound::{self, Cue};
use crate::tray::{self, MenuItem, Tray, TrayState, WM_TRAY};
use crate::win::{SendHwnd, WideStr, loword};

pub const WINDOW_CLASS: PCWSTR = w!("AudianDaemon");
pub const CONFIG_CHANGED_MESSAGE: &str = "Audian.ConfigChanged";
pub const WM_APP_EVENT: u32 = WM_APP + 2;
pub const WM_APP_OPEN_SETTINGS: u32 = WM_APP + 3;

const HOTKEY_DICTATE: i32 = 1;
const HOTKEY_CANCEL: i32 = 2;
const HOTKEY_PASTE_LAST: i32 = 3;
const TIMER_KEY_POLL: usize = 1;
const TIMER_MAX_DURATION: usize = 2;
const TIMER_OVERLAY: usize = 3;
const TIMER_CLIPBOARD: usize = 4;
const TIMER_ENDPOINT: usize = 5;
/// In hybrid mode, a press shorter than this switches to hands-free (toggle) recording.
const TAP_THRESHOLD: Duration = Duration::from_millis(350);
const MIN_RECORDING: Duration = Duration::from_millis(300);
/// Less detected speech than this is treated as "nothing said".
const MIN_SPEECH_MS: u32 = 150;

const CMD_TOGGLE: u32 = 1;
const CMD_SETTINGS: u32 = 2;
const CMD_LOGS: u32 = 3;
const CMD_RELOAD: u32 = 4;
const CMD_EXIT: u32 = 5;
const CMD_REWRITE_ENABLED: u32 = 6;
const CMD_OPEN_CONFIG: u32 = 7;
const CMD_COPY_LAST: u32 = 8;
const CMD_HISTORY: u32 = 9;
const CMD_AUTO_STOP: u32 = 10;
const CMD_PROFILES: u32 = 11;
const CMD_MODE_BASE: u32 = 100;
const CMD_PROVIDER_BASE: u32 = 200;
const CMD_MIC_DEFAULT: u32 = 300;
const CMD_MIC_BASE: u32 = 301;
const CMD_OUTPUT_BASE: u32 = 400;

/// Output languages offered in the tray (Settings offers more).
pub const OUTPUT_LANGUAGES: &[(&str, &str)] = &[
    ("same", "Same as spoken"),
    ("en", "English"),
    ("pt", "Portuguese"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
];

enum AppEvent {
    PipelineDone { session: u64, result: Result<Outcome, PipelineError> },
    Stage { session: u64, stage: Stage },
    Preview { session: u64, at_voice_ms: u32, result: Option<Precomputed> },
    AudioFailed { session: u64, message: String },
}

/// Lets worker threads deliver events to the UI thread.
#[derive(Clone)]
struct EventSink {
    tx: Sender<AppEvent>,
    hwnd: SendHwnd,
}

impl EventSink {
    fn send(&self, event: AppEvent) {
        if self.tx.send(event).is_ok() {
            self.hwnd.post(WM_APP_EVENT, 0, 0);
        }
    }
}

struct RecordingSession {
    session: u64,
    recorder: Recorder,
    target: Option<Target>,
    pressed_at: Instant,
    /// Hands-free: the shortcut isn't held; stops on the next press or on a pause.
    hands_free: bool,
    /// Configuration for this dictation (processing mode chosen by app profile).
    cfg: Config,
    preview: Option<Preview>,
    /// Rewrite of the preview taken when the voice ended at this `last_voice_ms`.
    ahead: Option<(u32, pipeline::RewriteSlot)>,
}

enum Phase {
    Idle,
    Recording(Box<RecordingSession>),
    Processing { session: u64, target: Option<Target>, app: String, mode: ProcessingMode },
}

pub struct App {
    hwnd: HWND,
    cfg: Config,
    hotkey: Option<Hotkey>,
    hotkey_error: Option<String>,
    paste_hotkey: Option<Hotkey>,
    phase: Phase,
    next_session: u64,
    overlay: Overlay,
    tray: Tray,
    injector: Injector,
    engines: Arc<Mutex<Engines>>,
    events: Receiver<AppEvent>,
    sink: EventSink,
    msg_taskbar_created: u32,
    msg_config_changed: u32,
    menu_devices: Vec<audio::DeviceInfo>,
    last_text: Option<String>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

enum Handled {
    Result(LRESULT),
    Default,
    Menu,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => match guard.as_mut() {
            Some(app) => app.handle(msg, wparam, lparam),
            None => Handled::Default,
        },
        // Re-entrant call (a modal loop or a synchronous clipboard notification while the app
        // is busy). Defer wake-ups so no event is lost.
        Err(_) => {
            if msg == WM_APP_EVENT {
                SendHwnd::new(hwnd).post(WM_APP_EVENT, 0, 0);
            }
            Handled::Default
        }
    });
    match handled {
        Handled::Result(r) => r,
        Handled::Default => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        Handled::Menu => {
            // Build the menu, then release the borrow: TrackPopupMenu runs a modal loop that
            // dispatches messages back into this window procedure.
            let items = APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(|a| a.menu_items())));
            if let Some(items) = items {
                if let Some(cmd) = tray::show_menu(hwnd, &items) {
                    APP.with(|cell| {
                        if let Ok(mut guard) = cell.try_borrow_mut() {
                            if let Some(app) = guard.as_mut() {
                                app.on_menu(cmd);
                            }
                        }
                    });
                }
            }
            LRESULT(0)
        }
    }
}

/// Runs the daemon until the user exits. Returns the process exit code.
pub fn run() -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Single instance: a second launch asks the running daemon to open its window.
        let mutex_name = WideStr::new("Local\\Audian.Daemon");
        let _mutex = CreateMutexW(None, false, mutex_name.pcwstr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) {
                let _ = PostMessageW(Some(existing), WM_APP_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
            }
            log::info!("already running; asked the existing instance to open its window");
            return 0;
        }

        let first_run = !paths::config_file().exists();
        let cfg = Config::load();
        tray::enable_dark_menus();

        let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Audian"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log::error!("cannot create main window: {e}");
                return 1;
            }
        };

        let app = match App::new(hwnd, instance, cfg) {
            Ok(a) => a,
            Err(e) => {
                log::error!("startup failed: {e:#}");
                return 1;
            }
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        APP.with(|cell| {
            if let Some(app) = cell.borrow_mut().as_mut() {
                app.start(first_run);
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Drop the app here (tray icon removed, helper processes terminated).
        let app = APP.with(|cell| cell.borrow_mut().take());
        drop(app);
        log::info!("exited");
        0
    }
}

fn models_missing(cfg: &Config) -> bool {
    let stt_missing = match cfg.transcription.provider {
        SttProvider::WhisperLocal => !paths::resolve_model(&cfg.transcription.whisper.model).is_file(),
        SttProvider::Parakeet => !paths::resolve_model(&cfg.transcription.parakeet.model).is_dir(),
    };
    let llm = paths::resolve_model(&cfg.processing.local_llm.model);
    stt_missing || (cfg.processing.enabled && cfg.processing.provider == RewriteProvider::LocalLlm && !llm.is_file())
}

impl App {
    fn new(hwnd: HWND, instance: HINSTANCE, cfg: Config) -> anyhow::Result<App> {
        let (tx, rx) = mpsc::channel();
        let mut overlay = Overlay::create(instance)?;
        overlay.set_appearance(cfg.appearance.theme, cfg.overlay.style);
        let engines = Arc::new(Mutex::new(Engines::new(&cfg)));
        let taskbar = WideStr::new("TaskbarCreated");
        let changed = WideStr::new(CONFIG_CHANGED_MESSAGE);
        Ok(App {
            hwnd,
            hotkey: None,
            hotkey_error: None,
            paste_hotkey: None,
            phase: Phase::Idle,
            next_session: 1,
            overlay,
            tray: Tray::new(hwnd),
            injector: Injector::new(hwnd),
            engines,
            events: rx,
            sink: EventSink { tx, hwnd: SendHwnd::new(hwnd) },
            msg_taskbar_created: unsafe { RegisterWindowMessageW(taskbar.pcwstr()) },
            msg_config_changed: unsafe { RegisterWindowMessageW(changed.pcwstr()) },
            menu_devices: Vec::new(),
            last_text: history::load().into_iter().next().map(|e| e.text),
            cfg,
        })
    }

    fn start(&mut self, first_run: bool) {
        self.tray.add();
        self.register_hotkeys();
        autostart::sync(self.cfg.general.start_with_windows);
        self.refresh_tray();
        log::info!("{} {} started", audian_common::APP_NAME, audian_common::APP_VERSION);
        crate::text::local_llm::prepare_gpu_in_background(&self.cfg);
        if first_run || !self.cfg.general.welcome_done || models_missing(&self.cfg) {
            self.open_settings(Some("welcome"));
        }
    }

    // ------------------------------------------------------------------ message dispatch

    fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Handled {
        match msg {
            WM_HOTKEY => {
                match wparam.0 as i32 {
                    HOTKEY_DICTATE => self.on_dictate_hotkey(),
                    HOTKEY_CANCEL => self.cancel("cancelled"),
                    HOTKEY_PASTE_LAST => self.paste_last(),
                    _ => {}
                }
                Handled::Result(LRESULT(0))
            }
            WM_TIMER => {
                match wparam.0 {
                    TIMER_KEY_POLL => self.on_key_poll(),
                    TIMER_ENDPOINT => self.on_endpoint_tick(),
                    TIMER_MAX_DURATION => {
                        self.notify("Recording limit reached", "Dictation stopped at the maximum duration set in Settings.", false);
                        self.stop_recording();
                    }
                    TIMER_OVERLAY => {
                        if !self.overlay.tick() {
                            self.kill_timer(TIMER_OVERLAY);
                            if matches!(self.phase, Phase::Idle) {
                                // Back to idle: hand temporary memory (audio buffers, glyphs)
                                // back to Windows so the background footprint stays minimal.
                                unsafe {
                                    let _ = windows::Win32::System::ProcessStatus::K32EmptyWorkingSet(
                                        windows::Win32::System::Threading::GetCurrentProcess(),
                                    );
                                }
                            }
                        }
                    }
                    TIMER_CLIPBOARD => {
                        self.kill_timer(TIMER_CLIPBOARD);
                        self.injector.restore_clipboard();
                    }
                    _ => {}
                }
                Handled::Result(LRESULT(0))
            }
            WM_APP_EVENT => {
                while let Ok(event) = self.events.try_recv() {
                    self.on_event(event);
                }
                Handled::Result(LRESULT(0))
            }
            WM_TRAY => {
                // NOTIFYICON_VERSION_4: the event is in the low word of lParam.
                const NIN_SELECT: u32 = WM_USER;
                const NIN_KEYSELECT: u32 = WM_USER + 1;
                match loword(lparam.0 as usize) {
                    WM_CONTEXTMENU => Handled::Menu,
                    NIN_SELECT | NIN_KEYSELECT => {
                        self.open_settings(None);
                        Handled::Result(LRESULT(0))
                    }
                    _ => Handled::Result(LRESULT(0)),
                }
            }
            WM_APP_OPEN_SETTINGS => {
                self.open_settings(None);
                Handled::Result(LRESULT(0))
            }
            WM_ENDSESSION | WM_CLOSE => {
                unsafe { PostQuitMessage(0) };
                Handled::Result(LRESULT(0))
            }
            m if m == self.msg_taskbar_created && m != 0 => {
                self.tray.add();
                self.refresh_tray();
                Handled::Result(LRESULT(0))
            }
            m if m == self.msg_config_changed && m != 0 => {
                self.reload_config();
                Handled::Result(LRESULT(0))
            }
            _ => Handled::Default,
        }
    }

    // ------------------------------------------------------------------ hotkeys

    fn register_hotkeys(&mut self) {
        if self.hotkey.is_some() {
            hotkey::unregister(self.hwnd, HOTKEY_DICTATE);
            self.hotkey = None;
        }
        self.hotkey_error = None;
        match Hotkey::parse(&self.cfg.hotkey.shortcut).and_then(|hk| hotkey::register(self.hwnd, HOTKEY_DICTATE, &hk).map(|_| hk)) {
            Ok(hk) => {
                log::info!("shortcut registered: {hk}");
                self.hotkey = Some(hk);
            }
            Err(e) => {
                log::error!("shortcut \"{}\" unavailable: {e}", self.cfg.hotkey.shortcut);
                let msg = format!("Shortcut unavailable: {e}. Choose another one in Settings.");
                self.notify("Shortcut conflict", &msg, true);
                self.hotkey_error = Some(msg);
            }
        }

        if self.paste_hotkey.take().is_some() {
            hotkey::unregister(self.hwnd, HOTKEY_PASTE_LAST);
        }
        let paste = self.cfg.hotkey.paste_last_shortcut.trim().to_string();
        if !paste.is_empty() {
            match Hotkey::parse(&paste).and_then(|hk| hotkey::register(self.hwnd, HOTKEY_PASTE_LAST, &hk).map(|_| hk)) {
                Ok(hk) => self.paste_hotkey = Some(hk),
                Err(e) => log::warn!("paste-last shortcut \"{paste}\" unavailable: {e}"),
            }
        }
    }

    fn register_cancel_key(&self) {
        if self.cfg.hotkey.escape_cancels {
            unsafe {
                let _ = RegisterHotKey(Some(self.hwnd), HOTKEY_CANCEL, MOD_NOREPEAT | HOT_KEY_MODIFIERS(0), VK_ESCAPE.0 as u32);
            }
        }
    }

    fn unregister_cancel_key(&self) {
        hotkey::unregister(self.hwnd, HOTKEY_CANCEL);
    }

    fn on_dictate_hotkey(&mut self) {
        match &self.phase {
            Phase::Idle => self.start_recording(),
            Phase::Recording(r) => {
                if r.hands_free {
                    self.stop_recording();
                }
            }
            Phase::Processing { .. } => log::info!("shortcut pressed while processing; ignored"),
        }
    }

    fn on_key_poll(&mut self) {
        let Some(hk) = self.hotkey else { return };
        let mode = self.cfg.hotkey.mode;
        let mut stop = false;
        let mut went_hands_free = false;
        if let Phase::Recording(r) = &mut self.phase {
            if !r.hands_free && !hk.key_is_down() {
                if mode == RecordingMode::Hybrid && r.pressed_at.elapsed() < TAP_THRESHOLD {
                    r.hands_free = true; // quick tap: hands-free until the shortcut or a pause
                    went_hands_free = true;
                } else {
                    stop = true;
                }
            }
        }
        if stop {
            self.stop_recording();
        } else if went_hands_free {
            log::info!("hands-free recording");
            self.kill_timer(TIMER_KEY_POLL);
            self.enter_hands_free();
        }
    }

    fn enter_hands_free(&mut self) {
        self.overlay.set_hands_free(true);
        if self.cfg.auto_stop.enabled {
            self.set_timer(TIMER_ENDPOINT, 100);
        }
        self.refresh_tray();
    }

    // ------------------------------------------------------------------ dictation flow

    fn start_recording(&mut self) {
        // Finish any clipboard restore from the previous dictation first.
        self.kill_timer(TIMER_CLIPBOARD);
        self.injector.restore_clipboard();

        let target = Target::capture();
        let (mode, profile) = match &target {
            Some(t) => self.cfg.mode_for(&t.process_name, &t.title),
            None => (self.cfg.processing.mode, None),
        };
        if let Some(p) = &profile {
            log::info!("app profile \"{p}\" -> {} mode", mode.label());
        }
        let mut session_cfg = self.cfg.clone();
        session_cfg.processing.mode = mode;

        let session = self.next_session;
        self.next_session += 1;
        let sink = self.sink.clone();
        let recorder = match Recorder::start(&self.cfg.audio, move |message| sink.send(AppEvent::AudioFailed { session, message })) {
            Ok(r) => r,
            Err(e) => {
                log::error!("microphone: {e}");
                self.show_error("Microphone problem", &e.to_string(), "Microphone unavailable");
                return;
            }
        };
        if let Some(hk) = &self.hotkey {
            hotkey::suppress_modifier_side_effects(hk);
        }
        self.cue(Cue::Start);
        let label = if self.cfg.processing.enabled { mode.short_label() } else { "Raw" };
        self.overlay.show_recording(recorder.meter(), target.as_ref(), self.cfg.overlay.position, label, self.cfg.overlay.show_label);
        self.set_timer(TIMER_OVERLAY, Overlay::frame_interval().as_millis() as u32);
        self.register_cancel_key();
        self.set_timer(TIMER_MAX_DURATION, self.cfg.audio.max_duration_secs.clamp(5, 3600) * 1000);

        // Load models / start helpers while the user speaks.
        let engines = self.engines.clone();
        let warm_cfg = session_cfg.clone();
        std::thread::spawn(move || {
            let mut engines = engines.lock().unwrap_or_else(|e| e.into_inner());
            engines.warm_up(&warm_cfg, session);
        });

        let hands_free = self.cfg.hotkey.mode == RecordingMode::Toggle;
        self.phase = Phase::Recording(Box::new(RecordingSession {
            session,
            recorder,
            target,
            pressed_at: Instant::now(),
            hands_free,
            cfg: session_cfg,
            preview: None,
            ahead: None,
        }));
        if hands_free {
            self.enter_hands_free();
        } else {
            self.set_timer(TIMER_KEY_POLL, 15);
        }
        self.refresh_tray();
    }

    /// Hands-free end-of-speech detection (see `endpoint`).
    fn on_endpoint_tick(&mut self) {
        let Phase::Recording(r) = &mut self.phase else {
            self.kill_timer(TIMER_ENDPOINT);
            return;
        };
        let status = r.recorder.vad_status();
        let outcome = endpoint::decide(&status, &self.cfg.auto_stop, r.preview.as_ref());
        self.overlay.set_endpoint_progress(outcome.progress);
        match outcome.decision {
            Decision::Continue => {}
            Decision::RequestPreview => {
                r.preview = Some(Preview { at_voice_ms: status.last_voice_ms, text: None, language: String::new() });
                let slot = pipeline::RewriteSlot::default();
                r.ahead = Some((status.last_voice_ms, slot.clone()));
                let samples = r.recorder.snapshot();
                let rate = r.recorder.sample_rate();
                let cfg = r.cfg.clone();
                let (session, at_voice_ms) = (r.session, status.last_voice_ms);
                let engines = self.engines.clone();
                let sink = self.sink.clone();
                std::thread::spawn(move || {
                    let mut sent = false;
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let mut engines = engines.lock().unwrap_or_else(|e| e.into_inner());
                        let result = pipeline::preview(&mut engines, &samples, rate, &cfg);
                        // The pause decision needs the transcript now; the rewrite takes longer.
                        sink.send(AppEvent::Preview { session, at_voice_ms, result: result.clone() });
                        sent = true;
                        if let Some(preview) = result {
                            pipeline::rewrite_ahead(&mut engines, &preview, &cfg, &slot);
                        }
                    }));
                    if !sent {
                        sink.send(AppEvent::Preview { session, at_voice_ms, result: None });
                    }
                });
            }
            Decision::Stop => {
                log::info!("auto-stop after {} ms of silence", status.silence_ms());
                self.stop_recording();
            }
            Decision::CancelNoSpeech => {
                log::info!("hands-free: nothing said, cancelling");
                self.cancel("cancelled (no speech)");
                self.show_overlay_phase(OverlayPhase::NoSpeech, "No speech");
            }
        }
    }

    fn stop_recording(&mut self) {
        self.kill_timer(TIMER_KEY_POLL);
        self.kill_timer(TIMER_ENDPOINT);
        self.kill_timer(TIMER_MAX_DURATION);
        let Phase::Recording(r) = std::mem::replace(&mut self.phase, Phase::Idle) else {
            return;
        };
        let RecordingSession { session, recorder, target, cfg, preview, ahead, .. } = *r;
        let device = recorder.device_name.clone();
        let recording = recorder.stop();
        log::info!(
            "recorded {:.1}s from {device} (peak {:.3}, speech {} ms)",
            recording.duration().as_secs_f32(),
            recording.peak_rms,
            recording.vad.speech_ms
        );

        if recording.duration() < MIN_RECORDING {
            self.finish_cancelled();
            return;
        }
        if recording.peak_rms < self.cfg.audio.silence_threshold || recording.vad.speech_ms < MIN_SPEECH_MS {
            log::info!("recording contained no speech");
            self.finish_cancelled();
            self.show_overlay_phase(OverlayPhase::NoSpeech, "No speech");
            return;
        }
        self.cue(Cue::Stop);

        // Reuse the transcript computed during the final pause if nothing was said since.
        let last_voice_ms = recording.vad.last_voice_ms;
        let rewritten = ahead.filter(|(at, _)| *at == last_voice_ms).map(|(_, slot)| slot);
        let precomputed = preview
            .filter(|p| p.at_voice_ms == last_voice_ms)
            .and_then(|p| p.text.filter(|t| !t.trim().is_empty()).map(|text| Precomputed { text, language: p.language, rewritten }));
        let mode = cfg.processing.mode;
        let ai = cfg.processing.enabled && mode != ProcessingMode::Literal;
        let first_label = if precomputed.is_some() && ai { "Polishing" } else { "Transcribing" };
        self.overlay.set_phase(OverlayPhase::Processing, first_label);
        self.set_timer(TIMER_OVERLAY, Overlay::frame_interval().as_millis() as u32);

        let app = target.as_ref().map(|t| t.app_name()).unwrap_or_default();
        let job = Job { recording, cfg, target_is_terminal: target.as_ref().is_some_and(|t| t.is_terminal), precomputed };
        let engines = self.engines.clone();
        let sink = self.sink.clone();
        std::thread::spawn(move || {
            let stage_sink = sink.clone();
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut engines = engines.lock().unwrap_or_else(|e| e.into_inner());
                pipeline::run(&mut engines, job, &|stage| stage_sink.send(AppEvent::Stage { session, stage }))
            }))
            .unwrap_or_else(|_| Err(PipelineError::Rewrite(crate::text::ProcessError::Engine("internal error".into()))));
            sink.send(AppEvent::PipelineDone { session, result });
        });
        self.phase = Phase::Processing { session, target, app, mode };
        self.refresh_tray();
    }

    fn cancel(&mut self, why: &str) {
        log::info!("dictation {why}");
        self.kill_timer(TIMER_KEY_POLL);
        self.kill_timer(TIMER_ENDPOINT);
        self.kill_timer(TIMER_MAX_DURATION);
        if let Phase::Recording(r) = std::mem::replace(&mut self.phase, Phase::Idle) {
            drop(r.recorder.stop());
        }
        self.finish_cancelled();
        self.overlay.hide();
    }

    /// Returns to idle after a dictation that produced nothing to insert.
    fn finish_cancelled(&mut self) {
        self.phase = Phase::Idle;
        self.unregister_cancel_key();
        // Stop anything the warm-up started and, with "unload after use", free the engines
        // (waiting for a warm-up still in progress).
        let engines = self.engines.clone();
        let cfg = self.cfg.clone();
        let session = self.next_session - 1;
        std::thread::spawn(move || {
            engines.lock().unwrap_or_else(|e| e.into_inner()).abandon(&cfg, session);
        });
        self.overlay.hide();
        self.refresh_tray();
    }

    fn on_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::AudioFailed { session, message } => {
                if matches!(&self.phase, Phase::Recording(r) if r.session == session) {
                    log::error!("audio stream failed: {message}");
                    self.cancel("aborted: microphone failure");
                    self.show_error(
                        "Microphone problem",
                        &format!("{} The recording was stopped.", AudioError::Other(message)),
                        "Microphone lost",
                    );
                }
            }
            AppEvent::Preview { session, at_voice_ms, result } => {
                if let Phase::Recording(r) = &mut self.phase {
                    if r.session == session {
                        if let Some(p) = r.preview.as_mut().filter(|p| p.at_voice_ms == at_voice_ms) {
                            match result {
                                Some(pc) => {
                                    log::debug!("preview ready ({} chars, unfinished: {})", pc.text.len(), endpoint::sounds_unfinished(&pc.text));
                                    p.text = Some(pc.text);
                                    p.language = pc.language;
                                }
                                None => p.text = Some(String::new()),
                            }
                        }
                    }
                }
            }
            AppEvent::Stage { session, stage } => {
                if matches!(&self.phase, Phase::Processing { session: s, .. } if *s == session) {
                    self.overlay.set_label(match stage {
                        Stage::Transcribing => "Transcribing",
                        Stage::Rewriting => "Polishing",
                    });
                }
            }
            AppEvent::PipelineDone { session, result } => {
                let (target, app, mode) = match std::mem::replace(&mut self.phase, Phase::Idle) {
                    Phase::Processing { session: s, target, app, mode } if s == session => (target, app, mode),
                    other => {
                        self.phase = other; // stale result of a cancelled dictation
                        return;
                    }
                };
                self.unregister_cancel_key();
                match result {
                    Ok(outcome) => self.insert(outcome, target, app, mode),
                    Err(PipelineError::NoSpeech) => self.show_overlay_phase(OverlayPhase::NoSpeech, "No speech"),
                    Err(e) => {
                        log::error!("pipeline failed: {e}");
                        self.show_error("Dictation failed", &e.to_string(), "Something went wrong");
                    }
                }
                self.refresh_tray();
            }
        }
    }

    fn insert(&mut self, outcome: Outcome, target: Option<Target>, app: String, mode: ProcessingMode) {
        if outcome.text.trim().is_empty() {
            self.show_overlay_phase(OverlayPhase::NoSpeech, "No speech");
            return;
        }
        let target = target.or_else(Target::capture);
        let result = match &target {
            Some(t) => self.injector.insert(&outcome.text, t, &self.cfg.insertion),
            None => Ok(InsertOutcome::CopiedOnly("No window was focused; the text was copied to the clipboard.".into())),
        };
        match result {
            Ok(InsertOutcome::Inserted) => {
                let words = history::word_count(&outcome.text);
                self.show_overlay_phase(OverlayPhase::Done, &format!("{words} word{}", if words == 1 { "" } else { "s" }));
                if self.injector.has_pending_restore() {
                    self.set_timer(TIMER_CLIPBOARD, self.cfg.insertion.restore_delay_ms.clamp(100, 5000));
                }
                if let Some(notice) = &outcome.notice {
                    self.notify("Used basic cleanup", notice, true);
                }
            }
            Ok(InsertOutcome::CopiedOnly(reason)) => {
                log::warn!("{reason}");
                self.show_error("Text copied to clipboard", &reason, "Copied to clipboard");
            }
            Err(e) => {
                log::error!("{e}");
                self.show_error("Could not insert text", &e.to_string(), "Insert failed");
            }
        }
        self.remember(&outcome, &app, mode);
    }

    /// History, statistics and "paste last".
    fn remember(&mut self, outcome: &Outcome, app: &str, mode: ProcessingMode) {
        self.last_text = Some(outcome.text.trim_end().to_string());
        history::record_stats(history::word_count(&outcome.text), outcome.audio_ms);
        if self.cfg.privacy.history {
            history::append(
                &history::Entry {
                    time: history::now_local(),
                    app: app.to_string(),
                    mode: mode.label().to_string(),
                    provider: outcome.provider.clone(),
                    language: outcome.language.clone(),
                    raw: outcome.raw.clone(),
                    text: outcome.text.trim_end().to_string(),
                    audio_ms: outcome.audio_ms,
                    process_ms: outcome.process_ms,
                },
                self.cfg.privacy.history_limit.max(1),
            );
        }
    }

    fn paste_last(&mut self) {
        if !matches!(self.phase, Phase::Idle) {
            return;
        }
        let Some(text) = self.last_text.clone() else {
            self.show_overlay_phase(OverlayPhase::NoSpeech, "Nothing to paste");
            return;
        };
        if let Some(target) = Target::capture() {
            match self.injector.insert(&text, &target, &self.cfg.insertion) {
                Ok(InsertOutcome::Inserted) => {
                    if self.injector.has_pending_restore() {
                        self.set_timer(TIMER_CLIPBOARD, self.cfg.insertion.restore_delay_ms.clamp(100, 5000));
                    }
                }
                Ok(InsertOutcome::CopiedOnly(reason)) => self.notify("Text copied to clipboard", &reason, false),
                Err(e) => self.notify("Could not paste", &e.to_string(), true),
            }
        }
    }

    // ------------------------------------------------------------------ UI helpers

    fn cue(&self, cue: Cue) {
        if self.cfg.general.sounds {
            sound::play(cue, self.cfg.general.sound_volume);
        }
    }

    fn show_overlay_phase(&mut self, phase: OverlayPhase, label: &str) {
        if !self.overlay.is_visible() {
            // Make sure the pill is positioned before showing a transient state.
            self.overlay.place_default();
        }
        self.overlay.set_phase(phase, label);
        self.set_timer(TIMER_OVERLAY, Overlay::frame_interval().as_millis() as u32);
    }

    fn show_error(&mut self, title: &str, body: &str, short: &str) {
        self.cue(Cue::Error);
        self.show_overlay_phase(OverlayPhase::Error, short);
        self.notify(title, body, true);
    }

    fn notify(&self, title: &str, body: &str, error: bool) {
        if self.cfg.general.notifications {
            self.tray.notify(title, body, error);
        }
    }

    fn set_timer(&self, id: usize, ms: u32) {
        unsafe {
            SetTimer(Some(self.hwnd), id, ms, None);
        }
    }

    fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    fn status(&self) -> String {
        match (&self.phase, &self.hotkey) {
            (Phase::Recording(r), Some(hk)) if r.hands_free => {
                if self.cfg.auto_stop.enabled {
                    format!("Listening hands-free — pause or press {hk} to finish")
                } else {
                    format!("Listening hands-free — press {hk} to finish")
                }
            }
            (Phase::Recording(_), _) => "Listening…".into(),
            (Phase::Processing { .. }, _) => "Processing…".into(),
            (Phase::Idle, Some(hk)) => format!("Ready — {hk}"),
            (Phase::Idle, None) => self.hotkey_error.clone().unwrap_or_else(|| "No shortcut set".into()),
        }
    }

    fn refresh_tray(&mut self) {
        let state = match self.phase {
            Phase::Idle => TrayState::Idle,
            Phase::Recording(_) => TrayState::Recording,
            Phase::Processing { .. } => TrayState::Processing,
        };
        let tip = format!("Audian — {}", self.status());
        self.tray.set_state(state, &tip);
    }

    fn open_settings(&self, page: Option<&str>) {
        match std::env::current_exe() {
            Ok(exe) => {
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("--settings");
                if let Some(p) = page {
                    cmd.arg(p);
                }
                if let Err(e) = cmd.spawn() {
                    log::error!("cannot open settings: {e}");
                }
            }
            Err(e) => log::error!("cannot locate executable: {e}"),
        }
    }

    fn reload_config(&mut self) {
        let new = Config::load();
        log::info!("configuration reloaded");
        self.apply_config(new);
    }

    fn apply_config(&mut self, new: Config) {
        let hotkeys_changed = new.hotkey.shortcut != self.cfg.hotkey.shortcut
            || new.hotkey.paste_last_shortcut != self.cfg.hotkey.paste_last_shortcut;
        let autostart_changed = new.general.start_with_windows != self.cfg.general.start_with_windows;
        if !new.privacy.history && self.cfg.privacy.history {
            self.last_text = None;
        }
        if new.processing.local_llm != self.cfg.processing.local_llm || new.processing.provider != self.cfg.processing.provider {
            crate::text::local_llm::prepare_gpu_in_background(&new);
        }
        self.cfg = new;
        self.overlay.set_appearance(self.cfg.appearance.theme, self.cfg.overlay.style);
        if hotkeys_changed || self.hotkey.is_none() {
            self.register_hotkeys();
        }
        if autostart_changed {
            autostart::sync(self.cfg.general.start_with_windows);
        }
        self.refresh_tray();
    }

    fn save_and_apply(&mut self, cfg: Config) {
        if let Err(e) = cfg.save() {
            log::error!("saving config: {e:#}");
            self.notify("Could not save settings", &format!("{e:#}"), true);
        }
        self.apply_config(cfg);
    }

    // ------------------------------------------------------------------ tray menu

    fn menu_items(&mut self) -> Vec<MenuItem> {
        let cfg = &self.cfg;
        let toggle_label = match self.phase {
            Phase::Recording(_) => "Stop dictation",
            _ => "Start dictation",
        };

        let modes = ProcessingMode::ALL
            .iter()
            .enumerate()
            .map(|(i, m)| MenuItem::Radio { id: CMD_MODE_BASE + i as u32, label: m.label().into(), selected: cfg.processing.mode == *m })
            .collect();

        let mut rewriting = vec![
            MenuItem::Action { id: CMD_REWRITE_ENABLED, label: "Rewrite with AI".into(), checked: cfg.processing.enabled, enabled: true },
            MenuItem::Separator,
        ];
        rewriting.extend(RewriteProvider::ALL.iter().enumerate().map(|(i, p)| MenuItem::Radio {
            id: CMD_PROVIDER_BASE + i as u32,
            label: p.label().into(),
            selected: cfg.processing.provider == *p,
        }));

        let outputs = OUTPUT_LANGUAGES
            .iter()
            .enumerate()
            .map(|(i, (code, name))| MenuItem::Radio {
                id: CMD_OUTPUT_BASE + i as u32,
                label: name.to_string(),
                selected: cfg.processing.output_language == *code,
            })
            .collect();

        self.menu_devices = audio::list_input_devices();
        let mut mics = vec![MenuItem::Radio { id: CMD_MIC_DEFAULT, label: "System default".into(), selected: cfg.audio.device_id.is_empty() }];
        mics.extend(self.menu_devices.iter().enumerate().map(|(i, d)| MenuItem::Radio {
            id: CMD_MIC_BASE + i as u32,
            label: d.name.clone(),
            selected: !cfg.audio.device_id.is_empty() && cfg.audio.device_id == d.id,
        }));

        let paste_hint = self.paste_hotkey.map(|h| format!("\t{h}")).unwrap_or_default();
        vec![
            MenuItem::Action { id: 0, label: format!("Audian — {}", self.status()), checked: false, enabled: false },
            MenuItem::Separator,
            MenuItem::Action { id: CMD_TOGGLE, label: toggle_label.into(), checked: false, enabled: !matches!(self.phase, Phase::Processing { .. }) },
            MenuItem::Action {
                id: CMD_COPY_LAST,
                label: format!("Copy last dictation{paste_hint}"),
                checked: false,
                enabled: self.last_text.is_some(),
            },
            MenuItem::Separator,
            MenuItem::Submenu { label: format!("Mode: {}", cfg.processing.mode.label()), items: modes },
            MenuItem::Submenu { label: "Rewriting".into(), items: rewriting },
            MenuItem::Submenu { label: "Output language".into(), items: outputs },
            MenuItem::Submenu { label: "Microphone".into(), items: mics },
            MenuItem::Action { id: CMD_AUTO_STOP, label: "Auto-stop when I pause".into(), checked: cfg.auto_stop.enabled, enabled: true },
            MenuItem::Action { id: CMD_PROFILES, label: "Per-app modes".into(), checked: cfg.profiles.enabled, enabled: true },
            MenuItem::Separator,
            MenuItem::action(CMD_SETTINGS, "Open Audian…"),
            MenuItem::action(CMD_HISTORY, "History…"),
            MenuItem::Submenu {
                label: "More".into(),
                items: vec![
                    MenuItem::action(CMD_RELOAD, "Reload configuration"),
                    MenuItem::action(CMD_OPEN_CONFIG, "Open configuration file"),
                    MenuItem::action(CMD_LOGS, "Open logs folder"),
                ],
            },
            MenuItem::Separator,
            MenuItem::action(CMD_EXIT, "Exit Audian"),
        ]
    }

    fn on_menu(&mut self, cmd: u32) {
        let mut cfg = self.cfg.clone();
        match cmd {
            CMD_TOGGLE => match self.phase {
                Phase::Idle => self.start_recording_from_menu(),
                Phase::Recording(_) => self.stop_recording(),
                Phase::Processing { .. } => {}
            },
            CMD_COPY_LAST => {
                if let Some(text) = &self.last_text {
                    let _ = crate::inject::clipboard::set_text(self.hwnd, text, false);
                }
            }
            CMD_SETTINGS => self.open_settings(None),
            CMD_HISTORY => self.open_settings(Some("history")),
            CMD_RELOAD => self.reload_config(),
            CMD_LOGS => open_path(&paths::logs_dir()),
            CMD_OPEN_CONFIG => open_path(&paths::config_file()),
            CMD_EXIT => unsafe { PostQuitMessage(0) },
            CMD_REWRITE_ENABLED => {
                cfg.processing.enabled = !cfg.processing.enabled;
                self.save_and_apply(cfg);
            }
            CMD_AUTO_STOP => {
                cfg.auto_stop.enabled = !cfg.auto_stop.enabled;
                self.save_and_apply(cfg);
            }
            CMD_PROFILES => {
                cfg.profiles.enabled = !cfg.profiles.enabled;
                self.save_and_apply(cfg);
            }
            c if (CMD_MODE_BASE..CMD_MODE_BASE + ProcessingMode::ALL.len() as u32).contains(&c) => {
                cfg.processing.mode = ProcessingMode::ALL[(c - CMD_MODE_BASE) as usize];
                self.save_and_apply(cfg);
            }
            c if (CMD_PROVIDER_BASE..CMD_PROVIDER_BASE + RewriteProvider::ALL.len() as u32).contains(&c) => {
                cfg.processing.provider = RewriteProvider::ALL[(c - CMD_PROVIDER_BASE) as usize];
                if cfg.processing.provider.is_cloud() {
                    self.notify(
                        "Cloud rewriting enabled",
                        "Transcribed text will be sent to the cloud via your Antigravity account. Audio stays on this computer.",
                        false,
                    );
                } else if cfg.processing.provider == RewriteProvider::LocalLlm && models_missing(&cfg) {
                    self.open_settings(Some("models"));
                }
                self.save_and_apply(cfg);
            }
            c if (CMD_OUTPUT_BASE..CMD_OUTPUT_BASE + OUTPUT_LANGUAGES.len() as u32).contains(&c) => {
                cfg.processing.output_language = OUTPUT_LANGUAGES[(c - CMD_OUTPUT_BASE) as usize].0.to_string();
                self.save_and_apply(cfg);
            }
            CMD_MIC_DEFAULT => {
                cfg.audio.device_id.clear();
                cfg.audio.device_name.clear();
                self.save_and_apply(cfg);
            }
            c if c >= CMD_MIC_BASE && ((c - CMD_MIC_BASE) as usize) < self.menu_devices.len() => {
                let d = &self.menu_devices[(c - CMD_MIC_BASE) as usize];
                cfg.audio.device_id = d.id.clone();
                cfg.audio.device_name = d.name.clone();
                self.save_and_apply(cfg);
            }
            _ => {}
        }
    }

    /// Starting from the menu: the menu itself took focus, so target whatever window becomes
    /// foreground once it closes, and record hands-free (there is no key to hold).
    fn start_recording_from_menu(&mut self) {
        self.start_recording();
        self.kill_timer(TIMER_KEY_POLL);
        let mut became_hands_free = false;
        if let Phase::Recording(r) = &mut self.phase {
            r.target = None;
            if !r.hands_free {
                r.hands_free = true;
                became_hands_free = true;
            }
        }
        if became_hands_free {
            self.enter_hands_free();
        }
    }
}

fn open_path(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

