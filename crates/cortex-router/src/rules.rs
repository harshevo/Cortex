//! The routing rules: what kind of task this is, and which context
//! sources are worth consulting for it.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// The project's own checks are failing and the task is to fix them.
    FailingChecks,

    /// Something is to be renamed, moved, or have its signature changed.
    Refactor,

    /// A described behaviour is wrong.
    BugFix,

    /// Something new is to be built.
    Feature,

    /// The user wants an answer, not a change.
    Question,

    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Definitions and call sites of the names the task mentions.
    Symbols,

    /// Code found from the task's description by embedding search.
    SemanticSearch,

    /// The current output of the project's failing checks.
    CheckOutput,
}

/// Which sources are switched on for this run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Available {
    pub graph: bool,
    pub semantic: bool,
    pub verifier: bool,
}

/// The router's decision for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    pub kind: TaskKind,
    pub sources: Vec<Source>,
}

const FAILING_CHECKS: [&str; 9] = [
    "test suite is failing",
    "tests are failing",
    "tests fail",
    "failing test",
    "tests do not pass",
    "build is broken",
    "build fails",
    "does not compile",
    "doesn't compile",
];

const REFACTOR: [&str; 7] = [
    "rename",
    "refactor",
    "signature",
    "move the",
    "extract",
    "inline the",
    "deprecate",
];

const QUESTION_OPENERS: [&str; 9] = [
    "what",
    "why",
    "how",
    "where",
    "which",
    "explain",
    "describe",
    "summarise",
    "summarize",
];

const EXPLICIT_FIX: [&str; 3] = ["fix", "bug", "regression"];

const FEATURE: [&str; 7] = [
    "implement",
    "add ",
    "support for",
    "create",
    "introduce",
    "new option",
    "build a",
];

const SYMPTOM: [&str; 10] = [
    "wrong",
    "incorrect",
    "should",
    "fails",
    "broken",
    "error",
    "crash",
    "missing",
    "instead of",
    "no longer",
];

pub fn classify(instruction: &str) -> TaskKind {
    let text = instruction.to_lowercase();

    let has = |phrases: &[&str]| phrases.iter().any(|phrase| text.contains(phrase));

    let first_word = text.split_whitespace().next().unwrap_or_default();

    if has(&FAILING_CHECKS) {
        TaskKind::FailingChecks
    } else if has(&REFACTOR) {
        TaskKind::Refactor
    } else if QUESTION_OPENERS.contains(&first_word) && !has(&EXPLICIT_FIX) {
        TaskKind::Question
    } else if has(&EXPLICIT_FIX) {
        TaskKind::BugFix
    } else if has(&FEATURE) {
        TaskKind::Feature
    } else if has(&SYMPTOM) {
        TaskKind::BugFix
    } else {
        TaskKind::Other
    }
}

/// Picks the sources for a task.
///
/// `names_code` says whether the task mentions a name that is defined in
/// the project. When it does, the place to start is known and searching by
/// meaning would only add noise.
pub fn route(kind: TaskKind, names_code: bool, available: Available) -> Route {
    let mut sources = Vec::new();

    if available.verifier && kind == TaskKind::FailingChecks {
        sources.push(Source::CheckOutput);
    }

    if available.graph && names_code {
        sources.push(Source::Symbols);
    }

    let describes_behaviour =
        matches!(kind, TaskKind::BugFix | TaskKind::Feature | TaskKind::Other);

    if available.semantic && describes_behaviour && !names_code {
        sources.push(Source::SemanticSearch);
    }

    Route { kind, sources }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERYTHING: Available = Available {
        graph: true,
        semantic: true,
        verifier: true,
    };

    #[test]
    fn tasks_are_classified_by_their_wording() {
        let cases = [
            (
                "The project's test suite is failing. Run it and fix the code.",
                TaskKind::FailingChecks,
            ),
            (
                "Rename `toCents` to `asCents` everywhere.",
                TaskKind::Refactor,
            ),
            ("Where are trades grouped into days?", TaskKind::Question),
            (
                "Evening trades show up under the wrong day. Fix it.",
                TaskKind::BugFix,
            ),
            (
                "`withBinanceClosingQuantities` is not implemented. Implement it.",
                TaskKind::Feature,
            ),
            (
                "A net P&L of -1.50 shows as a loss of 0.50, which is wrong.",
                TaskKind::BugFix,
            ),
            ("Tidy the README.", TaskKind::Other),
        ];

        for (instruction, expected) in cases {
            assert_eq!(classify(instruction), expected, "{instruction}");
        }
    }

    #[test]
    fn a_described_bug_is_searched_by_meaning() {
        let route = route(TaskKind::BugFix, false, EVERYTHING);

        assert_eq!(route.sources, [Source::SemanticSearch]);
    }

    #[test]
    fn a_named_symbol_replaces_the_search() {
        let route = route(TaskKind::Feature, true, EVERYTHING);

        assert_eq!(route.sources, [Source::Symbols]);
    }

    #[test]
    fn failing_checks_are_run_up_front() {
        let route = route(TaskKind::FailingChecks, false, EVERYTHING);

        assert_eq!(route.sources, [Source::CheckOutput]);
    }

    #[test]
    fn nothing_is_chosen_that_is_switched_off() {
        let route = route(TaskKind::BugFix, true, Available::default());

        assert!(route.sources.is_empty());
    }
}
