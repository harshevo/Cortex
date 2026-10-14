//! The optional capabilities as a person meets them: a name to call each
//! by, a line that says what it does, and the heading it is listed under.
//!
//! The engine knows a capability by its `--with` name and explains it at
//! length. Here each is found by that name, so that one the engine gains
//! later is still listed, under its own name and the first sentence of
//! the engine's explanation, until it is given a place here.

use std::borrow::Cow;

use clap::ValueEnum;
use cortex_cli::features::Feature;

use super::{capability_help, capability_name};

/// The headings capabilities are listed under, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Understanding,
    Spending,
    Memory,
    Checking,
    Learning,
    Tools,
    Experimental,
}

impl Group {
    pub const ALL: [Self; 7] = [
        Self::Understanding,
        Self::Spending,
        Self::Memory,
        Self::Checking,
        Self::Learning,
        Self::Tools,
        Self::Experimental,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Understanding => "Understanding the project",
            Self::Spending => "Spending less",
            Self::Memory => "Memory",
            Self::Checking => "Checking the work",
            Self::Learning => "Learning",
            Self::Tools => "Tools",
            Self::Experimental => "Experimental",
        }
    }
}

/// What is known here about one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct About {
    pub feature: Feature,

    /// Its `--with` name.
    pub name: String,

    /// What a person would call it.
    pub label: Cow<'static, str>,

    /// One line on what it does.
    pub help: Cow<'static, str>,

    pub group: Group,

    /// Rarely wanted: listed folded away under its heading.
    pub advanced: bool,
}

