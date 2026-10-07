//! Microphone capture via WASAPI (through `cpal`).
//!
//! The input stream exists only while recording: when idle there is no open audio device,
//! no audio thread and no "microphone in use" indicator. Samples are kept in memory only.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use audian_common::config::AudioConfig;

use super::vad::{Vad, VadStatus};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("No microphone was found. Connect a microphone and try again.")]
    NoDevice,
    #[error("The selected microphone \"{0}\" is not available.")]
    DeviceUnavailable(String),
    #[error("Microphone access is blocked. Allow desktop apps to use the microphone in Windows Settings › Privacy › Microphone.")]
    PermissionDenied,
    #[error("The microphone is busy or in exclusive use by another application.")]
    Busy,
    #[error("Could not start the microphone: {0}")]
    Other(String),
}

impl From<cpal::Error> for AudioError {
    fn from(e: cpal::Error) -> Self {
        use cpal::ErrorKind as K;
        match e.kind() {
            K::PermissionDenied => AudioError::PermissionDenied,
            K::DeviceBusy => AudioError::Busy,
            K::DeviceNotAvailable => AudioError::DeviceUnavailable(e.to_string()),
            _ => AudioError::Other(e.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|id| id.to_string());
    let Ok(devices) = host.input_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.to_string();
            let is_default = default_id.as_deref() == Some(id.as_str());
            Some(DeviceInfo { id, name, is_default })
        })
        .collect()
}

fn find_device(cfg: &AudioConfig) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    if !cfg.device_id.is_empty() {
        if let Ok(id) = cfg.device_id.parse::<cpal::DeviceId>() {
            if let Some(d) = host.device_by_id(&id) {
                return Ok(d);
            }
        }
        // The id can change (e.g. after driver updates); fall back to matching by name.
        if !cfg.device_name.is_empty() {
            if let Ok(mut devices) = host.input_devices() {
                if let Some(d) = devices.find(|d| d.to_string() == cfg.device_name) {
                    return Ok(d);
                }
            }
        }
        log::warn!("configured microphone \"{}\" not found; using the default device", cfg.device_name);
    }
    host.default_input_device().ok_or(AudioError::NoDevice)
}

/// State shared between the audio callback thread and the rest of the app.
struct Shared {
    samples: Mutex<Vec<f32>>,
    /// Peak RMS (f32 bits) since the overlay last read it.
    level: AtomicU32,
    /// Loudest RMS block seen during the whole recording (f32 bits).
    peak: AtomicU32,
    failed: AtomicBool,
    vad: Mutex<Vad>,
}

/// Cheap handle the overlay uses to animate the waveform.
#[derive(Clone)]
pub struct LevelMeter(Arc<Shared>);

impl LevelMeter {
    /// Returns the loudest level since the previous call and resets it.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.level.swap(0, Ordering::Relaxed))
    }
}

pub struct Recording {
    /// Mono samples at `sample_rate`.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Loudest 10 ms RMS block, used to reject silent recordings.
    pub peak_rms: f32,
    /// Voice activity summary for the whole recording.
    pub vad: VadStatus,
}

impl Recording {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / self.sample_rate.max(1) as f64)
    }
}

pub struct Recorder {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    sample_rate: u32,
    pub device_name: String,
}

impl Recorder {
    /// Opens the microphone and starts capturing. `on_error` is called from the audio thread if
    /// the stream fails later (e.g. the device is unplugged mid-recording).
    pub fn start(cfg: &AudioConfig, on_error: impl Fn(String) + Send + 'static) -> Result<Recorder, AudioError> {
        let device = find_device(cfg)?;
        let device_name = device.to_string();
        let supported = device.default_input_config()?;
        let format = supported.sample_format();
        let config = supported.config();
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;

        let shared = Arc::new(Shared {
            samples: Mutex::new(Vec::with_capacity(sample_rate as usize * 30)),
            level: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            failed: AtomicBool::new(false),
            vad: Mutex::new(Vad::new(cfg.vad_sensitivity)),
        });
        let gain = if cfg.gain.is_finite() && cfg.gain > 0.0 { cfg.gain.min(8.0) } else { 1.0 };
        let block = (sample_rate as usize / 100).max(1); // 10 ms

        let err_shared = shared.clone();
        let error_cb = move |e: cpal::Error| {
            if !err_shared.failed.swap(true, Ordering::Relaxed) {
                on_error(e.to_string());
            }
        };

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            SampleFormat::I16 => build::<i16>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            SampleFormat::I32 => build::<i32>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            SampleFormat::U16 => build::<u16>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            SampleFormat::U8 => build::<u8>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            SampleFormat::F64 => build::<f64>(&device, &config, channels, gain, block, shared.clone(), error_cb)?,
            other => return Err(AudioError::Other(format!("unsupported sample format {other:?}"))),
        };
        stream.play()?;
        log::info!("recording from \"{device_name}\" ({sample_rate} Hz, {channels} ch, {format:?})");
        Ok(Recorder { stream, shared, sample_rate, device_name })
    }

    pub fn meter(&self) -> LevelMeter {
        LevelMeter(self.shared.clone())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn vad_status(&self) -> VadStatus {
        self.shared.vad.lock().map(|v| v.status()).unwrap_or_default()
    }

    /// Copy of everything captured so far (for preview transcription while still recording).
    pub fn snapshot(&self) -> Vec<f32> {
        self.shared.samples.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn stop(self) -> Recording {
        let _ = self.stream.pause();
        drop(self.stream); // closes the device
        let samples = std::mem::take(&mut *self.shared.samples.lock().unwrap_or_else(|e| e.into_inner()));
        Recording {
            samples,
            sample_rate: self.sample_rate,
            peak_rms: f32::from_bits(self.shared.peak.load(Ordering::Relaxed)),
            vad: self.shared.vad.lock().map(|v| v.status()).unwrap_or_default(),
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    gain: f32,
    block: usize,
    shared: Arc<Shared>,
    error_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut mono: Vec<f32> = Vec::with_capacity(4096);
    let (mut sum_sq, mut count) = (0.0f32, 0usize);
    let data_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
        mono.clear();
        for frame in data.chunks_exact(channels) {
            let s: f32 = frame.iter().map(|&x| x.to_sample::<f32>()).sum::<f32>() / channels as f32;
            mono.push((s * gain).clamp(-1.0, 1.0));
        }
        // Level metering in 10 ms blocks.
        for &s in &mono {
            sum_sq += s * s;
            count += 1;
            if count == block {
                let rms = (sum_sq / count as f32).sqrt();
                if rms > f32::from_bits(shared.level.load(Ordering::Relaxed)) {
                    shared.level.store(rms.to_bits(), Ordering::Relaxed);
                }
                if rms > f32::from_bits(shared.peak.load(Ordering::Relaxed)) {
                    shared.peak.store(rms.to_bits(), Ordering::Relaxed);
                }
                if let Ok(mut vad) = shared.vad.lock() {
                    vad.push(rms);
                }
                sum_sq = 0.0;
                count = 0;
            }
        }
        if let Ok(mut buf) = shared.samples.lock() {
            buf.extend_from_slice(&mono);
        }
    };
    Ok(device.build_input_stream::<T, _, _>(config.clone(), data_cb, error_cb, Some(Duration::from_secs(3)))?)
}
