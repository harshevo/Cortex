//! Analyses a CSV file of price bars and prints what was found.
//!
//! ```text
//! cargo run --release -p cortex-liquid --example analyse -- path/to.csv [horizon]
//! cargo run --release -p cortex-liquid --example analyse -- --pattern [horizon] [bars]
//! cargo run --release -p cortex-liquid --example analyse -- --walk [horizon] [bars]
//! ```
//!
//! `--pattern` and `--walk` use a made-up series in place of a file: one
//! with a pattern to find, and a random walk with none.

use std::cell::Cell;
use std::io::Write;
use std::time::Instant;

use cortex_liquid::{Model, Predictor, Settings, parse_csv, synthetic};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let Some(source) = arguments.first() else {
        eprintln!("usage: analyse <file.csv | --pattern | --walk> [horizon] [bars]");

        std::process::exit(2);
    };

    let number = |at: usize, otherwise: usize| {
        arguments
            .get(at)
            .and_then(|text| text.parse().ok())
            .unwrap_or(otherwise)
    };

    let settings = Settings {
        horizon: number(1, 1),
        ..Settings::default()
    };

    let made = number(2, 5_000);

    let bars = match source.as_str() {
        "--pattern" => synthetic::patterned(made, 1),
        "--walk" => synthetic::random_walk(made, 1),
        path => {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("cannot read {path}: {error}");

                    std::process::exit(1);
                }
            };

            let began = Instant::now();

            let parsed = match parse_csv(&text) {
                Ok(parsed) => parsed,
                Err(error) => {
                    eprintln!("cannot use {path}: {error}");

                    std::process::exit(1);
                }
            };

            println!(
                "read {} bars in {:.1} ms ({} rows skipped, {} repeated times dropped)",
                parsed.bars.len(),
                began.elapsed().as_secs_f64() * 1e3,
                parsed.skipped,
                parsed.duplicates
            );

            if let Some(problem) = &parsed.first_problem {
                println!("  first skipped row: {problem}");
            }

            for note in &parsed.notes {
                println!("  {note}");
            }

            parsed.bars
        }
    };

    println!(
        "{} bars, {} ahead, {} units, {} epochs, window {}, last {:.0}% held out",
        bars.len(),
        settings.horizon,
        settings.units,
        settings.epochs,
        settings.window,
        settings.test_share * 100.0
    );

    let shown = Cell::new(-1);

    let result = Model::train(&bars, &settings, &|progress| {
        let percent = (progress.fraction * 100.0) as i32;

        if percent >= shown.get() + 5 {
            shown.set(percent);

            eprint!("\r{percent:>3}% {:?}        ", progress.stage);
            std::io::stderr().flush().ok();
        }
    });

    eprintln!();

    let (model, analysis) = match result {
        Ok(done) => done,
        Err(error) => {
            eprintln!("cannot analyse: {error}");

            std::process::exit(1);
        }
    };

    println!(
        "\ntrained on the first {} bars, scored on the last {}\n",
        analysis.train_bars,
        analysis.bars - analysis.train_bars
    );

    println!(
        "{:<26} {:>8} {:>15} {:>9} {:>13} {:>12}",
        "predictor", "right", "99% interval", "p", "scored", "mean error"
    );

    let percent =
        |share: Option<f64>| share.map_or("-".to_string(), |x| format!("{:.1}%", x * 100.0));

    for score in &analysis.scores {
        let interval = match (score.accuracy_low, score.accuracy_high) {
            (Some(low), Some(high)) => format!("{:.1}-{:.1}%", low * 100.0, high * 100.0),
            _ => "-".to_string(),
        };

        println!(
            "{:<26} {:>8} {:>15} {:>9} {:>13} {:>12.6}",
            score.name,
            percent(score.accuracy),
            interval,
            score.p_value.map_or("-".to_string(), |p| {
                if p < 0.0001 {
                    format!("{p:.1e}")
                } else {
                    format!("{p:.4}")
                }
            }),
            format!("{} of {}", score.scored, score.bars),
            score.mean_abs_error
        );
    }

    println!("\nverdict: {:?}", analysis.verdict.edge);
    println!("{}", analysis.verdict.text);

    println!();

    for note in &analysis.notes {
        println!("note: {note}");
    }

    let forecast = &analysis.forecast;

    println!(
        "\nforecast for the next {} bar(s): {:?}, expected log return {:+.5}, the model's own \
         confidence {:.1}%",
        forecast.horizon,
        forecast.direction,
        forecast.expected_return,
        forecast.confidence * 100.0
    );

    println!(
        "dendritic network: now [{}], regime \"{}\", surprise {:.2}",
        forecast.dendritic.now.join(", "),
        forecast.dendritic.regime,
        forecast.dendritic.surprise
    );

    if forecast.dendritic.expectations.is_empty() {
        println!("  it expects nothing in particular of the next bar");
    }

    for expectation in &forecast.dendritic.expectations {
        println!(
            "  expects {} ({:.2})",
            expectation.label, expectation.confidence
        );
    }

    for (at, step) in forecast.dendritic.imagined.iter().enumerate() {
        println!(
            "  imagines step {}: {} ({:.2})",
            at + 1,
            step.labels.join(", "),
            step.confidence
        );
    }

    let growth = &analysis.growth;

    println!(
        "  grew to {} neurons, {} branches, {} synapses over {} bars; expected {:.1}% of each bar \
         against {:.1}% for guessing the commonest",
        growth.neurons,
        growth.segments,
        growth.synapses,
        growth.moments,
        growth.expected_right * 100.0,
        growth.baseline_right * 100.0
    );

    for (name, losses) in [
        ("liquid alone", &analysis.liquid_losses),
        ("liquid + dendritic", &analysis.combined_losses),
    ] {
        let list = |pick: &dyn Fn(&cortex_liquid::EpochLoss) -> f32| {
            losses
                .iter()
                .map(|loss| format!("{:.3}", pick(loss)))
                .collect::<Vec<_>>()
                .join(" ")
        };

        println!("\nloss by epoch, {name}");
        println!("  fitted:  {}", list(&|loss| loss.fit));
        println!("  checked: {}", list(&|loss| loss.check));
    }

    println!();

    let timings = &analysis.timings;

    println!(
        "timings: features {:.0} ms, dendritic network {:.0} ms, training both liquid networks \
         {:.0} ms, in all {:.0} ms",
        timings.features_ms, timings.dendritic_ms, timings.training_ms, timings.total_ms
    );

    let began = Instant::now();
    let saved = model.to_json();
    let saving = began.elapsed().as_secs_f64() * 1e3;

    let began = Instant::now();
    let loaded = Model::from_json(&saved).is_ok_and(|loaded| loaded == model);

    println!(
        "saved model: {} KB, written in {saving:.0} ms, read back {} in {:.0} ms",
        saved.len() / 1024,
        if loaded { "the same" } else { "DIFFERENT" },
        began.elapsed().as_secs_f64() * 1e3
    );

    if analysis.score(Predictor::Combined).is_none() {
        std::process::exit(1);
    }
}
