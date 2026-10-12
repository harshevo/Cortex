//! Consolidation, step two: promoting patterns from episodes to facts.
//!
//! One episode is an anecdote. Something seen in several episodes, or
//! confirmed by running it, is worth showing on every later task. This is
//! done with plain counting; a learned version would replace this module.

use std::collections::BTreeMap;

use crate::episode::{Episode, clip};
use crate::fact::{Fact, FactKind};

/// A pattern must appear in this many episodes to become a fact.
const MIN_SUPPORT: u32 = 2;

const MAX_TASKS_LISTED: usize = 3;
const MAX_TASK_CHARS: usize = 70;

/// Words that mark a command as a check of the project rather than a
/// one-off inspection.
const CHECK_WORDS: [&str; 9] = [
    "test", "pytest", "vitest", "jest", "tsc", "lint", "build", "check", "mypy",
];

/// The facts supported by `episodes`, which must all be from one
/// repository. The result replaces the derived facts stored before, so a
/// pattern that stops being supported disappears.
pub fn derive_facts(episodes: &[Episode], now_ms: u64) -> Vec<Fact> {
    let mut facts = Vec::new();

    facts.extend(check_commands(episodes, now_ms));
    facts.extend(hot_files(episodes, now_ms));
    facts.extend(dead_ends(episodes, now_ms));

    facts
}

/// Commands the verification gate ran and saw pass, plus check-like
/// commands the agent ran successfully in several tasks.
fn check_commands(episodes: &[Episode], now_ms: u64) -> Vec<Fact> {
    let mut gate_passes: BTreeMap<&str, u32> = BTreeMap::new();
    let mut agent_passes: BTreeMap<&str, u32> = BTreeMap::new();

    for episode in episodes {
        for command in &episode.commands_worked {
            if is_check(command) {
                *agent_passes.entry(command).or_default() += 1;
            }
        }
    }

    for episode in episodes
        .iter()
        .filter(|episode| episode.verified == Some(true))
    {
        for command in episode.checks_passed() {
            *gate_passes.entry(command).or_default() += 1;
        }
    }

    let mut facts = Vec::new();

    for (command, count) in &gate_passes {
        facts.push(Fact {
            kind: FactKind::CheckCommand,
            key: command.to_string(),
            text: format!(
                "`{command}` is the project's check; the harness runs it when you finish \
                 (passed after {count} earlier task(s))."
            ),
            paths: Vec::new(),
            support: *count,
            updated_ms: now_ms,
        });
    }

    for (command, count) in agent_passes {
        if count < MIN_SUPPORT || gate_passes.contains_key(command) {
            continue;
        }

        facts.push(Fact {
            kind: FactKind::CheckCommand,
            key: command.to_string(),
            text: format!("`{command}` works here (ran cleanly in {count} earlier tasks)."),
            paths: Vec::new(),
            support: count,
            updated_ms: now_ms,
        });
    }

    facts
}

/// Files that several tasks ended up changing.
fn hot_files(episodes: &[Episode], now_ms: u64) -> Vec<Fact> {
    let mut tasks_by_file: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

    for episode in episodes {
        for path in &episode.files_changed {
            tasks_by_file
                .entry(path)
                .or_default()
                .push(&episode.instruction);
        }
    }

    tasks_by_file
        .into_iter()
        .filter(|(_, tasks)| tasks.len() as u32 >= MIN_SUPPORT)
        .map(|(path, tasks)| {
            let listed: Vec<String> = tasks
                .iter()
                .rev()
                .take(MAX_TASKS_LISTED)
                .map(|task| format!("\"{}\"", clip(task, MAX_TASK_CHARS)))
                .collect();

            Fact {
                kind: FactKind::HotFile,
                key: path.to_string(),
                text: format!(
                    "`{path}` was changed by {} earlier tasks: {}.",
                    tasks.len(),
                    listed.join("; ")
                ),
                paths: vec![path.to_string()],
                support: tasks.len() as u32,
                updated_ms: now_ms,
            }
        })
        .collect()
}

