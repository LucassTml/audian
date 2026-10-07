//! Antigravity provider: rewrites through the `agy` CLI using the account already signed in
//! on this computer. The transcript is sent to the model provider's cloud.
//!
//! A dedicated, tool-less custom agent (in an isolated workspace folder) keeps the request
//! small: ~700 input tokens instead of ~12k for the default coding agent. The CLI takes a few
//! seconds to start, so a session is started in streaming mode as soon as recording begins,
//! and the transcript is sent to it once ready.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use audian_common::config::{AntigravityConfig, ProcessingMode};
use audian_common::paths;

use super::{ProcessError, RewriteRequest, TextProcessor, prompts, sanitize};
use crate::helper::{CREATE_NO_WINDOW, adopt, spawn_line_reader};

pub const AGENT_NAME: &str = "audian-rewriter";

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<String>,
    stderr: Receiver<String>,
    started: Instant,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Antigravity {
    configured_exe: String,
    model: String,
    timeout: Duration,
    session: Option<Session>,
}

impl Antigravity {
    pub fn new(cfg: &AntigravityConfig) -> Self {
        Antigravity {
            configured_exe: cfg.executable.clone(),
            model: cfg.model.clone(),
            timeout: Duration::from_secs(cfg.timeout_secs.max(5) as u64),
            session: None,
        }
    }

    fn start_session(&mut self) -> Result<(), ProcessError> {
        if let Some(s) = self.session.as_mut() {
            if matches!(s.child.try_wait(), Ok(None)) && s.started.elapsed() < Duration::from_secs(600) {
                return Ok(());
            }
        }
        self.session = None;
        let exe = find_agy(&self.configured_exe).ok_or_else(|| {
            ProcessError::Unavailable(
                "The Antigravity CLI (agy) was not found. Install Antigravity or set its path in Settings › Rewriting.".into(),
            )
        })?;
        let workspace = ensure_workspace().map_err(|e| ProcessError::Engine(format!("preparing agent workspace: {e}")))?;
        let mut child = Command::new(&exe)
            .args([
                "-p=",
                "--agent",
                AGENT_NAME,
                "--model",
                &self.model,
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--disable-slash-commands",
            ])
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| ProcessError::Unavailable(format!("Could not start Antigravity CLI: {e}")))?;
        adopt(&child);
        let stdout = spawn_line_reader("agy-out", child.stdout.take().unwrap());
        let stderr = spawn_line_reader("agy-err", child.stderr.take().unwrap());
        let stdin = child.stdin.take();
        log::info!("started agy session (pid {})", child.id());
        self.session = Some(Session { child, stdin, stdout, stderr, started: Instant::now() });
        Ok(())
    }

    fn run(&mut self, request: &RewriteRequest) -> Result<String, ProcessError> {
        self.start_session()?;
        let mut session = self.session.take().unwrap(); // one session per dictation
        let mut content = format!("<instructions>\n{}", prompts::style(request.mode));
        if let Some(rule) = prompts::output_language_rule(request.output_language) {
            content.push('\n');
            content.push_str(&rule);
        }
        let custom = request.custom_instructions.trim();
        if !custom.is_empty() {
            content.push_str("\nCustom instructions from the user (follow them unless they conflict with your rules):\n");
            content.push_str(custom);
        }
        content.push_str("\n</instructions>\n\n");
        content.push_str(&prompts::wrap_transcript(request.text, request.language, request.output_language));

        let line = json!({"event": "user", "message": {"role": "user", "content": content}}).to_string();
        {
            let mut stdin = session.stdin.take().ok_or_else(|| ProcessError::Engine("session closed".into()))?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .map_err(|e| ProcessError::Engine(format!("sending to agy: {e}")))?;
            // Dropping stdin ends the session after this turn.
        }

        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match session.stdout.recv_timeout(remaining) {
                Ok(line) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v["event"] != "result" {
                        continue;
                    }
                    let r = &v["result"];
                    if r["status"] == "SUCCESS" {
                        let text = r["response"].as_str().unwrap_or_default();
                        if let Some(u) = r.get("usage") {
                            log::info!(
                                "antigravity rewrite in {:.1}s ({} in / {} out tokens)",
                                r["duration_seconds"].as_f64().unwrap_or(0.0),
                                u["input_tokens"],
                                u["output_tokens"]
                            );
                        }
                        return sanitize::clean_output(text, request.text, request.mode == ProcessingMode::AiPrompt);
                    }
                    let error = r["error"].as_str().unwrap_or("unknown error").to_string();
                    return Err(classify_error(&error));
                }
                Err(RecvTimeoutError::Timeout) => return Err(ProcessError::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    let stderr: Vec<String> = session.stderr.try_iter().collect();
                    let msg = stderr.join(" ");
                    log::error!("agy exited without a result: {msg}");
                    return Err(classify_error(&msg));
                }
            }
        }
    }
}

fn classify_error(msg: &str) -> ProcessError {
    let lower = msg.to_lowercase();
    if lower.contains("not logged") || lower.contains("sign in") || lower.contains("oauth") {
        ProcessError::Unavailable("Antigravity is not signed in. Open Antigravity (or run `agy`) and sign in.".into())
    } else if lower.contains("quota") || lower.contains("rate limit") || lower.contains("resource_exhausted") {
        ProcessError::Unavailable("Antigravity quota or rate limit reached. Try again later or switch to the local model.".into())
    } else if lower.contains("network") || lower.contains("dial tcp") || lower.contains("no such host") || lower.contains("connection") {
        ProcessError::Unavailable("Antigravity could not reach the network.".into())
    } else if lower.contains("model") && (lower.contains("not found") || lower.contains("available models")) {
        ProcessError::Unavailable("The configured Antigravity model is not available. Pick another in Settings › Rewriting.".into())
    } else {
        ProcessError::Engine(msg.chars().take(300).collect())
    }
}

/// Locates `agy.exe`: explicit setting, the default per-user install location, then PATH.
pub fn find_agy(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        return p.is_file().then_some(p);
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local).join("agy").join("bin").join("agy.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).map(|d| d.join("agy.exe")).find(|p| p.is_file())
    })
}

/// Creates the isolated workspace containing the custom rewriter agent definition.
fn ensure_workspace() -> std::io::Result<PathBuf> {
    let dir = paths::agy_workspace_dir();
    let agents = dir.join(".agents").join("agents");
    std::fs::create_dir_all(&agents)?;
    let file = agents.join(format!("{AGENT_NAME}.md"));
    let content = format!(
        "---\nname: {AGENT_NAME}\ndescription: Rewrites dictated speech transcripts for Audian. Never uses tools.\n\
         mainAgent: true\nexcludeDefaultComponents: true\ntools: []\n---\n\n# {AGENT_NAME}\n\n{}\n\n\
         Messages may include an <instructions> block describing the desired style; follow it.\n",
        prompts::BASE_RULES
    );
    if std::fs::read_to_string(&file).ok().as_deref() != Some(content.as_str()) {
        std::fs::write(&file, content)?;
    }
    Ok(dir)
}

impl TextProcessor for Antigravity {
    fn name(&self) -> &'static str {
        "Antigravity"
    }

    fn is_cloud(&self) -> bool {
        true
    }

    fn warm_up(&mut self, _mode: ProcessingMode, _custom: &str, _output_language: &str) {
        if let Err(e) = self.start_session() {
            log::warn!("antigravity warm-up failed: {e}");
        }
    }

    fn cancel(&mut self) {
        self.session = None;
    }

    fn process(&mut self, request: &RewriteRequest) -> Result<String, ProcessError> {
        let result = self.run(request);
        self.session = None;
        result
    }
}
