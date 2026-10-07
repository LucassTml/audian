//! System tray icon, context menu and notifications (plain Shell_NotifyIcon, no toolkit).

use tiny_skia::Color;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCSTR, w};

use crate::win::{WideStr, copy_to_wide_buf};

pub const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Processing,
}

pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 3],
    state: TrayState,
    tooltip: String,
    added: bool,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let light_taskbar = system_uses_light_theme();
        let idle = if light_taskbar { Color::from_rgba8(32, 32, 36, 255) } else { Color::from_rgba8(245, 245, 247, 255) };
        let icons = [
            make_icon(idle),
            make_icon(Color::from_rgba8(255, 69, 58, 255)),
            make_icon(Color::from_rgba8(90, 160, 255, 255)),
        ];
        Tray { hwnd, icons, state: TrayState::Idle, tooltip: audian_common::APP_NAME.into(), added: false }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icons[self.state as usize],
            ..Default::default()
        };
        copy_to_wide_buf(&mut nid.szTip, &self.tooltip);
        nid
    }

    /// Adds the icon (also used to re-add it after Explorer restarts).
    pub fn add(&mut self) {
        unsafe {
            let mut nid = self.data();
            self.added = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        if !self.added {
            log::warn!("could not add tray icon (Explorer not ready?)");
        }
    }

    pub fn set_state(&mut self, state: TrayState, tooltip: &str) {
        if state == self.state && tooltip == self.tooltip {
            return;
        }
        self.state = state;
        self.tooltip = tooltip.to_string();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data());
        }
    }

    pub fn notify(&self, title: &str, body: &str, error: bool) {
        let mut nid = self.data();
        nid.uFlags |= NIF_INFO;
        copy_to_wide_buf(&mut nid.szInfoTitle, title);
        copy_to_wide_buf(&mut nid.szInfo, body);
        nid.dwInfoFlags = if error { NIIF_WARNING } else { NIIF_INFO } | NIIF_RESPECT_QUIET_TIME;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            for icon in self.icons {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

fn system_uses_light_theme() -> bool {
    let mut value = 0u32;
    let mut size = 4u32;
    let key = WideStr::new("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let name = WideStr::new("SystemUsesLightTheme");
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.pcwstr(),
            name.pcwstr(),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
        .is_ok()
            && value == 1
    }
}

/// Renders the mic glyph into an HICON at the system's small-icon size.
fn make_icon(color: Color) -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
        let pm = audian_art::draw_glyph(size, color);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color_bmp) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        };
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for (d, p) in dst.chunks_exact_mut(4).zip(pm.pixels()) {
            let c = p.demultiply();
            d[0] = c.blue();
            d[1] = c.green();
            d[2] = c.red();
            d[3] = c.alpha();
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let icon_info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color_bmp };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(color_bmp.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// Lets popup menus follow the system dark mode (uxtheme ordinal 135 `SetPreferredAppMode`,
/// the same undocumented-but-stable call used by Notepad++ and others). Best effort.
pub fn enable_dark_menus() {
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(set_mode) = GetProcAddress(lib, PCSTR(135 as *const u8)) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            set_mode(1); // AllowDark
        }
        if let Some(flush) = GetProcAddress(lib, PCSTR(136 as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

pub enum MenuItem {
    Action { id: u32, label: String, checked: bool, enabled: bool },
    Radio { id: u32, label: String, selected: bool },
    Submenu { label: String, items: Vec<MenuItem> },
    Separator,
}

impl MenuItem {
    pub fn action(id: u32, label: impl Into<String>) -> Self {
        MenuItem::Action { id, label: label.into(), checked: false, enabled: true }
    }
}

fn build_menu(items: &[MenuItem]) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu().unwrap_or_default();
        for item in items {
            match item {
                MenuItem::Action { id, label, checked, enabled } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let text = WideStr::new(label);
                    let _ = AppendMenuW(menu, flags, *id as usize, text.pcwstr());
                }
                MenuItem::Radio { id, label, selected } => {
                    let text = WideStr::new(label);
                    let _ = AppendMenuW(menu, MF_STRING | if *selected { MF_CHECKED } else { MF_UNCHECKED }, *id as usize, text.pcwstr());
                    if *selected {
                        let count = GetMenuItemCount(Some(menu));
                        let pos = (count - 1).max(0) as u32;
                        let _ = CheckMenuRadioItem(menu, pos, pos, pos, MF_BYPOSITION.0);
                    }
                }
                MenuItem::Submenu { label, items } => {
                    let sub = build_menu(items);
                    let text = WideStr::new(label);
                    let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, text.pcwstr());
                }
                MenuItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
            }
        }
        menu
    }
}

/// Shows a popup menu at the cursor and returns the chosen command id. Must be called while
/// no `App` borrow is held: the menu runs a modal message loop.
pub fn show_menu(hwnd: HWND, items: &[MenuItem]) -> Option<u32> {
    unsafe {
        let menu = build_menu(items);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let id = cmd.0 as u32;
        (id != 0).then_some(id)
    }
}
