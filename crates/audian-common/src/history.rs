//! Local dictation history (text only — never audio) and usage statistics.
//!
//! History is an append-only JSON-lines file trimmed to the configured length; it can be
//! disabled or cleared in Settings › Privacy. Statistics are plain counters.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::paths;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Entry {
    /// Local time, "YYYY-MM-DD HH:MM".
    pub time: String,
    pub app: String,
    pub mode: String,
    pub provider: String,
    pub language: String,
    pub raw: String,
    pub text: String,
    pub audio_ms: u64,
    pub process_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Stats {
    pub dictations: u64,
    pub words: u64,
    pub audio_secs: f64,
    /// First use, "YYYY-MM-DD".
    pub since: String,
}

impl Stats {
    /// Minutes saved compared with typing at 40 words per minute.
    pub fn minutes_saved(&self) -> f64 {
        (self.words as f64 / 40.0 - self.audio_secs / 60.0).max(0.0)
    }
}

pub fn now_local() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute)
}

pub fn word_count(text: &str) -> u64 {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count() as u64
}

pub fn append(entry: &Entry, limit: u32) {
    let path = paths::history_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(line) = serde_json::to_string(entry) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
    // Trim occasionally rather than on every write.
    let lines = count_lines(&path);
    if lines > limit as usize + 25 {
        let keep: Vec<String> = read_lines(&path).into_iter().rev().take(limit as usize).collect::<Vec<_>>().into_iter().rev().collect();
        let tmp = path.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, keep.join("\n") + "\n").is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::File::open(path)
        .map(|f| std::io::BufReader::new(f).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).collect())
        .unwrap_or_default()
}

fn count_lines(path: &std::path::Path) -> usize {
    read_lines(path).len()
}

/// Newest first.
pub fn load() -> Vec<Entry> {
    let mut v: Vec<Entry> = read_lines(&paths::history_file()).iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v
}

pub fn clear() {
    let _ = std::fs::remove_file(paths::history_file());
}

pub fn load_stats() -> Stats {
    std::fs::read_to_string(paths::stats_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn record_stats(words: u64, audio_ms: u64) {
    let mut s = load_stats();
    if s.since.is_empty() {
        s.since = now_local().chars().take(10).collect();
    }
    s.dictations += 1;
    s.words += words;
    s.audio_secs += audio_ms as f64 / 1000.0;
    if let Ok(text) = serde_json::to_string_pretty(&s) {
        let _ = std::fs::write(paths::stats_file(), text);
    }
}

pub fn reset_stats() {
    let _ = std::fs::remove_file(paths::stats_file());
}
