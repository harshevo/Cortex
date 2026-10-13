//! Replays saved eval trajectories through the mind, oldest first, as if
//! the runs were happening now. No model is called.
//!
//!     python3 -m cortex.brain.server --dir /tmp/mind-replay      (in the brain project)
//!     cargo run --release -p cortex-brain --example mind_replay -- eval/results tradelyzer
//!
//! Before each step the mind is asked for its guess of the model's next
//! action, exactly as in a live run, and then shown what the model did and
//! how it turned out. A guess is counted right when it is one of the
//! actions the model took in its next response. This measures how well the
//! mind anticipates the model; whether its advice makes the model better
//! needs live runs with and without `--with mind`.
//!
//! Replaying also teaches: point the service at the directory you mean to
//! keep and the mind starts with these runs behind it.

use std::fs;
use std::path::{Path, PathBuf};

use cortex_brain::Mind;
use cortex_brain::mind::{DEFAULT_URL, describe};
use cortex_core::{Adviser, Event, TrajectorySink};
use serde_json::Value;

struct Run {
    solved: bool,
    events: Vec<Event>,
}

#[derive(Default)]
struct Tally {
    runs: u32,
    steps: u32,

    /// Steps at which the mind had a guess at all.
    guesses: u32,
    guesses_right: u32,

    /// Steps at which it was sure enough to say its guess to the model.
    spoken: u32,
    spoken_right: u32,
}

fn main() {
    let results = std::env::args()
        .nth(1)
        .unwrap_or("eval/results".to_string());
    let suite = std::env::args().nth(2).unwrap_or("tradelyzer".to_string());
    let url = std::env::args().nth(3).unwrap_or(DEFAULT_URL.to_string());

    let mind = Mind::new(&url, &suite);

    if mind.health().is_none() {
        eprintln!("the mind service at {url} is not reachable");
        std::process::exit(1);
    }

    let runs = load_runs(Path::new(&results), &suite);
    let half = runs.len() / 2;

    let mut early = Tally::default();
    let mut late = Tally::default();

    for (index, run) in runs.iter().enumerate() {
        replay(
            &mind,
            run,
            if index < half { &mut early } else { &mut late },
        );
    }

    println!("{} runs of {suite}, replayed oldest first\n", runs.len());
    print(&early, "first half");
    print(&late, "second half");
    println!(
        "\nthe mind now holds: {}",
        mind.health().unwrap_or(Value::Null)
    );
}

fn replay(mind: &Mind, run: &Run, tally: &mut Tally) {
    tally.runs += 1;

    // What the mind expected of the response that is about to be shown.
    let mut expected: Option<(String, bool)> = None;

    for event in &run.events {
        match event {
            Event::ModelResponded { step, message, .. } => {
                tally.steps += 1;

                let mut taken: Vec<String> = message
                    .tool_calls()
                    .iter()
                    .map(|call| describe(call).0)
                    .collect();

                if taken.is_empty() {
                    taken.push("finish".to_string());
                }

                if let Some((guess, spoken)) = expected.take() {
                    let right = taken.contains(&guess);

                    tally.guesses += 1;
                    tally.guesses_right += u32::from(right);
                    tally.spoken += u32::from(spoken);
                    tally.spoken_right += u32::from(spoken && right);
                }

                mind.record(event);

                // The tools of this step run next in the log; the guess for
                // the following response is asked for after them, below.
                let _ = step;
            }

            Event::ModelStarted { step } if *step > 1 => {
                expected = ask(mind, *step);
            }

            // The eval's hidden check is the outcome the run really had.
            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                ..
            } => mind.record(&Event::RunFinished {
                outcome: *outcome,
                steps: *steps,
                usage: *usage,
                duration_ms: *duration_ms,
                verified: Some(run.solved),
            }),

            other => mind.record(other),
        }
    }
}

/// The mind's guess before a step, and whether it would have said it.
fn ask(mind: &Mind, step: u32) -> Option<(String, bool)> {
    let thought = mind.consider()?;
    let guess = thought["guess"].as_str()?.to_string();
    let spoken = mind.advise(step, 0).is_some();

    Some((guess, spoken))
}

fn print(tally: &Tally, name: &str) {
    let share = |part: u32, whole: u32| {
        if whole == 0 {
            "-".to_string()
        } else {
            format!("{:.0}%", 100.0 * f64::from(part) / f64::from(whole))
        }
    };

    println!(
        "{name}: {} runs, {} steps; had a guess at {} steps, right {} ({}); spoke at {}, right {} ({})",
        tally.runs,
        tally.steps,
        tally.guesses,
        tally.guesses_right,
        share(tally.guesses_right, tally.guesses),
        tally.spoken,
        tally.spoken_right,
        share(tally.spoken_right, tally.spoken),
    );
}

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
                    solved: result["solved"].as_bool().unwrap_or(false),
                    events,
                });
            }
        }
    }

    runs
}
