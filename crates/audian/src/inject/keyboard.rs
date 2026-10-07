//! Simulated keyboard input via `SendInput`.

use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::hotkey::{any_modifier_down, key_input};

#[derive(Clone, Copy, Debug)]
pub enum PasteKeys {
    CtrlV,
    ShiftInsert,
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!("input was blocked ({} of {} events sent)", sent, inputs.len()))
    }
}

/// Waits (up to `max`) for the user to release modifier keys, so e.g. a still-held Alt from
/// the shortcut does not turn our Ctrl+V into Ctrl+Alt+V.
pub fn wait_for_modifiers_released(max: Duration) {
    let deadline = Instant::now() + max;
    while any_modifier_down() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn send_paste(keys: PasteKeys) -> Result<(), String> {
    let (modifier, key) = match keys {
        PasteKeys::CtrlV => (VK_CONTROL.0, 0x56u16),
        PasteKeys::ShiftInsert => (VK_SHIFT.0, VK_INSERT.0),
    };
    send(&[key_input(modifier, false), key_input(key, false), key_input(key, true), key_input(modifier, true)])
}

/// Types `text` as Unicode keystrokes. Newlines become Shift+Enter so chat apps (where Enter
/// sends the message) receive a line break instead.
pub fn type_text(text: &str) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(64);
    let flush = |inputs: &mut Vec<INPUT>| -> Result<(), String> {
        if !inputs.is_empty() {
            send(inputs)?;
            inputs.clear();
            std::thread::sleep(Duration::from_millis(2)); // let slow apps keep up
        }
        Ok(())
    };
    for c in text.chars() {
        match c {
            '\r' => continue,
            '\n' => {
                inputs.push(key_input(VK_SHIFT.0, false));
                inputs.push(key_input(VK_RETURN.0, false));
                inputs.push(key_input(VK_RETURN.0, true));
                inputs.push(key_input(VK_SHIFT.0, true));
            }
            '\t' => {
                inputs.push(key_input(VK_TAB.0, false));
                inputs.push(key_input(VK_TAB.0, true));
            }
            _ => {
                let mut buf = [0u16; 2];
                for &unit in c.encode_utf16(&mut buf).iter() {
                    for up in [false, true] {
                        let mut flags = KEYEVENTF_UNICODE;
                        if up {
                            flags |= KEYEVENTF_KEYUP;
                        }
                        inputs.push(INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                            },
                        });
                    }
                }
            }
        }
        if inputs.len() >= 64 {
            flush(&mut inputs)?;
        }
    }
    flush(&mut inputs)
}
