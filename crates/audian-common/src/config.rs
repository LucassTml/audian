//! User configuration, persisted as TOML in `%APPDATA%\Audian\config.toml`.
//!
//! Every struct uses `#[serde(default)]` so that a config written by an older version (or
//! edited by hand with missing keys) still loads; unknown keys are ignored.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::paths;
pub use crate::theme::{IndicatorStyle, ThemeId, WindowMode};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub hotkey: HotkeyConfig,
    pub auto_stop: AutoStopConfig,
    pub audio: AudioConfig,
    pub transcription: TranscriptionConfig,
    pub processing: ProcessingConfig,
    pub profiles: ProfilesConfig,
    pub insertion: InsertionConfig,
    pub overlay: OverlayConfig,
    pub appearance: AppearanceConfig,
    pub privacy: PrivacyConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GeneralConfig {
    pub start_with_windows: bool,
    /// Show Windows notifications for errors and important events.
    pub notifications: bool,
    /// Soft audio cues when recording starts/stops.
    pub sounds: bool,
    /// 0.0 – 1.0
    pub sound_volume: f32,
    /// The first-run welcome has been completed.
    pub welcome_done: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self { start_with_windows: false, notifications: true, sounds: true, sound_volume: 0.5, welcome_done: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    /// Record only while the shortcut is held down.
    Hold,
    /// Press once to start, press again (or pause) to stop.
    Toggle,
    /// Hold to talk, or tap quickly to switch to hands-free mode.
    Hybrid,
}

impl RecordingMode {
    pub const ALL: [RecordingMode; 3] = [Self::Hybrid, Self::Hold, Self::Toggle];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hold => "Push-to-talk",
            Self::Toggle => "Toggle",
            Self::Hybrid => "Hybrid",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Hold => "Record only while the shortcut is held down.",
            Self::Toggle => "Press once to start; press again or pause to finish.",
            Self::Hybrid => "Hold to talk, or tap once for hands-free dictation.",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HotkeyConfig {
    pub shortcut: String,
    pub mode: RecordingMode,
    /// While recording or processing, Esc cancels the dictation.
    pub escape_cancels: bool,
    /// Optional shortcut that pastes the last dictation again (empty = disabled).
    pub paste_last_shortcut: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            shortcut: "Ctrl+Shift+Space".into(),
            mode: RecordingMode::Hybrid,
            escape_cancels: true,
            paste_last_shortcut: "Ctrl+Shift+Alt+V".into(),
        }
    }
}

/// End-of-speech detection for hands-free dictation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AutoStopConfig {
    /// Stop hands-free recordings automatically when you stop talking.
    pub enabled: bool,
    /// Silence (seconds) after speech that counts as "finished".
    pub pause_secs: f32,
    /// Look at what was said so far: if the sentence sounds unfinished ("…and", "…porque"),
    /// wait longer before stopping.
    pub smart: bool,
    /// Extra seconds of patience when the sentence sounds unfinished.
    pub unfinished_extra_secs: f32,
    /// Cancel a hands-free recording if nothing is said for this long.
    pub no_speech_secs: f32,
}

