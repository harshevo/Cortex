//! Lessons: what earlier runs said a later run should know.
//!
//! An episode says what happened in one task. A lesson is advice taken
//! from a task, in words that carry over to another: where a kind of
//! problem lives, which approach failed and what worked instead. The
//! model writes them after a run (`cortex_core::reflect`), from failed
//! runs as well as passing ones.
//!
//! The same advice arriving from several runs is kept once, with a count
//! of how often it was seen.

use cortex_core::Lesson;
use serde::{Deserialize, Serialize};

use crate::similarity;

/// Two lessons this alike in their words are taken to be the same advice.
const SAME_ADVICE: f32 = 0.6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredLesson {
    /// Row id in the store. Zero until the lesson is saved.
    #[serde(default)]
    pub id: i64,

    #[serde(flatten)]
    pub lesson: Lesson,

    /// How many runs arrived at it.
    pub support: u32,

    pub created_ms: u64,
}

impl StoredLesson {
    pub fn new(lesson: Lesson, created_ms: u64) -> Self {
        Self {
            id: 0,
            lesson,
            support: 1,
            created_ms,
        }
    }

    /// The text that stands for the lesson when it is compared with a
    /// task: mostly the situation it is for.
    pub fn search_text(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.lesson.when, self.lesson.title, self.lesson.advice
        )
    }

    /// Whether `other` says the same thing in nearly the same words.
    pub fn same_advice(&self, other: &Lesson) -> bool {
        let mine = format!("{} {}", self.lesson.title, self.lesson.advice);
        let theirs = format!("{} {}", other.title, other.advice);

        similarity::lexical(&mine, &theirs) >= SAME_ADVICE
    }

    /// Whether every file the lesson is about still exists.
    pub fn still_holds(&self, exists: impl Fn(&str) -> bool) -> bool {
        self.lesson.paths.iter().all(|path| exists(path))
    }

    /// One line for the briefing.
    pub fn render(&self) -> String {
        let source = if self.lesson.from_failure {
            "learned from a run that failed"
        } else {
            "from a run that passed"
        };

        let seen = if self.support > 1 {
            format!(", seen {} times", self.support)
        } else {
            String::new()
        };

        format!(
            "**{}** When {}: {} ({source}{seen})",
            self.lesson.title.trim().trim_end_matches('.'),
            lower_first(self.lesson.when.trim().trim_end_matches('.')),
            self.lesson.advice.trim(),
        )
    }
}

/// "When" is already written before the situation, so a situation that
/// itself starts with it is not made to say it twice.
fn lower_first(text: &str) -> String {
    let text = text
        .strip_prefix("When ")
        .or_else(|| text.strip_prefix("when "))
        .unwrap_or(text);

    let mut chars = text.chars();

    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(title: &str, advice: &str) -> Lesson {
        Lesson {
            title: title.to_string(),
            when: "When a money amount is displayed wrongly.".to_string(),
            advice: advice.to_string(),
            paths: vec!["src/lib/money.ts".to_string()],
            from_failure: true,
        }
    }

    #[test]
    fn nearly_the_same_words_are_the_same_advice() {
        let stored = StoredLesson::new(
            lesson(
                "Money formatting",
                "Look in src/lib/money.ts first; formatting lives there.",
            ),
            1,
        );

        assert!(stored.same_advice(&lesson(
            "Money formatting",
            "Formatting lives in src/lib/money.ts; look there first."
        )));

        assert!(!stored.same_advice(&lesson(
            "Calendar weeks",
            "Weeks start on Monday in the calendar grid."
        )));
    }

    #[test]
    fn a_lesson_reads_as_one_line() {
        let mut stored = StoredLesson::new(lesson("Money formatting.", "Look in money.ts."), 1);

        stored.support = 3;

        assert_eq!(
            stored.render(),
            "**Money formatting** When a money amount is displayed wrongly: Look in money.ts. \
             (learned from a run that failed, seen 3 times)"
        );
    }
}
