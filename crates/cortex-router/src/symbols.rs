//! Finding the code names a task mentions.

/// A name shorter than this is too likely to be an ordinary word.
const MIN_NAME_CHARS: usize = 4;

/// The words in `instruction` that look like identifiers, in order and
/// without repeats: anything in backticks, and bare words written in
/// `camelCase`, `PascalCase` or `snake_case`.
///
/// These are candidates only. The caller keeps the ones that are actually
/// defined in the project.
pub fn candidate_names(instruction: &str) -> Vec<String> {
    let mut names = Vec::new();

    let mut add = |name: &str| {
        let name = name.trim_end_matches("()").trim();

        let is_identifier = !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_alphanumeric() || character == '_');

        if is_identifier
            && name.chars().count() >= MIN_NAME_CHARS
            && !names.iter().any(|n| n == name)
        {
            names.push(name.to_string());
        }
    };

    // Text between backticks, which is how people mark code.
    for (position, span) in instruction.split('`').enumerate() {
        // A span with spaces is a phrase or a command, not a name.
        if position % 2 == 1 && !span.trim().contains(char::is_whitespace) {
            // `Cart.total` names both the class and the method.
            for part in span.split(['.', ':', '(']) {
                add(part);
            }
        }
    }

    for word in
        instruction.split(|character: char| !(character.is_alphanumeric() || character == '_'))
    {
        if looks_like_code(word) {
            add(word);
        }
    }

    names
}

/// `snake_case`, or a capital letter after a lower-case one.
fn looks_like_code(word: &str) -> bool {
    let has_inner_underscore = word.trim_matches('_').contains('_');

    let has_inner_capital = word
        .chars()
        .zip(word.chars().skip(1))
        .any(|(previous, current)| previous.is_lowercase() && current.is_uppercase());

    has_inner_underscore || has_inner_capital
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backticked_and_code_shaped_words_are_found() {
        let names = candidate_names(
            "`withBinanceClosingQuantities` is not implemented. It is called from \
             fetchBinanceFills and uses to_cents and `Cart.total()`. Run `npx vitest run`.",
        );

        assert_eq!(
            names,
            [
                "withBinanceClosingQuantities",
                "Cart",
                "total",
                "fetchBinanceFills",
                "to_cents"
            ]
        );
    }

    #[test]
    fn ordinary_prose_names_nothing() {
        assert!(candidate_names("Evening trades show up under the wrong day.").is_empty());
    }
}
