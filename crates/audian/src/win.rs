//! Small Win32 helpers shared across modules.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetWindowThreadProcessId, PostMessageW,
};
use windows::core::{PCWSTR, PWSTR};

/// Null-terminated UTF-16 string kept alive for the duration of a Win32 call.
pub struct WideStr(Vec<u16>);

impl WideStr {
    pub fn new(s: impl AsRef<OsStr>) -> Self {
        WideStr(s.as_ref().encode_wide().chain(std::iter::once(0)).collect())
    }

    pub fn pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

/// Copies `s` into a fixed-size UTF-16 buffer (e.g. NOTIFYICONDATAW fields), truncating safely.
pub fn copy_to_wide_buf(buf: &mut [u16], s: &str) {
    let mut i = 0;
    for u in s.encode_utf16() {
        if i + 1 >= buf.len() {
            break;
        }
        buf[i] = u;
        i += 1;
    }
    buf[i] = 0;
}

/// HWNDs are raw pointers and therefore not `Send`; this wrapper lets worker threads hold one
/// to post messages back to the UI thread (PostMessage is thread-safe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

impl SendHwnd {
    pub fn new(hwnd: HWND) -> Self {
        SendHwnd(hwnd.0 as isize)
    }

    pub fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    pub fn post(self, msg: u32, wparam: usize, lparam: isize) -> bool {
        unsafe { PostMessageW(Some(self.hwnd()), msg, WPARAM(wparam), LPARAM(lparam)).is_ok() }
    }
}

pub fn root_window(hwnd: HWND) -> HWND {
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if root.is_invalid() { hwnd } else { root }
}

pub fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_process_id(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Full path of the executable that owns `hwnd`, if accessible.
pub fn window_process_path(hwnd: HWND) -> Option<String> {
    let pid = window_process_id(hwnd);
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OwnedHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Executable file name (e.g. `Code.exe`) of the process owning `hwnd`.
pub fn window_process_name(hwnd: HWND) -> Option<String> {
    window_process_path(hwnd).and_then(|p| p.rsplit(['\\', '/']).next().map(str::to_string))
}

pub const fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}
