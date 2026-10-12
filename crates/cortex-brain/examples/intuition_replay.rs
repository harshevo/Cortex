//! Replays saved eval trajectories through the intuition, to see what it
//! would have brought to mind without calling a model.
//!
//!     cargo run --release -p cortex-brain --example intuition_replay -- eval/results tradelyzer
//!
//! Two settings:
//!
//! - **New task**: the network has learned from every run of the *other*
//!   tasks and never from the one it is shown.
//! - **In order**: the runs are played oldest first, so a task may have
//!   been attempted before, as when work returns to the same code.
//!
//! A hunch about a file is counted right when the run went on to change
//! that file, and half right when it went on to read it.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use cortex_brain::Intuition;
use cortex_brain::intuition::concept;
use cortex_core::{Adviser, Event, StopReason, TrajectorySink};
use serde_json::Value;

struct Run {
    task: String,
    solved: bool,
    events: Vec<Event>,
}

#[derive(Default)]
struct Tally {
    runs: u32,

    /// Runs in which a file was named before the first step.
    briefed: u32,

    /// Of those, runs where a named file was one the run changed.
    briefed_right: u32,

    /// Files named, before the task or between steps.
    named: u32,
    changed_later: u32,
    read_later: u32,

    commands: u32,
    reminders: u32,

    /// Steps the runs took before first opening a file they then changed,
    /// in runs where that file had been named before the first step.
    steps_before_opening: u32,
}

fn main() {
    let results = std::env::args()
        .nth(1)
        .unwrap_or("eval/results".to_string());

    // Memory is kept per repository, so one suite is replayed at a time.
    let suite = std::env::args().nth(2).unwrap_or("tradelyzer".to_string());

    let runs = load_runs(Path::new(&results), &suite);

    println!(
        "{} finished runs of the {suite} suite from {results}",
        runs.len()
    );

    let mut tasks: Vec<&str> = runs.iter().map(|run| run.task.as_str()).collect();

    tasks.sort_unstable();
    tasks.dedup();

    let mut new_task = Tally::default();

    for task in &tasks {
        let intuition = Intuition::new(None);

        for run in runs.iter().filter(|run| run.task != *task) {
            play(&intuition, run, true, None);
        }

        for run in runs.iter().filter(|run| run.task == *task) {
            play(&intuition, run, false, Some(&mut new_task));
        }
    }

    report(
        "new task (never seen, learned from the other tasks)",
        &new_task,
    );

    // A third argument saves the network learned in order, to look at in
    // the app: put it beside a project's memory as
    // `.cortex/memory.intuition.json`.
    let save = std::env::args().nth(3).map(PathBuf::from);

    let mut in_order = Tally::default();
    let intuition = Intuition::new(save);

    for run in &runs {
        play(&intuition, run, true, Some(&mut in_order));
    }

    report(
        "in order (a task may have been attempted before)",
        &in_order,
    );

    println!(
        "\nnetwork after all runs: {} neurons, {} synapses",
        intuition.neurons(),
        intuition.synapses()
    );
}

fn report(name: &str, tally: &Tally) {
    let share = |part: u32, whole: u32| 100.0 * part as f32 / whole.max(1) as f32;

    println!("\n== {name}");

    println!(
        "   before the task: named a file in {} of {} runs; in {} of those ({:.0}%) it was a \
         file the run went on to change",
        tally.briefed,
        tally.runs,
        tally.briefed_right,
        share(tally.briefed_right, tally.briefed)
    );

    println!(
        "   those runs took {:.1} steps on average before first opening that file",
        tally.steps_before_opening as f32 / tally.briefed_right.max(1) as f32
    );

    println!(
        "   all files named: {}; changed later {} ({:.0}%), only read later {} ({:.0}%), neither {}",
        tally.named,
        tally.changed_later,
        share(tally.changed_later, tally.named),
        tally.read_later,
        share(tally.read_later, tally.named),
        tally.named - tally.changed_later - tally.read_later
    );

    println!(
        "   commands named: {}; reminders of unfinished business: {}",
        tally.commands, tally.reminders
    );
}

