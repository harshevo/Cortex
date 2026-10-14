//! `cortex usage`

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use cortex_core::ledger::{
    CostEstimate, Ledger, MonthReport, PriceTable, Summary, Totals, local_month,
};
use cortex_core::trajectory::unix_millis;
use serde_json::json;

use crate::args::UsageArgs;
use crate::features;

/// Sessions and models listed at most.
const MAX_LISTED: usize = 10;

pub fn execute(args: UsageArgs) -> Result<ExitCode> {
    let folder = args
        .records_dir
        .clone()
        .or_else(features::default_records_folder)
        .context(
            "this machine has no home directory; say where the records are with `--records-dir`",
        )?;

    let ledger = Ledger::open(folder.join("usage"));
    let prices = PriceTable::load(&folder.join("prices.json"));

    let (offset, offset_known) = match args.utc_offset {
        Some(offset) => (offset, true),
        None => match utc_offset_minutes() {
            Some(offset) => (offset, true),
            None => (0, false),
        },
    };

    let now = unix_millis() as u64;

    let (year, month) = match &args.month {
        Some(month) => parse_month(month)?,
        None => local_month(now, offset),
    };

    let report = ledger.month(year, month, offset, &prices);
    let streak = ledger.streak_days(now, offset);

    if args.json {
        let whole = json!({
            "folder": ledger.folder(),
            "utc_offset_minutes": offset,
            "streak_days": streak,
            "month": report,
        });

        println!("{}", serde_json::to_string_pretty(&whole)?);
    } else {
        print!("{}", describe(&report, streak));

        if !offset_known {
            println!(
                "\nYour clock's offset from UTC could not be found out, so days are UTC days. \
                 Give it with `--utc-offset`."
            );
        }

        println!("\nRecords: {}", ledger.folder().display());
    }

    Ok(ExitCode::SUCCESS)
}

fn parse_month(text: &str) -> Result<(i32, u32)> {
    let parsed = text
        .split_once('-')
        .and_then(|(year, month)| Some((year.parse::<i32>().ok()?, month.parse::<u32>().ok()?)))
        .filter(|(_, month)| (1..=12).contains(month));

    match parsed {
        Some(month) => Ok(month),
        None => bail!("`--month` is a year and a month, such as 2026-10"),
    }
}

/// Minutes the system's clock is ahead of UTC, as the `date` command
/// says. `None` where there is no such command or its answer cannot be
/// read.
pub fn utc_offset_minutes() -> Option<i32> {
    let output = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()?;

    parse_offset(String::from_utf8_lossy(&output.stdout).trim())
}

/// An offset written `+0530` or `-0700`, in minutes.
fn parse_offset(text: &str) -> Option<i32> {
    let (sign, digits) = match text.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };

    if digits.len() != 4 {
        return None;
    }

    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;

    Some(sign * (hours * 60 + minutes))
}

/// A month as a person reads it: the calendar, then the summary.
pub fn describe(report: &MonthReport, streak: u32) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    let mut text = format!(
        "{} {}: tokens used each day\n\n",
        MONTHS[(report.month as usize - 1).min(11)],
        report.year
    );

    for day in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
        text.push_str(&format!("{day:>10}"));
    }

    text.push('\n');

    // Two lines to a week: the day of the month, and its tokens under it.
    let mut numbers = "          ".repeat(report.first_weekday as usize);
    let mut amounts = numbers.clone();

    for (index, day) in report.days.iter().enumerate() {
        let tokens = day.totals.tokens();

        let amount = if tokens == 0 {
            ".".to_string()
        } else {
            count(tokens)
        };

        numbers.push_str(&format!("{:>10}", index + 1));
        amounts.push_str(&format!("{amount:>10}"));

        let week_over = (report.first_weekday as usize + index + 1).is_multiple_of(7);

        if week_over || index + 1 == report.days.len() {
            text.push_str(&format!("{numbers}\n{amounts}\n\n"));

            numbers.clear();
            amounts.clear();
        }
    }

    text.push_str(&summary(&report.summary, streak));

    text
}

fn summary(summary: &Summary, streak: u32) -> String {
    let totals = &summary.totals;

    if totals.calls == 0 {
        return format!("Nothing was used this month. {}\n", streak_text(streak));
    }

    let mut text = format!(
        "{} tokens in {} model calls, over {} sessions on {} days.\n",
        count(totals.tokens()),
        totals.calls,
        summary.session_count,
        summary.days.len(),
    );

    text.push_str(&format!("  {}\n", split(totals)));

    if let Some(per_call) = &summary.per_call {
        text.push_str(&format!(
            "  an average call: {} in, {} out\n",
            count(per_call.input_tokens as u64),
            count(per_call.output_tokens as u64),
        ));
    }

    if let Some(peak) = &summary.peak_day {
        text.push_str(&format!(
            "Busiest day: {} ({} tokens).",
            peak.date,
            count(peak.tokens)
        ));
    }

    if let Some(peak) = &summary.peak_hour {
        text.push_str(&format!(
            " Busiest hour of the day: {:02}:00 to {:02}:00.",
            peak.hour,
            (peak.hour + 1) % 24
        ));
    }

    text.push_str(&format!("\n{}\n", streak_text(streak)));
    text.push_str(&format!("Cost: {}\n", cost(&summary.cost)));

    text.push_str("\nModels\n");

    for model in summary.models.iter().take(MAX_LISTED) {
        text.push_str(&format!(
            "  {:<28} {:>8} tokens  {:>3.0}%  {:>5} calls  {}\n",
            clip(&model.model, 28),
            count(model.totals.tokens()),
            model.share * 100.0,
            model.totals.calls,
            cost(&model.cost),
        ));
    }

    text.push_str("\nSessions, newest first\n");

    for session in summary.sessions.iter().take(MAX_LISTED) {
        let title = session.title.as_deref().unwrap_or(&session.id);

        text.push_str(&format!(
            "  {:<44} {:>8} tokens  {:>4} calls  {}\n",
            clip(title, 44),
            count(session.totals.tokens()),
            session.totals.calls,
            clip(&session.model, 24),
        ));
    }

    if summary.sessions.len() > MAX_LISTED {
        text.push_str(&format!(
            "  and {} more; `--json` lists them all\n",
            summary.sessions.len() - MAX_LISTED
        ));
    }

    text
}

