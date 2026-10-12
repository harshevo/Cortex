//! Checking finished work against what was asked.
//!
//! The project's checks say whether the code still works. They do not say
//! whether the request was carried out: an agent can pass every test and
//! leave half the task undone. The review lists what the request asks for
//! and holds the changes against each item.
//!
//! The reviewer reads; it runs nothing. Its opinion is sent back to the
//! model once, as an opinion, and never blocks a run from ending.

use serde::{Deserialize, Serialize};

use crate::error::ProviderError;
use crate::judge;
use crate::provider::ModelProvider;
use crate::usage::Usage;

/// The request and the diff are shortened to these sizes before they are
/// shown to the reviewer.
const MAX_INSTRUCTION_CHARS: usize = 6_000;
const MAX_DIFF_CHARS: usize = 24_000;
const MAX_REPORT_CHARS: usize = 3_000;

const SYSTEM: &str = "\
You review the work of a coding agent. You are given the request it was \
handed, the changes it made as a diff, and its closing report. You cannot \
run anything.

First list what the request asks for, as separate requirements a reader \
could check one at a time. Include only what the request states or plainly \
implies. Then decide for each requirement whether the diff carries it out.

Mark a requirement as not met only when the diff shows it is missing or \
wrong. If you cannot tell from the diff, mark it met.

Answer with JSON only, in this shape:

{\"requirements\": [{\"text\": \"...\", \"met\": true, \"why\": \"one sentence\"}]}";

/// One thing the request asks for, and whether the work does it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    pub text: String,
    pub met: bool,

    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Default)]
pub struct Review {
    pub requirements: Vec<Requirement>,
    pub usage: Usage,
}

#[derive(Deserialize)]
struct Parsed {
    requirements: Vec<Requirement>,
}

impl Review {
    pub fn unmet(&self) -> impl Iterator<Item = &Requirement> {
        self.requirements
            .iter()
            .filter(|requirement| !requirement.met)
    }

    /// The share of requirements met, from 0 to 1. A review that found no
    /// requirements has nothing against the work.
    pub fn score(&self) -> f64 {
        if self.requirements.is_empty() {
            return 1.0;
        }

        let met = self
            .requirements
            .iter()
            .filter(|requirement| requirement.met)
            .count();

        met as f64 / self.requirements.len() as f64
    }

    /// The message sent back to the model, when something is unmet.
    pub fn feedback(&self) -> Option<String> {
        let unmet: Vec<&Requirement> = self.unmet().collect();

        if unmet.is_empty() {
            return None;
        }

        let mut text = String::from(
            "A reviewer compared your changes with the request and thinks part of it is \
             not done. The reviewer read the diff and ran nothing, so it can be wrong. \
             Check each point against the code. Finish what is missing; if a point is \
             already handled, say where, and finish again.\n",
        );

        for requirement in unmet {
            text.push_str(&format!("\n- {}", requirement.text));

            if !requirement.why.is_empty() {
                text.push_str(&format!(" ({})", requirement.why));
            }
        }

        text.push('\n');

        Some(text)
    }
}

/// Reviews `diff` against `instruction`.
///
/// `Ok(None)` means the model answered with something that is not a
/// review; the work is then judged without one.
pub async fn review(
    provider: &dyn ModelProvider,
    instruction: &str,
    diff: &str,
    report: &str,
) -> Result<Option<Review>, ProviderError> {
    let question = format!(
        "## Request\n\n{}\n\n## Changes\n\n{}\n\n## The agent's report\n\n{}",
        judge::clip_middle(instruction, MAX_INSTRUCTION_CHARS),
        judge::clip_middle(diff, MAX_DIFF_CHARS),
        judge::clip_middle(report, MAX_REPORT_CHARS),
    );

    let answer = judge::ask(provider, SYSTEM, &question).await?;

    let parsed = answer
        .json()
        .and_then(|value| serde_json::from_value::<Parsed>(value).ok());

    Ok(parsed.map(|parsed| Review {
        requirements: parsed.requirements,
        usage: answer.usage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirement(text: &str, met: bool) -> Requirement {
        Requirement {
            text: text.to_string(),
            met,
            why: "because".to_string(),
        }
    }

    #[test]
    fn feedback_lists_only_what_is_unmet() {
        let review = Review {
            requirements: vec![
                requirement("losses round correctly", true),
                requirement("the CSV export is fixed too", false),
            ],
            usage: Usage::default(),
        };

        let feedback = review.feedback().unwrap();

        assert!(feedback.contains("- the CSV export is fixed too (because)"));
        assert!(!feedback.contains("losses round correctly"));
        assert_eq!(review.score(), 0.5);
    }

    #[test]
    fn a_review_with_everything_met_says_nothing() {
        let review = Review {
            requirements: vec![requirement("it works", true)],
            usage: Usage::default(),
        };

        assert_eq!(review.feedback(), None);
        assert_eq!(Review::default().score(), 1.0);
    }
}
