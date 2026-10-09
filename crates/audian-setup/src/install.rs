//! The actual install / uninstall work (no UI), so it can run headless (`--quiet`) too.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, LPARAM, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};
use windows::core::{HSTRING, Interface, PCWSTR, w};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Audian";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const UNINSTALLER: &str = "uninstall.exe";

#[cfg(not(no_payload))]
const PAYLOAD: &[(&str, &[u8])] = &[
    ("audian.exe", include_bytes!(concat!(env!("OUT_DIR"), "/audian.exe.z"))),
    ("audian-stt.exe", include_bytes!(concat!(env!("OUT_DIR"), "/audian-stt.exe.z"))),
    ("audian-parakeet.exe", include_bytes!(concat!(env!("OUT_DIR"), "/audian-parakeet.exe.z"))),
    ("audian-llm.exe", include_bytes!(concat!(env!("OUT_DIR"), "/audian-llm.exe.z"))),
];
#[cfg(no_payload)]
const PAYLOAD: &[(&str, &[u8])] = &[];

#[derive(Clone, Debug)]
pub struct Options {
    pub dir: PathBuf,
    pub autostart: bool,
    pub desktop_shortcut: bool,
}

pub fn default_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Programs").join("Audian")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepState {
    Pending,
    Running,
    Done,
}

#[derive(Default)]
pub struct Progress {
    pub steps: Vec<(String, StepState)>,
    pub finished: bool,
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Progress>>;

fn set_step(p: &Shared, i: usize, state: StepState) {
    if let Ok(mut g) = p.lock() {
        if let Some(s) = g.steps.get_mut(i) {
            s.1 = state;
        }
    }
}

fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Version recorded by a previous install, if any.
pub fn installed_version() -> Option<(String, PathBuf)> {
    let version = reg_get(UNINSTALL_KEY, "DisplayVersion")?;
    let location = reg_get(UNINSTALL_KEY, "InstallLocation")?;
    Some((version, PathBuf::from(location)))
}

fn reg_get(key: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &wide(key),
            &wide(name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    } == ERROR_SUCCESS;
    ok.then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
}

fn reg_set_sz(key: HKEY, name: &str, value: &str) {
    let data: Vec<u8> = value.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
    unsafe {
        let _ = RegSetValueExW(key, &wide(name), None, REG_SZ, Some(&data));
    }
}

fn reg_set_dword(key: HKEY, name: &str, value: u32) {
    unsafe {
        let _ = RegSetValueExW(key, &wide(name), None, REG_DWORD, Some(&value.to_le_bytes()));
    }
}

/// Asks a running Audian to exit, then makes sure all its processes are gone.
pub fn stop_running() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("AudianDaemon"), PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline && !audian_processes().is_empty() {
        std::thread::sleep(Duration::from_millis(150));
    }
    for pid in audian_processes() {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 0);
                let _ = CloseHandle(h);
            }
        }
    }
    std::thread::sleep(Duration::from_millis(300));
}

fn audian_processes() -> Vec<u32> {
    let me = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            if e.th32ProcessID != me && matches!(name.as_str(), "audian.exe" | "audian-stt.exe" | "audian-parakeet.exe" | "audian-llm.exe") {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn write_file(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    // The old file may still be locked for a moment after its process exits.
    for attempt in 0..20 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if attempt == 19 => return Err(anyhow!("replacing {}: {e}", path.display())),
            Err(_) => std::thread::sleep(Duration::from_millis(150)),
        }
    }
    Ok(())
}

fn create_shortcut(link: &Path, target: &Path, args: &str, description: &str) -> anyhow::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell.SetPath(&wide(&target.display().to_string()))?;
        shell.SetArguments(&wide(args))?;
        shell.SetDescription(&wide(description))?;
        if let Some(dir) = target.parent() {
            shell.SetWorkingDirectory(&wide(&dir.display().to_string()))?;
        }
        shell.SetIconLocation(&wide(&target.display().to_string()), 0)?;
        let file: IPersistFile = shell.cast()?;
        if let Some(dir) = link.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        file.Save(&wide(&link.display().to_string()), true)?;
    }
    Ok(())
}

fn start_menu_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs).map(|p| p.join("Audian.lnk"))
}

fn desktop_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop).map(|p| p.join("Audian.lnk"))
}

pub fn has_payload() -> bool {
    !PAYLOAD.is_empty()
}

pub fn steps() -> Vec<(String, StepState)> {
    ["Closing Audian if it's running", "Copying files", "Creating shortcuts", "Registering with Windows"]
        .iter()
        .map(|s| (s.to_string(), StepState::Pending))
        .collect()
}

