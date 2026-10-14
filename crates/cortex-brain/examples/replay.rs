//! Replays saved eval trajectories through the decision controllers, to
//! see what they would have learned and said without calling a model.
//!
//!     cargo run --release -p cortex-brain --example replay -- eval/results
//!
//! A run's reward is whether the eval scored it solved, taken from the
//! `report.json` beside its trajectory, because most saved runs had no
//! verification gate.
//!
//! Two questions are answered for each encoder:
//!
//! 1. Played in order, as in use: how often does the controller speak, what
//!    does it advise, and did the model go on to do that anyway?
//! 2. With each task held out in turn: how well does the learner predict
//!    the reward of the action taken, compared with always predicting the
//!    average? An encoder is only worth its cost if this error is lower.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cortex_brain::learner::{self, Learner};
use cortex_brain::situation::FEATURES;
use cortex_brain::{
    Action, Controller, DirectEncoder, Encoder, Situation,
};
use cortex_core::{Adviser, Event, StopReason, TrajectorySink};
use serde_json::Value;

const MAX_STEPS: u32 = 60;
const DISCOUNT: f32 = 0.93;

struct Run {
    task: String,
    solved: bool,
    events: Vec<Event>,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let results = PathBuf::from(arguments.first().map_or("eval/results", String::as_str));

    let runs = load_runs(&results);

    let solved = runs.iter().filter(|run| run.solved).count();

    println!(
        "{} finished runs ({} solved, {} not) from {}",
        runs.len(),
        solved,
        runs.len() - solved,
        results.display()
    );

    let encoders: Vec<Box<dyn Encoder>> = vec![Box::new(DirectEncoder)];

    let names = ["17 features"];

    for (encoder, name) in encoders.into_iter().zip(names) {
        println!("\n== {name}");

        let mut encoder = Caching::new(encoder);

        held_out(&runs, &mut encoder);

        println!(
            "   {} distinct situations encoded, {:.1} ms each",
            encoder.codes.len(),
            encoder.millis / encoder.codes.len().max(1) as f64
        );

        in_order(&runs, Box::new(encoder));
    }
}

