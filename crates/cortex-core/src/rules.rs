//! The project's and the person's standing instructions.
//!
//! Projects already write down how an agent should work in them, in files
//! other coding agents read: `AGENTS.md`, `CLAUDE.md`. Cortex reads those,
//! its own `CORTEX.md` and `.cortex/rules.md`, and the person's
//! `~/.cortex/rules.md`, and puts them in the system prompt. They change
//! rarely, so the prompt stays the same from request to request and the
//! provider's cache keeps working.

use std::path::Path;

/// Read in this order; each one found is used.
const PROJECT_FILES: [&str; 5] = [
    "AGENTS.md",
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    "CORTEX.md",
    ".cortex/rules.md",
];

/// Characters of one file at most, so one large file cannot crowd out the
/// conversation.
const MAX_CHARS: usize = 12_000;

/// One file of rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// Where it was read from, as shown to the model.
    pub source: String,

    pub text: String,
}

/// The rules for a project at `root`, the person's own first.
pub fn load(root: &Path, home: Option<&Path>) -> Vec<Rules> {
    let mut found = Vec::new();

    if let Some(home) = home {
        found.extend(read(&home.join(".cortex/rules.md"), "~/.cortex/rules.md"));
    }

    for file in PROJECT_FILES {
        found.extend(read(&root.join(file), file));
    }

    found
}

fn read(path: &Path, shown: &str) -> Option<Rules> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();

    if text.is_empty() {
        return None;
    }

    let text = match text.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!(
            "{}\n\n[... the rest of the file is left out ...]",
            &text[..cut]
        ),
        None => text.to_string(),
    };

    Some(Rules {
        source: shown.to_string(),
        text,
    })
}

/// The rules as one paragraph of the system prompt. `None` without any.
pub fn note(rules: &[Rules]) -> Option<String> {
    if rules.is_empty() {
        return None;
    }

    let mut text = String::from(
        "Standing instructions for this project, from its own files and the person's settings. \
         Follow them unless the task says otherwise; where they disagree, the project's files \
         win over the person's general rules, and the task wins over both.",
    );

    for rules in rules {
        text.push_str(&format!(
            "\n\n<rules source=\"{}\">\n{}\n</rules>",
            rules.source, rules.text
        ));
    }

    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_file_is_read_and_the_persons_come_first() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        std::fs::write(project.path().join("AGENTS.md"), "Use pnpm.").unwrap();
        std::fs::create_dir_all(project.path().join(".cortex")).unwrap();
        std::fs::write(project.path().join(".cortex/rules.md"), "No unwrap.").unwrap();
        std::fs::create_dir_all(home.path().join(".cortex")).unwrap();
        std::fs::write(home.path().join(".cortex/rules.md"), "Be brief.").unwrap();
        std::fs::write(project.path().join("CLAUDE.md"), "  ").unwrap();

        let rules = load(project.path(), Some(home.path()));

        let sources: Vec<&str> = rules.iter().map(|rules| rules.source.as_str()).collect();

        assert_eq!(
            sources,
            ["~/.cortex/rules.md", "AGENTS.md", ".cortex/rules.md"]
        );

        let text = note(&rules).unwrap();

        assert!(text.contains("<rules source=\"AGENTS.md\">\nUse pnpm.\n</rules>"));
    }

    #[test]
    fn a_long_file_is_cut() {
        let project = tempfile::tempdir().unwrap();

        std::fs::write(project.path().join("AGENTS.md"), "x".repeat(MAX_CHARS + 50)).unwrap();

        let rules = load(project.path(), None);

        assert!(rules[0].text.ends_with("left out ...]"));
    }
}
