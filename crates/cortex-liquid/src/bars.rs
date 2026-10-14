//! Price bars, and reading them from a CSV file as people actually have
//! them: any column order, several spellings of each name, several ways
//! of writing a time.
//!
//! What it cannot do: it does not know time zones by name, it reads a
//! time without an offset as UTC, and it does not understand dates
//! written day-first or month-first (`31/12/2024`), because the two cannot
//! be told apart. Such rows are skipped and counted.

use serde::{Deserialize, Serialize};

use crate::Error;

/// One bar: what the price did over one stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    /// When the bar begins, in milliseconds since 1970 UTC.
    pub time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,

    /// 0 where the file has none.
    pub volume: f64,
}

impl Bar {
    /// Why this bar cannot be used, if it cannot.
    pub fn problem(&self) -> Option<String> {
        let prices = [self.open, self.high, self.low, self.close];

        if prices.iter().any(|price| !price.is_finite()) {
            return Some("a price is not a number".to_string());
        }

        if prices.iter().any(|price| *price <= 0.0) {
            return Some("a price is zero or negative".to_string());
        }

        None
    }
}

/// What reading a file gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parsed {
    /// In time order, one per time.
    pub bars: Vec<Bar>,

    /// Rows that could not be read.
    pub skipped: usize,

    /// Rows dropped because a later row had the same time.
    pub duplicates: usize,

    /// Why the first skipped row was skipped, with its line number.
    pub first_problem: Option<String>,

    /// What had to be assumed, such as a missing volume column, in plain
    /// sentences a person can be shown.
    pub notes: Vec<String>,
}

const TIME_NAMES: [&str; 12] = [
    "date",
    "time",
    "timestamp",
    "datetime",
    "date time",
    "open time",
    "unix",
    "unix timestamp",
    "gmt time",
    "local time",
    "ts",
    "t",
];
const OPEN_NAMES: [&str; 3] = ["open", "o", "open price"];
const HIGH_NAMES: [&str; 4] = ["high", "h", "high price", "max"];
const LOW_NAMES: [&str; 4] = ["low", "l", "low price", "min"];
const CLOSE_NAMES: [&str; 7] = [
    "close",
    "c",
    "close price",
    "last",
    "price",
    "adj close",
    "adjusted close",
];
const VOLUME_NAMES: [&str; 5] = ["volume", "vol", "v", "tick volume", "qty"];

/// Which column holds what.
struct Columns {
    /// One column, or a date column and a time-of-day column to join.
    time: (usize, Option<usize>),
    open: Option<usize>,
    high: Option<usize>,
    low: Option<usize>,
    close: usize,
    volume: Option<usize>,
}

/// Reads bars from the text of a CSV file.
///
/// The first line names the columns. A time column and a closing price
/// are required; open, high and low fall back to the close and volume to
/// 0, each with a note. A file with no header whose rows are a time and
/// five numbers is read as time, open, high, low, close, volume.
pub fn parse_csv(text: &str) -> Result<Parsed, Error> {
    let text = text.trim_start_matches('\u{feff}');

    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());

    let (_, header) = lines.next().ok_or(Error::Empty)?;

    let separator = separator_of(header);
    let names: Vec<String> = split(header, separator).iter().map(|x| plain(x)).collect();

    let mut notes = Vec::new();
    let mut first_row = None;

    let columns = if headless(header, separator) {
        notes.push(
            "The file has no header line, so its columns were read as time, open, high, low, \
             close, volume."
                .to_string(),
        );

        first_row = Some((0, header));

        Columns {
            time: (0, None),
            open: Some(1),
            high: Some(2),
            low: Some(3),
            close: 4,
            volume: (names.len() > 5).then_some(5),
        }
    } else {
        columns(&names, &mut notes)?
    };

    let mut bars = Vec::new();
    let mut skipped = 0;
    let mut first_problem = None;

    for (at, line) in first_row.into_iter().chain(lines) {
        match row(line, separator, &columns) {
            Ok(bar) => bars.push(bar),
            Err(problem) => {
                skipped += 1;

                if first_problem.is_none() {
                    first_problem = Some(format!("line {}: {problem}", at + 1));
                }
            }
        }
    }

    if bars.is_empty() {
        return Err(Error::NoRows {
            skipped,
            problem: first_problem.unwrap_or_else(|| "there are no rows".to_string()),
        });
    }

    let (bars, duplicates) = tidy(bars);

    Ok(Parsed {
        bars,
        skipped,
        duplicates,
        first_problem,
        notes,
    })
}

