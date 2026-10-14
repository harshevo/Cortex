//! What the agent on this phone has cost, and whether its learning parts
//! help, for the phones' Insights screen.
//!
//! Every session on the phone records its model calls and its runs under
//! the app's folder, as the desktop does under the home folder. These
//! read those records and hand them over in plain shapes.

use std::path::{Path, PathBuf};

use cortex_core::effect::{EffectLedger, Filter, Metric, SourceEffect, Verdict};
use cortex_core::ledger::{CostEstimate, Ledger, PriceTable};

/// One day of the month.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageDayView {
    /// `YYYY-MM-DD`, local.
    pub date: String,

    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub calls: u64,
    pub sessions: u64,

    /// The cost in words: "$0.42", "subscription", "no price known".
    pub cost: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageSessionView {
    pub id: String,
    pub title: String,
    pub model: String,
    pub tokens: u64,
    pub cost: String,

    /// Milliseconds since 1970 of its first and last call.
    pub first_ms: u64,
    pub last_ms: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageModelView {
    pub model: String,
    pub provider: String,
    pub tokens: u64,

    /// From 0 to 1.
    pub share: f64,
    pub cost: String,
}

/// One month of use.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageMonthView {
    pub year: i32,
    pub month: u32,

    /// The weekday of the first, with Monday as 0.
    pub first_weekday: u8,

    /// Every day of the month, used or not.
    pub days: Vec<UsageDayView>,

    pub tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: u64,
    pub sessions: u64,
    pub cost: String,

    /// What the cost leaves out, in words.
    pub cost_note: String,

    /// Days in a row with use, up to today.
    pub streak: u32,

    pub favourite_model: Option<String>,
    pub peak_day: Option<String>,
    pub peak_day_tokens: u64,

    /// 0 to 23, local.
    pub peak_hour: Option<u8>,

    /// Tokens in each hour of the day, midnight first.
    pub hours: Vec<u64>,

    /// The share of input served from the provider's cache.
    pub cache_hit: Option<f64>,

    pub models: Vec<UsageModelView>,

    /// Newest first, at most 100.
    pub recent: Vec<UsageSessionView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VerdictView {
    Helps,
    Hurts,
    NoDifferenceSeen,
    TooFewRuns,
}

/// One metric with an adviser speaking and with it held out.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MetricView {
    pub name: String,

    /// A share from 0 to 1, as opposed to a count.
    pub rate: bool,
    pub higher_is_better: bool,

    pub with_mean: Option<f64>,
    pub with_runs: u64,
    pub without_mean: Option<f64>,
    pub without_runs: u64,

    /// With less without, and its 95% interval.
    pub difference: Option<f64>,
    pub low: Option<f64>,
    pub high: Option<f64>,

    pub verdict: VerdictView,
}

/// One week of the checks pass rate.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct WeekView {
    /// The Monday it begins on.
    pub week: String,
    pub with_rate: Option<f64>,
    pub without_rate: Option<f64>,
}

/// One adviser, or all of them together.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AdviserView {
    pub name: String,
    pub runs_spoke: u64,
    pub runs_held_out: u64,
    pub runs_nothing_to_say: u64,
    pub metrics: Vec<MetricView>,
    pub weeks: Vec<WeekView>,

    /// Not causal: advice that named something checkable, and the times
    /// the model did it next.
    pub comparable_observational: u64,
    pub followed_observational: u64,
}

/// Whether the advisers help, from the runs recorded on this phone.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EffectView {
    pub runs: u64,
    pub runs_in_experiment: u64,

    /// Runs needed on each side before a verdict is given.
    pub min_per_arm: u64,

    /// Comparisons with enough runs for a verdict. About one in twenty
    /// shows a difference by chance.
    pub comparisons: u32,

    pub overall: AdviserView,
    pub advisers: Vec<AdviserView>,
}

/// One month of the use recorded under `folder`, the app's own folder.
/// `utc_offset_minutes` is how far the phone's clock is ahead of UTC.
#[uniffi::export]
pub fn usage_month(
    folder: String,
    year: i32,
    month: u32,
    utc_offset_minutes: i32,
    now_ms: u64,
) -> UsageMonthView {
    let records = records(&folder);
    let ledger = Ledger::open(records.join("usage"));
    let prices = PriceTable::load(&records.join("prices.json"));

    let report = ledger.month(year, month, utc_offset_minutes, &prices);
    let summary = &report.summary;

    UsageMonthView {
        year: report.year,
        month: report.month,
        first_weekday: report.first_weekday,
        days: report
            .days
            .iter()
            .map(|day| UsageDayView {
                date: day.date.clone(),
                input_tokens: day.totals.input_tokens + day.totals.cache_write_tokens,
                cached_tokens: day.totals.cached_input_tokens,
                output_tokens: day.totals.output_tokens,
                calls: day.totals.calls,
                sessions: day.sessions,
                cost: cost_text(&day.cost),
            })
            .collect(),
        tokens: summary.totals.tokens(),
        input_tokens: summary.totals.all_input(),
        output_tokens: summary.totals.output_tokens,
        calls: summary.totals.calls,
        sessions: summary.session_count,
        cost: cost_text(&summary.cost),
        cost_note: cost_note(&summary.cost),
        streak: ledger.streak_days(now_ms, utc_offset_minutes),
        favourite_model: summary.favourite_model.clone(),
        peak_day: summary.peak_day.as_ref().map(|peak| peak.date.clone()),
        peak_day_tokens: summary.peak_day.as_ref().map_or(0, |peak| peak.tokens),
        peak_hour: summary.peak_hour.map(|hour| hour.hour),
        hours: summary.hours.iter().map(|hour| hour.tokens).collect(),
        cache_hit: summary.cache_hit_share,
        models: summary
            .models
            .iter()
            .map(|model| UsageModelView {
                model: model.model.clone(),
                provider: model.provider.clone(),
                tokens: model.totals.tokens(),
                share: model.share,
                cost: cost_text(&model.cost),
            })
            .collect(),
        recent: summary
            .sessions
            .iter()
            .take(100)
            .map(|session| UsageSessionView {
                id: session.id.clone(),
                title: session.title.clone().unwrap_or_else(|| session.id.clone()),
                model: session.model.clone(),
                tokens: session.totals.tokens(),
                cost: cost_text(&session.cost),
                first_ms: session.first_ms,
                last_ms: session.last_ms,
            })
            .collect(),
    }
}

