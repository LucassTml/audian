//! Audian local rewriting helper.
//!
//! Runs a small instruction-tuned LLM (GGUF, via llama.cpp) on the CPU in a separate process,
//! for the same reasons as the speech helper: the daemon stays small, memory is released when
//! this process exits after being idle, and native crashes are isolated.
//!
//! The prompt prefix (system prompt + few-shot examples) is evaluated once and its sequence
//! state cached in memory and on disk, so a dictation only pays for its own transcript tokens
//! and a freshly started helper restores the prefix in milliseconds instead of re-evaluating
//! it. The daemon sends `Load` + `Prepare` while the user is still speaking.
//!
//! Memory: weights stay memory-mapped from the model file (no repacked private copy) and the
//! micro-batch is small, which keeps the compute buffer — dominated by the 248k-entry
//! vocabulary's logits — small too.
//!
//! Also supports `audian-llm --bench <model> <text>` for performance measurements.

#![windows_subsystem = "windows"]

use std::hash::{Hash, Hasher};
use std::io::{self, BufReader, BufWriter, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::session::LlamaStateSeqFlags;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use audian_common::ipc::{self, ChatMessage, LlmRequest, LlmResponse};

const SENTINEL: &str = "\u{1}WF_USER_CONTENT\u{1}";
const BATCH: usize = 512;
/// Tokens computed per step. llama.cpp sizes its compute buffer for the worst case of this
/// many output rows over the whole vocabulary (~1 MB per token for Qwen3.5), so 512 would
/// reserve ~0.5 GB; 128 costs ~4x less with no measurable speed difference here.
const UBATCH: u32 = 128;
/// Evaluated prefixes kept on disk (one per mode / custom instructions / output language).
const PREFIX_CACHE_FILES: usize = 6;

struct CachedPrefix {
    key: Vec<ChatMessage>,
    n_tokens: i32,
    /// Snapshot of sequence 0 after evaluating the prefix. `None` if snapshots are unsupported,
    /// in which case the prefix is re-evaluated each time.
    state: Option<llama_cpp_2::context::session::SeqState>,
}

struct Engine {
    // Field order matters: `ctx` borrows `model` and must be dropped first.
    ctx: LlamaContext<'static>,
    model: Box<LlamaModel>,
    model_path: String,
    template: Option<LlamaChatTemplate>,
    /// Model uses `<think>` reasoning blocks (Qwen3-style); we pre-fill an empty one to skip it.
    thinking_model: bool,
    cached: Option<CachedPrefix>,
    n_ctx: u32,
}

impl Engine {
    fn load(backend: &LlamaBackend, model_path: &str, threads: u32, context_tokens: u32) -> anyhow::Result<Engine> {
        if !Path::new(model_path).is_file() {
            bail!("model file not found: {model_path}");
        }
        let model = Box::new(load_model(model_path)?);
        // SAFETY: the model is heap-allocated and owned by the returned Engine, which drops
        // `ctx` (the only borrower) before `model`. The Box is never moved out or replaced.
        let model_ref: &'static LlamaModel = unsafe { &*(model.as_ref() as *const LlamaModel) };

        let threads = if threads == 0 { audian_common::default_threads() } else { threads } as i32;
        let n_ctx = context_tokens.clamp(1024, 32768);
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(BATCH as u32)
            .with_n_ubatch(UBATCH)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let ctx = model_ref.new_context(backend, params).map_err(|e| anyhow!("failed to create context: {e}"))?;

        let template = model.chat_template(None).ok();
        let raw_template = template.as_ref().and_then(|t| t.to_str().ok()).unwrap_or("");
        let thinking_model = raw_template.contains("<think>") || raw_template.contains("enable_thinking");
        log::info!(
            "model has {} params, chat template: {}, thinking model: {thinking_model}",
            model.n_params(),
            if template.is_some() { "yes" } else { "none (using ChatML)" }
        );
        Ok(Engine { ctx, model, model_path: model_path.into(), template, thinking_model, cached: None, n_ctx })
    }

    /// Formats `prefix + [user: SENTINEL]` and splits around the sentinel, giving the
    /// cacheable prefix text and the text that follows the user content.
    fn split_prompt(&self, prefix: &[ChatMessage]) -> anyhow::Result<(String, String)> {
        let mut chat = Vec::with_capacity(prefix.len() + 1);
        for m in prefix {
            chat.push(LlamaChatMessage::new(m.role.clone(), m.content.clone()).context("invalid message")?);
        }
        chat.push(LlamaChatMessage::new("user".into(), SENTINEL.into()).context("invalid message")?);

        let formatted = match &self.template {
            Some(t) => self.model.apply_chat_template(t, &chat, true).ok(),
            None => None,
        };
        let formatted = formatted.unwrap_or_else(|| chatml(prefix));
        let at = formatted.find(SENTINEL).ok_or_else(|| anyhow!("chat template dropped the user message"))?;
        let before = formatted[..at].to_string();
        let mut after = formatted[at + SENTINEL.len()..].to_string();
        if self.thinking_model && !after.contains("<think>") {
            after.push_str("<think>\n\n</think>\n\n");
        }
        Ok((before, after))
    }

    fn tokenize(&self, text: &str, add_bos: bool) -> Vec<LlamaToken> {
        self.model.vocab().tokenize(text.as_bytes(), add_bos, true)
    }

    fn decode_tokens(&mut self, tokens: &[LlamaToken], start_pos: i32, logits_last: bool) -> anyhow::Result<()> {
        let mut batch = LlamaBatch::new(BATCH, 1);
        let mut pos = start_pos;
        for (ci, chunk) in tokens.chunks(BATCH).enumerate() {
            batch.clear();
            let last_chunk = (ci + 1) * BATCH >= tokens.len();
            for (i, &tok) in chunk.iter().enumerate() {
                let is_last = last_chunk && i == chunk.len() - 1;
                batch.add(tok, pos, &[0], logits_last && is_last)?;
                pos += 1;
            }
            self.ctx.decode(&mut batch).map_err(|e| anyhow!("decode failed: {e}"))?;
        }
        Ok(())
    }

    /// Makes sure sequence 0 holds exactly the evaluated prefix. Returns its token count.
    fn ensure_prefix(&mut self, prefix: &[ChatMessage]) -> anyhow::Result<i32> {
        if let Some(cached) = &self.cached {
            if cached.key == prefix {
                if let Some(state) = &cached.state {
                    self.ctx.clear_kv_cache();
                    if self.ctx.state_seq_set(state, 0).is_ok() {
                        return Ok(cached.n_tokens);
                    }
                    log::warn!("restoring prefix snapshot failed; re-evaluating");
                }
            }
        }
        let t = Instant::now();
        let (prefix_text, _) = self.split_prompt(prefix)?;
        let tokens = self.tokenize(&prefix_text, true);
        let n_tokens = tokens.len() as i32;
        let file = self.prefix_cache_file(&tokens);
        let from_disk = self.load_prefix_file(&file, &tokens);
        if !from_disk {
            self.ctx.clear_kv_cache();
            self.decode_tokens(&tokens, 0, false)?;
            self.save_prefix_file(&file, &tokens);
        }
        let state = self.ctx.state_seq_get(0, LlamaStateSeqFlags::empty()).ok();
        log::info!(
            "{} {n_tokens}-token prefix in {} ms (snapshot: {})",
            if from_disk { "restored cached" } else { "evaluated" },
            t.elapsed().as_millis(),
            state.as_ref().map(|s| format!("{} KiB", s.byte_len() / 1024)).unwrap_or_else(|| "unsupported".into())
        );
        self.cached = Some(CachedPrefix { key: prefix.to_vec(), n_tokens, state });
        Ok(n_tokens)
    }

    /// Cache file for an evaluated prefix. The name covers everything the state depends on;
    /// the file itself also stores the tokens, which are compared on load.
    fn prefix_cache_file(&self, tokens: &[LlamaToken]) -> PathBuf {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut h);
        self.model_path.hash(&mut h);
        if let Ok(meta) = std::fs::metadata(&self.model_path) {
            meta.len().hash(&mut h);
            meta.modified().ok().hash(&mut h);
        }
        (self.n_ctx, UBATCH).hash(&mut h);
        tokens.hash(&mut h);
        audian_common::paths::cache_dir().join(format!("llm-prefix-{:016x}.bin", h.finish()))
    }

    fn load_prefix_file(&mut self, file: &Path, tokens: &[LlamaToken]) -> bool {
        if !file.is_file() {
            return false;
        }
        self.ctx.clear_kv_cache();
        match self.ctx.state_seq_load_file(file, 0, tokens.len()) {
            Ok((saved, _)) if saved == tokens => {
                // Mark as recently used for pruning.
                if let Ok(f) = std::fs::File::options().write(true).open(file) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                true
            }
            other => {
                log::warn!("ignoring prefix cache {} ({})", file.display(), if other.is_ok() { "different prompt" } else { "unreadable" });
                self.ctx.clear_kv_cache();
                let _ = std::fs::remove_file(file);
                false
            }
        }
    }

    fn save_prefix_file(&self, file: &Path, tokens: &[LlamaToken]) {
        let Some(dir) = file.parent() else { return };
        let tmp = file.with_extension("tmp");
        let saved = std::fs::create_dir_all(dir).is_ok()
            && self.ctx.state_seq_save_file(&tmp, 0, tokens).is_ok()
            && std::fs::rename(&tmp, file).is_ok();
        if !saved {
            log::warn!("could not cache the evaluated prefix in {}", dir.display());
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        // Keep only the most recently used prefixes.
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("llm-prefix-"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, old) in files.into_iter().skip(PREFIX_CACHE_FILES) {
            let _ = std::fs::remove_file(old);
        }
    }

    fn generate(&mut self, prefix: &[ChatMessage], user: &str, max_tokens: u32) -> anyhow::Result<(String, u32, u32)> {
        let n_prefix = self.ensure_prefix(prefix)?;
        let (_, after) = self.split_prompt(prefix)?;
        let user_tokens = self.tokenize(&format!("{user}{after}"), false);
        let available = self.n_ctx as i64 - n_prefix as i64 - user_tokens.len() as i64 - 8;
        if available < 16 {
            bail!("text is too long for the model context ({} tokens)", user_tokens.len());
        }
        let max_tokens = (max_tokens as i64).min(available) as usize;

        self.decode_tokens(&user_tokens, n_prefix, true)?;
        let mut pos = n_prefix + user_tokens.len() as i32;

        let mut sampler = LlamaSampler::greedy();
        let vocab = self.model.vocab();
        let mut out: Vec<u8> = Vec::new();
        let mut batch = LlamaBatch::new(1, 1);
        let mut logits_idx = (user_tokens.len() as i32 - 1) % BATCH as i32;
        let mut generated = 0u32;
        for _ in 0..max_tokens {
            let token = sampler.sample(&self.ctx, logits_idx);
            sampler.accept(token);
            if vocab.is_eog(token) {
                break;
            }
            out.extend_from_slice(&vocab.token_to_piece(token, false, None));
            generated += 1;
            batch.clear();
            batch.add(token, pos, &[0], true)?;
            self.ctx.decode(&mut batch).map_err(|e| anyhow!("decode failed: {e}"))?;
            pos += 1;
            logits_idx = 0;
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        Ok((strip_thinking(&text), user_tokens.len() as u32, generated))
    }
}

/// Loads a model with weight repacking disabled. llama.cpp normally copies most CPU weights
/// into a SIMD-friendly layout in private memory (~0.5 GB for a 2B model) on top of the
/// memory-mapped file; without it the mapped file pages are used directly.
fn load_model(model_path: &str) -> anyhow::Result<LlamaModel> {
    let path = std::ffi::CString::new(model_path)?;
    // SAFETY: plain FFI calls; the returned pointer is checked for null and owned by the
    // LlamaModel below, which frees it on drop.
    let raw = unsafe {
        let mut params = llama_cpp_sys_2::llama_model_default_params();
        params.use_extra_bufts = false;
        llama_cpp_sys_2::llama_model_load_from_file(path.as_ptr(), params)
    };
    let raw = std::ptr::NonNull::new(raw).ok_or_else(|| anyhow!("failed to load model"))?;
    // SAFETY: LlamaModel is #[repr(transparent)] over NonNull<llama_model>.
    Ok(unsafe { std::mem::transmute::<std::ptr::NonNull<llama_cpp_sys_2::llama_model>, LlamaModel>(raw) })
}

/// Fallback ChatML formatting (Qwen and many others) when the model has no usable template.
fn chatml(prefix: &[ChatMessage]) -> String {
    let mut s = String::new();
    for m in prefix {
        s.push_str(&format!("<|im_start|>{}\n{}<|im_end|>\n", m.role, m.content));
    }
    s.push_str(&format!("<|im_start|>user\n{SENTINEL}<|im_end|>\n<|im_start|>assistant\n"));
    s
}

/// Removes any `<think>...</think>` block a reasoning model may still emit.
fn strip_thinking(text: &str) -> String {
    let mut t = text.to_string();
    while let Some(start) = t.find("<think>") {
        match t[start..].find("</think>") {
            Some(end) => t.replace_range(start..start + end + "</think>".len(), ""),
            None => t.truncate(start),
        }
    }
    t.trim().to_string()
}

fn main() {
    audian_common::logging::init("audian-llm");
    let args: Vec<String> = std::env::args().collect();
    let mut backend = match LlamaBackend::init() {
        Ok(b) => b,
        Err(e) => {
            log::error!("llama backend init failed: {e}");
            std::process::exit(2);
        }
    };
    // llama.cpp's own logging (buffer sizes, layer offload, ...) goes to stderr when asked for.
    if std::env::var_os("AUDIAN_LLAMA_LOGS").is_none() {
        backend.void_logs();
    }

    if args.get(1).map(String::as_str) == Some("--bench") {
        std::process::exit(match bench(&backend, &args[2..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e:#}");
                1
            }
        });
    }

    let idle_secs: u64 = args
        .iter()
        .position(|a| a == "--idle-exit-secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let started = Instant::now();
    let last_activity = Arc::new(AtomicU64::new(0));
    {
        let last_activity = last_activity.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let idle = started.elapsed().as_secs().saturating_sub(last_activity.load(Ordering::Relaxed));
            if idle >= idle_secs {
                log::info!("idle for {idle}s, exiting to free memory");
                log::logger().flush();
                std::process::exit(0);
            }
        });
    }
    let touch = || last_activity.store(started.elapsed().as_secs(), Ordering::Relaxed);

    log::info!("started (pid {}, idle exit after {idle_secs}s)", std::process::id());
    let mut reader = BufReader::new(io::stdin().lock());
    let mut writer = BufWriter::new(io::stdout().lock());
    let mut engine: Option<Engine> = None;

    loop {
        let request: LlmRequest = match ipc::recv_json(&mut reader) {
            Ok(r) => r,
            Err(e) => {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    log::error!("reading request: {e}");
                }
                break;
            }
        };
        touch();
        let response = match request {
            LlmRequest::Load { model_path, threads, context_tokens } => {
                if engine.as_ref().is_some_and(|e| e.model_path == model_path) {
                    LlmResponse::Loaded { model: model_path, load_ms: 0 }
                } else {
                    engine = None;
                    let t = Instant::now();
                    match Engine::load(&backend, &model_path, threads, context_tokens) {
                        Ok(e) => {
                            let load_ms = t.elapsed().as_millis() as u64;
                            log::info!("loaded {model_path} in {load_ms} ms");
                            engine = Some(e);
                            LlmResponse::Loaded { model: model_path, load_ms }
                        }
                        Err(e) => {
                            log::error!("{e:#}");
                            LlmResponse::Error { id: None, message: format!("{e:#}") }
                        }
                    }
                }
            }
            LlmRequest::Prepare { prefix } => match engine.as_mut() {
                None => LlmResponse::Error { id: None, message: "no model loaded".into() },
                Some(engine) => {
                    let t = Instant::now();
                    match engine.ensure_prefix(&prefix) {
                        Ok(n) => LlmResponse::Prepared { prefix_tokens: n as u32, elapsed_ms: t.elapsed().as_millis() as u64 },
                        Err(e) => LlmResponse::Error { id: None, message: format!("{e:#}") },
                    }
                }
            },
            LlmRequest::Generate { id, prefix, user, max_tokens } => match engine.as_mut() {
                None => LlmResponse::Error { id: Some(id), message: "no model loaded".into() },
                Some(engine) => {
                    let t = Instant::now();
                    match engine.generate(&prefix, &user, max_tokens) {
                        Ok((text, prompt_tokens, generated_tokens)) => {
                            let elapsed_ms = t.elapsed().as_millis() as u64;
                            log::info!(
                                "generated {generated_tokens} tokens ({prompt_tokens} prompt) in {elapsed_ms} ms; {}",
                                audian_common::memory_summary()
                            );
                            LlmResponse::Output { id, text, prompt_tokens, generated_tokens, elapsed_ms }
                        }
                        Err(e) => {
                            log::error!("{e:#}");
                            // A failed decode can leave the sequence in an unknown state.
                            engine.cached = None;
                            LlmResponse::Error { id: Some(id), message: format!("{e:#}") }
                        }
                    }
                }
            },
            LlmRequest::Shutdown => break,
        };
        if let Err(e) = ipc::send_json(&mut writer, &response) {
            log::error!("writing response: {e}");
            break;
        }
        touch();
    }
    log::info!("exiting");
    log::logger().flush();
}

