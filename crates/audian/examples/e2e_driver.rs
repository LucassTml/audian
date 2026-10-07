//! End-to-end test driver (development tool, not shipped).
//!
//! Simulates a user dictating: presses the global shortcut, "speaks" by playing a WAV file into
//! a virtual audio cable (e.g. VB-Audio CABLE Input, whose output Audian records from),
//! then releases the shortcut.
//!
//! usage: e2e_driver <wav> [--style hold|tap] [--device "CABLE Input"] [--title "Window title"]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

fn key(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

const HOTKEY: [u16; 3] = [0x11, 0x10, 0x20]; // Ctrl, Shift, Space

fn press() {
    send(&HOTKEY.map(|k| key(k, false)));
}

fn release() {
    let mut keys = HOTKEY;
    keys.reverse();
    send(&keys.map(|k| key(k, true)));
}

fn read_wav(path: &str) -> (Vec<f32>, u32) {
    let bytes = std::fs::read(path).expect("read wav");
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let data = bytes.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    let samples = bytes[data..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
    (samples, rate)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wav = args.get(1).expect("usage: e2e_driver <wav> [--style hold|tap] [--device name] [--title t]");
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let style = opt("--style").unwrap_or_else(|| "hold".into());
    let device_name = opt("--device").unwrap_or_else(|| "CABLE Input".into());

    if let Some(title) = opt("--title") {
        unsafe {
            if let Ok(hwnd) = FindWindowW(None, &HSTRING::from(title.as_str())) {
                let _ = SetForegroundWindow(hwnd);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    let host = cpal::default_host();
    let device = host
        .output_devices()
        .unwrap()
        .find(|d| d.to_string().contains(&device_name))
        .unwrap_or_else(|| panic!("output device containing \"{device_name}\" not found"));
    let config = device.default_output_config().unwrap().config();
    let (samples, rate) = read_wav(wav);
    // Linear resampling is fine for a test signal.
    let ratio = config.sample_rate as f64 / rate as f64;
    let out_len = (samples.len() as f64 * ratio) as usize;
    let resampled: Vec<f32> = (0..out_len)
        .map(|i| {
            let t = i as f64 / ratio;
            let i0 = t.floor() as usize;
            let f = (t - i0 as f64) as f32;
            let a = samples[i0.min(samples.len() - 1)];
            let b = samples[(i0 + 1).min(samples.len() - 1)];
            a + (b - a) * f
        })
        .collect();
    let channels = config.channels as usize;
    let pos = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let (p2, d2, data) = (pos.clone(), done.clone(), Arc::new(resampled));
    let stream = device
        .build_output_stream::<f32, _, _>(
            config.clone(),
            move |out: &mut [f32], _| {
                for frame in out.chunks_exact_mut(channels) {
                    let i = p2.fetch_add(1, Ordering::Relaxed);
                    let v = data.get(i).copied().unwrap_or(0.0);
                    if i >= data.len() {
                        d2.store(true, Ordering::Relaxed);
                    }
                    frame.iter_mut().for_each(|s| *s = v);
                }
            },
            |e| eprintln!("stream error: {e}"),
            None,
        )
        .unwrap();

    let t0 = Instant::now();
    press();
    if style == "tap" || style == "handsfree" {
        std::thread::sleep(Duration::from_millis(80));
        release();
    }
    std::thread::sleep(Duration::from_millis(400)); // let the recorder open the device
    stream.play().unwrap();
    while !done.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(400));
    if style == "handsfree" {
        // Never touch the keys again: the app must notice the pause by itself.
        println!("dictation audio finished after {} ms", t0.elapsed().as_millis());
        return;
    }
    if style == "tap" {
        press();
        std::thread::sleep(Duration::from_millis(60));
    }
    release();
    println!("dictation finished after {} ms", t0.elapsed().as_millis());
}
