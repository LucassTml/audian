//! Global shortcut parsing and registration.
//!
//! `RegisterHotKey` is used rather than a low-level keyboard hook: it costs nothing per
//! keystroke, works while elevated windows are focused, and reports conflicts with other
//! applications directly. It only signals key-down, so push-to-talk release is detected by
//! polling the key state while (and only while) a recording is active.

use std::fmt;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("\"{0}\" is not a recognised key")]
    UnknownKey(String),
    #[error("the shortcut needs a main key (e.g. Space or a letter)")]
    MissingKey,
    #[error("the shortcut has more than one main key")]
    MultipleKeys,
    #[error("{0} is already used by another application")]
    InUse(String),
    #[error("Windows rejected the shortcut {0}: {1}")]
    Rejected(String, String),
}

const KEY_NAMES: &[(&str, u16)] = &[
    ("Space", 0x20),
    ("Enter", 0x0D),
    ("Tab", 0x09),
    ("Backspace", 0x08),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Up", 0x26),
    ("Down", 0x28),
    ("Left", 0x25),
    ("Right", 0x27),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("CapsLock", 0x14),
    ("Escape", 0x1B),
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    ("[", 0xDB),
    ("]", 0xDD),
    ("\\", 0xDC),
    (";", 0xBA),
    ("'", 0xDE),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
];

impl Hotkey {
    pub fn parse(s: &str) -> Result<Hotkey, HotkeyError> {
        let mut hk = Hotkey { ctrl: false, alt: false, shift: false, win: false, vk: 0 };
        for raw in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            let lower = raw.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => hk.ctrl = true,
                "alt" => hk.alt = true,
                "shift" => hk.shift = true,
                "win" | "windows" | "super" | "meta" | "cmd" => hk.win = true,
                _ => {
                    let vk = key_from_name(raw).ok_or_else(|| HotkeyError::UnknownKey(raw.to_string()))?;
                    if hk.vk != 0 {
                        return Err(HotkeyError::MultipleKeys);
                    }
                    hk.vk = vk;
                }
            }
        }
        // "Ctrl++" style: a trailing '+' means the plus key.
        if hk.vk == 0 && s.trim_end().ends_with("++") {
            hk.vk = 0xBB;
        }
        if hk.vk == 0 {
            return Err(HotkeyError::MissingKey);
        }
        Ok(hk)
    }

    pub fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT;
        if self.ctrl {
            m |= MOD_CONTROL;
        }
        if self.alt {
            m |= MOD_ALT;
        }
        if self.shift {
            m |= MOD_SHIFT;
        }
        if self.win {
            m |= MOD_WIN;
        }
        m
    }

    pub fn key_is_down(&self) -> bool {
        key_down(self.vk)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.win {
            parts.push("Win".into());
        }
        parts.push(key_name(self.vk));
        write!(f, "{}", parts.join("+"))
    }
}

pub fn key_from_name(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = upper.strip_prefix("NUMPAD").and_then(|n| n.parse::<u16>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    KEY_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name) || (name == "Esc" && *n == "Escape"))
        .map(|&(_, vk)| vk)
}

pub fn key_name(vk: u16) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x60..=0x69 => format!("Numpad{}", vk - 0x60),
        _ => KEY_NAMES
            .iter()
            .find(|&&(_, v)| v == vk)
            .map(|(n, _)| n.to_string())
            .unwrap_or_else(|| format!("VK{vk:#04x}")),
    }
}

pub fn key_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

pub fn any_modifier_down() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].iter().any(|vk| key_down(vk.0))
}

pub fn register(hwnd: HWND, id: i32, hotkey: &Hotkey) -> Result<(), HotkeyError> {
    unsafe {
        RegisterHotKey(Some(hwnd), id, hotkey.modifiers(), hotkey.vk as u32).map_err(|e| {
            // ERROR_HOTKEY_ALREADY_REGISTERED = 1409
            if e.code().0 as u32 & 0xFFFF == 1409 {
                HotkeyError::InUse(hotkey.to_string())
            } else {
                HotkeyError::Rejected(hotkey.to_string(), e.message())
            }
        })
    }
}

pub fn unregister(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

/// Checks whether a shortcut could be registered right now (registers and releases it,
/// thread-bound, so no window is needed).
pub fn probe(hotkey: &Hotkey) -> Result<(), HotkeyError> {
    const PROBE_ID: i32 = 0x7FF0;
    unsafe {
        RegisterHotKey(None, PROBE_ID, hotkey.modifiers(), hotkey.vk as u32).map_err(|e| {
            if e.code().0 as u32 & 0xFFFF == 1409 {
                HotkeyError::InUse(hotkey.to_string())
            } else {
                HotkeyError::Rejected(hotkey.to_string(), e.message())
            }
        })?;
        let _ = UnregisterHotKey(None, PROBE_ID);
    }
    Ok(())
}

/// Main keys offered in the shortcut editor.
pub fn selectable_keys() -> Vec<String> {
    let mut keys: Vec<String> = ["Space", "Enter", "Tab", "Insert", "Delete", "Home", "End", "PageUp", "PageDown", "Pause", "ScrollLock"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    keys.extend((b'A'..=b'Z').map(|c| (c as char).to_string()));
    keys.extend((b'0'..=b'9').map(|c| (c as char).to_string()));
    keys.extend((1..=24).map(|n| format!("F{n}")));
    keys.extend(["`", "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/"].iter().map(|s| s.to_string()));
    keys
}

/// When a shortcut includes Alt or Win, releasing that modifier on its own would open the
/// app's menu bar or the Start menu. Injecting an unassigned key while the modifier is held
/// (the same trick AutoHotkey uses) prevents that.
pub fn suppress_modifier_side_effects(hotkey: &Hotkey) {
    if !(hotkey.alt || hotkey.win) {
        return;
    }
    const VK_UNASSIGNED: u16 = 0xE8;
    let inputs = [key_input(VK_UNASSIGNED, false), key_input(VK_UNASSIGNED, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn key_input(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    if matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let hk = Hotkey::parse("ctrl + shift + space").unwrap();
        assert!(hk.ctrl && hk.shift && !hk.alt && !hk.win);
        assert_eq!(hk.vk, 0x20);
        assert_eq!(hk.to_string(), "Ctrl+Shift+Space");
        assert_eq!(Hotkey::parse("Win+Shift+V").unwrap().to_string(), "Shift+Win+V");
        assert_eq!(Hotkey::parse("Alt+F13").unwrap().vk, 0x7C);
        assert!(matches!(Hotkey::parse("Ctrl+Shift"), Err(HotkeyError::MissingKey)));
        assert!(matches!(Hotkey::parse("Ctrl+Foo"), Err(HotkeyError::UnknownKey(_))));
    }
}
