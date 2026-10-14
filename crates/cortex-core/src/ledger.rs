//! A lasting record of the tokens every model call used.
//!
//! A session's statistics are gone when its window closes. The ledger is
//! what is left: one line per model call, in one file per month, in a
//! folder the caller names (the apps use `~/.cortex/usage`). From it come
//! the totals per day, per session and per model, the busiest day and
//! hour, and an estimate of the cost where the price is known.
//!
//! Writing is one short append per call. A crash can leave the last line
//! of a file unfinished; such a line is skipped when reading and closed
//! off before the next one is written, so it never spoils a later record.
//!
//! Days are the person's own. The ledger stores times in UTC and every
//! question takes the offset of local time from UTC in minutes, which the
//! caller knows, so no time-zone data is needed here.
//!
//! # Prices
//!
//! A cost is only given for a model whose price is known. Prices are US
//! dollars per million tokens, looked up by the longest model-name prefix
//! that matches. A few are built in ([`PriceTable::built_in`]); a file at
//! `~/.cortex/prices.json` adds to them or replaces them:
//!
//! ```json
//! {
//!   "claude-opus-5-5": { "input": 4.0, "cached_input": 0.2, "cache_write": 5.0, "output": 20.0 },
//!   "my-hosted-model": { "input": 0.5, "output": 1.5 }
//! }
//! ```
//!
//! `cached_input` and `cache_write` may be left out; a call that used such
//! tokens then has no cost rather than a guessed one. Calls made through a
//! ChatGPT subscription have no price per token and are reported as
//! covered by the subscription.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::trajectory::{Event, TrajectorySink, unix_millis};
use crate::usage::Usage;

pub(crate) const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const MINUTE_MS: i64 = 60_000;

/// No file is looked for past the year 9999, whatever span is asked for.
const MAX_MS: u64 = 253_402_300_799_000;

/// A session's title is the start of its first instruction, this long at
/// most.
const MAX_TITLE_CHARS: usize = 80;

/// The provider whose calls are paid for by a plan, not per token.
const SUBSCRIPTION_PROVIDER: &str = "codex";

/// Stand-ins for a model, used by tests and the app's demonstration. They
/// use no tokens, so their calls are not recorded.
pub(crate) const STAND_INS: [&str; 2] = ["scripted", "demo"];

/// What a model call was for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// A step of the loop.
    #[default]
    Step,

    /// The review of finished work against the request.
    Review,

    /// The lessons taken from a run.
    Lessons,

    /// The summary written when a conversation was compacted.
    Compaction,

    /// One of several attempts at a task, all of its calls together.
    Attempt,

    /// Calls a run reported in its total that no other line accounts for.
    #[serde(other)]
    Other,
}

/// One line of the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Milliseconds since 1970, UTC.
    pub ts_ms: u64,

    /// The session the call belongs to: the stem of its log file, or
    /// whatever the caller named it.
    pub session: String,

    /// The project or place the session worked in.
    pub place: String,

    pub provider: String,
    pub model: String,

    #[serde(default)]
    pub kind: CallKind,

    #[serde(flatten)]
    pub usage: Usage,

    #[serde(default)]
    pub duration_ms: u64,

    /// Model calls this line stands for. More than one only for an
    /// attempt, which is recorded whole.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub calls: u32,

    /// The start of the session's first instruction, on the first line
    /// written for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

fn one() -> u32 {
    1
}

fn is_one(calls: &u32) -> bool {
    *calls == 1
}

/// A folder of files of JSON lines, one file per month.
#[derive(Debug, Clone)]
pub(crate) struct Months {
    folder: PathBuf,
}

impl Months {
    pub(crate) fn new(folder: PathBuf) -> Self {
        Self { folder }
    }

    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    pub(crate) fn file(&self, year: i32, month: u32) -> PathBuf {
        self.folder.join(format!("{year:04}-{month:02}.jsonl"))
    }

    /// Adds `json` as one line to the file of the month `ts_ms` falls in.
    pub(crate) fn append(&self, ts_ms: u64, json: &str) -> io::Result<()> {
        fs::create_dir_all(&self.folder)?;

        let (year, month, _) = civil(ts_ms as i64 / DAY_MS);

        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(self.file(year, month))?;

        let mut line = String::new();

        // A crash can leave the last line without its end. Starting on a
        // new line keeps this record from being joined to it.
        if ends_mid_line(&mut file)? {
            line.push('\n');
        }

        line.push_str(json);
        line.push('\n');

        // One write, so two sessions adding to the same file cannot
        // interleave their lines.
        file.write_all(line.as_bytes())?;
        file.flush()
    }

