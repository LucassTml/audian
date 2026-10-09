//! Finds spoken self-corrections ("às 9, não, na verdade às 8", "send it to Mark, I mean Sarah")
//! so the rewriting prompt can point the model at them. Small models often miss a correction
//! buried in a sentence, but fix it reliably once the passage is quoted for them.
//!
//! Detection is deliberately loose: the quoted passage is a hint, and the model still decides
//! whether it really is a correction ("na verdade" is also used just for emphasis).

/// Phrases that introduce a correction, lower-case, as word sequences.
const MARKERS: &[&str] = &[
    // Portuguese
    "na verdade",
    "quer dizer",
    "ou melhor",
    "melhor dizendo",
    "digo",
    "aliás",
    "eu errei",
    "errei",
    "me enganei",
    "corrigindo",
    "desculpa",
    "desculpe",
    "perdão",
    // English
    "actually",
    "i mean",
    "i meant",
    "or rather",
    "scratch that",
    "make that",
    "no wait",
    "wait no",
    "correction",
    "sorry",
    // Spanish / French
    "o sea",
    "mejor dicho",
    "perdón",
    "je veux dire",
    "pardon",
];

/// "No"/"não" set off by a comma right after other words: "às 3, não, às 4".
const BARE_NO: &[&str] = &["não", "no", "nein", "non"];

/// Words kept before and after a marker in the quoted passage.
const BEFORE: usize = 6;
const AFTER: usize = 8;

/// A word with its byte span in the original text.
struct Word {
    start: usize,
    end: usize,
    lower: String,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        let part_of_word = c.is_alphanumeric() || c == '\'' || c == '’';
        match (part_of_word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(Word { start: s, end: i, lower: text[s..i].to_lowercase() });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Word { start: s, end: text.len(), lower: text[s..].to_lowercase() });
    }
    out
}

/// Passages that look like self-corrections, quoted from `text` (at most three).
///
/// Only the passage is quoted. Naming the correction phrase as well was tried: the model then
/// just deleted the phrase and kept both versions ("amanhã de manhã, hoje à tarde").
pub fn find(text: &str) -> Vec<String> {
    let w = words(text);
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (first word, last word) of each marker
    for i in 0..w.len() {
        for marker in MARKERS {
            let parts: Vec<&str> = marker.split(' ').collect();
            if i + parts.len() <= w.len() && parts.iter().enumerate().all(|(k, p)| w[i + k].lower == *p) {
                hits.push((i, i + parts.len() - 1));
            }
        }
        if BARE_NO.contains(&w[i].lower.as_str()) && i > 0 {
            // Needs a comma (or dash) between the previous word and "no".
            let gap = &text[w[i - 1].end..w[i].start];
            if gap.contains(',') || gap.contains('—') || gap.contains('-') {
                hits.push((i, i));
            }
        }
    }
    let mut passages: Vec<(usize, usize)> = Vec::new();
    for (first, last) in hits {
        // Nothing before it in the transcript: an opener ("Na verdade, eu acho..."), not a fix.
        if first == 0 {
            continue;
        }
        let from = first.saturating_sub(BEFORE);
        let to = (last + AFTER).min(w.len() - 1);
        match passages.last_mut() {
            Some(p) if from <= p.1 => p.1 = p.1.max(to),
            _ => passages.push((from, to)),
        }
    }
    passages.into_iter().take(3).map(|(from, to)| text[w[from].start..w[to].end].to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::find;

    #[test]
    fn finds_portuguese_and_english_corrections() {
        let p = find("Vamos marcar a reunião para as 9 da noite. Não, na verdade, eu errei, é para ser às 8.");
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("9 da noite") && p[0].contains("às 8"), "{p:?}");
        assert_eq!(find("O voo sai na sexta, aliás, no sábado de manhã.").len(), 1);
        assert_eq!(find("Eu vou chegar às 3, não, às 4, e aí a gente conversa.").len(), 1);
        assert_eq!(find("Send it to Mark, I mean to Sarah, before Friday.").len(), 1);
    }

    #[test]
    fn ignores_openers_and_plain_text() {
        assert!(find("Na verdade, eu gosto muito desse projeto.").is_empty());
        assert!(find("Quer dizer que você vai viajar amanhã?").is_empty());
        assert!(find("Actually, I really like this design.").is_empty());
        assert!(find("Então, o servidor caiu de novo e a gente precisa ver isso hoje.").is_empty());
        assert!(find("I said no to the offer.").is_empty());
    }
}