/// Every run under `results` that reached its end, oldest first.
fn load_runs(results: &Path) -> Vec<Run> {
    let mut directories: Vec<PathBuf> = fs::read_dir(results)
        .expect("cannot read the results directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("report.json").exists())
        .collect();

    directories.sort_by_key(|path| report(path)["created_unix_ms"].as_u64().unwrap_or_default());

    let mut runs = Vec::new();

    for directory in directories {
        let report = report(&directory);

        for result in report["results"].as_array().into_iter().flatten() {
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

fn report(directory: &Path) -> Value {
    fs::read_to_string(directory.join("report.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// Remembers the code of each situation, since a spiking brain takes
/// milliseconds per situation and many situations recur.
struct Caching {
    inner: Box<dyn Encoder>,
    codes: HashMap<[u32; FEATURES], Vec<f32>>,
    millis: f64,
}

impl Caching {
    fn new(inner: Box<dyn Encoder>) -> Self {
        Self {
            inner,
            codes: HashMap::new(),
            millis: 0.0,
        }
    }
}

impl Encoder for Caching {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn encode(&mut self, features: &[f32; FEATURES]) -> Vec<f32> {
        let key = features.map(f32::to_bits);

        if let Some(code) = self.codes.get(&key) {
            return code.clone();
        }

        let started = Instant::now();
        let code = self.inner.encode(features);

        self.millis += started.elapsed().as_secs_f64() * 1000.0;
        self.codes.insert(key, code.clone());

        code
    }
}

/// The situation before each model response of a run, and the action the
/// response amounted to.
fn choices(run: &Run) -> Vec<([f32; FEATURES], Action)> {
    let mut situation = Situation::default();
    let mut choices = Vec::new();

    for event in &run.events {
        match event {
            Event::ModelResponded {
                step,
                message,
                stop_reason,
                ..
            } => {
                let action = Action::of_response(message, stop_reason);

                choices.push((situation.features(*step, MAX_STEPS), action));
                situation.note_action(action);
            }

            other => situation.observe(other),
        }
    }

    choices
}

/// Question 2: prediction error on tasks the learner never saw.
fn held_out(runs: &[Run], encoder: &mut Caching) {
    let mut tasks: Vec<&str> = runs.iter().map(|run| run.task.as_str()).collect();

    tasks.sort_unstable();
    tasks.dedup();

    // Each run as codes, actions and the reward each action earned.
    let coded: Vec<Vec<(Vec<f32>, Action, f32)>> = runs
        .iter()
        .map(|run| {
            let choices = choices(run);
            let last = choices.len().saturating_sub(1);
            let reward = if run.solved { 1.0 } else { 0.0 };

            choices
                .iter()
                .enumerate()
                .map(|(position, (features, action))| {
                    let mut code = encoder.encode(features);

                    learner::normalize(&mut code);

                    let earned = reward * DISCOUNT.powi((last - position) as i32);

                    (code, *action, earned)
                })
                .collect()
        })
        .collect();

    let mut learned_error = 0.0_f64;
    let mut average_error = 0.0_f64;
    let mut ranked_right = 0_u32;
    let mut count = 0_u32;

    for task in &tasks {
        let mut learner = Learner::new(encoder.dimension());

        let mut sums = [0.0_f32; 5];
        let mut seen = [0_u32; 5];

        // Three passes, so that a small set of runs is learned properly.
        for _ in 0..3 {
            for (run, steps) in runs.iter().zip(&coded) {
                if run.task == *task {
                    continue;
                }

                for (code, action, earned) in steps {
                    learner.learn(code, *action, *earned);

                    sums[action.index()] += earned;
                    seen[action.index()] += 1;
                }
            }
        }

        for (run, steps) in runs.iter().zip(&coded) {
            if run.task != *task {
                continue;
            }

            for (code, action, earned) in steps {
                let average = sums[action.index()] / seen[action.index()].max(1) as f32;
                let values = learner.values(code);

                learned_error += f64::from((values[action.index()] - earned).powi(2));
                average_error += f64::from((average - earned).powi(2));

                // Was the action the model took the one valued highest?
                let best = (0..5)
                    .max_by(|left, right| values[*left].total_cmp(&values[*right]))
                    .unwrap_or(0);

                ranked_right += u32::from(best == action.index());
                count += 1;
            }
        }
    }

    let count = f64::from(count.max(1));

    println!(
        "   held-out tasks: squared error {:.4} (always predicting the action's average: {:.4}); \
         top-valued action was the one taken {:.0}% of steps",
        learned_error / count,
        average_error / count,
        100.0 * f64::from(ranked_right) / count
    );
}

/// Question 1: the controller as it would have run, one task after another.
fn in_order(runs: &[Run], encoder: Box<dyn Encoder>) {
    let controller = Controller::new(encoder, None);

    let mut advised = [0_u32; 5];
    let mut followed = 0_u32;
    let mut early_finish = 0_u32;
    let mut steps = 0_u32;

    for run in runs {
        // The advice waiting to be compared with the model's next choice.
        let mut pending: Option<Action> = None;
        let mut after_tools = false;

        for event in &run.events {
            if let Event::ModelResponded {
                step,
                message,
                stop_reason,
                ..
            } = event
            {
                if after_tools && let Some(advice) = controller.advise(step - 1, MAX_STEPS) {
                    let action = Action::ALL
                        .into_iter()
                        .find(|action| advice.detail["advised"] == action.label());

                    pending = action;
                }

                let taken = Action::of_response(message, stop_reason);

                if let Some(action) = pending.take() {
                    advised[action.index()] += 1;
                    followed += u32::from(action == taken);

                    // "Finish" said when the model still had work to do.
                    early_finish += u32::from(action == Action::Finish && taken != Action::Finish);
                }

                after_tools = *stop_reason == StopReason::ToolUse;
                steps += 1;
            }

            // The eval's verdict stands in for the gate's.
            match event {
                Event::RunFinished {
                    outcome,
                    steps,
                    usage,
                    duration_ms,
                    ..
                } => controller.record(&Event::RunFinished {
                    outcome: *outcome,
                    steps: *steps,
                    usage: *usage,
                    duration_ms: *duration_ms,
                    verified: Some(run.solved),
                }),

                other => controller.record(other),
            }
        }
    }

    let total: u32 = advised.iter().sum();

    let breakdown: Vec<String> = Action::ALL
        .iter()
        .filter(|action| advised[action.index()] > 0)
        .map(|action| format!("{} {}", action.label(), advised[action.index()]))
        .collect();

    println!(
        "   in order: spoke {total} times in {steps} steps ({}); the model did that anyway {followed} \
         times; said \"finish\" when the model went on working {early_finish} times",
        if breakdown.is_empty() {
            "nothing".to_string()
        } else {
            breakdown.join(", ")
        }
    );
}