    /// The lines of one month's file that can be read as a `T`.
    pub(crate) fn read<T: serde::de::DeserializeOwned>(&self, year: i32, month: u32) -> Vec<T> {
        let Ok(text) = fs::read(self.file(year, month)) else {
            return Vec::new();
        };

        String::from_utf8_lossy(&text)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The lines of every month from the one `from_ms` falls in to the
    /// one just before `to_ms` falls in.
    pub(crate) fn spanning<T: serde::de::DeserializeOwned>(
        &self,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<T> {
        if from_ms >= to_ms {
            return Vec::new();
        }

        let month_of = |ms: u64| {
            let (year, month, _) = civil(ms.min(MAX_MS) as i64 / DAY_MS);

            (year, month)
        };

        let (first, last) = (month_of(from_ms), month_of(to_ms - 1));

        // The files that are there, not every month of the span: a span
        // may be "everything", and that is centuries of months.
        let mut months: Vec<(i32, u32)> = fs::read_dir(&self.folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| month_named(&entry.file_name().to_string_lossy()))
            .filter(|month| (first..=last).contains(month))
            .collect();

        months.sort_unstable();

        let mut lines = Vec::new();

        for (year, month) in months {
            lines.extend(self.read(year, month));
        }

        lines
    }
}

/// The folder of monthly files.
#[derive(Debug, Clone)]
pub struct Ledger {
    months: Months,
}

impl Ledger {
    /// The ledger kept in `folder`. Nothing is read or created until a
    /// record is written or a question asked.
    pub fn open(folder: impl Into<PathBuf>) -> Self {
        Self {
            months: Months::new(folder.into()),
        }
    }

    /// The ledger every session of this person writes to, in
    /// `~/.cortex/usage`. `None` on a machine without a home directory.
    pub fn at_home() -> Option<Self> {
        Some(Self::open(
            std::env::home_dir()?.join(".cortex").join("usage"),
        ))
    }

    pub fn folder(&self) -> &Path {
        self.months.folder()
    }

    /// Adds one line to the file of the month `record` falls in.
    pub fn append(&self, record: &Record) -> io::Result<()> {
        self.months
            .append(record.ts_ms, &serde_json::to_string(record)?)
    }

    /// Every record from `from_ms` up to, not including, `to_ms`, oldest
    /// first. Lines that cannot be read are left out.
    pub fn records(&self, from_ms: u64, to_ms: u64) -> Vec<Record> {
        let mut records: Vec<Record> = self.months.spanning(from_ms, to_ms);

        records.retain(|record| (from_ms..to_ms).contains(&record.ts_ms));
        records.sort_by_key(|record| record.ts_ms);

        records
    }

    /// What was used from `from_ms` up to, not including, `to_ms`.
    pub fn range(
        &self,
        from_ms: u64,
        to_ms: u64,
        utc_offset_minutes: i32,
        prices: &PriceTable,
    ) -> Summary {
        summarise(
            &self.records(from_ms, to_ms),
            from_ms,
            to_ms,
            utc_offset_minutes,
            prices,
        )
    }

    /// One calendar month as the person's own clock has it: every day of
    /// it, used or not, and the summary of the whole.
    pub fn month(
        &self,
        year: i32,
        month: u32,
        utc_offset_minutes: i32,
        prices: &PriceTable,
    ) -> MonthReport {
        let month = month.clamp(1, 12);
        let (following_year, following_month) = next_month(year, month);

        let first_day = days_from_civil(year, month, 1);
        let end_day = days_from_civil(following_year, following_month, 1);

        let from_ms = local_midnight(first_day, utc_offset_minutes);
        let to_ms = local_midnight(end_day, utc_offset_minutes);

        let summary = self.range(from_ms, to_ms, utc_offset_minutes, prices);

        let used: HashMap<&str, &DayUse> = summary
            .days
            .iter()
            .map(|day| (day.date.as_str(), day))
            .collect();

        let days = (first_day..end_day)
            .map(|day| {
                let date = date_text(day);

                match used.get(date.as_str()) {
                    Some(day) => (*day).clone(),

                    None => DayUse {
                        date,
                        ..DayUse::default()
                    },
                }
            })
            .collect();

        MonthReport {
            year,
            month,
            first_weekday: weekday(first_day),
            days,
            summary,
        }
    }

    /// How many days in a row, up to today, something was used. A day
    /// that has only just begun does not break the run: with nothing used
    /// yet today, the run that ended yesterday still counts.
    pub fn streak_days(&self, now_ms: u64, utc_offset_minutes: i32) -> u32 {
        let today = local_day(now_ms, utc_offset_minutes);

        let mut used: BTreeSet<i64> = BTreeSet::new();
        let mut read: BTreeSet<(i32, u32)> = BTreeSet::new();

        let mut day = today;
        let mut streak = 0;

        loop {
            // A local day can lie partly in the UTC day before or after
            // it, and so in a neighbouring month's file.
            for near in [day - 1, day, day + 1] {
                let (year, month, _) = civil(near);

                if read.insert((year, month)) {
                    used.extend(self.days_used(year, month, utc_offset_minutes));
                }
            }

            if used.contains(&day) {
                streak += 1;
            } else if day != today {
                return streak;
            }

            day -= 1;
        }
    }

    fn days_used(&self, year: i32, month: u32, utc_offset_minutes: i32) -> Vec<i64> {
        self.months
            .read::<Record>(year, month)
            .iter()
            .map(|record| local_day(record.ts_ms, utc_offset_minutes))
            .collect()
    }
}

/// The year and month a file named `YYYY-MM.jsonl` holds.
fn month_named(file: &str) -> Option<(i32, u32)> {
    let (year, month) = file.strip_suffix(".jsonl")?.split_once('-')?;

    Some((year.parse().ok()?, month.parse().ok()?))
}

/// Whether `file` has content that does not end in a line break.
fn ends_mid_line(file: &mut fs::File) -> io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }

    let mut last = [0u8; 1];

    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut last)?;

    Ok(last[0] != b'\n')
}

/// Tokens and calls added up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// Input that was not served from the provider's cache.
    pub input_tokens: u64,

    /// Input served from the provider's cache.
    pub cached_input_tokens: u64,

    /// Input written to the provider's cache.
    pub cache_write_tokens: u64,

    pub output_tokens: u64,

    /// Model calls.
    pub calls: u64,

    /// Time the calls took, added up.
    pub duration_ms: u64,
}

impl Totals {
    fn add(&mut self, record: &Record) {
        self.input_tokens += record.usage.input_tokens;
        self.cached_input_tokens += record.usage.cache_read_tokens;
        self.cache_write_tokens += record.usage.cache_write_tokens;
        self.output_tokens += record.usage.output_tokens;
        self.calls += u64::from(record.calls.max(1));
        self.duration_ms += record.duration_ms;
    }

    /// Everything sent to the model, cached or not.
    pub fn all_input(&self) -> u64 {
        self.input_tokens + self.cached_input_tokens + self.cache_write_tokens
    }

    /// Every token, in and out.
    pub fn tokens(&self) -> u64 {
        self.all_input() + self.output_tokens
    }

    /// Share of the input the provider served from its cache, from 0 to
    /// 1. `None` when nothing was sent.
    pub fn cache_hit_share(&self) -> Option<f64> {
        let input = self.all_input();

        (input > 0).then(|| self.cached_input_tokens as f64 / input as f64)
    }
}

/// What the tokens cost, as far as that is known.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    /// US dollars for the calls whose model has a known price, at list
    /// price. `None` when no call had one. An estimate: check the
    /// provider's invoice.
    pub usd: Option<f64>,

    /// Tokens of calls with a known price, which `usd` covers.
    pub priced_tokens: u64,

    /// Tokens of calls paid for by a subscription, which have no price
    /// per token.
    pub subscription_tokens: u64,

    /// Tokens of calls whose model has no known price. They are in no
    /// figure here.
    pub unpriced_tokens: u64,
}

impl CostEstimate {
    fn add(&mut self, record: &Record, prices: &PriceTable) {
        let tokens = record.usage.total();

        match prices.cost(&record.provider, &record.model, &record.usage) {
            CallCost::Usd(usd) => {
                self.usd = Some(self.usd.unwrap_or_default() + usd);
                self.priced_tokens += tokens;
            }

            CallCost::Subscription => self.subscription_tokens += tokens,
            CallCost::Unknown => self.unpriced_tokens += tokens,
        }
    }

    /// Whether `usd` accounts for every token that has a price per token
    /// at all.
    pub fn is_complete(&self) -> bool {
        self.unpriced_tokens == 0
    }
}

