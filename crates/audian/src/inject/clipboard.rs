//! Clipboard access with snapshot/restore, so pasting dictated text does not destroy whatever
//! the user had copied.

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};

use crate::win::WideStr;

const CF_TEXT: u32 = 1;
const CF_OEMTEXT: u32 = 7;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_DIBV5: u32 = 17;
/// Formats whose data is a GDI handle rather than global memory; they cannot be copied
/// byte-wise (and Windows re-synthesises the important ones, e.g. CF_BITMAP from CF_DIB).
const HANDLE_FORMATS: &[u32] = &[2, 3, 9, 14, 0x80, 0x82, 0x83, 0x8E];
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub struct Snapshot {
    items: Vec<(u32, Vec<u8>)>,
}

/// Holds the clipboard open; closes it on drop.
struct Open;

impl Open {
    fn new(owner: HWND) -> Result<Open, String> {
        // Another application may hold the clipboard briefly; retry for ~300 ms.
        for _ in 0..20 {
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Ok(Open);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err("the clipboard is in use by another application".into())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

pub fn snapshot(owner: HWND) -> Result<Snapshot, String> {
    let _open = Open::new(owner)?;
    let mut formats = Vec::new();
    let mut f = 0u32;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 {
            break;
        }
        formats.push(f);
    }
    let has = |x: u32| formats.contains(&x);
    let mut snap = Snapshot::default();
    let mut total = 0usize;
    for &format in &formats {
        let synthesized = ((format == CF_TEXT || format == CF_OEMTEXT) && has(CF_UNICODETEXT))
            || (format == CF_DIBV5 && has(CF_DIB));
        if synthesized || HANDLE_FORMATS.contains(&format) || (0x300..=0x3FF).contains(&format) {
            continue;
        }
        let Ok(handle) = (unsafe { GetClipboardData(format) }) else { continue };
        let hglobal = HGLOBAL(handle.0);
        unsafe {
            let size = GlobalSize(hglobal);
            if size == 0 || total + size > MAX_SNAPSHOT_BYTES {
                continue;
            }
            let ptr = GlobalLock(hglobal) as *const u8;
            if ptr.is_null() {
                continue;
            }
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            let _ = GlobalUnlock(hglobal);
            total += size;
            snap.items.push((format, bytes));
        }
    }
    Ok(snap)
}

fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("GlobalLock failed".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(mem);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(mem));
            return Err(e.to_string());
        }
        // On success the system owns the memory.
        Ok(())
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}

/// Places `text` on the clipboard. With `private`, the entry is marked so clipboard history,
/// cloud clipboard and clipboard managers ignore it (it is only there to be pasted).
/// Returns the clipboard sequence number after the change.
pub fn set_text(owner: HWND, text: &str, private: bool) -> Result<u32, String> {
    {
        let _open = Open::new(owner)?;
        unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
        put(CF_UNICODETEXT, &utf16z(text))?;
        if private {
            let zero = 0u32.to_le_bytes();
            for name in ["ExcludeClipboardContentFromMonitorProcessing", "CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
                let w = WideStr::new(name);
                let id = unsafe { RegisterClipboardFormatW(w.pcwstr()) };
                if id != 0 {
                    let _ = put(id, &zero);
                }
            }
        }
    }
    Ok(unsafe { GetClipboardSequenceNumber() })
}

pub fn restore(owner: HWND, snap: &Snapshot) -> Result<(), String> {
    let _open = Open::new(owner)?;
    unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
    for (format, bytes) in &snap.items {
        if let Err(e) = put(*format, bytes) {
            log::debug!("could not restore clipboard format {format}: {e}");
        }
    }
    Ok(())
}

pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}
