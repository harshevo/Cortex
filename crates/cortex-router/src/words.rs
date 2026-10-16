use std::collections::HashSet;

const STOPWORDS: [&str; 28] = [
    "the", "and", "for", "that", "this", "with", "from", "are", "was", "not", "but", "has", "have",
    "its", "can", "should", "when", "which", "into", "then", "they", "them", "one", "all", "any",
    "fix", "bug", "does",
];

pub fn words(text: &str) -> HashSet<String> {
    let mut words = HashSet::new();
    let mut current = String::new();
    let mut previous_lower = false;

    let mut flush = |current: &mut String| {
        if current.chars().count() >= 3 && !STOPWORDS.contains(&current.as_str()) {
            words.insert(std::mem::take(current));
        } else {
            current.clear();
        }
    };

    for character in text.chars() {
        if !character.is_alphanumeric() {
            flush(&mut current);

            previous_lower = false;

            continue;
        }

        // A capital after a lower-case letter starts a new word.
        if character.is_uppercase() && previous_lower {
            flush(&mut current);
        }

        previous_lower = character.is_lowercase();

        current.extend(character.to_lowercase());
    }

    flush(&mut current);

    words
}
