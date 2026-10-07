//! Post-processing of AI output: strip wrappers models like to add, and reject results that
//! clearly are not a rewrite (e.g. the model answered the question instead).

use super::ProcessError;

pub fn clean_output(output: &str, input: &str, allow_markdown: bool) -> Result<String, ProcessError> {
    let mut text = output.trim().to_string();

    // Small models like to wrap code names in backticks even when told not to; that looks
    // wrong in chat apps and documents, so strip them unless Markdown is wanted.
    if !allow_markdown && !input.contains('`') {
        text = text.replace('`', "");
    }

    // Echoed tags.
    for tag in ["<transcript>", "</transcript>", "<rewritten>", "</rewritten>", "<output>", "</output>"] {
        text = text.replace(tag, "");
    }
    text = text.trim().to_string();

    // Leading labels such as "Rewritten text:".
    let lower = text.to_lowercase();
    for label in ["rewritten text:", "rewritten:", "cleaned text:", "output:", "here is the rewritten text:", "texto reescrito:"] {
        if lower.starts_with(label) {
            text = text[label.len()..].trim().to_string();
            break;
        }
    }

    // Surrounding quotes the input did not have.
    let input_quoted = input.trim().starts_with('"');
    if !input_quoted && text.len() >= 2 {
        let quoted = (text.starts_with('"') && text.ends_with('"'))
            || (text.starts_with('“') && text.ends_with('”'))
            || (text.starts_with('\'') && text.ends_with('\'') && !text[1..text.len() - 1].contains('\''));
        if quoted {
            let first = text.chars().next().unwrap().len_utf8();
            let last = text.chars().last().unwrap().len_utf8();
            text = text[first..text.len() - last].trim().to_string();
        }
    }

    if text.is_empty() {
        return Err(ProcessError::BadOutput);
    }

    // A rewrite should be roughly as long as the input. A far longer result means the model
    // generated content (answered, elaborated) rather than editing.
    let in_len = input.chars().count();
    let out_len = text.chars().count();
    if out_len > (in_len * 3).max(in_len + 300) {
        log::warn!("rejecting AI output: {out_len} chars for {in_len} chars of input");
        return Err(ProcessError::BadOutput);
    }
    Ok(text)
}

/// Joins lines into one so a multi-line result cannot execute commands in a terminal.
pub fn single_line(text: &str) -> String {
    text.lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_wrappers() {
        assert_eq!(clean_output("\"Hello there.\"", "hello there", false).unwrap(), "Hello there.");
        assert_eq!(clean_output("Call `foo` now.", "call foo now", false).unwrap(), "Call foo now.");
        assert_eq!(clean_output("Call `foo` now.", "call foo now", true).unwrap(), "Call `foo` now.");
        assert_eq!(clean_output("Rewritten text: Hi.", "hi", false).unwrap(), "Hi.");
        assert_eq!(clean_output("<transcript>\nHi.\n</transcript>", "hi", false).unwrap(), "Hi.");
    }

    #[test]
    fn rejects_answers() {
        let long = "Paris is the capital of France. ".repeat(20);
        assert!(clean_output(&long, "what's the capital of France", false).is_err());
        assert!(clean_output("   ", "x", false).is_err());
    }

    #[test]
    fn single_line_join() {
        assert_eq!(single_line("Do this:\n- a\n- b"), "Do this: a b");
    }
}
