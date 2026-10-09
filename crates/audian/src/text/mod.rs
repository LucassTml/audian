//! Text processing: `raw transcript -> TextProcessor -> final text`.
//!
//! Rewriting modes are data (see `prompts`), so adding a mode means adding a prompt, not
//! code. Processors are interchangeable behind the trait: offline rules, a local LLM, or
//! Antigravity (cloud). The rules processor doubles as the fallback when an AI provider fails.

mod antigravity;
pub mod local_llm;
mod corrections;
pub mod prompts;
pub mod rules;
pub mod sanitize;

use audian_common::config::{Config, ProcessingMode, RewriteProvider};

pub use antigravity::Antigravity;
pub use local_llm::LocalLlm;
pub use rules::Rules;

pub struct RewriteRequest<'a> {
    pub text: &'a str,
    /// Language code detected by speech recognition ("en", "pt", ...) or "auto" if unknown.
    pub language: &'a str,
    /// "same", or a language code to translate the result into.
    pub output_language: &'a str,
    pub mode: ProcessingMode,
    pub custom_instructions: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Rewriting model not found ({0}). Choose an installed model in Settings › Rewriting.")]
    ModelMissing(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("The rewriting engine failed: {0}")]
    Engine(String),
    #[error("Rewriting took too long and was cancelled.")]
    Timeout,
    #[error("The AI returned an unusable result.")]
    BadOutput,
}

pub trait TextProcessor: Send {
    fn name(&self) -> &'static str;

    /// True if text is sent off this computer.
    fn is_cloud(&self) -> bool;

    /// Called when recording starts: load models / start processes while the user speaks.
    fn warm_up(&mut self, _mode: ProcessingMode, _custom_instructions: &str, _output_language: &str) {}

    /// Called when a dictation is cancelled, to release anything started by `warm_up`.
    fn cancel(&mut self) {}

    fn process(&mut self, request: &RewriteRequest) -> Result<String, ProcessError>;
}

pub fn create(cfg: &Config) -> Box<dyn TextProcessor> {
    match cfg.processing.provider {
        RewriteProvider::Rules => Box::new(Rules),
        RewriteProvider::LocalLlm => Box::new(LocalLlm::new(&cfg.processing.local_llm)),
        RewriteProvider::Antigravity => Box::new(Antigravity::new(&cfg.processing.antigravity)),
    }
}

/// Location of the Antigravity CLI, if installed (for the settings UI).
pub fn antigravity_path(configured: &str) -> Option<std::path::PathBuf> {
    antigravity::find_agy(configured)
}

pub fn config_key(cfg: &Config) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        cfg.processing.provider, cfg.processing.local_llm, cfg.processing.antigravity
    )
}
