//! Voice activity detection on 10 ms blocks, cheap enough to run inside the audio callback.
//!
//! Energy-based with an adaptive noise floor:
//! * the floor follows the quietest recent level — it falls quickly and rises slowly
//!   (3 dB/s), so steady background noise (fans, hum) is learned within a few seconds while
//!   the natural dips between words keep it from creeping up during speech;
//! * a block is "voiced" when it is `margin` dB above the floor (margin set by sensitivity);
//! * speech starts after 3 consecutive voiced blocks (30 ms — ignores clicks and taps) and
//!   ends after 200 ms of unvoiced blocks (bridges gaps between words).

pub const BLOCK_MS: u32 = 10;
const START_BLOCKS: u32 = 3;
const HANGOVER_BLOCKS: u32 = 20;
const CALIBRATION_BLOCKS: u32 = 15;
const RISE_DB_PER_BLOCK: f32 = 0.03;
const ABSOLUTE_FLOOR_DB: f32 = -60.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VadStatus {
    /// Total time classified as speech.
    pub speech_ms: u32,
    /// Time (since recording start) of the most recent voiced block.
    pub last_voice_ms: u32,
    /// Recording length so far.
    pub elapsed_ms: u32,
    pub in_speech: bool,
}

impl VadStatus {
    pub fn silence_ms(&self) -> u32 {
        if self.speech_ms == 0 { self.elapsed_ms } else { self.elapsed_ms.saturating_sub(self.last_voice_ms) }
    }
}

pub struct Vad {
    margin_db: f32,
    noise_db: f32,
    blocks: u32,
    voiced_run: u32,
    unvoiced_run: u32,
    status: VadStatus,
}

impl Vad {
    /// `sensitivity` 0.0 (only clear, loud speech) – 1.0 (picks up soft speech).
    pub fn new(sensitivity: f32) -> Vad {
        let s = sensitivity.clamp(0.0, 1.0);
        Vad {
            margin_db: 16.0 - 10.0 * s,
            noise_db: 0.0,
            blocks: 0,
            voiced_run: 0,
            unvoiced_run: 0,
            status: VadStatus::default(),
        }
    }

    pub fn status(&self) -> VadStatus {
        self.status
    }

    /// Feeds the RMS level of one 10 ms block.
    pub fn push(&mut self, rms: f32) {
        let db = 20.0 * rms.max(1e-6).log10();
        let t = self.blocks * BLOCK_MS;
        self.blocks += 1;
        self.status.elapsed_ms = self.blocks * BLOCK_MS;

        if self.blocks <= CALIBRATION_BLOCKS {
            self.noise_db = if self.blocks == 1 { db } else { self.noise_db.min(db) };
        } else if db < self.noise_db {
            self.noise_db = 0.7 * self.noise_db + 0.3 * db;
        } else {
            self.noise_db += RISE_DB_PER_BLOCK;
        }

        let threshold = (self.noise_db + self.margin_db).max(ABSOLUTE_FLOOR_DB);
        let voiced = db > threshold;
        if voiced {
            self.voiced_run += 1;
            self.unvoiced_run = 0;
        } else {
            self.unvoiced_run += 1;
            self.voiced_run = 0;
        }
        if !self.status.in_speech && self.voiced_run >= START_BLOCKS {
            self.status.in_speech = true;
            // Count the blocks that triggered the start.
            self.status.speech_ms += (START_BLOCKS - 1) * BLOCK_MS;
        } else if self.status.in_speech && self.unvoiced_run >= HANGOVER_BLOCKS {
            self.status.in_speech = false;
        }
        if self.status.in_speech && voiced {
            self.status.speech_ms += BLOCK_MS;
            self.status.last_voice_ms = t + BLOCK_MS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(level_db: f32) -> f32 {
        10f32.powf(level_db / 20.0)
    }

    fn feed(vad: &mut Vad, level_db: f32, ms: u32, wobble: bool) {
        for i in 0..ms / BLOCK_MS {
            // Speech fluctuates between syllables; noise is steady.
            let l = if wobble && i % 7 == 6 { level_db - 18.0 } else { level_db };
            vad.push(db(l));
        }
    }

    #[test]
    fn detects_speech_and_pauses_over_noise() {
        let mut vad = Vad::new(0.5);
        feed(&mut vad, -50.0, 600, false); // room noise
        assert_eq!(vad.status().speech_ms, 0);
        feed(&mut vad, -22.0, 1500, true); // speech
        feed(&mut vad, -50.0, 400, false); // short pause between phrases
        feed(&mut vad, -22.0, 1000, true); // more speech
        let after_speech = vad.status();
        feed(&mut vad, -50.0, 2000, false); // silence
        let s = vad.status();
        assert!(s.speech_ms > 2000 && s.speech_ms <= 2600, "speech_ms {}", s.speech_ms);
        assert_eq!(s.last_voice_ms, after_speech.last_voice_ms);
        assert!(s.silence_ms() >= 1990, "silence {}", s.silence_ms());
        assert!(!s.in_speech);
    }

    #[test]
    fn ignores_clicks() {
        let mut vad = Vad::new(0.5);
        feed(&mut vad, -55.0, 500, false);
        feed(&mut vad, -15.0, 20, false); // a 20 ms key click
        feed(&mut vad, -55.0, 500, false);
        assert_eq!(vad.status().speech_ms, 0);
    }

    #[test]
    fn digital_silence() {
        let mut vad = Vad::new(0.5);
        feed(&mut vad, -120.0, 300, false);
        feed(&mut vad, -25.0, 500, true);
        feed(&mut vad, -120.0, 1000, false);
        let s = vad.status();
        assert!(s.speech_ms >= 300, "speech_ms {}", s.speech_ms);
        assert!(s.silence_ms() >= 990);
    }
}
