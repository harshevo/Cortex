//! Unfinished business: what a long run started and has not come back to.
//!
//! Over many steps a model's attention moves on, and a failing check or a
//! pile of unchecked edits can scroll out of mind. This keeps hold of the
//! two that matter and speaks once when one has been left for too long.
//! It is plain bookkeeping, with nothing learned.

/// Steps a failing command may be left alone, with edits made since,
/// before it is mentioned.
const FAILING_PATIENCE: u32 = 5;

/// Steps edits may go without anything being run before it is mentioned.
const UNCHECKED_PATIENCE: u32 = 6;

/// Files named in one reminder.
const FILES_NAMED: usize = 3;

#[derive(Debug, Clone)]
struct Failing {
    command: String,
    step: u32,
    edits_since: u32,
}

#[derive(Debug, Clone, Default)]
pub struct OpenLoops {
    /// The command that failed last and has not passed since.
    failing: Option<Failing>,

    /// The step of the first edit that nothing has been run after, and
    /// the files edited since.
    unchecked_since: Option<u32>,
    unchecked: Vec<String>,

    /// Each loop is mentioned once in a run.
    said_failing: Vec<String>,
    said_unchecked: bool,
}

impl OpenLoops {
    pub fn edited(&mut self, path: &str, step: u32) {
        self.unchecked_since.get_or_insert(step);

        if !self.unchecked.iter().any(|file| file == path) {
            self.unchecked.push(path.to_string());
        }

        if let Some(failing) = &mut self.failing {
            failing.edits_since += 1;
        }
    }

    /// Something was run: `commands` are the parts of the line worth
    /// naming, the first of them the one blamed if it failed.
    pub fn ran(&mut self, commands: &[String], passed: bool, step: u32) {
        self.unchecked_since = None;
        self.unchecked.clear();

        if passed {
            if let Some(failing) = &self.failing
                && commands.contains(&failing.command)
            {
                self.failing = None;
            }
        } else if let Some(command) = commands.first() {
            self.failing = Some(Failing {
                command: command.clone(),
                step,
                edits_since: 0,
            });
        }
    }

    /// What should be mentioned after `step`, if anything has been left
    /// long enough.
    pub fn reminder(&mut self, step: u32) -> Option<String> {
        if let Some(failing) = &self.failing {
            let left = step.saturating_sub(failing.step);

            let due = left >= FAILING_PATIENCE
                && failing.edits_since > 0
                && !self.said_failing.contains(&failing.command);

            if due {
                self.said_failing.push(failing.command.clone());

                return Some(format!(
                    "`{}` failed at step {} and has not been run again, {} later.",
                    failing.command,
                    failing.step,
                    count(failing.edits_since as usize, "edit"),
                ));
            }
        }

        let since = self.unchecked_since?;

        if step.saturating_sub(since) < UNCHECKED_PATIENCE || self.said_unchecked {
            return None;
        }

        self.said_unchecked = true;

        let mut named: Vec<&str> = self.unchecked.iter().map(String::as_str).collect();

        named.truncate(FILES_NAMED);

        Some(format!(
            "{} changed since step {since} with nothing run to check them: {}.",
            count(self.unchecked.len(), "file"),
            named.join(", "),
        ))
    }
}

fn count(number: usize, thing: &str) -> String {
    match number {
        1 => format!("1 {thing}"),
        number => format!("{number} {thing}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(command: &str) -> Vec<String> {
        vec![command.to_string()]
    }

    #[test]
    fn a_failing_check_left_behind_is_mentioned_once() {
        let mut loops = OpenLoops::default();

        loops.ran(&commands("npm test"), false, 4);
        loops.edited("a.ts", 5);

        assert_eq!(loops.reminder(6), None, "too soon");

        // Something else was run meanwhile, so the edit is not unchecked.
        loops.ran(&commands("npx tsc"), true, 7);

        let reminder = loops.reminder(9).unwrap();

        assert_eq!(
            reminder,
            "`npm test` failed at step 4 and has not been run again, 1 edit later."
        );

        assert_eq!(loops.reminder(10), None);
    }

    #[test]
    fn a_check_that_passes_again_closes_the_loop() {
        let mut loops = OpenLoops::default();

        loops.ran(&commands("npm test"), false, 4);
        loops.edited("a.ts", 5);
        loops.ran(&commands("npm test"), true, 6);

        assert_eq!(loops.reminder(20), None);
    }

    #[test]
    fn edits_nobody_checked_are_mentioned() {
        let mut loops = OpenLoops::default();

        loops.edited("a.ts", 3);
        loops.edited("b.ts", 4);
        loops.edited("a.ts", 5);

        assert_eq!(loops.reminder(8), None);

        assert_eq!(
            loops.reminder(9).unwrap(),
            "2 files changed since step 3 with nothing run to check them: a.ts, b.ts."
        );

        assert_eq!(loops.reminder(12), None);
    }

    #[test]
    fn a_failure_with_no_edits_since_is_left_alone() {
        let mut loops = OpenLoops::default();

        loops.ran(&commands("npm test"), false, 2);

        assert_eq!(loops.reminder(30), None);
    }
}
