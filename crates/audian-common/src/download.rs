//! Model downloads, run only when the user asks for them (model manager, welcome screen or
//! installer). Uses the `curl.exe` that ships with Windows 10+ for HTTPS (resumable with
//! `-C -`), then verifies the size and SHA-256 before the file becomes visible to Audian.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::catalog::ModelInfo;
use crate::paths;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Downloading,
    Verifying,
    Done,
    Cancelled,
    Failed(String),
}

/// Handle to a background download; cheap to poll from a UI.
#[derive(Clone)]
pub struct Download {
    pub file: &'static str,
    pub total: u64,
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Download {
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.received() as f64 / self.total as f64).min(1.0) as f32 }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Failed("poisoned".into()))
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state(), State::Downloading | State::Verifying)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn curl() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    root.join("System32").join("curl.exe")
}

/// Starts downloading `model` into the models folder on a background thread.
pub fn start(model: &'static ModelInfo) -> Download {
    let dl = Download {
        file: model.file,
        total: model.size,
        received: Arc::new(AtomicU64::new(0)),
        cancel: Arc::new(AtomicBool::new(false)),
        state: Arc::new(Mutex::new(State::Downloading)),
    };
    let handle = dl.clone();
    std::thread::spawn(move || {
        let result = run(model, &handle);
        let state = match result {
            Ok(()) => State::Done,
            Err(_) if handle.cancel.load(Ordering::Relaxed) => State::Cancelled,
            Err(e) => State::Failed(e),
        };
        log::info!("download {}: {state:?}", model.file);
        if let Ok(mut s) = handle.state.lock() {
            *s = state;
        }
    });
    dl
}

fn run(model: &ModelInfo, dl: &Download) -> Result<(), String> {
    let dir = paths::models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create models folder: {e}"))?;
    if model.parts.is_empty() {
        let dest = dir.join(model.file);
        let part = dir.join(format!("{}.part", model.file));
        fetch(model.url, &part, model.size, model.sha256, dl, 0)?;
        return std::fs::rename(&part, &dest).map_err(|e| format!("cannot finalise download: {e}"));
    }

    // Multi-file model: download every file into "<folder>.part", then publish the folder.
    let staging = dir.join(format!("{}.part", model.file));
    std::fs::create_dir_all(&staging).map_err(|e| format!("cannot create download folder: {e}"))?;
    let mut done = 0;
    for part in model.parts {
        let dest = staging.join(part.file);
        let complete = std::fs::metadata(&dest).map(|m| m.len() == part.size).unwrap_or(false)
            && sha256_file(&dest).map(|d| d.eq_ignore_ascii_case(part.sha256)).unwrap_or(false);
        if !complete {
            let tmp = staging.join(format!("{}.download", part.file));
            fetch(part.url, &tmp, part.size, part.sha256, dl, done)?;
            std::fs::rename(&tmp, &dest).map_err(|e| format!("cannot finalise download: {e}"))?;
        }
        done += part.size;
    }
    let final_dir = dir.join(model.file);
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }
    std::fs::rename(&staging, &final_dir).map_err(|e| format!("cannot finalise download: {e}"))
}

/// Downloads `url` into `part` (resuming a previous partial file) and verifies its size and
/// SHA-256. Progress is reported as `base` plus the bytes of this file.
fn fetch(url: &str, part: &Path, size: u64, sha256: &str, dl: &Download, base: u64) -> Result<(), String> {
    if std::fs::metadata(part).map(|m| m.len() > size).unwrap_or(false) {
        let _ = std::fs::remove_file(part);
    }
    if let Ok(mut s) = dl.state.lock() {
        *s = State::Downloading;
    }
    log::info!("downloading {url}");
    let mut child = Command::new(curl())
        .args(["-L", "--fail", "--silent", "--show-error", "--retry", "3", "--retry-delay", "2", "-C", "-", "-o"])
        .arg(part)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("could not start the download (curl.exe): {e}"))?;
    loop {
        if let Ok(meta) = std::fs::metadata(part) {
            dl.received.store(base + meta.len(), Ordering::Relaxed);
        }
        if dl.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = e.read_to_string(&mut err);
                    }
                    let err = err.trim();
                    return Err(if err.is_empty() { format!("download failed ({status})") } else { err.to_string() });
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let len = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    dl.received.store(base + len, Ordering::Relaxed);
    if len != size {
        return Err(format!("incomplete download ({len} of {size} bytes)"));
    }
    if let Ok(mut s) = dl.state.lock() {
        *s = State::Verifying;
    }
    let digest = sha256_file(part).map_err(|e| format!("cannot verify download: {e}"))?;
    if !digest.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(part);
        return Err("the downloaded file is corrupted (checksum mismatch); please retry".into());
    }
    Ok(())
}

pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
