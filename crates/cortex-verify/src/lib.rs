//! The verification gate.
//!
//! The agent loop does not take the model's word that a task is finished.
//! It asks a [`ProjectVerifier`] to run the project's own checks and sends
//! failures back to the model.
//!
//! - [`detect`] works out which commands check a project.
//! - [`summary`] shortens their output to what the model needs.

pub mod detect;
pub mod summary;

use std::time::{Duration, Instant};

use async_trait::async_trait;
use cortex_core::{CheckOutcome, Verification, Verifier, Workspace, process};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Runs the project's checks, either the ones given or the ones detected.
pub struct ProjectVerifier {
    /// Commands supplied by the user. Empty means detect them.
    commands: Vec<String>,

    timeout: Duration,
}

impl ProjectVerifier {
    /// Detects the checks from the project's files on every run.
    pub fn detecting() -> Self {
        Self::with_commands(Vec::new())
    }

    /// Uses exactly `commands`. An empty list means detect them.
    pub fn with_commands(commands: Vec<String>) -> Self {
        Self {
            commands,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;

        self
    }

    async fn run_check(&self, command: &str, workspace: &Workspace) -> CheckOutcome {
        let started = Instant::now();

        let result = process::run_shell(command, workspace.root(), self.timeout).await;

        let (passed, output) = match result {
            Ok(output) if output.timed_out => (
                false,
                format!(
                    "The command did not finish within {} seconds and was stopped.",
                    self.timeout.as_secs()
                ),
            ),

            Ok(output) if output.succeeded() => (true, String::new()),

            Ok(output) => {
                let combined = format!("{}\n{}", output.stdout, output.stderr);

                (false, summary::summarize(&combined))
            }

            Err(error) => (false, format!("The command could not be started: {error}")),
        };

        CheckOutcome {
            command: command.to_string(),
            passed,
            duration_ms: started.elapsed().as_millis() as u64,
            output,
        }
    }
}

#[async_trait]
impl Verifier for ProjectVerifier {
    fn commands(&self, workspace: &Workspace) -> Vec<String> {
        if self.commands.is_empty() {
            detect::checks(workspace.root())
        } else {
            self.commands.clone()
        }
    }

    async fn verify(&self, workspace: &Workspace) -> Verification {
        let mut checks = Vec::new();

        for command in self.commands(workspace) {
            let outcome = self.run_check(&command, workspace).await;

            let failed = !outcome.passed;

            checks.push(outcome);

            // Later checks usually fail for the same reason; one clear
            // failure is more useful to the model than three.
            if failed {
                break;
            }
        }

        Verification { checks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failing_command_is_reported_with_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();

        let verifier = ProjectVerifier::with_commands(vec![
            "true".to_string(),
            "echo expected 3 got 4; exit 1".to_string(),
            "echo never runs".to_string(),
        ]);

        let verification = verifier.verify(&workspace).await;

        assert!(!verification.passed());
        assert_eq!(verification.checks.len(), 2);
        assert!(verification.checks[0].passed);
        assert!(verification.checks[1].output.contains("expected 3 got 4"));
    }

    #[tokio::test]
    async fn a_project_without_checks_passes() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();

        let verification = ProjectVerifier::detecting().verify(&workspace).await;

        assert!(verification.checks.is_empty());
        assert!(verification.passed());
    }
}
