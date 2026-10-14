//! Skills: written procedures for particular kinds of task.
//!
//! A skill is a Markdown file with a short header:
//!
//! ```text
//! ---
//! name: code-review
//! description: Review local changes for real bugs.
//! argument-hint: "[path | branch]"
//! ---
//!
//! What to do, step by step. `$ARGUMENTS` is replaced by what was typed
//! after the name.
//! ```
//!
//! They come from three places, later ones replacing earlier ones of the
//! same name: those built into this crate (`skills/`), the person's own in
//! `~/.cortex/skills`, and the project's in `.cortex/skills`. On disk a
//! skill is either `<name>/SKILL.md` or `<name>.md`.
//!
//! A skill reaches the model in two ways. The person types `/name ...` and
//! [`SkillSource`] puts its text in front of the task. Or, with the list
//! from [`SkillLibrary::listing`] in the system prompt, the model asks for
//! one itself through the [`UseSkill`] tool.

mod source;
mod tool;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub use source::SkillSource;
pub use tool::UseSkill;

/// Folder, inside the project or the home directory, that holds skills.
const SKILLS_DIRECTORY: &str = ".cortex/skills";

/// The file that holds a skill kept in a folder of its own.
const SKILL_FILE: &str = "SKILL.md";

/// Where the text of the skill's arguments goes.
const ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";

const BUILT_IN: &[(&str, &str)] = &[
    ("teach", include_str!("../skills/teach/SKILL.md")),
    ("browser", include_str!("../skills/browser/SKILL.md")),
    (
        "code-review",
        include_str!("../skills/code-review/SKILL.md"),
    ),
    ("commit", include_str!("../skills/commit/SKILL.md")),
    ("fix-ci", include_str!("../skills/fix-ci/SKILL.md")),
    ("fix-issue", include_str!("../skills/fix-issue/SKILL.md")),
    ("pr", include_str!("../skills/pr/SKILL.md")),
    (
        "pr-comments",
        include_str!("../skills/pr-comments/SKILL.md"),
    ),
    ("review", include_str!("../skills/review/SKILL.md")),
    (
        "security-review",
        include_str!("../skills/security-review/SKILL.md"),
    ),
    ("ui-design", include_str!("../skills/ui-design/SKILL.md")),
];

/// Where a skill was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,

    /// `~/.cortex/skills`.
    User,

    /// `.cortex/skills` in the project.
    Project,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "built in",
            Self::User => "yours",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,

    /// One sentence saying what it is for, shown in the list.
    pub description: String,

    /// What may follow the name, for the person typing it.
    pub argument_hint: Option<String>,

    /// The procedure itself.
    pub body: String,

    pub origin: Origin,
}

impl Skill {
    /// Reads a skill from its file's text. `fallback_name` is used when
    /// the header gives none.
    pub fn parse(text: &str, fallback_name: &str, origin: Origin) -> Self {
        let (header, body) = split_header(text);

        let field = |key: &str| {
            header
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .filter(|value| !value.is_empty())
        };

        Self {
            name: field("name").unwrap_or_else(|| fallback_name.to_string()),
            description: field("description").unwrap_or_default(),
            argument_hint: field("argument-hint"),
            body: body.trim().to_string(),
            origin,
        }
    }

    /// The procedure with the arguments filled in. A skill that does not
    /// say where they go gets them on a line at the end.
    pub fn render(&self, arguments: &str) -> String {
        let arguments = arguments.trim();

        if self.body.contains(ARGUMENTS_PLACEHOLDER) {
            let shown = if arguments.is_empty() {
                "(none)"
            } else {
                arguments
            };

            return self.body.replace(ARGUMENTS_PLACEHOLDER, shown);
        }

        if arguments.is_empty() {
            return self.body.clone();
        }

        format!("{}\n\nArguments: {arguments}", self.body)
    }
}

/// The skills that can be used in one place, by name.
#[derive(Debug, Clone, Default)]
pub struct SkillLibrary {
    skills: BTreeMap<String, Skill>,
}

impl SkillLibrary {
    /// The built-in skills and the person's own: everything that does not
    /// depend on which project is open.
    pub fn standard() -> Self {
        let mut library = Self::default();

        for (name, text) in BUILT_IN {
            library.add(Skill::parse(text, name, Origin::BuiltIn));
        }

        if let Some(home) = home_directory() {
            library.add_directory(&home.join(SKILLS_DIRECTORY), Origin::User);
        }

        library
    }

    /// [`Self::standard`], plus the skills the project at `root` brings.
    pub fn for_project(root: &Path) -> Self {
        let mut library = Self::standard();

        library.add_directory(&root.join(SKILLS_DIRECTORY), Origin::Project);

        library
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Every skill, in name order.
    pub fn skills(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    pub fn names(&self) -> Vec<&str> {
        self.skills.keys().map(String::as_str).collect()
    }

    /// A paragraph for the system prompt that tells the model which skills
    /// exist and how to use one. `None` when there are none.
    pub fn listing(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }

        let mut text = String::from(
            "Skills are written procedures for particular kinds of task. When a task matches \
             one, call the `skill` tool with its name before you start and follow what it \
             returns. When the task already comes with a skill's text (the person started it \
             with `/name`), do not load it again.\n",
        );

        for skill in self.skills() {
            text.push_str(&format!("\n- {}: {}", skill.name, skill.description));
        }

        Some(text)
    }

    /// As [`Self::listing`], for only the skills the project brings. They
    /// are listed apart so that the rest of the system prompt stays the
    /// same from project to project.
    pub fn project_listing(&self) -> Option<String> {
        let mut text = String::from("This project adds its own skills:\n");

        let mut any = false;

        for skill in self
            .skills()
            .filter(|skill| skill.origin == Origin::Project)
        {
            text.push_str(&format!("\n- {}: {}", skill.name, skill.description));

            any = true;
        }

        any.then_some(text)
    }

    fn add(&mut self, skill: Skill) {
        if is_valid_name(&skill.name) {
            self.skills.insert(skill.name.clone(), skill);
        }
    }

    /// Adds every skill in `directory`. A folder that is missing or cannot
    /// be read simply adds nothing.
    fn add_directory(&mut self, directory: &Path, origin: Origin) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };

        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();