/// Puts bars in time order and keeps the last of any with the same time.
/// Returns how many were dropped.
pub fn tidy(mut bars: Vec<Bar>) -> (Vec<Bar>, usize) {
    // A stable sort, so that of two rows with one time the later in the
    // file is still the later.
    bars.sort_by_key(|bar| bar.time_ms);

    let before = bars.len();
    let mut kept: Vec<Bar> = Vec::with_capacity(before);

    for bar in bars {
        match kept.last_mut() {
            Some(last) if last.time_ms == bar.time_ms => *last = bar,
            _ => kept.push(bar),
        }
    }

    let dropped = before - kept.len();

    (kept, dropped)
}

/// The separator a line uses: whichever of tab, semicolon and comma it
/// has most of outside quotes.
fn separator_of(line: &str) -> char {
    let count = |separator: char| split(line, separator).len();

    [',', ';', '\t']
        .into_iter()
        .rev()
        .max_by_key(|separator| count(*separator))
        .unwrap_or(',')
}

/// The fields of a line. A field in double quotes may hold the separator.
fn split(line: &str, separator: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;

    for letter in line.chars() {
        if letter == '"' {
            quoted = !quoted;
        } else if letter == separator && !quoted {
            fields.push(std::mem::take(&mut field).trim().to_string());
        } else {
            field.push(letter);
        }
    }

    fields.push(field.trim().to_string());

    fields
}

