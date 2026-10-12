//! Checking finished work against the project's own checks.
//!
//! A model that says "done" has only claimed it. The agent loop asks a
//! [`Verifier`] to run the project's tests and other checks, and sends any
//! failures back to the model instead of accepting the claim.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;

/// The result of one check command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub command: String,
    pub passed: bool,
    pub duration_ms: u64,

    /// What the command printed, shortened. Empty when it passed.
    pub output: String,
}

/// The result of running every check once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub checks: Vec<CheckOutcome>,
}

impl Verification {
    /// True when no check failed. A project with no checks passes, because
    /// there is nothing to hold the work against.
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }

    pub fn failures(&self) -> impl Iterator<Item = &CheckOutcome> {
        self.checks.iter().filter(|check| !check.passed)
    }

    /// The message sent back to the model when a check fails.
    pub fn feedback(&self) -> String {
        let mut text = String::from(
            "The harness ran the project's checks on your work and they do not pass, so the \
             task is not finished. Read the failures, fix their cause in the source code, and \
             finish again.\n",
        );

        for check in self.failures() {
            text.push_str(&format!(
                "\n`{}` failed:\n\n{}\n",
                check.command, check.output
            ));
        }

        text
    }
}

#[async_trait]
pub trait Verifier: Send + Sync {
    /// The commands [`Verifier::verify`] would run in `workspace`.
    fn commands(&self, workspace: &Workspace) -> Vec<String>;

    async fn verify(&self, workspace: &Workspace) -> Verification;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(command: &str, passed: bool) -> CheckOutcome {
        CheckOutcome {
            command: command.to_string(),
            passed,
            duration_ms: 1,
            output: "boom".to_string(),
        }
    }

    #[test]
    fn no_checks_counts_as_passed() {
        assert!(Verification::default().passed());
    }

    #[test]
    fn feedback_names_only_the_failed_checks() {
        let verification = Verification {
            checks: vec![check("npm test", false), check("npm run lint", true)],
        };

        let feedback = verification.feedback();

        assert!(!verification.passed());
        assert!(feedback.contains("`npm test` failed"));
        assert!(!feedback.contains("npm run lint"));
    }
}
