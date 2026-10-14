//! Times as Indian markets keep them: India Standard Time, five and a
//! half hours ahead of UTC with no summer time, and the hours the
//! exchanges are open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ALWAYS_OPEN: AtomicBool = AtomicBool::new(false);

/// Whether every segment is to be taken as open all day, every day: for
/// trying the live screens against the stand-in out of hours. On when
/// `CORTEX_MARKET_ALWAYS_OPEN` is set, or when connected to a stand-in.
pub fn always_open() -> bool {
    ALWAYS_OPEN.load(Ordering::Relaxed) || std::env::var_os("CORTEX_MARKET_ALWAYS_OPEN").is_some()
}

pub fn set_always_open(open: bool) {
    ALWAYS_OPEN.store(open, Ordering::Relaxed);
}

/// India Standard Time, ahead of UTC.
pub const IST_MS: i64 = 19_800_000;

pub const MINUTE_MS: i64 = 60_000;
pub const DAY_MS: i64 = 86_400_000;

/// Milliseconds since 1970 UTC, now.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The day in India a time falls on, as days since 1970.
pub fn ist_day(time_ms: i64) -> i64 {
    (time_ms + IST_MS).div_euclid(DAY_MS)
}

/// Minutes since midnight in India.
pub fn ist_minute(time_ms: i64) -> i64 {
    (time_ms + IST_MS).rem_euclid(DAY_MS) / MINUTE_MS
}

/// Monday is 0. 1970-01-01 was a Thursday.
pub fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

/// The UTC time at which `minute` past midnight in India falls on `day`.
pub fn at_ist(day: i64, minute: i64) -> i64 {
    day * DAY_MS + minute * MINUTE_MS - IST_MS
}

/// The hours an exchange segment trades, in minutes past midnight in
/// India.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub open: i64,
    pub close: i64,

    /// Trades on weekends too: only for testing.
    pub every_day: bool,
}

impl Session {
    /// NSE and BSE cash and derivatives, and their indices.
    pub const EQUITY: Session = Session {
        open: 9 * 60 + 15,
        close: 15 * 60 + 30,
        every_day: false,
    };

    /// Open all day, every day: what every segment trades in when
    /// `CORTEX_MARKET_ALWAYS_OPEN` is set, to try the live screens out
    /// of hours against the stand-in.
    pub const ALWAYS: Session = Session {
        open: 0,
        close: 24 * 60,
        every_day: true,
    };

    /// MCX commodities.
    pub const COMMODITY: Session = Session {
        open: 9 * 60,
        close: 23 * 60 + 30,
        every_day: false,
    };

    /// Currency derivatives.
    pub const CURRENCY: Session = Session {
        open: 9 * 60,
        close: 17 * 60,
        every_day: false,
    };

    fn trades_on(&self, day: i64) -> bool {
        self.every_day || weekday(day) < 5
    }

    /// Whether the exchange is open at `time_ms`, holidays aside.
    pub fn is_open(&self, time_ms: i64) -> bool {
        let day = ist_day(time_ms);
        let minute = ist_minute(time_ms);

        self.trades_on(day) && minute >= self.open && minute < self.close
    }

    /// The start of the candle of `interval` minutes that `time_ms` falls
    /// in, counting candles from the session's open.
    pub fn bucket(&self, time_ms: i64, interval: i64) -> i64 {
        let day = ist_day(time_ms);
        let since = (ist_minute(time_ms) - self.open).max(0);

        at_ist(day, self.open + since / interval * interval)
    }

    /// The start of the candle after the one that starts at `time_ms`,
    /// skipping nights and weekends (not holidays).
    pub fn next(&self, time_ms: i64, interval: i64) -> i64 {
        let next = time_ms + interval * MINUTE_MS;
        let day = ist_day(next);

        if ist_minute(next) < self.close
            && ist_minute(next) >= self.open
            && self.trades_on(day)
            && ist_day(next) == ist_day(time_ms)
        {
            return next;
        }

        let mut day = ist_day(time_ms) + 1;

        while !self.trades_on(day) {
            day += 1;
        }

        at_ist(day, self.open)
    }
}

/// The next trading day's start after a daily bar at `time_ms`.
pub fn next_weekday(time_ms: i64) -> i64 {
    let mut day = ist_day(time_ms) + 1;

    while weekday(day) >= 5 {
        day += 1;
    }

    day * DAY_MS - IST_MS
}

/// `2024-03-09` for a day in India.
pub fn date_text(time_ms: i64) -> String {
    let (year, month, day) = civil(ist_day(time_ms));

    format!("{year:04}-{month:02}-{day:02}")
}

/// `14:35` in India.
pub fn time_text(time_ms: i64) -> String {
    let minute = ist_minute(time_ms);

    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// `09 Mar 14:35` in India.
pub fn moment_text(time_ms: i64) -> String {
    let (_, month, day) = civil(ist_day(time_ms));

    format!(
        "{day:02} {} {}",
        MONTHS[(month - 1) as usize],
        time_text(time_ms)
    )
}

/// `09 Mar` in India.
pub fn day_text(time_ms: i64) -> String {
    let (_, month, day) = civil(ist_day(time_ms));

    format!("{day:02} {}", MONTHS[(month - 1) as usize])
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days since 1970 as year, month and day (Howard Hinnant's method).
pub fn civil(days: i64) -> (i64, i64, i64) {
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

    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2024-09-11 09:15 IST is 03:45 UTC.
    const OPEN_0911: i64 = 1_726_026_300_000;

    #[test]
    fn india_time_is_read_and_written() {
        assert_eq!(ist_minute(OPEN_0911), 9 * 60 + 15);
        assert_eq!(date_text(OPEN_0911), "2024-09-11");
        assert_eq!(time_text(OPEN_0911), "09:15");
        assert_eq!(moment_text(OPEN_0911), "11 Sep 09:15");
        assert_eq!(weekday(ist_day(OPEN_0911)), 2, "a Wednesday");
    }

    #[test]
    fn candles_are_counted_from_the_open_and_skip_the_night() {
        let session = Session::EQUITY;

        let at = OPEN_0911 + 7 * MINUTE_MS;

        assert_eq!(session.bucket(at, 5), OPEN_0911 + 5 * MINUTE_MS);
        assert_eq!(session.bucket(at, 60), OPEN_0911);

        // The last five-minute candle, 15:25, is followed by 09:15 the
        // next day.
        let last = OPEN_0911 + (6 * 60 + 10) * MINUTE_MS;

        assert_eq!(time_text(last), "15:25");
        assert_eq!(session.next(last, 5), OPEN_0911 + DAY_MS);

        // Friday's last candle is followed by Monday's first.
        let friday_last = last + 2 * DAY_MS;

        assert_eq!(session.next(friday_last, 5), OPEN_0911 + 5 * DAY_MS);
    }

    #[test]
    fn the_market_is_open_on_weekdays_in_its_hours() {
        let session = Session::EQUITY;

        assert!(session.is_open(OPEN_0911));
        assert!(!session.is_open(OPEN_0911 - MINUTE_MS));
        assert!(!session.is_open(OPEN_0911 + 4 * DAY_MS), "a Sunday");
    }
}
