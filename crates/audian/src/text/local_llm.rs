//! Local LLM provider: a small GGUF model running in a helper process, on a dedicated graphics
//! card when there is one (`audian-llm-gpu`, Vulkan) and on the CPU otherwise (`audian-llm`).

use std::io::Read as _;
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use audian_common::config::{Config, LocalLlmConfig, ProcessingMode, RewriteProvider};
use audian_common::ipc::{ChatMessage, LlmRequest, LlmResponse};
use audian_common::paths;

use super::{ProcessError, RewriteRequest, TextProcessor, prompts, sanitize};
use crate::helper::{CREATE_NO_WINDOW, FramedHelper, HelperError};

const CONTEXT_TOKENS: u32 = 4096;
const CPU_EXE: &str = "audian-llm.exe";
const GPU_EXE: &str = "audian-llm-gpu.exe";

/// Set when the graphics card could not be used in this run (the GPU engine crashed, hung or
/// fell back to the CPU itself), so later dictations go straight to the CPU engine.
static GPU_OFF: AtomicBool = AtomicBool::new(false);

/// A dedicated graphics card found by `audian-llm-gpu --probe`.
pub struct Gpu {
    pub name: String,
    pub free_mb: u64,
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The dedicated graphics card the rewriting model can run on. Probed once per process (it
/// starts the Vulkan driver, ~0.3 s); the first caller waits for the result.
pub fn gpu() -> Option<&'static Gpu> {
    GPU.get_or_init(probe_gpu).as_ref()
}

/// Without waiting: `Some(card)` once probed, `None` while the probe (started here) runs.
pub fn gpu_status() -> Option<Option<&'static Gpu>> {
    if let Some(gpu) = GPU.get() {
        return Some(gpu.as_ref());
    }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            gpu();
        });
    }
    None
}

fn probe_gpu() -> Option<Gpu> {
    let exe = paths::exe_dir().join(GPU_EXE);
    // The GPU engine links the Vulkan loader, which graphics drivers install; without it the
    // executable cannot even start.
    let windows = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    if !exe.is_file() || !windows.join("System32\\vulkan-1.dll").is_file() {
        return None;
    }
    let mut child = Command::new(&exe)
        .arg("--probe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                log::warn!("graphics card probe did not finish");
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let (name, free) = out.lines().next()?.split_once('\t')?;
    let gpu = Gpu { name: name.trim().to_string(), free_mb: free.trim().parse().unwrap_or(0) };
    log::info!("graphics card for rewriting: {} ({} MB free)", gpu.name, gpu.free_mb);
    Some(gpu)
}

/// Whether the GPU engine should be used for `model_path`: a dedicated card with room for it.
fn use_gpu(enabled: bool, model_path: &str) -> bool {
    if !enabled || GPU_OFF.load(Ordering::Relaxed) {
        return false;
    }
    let model_mb = std::fs::metadata(model_path).map(|m| m.len() >> 20).unwrap_or(0);
    gpu().is_some_and(|g| g.free_mb >= model_mb + 600)
}

/// The graphics driver compiles its compute shaders the first time they run (several seconds,
/// then cached on disk by the driver), and the prompt prefix is evaluated and cached for the
/// card. Doing one rewrite in the background at startup spares the first real dictation both.
/// Runs once per app version, card and model.
pub fn prepare_gpu_in_background(cfg: &Config) {
    let llm = &cfg.processing.local_llm;
    if cfg.processing.provider != RewriteProvider::LocalLlm || !llm.gpu {
        return;
    }
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let cfg = cfg.clone();
    let spawned = std::thread::Builder::new().name("gpu-warm-up".into()).spawn(move || {
        prepare_gpu(&cfg);
        RUNNING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

fn prepare_gpu(cfg: &Config) {
    let llm = &cfg.processing.local_llm;
    let model_path = paths::resolve_model(&llm.model).to_string_lossy().into_owned();
    if !std::path::Path::new(&model_path).is_file() || !use_gpu(true, &model_path) {
        return;
    }
    let marker = paths::cache_dir().join("gpu-ready.txt");
    let stamp = format!("{}|{}|{}", audian_common::APP_VERSION, gpu().map_or("", |g| &g.name), llm.model);
    if std::fs::read_to_string(&marker).is_ok_and(|s| s == stamp) {
        return;
    }
    let t = Instant::now();
    let mut engine = LocalLlm::new(llm);
    let request = RewriteRequest {
        text: "Então, eu queria confirmar a reunião de amanhã às 10, quer dizer, às 11, pode ser?",
        language: "pt",
        output_language: &cfg.processing.output_language,
        mode: cfg.processing.mode,
        custom_instructions: &cfg.processing.custom_instructions,
    };
    match engine.process(&request) {
        Ok(_) if engine.on_gpu => {
            log::info!("graphics card prepared for rewriting in {} ms", t.elapsed().as_millis());
            let _ = std::fs::write(&marker, stamp);
        }
        Ok(_) => {}
        Err(e) => log::warn!("preparing the graphics card failed: {e}"),
    }
}

pub struct LocalLlm {
    model_path: String,
    threads: u32,
    gpu: bool,
    /// The running helper is the GPU engine.
    on_gpu: bool,
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
            gpu: cfg.gpu,
            on_gpu: false,
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
        self.on_gpu = use_gpu(self.gpu, &self.model_path);
        let exe = paths::exe_dir().join(if self.on_gpu { GPU_EXE } else { CPU_EXE });
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
                LlmResponse::Loaded { load_ms, device, .. } => {
                    self.loaded = true;
                    let on = if device.is_empty() { "the CPU" } else { device.as_str() };
                    log::info!("rewriting model ready on {on} (load {load_ms} ms)");
                    if self.on_gpu && device.is_empty() {
                        // The card was short on memory this time. The engine already runs on
                        // the CPU, so keep it, but start the leaner CPU build from now on.
                        GPU_OFF.store(true, Ordering::Relaxed);
                        self.on_gpu = false;
                    }
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
            Err(e @ (ProcessError::Engine(_) | ProcessError::Timeout)) if self.on_gpu => {
                // Graphics drivers can crash or hang; the CPU always works.
                log::warn!("the GPU rewriting engine failed ({e}); using the CPU from now on");
                GPU_OFF.store(true, Ordering::Relaxed);
                self.reset();
                self.try_process(request)
            }
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