/// One day of the person's calendar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DayUse {
    /// `YYYY-MM-DD`, local.
    pub date: String,

    pub totals: Totals,

    /// Sessions that made a call on this day.
    pub sessions: u64,

    pub cost: CostEstimate,
}

/// One session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUse {
    pub id: String,

    /// The start of its first instruction, when that was recorded.
    pub title: Option<String>,

    pub place: String,
    pub provider: String,

    /// The model most of its tokens went to.
    pub model: String,

    /// Its first and last call in the span asked about, in milliseconds
    /// since 1970.
    pub first_ms: u64,
    pub last_ms: u64,

    pub totals: Totals,
    pub cost: CostEstimate,
}

/// One model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelUse {
    pub provider: String,
    pub model: String,
    pub totals: Totals,

    /// Its share of all tokens in the span, from 0 to 1.
    pub share: f64,

    pub cost: CostEstimate,
}

/// One hour of the day, over every day in the span.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HourUse {
    /// 0 to 23, local.
    pub hour: u8,

    pub tokens: u64,
    pub calls: u64,
}

/// The day on which the most tokens were used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeakDay {
    pub date: String,
    pub tokens: u64,
}

/// Tokens of an average call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PerCall {
    /// Input, cached or not.
    pub input_tokens: f64,

    pub output_tokens: f64,
}

/// Everything known about a span of time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub from_ms: u64,
    pub to_ms: u64,
    pub utc_offset_minutes: i32,

    pub totals: Totals,

    /// How many sessions made a call.
    pub session_count: u64,

    /// The days something was used on, oldest first.
    pub days: Vec<DayUse>,

    /// Newest first.
    pub sessions: Vec<SessionUse>,

    /// Most tokens first.
    pub models: Vec<ModelUse>,

    /// All 24 hours of the day, midnight first.
    pub hours: Vec<HourUse>,

    pub peak_day: Option<PeakDay>,

    /// The hour of the day in which the most tokens were used.
    pub peak_hour: Option<HourUse>,

    /// The model most tokens went to.
    pub favourite_model: Option<String>,

    pub per_call: Option<PerCall>,

    /// Share of the input the provider served from its cache.
    pub cache_hit_share: Option<f64>,

    pub cost: CostEstimate,
}

/// One calendar month.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MonthReport {
    pub year: i32,
    pub month: u32,

    /// The weekday of the first, with Monday as 0.
    pub first_weekday: u8,

    /// Every day of the month in order, whether used or not.
    pub days: Vec<DayUse>,

    pub summary: Summary,
}

fn summarise(
    records: &[Record],
    from_ms: u64,
    to_ms: u64,
    utc_offset_minutes: i32,
    prices: &PriceTable,
) -> Summary {
    let mut totals = Totals::default();
    let mut cost = CostEstimate::default();

    let mut days: BTreeMap<i64, (DayUse, BTreeSet<&str>)> = BTreeMap::new();
    let mut sessions: HashMap<&str, (SessionUse, HashMap<&str, u64>)> = HashMap::new();
    let mut models: BTreeMap<(&str, &str), ModelUse> = BTreeMap::new();

    let mut hours: Vec<HourUse> = (0..24)
        .map(|hour| HourUse {
            hour,
            ..HourUse::default()
        })
        .collect();

    for record in records {
        let tokens = record.usage.total();
        let calls = u64::from(record.calls.max(1));

        totals.add(record);
        cost.add(record, prices);

        let day = local_day(record.ts_ms, utc_offset_minutes);
        let (of_day, seen) = days.entry(day).or_default();

        of_day.totals.add(record);
        of_day.cost.add(record, prices);
        seen.insert(&record.session);

        let hour = &mut hours[local_hour(record.ts_ms, utc_offset_minutes)];

        hour.tokens += tokens;
        hour.calls += calls;

        let (session, by_model) = sessions.entry(&record.session).or_insert_with(|| {
            let session = SessionUse {
                id: record.session.clone(),
                place: record.place.clone(),
                provider: record.provider.clone(),
                first_ms: record.ts_ms,
                ..SessionUse::default()
            };

            (session, HashMap::new())
        });

        session.totals.add(record);
        session.cost.add(record, prices);
        session.first_ms = session.first_ms.min(record.ts_ms);
        session.last_ms = session.last_ms.max(record.ts_ms);

        if session.title.is_none() {
            session.title.clone_from(&record.title);
        }

        *by_model.entry(&record.model).or_default() += tokens;

        let model = models
            .entry((&record.provider, &record.model))
            .or_insert_with(|| ModelUse {
                provider: record.provider.clone(),
                model: record.model.clone(),
                ..ModelUse::default()
            });

        model.totals.add(record);
        model.cost.add(record, prices);
    }

    let days: Vec<DayUse> = days
        .into_iter()
        .map(|(day, (mut of_day, seen))| {
            of_day.date = date_text(day);
            of_day.sessions = seen.len() as u64;

            of_day
        })
        .collect();

    let mut sessions: Vec<SessionUse> = sessions
        .into_values()
        .map(|(mut session, by_model)| {
            // The most used; the name settles a tie so the answer does
            // not depend on the order of a hash map.
            let most = by_model
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)));

            if let Some((model, _)) = most {
                session.model = model.to_string();
            }

            session
        })
        .collect();

    sessions.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.id.cmp(&b.id)));

    let all_tokens = totals.tokens();

    let mut models: Vec<ModelUse> = models
        .into_values()
        .map(|mut model| {
            if all_tokens > 0 {
                model.share = model.totals.tokens() as f64 / all_tokens as f64;
            }

            model
        })
        .collect();

    // Stable, so models with the same use stay in name order.
    models.sort_by_key(|model| std::cmp::Reverse(model.totals.tokens()));

    let peak_day = days
        .iter()
        .filter(|day| day.totals.tokens() > 0)
        .max_by(|a, b| {
            let tokens = a.totals.tokens().cmp(&b.totals.tokens());

            // Of two equal days the earlier one is the peak.
            tokens.then_with(|| b.date.cmp(&a.date))
        })
        .map(|day| PeakDay {
            date: day.date.clone(),
            tokens: day.totals.tokens(),
        });

    let peak_hour = hours
        .iter()
        .filter(|hour| hour.tokens > 0)
        .max_by(|a, b| a.tokens.cmp(&b.tokens).then_with(|| b.hour.cmp(&a.hour)))
        .copied();

    let per_call = (totals.calls > 0).then(|| PerCall {
        input_tokens: totals.all_input() as f64 / totals.calls as f64,
        output_tokens: totals.output_tokens as f64 / totals.calls as f64,
    });

    Summary {
        from_ms,
        to_ms,
        utc_offset_minutes,
        session_count: sessions.len() as u64,
        favourite_model: models
            .first()
            .filter(|model| model.totals.tokens() > 0)
            .map(|model| model.model.clone()),
        cache_hit_share: totals.cache_hit_share(),
        totals,
        days,
        sessions,
        models,
        hours,
        peak_day,
        peak_hour,
        per_call,
        cost,
    }
}