/// The four kinds of token, and how much of the input came from the
/// provider's cache.
fn split(totals: &Totals) -> String {
    let mut text = format!(
        "{} input, {} input from cache, {} written to cache, {} output",
        count(totals.input_tokens),
        count(totals.cached_input_tokens),
        count(totals.cache_write_tokens),
        count(totals.output_tokens),
    );

    if let Some(share) = totals.cache_hit_share() {
        text.push_str(&format!("; {:.0}% of input came from cache", share * 100.0));
    }

    text
}

fn streak_text(streak: u32) -> String {
    match streak {
        0 => "No run of days in a row at the moment.".to_string(),
        1 => "Used 1 day in a row.".to_string(),
        days => format!("Used {days} days in a row."),
    }
}

/// A cost as far as it is known. Never a guess for a model without a
/// price.
fn cost(cost: &CostEstimate) -> String {
    let mut parts = Vec::new();

    if let Some(usd) = cost.usd {
        parts.push(format!("about ${usd:.2} at list price"));
    }

    if cost.subscription_tokens > 0 {
        parts.push(format!(
            "{} tokens covered by a subscription",
            count(cost.subscription_tokens)
        ));
    }

    if cost.unpriced_tokens > 0 {
        parts.push(format!(
            "{} tokens of models with no known price (add them to prices.json)",
            count(cost.unpriced_tokens)
        ));
    }

    if parts.is_empty() {
        return "nothing to price".to_string();
    }

    parts.join(", ")
}

/// A count as a person reads it: 950, 12.3k, 1.2M.
fn count(value: u64) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => format!("{:.1}k", value as f64 / 1e3),
        _ => format!("{:.1}M", value as f64 / 1e6),
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let start: String = text.chars().take(max_chars.saturating_sub(3)).collect();

    format!("{start}...")
}

#[cfg(test)]
mod tests {
    use cortex_core::Usage;
    use cortex_core::ledger::{CallKind, Record, days_from_civil};

    use super::*;

    #[test]
    fn an_offset_is_read_as_the_date_command_writes_it() {
        assert_eq!(parse_offset("+0530"), Some(330));
        assert_eq!(parse_offset("-0700"), Some(-420));
        assert_eq!(parse_offset("+0000"), Some(0));
        assert_eq!(parse_offset("UTC"), None);
        assert_eq!(parse_offset(""), None);
    }

    #[test]
    fn a_month_is_a_year_and_a_month() {
        assert_eq!(parse_month("2026-10").unwrap(), (2026, 10));
        assert!(parse_month("2026-13").is_err());
        assert!(parse_month("October").is_err());
    }

    #[test]
    fn the_calendar_puts_each_day_under_its_weekday() {
        let folder = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(folder.path());

        let noon = |day: u32| (days_from_civil(2026, 10, day) * 86_400_000 + 43_200_000) as u64;

        for (day, session, tokens) in [(1, "a", 1_500), (14, "b", 2_000_000), (14, "c", 900)] {
            ledger
                .append(&Record {
                    ts_ms: noon(day),
                    session: session.to_string(),
                    place: "/work".to_string(),
                    provider: "anthropic".to_string(),
                    model: "claude-opus-5-5".to_string(),
                    kind: CallKind::Step,
                    usage: Usage {
                        input_tokens: tokens,
                        ..Usage::default()
                    },
                    duration_ms: 1,
                    calls: 1,
                    title: (session == "b").then(|| "Fix the rounding".to_string()),
                })
                .unwrap();
        }

        let report = ledger.month(2026, 10, 0, &PriceTable::built_in());
        let text = describe(&report, 2);

        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines[0], "October 2026: tokens used each day");

        // The first of October 2026 was a Thursday: the fourth column.
        assert_eq!(
            lines[3].trim_end(),
            format!("{:>40}{:>10}{:>10}{:>10}", 1, 2, 3, 4)
        );
        assert_eq!(
            lines[4].trim_end(),
            format!("{:>40}{:>10}{:>10}{:>10}", "1.5k", ".", ".", ".")
        );

        assert!(text.contains("2.0M tokens in 3 model calls, over 3 sessions on 2 days."));
        assert!(text.contains("Busiest day: 2026-10-14 (2.0M tokens)."));
        assert!(text.contains("Busiest hour of the day: 12:00 to 13:00."));
        assert!(text.contains("Used 2 days in a row."));
        assert!(text.contains("Cost: about $8.01 at list price"));
        assert!(text.contains("Fix the rounding"));
    }

    #[test]
    fn a_month_with_nothing_in_it_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let report = Ledger::open(folder.path()).month(2026, 2, 0, &PriceTable::empty());

        let text = describe(&report, 0);

        assert!(text.contains("Nothing was used this month."));
        assert_eq!(report.days.len(), 28);
    }
}
