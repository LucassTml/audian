//! Offline, rule-based cleanup. Used for the Literal mode, when rewriting is disabled, when
//! the user picks "Offline rules", and as the fallback when an AI provider fails.

use audian_common::config::ProcessingMode;

use super::{ProcessError, RewriteRequest, TextProcessor};

/// Pure hesitation sounds that never carry meaning (English and Portuguese).
const FILLERS: &[&str] = &[
    "um", "umm", "ummm", "uh", "uhh", "uhm", "erm", "er", "ah", "hmm", "hm", "mm", "mhm", "ahn", "hã", "éh", "hum",
];

pub struct Rules;

impl TextProcessor for Rules {
    fn name(&self) -> &'static str {
        "Offline rules"
    }

    fn is_cloud(&self) -> bool {
        false
    }

    fn process(&mut self, request: &RewriteRequest) -> Result<String, ProcessError> {
        Ok(match request.mode {
            ProcessingMode::Literal => literal(request.text),
            _ => clean(request.text),
        })
    }
}

/// Minimal changes: whitespace normalisation and recognizer artefacts only.
pub fn literal(text: &str) -> String {
    tidy_spacing(&strip_annotations(text))
}

/// Light cleanup: also removes hesitation fillers and stutters, and fixes capitalisation.
pub fn clean(text: &str) -> String {
    let text = strip_annotations(text);
    let mut words: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        let core: String = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if FILLERS.contains(&core.as_str()) {
            // Keep sentence punctuation that was attached to the filler ("um." -> ".").
            if let Some(p) = raw.chars().last().filter(|c| matches!(c, '.' | '?' | '!')) {
                if let Some(prev) = words.last_mut() {
                    if !prev.ends_with(['.', '?', '!', ',']) {
                        prev.push(p);
                    }
                }
            }
            continue;
        }
        // Drop immediate stutters ("I I", "the the"), comparing without punctuation.
        if let Some(prev) = words.last() {
            let prev_core: String = prev.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            if !core.is_empty() && prev_core == core && !prev.ends_with(['.', '?', '!', ',']) {
                words.pop();
            }
        }
        words.push(raw.to_string());
    }
    let joined = words.join(" ");
    let joined = joined.replace(" ,", ",").replace(",,", ",");
    let joined = joined.trim_start_matches([',', ' ']).to_string();
    capitalize_sentences(&tidy_spacing(&joined))
}

/// Removes bracketed recognizer annotations such as `[BLANK_AUDIO]`, `(music)`, `*laughs*`.
pub fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth_sq = 0;
    let mut depth_par = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => depth_sq += 1,
            ']' if depth_sq > 0 => depth_sq -= 1,
            '(' if is_annotation_paren(chars.clone()) => depth_par += 1,
            ')' if depth_par > 0 => depth_par -= 1,
            _ if depth_sq > 0 || depth_par > 0 => {}
            _ => out.push(c),
        }
    }
    out
}

/// Parenthesised text is only treated as an annotation when it is a short, all-lowercase
/// sound description like "(music)" or "(inaudible)", so real parentheses survive.
fn is_annotation_paren(rest: std::iter::Peekable<std::str::Chars>) -> bool {
    let inner: String = rest.take_while(|&c| c != ')').collect();
    let known = ["music", "inaudible", "silence", "applause", "laughter", "laughs", "noise", "coughs", "sighs", "música", "risos"];
    let lower = inner.trim().to_lowercase();
    known.iter().any(|k| lower == *k)
}

fn tidy_spacing(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        if matches!(c, ',' | '.' | '?' | '!' | ';' | ':') && out.ends_with(' ') {
            out.pop();
        }
        out.push(c);
    }
    out.trim().to_string()
}

fn capitalize_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize = true;
    for c in text.chars() {
        if capitalize && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalize = false;
            continue;
        }
        if matches!(c, '.' | '?' | '!') {
            capitalize = true;
        } else if !c.is_whitespace() && !matches!(c, '"' | '\'' | '(' | '¿' | '¡') {
            capitalize = false;
        }
        out.push(c);
    }
    out
}

/// Known Whisper hallucinations on silent or near-silent audio.
pub fn is_hallucination(text: &str) -> bool {
    let t: String = text.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    const PHRASES: &[&str] = &[
        "thank you",
        "thanks for watching",
        "thank you for watching",
        "you",
        "bye",
        "obrigado",
        "obrigada",
        "legendas pela comunidade amara.org",
        "subtitles by the amara.org community",
    ];
    PHRASES.contains(&t.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_fillers_and_stutters() {
        assert_eq!(
            clean("um so I I want to uh fix the the bug."),
            "So I want to fix the bug."
        );
        assert_eq!(clean("Uh, what's the capital of France?"), "What's the capital of France?");
    }

    #[test]
    fn strips_annotations() {
        assert_eq!(literal("[BLANK_AUDIO] hello (music) world"), "hello world");
        assert_eq!(literal("call foo(bar) now"), "call foo(bar) now");
    }

    #[test]
    fn hallucinations() {
        assert!(is_hallucination(" Thank you. "));
        assert!(!is_hallucination("Thank you for the report."));
    }
}
