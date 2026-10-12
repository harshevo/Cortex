//! The things the network has neurons for, and how a run's events are
//! read as them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most words taken from one task.
const MAX_WORDS: usize = 32;

/// Longest command or error kept as a concept.
const MAX_TEXT: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A word of the task.
    Word,

    /// A file, by its path in the project.
    File,

    /// A command that was run.
    Command,

    /// A failure, by the line that names it.
    Error,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::File => "file",
            Self::Command => "command",
            Self::Error => "error",
        }
    }

    /// Whether activity in a neuron of this kind may strengthen a synapse
    /// onto one of kind `target`.
    ///
    /// Only what the model can act on is a target: a file to open, a
    /// command to run. Words and files lead to files; an error leads to
    /// the file or command that followed it.
    pub fn may_lead_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Word | Self::File | Self::Error, Self::File) | (Self::Error, Self::Command)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Concept {
    pub kind: Kind,
    pub text: String,
}

impl Concept {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// Words too common to tell one task from another.
const COMMON: &[&str] = &[
    "about", "after", "again", "also", "always", "been", "before", "being", "both", "cannot",
    "could", "does", "each", "even", "from", "have", "into", "just", "like", "make", "more",
    "most", "must", "never", "only", "other", "over", "same", "should", "some", "such", "than",
    "that", "their", "them", "then", "there", "these", "they", "this", "those", "through", "under",
    "until", "very", "what", "when", "where", "which", "while", "will", "with", "without", "would",
    "your",
];

/// The words of a task that may tell it from another: four letters or
/// more, lower case, a plural counted with its singular.
pub fn task_words(instruction: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();

    for piece in instruction.split(|character: char| !character.is_alphanumeric()) {
        // `withBinanceClosingQuantities` is four words.
        for part in split_camel(piece) {
            let mut word = part.to_lowercase();

            if word.len() > 5 && word.ends_with("ies") {
                word.truncate(word.len() - 3);
                word.push('y');
            } else if word.len() > 4 && word.ends_with('s') && !word.ends_with("ss") {
                word.pop();
            }

            let usable = word.len() >= 4
                && word.chars().any(char::is_alphabetic)
                && !COMMON.contains(&word.as_str());

            if usable && !words.contains(&word) {
                words.push(word);
            }
        }
    }

    words.truncate(MAX_WORDS);

    words
}

fn split_camel(piece: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;

    let characters: Vec<(usize, char)> = piece.char_indices().collect();

    for window in characters.windows(2) {
        let (_, before) = window[0];
        let (at, after) = window[1];

        if before.is_lowercase() && after.is_uppercase() {
            parts.push(&piece[start..at]);

            start = at;
        }
    }

    parts.push(&piece[start..]);

    parts
}

/// The file a tool call names, if it names one.
pub fn file_of(input: &Value) -> Option<String> {
    let path = input.get("path")?.as_str()?.trim().trim_start_matches("./");

    let name = path.rsplit('/').next()?;

    // A directory, or a scratch file outside the project, is not a file
    // to be reminded of.
    let is_file = name.contains('.') && !path.starts_with('/') && !path.starts_with("..");

    is_file.then(|| path.to_string())
}

/// The commands a shell line runs that are worth remembering: each part
/// between `&&` and `;`, without its pipes and redirections. Inline
/// scripts and plain file handling are left out.
pub fn commands_of(line: &str) -> Vec<String> {
    const PLAIN: &[&str] = &[
        "cat", "cd", "cp", "echo", "find", "grep", "head", "ls", "mkdir", "mv", "pwd", "rg", "rm",
        "sed", "tail", "touch", "wc", "which",
    ];

    if line.contains('\n') || line.contains("<<") {
        return Vec::new();
    }

    let mut commands: Vec<String> = Vec::new();

    for part in line.split("&&").flat_map(|part| part.split(';')) {
        let part = part.split('|').next().unwrap_or_default();
        let part = part.split("2>").next().unwrap_or_default();

        let command = part.split_whitespace().collect::<Vec<_>>().join(" ");

        let program = command.split(' ').next().unwrap_or_default();

        let usable = !command.is_empty()
            && command.len() <= MAX_TEXT
            && !command.contains(['\'', '"', '>', '<', '$'])
            && !PLAIN.contains(&program);

        if usable && !commands.contains(&command) {
            commands.push(command);
        }
    }

    commands
}

/// The line of a failed command's output that names the failure, with
/// numbers replaced so that the same failure reads the same each time.
pub fn error_of(output: &str) -> Option<String> {
    const SIGNS: &[&str] = &[
        "error",
        "Error",
        "FAIL",
        "failed",
        "Cannot",
        "cannot",
        "not found",
        "fatal",
        "panicked",
        "Exception",
    ];

    let line = output
        .lines()
        .skip_while(|line| line.starts_with("exit code:"))
        .map(strip_colour)
        .find(|line| SIGNS.iter().any(|sign| line.contains(sign)))?;

    let mut signature = String::new();
    let mut in_number = false;

    for character in line.trim().chars() {
        if character.is_ascii_digit() {
            if !in_number {
                signature.push('#');
            }

            in_number = true;
        } else {
            in_number = false;

            signature.push(character);
        }
    }

    let signature: String = signature.chars().take(MAX_TEXT).collect();

    (signature.len() >= 8).then_some(signature)
}

/// Removes terminal colour codes.
fn strip_colour(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();

    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            // Skip to the letter that ends the code.
            for code in characters.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }

    plain
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn task_words_are_the_telling_ones() {
        let words = task_words(
            "`withBinanceClosingQuantities` in the import code is not implemented. Fix the weeks.",
        );

        assert_eq!(
            words,
            [
                "binance",
                "closing",
                "quantity",
                "import",
                "code",
                "implemented",
                "week"
            ]
        );
    }

    #[test]
    fn a_tool_call_names_a_file_inside_the_project() {
        assert_eq!(
            file_of(&json!({ "path": "./src/money.ts" })).as_deref(),
            Some("src/money.ts")
        );

        assert_eq!(file_of(&json!({ "path": "src/features" })), None);
        assert_eq!(file_of(&json!({ "path": "/tmp/check.test.ts" })), None);
        assert_eq!(file_of(&json!({ "pattern": "week" })), None);
    }

    #[test]
    fn a_shell_line_is_read_as_its_commands() {
        assert_eq!(
            commands_of("npx vitest run 2>&1 | tail -20 && npx tsc --noEmit 2>&1 | tail -20"),
            ["npx vitest run", "npx tsc --noEmit"]
        );

        assert!(commands_of("cat package.json").is_empty());
        assert!(commands_of("node -e 'console.log(1)'").is_empty());
        assert!(commands_of("cat > /tmp/a.ts <<'EOF'\nx\nEOF").is_empty());
    }

    #[test]
    fn the_same_failure_reads_the_same() {
        let first =
            error_of("exit code: 1\n\nstdout:\nsrc/a.ts(12,5): error TS2322: Type mismatch");
        let second =
            error_of("exit code: 1\n\nstdout:\nsrc/a.ts(40,9): error TS2322: Type mismatch");

        assert_eq!(
            first.as_deref(),
            Some("src/a.ts(#,#): error TS#: Type mismatch")
        );
        assert_eq!(first, second);

        assert_eq!(
            error_of("exit code: 128\n\nstderr:\n\u{1b}[31mfatal: not a git repository\u{1b}[0m")
                .as_deref(),
            Some("fatal: not a git repository")
        );

        assert_eq!(error_of("exit code: 1\n\nstdout:\nnothing to see"), None);
    }
}
