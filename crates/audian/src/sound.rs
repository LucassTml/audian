//! Subtle audio cues, synthesised on the fly (no sound files): a soft rising two-note chime
//! when listening starts, the mirrored chime when it stops, and a low double pulse on errors.
//! The output device is opened only for the ~0.2 s a cue lasts.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Clone, Copy, Debug)]
pub enum Cue {
    Start,
    Stop,
    Error,
}

pub fn play(cue: Cue, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    let _ = std::thread::Builder::new().name("audio-cue".into()).spawn(move || {
        if let Err(e) = play_blocking(cue, volume) {
            log::debug!("audio cue failed: {e}");
        }
    });
}

/// One bell-like note: fundamental plus a soft octave, fast attack, exponential decay.
fn note(out: &mut [f32], rate: f32, start_s: f32, freq: f32, len_s: f32, amp: f32) {
    let start = (start_s * rate) as usize;
    let len = (len_s * rate) as usize;
    for i in 0..len {
        let Some(slot) = out.get_mut(start + i) else { break };
        let t = i as f32 / rate;
        let attack = (t / 0.004).min(1.0);
        let env = attack * (-t / (len_s * 0.35)).exp();
        let v = (TAU * freq * t).sin() + 0.18 * (TAU * 2.0 * freq * t).sin();
        *slot += v * env * amp;
    }
}

fn synth(cue: Cue, rate: f32, volume: f32) -> Vec<f32> {
    let amp = 0.16 * volume.clamp(0.0, 1.0);
    let mut out = vec![0.0f32; (rate * 0.32) as usize];
    match cue {
        Cue::Start => {
            note(&mut out, rate, 0.0, 659.25, 0.16, amp); // E5
            note(&mut out, rate, 0.07, 987.77, 0.22, amp); // B5
        }
        Cue::Stop => {
            note(&mut out, rate, 0.0, 987.77, 0.14, amp * 0.9);
            note(&mut out, rate, 0.06, 659.25, 0.2, amp * 0.9);
        }
        Cue::Error => {
            note(&mut out, rate, 0.0, 329.63, 0.12, amp * 1.2);
            note(&mut out, rate, 0.13, 293.66, 0.16, amp * 1.2);
        }
    }
    out
}

fn play_blocking(cue: Cue, volume: f32) -> anyhow::Result<()> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device"))?;
    let config = device.default_output_config()?.config();
    let channels = config.channels as usize;
    let samples = Arc::new(synth(cue, config.sample_rate as f32, volume));
    let pos = Arc::new(AtomicUsize::new(0));
    let (s2, p2) = (samples.clone(), pos.clone());
    let stream = device.build_output_stream::<f32, _, _>(
        config.clone(),
        move |out: &mut [f32], _| {
            for frame in out.chunks_exact_mut(channels) {
                let i = p2.fetch_add(1, Ordering::Relaxed);
                let v = s2.get(i).copied().unwrap_or(0.0);
                frame.iter_mut().for_each(|x| *x = v);
            }
        },
        |e| log::debug!("cue stream error: {e}"),
        Some(Duration::from_secs(2)),
    )?;
    stream.play()?;
    let dur = samples.len() as f32 / config.sample_rate as f32;
    std::thread::sleep(Duration::from_secs_f32(dur + 0.08));
    Ok(())
}
