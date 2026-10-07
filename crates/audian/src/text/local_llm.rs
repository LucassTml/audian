//! Local LLM provider: a small GGUF model on the CPU, running in the `audian-llm` helper.

use std::time::Duration;

use audian_common::config::{LocalLlmConfig, ProcessingMode};
use audian_common::ipc::{ChatMessage, LlmRequest, LlmResponse};
use audian_common::paths;

use super::{ProcessError, RewriteRequest, TextProcessor, prompts, sanitize};
use crate::helper::{FramedHelper, HelperError};

const CONTEXT_TOKENS: u32 = 4096;

pub struct LocalLlm {
    model_path: String,
    threads: u32,
    idle_secs: u32,
    max_tokens: u32,
    timeout: Duration,
    helper: Option<FramedHelper>,
    /// Replies to warm-up requests (Load / Prepare) not read yet.
    pending: usize,
    loaded: bool,
    prepared: Option<Vec<ChatMessage>>,
    next_id: u64,
}

impl LocalLlm {
    pub fn new(cfg: &LocalLlmConfig) -> Self {
        LocalLlm {
            model_path: paths::resolve_model(&cfg.model).to_string_lossy().into_owned(),
            threads: cfg.threads,
            idle_secs: cfg.keep_loaded_minutes.max(1) * 60,
            max_tokens: cfg.max_output_tokens.max(64),
            timeout: Duration::from_secs(cfg.timeout_secs.max(5) as u64),
            helper: None,
            pending: 0,
            loaded: false,
            prepared: None,
            next_id: 1,
        }
    }

    fn reset(&mut self) {
        if let Some(h) = self.helper.as_mut() {
            h.kill();
        }
        self.helper = None;
        self.pending = 0;
        self.loaded = false;
        self.prepared = None;
    }

    fn ensure_helper(&mut self) -> Result<(), ProcessError> {
        if !std::path::Path::new(&self.model_path).is_file() {
            return Err(ProcessError::ModelMissing(self.model_path.clone()));
        }
        if self.helper.as_mut().is_some_and(|h| h.is_alive()) {
            return Ok(());
        }
        self.reset();
        let exe = paths::exe_dir().join("audian-llm.exe");
        if !exe.is_file() {
            return Err(ProcessError::Unavailable("The local rewriting engine (audian-llm.exe) is missing.".into()));
        }
        let helper = FramedHelper::spawn(&exe, &["--idle-exit-secs".into(), self.idle_secs.to_string()])
            .map_err(|e| ProcessError::Engine(e.to_string()))?;
        self.helper = Some(helper);
        Ok(())
    }

    /// Queues Load and Prepare without waiting for replies.
    fn start(&mut self, mode: ProcessingMode, custom: &str, output_language: &str) -> Result<(), ProcessError> {
        self.ensure_helper()?;
        let prefix = prompts::local_prefix(mode, custom, output_language);
        let (model_path, threads) = (self.model_path.clone(), self.threads);
        let helper = self.helper.as_mut().unwrap();
        if !self.loaded && self.pending == 0 {
            helper
                .send(&LlmRequest::Load { model_path, threads, context_tokens: CONTEXT_TOKENS })
                .map_err(map_err)?;
            self.pending += 1;
        }
        if self.prepared.as_ref() != Some(&prefix) {
            helper.send(&LlmRequest::Prepare { prefix: prefix.clone() }).map_err(map_err)?;
            self.pending += 1;
            self.prepared = Some(prefix);
        }
        Ok(())
    }

    fn drain_pending(&mut self) -> Result<(), ProcessError> {
        while self.pending > 0 {
            let helper = self.helper.as_mut().ok_or_else(|| ProcessError::Engine("not running".into()))?;
            let reply: LlmResponse = helper.recv(Duration::from_secs(180)).map_err(map_err)?;
            self.pending -= 1;
            match reply {
                LlmResponse::Loaded { load_ms, .. } => {
                    self.loaded = true;
                    log::info!("rewriting model ready (load {load_ms} ms)");
                }
                LlmResponse::Prepared { prefix_tokens, elapsed_ms } => {
                    log::debug!("prompt prefix ready ({prefix_tokens} tokens, {elapsed_ms} ms)");
                }
                LlmResponse::Error { message, .. } => {
                    self.reset();
                    return Err(ProcessError::Engine(message));
                }
                LlmResponse::Output { .. } => {}
            }
        }
        Ok(())
    }

    fn try_process(&mut self, request: &RewriteRequest) -> Result<String, ProcessError> {
        self.start(request.mode, request.custom_instructions, request.output_language)?;
        self.drain_pending()?;
        let id = self.next_id;
        self.next_id += 1;
        let prefix = self.prepared.clone().unwrap_or_default();
        let chars = request.text.chars().count() as u32;
        let max_tokens = self.max_tokens.min(chars / 2 + 160);
        let helper = self.helper.as_mut().ok_or_else(|| ProcessError::Engine("not running".into()))?;
        helper
            .send(&LlmRequest::Generate { id, prefix, user: prompts::wrap_transcript(request.text, request.language, request.output_language), max_tokens })
            .map_err(map_err)?;
        loop {
            match helper.recv::<LlmResponse>(self.timeout).map_err(map_err)? {
                LlmResponse::Output { id: rid, text, generated_tokens, elapsed_ms, .. } if rid == id => {
                    log::info!("local rewrite: {generated_tokens} tokens in {elapsed_ms} ms");
                    return sanitize::clean_output(&text, request.text, request.mode == ProcessingMode::AiPrompt);
                }
                LlmResponse::Error { message, .. } => return Err(ProcessError::Engine(message)),
                _ => continue,
            }
        }
    }
}

fn map_err(e: HelperError) -> ProcessError {
    match e {
        HelperError::Timeout(_) => ProcessError::Timeout,
        other => ProcessError::Engine(other.to_string()),
    }
}

impl TextProcessor for LocalLlm {
    fn name(&self) -> &'static str {
        "Local LLM"
    }

    fn is_cloud(&self) -> bool {
        false
    }

    fn warm_up(&mut self, mode: ProcessingMode, custom: &str, output_language: &str) {
        if let Err(e) = self.start(mode, custom, output_language) {
            log::warn!("local LLM warm-up failed: {e}");
        }
    }

    fn process(&mut self, request: &RewriteRequest) -> Result<String, ProcessError> {
        let result = match self.try_process(request) {
            Err(ProcessError::Engine(msg)) if msg.contains("stopped unexpectedly") => {
                log::warn!("rewriting helper died ({msg}); retrying once");
                self.reset();
                self.try_process(request)
            }
            other => other,
        };
        if matches!(result, Err(ProcessError::Timeout)) {
            self.reset(); // the helper may be stuck mid-generation
        }
        result
    }
}
