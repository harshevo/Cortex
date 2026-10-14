//! Replays saved runs through the network and scores what it expected.
//!
//! Before each moment the network says what it expects. This holds that
//! against what came, over every saved run in a folder, and prints:
//!
//! - recall: the share of each moment that was expected;
//! - precision: the share of what was expected that came;
//! - the same two for guessing the most common percepts so far, as many
//!   guesses as the network made. The network has learned something only
//!   where it beats this.
//!
//! Runs are replayed in the order of their names, and the first half is
//! not scored: a network that has met nothing can expect nothing.
//!
//! ```text
//! cargo run --release -p cortex-living --example replay -- eval/results
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cortex_core::Event;
use cortex_living::{Network, percept};

#[derive(Default)]
struct Score {
    moments: u32,
    recall: f64,
    precision: f64,
    guesses: u32,
}

impl Score {
    fn add(&mut self, expected: &[String], came: &[String]) {
        let met = came.iter().filter(|percept| expected.contains(percept)).count() as f64;

        self.moments += 1;
        self.recall += met / came.len() as f64;

        if !expected.is_empty() {
            self.guesses += 1;
            self.precision += met / expected.len() as f64;
        }
    }

    fn print(&self, name: &str) {
        println!(
            "{name:<10} recall {:.3}   precision {:.3}   (spoke in {} of {} moments)",
            self.recall / self.moments.max(1) as f64,
            self.precision / self.guesses.max(1) as f64,
            self.guesses,
            self.moments,
        );
    }
}

fn main() {
    let folder = std::env::args()
        .nth(1)
        .expect("usage: replay <folder of saved runs>");

    let mut logs = Vec::new();

    collect(Path::new(&folder), &mut logs);
    logs.sort();

    let mut network = Network::new();
    let mut common: HashMap<String, u32> = HashMap::new();

    let mut learned = Score::default();
    let mut guessed = Score::default();

    let scored_from = logs.len() / 2;

    for (position, log) in logs.iter().enumerate() {
        let Ok(text) = fs::read_to_string(log) else {
            continue;
        };

        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Event>(line) else {
                continue;
            };

            for percepts in percept::moments(&event) {
                let mut came = percepts.clone();

                came.sort();
                came.dedup();

                let expected = network.expecting();

                if position >= scored_from {
                    let mut ranked: Vec<(&String, &u32)> = common.iter().collect();

                    ranked.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));

                    let guesses: Vec<String> = ranked
                        .into_iter()
                        .take(expected.len())
                        .map(|(percept, _)| percept.clone())
                        .collect();

                    learned.add(&expected, &came);
                    guessed.add(&guesses, &came);
                }

                for percept in &came {
                    *common.entry(percept.clone()).or_default() += 1;
                }

                network.live(&percepts);
            }
        }

        // Between runs there is time to rest.
        for _ in 0..8 {
            if !network.rest() {
                break;
            }
        }
    }

    let meters = network.meters();

    println!(
        "{} runs, {} moments, {} neurons, {} synapses",
        logs.len(),
        meters.moments,
        meters.neurons,
        meters.synapses
    );

    learned.print("network");
    guessed.print("common");
}

fn collect(folder: &Path, logs: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect(&path, logs);
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            logs.push(path);
        }
    }
}