/// Plays one run. With `learn`, its ending is passed on so the network
/// learns from it; with a tally, what the intuition says is scored.
fn play(intuition: &Intuition, run: &Run, learn: bool, mut tally: Option<&mut Tally>) {
    // What the run touched from each point on, for scoring.
    let touches: Vec<(usize, String, bool)> = run
        .events
        .iter()
        .enumerate()
        .filter_map(|(position, event)| {
            let Event::ToolStarted { call, .. } = event else {
                return None;
            };

            let changes = matches!(call.name.as_str(), "edit_file" | "write_file");

            Some((position, concept::file_of(&call.input)?, changes))
        })
        .collect();

    let later = |from: usize, path: &str, changes: bool| {
        touches.iter().any(|(position, file, changed)| {
            *position >= from && file == path && *changed == changes
        })
    };

    let mut after_tools: Option<u32> = None;
    let mut scored: HashSet<String> = HashSet::new();

    if let Some(tally) = tally.as_deref_mut() {
        tally.runs += 1;
    }

    for (position, event) in run.events.iter().enumerate() {
        // Advice comes after a step's tools, before the next response.
        if matches!(
            event,
            Event::ModelStarted { .. } | Event::ModelResponded { .. }
        ) && let Some(step) = after_tools.take()
            && let Some(advice) = intuition.advise(step, 60)
            && let Some(tally) = tally.as_deref_mut()
        {
            score(tally, &advice.detail, position, &later, &mut scored);

            tally.reminders += u32::from(!advice.detail["reminder"].is_null());
        }

        match event {
            Event::RunStarted { .. } => {
                intuition.record(event);

                // The briefing: what the task's words alone bring to mind.
                let hunches = intuition.hunches();

                if std::env::var("SHOW").is_ok() && tally.is_some() {
                    for hunch in &hunches {
                        println!(
                            "      {:<28} {:.2} {} {:<48} {:?}",
                            run.task,
                            hunch.strength,
                            if later(0, &hunch.concept.text, true) {
                                "RIGHT"
                            } else {
                                "wrong"
                            },
                            hunch.concept.text,
                            hunch.because
                        );
                    }
                }

                let files: Vec<String> = hunches
                    .into_iter()
                    .map(|hunch| hunch.concept.text)
                    .collect();

                if let Some(tally) = tally.as_deref_mut()
                    && !files.is_empty()
                {
                    tally.briefed += 1;

                    if let Some(right) = files.iter().find(|file| later(0, file, true)) {
                        tally.briefed_right += 1;

                        tally.steps_before_opening += first_opened(&run.events, right);
                    }
                }
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                ..
            } => {
                if learn {
                    intuition.record(&Event::RunFinished {
                        outcome: *outcome,
                        steps: *steps,
                        usage: *usage,
                        duration_ms: *duration_ms,
                        verified: Some(run.solved),
                    });
                }
            }

            Event::ModelResponded {
                step, stop_reason, ..
            } => {
                intuition.record(event);

                if *stop_reason == StopReason::ToolUse {
                    after_tools = Some(*step);
                }
            }

            other => intuition.record(other),
        }
    }
}

fn score(
    tally: &mut Tally,
    detail: &Value,
    position: usize,
    later: &dyn Fn(usize, &str, bool) -> bool,
    scored: &mut HashSet<String>,
) {
    for hunch in detail["hunches"].as_array().into_iter().flatten() {
        let text = hunch["text"].as_str().unwrap_or_default();

        if hunch["kind"] != "file" {
            tally.commands += 1;

            continue;
        }

        if !scored.insert(text.to_string()) {
            continue;
        }

        tally.named += 1;

        if later(position, text, true) {
            tally.changed_later += 1;
        } else if later(position, text, false) {
            tally.read_later += 1;
        }
    }
}

/// The step at which the run first touched `path`.
fn first_opened(events: &[Event], path: &str) -> u32 {
    events
        .iter()
        .find_map(|event| match event {
            Event::ToolStarted { step, call }
                if concept::file_of(&call.input).as_deref() == Some(path) =>
            {
                Some(*step)
            }

            _ => None,
        })
        .unwrap_or(0)
}

/// Every run under `results` that reached its end, oldest first.
fn load_runs(results: &Path, suite: &str) -> Vec<Run> {
    let report = |directory: &Path| -> Value {
        fs::read_to_string(directory.join("report.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    };

    let mut directories: Vec<PathBuf> = fs::read_dir(results)
        .expect("cannot read the results directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| report(path)["suite"] == suite)
        .collect();

    directories.sort_by_key(|path| report(path)["created_unix_ms"].as_u64().unwrap_or_default());

    let mut runs = Vec::new();

    for directory in directories {
        for result in report(&directory)["results"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let task = result["task_id"].as_str().unwrap_or_default();
            let attempt = result["attempt"].as_u64().unwrap_or(1);

            let file = directory
                .join("trajectories")
                .join(format!("{task}-{attempt}.jsonl"));

            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };

            let events: Vec<Event> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();

            let finished = events
                .iter()
                .any(|event| matches!(event, Event::RunFinished { .. }));

            if finished && result["error"].is_null() {
                runs.push(Run {
                    task: task.to_string(),
                    solved: result["solved"].as_bool().unwrap_or(false),
                    events,
                });
            }
        }
    }

    runs
}