/// `(name, label, one line of help, heading, rarely wanted)`.
type Row = (&'static str, &'static str, &'static str, Group, bool);

const ROWS: &[Row] = &[
    (
        "index",
        "Code index",
        "Tools that outline a file and read one symbol from it.",
        Group::Understanding,
        false,
    ),
    (
        "semantic",
        "Search by meaning",
        "Search code by what it means. Needs the embedding service from ml/ running.",
        Group::Understanding,
        true,
    ),
    (
        "map",
        "Project map",
        "Show the model every source file and the names it defines before it starts.",
        Group::Understanding,
        false,
    ),
    (
        "graph",
        "Code graph",
        "Tools that find where a name is used and what a file depends on.",
        Group::Understanding,
        false,
    ),
    (
        "router",
        "Context router",
        "Gather context for the task before the model starts, from whatever else is on.",
        Group::Understanding,
        true,
    ),
    (
        "likely",
        "Likely files",
        "Show the files the task is probably about.",
        Group::Understanding,
        false,
    ),
    (
        "prune",
        "Drop replaced output",
        "Remove tool output that a later, identical call has replaced.",
        Group::Spending,
        true,
    ),
    (
        "lean",
        "Lean tool output",
        "Return less text from the basic tools: outlines, changed lines, test summaries.",
        Group::Spending,
        false,
    ),
    (
        "batch",
        "Batched lookups",
        "Ask the model to put independent lookups in one response.",
        Group::Spending,
        false,
    ),
    (
        "mask",
        "Mask old output",
        "Replace tool output older than ten steps with one line. Not for Anthropic models.",
        Group::Spending,
        true,
    ),
    (
        "compact",
        "Compact long conversations",
        "When the context window is nearly full, replace the older part with a summary.",
        Group::Spending,
        false,
    ),
    (
        "memory",
        "Memory",
        "Remember earlier tasks in this project and show them before a new one.",
        Group::Memory,
        false,
    ),
    (
        "hope",
        "HOPE re-ranking",
        "Let the HOPE memory model re-score recalled memories. Needs its service running.",
        Group::Memory,
        true,
    ),
    (
        "lessons",
        "Lessons",
        "After each task, write down what a later task should know. One extra model call.",
        Group::Memory,
        false,
    ),
    (
        "priors",
        "Priors",
        "Tell the model how often a first answer has passed the checks here.",
        Group::Memory,
        true,
    ),
    (
        "gate",
        "Checks gate",
        "Run the project's checks before a task counts as done; failures go back to the model.",
        Group::Checking,
        false,
    ),
    (
        "review",
        "Review against the request",
        "Hold the finished changes against the request, once. One extra model call.",
        Group::Checking,
        false,
    ),
    (
        "repro",
        "Reproduce first",
        "Ask the model to reproduce a problem with a failing test before changing code.",
        Group::Checking,
        false,
    ),
    (
        "rollouts",
        "Several attempts",
        "Make several attempts, each in its own copy of the project, and keep the best.",
        Group::Checking,
        false,
    ),
    (
        "mind",
        "Mind",
        "A learning brain that watches each step, keeps your corrections and guesses your \
         next request.",
        Group::Learning,
        false,
    ),
    (
        "brain",
        "Dendritic network",
        "The network that grows as it watches runs, allowed to advise the model between \
         steps. The same network as Living network, which only watches.",
        Group::Learning,
        false,
    ),
    (
        "intuition",
        "Intuition",
        "Recalls which files, commands and errors went together in runs that passed.",
        Group::Learning,
        false,
    ),
    (
        "controller",
        "Decision controller",
        "Learns which kind of next action pays off. Cannot be on together with the \
         dendritic network.",
        Group::Learning,
        true,
    ),
    (
        "skills",
        "Skills",
        "Let the model pick a written procedure, such as code-review, by itself.",
        Group::Tools,
        false,
    ),
    (
        "github",
        "GitHub",
        "Read pull requests, issues and checks through gh; writing asks first.",
        Group::Tools,
        false,
    ),
    (
        "browser",
        "Browser",
        "A real browser the agent can open pages in, click, type and take screenshots of.",
        Group::Tools,
        false,
    ),
    (
        "living",
        "Living network",
        "The same network as Dendritic network, only watching: it grows and nothing it \
         learns reaches the model.",
        Group::Experimental,
        true,
    ),
];

/// Every capability the engine has, in the order of the headings.
pub fn all() -> Vec<About> {
    let mut all: Vec<About> = Feature::value_variants()
        .iter()
        .copied()
        .map(about)
        .collect();

    // Stable, so that within a heading they keep the engine's order.
    all.sort_by_key(|about| Group::ALL.iter().position(|group| *group == about.group));

    all
}

pub fn about(feature: Feature) -> About {
    let name = capability_name(feature);

    match ROWS.iter().find(|row| row.0 == name) {
        Some(&(_, label, help, group, advanced)) => About {
            feature,
            name,
            label: Cow::Borrowed(label),
            help: Cow::Borrowed(help),
            group,
            advanced,
        },

        // One the engine has gained since: its own name and words.
        None => About {
            feature,
            label: Cow::Owned(name.replace('-', " ")),
            help: Cow::Owned(one_sentence(&capability_help(feature))),
            name,
            group: Group::Experimental,
            advanced: true,
        },
    }
}

/// The first sentence of a longer explanation.
pub fn one_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    match flat.split_once(". ") {
        Some((first, _)) => format!("{first}."),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_is_listed_once_under_a_heading() {
        let all = all();

        assert_eq!(all.len(), Feature::value_variants().len());

        for about in &all {
            assert!(!about.label.is_empty(), "{}", about.name);
            assert!(!about.help.is_empty(), "{}", about.name);
            assert!(about.help.len() <= 160, "{}: {}", about.name, about.help);
        }

        // Headings come in their order, each in one run.
        let groups: Vec<Group> = all.iter().map(|about| about.group).collect();
        let mut seen = groups.clone();

        seen.dedup();

        assert!(seen.len() <= Group::ALL.len(), "{groups:?}");
    }

    #[test]
    fn every_row_names_a_capability_the_engine_has() {
        let names: Vec<String> = Feature::value_variants()
            .iter()
            .map(|feature| capability_name(*feature))
            .collect();

        for row in ROWS {
            assert!(
                names.iter().any(|name| name == row.0),
                "{} is not one",
                row.0
            );
        }
    }

    /// The same network is behind `brain` and `living`; the two lines say so.
    const SAME_NETWORK: &str = "The same network as";

    #[test]
    fn the_two_names_of_the_one_network_say_so() {
        let brain = about(Feature::Brain);
        let living = about(Feature::Living);

        assert_eq!(brain.label, "Dendritic network");
        assert_eq!(living.label, "Living network");

        assert!(brain.help.contains(SAME_NETWORK) && brain.help.contains("Living network"));
        assert!(living.help.contains(SAME_NETWORK) && living.help.contains("Dendritic network"));
    }

    #[test]
    fn help_is_cut_to_its_first_sentence() {
        assert_eq!(
            one_sentence("Trim old output. It saves\ntokens."),
            "Trim old output."
        );
        assert_eq!(one_sentence("No full stop"), "No full stop");
    }
}
