//! Microphone capture and the signal processing needed before speech recognition.

pub mod capture;
pub mod dsp;
pub mod vad;

pub use capture::{AudioError, DeviceInfo, LevelMeter, Recorder, Recording, list_input_devices};
