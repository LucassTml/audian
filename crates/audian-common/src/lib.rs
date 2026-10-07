//! Code shared by the Audian daemon and its helper processes:
//! configuration schema, filesystem locations, IPC framing and logging.

pub mod catalog;
pub mod config;
pub mod download;
pub mod history;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod theme;
pub mod wav;

/// Sensible default thread count for CPU inference: half the logical processors (roughly the
/// performance cores on hybrid CPUs), capped at 8 where returns diminish.
pub fn default_threads() -> u32 {
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as u32;
    (logical / 2).clamp(1, 8)
}

/// One-line summary of this process's memory use, for the logs: working set (resident pages,
/// including shared/mapped model files), its peak, and private committed memory.
pub fn memory_summary() -> String {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    c.cb = size;
    // SAFETY: `c` is a correctly sized PROCESS_MEMORY_COUNTERS_EX, which extends PROCESS_MEMORY_COUNTERS.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, size) };
    if !ok.as_bool() {
        return "memory: unavailable".into();
    }
    const MB: usize = 1024 * 1024;
    format!(
        "memory: working set {} MB (peak {} MB), private {} MB",
        c.WorkingSetSize / MB,
        c.PeakWorkingSetSize / MB,
        c.PrivateUsage / MB
    )
}

pub const APP_NAME: &str = "Audian";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sample rate of all audio exchanged between components (what Whisper expects).
pub const SAMPLE_RATE: u32 = 16_000;
