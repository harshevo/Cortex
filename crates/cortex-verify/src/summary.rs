//! Shortening a failed check's output.
//!
//! Test runners print colour codes, progress lines and long stack traces.
//! The model needs the failures, which are nearly always at the end.

/// Longest output passed on to the model.
const MAX_CHARS: usize = 6_000;

/// How much of the budget goes to the start of the output.
const HEAD_CHARS: usize = 1_500;

pub fn summarize(output: &str) -> String {
    let clean = strip_ansi(output);

    let lines: Vec<&str> = clean
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();

    let text = lines.join("\n");

    let total = text.chars().count();

    if total <= MAX_CHARS {
        return text;
    }

    let tail_chars = MAX_CHARS - HEAD_CHARS;

    let head: String = text.chars().take(HEAD_CHARS).collect();
    let tail: String = text.chars().skip(total - tail_chars).collect();

    format!(
        "{head}\n\n[... {} characters omitted ...]\n\n{tail}",
        total - MAX_CHARS
    )
}

/// Removes terminal colour and cursor sequences (`ESC [ ... letter`).
pub fn strip_ansi(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        if character != '\x1b' {
            output.push(character);

            continue;
        }

        if characters.peek() != Some(&'[') {
            continue;
        }

        characters.next();

        for code in characters.by_ref() {
            if code.is_ascii_alphabetic() {
                break;
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_blank_lines_are_removed() {
        let output = "\x1b[31mFAIL\x1b[0m tests/a.test.ts\n\n\n  expected 3\n";

        assert_eq!(summarize(output), "FAIL tests/a.test.ts\n  expected 3");
    }

    #[test]
    fn long_output_keeps_the_end() {
        let output = format!(
            "start\n{}\nAssertionError: the end",
            "noise\n".repeat(5_000)
        );

        let summary = summarize(&output);

        assert!(summary.chars().count() < MAX_CHARS + 100);
        assert!(summary.starts_with("start"));
        assert!(summary.ends_with("AssertionError: the end"));
    }
}