        paths.sort();

        for path in paths {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };

            let file = if path.is_dir() {
                path.join(SKILL_FILE)
            } else if path.extension().is_some_and(|extension| extension == "md") {
                path.clone()
            } else {
                continue;
            };

            if let Ok(text) = fs::read_to_string(&file) {
                self.add(Skill::parse(&text, stem, origin));
            }
        }
    }
}

/// An instruction that starts a skill: `/name` and whatever follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

impl<'a> Invocation<'a> {
    /// `None` unless `instruction` begins with a slash and a skill-like
    /// name, so that an instruction that merely starts with a path
    /// (`/usr/lib is missing ...`) is left alone.
    pub fn parse(instruction: &'a str) -> Option<Self> {
        let rest = instruction.trim_start().strip_prefix('/')?;

        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());

        let (name, arguments) = rest.split_at(end);

        is_valid_name(name).then_some(Self {
            name,
            arguments: arguments.trim(),
        })
    }
}

/// Lower-case letters, digits, `-` and `_`, starting with a letter.
fn is_valid_name(name: &str) -> bool {
    let mut characters = name.chars();

    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || "-_".contains(character)
        })
}

/// Splits a skill file into its header's `key: value` lines and the rest.
/// A file without a header is all body.
fn split_header(text: &str) -> (Vec<(String, String)>, &str) {
    let Some(after_open) = text.trim_start().strip_prefix("---") else {
        return (Vec::new(), text);
    };

    let Some(close) = after_open.find("\n---") else {
        return (Vec::new(), text);
    };

    let header = &after_open[..close];

    // Past the closing marker and the rest of its line.
    let body = after_open[close + 4..]
        .split_once('\n')
        .map_or("", |(_, body)| body);

    let fields = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), unquote(value.trim()).to_string()))
        .collect();

    (fields, body)
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }

    value
}

fn home_directory() -> Option<PathBuf> {
    std::env::home_dir().filter(|home| !home.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_skill_has_a_header_that_matches_its_folder() {
        for (name, text) in BUILT_IN {
            let skill = Skill::parse(text, "fallback", Origin::BuiltIn);

            assert_eq!(skill.name, *name);
            assert!(!skill.description.is_empty(), "{name} has no description");
            assert!(skill.body.len() > 200, "{name} has no body");
            assert!(!skill.body.starts_with("---"), "{name} kept its header");
        }
    }

    #[test]
    fn a_header_is_read_and_removed() {
        let text = "---\nname: tidy\ndescription: \"Tidy: up\"\n---\n\nDo it.\n";

        let skill = Skill::parse(text, "other", Origin::User);

        assert_eq!(skill.name, "tidy");
        assert_eq!(skill.description, "Tidy: up");
        assert_eq!(skill.body, "Do it.");
    }

    #[test]
    fn a_file_without_a_header_takes_its_name_from_the_file() {
        let skill = Skill::parse("Just do it.", "plain", Origin::Project);

        assert_eq!(skill.name, "plain");
        assert_eq!(skill.body, "Just do it.");
    }

    #[test]
    fn arguments_replace_the_placeholder_or_are_appended() {
        let with = Skill::parse("Review $ARGUMENTS now.", "a", Origin::User);
        let without = Skill::parse("Review.", "b", Origin::User);

        assert_eq!(with.render(" 12 "), "Review 12 now.");
        assert_eq!(with.render(""), "Review (none) now.");
        assert_eq!(without.render("12"), "Review.\n\nArguments: 12");
        assert_eq!(without.render(""), "Review.");
    }

    #[test]
    fn an_invocation_is_a_slash_and_a_name() {
        assert_eq!(
            Invocation::parse("/review 12 --comment"),
            Some(Invocation {
                name: "review",
                arguments: "12 --comment"
            })
        );

        assert_eq!(
            Invocation::parse("  /code-review\n"),
            Some(Invocation {
                name: "code-review",
                arguments: ""
            })
        );

        assert_eq!(Invocation::parse("/usr/lib is missing"), None);
        assert_eq!(Invocation::parse("fix /review"), None);
        assert_eq!(Invocation::parse("/"), None);
    }

    #[test]
    fn a_project_skill_replaces_a_built_in_one_of_the_same_name() {
        let project = tempfile::tempdir().unwrap();
        let skills = project.path().join(SKILLS_DIRECTORY);

        fs::create_dir_all(skills.join("review")).unwrap();

        fs::write(
            skills.join("review").join(SKILL_FILE),
            "---\ndescription: Ours.\n---\nOur way.",
        )
        .unwrap();

        fs::write(skills.join("deploy.md"), "Ship it.").unwrap();
        fs::write(skills.join("notes.txt"), "not a skill").unwrap();

        let library = SkillLibrary::for_project(project.path());

        let review = library.get("review").unwrap();

        assert_eq!(review.origin, Origin::Project);
        assert_eq!(review.body, "Our way.");
        assert_eq!(library.get("deploy").unwrap().body, "Ship it.");
        assert!(library.get("notes").is_none());
        assert_eq!(library.get("commit").unwrap().origin, Origin::BuiltIn);

        let listing = library.project_listing().unwrap();

        assert!(listing.contains("- review: Ours."));
        assert!(!listing.contains("commit"));
    }
}
