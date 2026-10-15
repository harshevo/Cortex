//! Made-up episodes for the demo, so the memory space has something in it.
//!
//! These are only ever written to the demo's throwaway project.

use std::path::Path;

use anyhow::{Context, Result};
use cortex_brain::memory::consolidate::derive_facts;
use cortex_brain::memory::{Episode, Store};
use cortex_core::Outcome;
use cortex_core::trajectory::unix_millis;

/// How a seeded task ended.
#[derive(Clone, Copy)]
enum Status {
    /// Finished and the project's checks passed.
    Verified,

    /// Finished; nothing checked the result.
    Completed,

    /// Finished, but the project's checks still failed.
    Unverified,

    /// Ran out of steps.
    Stopped,

    /// The run ended with an error.
    Failed,
}

/// The check every seeded task is supposed to have run.
const CHECK: &str = "python3 -m pytest -q";

/// The seeded tasks are spread over this many days before now.
const DAYS: u64 = 40;

/// `(instruction, files, status, steps)`
const SEEDS: &[(&str, &[&str], Status, u32)] = &[
    (
        "Fix the off-by-one in the RSI window",
        &["indicators/rsi.py"],
        Status::Verified,
        6,
    ),
    (
        "Fix EMA seed value using the wrong first sample",
        &["indicators/ema.py"],
        Status::Verified,
        5,
    ),
    (
        "MACD histogram has the wrong sign",
        &["indicators/macd.py"],
        Status::Verified,
        7,
    ),
    (
        "Bollinger bands use population instead of sample deviation",
        &["indicators/bollinger.py"],
        Status::Completed,
        8,
    ),
    (
        "ATR ignores the previous close on gaps",
        &["indicators/atr.py"],
        Status::Unverified,
        11,
    ),
    (
        "Fix division by zero in Sharpe ratio when returns are flat",
        &["metrics/sharpe.py"],
        Status::Verified,
        4,
    ),
    (
        "Max drawdown is computed from the wrong peak",
        &["metrics/drawdown.py"],
        Status::Verified,
        6,
    ),
    (
        "Sortino ratio should only use downside deviation",
        &["metrics/sortino.py"],
        Status::Completed,
        7,
    ),
    (
        "Win rate counts break-even trades as wins",
        &["metrics/winrate.py"],
        Status::Verified,
        5,
    ),
    (
        "Annualised return uses 365 instead of 252 trading days",
        &["metrics/returns.py"],
        Status::Verified,
        4,
    ),
    (
        "Add CSV export for the trade journal",
        &["journal/export.py"],
        Status::Completed,
        9,
    ),
    (
        "Export the trade journal as JSON",
        &["journal/export.py"],
        Status::Completed,
        7,
    ),
    (
        "Import trades from a broker CSV with a different column order",
        &["journal/importer.py"],
        Status::Unverified,
        14,
    ),
    (
        "Trade journal drops rows with empty notes",
        &["journal/importer.py"],
        Status::Verified,
        6,
    ),
    (
        "Add tags to journal entries",
        &["journal/models.py"],
        Status::Completed,
        10,
    ),
    (
        "Parse dates with timezone offsets in the importer",
        &["journal/dates.py"],
        Status::Verified,
        8,
    ),
    (
        "Timestamps are shifted by an hour during daylight saving",
        &["journal/dates.py"],
        Status::Stopped,
        22,
    ),
    (
        "Handle weekends when computing the next trading day",
        &["calendar/sessions.py"],
        Status::Verified,
        6,
    ),
    (
        "Market holidays are missing for 2026",
        &["calendar/holidays.py"],
        Status::Completed,
        5,
    ),
    (
        "Rename config key api_token to api_key everywhere",
        &["config/settings.py"],
        Status::Verified,
        9,
    ),
    (
        "Load config from environment variables before the file",
        &["config/loader.py"],
        Status::Completed,
        8,
    ),
    (
        "Config loader crashes on a missing optional section",
        &["config/loader.py"],
        Status::Verified,
        5,
    ),
    (
        "Add a setting for the default currency",
        &["config/settings.py"],
        Status::Completed,
        6,
    ),
    (
        "Validate the config file and report every error at once",
        &["config/validate.py"],
        Status::Unverified,
        13,
    ),
    (
        "Retry broker API requests on a 429 with backoff",
        &["broker/client.py"],
        Status::Verified,
        10,
    ),
    (
        "Broker client leaks connections after a timeout",
        &["broker/client.py"],
        Status::Stopped,
        25,
    ),
    (
        "Cache instrument metadata from the broker for a day",
        &["broker/cache.py"],
        Status::Completed,
        9,
    ),
    (
        "Websocket price feed reconnects in a tight loop",
        &["broker/feed.py"],
        Status::Failed,
        3,
    ),
    (
        "Parse fractional share quantities from the broker",
        &["broker/parse.py"],
        Status::Verified,
        5,
    ),
    (
        "Extract position sizing into its own module",
        &["risk/sizing.py"],
        Status::Completed,
        12,
    ),
    (
        "Position size ignores the account currency",
        &["risk/sizing.py"],
        Status::Verified,
        7,
    ),
    (
        "Stop loss distance should respect the tick size",
        &["risk/stops.py"],
        Status::Verified,
        6,
    ),
    (
        "Add a maximum daily loss guard",
        &["risk/guards.py"],
        Status::Completed,
        11,
    ),
    (
        "Risk per trade is rounded before it is summed",
        &["risk/sizing.py"],
        Status::Verified,
        5,
    ),
    (
        "Speed up the backtest loop by vectorising fills",
        &["backtest/engine.py"],
        Status::Unverified,
        18,
    ),
    (
        "Backtest applies commission twice on partial fills",
        &["backtest/fills.py"],
        Status::Verified,
        8,
    ),
    (
        "Slippage model ignores order side",
        &["backtest/slippage.py"],
        Status::Verified,
        6,
    ),
    (
        "Backtest report omits open positions at the end",
        &["backtest/report.py"],
        Status::Completed,
        7,
    ),
    (
        "Add a walk-forward split to the backtester",
        &["backtest/splits.py"],
        Status::Stopped,
        30,
    ),
    (
        "Chart endpoint returns candles in the wrong order",
        &["api/charts.py"],
        Status::Verified,
        5,
    ),
    (
        "Add pagination to the trades endpoint",
        &["api/trades.py"],
        Status::Completed,
        9,
    ),
    (
        "Trades endpoint returns 500 for an unknown symbol",
        &["api/trades.py"],
        Status::Verified,
        6,
    ),
    (
        "Add an endpoint for the equity curve",
        &["api/equity.py"],
        Status::Completed,
        10,
    ),
    (
        "Request validation accepts negative quantities",
        &["api/schemas.py"],
        Status::Verified,
        4,
    ),
    (
        "Flaky test in the scheduler depends on wall-clock time",
        &["tests/test_scheduler.py"],
        Status::Verified,
        9,
    ),
    (
        "Add tests for the RSI edge cases",
        &["tests/test_rsi.py"],
        Status::Completed,
        6,
    ),
    (
        "Test fixtures share state between runs",
        &["tests/conftest.py"],
        Status::Unverified,
        12,
    ),
    (
        "Add tests for the CSV importer",
        &["tests/test_importer.py"],
        Status::Completed,
        8,
    ),
];

