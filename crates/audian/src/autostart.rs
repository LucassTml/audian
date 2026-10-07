//! "Start with Windows" via the per-user Run registry key (no admin rights needed).

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

use crate::win::WideStr;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "Audian";

fn command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\" --background")
}

pub fn is_enabled() -> bool {
    let key = WideStr::new(RUN_KEY);
    let name = WideStr::new(VALUE_NAME);
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.pcwstr(),
            name.pcwstr(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    } == ERROR_SUCCESS;
    if !ok {
        return false;
    }
    let len = (size as usize / 2).saturating_sub(1);
    String::from_utf16_lossy(&buf[..len]).eq_ignore_ascii_case(&command())
}

pub fn set(enabled: bool) -> Result<(), String> {
    unsafe {
        let key = WideStr::new(RUN_KEY);
        let mut hkey = HKEY::default();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, key.pcwstr(), None, KEY_SET_VALUE, &mut hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("cannot open Run key ({})", r.0));
        }
        let name = WideStr::new(VALUE_NAME);
        let result = if enabled {
            let data: Vec<u8> = command()
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            RegSetValueExW(hkey, name.pcwstr(), None, REG_SZ, Some(&data))
        } else {
            match RegDeleteValueW(hkey, name.pcwstr()) {
                r if r.0 == 2 => ERROR_SUCCESS, // ERROR_FILE_NOT_FOUND: already absent
                r => r,
            }
        };
        let _ = RegCloseKey(hkey);
        if result == ERROR_SUCCESS { Ok(()) } else { Err(format!("registry error {}", result.0)) }
    }
}

/// Makes the registry match the configuration (e.g. after the exe was moved).
pub fn sync(enabled: bool) {
    if is_enabled() != enabled {
        if let Err(e) = set(enabled) {
            log::warn!("could not update autostart: {e}");
        }
    }
}
