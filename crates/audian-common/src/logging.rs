//! Minimal file logger: one log file per process in `%LOCALAPPDATA%\Audian\logs`,
//! rotated once it exceeds a size limit. Kept deliberately small instead of pulling in a
//! full logging framework.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::paths;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct FileLogger {
    level: LevelFilter,
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl FileLogger {
    fn open(path: &PathBuf) -> Option<File> {
        if std::fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(path, path.with_extension("old.log"));
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Native inference libraries are chatty; only keep their warnings.
        let target = metadata.target();
        if target.starts_with("whisper_rs") || target.starts_with("llama") {
            return metadata.level() <= Level::Warn;
        }
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let t = unsafe { GetLocalTime() };
        let line = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {:<5} [{}] {}\n",
            t.wYear,
            t.wMonth,
            t.wDay,
            t.wHour,
            t.wMinute,
            t.wSecond,
            t.wMilliseconds,
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut guard) = self.file.lock() {
            if guard.is_none() {
                *guard = Self::open(&self.path);
            }
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
                if f.metadata().map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
                    *guard = None; // re-open (and rotate) on next write
                }
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Installs the logger for this process. `name` becomes the log file name.
/// Also installs a panic hook so crashes leave a trace in the log.
pub fn init(name: &str) -> PathBuf {
    let dir = paths::logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.log"));
    let level = match std::env::var("AUDIAN_LOG").ok().as_deref() {
        Some("trace") => LevelFilter::Trace,
        Some("debug") => LevelFilter::Debug,
        Some("warn") => LevelFilter::Warn,
        _ => LevelFilter::Info,
    };
    let logger = FileLogger { level, file: Mutex::new(FileLogger::open(&path)), path: path.clone() };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("panic in thread '{}': {info}", thread.name().unwrap_or("?"));
        log::logger().flush();
        default_hook(info);
    }));
    path
}
