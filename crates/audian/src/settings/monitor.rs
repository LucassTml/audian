//! Live resource usage of Audian's processes (shown on Home / About), refreshed every ~1.5 s
//! while the window is open. Demonstrates that the engines only run while needed.

use std::collections::HashMap;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
use windows::core::PCWSTR;

#[derive(Clone, Debug, Default)]
pub struct ProcStat {
    pub running: bool,
    pub memory_mb: f64,
    /// Percent of the whole machine (all cores).
    pub cpu_percent: f64,
}

#[derive(Default)]
pub struct Snapshot {
    pub tray: ProcStat,
    pub speech: ProcStat,
    pub rewrite: ProcStat,
    pub system_ram_gb: f64,
}

pub struct Monitor {
    last: Option<Instant>,
    prev: HashMap<u32, (u64, Instant)>,
    pub snapshot: Snapshot,
    cores: f64,
}

fn filetime(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

impl Monitor {
    pub fn new() -> Monitor {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        Monitor { last: None, prev: HashMap::new(), snapshot: Snapshot::default(), cores }
    }

    /// Refreshes if the last sample is older than 1.5 s.
    pub fn tick(&mut self) {
        if self.last.is_some_and(|t| t.elapsed().as_secs_f32() < 1.5) {
            return;
        }
        self.last = Some(Instant::now());
        let daemon_pid = unsafe {
            FindWindowW(crate::app::WINDOW_CLASS, PCWSTR::null())
                .ok()
                .map(|h| {
                    let mut pid = 0;
                    GetWindowThreadProcessId(h, Some(&mut pid));
                    pid
                })
                .unwrap_or(0)
        };
        let mut snap = Snapshot::default();
        unsafe {
            let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut status).is_ok() {
                snap.system_ram_gb = status.ullTotalPhys as f64 / 1e9;
            }
            let Ok(handle) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
            let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(handle, &mut entry).is_ok();
            while ok {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                let pid = entry.th32ProcessID;
                let slot = match name.as_str() {
                    "audian.exe" if pid == daemon_pid => Some(&mut snap.tray),
                    "audian-stt.exe" => Some(&mut snap.speech),
                    "audian-llm.exe" => Some(&mut snap.rewrite),
                    _ => None,
                };
                if let Some(slot) = slot {
                    if let Some(stat) = self.sample(pid) {
                        slot.running = true;
                        slot.memory_mb += stat.memory_mb;
                        slot.cpu_percent += stat.cpu_percent;
                    }
                }
                ok = Process32NextW(handle, &mut entry).is_ok();
            }
            let _ = CloseHandle(handle);
        }
        self.snapshot = snap;
    }

    fn sample(&mut self, pid: u32) -> Option<ProcStat> {
        unsafe {
            let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut counters = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
            let mem_ok = K32GetProcessMemoryInfo(h, &mut counters, counters.cb).as_bool();
            let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            let times_ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
            let _ = CloseHandle(h);
            let now = Instant::now();
            let busy = filetime(k) + filetime(u);
            let cpu = match (times_ok, self.prev.insert(pid, (busy, now))) {
                (true, Some((prev_busy, prev_t))) => {
                    let wall = now.duration_since(prev_t).as_secs_f64();
                    if wall > 0.0 { (busy.saturating_sub(prev_busy) as f64 / 1e7) / wall / self.cores * 100.0 } else { 0.0 }
                }
                _ => 0.0,
            };
            Some(ProcStat {
                running: true,
                memory_mb: if mem_ok { counters.WorkingSetSize as f64 / 1_048_576.0 } else { 0.0 },
                cpu_percent: cpu,
            })
        }
    }
}
