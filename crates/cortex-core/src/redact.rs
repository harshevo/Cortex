//! Keeping secrets out of what is written down.
//!
//! A run's log holds every command and every output, so a key printed by
//! `env` or read from a `.env` file would otherwise be kept on disk for
//! good. Before a line is written, anything that looks like a secret is
//! replaced by [`MARK`]:
//!
//! - tokens with a known prefix (`sk-`, `ghp_`, `AKIA`, `xoxb-`, …);
//! - the value after `Bearer `;
//! - the value given to a name that says it is secret
//!   (`API_KEY=…`, `"password": "…"`, `client_secret: …`);
//! - private key blocks.
//!
//! The replacement holds no quote or backslash, so it can be applied to
//! JSON text without breaking it. Short values are left alone: a secret
//! is long, and a short value is more often a placeholder or a flag.

use std::borrow::Cow;

pub const MARK: &str = "[redacted]";

/// Prefixes of tokens issued by well-known services.
const PREFIXES: [&str; 14] = [
    "sk-ant-",
    "sk-proj-",
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
    "hf_",
];

/// Words that, ending a name, say its value is secret.
const SECRET_NAMES: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "private_key",
];

/// A token after a known prefix is at least this long.
const MIN_TOKEN: usize = 16;

/// A secret value after a name is at least this long.
const MIN_VALUE: usize = 8;

/// `text` with every secret found replaced.
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut changed = false;
    let mut copied = 0;
    let bytes = text.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if let Some(end) = secret_at(text, index) {
            out.push_str(&text[copied..end.start]);
            out.push_str(MARK);
            copied = end.end;
            index = end.end;
            changed = true;

            continue;
        }

        index += text[index..].chars().next().map_or(1, char::len_utf8);
    }

    if !changed {
        return Cow::Borrowed(text);
    }

    out.push_str(&text[copied..]);

    Cow::Owned(out)
}

/// The part of `text` to replace, when a secret begins at `at`.
fn secret_at(text: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let rest = &text[at..];

    // A token or name starts a word. In JSON text a line break is `\n`,
    // so a word after one follows a letter.
    let before = &text[..at];
    let starts_word = at == 0
        || !is_token(before.chars().next_back()?)
        || ["\\n", "\\t", "\\r"]
            .iter()
            .any(|escape| before.ends_with(escape));

    if rest.starts_with("-----BEGIN ") && rest[..rest.len().min(60)].contains("PRIVATE KEY") {
        let end = rest.find("-----END ").and_then(|end| {
            rest[end + 9..]
                .find("-----")
                .map(|close| end + 9 + close + 5)
        })?;

        return Some(at..at + end);
    }

    if !starts_word {
        return None;
    }

    for prefix in PREFIXES {
        if let Some(after) = rest.strip_prefix(prefix) {
            let length = token_length(after);

            if length >= MIN_TOKEN {
                return Some(at..at + prefix.len() + length);
            }
        }
    }

    if let Some(after) = rest.strip_prefix("Bearer ") {
        let length = after
            .char_indices()
            .find(|(_, c)| !(is_token(*c) || matches!(c, '.' | '/' | '+' | '=')))
            .map_or(after.len(), |(index, _)| index);

        if length >= MIN_TOKEN {
            let start = at + "Bearer ".len();

            return Some(start..start + length);
        }
    }

    secret_value(text, at)
}

/// The value given to a secret-sounding name that begins at `at`.
fn secret_value(text: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let rest = &text[at..];

    let name_length = rest
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
        .map_or(rest.len(), |(index, _)| index);

    if name_length == 0 {
        return None;
    }

    let name = rest[..name_length].to_ascii_lowercase();

    if !SECRET_NAMES.iter().any(|secret| name.ends_with(secret)) {
        return None;
    }

    // What separates the name from its value: `=`, `:`, quotes (plain or
    // escaped inside JSON) and spaces.
    let after_name = &rest[name_length..];

    let separator = after_name
        .char_indices()
        .find(|(_, c)| !matches!(c, '=' | ':' | '"' | '\'' | '\\' | ' '))
        .map_or(after_name.len(), |(index, _)| index);

    let gap = &after_name[..separator];

    if !gap.contains('=') && !gap.contains(':') {
        return None;
    }

    let start = at + name_length + separator;
    let length = value_length(&text[start..]);

    (length >= MIN_VALUE).then_some(start..start + length)
}

fn is_token(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn token_length(text: &str) -> usize {
    text.char_indices()
        .find(|(_, c)| !is_token(*c))
        .map_or(text.len(), |(index, _)| index)
}

/// A value runs to the first quote, backslash, space or separator.
fn value_length(text: &str) -> usize {
    text.char_indices()
        .find(|(_, c)| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ',' | ';' | '&' | '}' | ')')
        })
        .map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tokens_are_replaced() {
        let text = "key sk-ant-api03-abcdefghijklmnopqrstuv and ghp_ABCDEFGHIJKLMNOPQRSTUVWX1234";

        assert_eq!(redact(text), "key [redacted] and [redacted]");
    }

    #[test]
    fn values_of_secret_names_are_replaced() {
        assert_eq!(
            redact("export OPENAI_API_KEY=abc123def456ghi"),
            "export OPENAI_API_KEY=[redacted]"
        );

        assert_eq!(
            redact(r#"{\"password\":\"hunter2hunter2\",\"user\":\"ann\"}"#),
            r#"{\"password\":\"[redacted]\",\"user\":\"ann\"}"#
        );

        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload"),
            "Authorization: Bearer [redacted]"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let text = "the token count was 12; skip the task-runner; password=short";

        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn private_keys_are_replaced_whole() {
        let text = "a\\n-----BEGIN OPENSSH PRIVATE KEY-----\\nb3BlbnNzaA\\n-----END OPENSSH PRIVATE KEY-----\\nz";

        assert_eq!(redact(text), "a\\n[redacted]\\nz");
    }

    #[test]
    fn redacted_json_still_parses() {
        let value = serde_json::json!({ "output": "TOKEN=abcdefghijkl\nsk-proj-AAAAAAAAAAAAAAAAAAAA" });
        let text = serde_json::to_string(&value).unwrap();

        let redacted = redact(&text);
        let back: serde_json::Value = serde_json::from_str(&redacted).unwrap();

        assert_eq!(back["output"], "TOKEN=[redacted]\n[redacted]");
    }
}