/// `audian-llm --bench <model> <system prompt file|-> <text>`: loads the model and runs a rewrite
/// twice (cold prefix, then cached prefix), printing timings.
fn bench(backend: &LlamaBackend, args: &[String]) -> anyhow::Result<()> {
    let model = args.first().ok_or_else(|| anyhow!("usage: --bench <model> <system-file|-> <text> [threads]"))?;
    let system = match args.get(1).map(String::as_str) {
        Some("-") | None => "Rewrite the user's dictated text so it is clear and grammatical. Output only the text.".to_string(),
        Some(path) => std::fs::read_to_string(path)?,
    };
    let text = args.get(2).cloned().unwrap_or_else(|| "um so basically I I want to like fix the the login bug".into());
    let threads: u32 = args.get(3).and_then(|t| t.parse().ok()).unwrap_or(0);
    let mut out = io::stdout().lock();
    let t = Instant::now();
    let mut engine = Engine::load(backend, &audian_common::paths::resolve_model(model).to_string_lossy(), threads, 4096)?;
    writeln!(out, "load: {} ms", t.elapsed().as_millis())?;
    let prefix = vec![ChatMessage::new("system", system)];
    for run in 1..=2 {
        let t = Instant::now();
        let (o, p, g) = engine.generate(&prefix, &format!("<transcript>\n{text}\n</transcript>"), 256)?;
        writeln!(out, "run {run}: {} ms ({p} prompt tokens, {g} generated)\n  {o}", t.elapsed().as_millis())?;
    }
    Ok(())
}
