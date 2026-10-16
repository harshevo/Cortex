//! `cortex effect`

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use cortex_core::effect::{
    ArmStat, EffectLedger, EffectReport, Filter, Metric, MetricEffect, SourceEffect, Verdict,
};
use cortex_core::ledger::days_from_civil;

use crate::args::EffectArgs;
use crate::commands::usage::utc_offset_minutes;
use crate::features;

pub fn execute(args: EffectArgs) -> Result<ExitCode> {
    let folder = args
        .records_dir
        .clone()
        .or_else(features::default_records_folder)
        .context(
            "this machine has no home directory; say where the records are with `--records-dir`",
        )?;

    let ledger = EffectLedger::open(folder.join("effect"));

    let offset = args.utc_offset.or_else(utc_offset_minutes).unwrap_or(0);

    let from_ms = match &args.since {
        Some(day) => since(day, offset)?,
        None => 0,
    };

    let filter = Filter {
        // A project is recorded by its real path.
        place: args.project.as_ref().map(|project| {
            std::path::Path::new(project)
                .canonicalize()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| project.clone())
        }),
        model: args.model.clone(),
    };

    let report = ledger.report(from_ms, u64::MAX, &filter, offset);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", describe(&report));
        println!("\nRecords: {}", ledger.folder().display());
    }

    Ok(ExitCode::SUCCESS)
}

/// The start of a local day written `YYYY-MM-DD`, in milliseconds since
/// 1970.
fn since(day: &str, utc_offset_minutes: i32) -> Result<u64> {
    let parts: Vec<&str> = day.split('-').collect();

    let parsed = match parts.as_slice() {
        [year, month, day] => year
            .parse::<i32>()
            .ok()
            .zip(month.parse::<u32>().ok())
            .zip(day.parse::<u32>().ok()),

        _ => None,
    };

    let Some(((year, month), day)) =
        parsed.filter(|((_, month), day)| (1..=12).contains(month) && (1..=31).contains(day))
    else {
        bail!("`--since` is a day, such as 2026-10-01");
    };

    let midnight =
        days_from_civil(year, month, day) * 86_400_000 - i64::from(utc_offset_minutes) * 60_000;

    Ok(midnight.max(0) as u64)
}

/// The report as a person reads it.
pub fn describe(report: &EffectReport) -> String {
    let mut text = format!(
        "{} runs recorded, {} of them part of the experiment.\n",
        report.runs, report.runs_in_experiment
    );

    if report.runs_in_experiment == 0 {
        text.push_str(
            "Nothing to compare yet. Advisers are held out of a share of runs (`--holdout`, 0.25 \
             unless changed) when an adviser is enabled.\n",
        );

        return text;
    }

    text.push_str(&format!(
        "\nEach adviser is compared between the runs it spoke in and the runs it was held out \
         of, among runs in which it had something to say. Which was which was decided by \
         chance, so a difference beyond chance is the adviser's doing. A verdict needs {} runs \
         on each side.\n",
        report.min_per_arm
    ));

    for source in report.advisers.iter().chain([&report.overall]) {
        text.push_str(&table(source, "spoke", "held out"));
    }

    if !report.briefings.is_empty() {
        text.push_str("\nBriefings put before the task\n");

        for source in &report.briefings {
            text.push_str(&table(source, "shown", "held out"));
        }
    }

    text.push_str(&format!(
        "\n{} comparisons had enough runs for a verdict. At a 95% interval about one in twenty \
         shows a difference by chance alone.\n",
        report.comparisons
    ));

    text.push_str(
        "\nNot a controlled comparison, only what was seen: how often the model did what an \
         adviser said. A model follows advice more readily on an easy task, so this says \
         nothing about whether the advice helped.\n",
    );

    for source in &report.advisers {
        let followed = match source.comparable_observational {
            0 => "none of it named a next action that can be checked".to_string(),

            comparable => format!(
                "the model's next step matched {} of the {comparable} that named one",
                source.followed_observational
            ),
        };

        text.push_str(&format!(
            "  {:<22} advised {} times in {} runs; {followed}\n",
            source.source, source.times_advised_observational, source.runs_spoke
        ));
    }

    text
}

