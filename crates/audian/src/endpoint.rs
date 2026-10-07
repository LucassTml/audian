//! Deciding when a hands-free dictation is finished.
//!
//! Silence alone is ambiguous: people pause mid-sentence to think. Audian combines the
//! measured pause with what was said so far — after a short pause it transcribes the audio
//! captured so far (a "preview"), and if the sentence sounds unfinished ("…and", "…because",
//! "…porque", a trailing comma) it waits longer before stopping. The preview transcript is
//! reused when the recording stops, so the final result also arrives sooner.

use audian_common::config::AutoStopConfig;

use crate::audio::vad::VadStatus;

/// Minimum speech before auto-stop may trigger (ignores coughs / clicks).
const MIN_SPEECH_MS: u32 = 250;
/// A preview is requested once a pause reaches this length (or half the pause tolerance).
const PREVIEW_AFTER_MS: u32 = 450;

#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    /// `VadStatus::last_voice_ms` when the preview audio was captured; the preview is only
    /// valid while no further speech has happened.
    pub at_voice_ms: u32,
    /// `None` while the transcription is still running.
    pub text: Option<String>,
    pub language: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Continue,
    /// Start a preview transcription of the audio so far.
    RequestPreview,
    Stop,
    /// Nothing was said within the no-speech timeout.
    CancelNoSpeech,
}

pub struct Outcome {
    pub decision: Decision,
    /// 0..1 progress towards auto-stop during a pause, for the overlay.
    pub progress: Option<f32>,
}

pub fn decide(status: &VadStatus, cfg: &AutoStopConfig, preview: Option<&Preview>) -> Outcome {
    let cont = Outcome { decision: Decision::Continue, progress: None };
    if status.speech_ms < MIN_SPEECH_MS {
        let no_speech_ms = (cfg.no_speech_secs.max(2.0) * 1000.0) as u32;
        if status.elapsed_ms >= no_speech_ms {
            return Outcome { decision: Decision::CancelNoSpeech, progress: None };
        }
        return cont;
    }
    let silence = status.silence_ms();
    let pause = (cfg.pause_secs.clamp(0.3, 10.0) * 1000.0) as u32;
    let extra = (cfg.unfinished_extra_secs.clamp(0.0, 10.0) * 1000.0) as u32;
    if silence < 200 || status.in_speech {
        return cont;
    }
    if !cfg.smart {
        return Outcome {
            decision: if silence >= pause { Decision::Stop } else { Decision::Continue },
            progress: Some(silence as f32 / pause as f32),
        };
    }

    let current = preview.filter(|p| p.at_voice_ms == status.last_voice_ms);
    // How long to wait, given what we know about the sentence so far.
    let (limit, known) = match current.and_then(|p| p.text.as_deref()) {
        Some(text) if sounds_unfinished(text) => (pause + extra, true),
        Some(_) => (pause, true),
        None => (pause + extra, false), // unknown yet: be patient, but never wait forever
    };
    let progress = Some((silence as f32 / limit as f32).min(1.0));
    if current.is_none() && silence >= PREVIEW_AFTER_MS.min(pause / 2) {
        return Outcome { decision: Decision::RequestPreview, progress };
    }
    let _ = known;
    if silence >= limit {
        return Outcome { decision: Decision::Stop, progress };
    }
    Outcome { decision: Decision::Continue, progress }
}