impl Default for AutoStopConfig {
    fn default() -> Self {
        Self { enabled: true, pause_secs: 2.0, smart: true, unfinished_extra_secs: 1.5, no_speech_secs: 8.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    /// Stable device identifier; empty means "system default input device".
    pub device_id: String,
    /// Human-readable name, used for display and as a fallback match if the id changes.
    pub device_name: String,
    /// Software gain applied to captured samples (1.0 = unchanged).
    pub gain: f32,
    pub max_duration_secs: u32,
    /// Recordings whose loudest moment stays below this RMS level are treated as silence.
    pub silence_threshold: f32,
    /// Voice detection sensitivity, 0.0 (only clear speech) – 1.0 (picks up soft speech).
    pub vad_sensitivity: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            device_name: String::new(),
            gain: 1.0,
            max_duration_secs: 300,
            silence_threshold: 0.006,
            vad_sensitivity: 0.5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SttProvider {
    /// whisper.cpp running locally in a helper process. Audio never leaves the machine.
    WhisperLocal,
    /// NVIDIA Parakeet TDT (ONNX Runtime) in a helper process; 25 European languages, local.
    Parakeet,
}

impl SttProvider {
    pub const ALL: [SttProvider; 2] = [Self::WhisperLocal, Self::Parakeet];

    pub fn label(self) -> &'static str {
        match self {
            Self::WhisperLocal => "Whisper (local, offline)",
            Self::Parakeet => "NVIDIA Parakeet (local, offline)",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub provider: SttProvider,
    /// ISO 639-1 code ("en", "pt", ...) or "auto".
    pub language: String,
    /// With "auto": only choose among these languages (empty = any language).
    pub auto_languages: Vec<String>,
    /// Words and names the recognizer should favour (product names, jargon, people).
    pub vocabulary: Vec<String>,
    pub whisper: WhisperConfig,
    pub parakeet: ParakeetConfig,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            provider: SttProvider::WhisperLocal,
            language: "auto".into(),
            auto_languages: Vec::new(),
            vocabulary: Vec::new(),
            whisper: WhisperConfig::default(),
            parakeet: ParakeetConfig::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct WhisperConfig {
    /// Model file name inside the models directory, or an absolute path.
    pub model: String,
    /// CPU threads; 0 = choose automatically.
    pub threads: u32,
    /// Minutes the model stays in memory after the last use (0 = unload right after use, the
    /// default: it is loaded again at the next shortcut press, while the user speaks).
    pub keep_loaded_minutes: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ParakeetConfig {
    /// Model folder inside the models directory, or an absolute path.
    pub model: String,
    /// CPU threads; 0 = choose automatically.
    pub threads: u32,
    /// Minutes the model stays in memory after the last use (0 = unload right after use).
    pub keep_loaded_minutes: u32,
}

impl Default for ParakeetConfig {
    fn default() -> Self {
        Self { model: "parakeet-tdt-0.6b-v3-int8".into(), threads: 0, keep_loaded_minutes: 0 }
    }
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self { model: "ggml-small-q8_0.bin".into(), threads: 0, keep_loaded_minutes: 0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingMode {
    Literal,
    Natural,
    AiPrompt,
    Professional,
    Document,
    Custom,
}

impl ProcessingMode {
    pub const ALL: [ProcessingMode; 6] = [
        Self::Natural,
        Self::AiPrompt,
        Self::Professional,
        Self::Document,
        Self::Literal,
        Self::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Literal => "Literal",
            Self::Natural => "Natural",
            Self::AiPrompt => "AI Prompt",
            Self::Professional => "Professional",
            Self::Document => "Document",
            Self::Custom => "Custom",
        }
    }

    /// Compact label for the recording overlay.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Literal => "Literal",
            Self::Natural => "Natural",
            Self::AiPrompt => "Prompt",
            Self::Professional => "Pro",
            Self::Document => "Doc",
            Self::Custom => "Custom",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Literal => "Keeps the transcription almost verbatim; only light cleanup.",
            Self::Natural => "Cleans up speech while staying close to what you said.",
            Self::AiPrompt => "Turns spoken instructions into a clear, structured prompt.",
            Self::Professional => "Polished, formal wording for work communication.",
            Self::Document => "Well-written prose suitable for documents.",
            Self::Custom => "Follows only your custom instructions.",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RewriteProvider {
    /// Offline rule-based cleanup (filler words, spacing, capitalization). No AI.
    Rules,
    /// Small LLM running locally via llama.cpp in a helper process.
    LocalLlm,
    /// Antigravity CLI (`agy`) using the signed-in account. Text is sent to the cloud.
    Antigravity,
}

impl RewriteProvider {
    pub const ALL: [RewriteProvider; 3] = [Self::LocalLlm, Self::Antigravity, Self::Rules];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rules => "Offline rules (no AI)",
            Self::LocalLlm => "Local AI (offline)",
            Self::Antigravity => "Antigravity (cloud)",
        }
    }

    pub fn is_cloud(self) -> bool {
        matches!(self, Self::Antigravity)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ProcessingConfig {
    /// When disabled, the raw transcription is inserted (after minimal cleanup).
    pub enabled: bool,
    pub mode: ProcessingMode,
    pub provider: RewriteProvider,
    /// Extra instructions appended to every AI rewriting request.
    pub custom_instructions: String,
    /// If the AI provider fails, fall back to offline rule-based cleanup instead of failing.
    pub fallback_to_rules: bool,
    /// "same" keeps the spoken language; a language code ("en", "pt", ...) translates.
    pub output_language: String,
    pub local_llm: LocalLlmConfig,
    pub antigravity: AntigravityConfig,
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: ProcessingMode::Natural,
            provider: RewriteProvider::LocalLlm,
            custom_instructions: String::new(),
            fallback_to_rules: true,
            output_language: "same".into(),
            local_llm: LocalLlmConfig::default(),
            antigravity: AntigravityConfig::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LocalLlmConfig {
    /// GGUF file name inside the models directory, or an absolute path.
    pub model: String,
    /// CPU threads; 0 = choose automatically.
    pub threads: u32,
    /// Minutes the model stays in memory after the last use (0 = unload right after use, the
    /// default: it is loaded again at the next shortcut press, while the user speaks).
    pub keep_loaded_minutes: u32,
    pub max_output_tokens: u32,
    pub timeout_secs: u32,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            model: "Qwen3.5-2B-Q4_K_M.gguf".into(),
            threads: 0,
            keep_loaded_minutes: 0,
            max_output_tokens: 768,
            timeout_secs: 45,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AntigravityConfig {
    /// Path to `agy.exe`; empty = auto-detect.
    pub executable: String,
    pub model: String,
    pub timeout_secs: u32,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self { executable: String::new(), model: "gemini-3.8-flash-low".into(), timeout_secs: 30 }
    }
}

/// A rule that switches the processing mode automatically based on the app you dictate into.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AppProfile {
    pub name: String,
    pub enabled: bool,
    pub mode: ProcessingMode,
    /// Executable names, e.g. "Code.exe" (case-insensitive).
    pub apps: Vec<String>,
    /// Matched against the window title, e.g. "ChatGPT" for the website in a browser.
    pub title_keywords: Vec<String>,
}

impl Default for AppProfile {
    fn default() -> Self {
        Self { name: "New profile".into(), enabled: true, mode: ProcessingMode::Natural, apps: Vec::new(), title_keywords: Vec::new() }
    }
}

impl AppProfile {
    pub fn matches(&self, process: &str, title: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let process = process.to_lowercase();
        let title = title.to_lowercase();
        self.apps.iter().any(|a| !a.trim().is_empty() && a.trim().to_lowercase() == process)
            || self.title_keywords.iter().any(|k| !k.trim().is_empty() && title.contains(&k.trim().to_lowercase()))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ProfilesConfig {
    pub enabled: bool,
    pub rules: Vec<AppProfile>,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: vec![
                AppProfile {
                    name: "AI assistants".into(),
                    enabled: true,
                    mode: ProcessingMode::AiPrompt,
                    apps: strings(&["claude.exe", "ChatGPT.exe"]),
                    title_keywords: strings(&["ChatGPT", "Claude", "Gemini", "Copilot", "Perplexity"]),
                },
                AppProfile {
                    name: "Code editors".into(),
                    enabled: true,
                    mode: ProcessingMode::AiPrompt,
                    apps: strings(&["Code.exe", "Cursor.exe", "Windsurf.exe", "Antigravity IDE.exe", "zed.exe", "devenv.exe", "idea64.exe", "pycharm64.exe"]),
                    title_keywords: Vec::new(),
                },
                AppProfile {
                    name: "Email".into(),
                    enabled: true,
                    mode: ProcessingMode::Professional,
                    apps: strings(&["OUTLOOK.EXE", "olk.exe", "thunderbird.exe"]),
                    title_keywords: strings(&["Gmail", "Outlook"]),
                },
                AppProfile {
                    name: "Documents".into(),
                    enabled: true,
                    mode: ProcessingMode::Document,
                    apps: strings(&["WINWORD.EXE", "soffice.bin", "Notion.exe", "Obsidian.exe"]),
                    title_keywords: strings(&["Google Docs"]),
                },
                AppProfile {
                    name: "Work chat".into(),
                    enabled: false,
                    mode: ProcessingMode::Professional,
                    apps: strings(&["ms-teams.exe", "Teams.exe", "slack.exe"]),
                    title_keywords: Vec::new(),
                },
            ],
        }
    }
}

impl ProfilesConfig {
    /// The first enabled profile matching the target app, if profiles are on.
    pub fn find(&self, process: &str, title: &str) -> Option<&AppProfile> {
        if !self.enabled {
            return None;
        }
        self.rules.iter().find(|r| r.matches(process, title))
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InsertMethod {
    /// Put the text on the clipboard, send the paste shortcut, then restore the clipboard.
    Paste,
    /// Simulate typing each character (slower; for apps where paste is blocked).
    Type,
}

impl InsertMethod {
    pub const ALL: [InsertMethod; 2] = [Self::Paste, Self::Type];

    pub fn label(self) -> &'static str {
        match self {
            Self::Paste => "Paste (recommended)",
            Self::Type => "Simulate typing",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InsertionConfig {
    pub method: InsertMethod,
    pub restore_clipboard: bool,
    /// How long to wait after pasting before restoring the previous clipboard contents.
    pub restore_delay_ms: u32,
    /// In terminals, join multi-line output into one line so nothing executes by accident.
    pub terminal_single_line: bool,
    /// Add a trailing space so consecutive dictations don't run together.
    pub trailing_space: bool,
}

impl Default for InsertionConfig {
    fn default() -> Self {
        Self {
            method: InsertMethod::Paste,
            restore_clipboard: true,
            restore_delay_ms: 600,
            terminal_single_line: true,
            trailing_space: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    /// Near the text cursor when the app exposes it, otherwise bottom-center.
    Auto,
    BottomCenter,
    TopCenter,
}

impl OverlayPosition {
    pub const ALL: [OverlayPosition; 3] = [Self::Auto, Self::BottomCenter, Self::TopCenter];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Near the text cursor",
            Self::BottomCenter => "Bottom center",
            Self::TopCenter => "Top center",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct OverlayConfig {
    pub position: OverlayPosition,
    /// Show the mode / status text next to the waveform.
    pub show_label: bool,
    /// Dark glass or light background.
    pub style: IndicatorStyle,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self { position: OverlayPosition::Auto, show_label: true, style: IndicatorStyle::Dark }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Accent colours of the Audian window and the recording indicator.
    pub theme: ThemeId,
    /// Light or dark Audian window.
    pub mode: WindowMode,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PrivacyConfig {
    /// Keep a WAV copy of each recording in the recordings folder. Off by default.
    pub save_recordings: bool,
    /// Write transcripts and rewritten text to the debug log. Off by default.
    pub log_transcripts: bool,
    /// Keep a local history of recent dictations (text only, never audio).
    pub history: bool,
    pub history_limit: u32,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self { save_recordings: false, log_transcripts: false, history: true, history_limit: 200 }
    }
}

impl Config {
    /// Loads the configuration. A missing file yields defaults (and writes them out);
    /// a corrupt file is backed up and replaced by defaults rather than crashing.
    pub fn load() -> Config {
        let path = paths::config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(text.trim_start_matches('\u{feff}')) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::error!("config file is invalid ({e}); backing it up and using defaults");
                    let _ = std::fs::copy(&path, path.with_extension("toml.invalid"));
                    let cfg = Config::default();
                    let _ = cfg.save();
                    cfg
                }
            },
            Err(_) => {
                let cfg = Config::default();
                if let Err(e) = cfg.save() {
                    log::warn!("could not write default config: {e:#}");
                }
                cfg
            }
        }
    }

    /// Writes the configuration atomically (temp file + rename).
    pub fn save(&self) -> anyhow::Result<()> {
        save_to(self, &paths::config_file())
    }

    /// The processing mode for a dictation into the given app (app profiles applied).
    pub fn mode_for(&self, process: &str, title: &str) -> (ProcessingMode, Option<String>) {
        match self.profiles.find(process, title) {
            Some(p) => (p.mode, Some(p.name.clone())),
            None => (self.processing.mode, None),
        }
    }
}

fn save_to(cfg: &Config, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(cfg).context("serializing config")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);

        let partial: Config = toml::from_str("[hotkey]\nshortcut = \"Alt+Space\"\n").unwrap();
        assert_eq!(partial.hotkey.shortcut, "Alt+Space");
        assert_eq!(partial.hotkey.mode, RecordingMode::Hybrid);
        assert_eq!(partial.processing, ProcessingConfig::default());
    }

    #[test]
    fn profiles_match() {
        let cfg = Config::default();
        assert_eq!(cfg.mode_for("Code.exe", "main.rs - project").0, ProcessingMode::AiPrompt);
        assert_eq!(cfg.mode_for("chrome.exe", "ChatGPT - Google Chrome").0, ProcessingMode::AiPrompt);
        assert_eq!(cfg.mode_for("OUTLOOK.EXE", "Inbox").0, ProcessingMode::Professional);
        assert_eq!(cfg.mode_for("notepad.exe", "notes.txt").0, ProcessingMode::Natural);
        // Disabled profile ("Work chat") does not apply.
        assert_eq!(cfg.mode_for("slack.exe", "general").0, ProcessingMode::Natural);
    }
}
