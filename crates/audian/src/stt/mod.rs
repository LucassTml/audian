//! Speech-to-text: `audio (16 kHz mono f32) -> TranscriptionProvider -> text`.
//!
//! Providers are interchangeable behind the trait; today both are local engines running in
//! helper processes: whisper.cpp and NVIDIA Parakeet.

mod local;

use audian_common::config::{Config, SttProvider};

pub use local::LocalEngine;

pub struct TranscribeOptions {
    /// ISO 639-1 code or "auto".
    pub language: String,
    /// Restricts "auto" detection to these languages (empty = any).
    pub candidates: Vec<String>,
    /// Names and jargon to bias recognition towards.
    pub vocabulary: Vec<String>,
}

pub struct Transcript {
    pub text: String,
    pub language: String,
    pub no_speech_prob: f32,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("Speech model not found ({0}). Choose an installed model in Settings › Transcription.")]
    ModelMissing(String),
    #[error("The speech engine is not installed next to Audian ({0} missing).")]
    EngineMissing(&'static str),
    #[error("The speech engine failed: {0}")]
    Engine(String),
    #[error("Transcription took too long and was cancelled.")]
    Timeout,
}

pub trait TranscriptionProvider: Send {
    fn name(&self) -> &'static str;

    /// True if audio never leaves this computer.
    fn is_local(&self) -> bool;

    /// Called when recording starts so model loading overlaps with the user speaking.
    fn warm_up(&mut self) -> Result<(), SttError> {
        Ok(())
    }

    fn transcribe(&mut self, audio: &[f32], options: &TranscribeOptions) -> Result<Transcript, SttError>;
}

pub fn create(cfg: &Config) -> Box<dyn TranscriptionProvider> {
    let t = &cfg.transcription;
    match t.provider {
        SttProvider::WhisperLocal => Box::new(LocalEngine::whisper(&t.whisper)),
        SttProvider::Parakeet => Box::new(LocalEngine::parakeet(&t.parakeet)),
    }
}

/// Key describing the provider configuration; when it changes the provider is recreated.
pub fn config_key(cfg: &Config) -> String {
    let t = &cfg.transcription;
    match t.provider {
        SttProvider::WhisperLocal => format!("whisper|{:?}", t.whisper),
        SttProvider::Parakeet => format!("parakeet|{:?}", t.parakeet),
    }
}

/// Minutes the current speech engine stays loaded after a dictation (0 = unload right away).
pub fn keep_loaded_minutes(cfg: &Config) -> u32 {
    let t = &cfg.transcription;
    match t.provider {
        SttProvider::WhisperLocal => t.whisper.keep_loaded_minutes,
        SttProvider::Parakeet => t.parakeet.keep_loaded_minutes,
    }
}