/// Heuristic: does this (partial) transcript end mid-sentence?
pub fn sounds_unfinished(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if t.ends_with(',') || t.ends_with(';') || t.ends_with(':') || t.ends_with('-') || t.ends_with("...") || t.ends_with('…') {
        return true;
    }
    // Whisper ends every segment with punctuation when the audio stops, even mid-sentence,
    // so trailing "." / "?" carry no information; look at the words instead.
    let words: Vec<String> = t
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    let last = words.last().cloned().unwrap_or_default();
    let before = words.len().checked_sub(2).and_then(|i| words.get(i)).cloned().unwrap_or_default();
    // A dangling "and" is often transcribed as "end" once the audio stops.
    const DETERMINERS: &[&str] = &["the", "an", "a", "this", "that", "its", "his", "her", "their", "our", "my", "your", "week", "dead", "front", "back", "high", "low", "rear", "tail"];
    if last == "end" && !DETERMINERS.contains(&before.as_str()) {
        return true;
    }
    const CONTINUATIONS: &[&str] = &[
        // English
        "and", "or", "but", "so", "because", "cause", "the", "a", "an", "to", "of", "with", "for", "in", "on", "at",
        "that", "which", "who", "if", "when", "then", "like", "um", "uh", "my", "your", "our", "their", "is", "are",
        "was", "were", "from", "about", "than", "as", "also", "plus", "into", "by", "this", "these", "maybe", "just",
        "and/or", "while", "until", "since", "although", "though", "where", "whether", "i", "we", "you", "it",
        "hmm", "uhm", "er", "erm", "well", "actually", "basically",
        // Portuguese
        "e", "ou", "mas", "então", "porque", "pois", "que", "o", "os", "as", "um", "uma", "de", "do", "da", "dos",
        "das", "para", "pra", "com", "em", "no", "na", "nos", "nas", "se", "quando", "como", "tipo", "eu", "você",
        "mais", "também", "pelo", "pela", "num", "numa", "ao", "aos", "meu", "minha", "seu", "sua", "é", "né",
    ];
    CONTINUATIONS.contains(&last.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(speech_ms: u32, last_voice_ms: u32, elapsed_ms: u32) -> VadStatus {
        VadStatus { speech_ms, last_voice_ms, elapsed_ms, in_speech: false }
    }

    fn cfg() -> AutoStopConfig {
        AutoStopConfig { enabled: true, pause_secs: 1.5, smart: true, unfinished_extra_secs: 1.5, no_speech_secs: 8.0 }
    }

    #[test]
    fn unfinished_heuristic() {
        assert!(sounds_unfinished("I want to go to the"));
        assert!(sounds_unfinished("We should fix the bug and..."));
        assert!(sounds_unfinished("Eu acho que a gente devia, porque"));
        assert!(sounds_unfinished("First,"));
        assert!(sounds_unfinished("I think we should push the release end."));
        assert!(sounds_unfinished("So the plan is, um."));
        assert!(!sounds_unfinished("We'll review it at the end."));
        assert!(!sounds_unfinished("Let's meet on Wednesday at 3."));
        assert!(!sounds_unfinished("Você pode me mandar o relatório amanhã?"));
    }

    #[test]
    fn smart_flow() {
        let c = cfg();
        // Speaking: continue.
        assert_eq!(decide(&status(2000, 3000, 3100), &c, None).decision, Decision::Continue);
        // Short pause: ask for a preview.
        assert_eq!(decide(&status(2000, 3000, 3500), &c, None).decision, Decision::RequestPreview);
        // Complete sentence: stop at the normal pause.
        let done = Preview { at_voice_ms: 3000, text: Some("Send it to Maria.".into()), language: "en".into() };
        assert_eq!(decide(&status(2000, 3000, 4200), &c, Some(&done)).decision, Decision::Continue);
        assert_eq!(decide(&status(2000, 3000, 4500), &c, Some(&done)).decision, Decision::Stop);
        // Unfinished sentence: wait for pause + extra.
        let open = Preview { at_voice_ms: 3000, text: Some("Send it to Maria and".into()), language: "en".into() };
        assert_eq!(decide(&status(2000, 3000, 4600), &c, Some(&open)).decision, Decision::Continue);
        assert_eq!(decide(&status(2000, 3000, 6000), &c, Some(&open)).decision, Decision::Stop);
        // A stale preview (user spoke again since) triggers a new one.
        assert_eq!(decide(&status(2500, 5000, 5600), &c, Some(&open)).decision, Decision::RequestPreview);
    }

    #[test]
    fn no_speech_and_simple_mode() {
        let mut c = cfg();
        assert_eq!(decide(&status(0, 0, 8100), &c, None).decision, Decision::CancelNoSpeech);
        c.smart = false;
        assert_eq!(decide(&status(2000, 3000, 4400), &c, None).decision, Decision::Continue);
        assert_eq!(decide(&status(2000, 3000, 4600), &c, None).decision, Decision::Stop);
    }
}
