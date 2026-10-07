//! The dictation pipeline: recording -> 16 kHz audio -> transcript -> processed text.
//! Runs on a worker thread; the UI thread only starts it and receives the result.

use std::time::Instant;

use audian_common::config::{Config, ProcessingMode};
use audian_common::{SAMPLE_RATE, paths, wav};

use crate::audio::{Recording, dsp};
use crate::stt::{self, SttError, TranscribeOptions, TranscriptionProvider};
use crate::text::{self, ProcessError, RewriteRequest, TextProcessor, rules, sanitize};

/// Long-lived providers (they own helper processes). Shared behind a mutex between the
/// warm-up thread, preview transcriptions and pipeline jobs.
pub struct Engines {
    stt: Box<dyn TranscriptionProvider>,
    stt_key: String,
    rewriter: Box<dyn TextProcessor>,
    rewriter_key: String,
    /// Dictation session the engines were last warmed up for.
    warmed_for: u64,
}

impl Engines {
    pub fn new(cfg: &Config) -> Engines {
        Engines {
            stt: stt::create(cfg),
            stt_key: stt::config_key(cfg),
            rewriter: text::create(cfg),
            rewriter_key: text::config_key(cfg),
            warmed_for: 0,
        }
    }

    /// Recreates providers whose configuration changed (dropping old helper processes).
    pub fn reconfigure(&mut self, cfg: &Config) {
        let key = stt::config_key(cfg);
        if key != self.stt_key {
            log::info!("speech-to-text configuration changed");
            self.stt = stt::create(cfg);
            self.stt_key = key;
        }
        let key = text::config_key(cfg);
        if key != self.rewriter_key {
            log::info!("rewriting configuration changed");
            self.rewriter = text::create(cfg);
            self.rewriter_key = key;
        }
    }

    pub fn warm_up(&mut self, cfg: &Config, session: u64) {
        self.warmed_for = session;
        self.reconfigure(cfg);
        if let Err(e) = self.stt.warm_up() {
            log::warn!("speech warm-up: {e}");
        }
        if uses_ai(cfg) {
            self.rewriter.warm_up(cfg.processing.mode, &cfg.processing.custom_instructions, &cfg.processing.output_language);
        }
    }

    /// Abandons work started for `session` (a dictation that was cancelled or had no speech).
    /// Does nothing if a newer dictation has already warmed the engines up again.
    pub fn abandon(&mut self, cfg: &Config, session: u64) {
        if self.warmed_for > session {
            return;
        }
        self.rewriter.cancel();
        self.release_if_configured(cfg);
    }

    /// With "keep loaded: 0 minutes", engines are shut down as soon as a dictation is done,
    /// returning all model memory to Windows immediately.
    pub fn release_if_configured(&mut self, cfg: &Config) {
        if cfg.transcription.whisper.keep_loaded_minutes == 0 {
            self.stt = stt::create(cfg);
        }
        if cfg.processing.local_llm.keep_loaded_minutes == 0 {
            self.rewriter = text::create(cfg);
        }
    }
}

fn uses_ai(cfg: &Config) -> bool {
    cfg.processing.enabled && cfg.processing.mode != ProcessingMode::Literal
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Transcribing,
    Rewriting,
}

/// A transcript computed while the user was still in a pause (see `endpoint`).
pub struct Precomputed {
    pub text: String,
    pub language: String,
}

pub struct Job {
    pub recording: Recording,
    /// Configuration snapshot, with the session's processing mode already applied.
    pub cfg: Config,
    pub target_is_terminal: bool,
    pub precomputed: Option<Precomputed>,
}

#[derive(Debug)]
pub struct Outcome {
    pub text: String,
    pub raw: String,
    pub language: String,
    pub provider: String,
    pub audio_ms: u64,
    pub process_ms: u64,
    /// Non-fatal problem worth telling the user about (e.g. AI failed, basic cleanup used).
    pub notice: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("No speech detected.")]
    NoSpeech,
    #[error(transparent)]
    Transcription(#[from] SttError),
    #[error(transparent)]
    Rewrite(ProcessError),
}

/// Resample to 16 kHz, trim leading/trailing silence, normalise.
fn prepare(samples: &[f32], sample_rate: u32, cfg: &Config) -> Option<Vec<f32>> {
    let audio = dsp::resample(samples, sample_rate, SAMPLE_RATE);
    let mut audio = dsp::trim_silence(&audio, SAMPLE_RATE, cfg.audio.silence_threshold * 0.6, 300).to_vec();
    if audio.len() < (SAMPLE_RATE as usize) / 5 {
        return None;
    }
    dsp::normalize(&mut audio);
    Some(audio)
}