fn table(source: &SourceEffect, with: &str, without: &str) -> String {
    let mut text = format!(
        "\n{}: {with} in {} runs, {without} in {}, nothing to say in {}\n",
        source.source,
        source.runs_spoke,
        source.runs_held_out,
        source.runs_nothing_to_say + source.runs_held_out_silent,
    );

    text.push_str(&format!(
        "  {:<18} {:>16} {:>16} {:>11}  {:<24} {}\n",
        "", with, without, "difference", "95% interval", "verdict"
    ));

    for metric in &source.metrics {
        text.push_str(&row(metric));
    }

    text
}

fn row(effect: &MetricEffect) -> String {
    let metric = effect.metric;

    let arm = |arm: &ArmStat| match arm.mean {
        Some(mean) => format!("{} (n={})", amount(metric, mean), arm.n),
        None => "-".to_string(),
    };

    let difference = match effect.difference {
        Some(difference) => signed(metric, difference),
        None => "-".to_string(),
    };

    let interval = match effect.interval {
        Some([low, high]) => format!("{} to {}", signed(metric, low), signed(metric, high)),
        None => "-".to_string(),
    };

    let verdict = match effect.verdict {
        Verdict::Helps => "helps",
        Verdict::Hurts => "hurts",
        Verdict::NoDifferenceSeen => "no difference seen",
        Verdict::TooFewRuns => "too few runs",
    };

    format!(
        "  {:<18} {:>16} {:>16} {:>11}  {:<24} {verdict}\n",
        metric.label(),
        arm(&effect.spoke),
        arm(&effect.held_out),
        difference,
        interval,
    )
}

/// A value of a metric: a share as a percentage, anything else as a
/// number.
fn amount(metric: Metric, value: f64) -> String {
    if metric.is_rate() {
        format!("{:.0}%", value * 100.0)
    } else if value.abs() >= 10_000.0 {
        format!("{:.1}k", value / 1e3)
    } else {
        format!("{value:.1}")
    }
}

fn signed(metric: Metric, value: f64) -> String {
    let sign = if value > 0.0 { "+" } else { "" };

    format!("{sign}{}", amount(metric, value))
}

#[cfg(test)]
mod tests {
    use cortex_core::Usage;
    use cortex_core::effect::{RunRecord, SourceRun, analyse};

    use super::*;

    #[test]
    fn a_day_is_read_as_the_start_of_that_local_day() {
        // Midnight in India is 18:30 the evening before in UTC.
        assert_eq!(since("1970-01-02", 330).unwrap(), 86_400_000 - 330 * 60_000);
        assert_eq!(since("1970-01-02", 0).unwrap(), 86_400_000);
        assert!(since("yesterday", 0).is_err());
        assert!(since("2026-13-01", 0).is_err());
    }

    #[test]
    fn the_table_gives_both_sides_the_difference_and_a_verdict() {
        let runs: Vec<RunRecord> = (0..80u64)
            .map(|index| {
                let held_out = index % 2 == 1;

                RunRecord {
                    ts_ms: 1_791_158_400_000 + index,
                    seed: Some(index),
                    advisers: vec![SourceRun {
                        source: "intuition".to_string(),
                        held_out,
                        advised: 1,
                        comparable: 1,
                        followed: 1,
                    }],
                    verified: Some(!held_out || index % 4 == 1),
                    steps: if held_out { 20 } else { 10 },
                    usage: Usage {
                        input_tokens: 50_000,
                        ..Usage::default()
                    },
                    ..RunRecord::default()
                }
            })
            .collect();

        let text = describe(&analyse(&runs, 0));

        assert!(text.contains("80 runs recorded, 80 of them part of the experiment."));
        assert!(text.contains("intuition: spoke in 40 runs, held out in 40, nothing to say in 0"));

        let passed = text
            .lines()
            .find(|line| line.trim_start().starts_with("checks passed"))
            .unwrap();

        assert!(passed.contains("100% (n=40)"), "{passed}");
        assert!(passed.contains("50% (n=40)"), "{passed}");
        assert!(passed.contains("+50%"), "{passed}");
        assert!(passed.ends_with("helps"), "{passed}");

        let tokens = text
            .lines()
            .find(|line| line.trim_start().starts_with("tokens"))
            .unwrap();

        assert!(tokens.contains("50.0k (n=40)"), "{tokens}");
        assert!(tokens.ends_with("no difference seen"), "{tokens}");

        assert!(text.contains("the model's next step matched 40 of the 40 that named one"));
    }

    #[test]
    fn with_no_run_in_the_experiment_it_says_how_to_start() {
        let text = describe(&analyse(&[], 0));

        assert!(text.contains("Nothing to compare yet."));
    }
}
