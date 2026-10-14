//! Trains the three networks on made-up bars (or a CSV file given as the
//! argument) and prints how each did on held-out bars.
//!
//! `cargo run --release -p cortex-liquid --example path -- [file.csv]`

use cortex_liquid::{Forecaster, Net, PathSettings, parse_csv, synthetic};

fn main() {
    let bars = match std::env::args().nth(1) {
        Some(file) => {
            parse_csv(&std::fs::read_to_string(file).expect("readable"))
                .expect("bars")
                .bars
        }
        None => synthetic::patterned(2_000, 3),
    };

    let (forecaster, report) =
        Forecaster::train(&bars, &PathSettings::default(), &|_| {}).expect("trained");

    println!(
        "{} bars, {} scored, {:.0} ms",
        report.bars, report.scored, report.total_ms
    );

    for metrics in &report.metrics {
        let steps: Vec<String> = metrics
            .steps
            .iter()
            .map(|s| {
                format!(
                    "+{}: {:.1}% (skill {:+.3})",
                    s.step,
                    s.accuracy.unwrap_or(0.0) * 100.0,
                    s.skill
                )
            })
            .collect();

        println!(
            "{:9} {:6.0} ms  band {:.0}%  {}",
            metrics.net.name(),
            metrics.train_ms,
            metrics.band_hit.unwrap_or(0.0) * 100.0,
            steps.join("  ")
        );
    }

    let base: Vec<String> = report
        .baseline
        .iter()
        .map(|s| format!("+{}: {:.1}%", s.step, s.accuracy.unwrap_or(0.0) * 100.0))
        .collect();
    println!("same as last  {}", base.join("  "));

    let path = forecaster.path(Net::Ensemble);
    for candle in &path.candles {
        println!(
            "{:2} o {:.2} h {:.2} l {:.2} c {:.2} v {:.0}",
            candle.step, candle.open, candle.high, candle.low, candle.close, candle.volume
        );
    }
}