fn options(cfg: &Config) -> TranscribeOptions {
    TranscribeOptions {
        language: cfg.transcription.language.clone(),
        candidates: cfg.transcription.auto_languages.clone(),
        vocabulary: cfg.transcription.vocabulary.clone(),
    }
}

/// Transcribes audio captured so far, during a pause (used by smart auto-stop).
pub fn preview(engines: &mut Engines, samples: &[f32], sample_rate: u32, cfg: &Config) -> Option<Precomputed> {
    engines.reconfigure(cfg);
    let audio = prepare(samples, sample_rate, cfg)?;
    let t = engines.stt.transcribe(&audio, &options(cfg)).ok()?;
    let text = rules::strip_annotations(&t.text).trim().to_string();
    Some(Precomputed { text, language: t.language })
}

pub fn run(engines: &mut Engines, job: Job, on_stage: &dyn Fn(Stage)) -> Result<Outcome, PipelineError> {
    let started = Instant::now();
    let cfg = &job.cfg;
    engines.reconfigure(cfg);
    let audio_ms = job.recording.duration().as_millis() as u64;

    let (raw, language, stt_ms) = match job.precomputed {
        Some(p) => {
            log::info!("reusing transcript computed during the pause");
            (p.text, p.language, 0)
        }
        None => {
            on_stage(Stage::Transcribing);
            let Recording { samples, sample_rate, .. } = job.recording;
            let audio = prepare(&samples, sample_rate, cfg).ok_or(PipelineError::NoSpeech)?;
            drop(samples);
            if cfg.privacy.save_recordings {
                save_recording(&audio);
            }
            let t = engines.stt.transcribe(&audio, &options(cfg))?;
            if rules::is_hallucination(&t.text) && t.no_speech_prob > 0.4 {
                return Err(PipelineError::NoSpeech);
            }
            (rules::strip_annotations(&t.text).trim().to_string(), t.language, t.elapsed_ms)
        }
    };
    if raw.is_empty() {
        return Err(PipelineError::NoSpeech);
    }
    if cfg.privacy.log_transcripts {
        log::info!("transcript ({language}): {raw}");
    }

    let rewrite_started = Instant::now();
    let mut notice = None;
    let mut provider = "none".to_string();
    let mut text = if !uses_ai(cfg) {
        rules::literal(&raw)
    } else {
        on_stage(Stage::Rewriting);
        provider = engines.rewriter.name().to_string();
        let request = RewriteRequest {
            text: &raw,
            language: &language,
            output_language: &cfg.processing.output_language,
            mode: cfg.processing.mode,
            custom_instructions: &cfg.processing.custom_instructions,
        };
        match engines.rewriter.process(&request) {
            Ok(t) => t,
            Err(e) if cfg.processing.fallback_to_rules => {
                log::warn!("{} failed: {e}; using rule-based cleanup", engines.rewriter.name());
                notice = Some(format!("{e} Basic cleanup was used instead."));
                provider = "Offline rules (fallback)".into();
                rules::clean(&raw)
            }
            Err(e) => return Err(PipelineError::Rewrite(e)),
        }
    };
    let rewrite_ms = rewrite_started.elapsed().as_millis();

    if job.target_is_terminal && cfg.insertion.terminal_single_line {
        text = sanitize::single_line(&text);
    }
    if cfg.insertion.trailing_space && !text.ends_with(char::is_whitespace) {
        text.push(' ');
    }
    if cfg.privacy.log_transcripts {
        log::info!("final text: {text}");
    }
    let process_ms = started.elapsed().as_millis() as u64;
    log::info!(
        "pipeline done in {process_ms} ms (speech {stt_ms} ms via {} ({}), rewrite {rewrite_ms} ms via {provider}{})",
        engines.stt.name(),
        if engines.stt.is_local() { "local" } else { "cloud" },
        if uses_ai(cfg) && engines.rewriter.is_cloud() { " (cloud)" } else { "" },
    );
    engines.release_if_configured(cfg);
    Ok(Outcome { text, raw, language, provider, audio_ms, process_ms, notice })
}

fn save_recording(audio: &[f32]) {
    let dir = paths::recordings_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let path = dir.join(format!("dictation-{stamp}.wav"));
    if let Err(e) = wav::write_pcm16_mono(&path, audio, SAMPLE_RATE) {
        log::warn!("could not save recording: {e:#}");
    }
}
