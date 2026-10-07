//! Local whisper.cpp provider, running in the `audian-stt` helper process.

use std::time::Duration;

use audian_common::config::WhisperConfig;
use audian_common::ipc::{SttRequest, SttResponse};
use audian_common::paths;

use super::{SttError, Transcript, TranscribeOptions, TranscriptionProvider};
use crate::helper::{FramedHelper, HelperError};

pub struct WhisperLocal {
    model_path: String,
    threads: u32,
    idle_secs: u32,
    helper: Option<FramedHelper>,
    /// A `Load` was sent and its reply has not been read yet.
    load_pending: bool,
    loaded: bool,
    next_id: u64,
}

impl WhisperLocal {
    pub fn new(cfg: &WhisperConfig) -> Self {
        WhisperLocal {
            model_path: paths::resolve_model(&cfg.model).to_string_lossy().into_owned(),
            threads: cfg.threads,
            idle_secs: cfg.keep_loaded_minutes.max(1) * 60,
            helper: None,
            load_pending: false,
            loaded: false,
            next_id: 1,
        }
    }

    fn helper(&mut self) -> Result<&mut FramedHelper, SttError> {
        let alive = self.helper.as_mut().is_some_and(|h| h.is_alive());
        if !alive {
            self.helper = None;
            self.loaded = false;
            self.load_pending = false;
            let exe = paths::exe_dir().join("audian-stt.exe");
            if !exe.is_file() {
                return Err(SttError::EngineMissing);
            }
            let helper = FramedHelper::spawn(&exe, &["--idle-exit-secs".into(), self.idle_secs.to_string()])
                .map_err(|e| SttError::Engine(e.to_string()))?;
            self.helper = Some(helper);
        }
        Ok(self.helper.as_mut().unwrap())
    }

    fn start_load(&mut self) -> Result<(), SttError> {
        if !std::path::Path::new(&self.model_path).is_file() {
            return Err(SttError::ModelMissing(self.model_path.clone()));
        }
        let (model_path, threads) = (self.model_path.clone(), self.threads);
        self.helper()?;
        if !self.loaded && !self.load_pending {
            let helper = self.helper.as_mut().unwrap();
            helper.send(&SttRequest::Load { model_path, threads }).map_err(map_err)?;
            self.load_pending = true;
        }
        Ok(())
    }

    fn finish_load(&mut self) -> Result<(), SttError> {
        if !self.load_pending {
            return Ok(());
        }
        let helper = self.helper.as_mut().ok_or_else(|| SttError::Engine("not running".into()))?;
        let response: SttResponse = helper.recv(Duration::from_secs(120)).map_err(map_err)?;
        self.load_pending = false;
        match response {
            SttResponse::Loaded { load_ms, .. } => {
                self.loaded = true;
                if load_ms > 0 {
                    log::info!("speech model loaded in {load_ms} ms");
                }
                Ok(())
            }
            SttResponse::Error { message, .. } => Err(SttError::Engine(message)),
            other => Err(SttError::Engine(format!("unexpected reply {other:?}"))),
        }
    }

    fn try_transcribe(&mut self, audio: &[f32], options: &TranscribeOptions) -> Result<Transcript, SttError> {
        self.start_load()?;
        self.finish_load()?;
        let id = self.next_id;
        self.next_id += 1;
        let initial_prompt = vocabulary_prompt(&options.vocabulary);
        let helper = self.helper.as_mut().ok_or_else(|| SttError::Engine("not running".into()))?;
        helper
            .send(&SttRequest::Transcribe {
                id,
                language: options.language.clone(),
                candidates: options.candidates.clone(),
                initial_prompt,
            })
            .map_err(map_err)?;
        helper.send_samples(audio).map_err(map_err)?;
        // Generous timeout: ~realtime worst case on slow machines plus model warm-up.
        let secs = audio.len() as u64 / audian_common::SAMPLE_RATE as u64;
        let timeout = Duration::from_secs(30 + secs * 2);
        loop {
            match helper.recv::<SttResponse>(timeout).map_err(map_err)? {
                SttResponse::Transcript { id: rid, text, language, elapsed_ms, no_speech_prob } if rid == id => {
                    return Ok(Transcript { text, language, no_speech_prob, elapsed_ms });
                }
                SttResponse::Error { message, .. } => return Err(SttError::Engine(message)),
                _ => continue, // stale reply from an earlier, abandoned request
            }
        }
    }
}

fn map_err(e: HelperError) -> SttError {
    match e {
        HelperError::Timeout(_) => SttError::Timeout,
        other => SttError::Engine(other.to_string()),
    }
}

/// Whisper's "initial prompt" biases decoding towards the given spellings.
fn vocabulary_prompt(words: &[String]) -> String {
    let words: Vec<&str> = words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() { String::new() } else { format!("Glossary: {}.", words.join(", ")) }
}

impl TranscriptionProvider for WhisperLocal {
    fn name(&self) -> &'static str {
        "Whisper"
    }

    fn is_local(&self) -> bool {
        true
    }

    fn warm_up(&mut self) -> Result<(), SttError> {
        self.start_load()
    }

    fn transcribe(&mut self, audio: &[f32], options: &TranscribeOptions) -> Result<Transcript, SttError> {
        match self.try_transcribe(audio, options) {
            Err(SttError::Engine(msg)) if msg.contains("stopped unexpectedly") => {
                // The helper may have exited for being idle just as we used it; retry once.
                log::warn!("speech helper died ({msg}); retrying once");
                self.helper = None;
                self.try_transcribe(audio, options)
            }
            Err(SttError::Timeout) => {
                if let Some(h) = self.helper.as_mut() {
                    h.kill();
                }
                self.helper = None;
                Err(SttError::Timeout)
            }
            other => other,
        }
    }
}
