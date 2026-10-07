//! Inserting the final text into the application the user was typing in.
//!
//! Default strategy: put the text on the clipboard, send the paste shortcut, then restore the
//! previous clipboard contents a moment later. Pasting works across Win32, Electron (VS Code,
//! Discord), browsers, Office and terminals, and avoids the auto-complete / auto-indent /
//! bracket-closing side effects editors apply to simulated typing. Simulated Unicode typing
//! is available as an option and as the fallback when the clipboard is unavailable.

pub mod clipboard;
pub mod keyboard;
pub mod target;

use std::time::Duration;

use audian_common::config::{InsertMethod, InsertionConfig};
use windows::Win32::Foundation::HWND;

pub use target::Target;

#[derive(Debug)]
pub enum InsertOutcome {
    /// The text was inserted into the target app.
    Inserted,
    /// The text could not be inserted, but it was left on the clipboard. Holds the reason.
    CopiedOnly(String),
}

#[derive(Debug, thiserror::Error)]
pub enum InjectError {
    #[error("Could not insert the text: {0}")]
    Failed(String),
}

pub struct Injector {
    owner: HWND,
    pending_restore: Option<(clipboard::Snapshot, u32)>,
}

impl Injector {
    pub fn new(owner: HWND) -> Self {
        Injector { owner, pending_restore: None }
    }

    /// Inserts `text` into `target`. If this returns with a pending clipboard restore, the
    /// caller must call [`Injector::restore_clipboard`] after `restore_delay_ms`.
    pub fn insert(&mut self, text: &str, target: &Target, cfg: &InsertionConfig) -> Result<InsertOutcome, InjectError> {
        self.restore_clipboard(); // never stack restores

        if !target.is_alive() {
            return self.copy_only(text, "The original window was closed, so the text was copied to the clipboard.");
        }
        if target.elevated {
            return self.copy_only(
                text,
                &format!("{} is running as administrator, so Windows blocks typing into it. The text is on your clipboard — press Ctrl+V.", target.app_name()),
            );
        }
        if !target.activate() {
            return self.copy_only(
                text,
                &format!("Couldn't switch back to {}. The text is on your clipboard — press Ctrl+V.", target.app_name()),
            );
        }
        keyboard::wait_for_modifiers_released(Duration::from_millis(1500));

        match cfg.method {
            InsertMethod::Type => match keyboard::type_text(text) {
                Ok(()) => Ok(InsertOutcome::Inserted),
                Err(e) => {
                    log::warn!("typing failed ({e}); falling back to paste");
                    self.paste(text, target, cfg)
                }
            },
            InsertMethod::Paste => self.paste(text, target, cfg),
        }
    }

    fn paste(&mut self, text: &str, target: &Target, cfg: &InsertionConfig) -> Result<InsertOutcome, InjectError> {
        let snapshot = if cfg.restore_clipboard {
            match clipboard::snapshot(self.owner) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("could not save clipboard: {e}");
                    None
                }
            }
        } else {
            None
        };
        let seq = match clipboard::set_text(self.owner, text, cfg.restore_clipboard) {
            Ok(seq) => seq,
            Err(e) => {
                log::warn!("clipboard unavailable ({e}); typing instead");
                return keyboard::type_text(text).map(|_| InsertOutcome::Inserted).map_err(InjectError::Failed);
            }
        };
        let keys = if target.prefers_shift_insert() { keyboard::PasteKeys::ShiftInsert } else { keyboard::PasteKeys::CtrlV };
        if let Err(e) = keyboard::send_paste(keys) {
            // Leave the text on the clipboard so the user can paste manually.
            return Ok(InsertOutcome::CopiedOnly(format!("Couldn't paste automatically ({e}). The text is on your clipboard.")));
        }
        if let Some(snapshot) = snapshot {
            self.pending_restore = Some((snapshot, seq));
        }
        Ok(InsertOutcome::Inserted)
    }

    fn copy_only(&mut self, text: &str, reason: &str) -> Result<InsertOutcome, InjectError> {
        clipboard::set_text(self.owner, text, false).map_err(InjectError::Failed)?;
        Ok(InsertOutcome::CopiedOnly(reason.to_string()))
    }

    pub fn has_pending_restore(&self) -> bool {
        self.pending_restore.is_some()
    }

    /// Restores the clipboard saved before pasting, unless the clipboard changed since
    /// (meaning the user copied something new, which must not be overwritten).
    pub fn restore_clipboard(&mut self) {
        if let Some((snapshot, seq)) = self.pending_restore.take() {
            if clipboard::sequence_number() != seq {
                log::info!("clipboard changed since paste; not restoring");
                return;
            }
            if let Err(e) = clipboard::restore(self.owner, &snapshot) {
                log::warn!("could not restore clipboard: {e}");
            }
        }
    }
}