pub fn install(opts: &Options, progress: &Shared) -> anyhow::Result<()> {
    anyhow::ensure!(has_payload(), "this installer was built without the application files");
    set_step(progress, 0, StepState::Running);
    stop_running();
    set_step(progress, 0, StepState::Done);

    set_step(progress, 1, StepState::Running);
    std::fs::create_dir_all(&opts.dir).with_context(|| format!("creating {}", opts.dir.display()))?;
    let mut total = 0u64;
    for (name, packed) in PAYLOAD {
        let data = miniz_oxide::inflate::decompress_to_vec(packed).map_err(|e| anyhow!("corrupt installer payload ({name}): {e:?}"))?;
        total += data.len() as u64;
        write_file(&opts.dir.join(name), &data)?;
    }
    let me = std::env::current_exe()?;
    let uninstaller = opts.dir.join(UNINSTALLER);
    if me != uninstaller {
        let data = std::fs::read(&me)?;
        total += data.len() as u64;
        write_file(&uninstaller, &data)?;
    }
    set_step(progress, 1, StepState::Done);

    set_step(progress, 2, StepState::Running);
    let app = opts.dir.join("audian.exe");
    if let Some(link) = start_menu_link() {
        create_shortcut(&link, &app, "", "Voice dictation: speak, get polished text wherever you type")?;
    }
    if let Some(link) = desktop_link() {
        if opts.desktop_shortcut {
            create_shortcut(&link, &app, "", "Audian voice dictation")?;
        } else {
            let _ = std::fs::remove_file(link);
        }
    }
    set_step(progress, 2, StepState::Done);

    set_step(progress, 3, StepState::Running);
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, &wide(UNINSTALL_KEY), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None) == ERROR_SUCCESS {
            let dir = opts.dir.display().to_string();
            reg_set_sz(key, "DisplayName", "Audian");
            reg_set_sz(key, "DisplayVersion", audian_common::APP_VERSION);
            reg_set_sz(key, "Publisher", "Audian");
            reg_set_sz(key, "DisplayIcon", &format!("{},0", app.display()));
            reg_set_sz(key, "InstallLocation", &dir);
            reg_set_sz(key, "UninstallString", &format!("\"{}\" --uninstall", uninstaller.display()));
            reg_set_sz(key, "QuietUninstallString", &format!("\"{}\" --uninstall --quiet", uninstaller.display()));
            reg_set_sz(key, "InstallDate", &audian_common::history::now_local().replace('-', "").chars().take(8).collect::<String>());
            reg_set_dword(key, "NoModify", 1);
            reg_set_dword(key, "NoRepair", 1);
            reg_set_dword(key, "EstimatedSize", (total / 1024) as u32);
            let _ = RegCloseKey(key);
        }
        let mut run = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &wide(RUN_KEY), None, KEY_SET_VALUE, &mut run) == ERROR_SUCCESS {
            if opts.autostart {
                reg_set_sz(run, "Audian", &format!("\"{}\" --background", app.display()));
            } else {
                let _ = RegDeleteValueW(run, &wide("Audian"));
            }
            let _ = RegCloseKey(run);
        }
    }
    // Keep the app's own setting in sync with the choice made here.
    let mut cfg = audian_common::config::Config::load();
    cfg.general.start_with_windows = opts.autostart;
    let _ = cfg.save();
    set_step(progress, 3, StepState::Done);
    Ok(())
}

pub fn launch(dir: &Path) {
    let _ = std::process::Command::new(dir.join("audian.exe")).current_dir(dir).spawn();
}

/// Removes the program; with `purge`, also settings, history, logs and models.
pub fn uninstall(purge: bool) -> anyhow::Result<()> {
    let dir = installed_version().map(|(_, d)| d).unwrap_or_else(default_dir);
    stop_running();
    unsafe {
        let mut run = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &wide(RUN_KEY), None, KEY_SET_VALUE, &mut run) == ERROR_SUCCESS {
            let _ = RegDeleteValueW(run, &wide("Audian"));
            let _ = RegCloseKey(run);
        }
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &wide(UNINSTALL_KEY));
    }
    for link in [start_menu_link(), desktop_link()].into_iter().flatten() {
        let _ = std::fs::remove_file(link);
    }
    for (name, _) in PAYLOAD {
        let _ = std::fs::remove_file(dir.join(name));
    }
    for name in ["audian.exe", "audian-stt.exe", "audian-parakeet.exe", "audian-llm.exe"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    if purge {
        let _ = std::fs::remove_dir_all(audian_common::paths::config_dir());
        let _ = std::fs::remove_dir_all(audian_common::paths::data_dir());
    }
    // A running executable cannot be deleted, but it can be renamed: move this uninstaller
    // out of the folder (same volume: %TEMP% lives under %LOCALAPPDATA%), then remove the
    // folder directly. The moved copy is deleted after exit (best effort; it is in %TEMP%).
    let me = std::env::current_exe().ok();
    let mut leftover = None;
    if let Some(me) = me.filter(|m| m.starts_with(&dir)) {
        let parked = std::env::temp_dir().join(format!("audian-uninstall-{}.exe", std::process::id()));
        if std::fs::rename(&me, &parked).is_ok() {
            leftover = Some(parked);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let script = match (&leftover, dir.exists()) {
        (Some(parked), false) => format!("ping 127.0.0.1 -n 3 > nul & del /f /q \"{}\"", parked.display()),
        _ => format!("ping 127.0.0.1 -n 3 > nul & rd /s /q \"{}\"", dir.display()),
    };
    let _ = std::process::Command::new("cmd.exe").args(["/c", &script]).creation_flags(CREATE_NO_WINDOW).spawn();
    Ok(())
}

/// Size of user data that a purge would remove (models dominate).
pub fn user_data_size() -> u64 {
    fn dir_size(p: &Path) -> u64 {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => dir_size(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    dir_size(&audian_common::paths::data_dir()) + dir_size(&audian_common::paths::config_dir())
}
