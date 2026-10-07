//! The window that had focus when dictation started, and how to get back to it.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::win::{self, OwnedHandle};

#[derive(Clone, Debug)]
pub struct Target {
    /// Top-level window that was in the foreground.
    pub hwnd: HWND,
    /// Text caret in screen coordinates, if the app exposes a system caret.
    pub caret: Option<RECT>,
    pub process_name: String,
    /// Window title — used only to match app profiles (e.g. "ChatGPT" in a browser); never logged.
    pub title: String,
    pub class: String,
    pub is_terminal: bool,
    /// The target runs with higher privileges than us; Windows blocks simulated input to it.
    pub elevated: bool,
}

// HWND is a plain handle value; Target is only used on the UI thread, but the pipeline job
// carries a copy of the non-handle fields across threads.
unsafe impl Send for Target {}

// Console windows (cmd, PowerShell) are recognised by window class instead of process name:
// shells can also host GUI windows (e.g. WinForms scripts), which must not be treated as
// terminals.
const TERMINAL_PROCESSES: &[&str] = &[
    "windowsterminal.exe",
    "mintty.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
    "putty.exe",
    "kitty.exe",
    "hyper.exe",
    "tabby.exe",
    "conemu64.exe",
    "conemu.exe",
];
const TERMINAL_CLASSES: &[&str] = &["CASCADIA_HOSTING_WINDOW_CLASS", "ConsoleWindowClass", "mintty", "PuTTY", "VirtualConsoleClass"];

impl Target {
    pub fn capture() -> Option<Target> {
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_invalid() {
            return None;
        }
        let hwnd = win::root_window(fg);
        let process_name = win::window_process_name(hwnd).unwrap_or_default();
        let class = win::window_class(hwnd);
        let title = win::window_title(hwnd);
        let lower = process_name.to_lowercase();
        let is_terminal =
            TERMINAL_PROCESSES.contains(&lower.as_str()) || TERMINAL_CLASSES.contains(&class.as_str());
        let elevated = is_more_privileged(win::window_process_id(hwnd));
        let caret = caret_rect(hwnd);
        log::info!(
            "target: {process_name} (class {class}){}{}",
            if is_terminal { ", terminal" } else { "" },
            if elevated { ", elevated" } else { "" }
        );
        Some(Target { hwnd, caret, process_name, title, class, is_terminal, elevated })
    }

    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(Some(self.hwnd)).as_bool() }
    }

    pub fn is_foreground(&self) -> bool {
        let fg = unsafe { GetForegroundWindow() };
        !fg.is_invalid() && win::root_window(fg) == self.hwnd
    }

    /// Brings the target back to the foreground if the user switched away while we worked.
    pub fn activate(&self) -> bool {
        if self.is_foreground() {
            return true;
        }
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            if !SetForegroundWindow(self.hwnd).as_bool() {
                // Foreground changes are restricted; temporarily sharing input state with the
                // current foreground thread is the documented way to be allowed to switch.
                let fg = GetForegroundWindow();
                let fg_thread = GetWindowThreadProcessId(fg, None);
                let me = GetCurrentThreadId();
                if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
                    let _ = BringWindowToTop(self.hwnd);
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = AttachThreadInput(me, fg_thread, false);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            if self.is_foreground() {
                std::thread::sleep(Duration::from_millis(40)); // let the app restore its focus
                return true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        false
    }

    /// A friendly app name for messages ("Code", "WindowsTerminal").
    pub fn app_name(&self) -> String {
        let name = self.process_name.trim_end_matches(".exe").trim_end_matches(".EXE");
        if name.is_empty() { "the original app".into() } else { name.to_string() }
    }

    /// mintty and PuTTY don't paste on Ctrl+V; Shift+Insert works there.
    pub fn prefers_shift_insert(&self) -> bool {
        matches!(self.class.as_str(), "mintty" | "PuTTY")
    }
}

fn caret_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd, None);
        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        if info.hwndCaret.is_invalid() {
            return None;
        }
        let r = info.rcCaret;
        if r.right <= r.left && r.bottom <= r.top {
            return None;
        }
        let mut tl = POINT { x: r.left, y: r.top };
        let mut br = POINT { x: r.right.max(r.left + 1), y: r.bottom };
        if !ClientToScreen(info.hwndCaret, &mut tl).as_bool() || !ClientToScreen(info.hwndCaret, &mut br).as_bool() {
            return None;
        }
        Some(RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y })
    }
}

fn integrity_level(process: HANDLE) -> Option<u32> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let token = OwnedHandle(token);
        let mut len = 0u32;
        let _ = GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        GetTokenInformation(token.0, TokenIntegrityLevel, Some(buf.as_mut_ptr().cast()), len, &mut len).ok()?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        Some(*GetSidSubAuthority(label.Label.Sid, count as u32 - 1))
    }
}

/// True if `pid` runs at a higher integrity level than this process (e.g. "Run as administrator").
fn is_more_privileged(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Some(ours) = integrity_level(unsafe { GetCurrentProcess() }) else { return false };
    let theirs = unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let h = OwnedHandle(h);
                integrity_level(h.0)
            }
            Err(_) => None,
        }
    };
    match theirs {
        Some(level) => level > ours,
        // We could open the process but not its token: almost always an elevated process.
        None => false,
    }
}
