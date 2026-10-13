//! `cortex mind`

use std::process::ExitCode;

use anyhow::{Result, bail};
use cortex_brain::Mind;
use serde_json::Value;

use crate::args::MindArgs;

pub fn execute(args: MindArgs) -> Result<ExitCode> {
    let mind = Mind::new(&args.mind_url, "");

    let Some(report) = mind.health() else {
        bail!(
            "the mind's service at {} is not running. It is started by `cortex run` and `cortex \
             chat`, or from the brain project with `python3 -m cortex.brain.server`",
            args.mind_url
        );
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", describe(&report));
    }

    Ok(ExitCode::SUCCESS)
}

/// The service's report, as a person reads it.
pub fn describe(report: &Value) -> String {
    let count = |name: &str| report[name].as_u64().unwrap_or_default();

    let share = |value: &Value| match value.as_f64() {
        Some(share) => format!("{:.0}%", share * 100.0),
        None => "-".to_string(),
    };

    let mut text = format!(
        "The mind has followed {} runs: {} steps, {} still remembered, {} rules drawn in sleep, \
         {} things you said.\n",
        count("tasks"),
        count("steps"),
        count("remembered"),
        count("rules"),
        count("told"),
    );

    text.push_str(&format!(
        "Its guess of the next action matched the model's at {} of the steps where it had one. \
         Its prediction of how a step would turn out was off by {:.2} on average (0 is exact, \
         0.5 is a coin).\n\n",
        share(&report["guess_matched_reply"]),
        report["mean_miss"].as_f64().unwrap_or(0.5),
    ));

    text.push_str(&format!(
        "{:<22}{:>6}{:>9}{:>9}{:>8}{:>11}{:>10}{:>10}\n",
        "", "runs", "checked", "passed", "steps", "tokens", "seconds", "mind ms"
    ));

    let arms = [("advised", "mind speaking"), ("control", "mind silent")];

    for (arm, label) in arms {
        let row = &report["measured"][arm];

        if row.is_null() {
            text.push_str(&format!("{label:<22}{:>6}\n", 0));

            continue;
        }

        let number = |name: &str, scale: f64, places: usize| match row[name].as_f64() {
            Some(value) => format!("{:.places$}", value / scale),
            None => "-".to_string(),
        };

        text.push_str(&format!(
            "{label:<22}{:>6}{:>9}{:>9}{:>8}{:>11}{:>10}{:>10}\n",
            row["runs"].as_u64().unwrap_or_default(),
            row["checked"].as_u64().unwrap_or_default(),
            share(&row["passed"]),
            number("steps", 1.0, 1),
            number("tokens", 1.0, 0),
            number("duration_ms", 1000.0, 1),
            number("mind_ms", 1.0, 0),
        ));
    }

    text.push_str(
        "\nAverages per run. `passed` is the share of checked runs whose checks passed, or that \
         you approved of. The mind is silent in a share of runs chosen at random \
         (`--mind-control`), so the two rows can be compared; with few runs in either the \
         difference is noise.\n",
    );

    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_report_compares_runs_with_the_mind_speaking_and_silent() {
        let report = json!({
            "tasks": 12, "steps": 80, "remembered": 75, "rules": 4, "told": 2,
            "guess_matched_reply": 0.4, "mean_miss": 0.12,
            "measured": {
                "advised": {"runs": 9, "checked": 8, "passed": 0.875, "steps": 7.5,
                            "tokens": 41234.0, "duration_ms": 61500.0, "mind_ms": 42.0},
            },
        });

        let text = describe(&report);

        assert!(text.contains("12 runs: 80 steps, 75 still remembered, 4 rules"));
        assert!(text.contains("matched the model's at 40%"));

        let advised = text
            .lines()
            .find(|line| line.starts_with("mind speaking"))
            .unwrap();
        let fields: Vec<&str> = advised.split_whitespace().skip(2).collect();

        assert_eq!(fields, ["9", "8", "88%", "7.5", "41234", "61.5", "42"]);
        assert!(text.contains("mind silent                0\n"));
    }
}
