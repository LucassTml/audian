//! Offline signal processing on a finished recording: resampling to 16 kHz and trimming silence.

use std::f32::consts::PI;

/// Resamples `input` from `from_hz` to `to_hz` with a windowed-sinc low-pass interpolator.
/// Runs once per recording (not in real time), so clarity is preferred over speed; a 30 s
/// clip at 48 kHz takes a few tens of milliseconds.
pub fn resample(input: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_hz as f64 / from_hz as f64;
    // Cut off slightly below the lower Nyquist frequency to avoid aliasing.
    let cutoff = (ratio.min(1.0) * 0.92) as f32;
    const HALF_TAPS: f32 = 16.0;
    let half_width = (HALF_TAPS / cutoff).ceil() as isize;
    let out_len = ((input.len() as f64) * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let t = n as f64 / ratio; // position in input samples
        let center = t.floor() as isize;
        let frac = (t - center as f64) as f32;
        let mut acc = 0.0f32;
        let mut norm = 0.0f32;
        for k in (center - half_width + 1)..=(center + half_width) {
            if k < 0 || k as usize >= input.len() {
                continue;
            }
            let x = (k - center) as f32 - frac; // distance in input samples
            let w = window(x / half_width as f32) * sinc(x * cutoff);
            acc += input[k as usize] * w;
            norm += w;
        }
        out.push(if norm.abs() > 1e-6 { acc / norm } else { 0.0 });
    }
    out
}

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 { 1.0 } else { (PI * x).sin() / (PI * x) }
}

/// Blackman window over [-1, 1].
fn window(x: f32) -> f32 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let p = PI * (x + 1.0); // 0..2π
    0.42 - 0.5 * p.cos() + 0.08 * (2.0 * p).cos()
}

/// Removes leading and trailing silence, keeping `pad_ms` of margin so word onsets survive.
/// Shorter input to the recognizer is faster and reduces hallucinations on silent tails.
pub fn trim_silence(samples: &[f32], rate: u32, threshold: f32, pad_ms: u32) -> &[f32] {
    let block = (rate as usize / 100).max(1); // 10 ms
    let rms = |chunk: &[f32]| (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
    let loud: Vec<bool> = samples.chunks(block).map(|c| rms(c) >= threshold).collect();
    let (Some(first), Some(last)) = (loud.iter().position(|&l| l), loud.iter().rposition(|&l| l)) else {
        return &samples[0..0];
    };
    let pad = (rate as usize * pad_ms as usize) / 1000;
    let start = (first * block).saturating_sub(pad);
    let end = ((last + 1) * block + pad).min(samples.len());
    &samples[start..end]
}

/// Scales quiet recordings up (never down) so the loudest part sits around -3 dBFS.
/// Whisper copes with quiet input, but normalisation makes results more consistent across
/// microphones with very different gain.
pub fn normalize(samples: &mut [f32]) {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1e-4 && peak < 0.5 {
        let gain = (0.7 / peak).min(10.0);
        for s in samples.iter_mut() {
            *s *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_tone() {
        // 440 Hz tone at 48 kHz -> 16 kHz should keep its amplitude and length ratio.
        let input: Vec<f32> = (0..48_000).map(|i| (2.0 * PI * 440.0 * i as f32 / 48_000.0).sin() * 0.5).collect();
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        let peak = out[1000..15_000].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn trims_silence() {
        let mut v = vec![0.0f32; 16_000];
        for s in &mut v[8_000..9_600] {
            *s = 0.3;
        }
        let t = trim_silence(&v, 16_000, 0.01, 100);
        assert!(t.len() >= 1_600 && t.len() <= 1_600 + 2 * 1_600 + 320, "len {}", t.len());
        assert!(trim_silence(&[0.0; 1000], 16_000, 0.01, 100).is_empty());
    }
}