/// The comparison of runs with each adviser speaking and held out, from
/// every run recorded under `folder`.
#[uniffi::export]
pub fn effect_report(folder: String, utc_offset_minutes: i32) -> EffectView {
    let ledger = EffectLedger::open(records(&folder).join("effect"));
    let report = ledger.report(0, u64::MAX, &Filter::default(), utc_offset_minutes);

    EffectView {
        runs: report.runs,
        runs_in_experiment: report.runs_in_experiment,
        min_per_arm: report.min_per_arm,
        comparisons: report.comparisons,
        overall: adviser(&report.overall),
        advisers: report.advisers.iter().map(adviser).collect(),
    }
}

/// Where the sessions on the phone keep their records: the engine's
/// `~/.cortex`, with the app's folder standing in for the home folder.
fn records(folder: &str) -> PathBuf {
    Path::new(folder).join(".cortex")
}

fn adviser(of: &SourceEffect) -> AdviserView {
    AdviserView {
        name: of.source.clone(),
        runs_spoke: of.runs_spoke,
        runs_held_out: of.runs_held_out,
        runs_nothing_to_say: of.runs_nothing_to_say,
        metrics: of
            .metrics
            .iter()
            .map(|metric| MetricView {
                name: metric.metric.label().to_string(),
                rate: metric.metric.is_rate(),
                higher_is_better: metric.higher_is_better,
                with_mean: metric.spoke.mean,
                with_runs: metric.spoke.n,
                without_mean: metric.held_out.mean,
                without_runs: metric.held_out.n,
                difference: metric.difference,
                low: metric.interval.map(|[low, _]| low),
                high: metric.interval.map(|[_, high]| high),
                verdict: match metric.verdict {
                    Verdict::Helps => VerdictView::Helps,
                    Verdict::Hurts => VerdictView::Hurts,
                    Verdict::NoDifferenceSeen => VerdictView::NoDifferenceSeen,
                    Verdict::TooFewRuns => VerdictView::TooFewRuns,
                },
            })
            .collect(),
        weeks: of
            .weekly
            .iter()
            .filter(|point| point.metric == Metric::ChecksPassed)
            .map(|point| WeekView {
                week: point.week.clone(),
                with_rate: point.spoke.mean,
                without_rate: point.held_out.mean,
            })
            .collect(),
        comparable_observational: of.comparable_observational,
        followed_observational: of.followed_observational,
    }
}

fn cost_text(cost: &CostEstimate) -> String {
    match cost.usd {
        Some(usd) if cost.is_complete() => format!("${usd:.2}"),
        Some(usd) => format!("${usd:.2}+"),
        None if cost.subscription_tokens > 0 && cost.unpriced_tokens == 0 => {
            "subscription".to_string()
        }
        None if cost.unpriced_tokens > 0 => "no price known".to_string(),
        None => "$0.00".to_string(),
    }
}

fn cost_note(cost: &CostEstimate) -> String {
    let mut parts = Vec::new();

    if cost.subscription_tokens > 0 {
        parts.push(format!("{} tokens on a subscription", cost.subscription_tokens));
    }

    if cost.unpriced_tokens > 0 {
        parts.push(format!("{} tokens without a known price", cost.unpriced_tokens));
    }

    if parts.is_empty() {
        "estimated from list prices".to_string()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_with_no_records_shows_an_empty_month_and_no_verdicts() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path().display().to_string();

        let month = usage_month(folder.clone(), 2026, 2, 330, 0);

        assert_eq!(month.days.len(), 28);
        assert_eq!(month.tokens, 0);
        assert_eq!(month.hours.len(), 24);
        assert_eq!(month.favourite_model, None);

        let effect = effect_report(folder, 330);

        assert_eq!(effect.runs, 0);
        assert!(effect.advisers.is_empty());
        assert!(
            effect
                .overall
                .metrics
                .iter()
                .all(|metric| metric.verdict == VerdictView::TooFewRuns)
        );
    }
}