/// US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    /// Input that was not served from the cache.
    pub input: f64,

    /// Input served from the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<f64>,

    /// Input written to the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,

    pub output: f64,
}

/// What one call cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CallCost {
    /// An estimate at list price.
    Usd(f64),

    /// Paid for by a plan; there is no price per token.
    Subscription,

    /// The model's price is not known.
    Unknown,
}

/// Prices by model-name prefix.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceTable {
    prices: BTreeMap<String, Price>,
}

impl PriceTable {
    /// No prices at all: every cost is unknown.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The prices that are built in. Deliberately few: only models whose
    /// list price Anthropic's own reference for its API gave on
    /// 2026-10-06, with cache writes at the five-minute rate, which is
    /// the one Cortex uses. `claude-haiku-5-5` is left out because its
    /// price depends on the size of the prompt. Everything else, and any
    /// correction, belongs in the prices file.
    pub fn built_in() -> Self {
        let price = |input, cached_input, cache_write, output| Price {
            input,
            cached_input: Some(cached_input),
            cache_write: Some(cache_write),
            output,
        };

        let prices = [
            ("claude-opus-5-5", price(4.0, 0.20, 5.0, 20.0)),
            ("claude-sonnet-5-5", price(2.0, 0.20, 2.5, 10.0)),
            ("claude-fable-5-1", price(10.0, 0.25, 12.5, 50.0)),
        ];

        Self {
            prices: prices
                .into_iter()
                .map(|(model, price)| (model.to_string(), price))
                .collect(),
        }
    }

    /// The built-in prices with those in the file at `path` on top. A
    /// file that is missing or cannot be read changes nothing.
    pub fn load(path: &Path) -> Self {
        let mut table = Self::built_in();

        let read = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<BTreeMap<String, Price>>(&text).ok());

        if let Some(prices) = read {
            table.prices.extend(prices);
        }

        table
    }

    /// The built-in prices with the person's own, from
    /// `~/.cortex/prices.json`, on top.
    pub fn at_home() -> Self {
        match std::env::home_dir() {
            Some(home) => Self::load(&home.join(".cortex").join("prices.json")),
            None => Self::built_in(),
        }
    }

    pub fn set(&mut self, model_prefix: &str, price: Price) {
        self.prices.insert(model_prefix.to_string(), price);
    }

    /// The price of `model`: that of the longest prefix of its name that
    /// has one.
    pub fn price_for(&self, model: &str) -> Option<Price> {
        self.prices
            .iter()
            .filter(|(prefix, _)| model.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, price)| *price)
    }

    /// What a call cost. Unknown when the model has no price, or when it
    /// used cache tokens whose price is not given.
    pub fn cost(&self, provider: &str, model: &str, usage: &Usage) -> CallCost {
        if provider == SUBSCRIPTION_PROVIDER {
            return CallCost::Subscription;
        }

        let Some(price) = self.price_for(model) else {
            return CallCost::Unknown;
        };

        let part = |tokens: u64, per_million: Option<f64>| match (tokens, per_million) {
            (0, _) => Some(0.0),
            (tokens, Some(per_million)) => Some(tokens as f64 * per_million / 1e6),
            (_, None) => None,
        };

        let usd = (|| {
            Some(
                part(usage.input_tokens, Some(price.input))?
                    + part(usage.cache_read_tokens, price.cached_input)?
                    + part(usage.cache_write_tokens, price.cache_write)?
                    + part(usage.output_tokens, Some(price.output))?,
            )
        })();

        usd.map_or(CallCost::Unknown, CallCost::Usd)
    }
}

/// Writes every model call of the runs it hears about to a [`Ledger`].
///
/// Each call is written once. The steps of a run, its review, its lessons
/// and a compaction are written as they happen. Of several attempts at a
/// task each is written whole when it ends, and the steps of the one that
/// is kept, which the run then reports again, are not written a second
/// time. Whatever a run's own total holds beyond all that is written as
/// one last line when the run ends.
pub struct LedgerSink {
    ledger: Ledger,
    place: String,
    stand_ins: bool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    session: String,
    provider: String,
    model: String,

    /// The session's title, until its first line has carried it.
    title: Option<String>,

    /// The session has had a line with its title, or was taken up from an
    /// earlier day and already has one.
    titled: bool,

    /// Tokens of the run in progress that have been written.
    counted: Usage,

    /// The steps now arriving belong to an attempt that has already been
    /// written whole.
    repeating: bool,
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

impl LedgerSink {
    /// A sink for sessions that work in `place`. Until
    /// [`Self::set_session`] names the session, its calls are filed
    /// under a name made up here.
    pub fn new(ledger: Ledger, place: &str) -> Self {
        let session = format!(
            "{}-{}-{}",
            unix_millis(),
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );

        Self {
            ledger,
            place: place.to_string(),
            stand_ins: false,
            state: Mutex::new(State {
                session,
                ..State::default()
            }),
        }
    }

    /// Also records the calls of the stand-in models that tests and the
    /// demonstration use.
    pub fn recording_stand_ins(mut self) -> Self {
        self.stand_ins = true;

        self
    }

    /// Files what follows under `session`: the stem of the session's log
    /// file, so the ledger and the log name it alike. Call it when a
    /// session is started or an earlier one is taken up.
    pub fn set_session(&self, session: &str) {
        let mut state = self.lock();

        if state.session != session {
            state.session = session.to_string();
            state.title = None;
            state.titled = false;
        }
    }

