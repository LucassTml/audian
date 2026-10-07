//! Well-known filesystem locations.
//!
//! * Roaming config (`%APPDATA%\Audian`) holds the small, user-editable settings file.
//! * Local data (`%LOCALAPPDATA%\Audian`) holds models, logs and other machine-specific data.

use std::path::PathBuf;

use crate::APP_NAME;

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join(APP_NAME)
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join(APP_NAME)
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Disposable caches (e.g. the rewriting engine's evaluated prompt). Safe to delete any time.
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn history_file() -> PathBuf {
    data_dir().join("history.jsonl")
}

pub fn stats_file() -> PathBuf {
    data_dir().join("stats.json")
}

/// Only used when the user explicitly enables "keep recordings".
pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// Isolated working directory for the Antigravity CLI, holding our custom rewriter agent.
pub fn agy_workspace_dir() -> PathBuf {
    data_dir().join("agy-workspace")
}

/// Directory containing the running executable (helper binaries live next to it).
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolves a model reference: absolute paths are used as-is, bare file names are looked up
/// in the models directory.
pub fn resolve_model(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() { p } else { models_dir().join(p) }
}
