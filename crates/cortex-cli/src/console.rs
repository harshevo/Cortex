//! Showing a run in the terminal as it happens.

use cortex_core::{Event, TrajectorySink};
use cortex_providers::pricing;

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

const MAX_SUMMARY_CHARS: usize = 110;

pub struct ConsoleSink {
    model: String,
}

impl ConsoleSink {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
        }
    }
}

impl TrajectorySink for ConsoleSink {
    fn record(&self, event: &Event) {
        match event {
            Event::BrainObserved { .. } => {}
            Event::RunStarted { .. } | Event::ModelStarted { .. } => {}

            Event::RunResumed { steps } => {
                println!("{DIM}  carrying on after step {steps}{RESET}");
            }

            Event::ModelResponded { message, .. } => {
                let text = message.text();

                if !text.trim().is_empty() {
                    println!("\n{}", text.trim());
                }
            }

            Event::ToolStarted { call, .. } => {
                let input = one_line(&call.input.to_string());

                println!("\n{DIM}> {} {input}{RESET}", call.name);
            }

            Event::ToolFinished {
                result,
                duration_ms,
                ..
            } => {
                let first_line = result.output.lines().next().unwrap_or_default();
                let lines = result.output.lines().count();

                if result.is_error {
                    println!("{RED}  failed: {}{RESET}", one_line(first_line));
                } else {
                    println!("{DIM}  ok, {lines} lines, {duration_ms} ms{RESET}");
                }
            }

            Event::UserInterjected { text, .. } => {
                println!("{DIM}  you, meanwhile: {text}{RESET}");
            }

            Event::ContextPruned {
                results_trimmed,
                chars_removed,
                ..
            } => {
                println!(
                    "{DIM}  removed {results_trimmed} superseded tool results ({chars_removed} characters){RESET}"
                );
            }

            Event::ContextMasked {
                results_masked,
                chars_removed,
                ..
            } => {
                println!(
                    "{DIM}  masked {results_masked} old tool results ({chars_removed} characters){RESET}"
                );
            }

            Event::ReviewRan {
                requirements,
                feedback,
                ..
            } => {
                let met = requirements
                    .iter()
                    .filter(|requirement| requirement.met)
                    .count();

                println!(
                    "\n{DIM}> review: {met} of {} requirements met{RESET}",
                    requirements.len()
                );

                for requirement in requirements.iter().filter(|requirement| !requirement.met) {
                    println!("{RED}  not met: {}{RESET}", one_line(&requirement.text));
                }

                if feedback.is_some() {
                    println!("{RED}  the work was sent back to the model{RESET}");
                }
            }

            Event::LessonsLearned { lessons, .. } => {
                for lesson in lessons {
                    println!("{DIM}  lesson kept: {}{RESET}", one_line(&lesson.title));
                }
            }

            Event::AttemptStarted {
                attempt,
                of,
                strategy,
            } => {
                println!("\n{DIM}> attempt {attempt} of up to {of} ({strategy}){RESET}");
            }

            Event::AttemptFinished {
                steps,
                verified,
                review_score,
                changed_files,
                error,
                ..
            } => {
                if let Some(error) = error {
                    println!("{RED}  failed: {}{RESET}", one_line(error));

                    return;
                }

                let checks = match verified {
                    Some(true) => "checks passed",
                    Some(false) => "checks FAILED",
                    None => "no checks",
                };

                let review = match review_score {
                    Some(score) => format!("review {:.0}%", score * 100.0),
                    None => "not reviewed".to_string(),
                };

                println!(
                    "{DIM}  {steps} steps, {changed_files} files changed, {checks}, {review}{RESET}"
                );
            }

            Event::AttemptChosen {
                attempt, reason, ..
            } => {
                println!("{DIM}  kept attempt {attempt}: {reason}{RESET}");
            }

            Event::ContextPrepared {
                source,
                text,
                duration_ms,
                ..
            } => {
                println!(
                    "{DIM}  {source}: {} characters of context prepared in {duration_ms} ms{RESET}",
                    text.chars().count()
                );
            }

            Event::VerificationRan { passed, checks, .. } => {
                for check in checks {
                    let verdict = if check.passed { "passed" } else { "FAILED" };

                    println!(
                        "\n{DIM}> check `{}` {verdict} in {:.1}s{RESET}",
                        check.command,
                        check.duration_ms as f64 / 1000.0
                    );
                }

                if !passed {
                    println!("{RED}  the work was sent back to the model{RESET}");
                }
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                verified,
            } => {
                let checked = match verified {
                    Some(true) => ", checks passed",
                    Some(false) => ", checks FAILED",
                    None => "",
                };

                let cost = match pricing::cost_usd(&self.model, usage) {
                    Some(cost) => format!("about ${cost:.4}"),
                    None => "cost unknown".to_string(),
                };

                println!(
                    "\n{DIM}[{outcome:?}{checked}: {steps} steps, {} tokens in, {} out, {cost}, {:.1}s]{RESET}",
                    usage.total_input(),
                    usage.output_tokens,
                    *duration_ms as f64 / 1000.0,
                );
            }
        }
    }
}

fn one_line(text: &str) -> String {
    let flat = text.replace('\n', " ");

    if flat.chars().count() <= MAX_SUMMARY_CHARS {
        return flat;
    }

    let clipped: String = flat.chars().take(MAX_SUMMARY_CHARS).collect();

    format!("{clipped}...")
}