    /// The name the calls are being filed under.
    pub fn session(&self) -> String {
        self.lock().session.clone()
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self, state: &mut State, kind: CallKind, usage: Usage, duration_ms: u64, calls: u32) {
        if !self.stand_ins && STAND_INS.contains(&state.provider.as_str()) {
            return;
        }

        let record = Record {
            ts_ms: unix_millis() as u64,
            session: state.session.clone(),
            place: self.place.clone(),
            provider: state.provider.clone(),
            model: state.model.clone(),
            kind,
            usage,
            duration_ms,
            calls: calls.max(1),
            title: state.title.take(),
        };

        if let Err(error) = self.ledger.append(&record) {
            eprintln!(
                "warning: could not write to the usage ledger in {}: {error}",
                self.ledger.folder().display()
            );
        }
    }
}

impl TrajectorySink for LedgerSink {
    fn record(&self, event: &Event) {
        let mut state = self.lock();

        match event {
            Event::RunStarted {
                instruction,
                provider,
                model,
            } => {
                state.provider.clone_from(provider);
                state.model.clone_from(model);
                state.counted = Usage::default();
                state.repeating = false;

                if !state.titled {
                    state.title = title_of(instruction);
                    state.titled = true;
                }
            }

            Event::ModelResponded {
                usage, duration_ms, ..
            } if !state.repeating => {
                state.counted += *usage;

                self.write(&mut state, CallKind::Step, *usage, *duration_ms, 1);
            }

            Event::ReviewRan {
                usage, duration_ms, ..
            } if !state.repeating => {
                state.counted += *usage;

                self.write(&mut state, CallKind::Review, *usage, *duration_ms, 1);
            }

            Event::LessonsLearned {
                usage, duration_ms, ..
            } => {
                state.counted += *usage;

                self.write(&mut state, CallKind::Lessons, *usage, *duration_ms, 1);
            }

            Event::ContextCompacted {
                step,
                usage,
                duration_ms,
                ..
            } if !state.repeating => {
                // One made between runs is in no run's total.
                if *step > 0 {
                    state.counted += *usage;
                }

                self.write(&mut state, CallKind::Compaction, *usage, *duration_ms, 1);
            }

            Event::AttemptFinished {
                steps,
                usage,
                duration_ms,
                ..
            } => {
                state.counted += *usage;

                if usage.total() > 0 {
                    self.write(&mut state, CallKind::Attempt, *usage, *duration_ms, *steps);
                }
            }

            // The steps of the attempt that was kept follow, as if it had
            // been the only one. They were written with the attempt.
            Event::AttemptChosen { .. } => state.repeating = true,

            Event::RunFinished { usage, .. } => {
                let rest = beyond(*usage, state.counted);

                if rest.total() > 0 {
                    self.write(&mut state, CallKind::Other, rest, 0, 1);
                }

                state.counted = *usage;
                state.repeating = false;
            }

            _ => {}
        }
    }
}

/// What `total` holds that `counted` does not.
pub(crate) fn beyond(total: Usage, counted: Usage) -> Usage {
    Usage {
        input_tokens: total.input_tokens.saturating_sub(counted.input_tokens),
        output_tokens: total.output_tokens.saturating_sub(counted.output_tokens),
        cache_read_tokens: total
            .cache_read_tokens
            .saturating_sub(counted.cache_read_tokens),
        cache_write_tokens: total
            .cache_write_tokens
            .saturating_sub(counted.cache_write_tokens),
    }
}

fn title_of(instruction: &str) -> Option<String> {
    let line = instruction.lines().find(|line| !line.trim().is_empty())?;
    let line = line.trim();

    if line.chars().count() <= MAX_TITLE_CHARS {
        return Some(line.to_string());
    }

    let start: String = line.chars().take(MAX_TITLE_CHARS).collect();

    Some(format!("{}...", start.trim_end()))
}

/// The local day `ts_ms` falls on, in days since 1970-01-01.
pub(crate) fn local_day(ts_ms: u64, utc_offset_minutes: i32) -> i64 {
    local_ms(ts_ms, utc_offset_minutes).div_euclid(DAY_MS)
}

/// The local hour of the day, 0 to 23.
fn local_hour(ts_ms: u64, utc_offset_minutes: i32) -> usize {
    (local_ms(ts_ms, utc_offset_minutes).rem_euclid(DAY_MS) / HOUR_MS) as usize
}

fn local_ms(ts_ms: u64, utc_offset_minutes: i32) -> i64 {
    ts_ms as i64 + i64::from(utc_offset_minutes) * MINUTE_MS
}

/// The moment local day `day` begins, in milliseconds since 1970, UTC.
fn local_midnight(day: i64, utc_offset_minutes: i32) -> u64 {
    (day * DAY_MS - i64::from(utc_offset_minutes) * MINUTE_MS).max(0) as u64
}

pub(crate) fn date_text(day: i64) -> String {
    let (year, month, day) = civil(day);

    format!("{year:04}-{month:02}-{day:02}")
}

/// The weekday of `day`, with Monday as 0. 1970-01-01 was a Thursday.
pub(crate) fn weekday(day: i64) -> u8 {
    (day + 3).rem_euclid(7) as u8
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month >= 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// The year, month and day of a count of days since 1970-01-01, in the
/// calendar everyone uses. The arithmetic is Howard Hinnant's
/// `civil_from_days`.
pub fn civil(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);

    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;

    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;

    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;

    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };

    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (year as i32, month as u32, day as u32)
}

/// Days since 1970-01-01 of a date. The inverse of [`civil`].
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let month = i64::from(month);

    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);

    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;

    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// The year and month `now_ms` falls in on the person's own calendar.
