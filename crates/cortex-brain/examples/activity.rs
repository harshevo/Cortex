//! Prints how active a brain is for a few situations, for tuning.
//!
//!     cargo run --release -p cortex-brain --example activity -- fly
//!     cargo run --release -p cortex-brain --example activity -- tiny 0.12 0.2 6.0
//!
//! The optional numbers are the central gain, Kenyon gain and global
//! inhibition.

use std::time::Instant;

use cortex_brain::situation::FEATURES;
use cortex_brain::{Brain, BrainConfig};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let mut config = match arguments.first().map(String::as_str) {
        Some("fly") => BrainConfig::fly(),
        _ => BrainConfig::tiny(),
    };

    let number = |position: usize| {
        arguments
            .get(position)
            .and_then(|text| text.parse::<f32>().ok())
    };

    config.central_gain = number(1).unwrap_or(config.central_gain);
    config.kenyon_gain = number(2).unwrap_or(config.kenyon_gain);
    config.global_inhibition = number(3).unwrap_or(config.global_inhibition);

    let started = Instant::now();

    let mut brain = match arguments.iter().find(|argument| argument.ends_with(".csv")) {
        Some(path) => {
            Brain::from_file(config, path.as_ref()).expect("could not read the connectome")
        }
        None => Brain::generate(config),
    };

    println!(
        "{} neurons, {} connections, {} MB, built in {} ms",
        brain.config().neurons,
        brain.connectome().synapses(),
        brain.connectome().memory_bytes() / 1_000_000,
        started.elapsed().as_millis()
    );

    separation(&mut brain);

    for active in [2, 4, 8, 12] {
        let mut features = [0.0; FEATURES];

        for feature in features.iter_mut().take(active) {
            *feature = 1.0;
        }

        let started = Instant::now();

        brain.respond(&features);

        println!(
            "{active:>2} features on: {:?}, {} ms",
            brain.last_activity(),
            started.elapsed().as_millis()
        );
    }
}

/// How alike the codes of alike and unalike situations are. A useful
/// encoder keeps near situations near and far ones apart.
fn separation(brain: &mut Brain) {
    let situation = |on: &[usize]| {
        let mut features = [0.0; FEATURES];

        for &index in on {
            features[index] = 1.0;
        }

        features
    };

    let base = brain.respond(&situation(&[0, 2, 3, 13]));
    let near = brain.respond(&situation(&[0, 2, 3, 13, 16]));
    let far = brain.respond(&situation(&[0, 4, 5, 8, 14]));

    println!(
        "code similarity: near situation {:.2}, far situation {:.2}",
        cosine(&base, &near),
        cosine(&base, &far)
    );
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();

    let length = |code: &[f32]| code.iter().map(|value| value * value).sum::<f32>().sqrt();

    dot / (length(left) * length(right)).max(f32::MIN_POSITIVE)
}