/// A column name without case, punctuation or a unit in brackets, so that
/// `Adj Close`, `adj_close` and `Adj. Close*` are one name.
fn plain(name: &str) -> String {
    let name = name.split(['(', '[']).next().unwrap_or_default();

    let spaced: String = name
        .chars()
        .map(|letter| match letter {
            '_' | '-' | '.' => ' ',
            other => other.to_ascii_lowercase(),
        })
        .filter(|letter| letter.is_ascii_alphanumeric() || *letter == ' ')
        .collect();

    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the first line is a row of data and not column names.
fn headless(line: &str, separator: char) -> bool {
    let fields = split(line, separator);

    fields.len() >= 5
        && parse_time(&fields[0]).is_some()
        && fields[1..5]
            .iter()
            .all(|field| number(field, separator).is_some())
}

fn columns(names: &[String], notes: &mut Vec<String>) -> Result<Columns, Error> {
    let find = |wanted: &[&str]| {
        wanted
            .iter()
            .find_map(|name| names.iter().position(|found| found == name))
    };

    let missing = |wanted: &str, expected: &[&str]| Error::MissingColumn {
        wanted: wanted.to_string(),
        expected: expected.join(", "),
        found: names
            .iter()
            .map(|name| {
                if name.is_empty() {
                    "(unnamed)"
                } else {
                    name.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    };

    let date = names.iter().position(|name| name == "date");
    let hour = names.iter().position(|name| name == "time");

    let time = match (date, hour) {
        // Some exports split the day and the time of day in two columns.
        (Some(date), Some(hour)) => (date, Some(hour)),
        _ => (
            find(&TIME_NAMES).ok_or_else(|| missing("the time of each bar", &TIME_NAMES))?,
            None,
        ),
    };

    let close = find(&CLOSE_NAMES).ok_or_else(|| missing("the closing price", &CLOSE_NAMES))?;

    let open = find(&OPEN_NAMES);
    let high = find(&HIGH_NAMES);
    let low = find(&LOW_NAMES);

    // "Volume BTC", "Volume USD" and the like.
    let volume = find(&VOLUME_NAMES).or_else(|| {
        names
            .iter()
            .position(|name| name.starts_with("volume") || name.starts_with("vol "))
    });

    for (column, what) in [(open, "open"), (high, "high"), (low, "low")] {
        if column.is_none() {
            notes.push(format!(
                "The file has no {what} column, so the close was used for it."
            ));
        }
    }

    if volume.is_none() {
        notes.push("The file has no volume column, so volume was left out.".to_string());
    }

    Ok(Columns {
        time,
        open,
        high,
        low,
        close,
        volume,
    })
}

/// One row as a bar, or why not.
fn row(line: &str, separator: char, columns: &Columns) -> Result<Bar, String> {
    let fields = split(line, separator);

    let field = |at: usize| fields.get(at).map(String::as_str).unwrap_or_default();

    let time = match columns.time {
        (date, Some(hour)) => format!("{} {}", field(date), field(hour)),
        (time, None) => field(time).to_string(),
    };

    let time_ms =
        parse_time(&time).ok_or_else(|| format!("\"{time}\" is not a time it can read"))?;

    let price = |at: usize, what: &str| {
        number(field(at), separator)
            .filter(|price| price.is_finite() && *price > 0.0)
            .ok_or_else(|| format!("the {what} \"{}\" is not a price", field(at)))
    };

    let close = price(columns.close, "close")?;

    let or_close = |column: Option<usize>, what: &str| match column {
        Some(at) => price(at, what),
        None => Ok(close),
    };

    let open = or_close(columns.open, "open")?;
    let high = or_close(columns.high, "high")?;
    let low = or_close(columns.low, "low")?;

    let volume = columns
        .volume
        .and_then(|at| number(field(at), separator))
        .filter(|volume| volume.is_finite() && *volume > 0.0)
        .unwrap_or(0.0);

    Ok(Bar {
        time_ms,
        open,
        // A high below the close, or a low above it, is a slip in the
        // data. The bar is widened to hold all four prices.
        high: high.max(open).max(close).max(low),
        low: low.min(open).min(close).min(high),
        close,
        volume,
    })
}

/// A number as files write them: thousands separators, a decimal comma
/// in a file that separates its columns with semicolons (the usual pair
/// in Europe), and K, M or B after a volume.
fn number(text: &str, separator: char) -> Option<f64> {
    let mut text: String = text
        .chars()
        .filter(|letter| !letter.is_whitespace() && *letter != '$')
        .collect();

    if text.contains(',') {
        if separator == ';' && !text.contains('.') && text.matches(',').count() == 1 {
            text = text.replace(',', ".");
        } else {
            text = text.replace(',', "");
        }
    }

    let scale = match text.chars().last()? {
        'K' | 'k' => 1e3,
        'M' | 'm' => 1e6,
        'B' | 'b' => 1e9,
        _ => 1.0,
    };

    if scale != 1.0 {
        text.pop();
    }

    // Rust's own reading accepts "nan" and "inf", which are not numbers
    // here.
    if !text.chars().any(|letter| letter.is_ascii_digit()) {
        return None;
    }

    text.parse::<f64>().ok().map(|value| value * scale)
}

/// A time in milliseconds since 1970 UTC, from any of:
///
/// - a count since 1970 in seconds, milliseconds, microseconds or
///   nanoseconds, told apart by size;
/// - `YYYY-MM-DD`, also with `/` or `.` between the parts, or `YYYYMMDD`;
/// - either of those followed by a space or `T` and `HH:MM[:SS[.fff]]`,
///   then nothing, `Z`, or an offset such as `+05:30`.
pub fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim().trim_matches('"').trim();

    if text.is_empty() {
        return None;
    }

    let digits = text.bytes().all(|byte| byte.is_ascii_digit());

    if digits
        && text.len() == 8
        && let Some(day) = date(&format!("{}-{}-{}", &text[..4], &text[4..6], &text[6..]))
    {
        return Some(day * 86_400_000);
    }

    if digits || text.parse::<f64>().is_ok() {
        let count = text.parse::<f64>().ok().filter(|count| count.is_finite())?;
        let size = count.abs();

        let ms = if size < 1e11 {
            count * 1e3
        } else if size < 1e14 {
            count
        } else if size < 1e17 {
            count / 1e3
        } else {
            count / 1e6
        };

        return Some(ms.round() as i64);
    }

    let (day, clock) = match text.find(['T', 't', ' ']) {
        Some(at) => (&text[..at], text[at + 1..].trim()),
        None => (text, ""),
    };

    let day = date(day)?;

    let ms = if clock.is_empty() {
        0
    } else {
        time_of_day(clock)?
    };

    Some(day * 86_400_000 + ms)
}

/// Days since 1970-01-01 for a date written year, month, day.
fn date(text: &str) -> Option<i64> {
    let mut parts = text.split(['-', '/', '.']);

    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;

    if parts.next().is_some() || year.len() != 4 {
        return None;
    }

    let whole = |part: &str| {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse::<i64>().ok())
            .flatten()
    };

    let (year, month, day) = (whole(year)?, whole(month)?, whole(day)?);

    if !(1..=12).contains(&month) || day < 1 || day > days_in(year, month) {
        return None;
    }

    Some(days_from_civil(year, month, day))
}

fn days_in(year: i64, month: i64) -> i64 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;

    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 in the calendar now in use, counted in whole
/// 400-year cycles from a year that starts in March, so that the leap
/// day is the last day of its year.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// Milliseconds into the day for `HH:MM[:SS[.fff]]` with nothing, `Z` or
/// an offset after it. An offset is taken off, so the result is UTC and
/// may be negative or more than a day.
fn time_of_day(text: &str) -> Option<i64> {
    let text = text
        .trim_end_matches(" UTC")
        .trim_end_matches(" GMT")
        .trim();

    let (clock, offset) = if let Some(clock) = text.strip_suffix(['Z', 'z']) {
        (clock, 0)
    } else if let Some(at) = text.rfind(['+', '-']) {
        let sign = if text[at..].starts_with('-') { -1 } else { 1 };
        let written: String = text[at + 1..].chars().filter(|x| *x != ':').collect();

        if !written.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        let (hours, minutes) = match written.len() {
            2 => (written.parse::<i64>().ok()?, 0),
            4 => (
                written[..2].parse::<i64>().ok()?,
                written[2..].parse::<i64>().ok()?,
            ),
            _ => return None,
        };

        (&text[..at], sign * (hours * 3_600_000 + minutes * 60_000))
    } else {
        (text, 0)
    };

    let mut parts = clock.trim().split(':');

    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = match parts.next() {
        Some(seconds) => seconds.parse::<f64>().ok()?,
        None => 0.0,
    };

    if parts.next().is_some()
        || !(0..24).contains(&hours)
        || !(0..60).contains(&minutes)
        || !(0.0..61.0).contains(&seconds)
    {
        return None;
    }

    Some(hours * 3_600_000 + minutes * 60_000 + (seconds * 1e3).round() as i64 - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn dates_convert_to_days_since_1970() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(2024, 3, 1), 19_783);

        // 2021-01-01 00:00:00 UTC is 1609459200 seconds.
        assert_eq!(days_from_civil(2021, 1, 1) * 86_400, 1_609_459_200);
    }

    #[test]
    fn every_day_follows_the_one_before_for_two_centuries() {
        let mut expected = days_from_civil(1900, 1, 1);

        for year in 1900..2100 {
            for month in 1..=12 {
                for day in 1..=days_in(year, month) {
                    assert_eq!(days_from_civil(year, month, day), expected);

                    expected += 1;
                }
            }
        }
    }

    #[test]
    fn times_are_read_in_each_way_of_writing_them() {
        let noon = 1_609_459_200_000 + 12 * 3_600_000 + 30 * 60_000;

        assert_eq!(parse_time("1609459200"), Some(1_609_459_200_000));
        assert_eq!(parse_time("1609459200000"), Some(1_609_459_200_000));
        assert_eq!(parse_time("1609459200000000"), Some(1_609_459_200_000));
        assert_eq!(parse_time("1609459200.5"), Some(1_609_459_200_500));
        assert_eq!(parse_time("2021-01-01"), Some(1_609_459_200_000));
        assert_eq!(parse_time("2021/01/01"), Some(1_609_459_200_000));
        assert_eq!(parse_time("20210101"), Some(1_609_459_200_000));
        assert_eq!(parse_time("2021-01-01 12:30"), Some(noon));
        assert_eq!(parse_time("2021-01-01 12:30:00"), Some(noon));
        assert_eq!(parse_time("2021-01-01T12:30:00Z"), Some(noon));
        assert_eq!(parse_time("2021-01-01T12:30:00.250Z"), Some(noon + 250));
        assert_eq!(parse_time("\"2021-01-01T12:30:00\""), Some(noon));

        // Half past six in the evening in India is one in the afternoon
        // in UTC.
        assert_eq!(
            parse_time("2021-01-01T18:00:00+05:30"),
            Some(noon),
            "an offset is taken off"
        );
        assert_eq!(parse_time("2021-01-01 07:30:00-0500"), Some(noon));
    }

    #[test]
    fn what_is_not_a_time_is_refused() {
        for text in [
            "",
            "yesterday",
            "2021-13-01",
            "2021-02-30",
            "2023-02-29",
            "31/12/2021",
            "2021-01-01 25:00",
            "2021-01-01 12:61",
            "2021-01",
            "nan",
        ] {
            assert_eq!(parse_time(text), None, "{text}");
        }
    }

    #[test]
    fn a_plain_file_is_read() {
        let parsed = parse_csv(
            "Date,Open,High,Low,Close,Adj Close,Volume\n\
             2021-01-04,10,11,9.5,10.5,10.4,1000\n\
             2021-01-05,10.5,12,10,11.5,11.4,1500\n",
        )
        .unwrap();

        assert_eq!(parsed.bars.len(), 2);
        assert_eq!(parsed.skipped, 0);
        assert!(parsed.notes.is_empty());

        assert_eq!(
            parsed.bars[1],
            Bar {
                time_ms: days_from_civil(2021, 1, 5) * DAY,
                open: 10.5,
                high: 12.0,
                low: 10.0,
                // The close itself, not the adjusted one, to go with the
                // other three prices.
                close: 11.5,
                volume: 1500.0,
            }
        );
    }

    #[test]
    fn columns_may_come_in_any_order_under_other_names() {
        let parsed = parse_csv(
            "vol;LOW;timestamp;Close;HIGH;open\n\
             5;9;1609459200;10;11;9,5\n\
             6;10;1609545600;11;12;10\n",
        )
        .unwrap();

        assert_eq!(parsed.bars.len(), 2);
        assert_eq!(parsed.bars[0].time_ms, 1_609_459_200_000);
        assert_eq!(parsed.bars[0].open, 9.5, "a decimal comma");
        assert_eq!(parsed.bars[0].low, 9.0);
        assert_eq!(parsed.bars[0].high, 11.0);
        assert_eq!(parsed.bars[1].volume, 6.0);
    }

    #[test]
    fn tabs_quotes_and_thousands_separators_are_read() {
        let parsed = parse_csv(
            "\u{feff}\"Datetime\"\t\"Open\"\t\"High\"\t\"Low\"\t\"Adj Close\"\t\"Volume\"\n\
             \"2021-01-04T09:30:00Z\"\t\"1,010.5\"\t\"1,020\"\t\"1,000\"\t\"1,015\"\t\"1.5M\"\n",
        )
        .unwrap();

        assert_eq!(parsed.bars.len(), 1);
        assert_eq!(parsed.bars[0].open, 1010.5);
        assert_eq!(
            parsed.bars[0].close, 1015.0,
            "adj close when there is no close"
        );
        assert_eq!(parsed.bars[0].volume, 1_500_000.0);
    }

    #[test]
    fn a_date_column_and_a_time_column_are_joined() {
        let parsed = parse_csv(
            "Date,Time,Open,High,Low,Close,Volume\n\
             2021.01.04,09:30,10,11,9,10,7\n\
             2021.01.04,09:31,10,11,9,10.5,8\n",
        )
        .unwrap();

        assert_eq!(parsed.bars.len(), 2);
        assert_eq!(parsed.bars[1].time_ms - parsed.bars[0].time_ms, 60_000);
    }

    #[test]
    fn a_file_with_no_header_is_read_in_the_usual_order() {
        let parsed =
            parse_csv("1609459200,10,11,9,10.5,100\n1609545600,10.5,12,10,11,200\n").unwrap();

        assert_eq!(parsed.bars.len(), 2);
        assert_eq!(parsed.bars[0].close, 10.5);
        assert_eq!(parsed.notes.len(), 1);
    }

    #[test]
    fn rows_are_sorted_and_the_later_of_two_with_one_time_is_kept() {
        let parsed = parse_csv(
            "date,open,high,low,close,volume\n\
             2021-01-06,3,3,3,3,1\n\
             2021-01-04,1,1,1,1,1\n\
             2021-01-05,2,2,2,2,1\n\
             2021-01-04,1,1,1,1.5,1\n",
        )
        .unwrap();

        let closes: Vec<f64> = parsed.bars.iter().map(|bar| bar.close).collect();

        assert_eq!(closes, [1.5, 2.0, 3.0]);
        assert_eq!(parsed.duplicates, 1);
    }

    #[test]
    fn bad_rows_are_skipped_and_counted() {
        let parsed = parse_csv(
            "date,open,high,low,close,volume\n\
             2021-01-04,1,1,1,1,1\n\
             not a date,1,1,1,1,1\n\
             2021-01-05,1,1,1,abc,1\n\
             2021-01-06,1,1,1,-4,1\n\
             2021-01-07,1,1\n\
             \n\
             2021-01-08,2,1,3,2,\n",
        )
        .unwrap();

        assert_eq!(parsed.bars.len(), 2);
        assert_eq!(parsed.skipped, 4);
        assert!(
            parsed
                .first_problem
                .as_deref()
                .unwrap()
                .starts_with("line 3:"),
            "{:?}",
            parsed.first_problem
        );

        // A high below the low is a slip; the bar is widened.
        assert_eq!(parsed.bars[1].high, 3.0);
        assert_eq!(parsed.bars[1].low, 1.0);
        assert_eq!(parsed.bars[1].volume, 0.0);
    }

    #[test]
    fn a_missing_column_is_named_with_what_was_found() {
        let error = parse_csv("date,open,high,low,volume\n2021-01-04,1,1,1,1\n").unwrap_err();

        let Error::MissingColumn {
            wanted,
            expected,
            found,
        } = &error
        else {
            panic!("{error:?}");
        };

        assert_eq!(wanted, "the closing price");
        assert!(expected.contains("close") && expected.contains("adj close"));
        assert_eq!(found, "date, open, high, low, volume");

        let said = error.to_string();

        assert!(
            said.contains("closing price") && said.contains("date, open, high"),
            "{said}"
        );

        assert!(matches!(
            parse_csv("open,high,low,close\n1,1,1,1\n"),
            Err(Error::MissingColumn { .. })
        ));
    }

    #[test]
    fn missing_open_high_low_and_volume_are_noted_not_refused() {
        let parsed = parse_csv("date,price\n2021-01-04,10\n2021-01-05,11\n").unwrap();

        assert_eq!(parsed.bars[1].open, 11.0);
        assert_eq!(parsed.bars[1].volume, 0.0);
        assert_eq!(parsed.notes.len(), 4);
    }

    #[test]
    fn nothing_usable_is_an_error_that_says_why() {
        assert_eq!(parse_csv("  \n"), Err(Error::Empty));

        let error = parse_csv("date,close\nsoon,1\n").unwrap_err();

        assert!(
            matches!(&error, Error::NoRows { skipped: 1, problem } if problem.contains("soon")),
            "{error:?}"
        );
    }
}