/// Commands that failed in several tasks and never worked in any.
fn dead_ends(episodes: &[Episode], now_ms: u64) -> Vec<Fact> {
    let mut failures: BTreeMap<&str, u32> = BTreeMap::new();

    for episode in episodes {
        for command in &episode.dead_ends {
            *failures.entry(command).or_default() += 1;
        }
    }

    let ever_worked = |command: &str| {
        episodes.iter().any(|episode| {
            episode
                .commands_worked
                .iter()
                .any(|worked| worked == command)
        })
    };

    failures
        .into_iter()
        .filter(|(command, count)| *count >= MIN_SUPPORT && !ever_worked(command))
        .map(|(command, count)| Fact {
            kind: FactKind::DeadEnd,
            key: command.to_string(),
            text: format!("`{command}` failed in {count} earlier tasks and never worked here."),
            paths: Vec::new(),
            support: count,
            updated_ms: now_ms,
        })
        .collect()
}

fn is_check(command: &str) -> bool {
    let lowered = command.to_lowercase();

    CHECK_WORDS.iter().any(|word| lowered.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(instruction: &str, changed: &[&str], worked: &[&str], dead: &[&str]) -> Episode {
        let owned = |items: &[&str]| items.iter().map(|item| item.to_string()).collect();

        Episode {
            id: 0,
            created_ms: 0,
            instruction: instruction.to_string(),
            outcome: None,
            verified: None,
            rejections: 0,
            steps: 5,
            tokens: 0,
            duration_ms: 0,
            files_changed: owned(changed),
            files_read: Vec::new(),
            commands_worked: owned(worked),
            calculations: Vec::new(),
            dead_ends: owned(dead),
            checks_failed: Vec::new(),
            checks_run: Vec::new(),
            report: String::new(),
        }
    }

    fn keys(facts: &[Fact], kind: FactKind) -> Vec<&str> {
        facts
            .iter()
            .filter(|fact| fact.kind == kind)
            .map(|fact| fact.key.as_str())
            .collect()
    }

    #[test]
    fn one_episode_promotes_nothing_unverified() {
        let episodes = [episode("a", &["x.ts"], &["npm test"], &["npx jest"])];

        assert!(derive_facts(&episodes, 0).is_empty());
    }

    #[test]
    fn repeated_patterns_become_facts() {
        let episodes = [
            episode(
                "fix the calendar",
                &["x.ts"],
                &["npm test", "ls"],
                &["npx jest"],
            ),
            episode(
                "fix the journal",
                &["x.ts", "y.ts"],
                &["npm test", "ls"],
                &["npx jest"],
            ),
        ];

        let facts = derive_facts(&episodes, 0);

        assert_eq!(keys(&facts, FactKind::CheckCommand), ["npm test"]);
        assert_eq!(keys(&facts, FactKind::HotFile), ["x.ts"]);
        assert_eq!(keys(&facts, FactKind::DeadEnd), ["npx jest"]);

        let hot = facts
            .iter()
            .find(|fact| fact.kind == FactKind::HotFile)
            .unwrap();

        assert!(hot.text.contains("fix the journal"));
        assert_eq!(hot.paths, ["x.ts"]);
    }

    #[test]
    fn a_check_the_gate_saw_pass_is_a_fact_after_one_task() {
        let mut verified = episode("a", &["x.ts"], &[], &[]);

        verified.verified = Some(true);
        verified.checks_run = vec!["CI=1 npm run test".to_string()];

        let facts = derive_facts(&[verified], 0);

        assert_eq!(keys(&facts, FactKind::CheckCommand), ["CI=1 npm run test"]);
    }

    #[test]
    fn a_command_that_worked_once_is_not_a_dead_end() {
        let episodes = [
            episode("a", &[], &[], &["npm test"]),
            episode("b", &[], &[], &["npm test"]),
            episode("c", &[], &["npm test"], &[]),
        ];

        assert!(keys(&derive_facts(&episodes, 0), FactKind::DeadEnd).is_empty());
    }
}