pub fn local_month(now_ms: u64, utc_offset_minutes: i32) -> (i32, u32) {
    let (year, month, _) = civil(local_day(now_ms, utc_offset_minutes));

    (year, month)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::agent::Outcome;
    use crate::message::Message;
    use crate::provider::StopReason;

    /// Milliseconds since 1970 of a UTC date and time.
    fn at(year: i32, month: u32, day: u32, hour: i64, minute: i64) -> u64 {
        (days_from_civil(year, month, day) * DAY_MS + hour * HOUR_MS + minute * MINUTE_MS) as u64
    }

    fn usage(input: u64, cached: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
        }
    }

    fn record(ts_ms: u64, session: &str, model: &str, usage: Usage) -> Record {
        Record {
            ts_ms,
            session: session.to_string(),
            place: "/work/project".to_string(),
            provider: "anthropic".to_string(),
            model: model.to_string(),
            kind: CallKind::Step,
            usage,
            duration_ms: 1_000,
            calls: 1,
            title: None,
        }
    }

    fn ledger() -> (TempDir, Ledger) {
        let folder = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(folder.path().join("usage"));

        (folder, ledger)
    }

    #[test]
    fn dates_and_day_counts_agree() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(civil(days_from_civil(2026, 12, 31) + 1), (2027, 1, 1));

        // 2026-10-14 was a Wednesday.
        assert_eq!(weekday(days_from_civil(2026, 10, 14)), 2);

        for day in (-800..40_000).step_by(37) {
            let (year, month, of_month) = civil(day);

            assert_eq!(days_from_civil(year, month, of_month), day);
        }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let (_folder, ledger) = ledger();

        let mut first = record(
            at(2026, 10, 3, 9, 0),
            "one",
            "claude-opus-5-5",
            usage(100, 900, 50),
        );

        first.title = Some("Fix the rounding".to_string());
        first.kind = CallKind::Review;

        let second = record(
            at(2026, 10, 3, 9, 5),
            "one",
            "claude-opus-5-5",
            usage(10, 0, 5),
        );

        ledger.append(&first).unwrap();
        ledger.append(&second).unwrap();

        let read = ledger.records(0, u64::MAX);

        assert_eq!(read, [first, second]);
    }

    #[test]
    fn each_month_has_its_own_file_and_a_span_reads_across_them() {
        let (_folder, ledger) = ledger();

        let before = record(at(2026, 12, 31, 23, 59), "old", "m", usage(1, 0, 1));
        let after = record(at(2027, 1, 1, 0, 0), "new", "m", usage(2, 0, 2));

        ledger.append(&before).unwrap();
        ledger.append(&after).unwrap();

        assert!(ledger.folder().join("2026-12.jsonl").is_file());
        assert!(ledger.folder().join("2027-01.jsonl").is_file());

        let both = ledger.records(at(2026, 12, 1, 0, 0), at(2027, 2, 1, 0, 0));

        assert_eq!(both, [before.clone(), after]);

        // The end of a span is not part of it.
        let december = ledger.records(at(2026, 12, 1, 0, 0), at(2027, 1, 1, 0, 0));

        assert_eq!(december, [before]);
    }

    #[test]
    fn a_torn_last_line_is_skipped_and_does_not_spoil_the_next() {
        let (_folder, ledger) = ledger();

        let whole = record(at(2026, 10, 3, 9, 0), "one", "m", usage(1, 0, 1));

        ledger.append(&whole).unwrap();

        // A crash half-way through a line.
        let file = ledger.folder().join("2026-10.jsonl");

        let mut text = fs::read_to_string(&file).unwrap();

        text.push_str("{\"ts_ms\":17595,\"session\":\"tw");

        fs::write(&file, text).unwrap();

        assert_eq!(ledger.records(0, u64::MAX), std::slice::from_ref(&whole));

        let later = record(at(2026, 10, 3, 9, 30), "two", "m", usage(2, 0, 2));

        ledger.append(&later).unwrap();

        assert_eq!(ledger.records(0, u64::MAX), [whole, later]);
    }

    #[test]
    fn a_day_is_the_persons_own_day() {
        let (_folder, ledger) = ledger();

        // 20:00 UTC on the 3rd is 01:30 on the 4th in India, and 13:00 on
        // the 3rd in California.
        ledger
            .append(&record(at(2026, 10, 3, 20, 0), "one", "m", usage(10, 0, 0)))
            .unwrap();

        let india = ledger.month(2026, 10, 330, &PriceTable::empty());

        assert_eq!(india.days.len(), 31);
        assert_eq!(india.days[2].totals.tokens(), 0);
        assert_eq!(india.days[3].date, "2026-10-04");
        assert_eq!(india.days[3].totals.tokens(), 10);
        assert_eq!(india.summary.peak_hour.unwrap().hour, 1);

        let california = ledger.month(2026, 10, -420, &PriceTable::empty());

        assert_eq!(california.days[2].totals.tokens(), 10);
        assert_eq!(california.summary.peak_hour.unwrap().hour, 13);

        // October 2026 began on a Thursday.
        assert_eq!(india.first_weekday, 3);
    }

    #[test]
    fn a_month_takes_in_the_hours_that_utc_files_under_its_neighbours() {
        let (_folder, ledger) = ledger();

        // 19:00 UTC on 30 September is already 1 October in India.
        ledger
            .append(&record(at(2026, 9, 30, 19, 0), "one", "m", usage(7, 0, 0)))
            .unwrap();

        // 19:00 UTC on 31 October is 1 November there.
        ledger
            .append(&record(at(2026, 10, 31, 19, 0), "two", "m", usage(9, 0, 0)))
            .unwrap();

        let october = ledger.month(2026, 10, 330, &PriceTable::empty());

        assert_eq!(october.summary.totals.tokens(), 7);
        assert_eq!(october.days[0].totals.tokens(), 7);

        let november = ledger.month(2026, 11, 330, &PriceTable::empty());

        assert_eq!(november.days[0].totals.tokens(), 9);
    }

    #[test]
    fn the_peaks_the_favourite_and_the_averages() {
        let (_folder, ledger) = ledger();

        let lines = [
            record(at(2026, 10, 3, 9, 0), "one", "small", usage(100, 0, 100)),
            record(at(2026, 10, 3, 9, 30), "one", "big", usage(100, 300, 100)),
            record(at(2026, 10, 5, 14, 0), "two", "big", usage(500, 500, 200)),
            record(at(2026, 10, 5, 14, 10), "three", "big", usage(100, 0, 100)),
        ];

        for line in &lines {
            ledger.append(line).unwrap();
        }

        let summary = ledger.range(0, u64::MAX, 0, &PriceTable::empty());

        assert_eq!(summary.totals.calls, 4);
        assert_eq!(summary.totals.tokens(), 2_100);
        assert_eq!(summary.session_count, 3);

        assert_eq!(summary.days.len(), 2);
        assert_eq!(summary.days[1].sessions, 2);

        let peak = summary.peak_day.unwrap();

        assert_eq!((peak.date.as_str(), peak.tokens), ("2026-10-05", 1_400));
        assert_eq!(summary.peak_hour.unwrap().hour, 14);
        assert_eq!(summary.hours.len(), 24);

        assert_eq!(summary.favourite_model.as_deref(), Some("big"));
        assert_eq!(summary.models[0].model, "big");
        assert!((summary.models[0].share - 1_900.0 / 2_100.0).abs() < 1e-9);

        // Newest session first, and each with the model it used most.
        assert_eq!(summary.sessions[0].id, "three");
        assert_eq!(summary.sessions[2].id, "one");
        assert_eq!(summary.sessions[2].model, "big");
        assert_eq!(summary.sessions[2].first_ms, at(2026, 10, 3, 9, 0));
        assert_eq!(summary.sessions[2].last_ms, at(2026, 10, 3, 9, 30));

        let per_call = summary.per_call.unwrap();

        assert_eq!(per_call.input_tokens, 400.0);
        assert_eq!(per_call.output_tokens, 125.0);

        assert_eq!(summary.cache_hit_share, Some(800.0 / 1_600.0));
    }

    #[test]
    fn nothing_used_gives_no_peaks_and_no_averages() {
        let (_folder, ledger) = ledger();

        let summary = ledger.range(0, u64::MAX, 0, &PriceTable::empty());

        assert_eq!(summary.peak_day, None);
        assert_eq!(summary.peak_hour, None);
        assert_eq!(summary.favourite_model, None);
        assert_eq!(summary.per_call, None);
        assert_eq!(summary.cache_hit_share, None);
        assert_eq!(summary.cost.usd, None);
    }

    #[test]
    fn a_streak_is_days_in_a_row_up_to_today() {
        let (_folder, ledger) = ledger();

        // Used on 29 and 30 September and 1 and 2 October; not on the 28th.
        for (month, day) in [(9, 27), (9, 29), (9, 30), (10, 1), (10, 2)] {
            ledger
                .append(&record(
                    at(2026, month, day, 12, 0),
                    "s",
                    "m",
                    usage(1, 0, 0),
                ))
                .unwrap();
        }

        assert_eq!(ledger.streak_days(at(2026, 10, 2, 18, 0), 0), 4);

        // Nothing yet today: yesterday's run still stands.
        assert_eq!(ledger.streak_days(at(2026, 10, 3, 8, 0), 0), 4);

        // A whole day missed ends it.
        assert_eq!(ledger.streak_days(at(2026, 10, 4, 8, 0), 0), 0);

        assert_eq!(ledger.streak_days(at(2026, 9, 27, 13, 0), 0), 1);
    }

    #[test]
    fn a_streak_counts_local_days() {
        let (_folder, ledger) = ledger();

        // 20:00 UTC on 30 September and on 1 October: the 1st and 2nd of
        // October in India.
        ledger
            .append(&record(at(2026, 9, 30, 20, 0), "s", "m", usage(1, 0, 0)))
            .unwrap();
        ledger
            .append(&record(at(2026, 10, 1, 20, 0), "s", "m", usage(1, 0, 0)))
            .unwrap();

        assert_eq!(ledger.streak_days(at(2026, 10, 1, 21, 0), 330), 2);
    }

    #[test]
    fn a_cost_is_given_only_where_the_price_is_known() {
        let mut prices = PriceTable::empty();

        prices.set(
            "priced",
            Price {
                input: 2.0,
                cached_input: Some(0.5),
                cache_write: None,
                output: 10.0,
            },
        );

        let used = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 2_000_000,
            cache_write_tokens: 0,
        };

        assert_eq!(
            prices.cost("hosted", "priced-large", &used),
            CallCost::Usd(13.0)
        );
        assert_eq!(prices.cost("hosted", "other", &used), CallCost::Unknown);

        // The price of a cache write is not given, so a call that made
        // one has no cost rather than a low one.
        let wrote = Usage {
            cache_write_tokens: 1,
            ..used
        };

        assert_eq!(prices.cost("hosted", "priced", &wrote), CallCost::Unknown);

        assert_eq!(
            prices.cost("codex", "priced", &used),
            CallCost::Subscription
        );
    }

    #[test]
    fn the_longest_matching_prefix_gives_the_price() {
        let mut prices = PriceTable::empty();

        let price = |input| Price {
            input,
            cached_input: None,
            cache_write: None,
            output: 0.0,
        };

        prices.set("claude-opus-5", price(5.0));
        prices.set("claude-opus-5-5", price(4.0));

        assert_eq!(prices.price_for("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(prices.price_for("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(prices.price_for("claude-opus"), None);
    }

    #[test]
    fn a_prices_file_adds_to_and_replaces_the_built_in_prices() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("prices.json");

        fs::write(
            &file,
            r#"{ "claude-opus-5-5": { "input": 1.0, "output": 2.0 },
                 "mine": { "input": 0.5, "cached_input": 0.1, "output": 1.5 } }"#,
        )
        .unwrap();

        let prices = PriceTable::load(&file);

        assert_eq!(prices.price_for("claude-opus-5-5").unwrap().input, 1.0);
        assert_eq!(prices.price_for("mine-7b").unwrap().cached_input, Some(0.1));
        assert_eq!(prices.price_for("claude-sonnet-5-5").unwrap().input, 2.0);

        // A file that is not there, or not JSON, leaves the built-in ones.
        let missing = PriceTable::load(&folder.path().join("none.json"));

        assert_eq!(missing, PriceTable::built_in());
    }

    #[test]
    fn a_summary_adds_up_cost_by_kind() {
        let (_folder, ledger) = ledger();

        let million = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };

        let mut subscribed = record(at(2026, 10, 3, 9, 0), "a", "gpt-6.1-sol", million);

        subscribed.provider = "codex".to_string();

        ledger.append(&subscribed).unwrap();
        ledger
            .append(&record(
                at(2026, 10, 3, 9, 1),
                "b",
                "claude-opus-5-5",
                million,
            ))
            .unwrap();
        ledger
            .append(&record(at(2026, 10, 3, 9, 2), "c", "unheard-of", million))
            .unwrap();

        let summary = ledger.range(0, u64::MAX, 0, &PriceTable::built_in());

        assert_eq!(summary.cost.usd, Some(4.0));
        assert_eq!(summary.cost.priced_tokens, 1_000_000);
        assert_eq!(summary.cost.subscription_tokens, 1_000_000);
        assert_eq!(summary.cost.unpriced_tokens, 1_000_000);
        assert!(!summary.cost.is_complete());

        let subscription = &summary.sessions[2];

        assert_eq!(subscription.id, "a");
        assert_eq!(subscription.cost.usd, None);
        assert!(subscription.cost.is_complete());
    }

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
        }
    }

    fn responded(step: u32, usage: Usage) -> Event {
        Event::ModelResponded {
            step,
            message: Message::assistant_text("ok"),
            stop_reason: StopReason::EndTurn,
            usage,
            duration_ms: 700,
        }
    }

    fn finished(steps: u32, usage: Usage) -> Event {
        Event::RunFinished {
            outcome: Outcome::Completed,
            steps,
            usage,
            duration_ms: 1,
            verified: None,
        }
    }

    fn sink() -> (TempDir, LedgerSink) {
        let (folder, ledger) = ledger();

        (folder, LedgerSink::new(ledger, "/work/project"))
    }

    fn all(sink: &LedgerSink) -> Vec<Record> {
        sink.ledger().records(0, u64::MAX)
    }

    fn total(records: &[Record]) -> Usage {
        let mut total = Usage::default();

        for record in records {
            total += record.usage;
        }

        total
    }

    #[test]
    fn a_run_is_written_call_by_call_under_its_session() {
        let (_folder, sink) = sink();

        sink.set_session("1760000000-42-0");

        sink.record(&started("Fix the rounding in money.ts\nand add a test"));
        sink.record(&responded(1, usage(100, 0, 20)));
        sink.record(&responded(2, usage(30, 90, 10)));
        sink.record(&finished(2, usage(130, 90, 30)));

        let records = all(&sink);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].session, "1760000000-42-0");
        assert_eq!(records[0].place, "/work/project");
        assert_eq!(records[0].model, "claude-opus-5-5");
        assert_eq!(records[0].duration_ms, 700);

        // The title rides on the session's first line only.
        assert_eq!(
            records[0].title.as_deref(),
            Some("Fix the rounding in money.ts")
        );
        assert_eq!(records[1].title, None);

        sink.record(&started("and now the docs"));
        sink.record(&responded(1, usage(5, 0, 5)));

        assert_eq!(all(&sink)[2].title, None);
    }

    #[test]
    fn calls_beside_the_steps_are_counted_once() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&responded(1, usage(100, 0, 20)));

        sink.record(&Event::ReviewRan {
            step: 1,
            requirements: Vec::new(),
            feedback: None,
            usage: usage(40, 0, 8),
            duration_ms: 300,
        });

        sink.record(&Event::LessonsLearned {
            lessons: Vec::new(),
            usage: usage(60, 0, 12),
            duration_ms: 400,
        });

        // The run's total holds the step, the review and the lessons.
        sink.record(&finished(1, usage(200, 0, 40)));

        let records = all(&sink);

        let kinds: Vec<CallKind> = records.iter().map(|record| record.kind).collect();

        assert_eq!(kinds, [CallKind::Step, CallKind::Review, CallKind::Lessons]);
        assert_eq!(total(&records), usage(200, 0, 40));
    }

    #[test]
    fn what_only_the_runs_total_knows_of_is_written_when_it_ends() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&responded(1, usage(100, 0, 20)));
        sink.record(&finished(1, usage(150, 0, 30)));

        let records = all(&sink);

        assert_eq!(records.len(), 2);
        assert_eq!(records[1].kind, CallKind::Other);
        assert_eq!(records[1].usage, usage(50, 0, 10));
        assert_eq!(total(&records), usage(150, 0, 30));
    }

    #[test]
    fn the_steps_of_the_attempt_that_is_kept_are_not_written_twice() {
        let (_folder, sink) = sink();

        let attempt = |attempt, usage| Event::AttemptFinished {
            attempt,
            strategy: "plain".to_string(),
            outcome: Some(Outcome::Completed),
            steps: 2,
            usage,
            verified: None,
            review_score: None,
            changed_files: 1,
            error: None,
            duration_ms: 900,
        };

        sink.record(&started("fix"));
        sink.record(&attempt(1, usage(300, 0, 60)));
        sink.record(&attempt(2, usage(200, 0, 40)));

        sink.record(&Event::AttemptChosen {
            attempt: 2,
            of: 2,
            reason: "best of 2".to_string(),
        });

        // The kept attempt's steps, reported again.
        sink.record(&responded(1, usage(100, 0, 20)));
        sink.record(&responded(2, usage(100, 0, 20)));

        sink.record(&Event::LessonsLearned {
            lessons: Vec::new(),
            usage: usage(50, 0, 10),
            duration_ms: 1,
        });

        sink.record(&finished(4, usage(550, 0, 110)));

        let records = all(&sink);

        let kinds: Vec<CallKind> = records.iter().map(|record| record.kind).collect();

        assert_eq!(
            kinds,
            [CallKind::Attempt, CallKind::Attempt, CallKind::Lessons]
        );
        assert_eq!(total(&records), usage(550, 0, 110));

        let summary = sink.ledger().range(0, u64::MAX, 0, &PriceTable::empty());

        // Two attempts of two steps each, and the lessons.
        assert_eq!(summary.totals.calls, 5);

        // The next run is written step by step again.
        sink.record(&started("more"));
        sink.record(&responded(1, usage(1, 0, 1)));

        assert_eq!(all(&sink).len(), 4);
    }

    #[test]
    fn a_run_carried_on_after_a_failure_is_not_counted_again() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&responded(1, usage(100, 0, 20)));

        // The provider failed; the run is carried on and its total, when
        // it ends, holds the step from before the failure too.
        sink.record(&Event::RunResumed { steps: 1 });
        sink.record(&responded(2, usage(100, 0, 20)));
        sink.record(&finished(2, usage(200, 0, 40)));

        assert_eq!(total(&all(&sink)), usage(200, 0, 40));
    }

    #[test]
    fn a_compaction_is_counted_in_a_run_and_between_runs() {
        let (_folder, sink) = sink();

        let compacted = |step| Event::ContextCompacted {
            step,
            kept: 0,
            text: "summary".to_string(),
            messages_replaced: 8,
            chars_before: 9_000,
            chars_after: 900,
            usage: usage(70, 0, 30),
            duration_ms: 500,
            automatic: step > 0,
        };

        sink.record(&started("fix"));
        sink.record(&responded(1, usage(100, 0, 20)));
        sink.record(&compacted(2));
        sink.record(&responded(2, usage(100, 0, 20)));
        sink.record(&finished(2, usage(270, 0, 70)));

        assert_eq!(total(&all(&sink)), usage(270, 0, 70));

        // One asked for while the session is idle belongs to no run.
        sink.record(&compacted(0));

        sink.record(&Event::RunResumed { steps: 2 });
        sink.record(&responded(3, usage(10, 0, 10)));
        sink.record(&finished(3, usage(280, 0, 80)));

        assert_eq!(total(&all(&sink)), usage(350, 0, 110));
    }

    #[test]
    fn a_stand_in_model_uses_nothing_worth_recording() {
        let (_folder, sink) = sink();

        let scripted = Event::RunStarted {
            instruction: "fix".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        };

        sink.record(&scripted);
        sink.record(&responded(1, usage(100, 0, 20)));

        assert!(all(&sink).is_empty());

        let (_folder, sink) = self::sink();
        let sink = sink.recording_stand_ins();

        sink.record(&scripted);
        sink.record(&responded(1, usage(100, 0, 20)));

        assert_eq!(all(&sink).len(), 1);
    }

    #[test]
    fn a_long_first_line_is_shortened_for_the_title() {
        let title = title_of(&format!("\n  {}", "word ".repeat(40))).unwrap();

        assert!(title.ends_with("..."));
        assert!(title.chars().count() <= MAX_TITLE_CHARS + 3);
        assert_eq!(title_of("  \n"), None);
    }
}