/// How many episodes [`plant`] writes.
#[cfg(test)]
pub fn count() -> usize {
    SEEDS.len()
}

/// Writes the seeded episodes into the memory at `path`, and the facts
/// the engine's own consolidation draws from them.
pub fn plant(path: &Path, scope: &str) -> Result<()> {
    let store = Store::open(path)
        .with_context(|| format!("cannot open the memory at {}", path.display()))?;

    let now = unix_millis() as u64;
    let episodes = episodes(now);

    for episode in &episodes {
        store.add_episode(scope, episode)?;
    }

    store.replace_derived_facts(scope, &derive_facts(&episodes, now))?;

    Ok(())
}

fn episodes(now: u64) -> Vec<Episode> {
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

    SEEDS
        .iter()
        .enumerate()
        .map(|(index, (instruction, files, status, steps))| {
            let age = DAYS * DAY_MS * (SEEDS.len() - index) as u64 / SEEDS.len() as u64;

            let (outcome, verified) = match status {
                Status::Verified => (Some(Outcome::Completed), Some(true)),
                Status::Completed => (Some(Outcome::Completed), None),
                Status::Unverified => (Some(Outcome::Completed), Some(false)),
                Status::Stopped => (Some(Outcome::StepLimit), None),
                Status::Failed => (None, None),
            };

            let checked = verified.is_some();

            Episode {
                id: 0,
                created_ms: now - age,
                instruction: instruction.to_string(),
                outcome,
                verified,
                rejections: 0,
                steps: *steps,
                tokens: u64::from(*steps) * 4_200,
                duration_ms: u64::from(*steps) * 5_200,
                files_changed: files.iter().map(|file| file.to_string()).collect(),
                files_read: Vec::new(),
                commands_worked: Vec::new(),
                calculations: Vec::new(),
                dead_ends: Vec::new(),
                checks_run: if checked {
                    vec![CHECK.to_string()]
                } else {
                    Vec::new()
                },
                checks_failed: if verified == Some(false) {
                    vec![CHECK.to_string()]
                } else {
                    Vec::new()
                },
                report: "Seeded for the demo. No model ran this task.".to_string(),
            }
        })
        .collect()
}
