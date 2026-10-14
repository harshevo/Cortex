//! The Dhan broker's data API, version 2: who the token belongs to,
//! price history, a quote with market depth, and the live feed
//! ([`feed`]). Only data is read: nothing here can place an order.
//!
//! A token is made at web.dhan.co, under My Profile → Access DhanHQ
//! APIs, and lasts a day. The data endpoints also need Dhan's data plan
//! on the account; without it they answer with error 806.

pub mod depth;
pub mod feed;
pub mod instruments;

use std::time::Duration;

use cortex_liquid::Bar;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock;

pub use instruments::{Instrument, Segment};

const API: &str = "https://api.dhan.co/v2";
const FEED: &str = "wss://api-feed.dhan.co";

/// The most days the intraday endpoint gives in one request.
const INTRADAY_DAYS: i64 = 90;

/// What a person gives to connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub client_id: String,
    pub access_token: String,

    /// Other addresses to use instead of Dhan's own, for testing against
    /// a stand-in. Empty for Dhan's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub feed: String,
}

impl Credentials {
    pub fn new(client_id: &str, access_token: &str) -> Self {
        Self {
            client_id: client_id.trim().to_string(),
            access_token: access_token.trim().to_string(),
            api: std::env::var("CORTEX_DHAN_API").unwrap_or_default(),
            feed: std::env::var("CORTEX_DHAN_FEED").unwrap_or_default(),
        }
    }

    pub fn api(&self) -> &str {
        if self.api.is_empty() { API } else { &self.api }
    }

    pub fn feed(&self) -> &str {
        if self.feed.is_empty() {
            FEED
        } else {
            &self.feed
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DhanError {
    #[error("Dhan cannot be reached: {0}")]
    Network(String),

    #[error("{0}")]
    Refused(String),

    #[error("Dhan's answer could not be read: {0}")]
    Unreadable(String),
}

/// Who the token belongs to, and whether the account has the data plan.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Profile {
    pub client_id: String,

    /// When the token stops working, as Dhan writes it.
    pub token_valid_until: String,

    /// "Active" when the data plan is on.
    pub data_plan: String,
    pub data_valid_until: String,
}

/// One level of the order book.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Level {
    pub price: f64,
    pub quantity: f64,
    pub orders: u32,
}

/// A snapshot of an instrument's trading today.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Quote {
    pub last_price: f64,

    /// The volume-weighted average price today, from the exchange.
    pub average_price: Option<f64>,

    /// Shares or contracts traded today.
    pub volume: f64,

    /// Orders waiting to buy and to sell, in all.
    pub buy_quantity: f64,
    pub sell_quantity: f64,

    pub open: f64,
    pub high: f64,
    pub low: f64,

    /// Yesterday's close.
    pub previous_close: Option<f64>,
    pub open_interest: Option<f64>,

    /// Best five bids and asks.
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,

    pub upper_circuit: Option<f64>,
    pub lower_circuit: Option<f64>,

    /// When it was taken, ms since 1970.
    pub at_ms: i64,
}

impl Quote {
    /// The gap between the best ask and the best bid.
    pub fn spread(&self) -> Option<f64> {
        let bid = self.bids.first().filter(|level| level.price > 0.0)?;
        let ask = self.asks.first().filter(|level| level.price > 0.0)?;

        Some(ask.price - bid.price)
    }
}

/// A connection to Dhan's data API.
#[derive(Clone)]
pub struct Dhan {
    http: reqwest::Client,
    credentials: Credentials,
}

/// Dhan's error codes, said so that a person can act on them.
fn explain(code: &str) -> Option<&'static str> {
    Some(match code.trim_start_matches("DH-") {
        "806" => "the Dhan account has no data plan. Data APIs need it: subscribe on web.dhan.co.",
        "807" | "DH-901" | "901" => "the access token has expired. Make a new one on web.dhan.co.",
        "808" => "the client ID or the access token is wrong.",
        "809" => "the access token is not valid.",
        "810" => "the client ID is not valid.",
        "805" => "too many requests or connections to Dhan. Wait a minute.",
        "804" => "too many instruments in one request.",
        "813" => "Dhan does not know that security ID.",
        "812" => "Dhan did not accept the date range.",
        "814" | "905" => "Dhan did not accept the request.",
        "904" => "too many requests to Dhan. Wait a few seconds.",
        "906" => "Dhan rejected the order details.",
        "800" | "908" => "Dhan had an internal error. Try again.",
        _ => return None,
    })
}

/// The reason in an error answer, in whichever shape Dhan sent it.
fn refusal(status: u16, body: &str) -> DhanError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();

    let mut code = None;
    let mut message = None;

    if let Some(value) = &parsed {
        for key in ["errorCode", "error_code", "code"] {
            if let Some(found) = find(value, key) {
                code = Some(found);
            }
        }

        for key in ["errorMessage", "error_message", "message", "remarks"] {
            if let Some(found) = find(value, key) {
                message = Some(found);
            }
        }

        // `{"data": {"806": "Data APIs not Subscribed"}, "status": "failed"}`
        if code.is_none()
            && let Some(data) = value.get("data").and_then(Value::as_object)
            && let Some((key, text)) = data.iter().next()
            && key.chars().all(|c| c.is_ascii_digit())
        {
            code = Some(key.clone());
            message = text.as_str().map(str::to_string);
        }
    }

    let text = match code.as_deref().and_then(explain) {
        Some(explained) => explained.to_string(),
        None => match (status, message) {
            (401 | 403, Some(message)) => {
                format!("Dhan refused the client ID or access token: {message}")
            }
            (401 | 403, None) => format!(
                "Dhan refused the client ID or access token ({status}). Make a new token on web.dhan.co."
            ),
            (_, Some(message)) => format!("Dhan said: {message}"),
            _ => format!("Dhan answered {status}."),
        },
    };

    DhanError::Refused(text)
}

fn find(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(|found| match found {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .or_else(|| map.values().find_map(|inner| find(inner, key))),
        _ => None,
    }
}

impl Dhan {
    pub fn new(credentials: Credentials) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_default();

        Self { http, credentials }
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, DhanError> {
        let response = request
            .header("access-token", &self.credentials.access_token)
            .header("client-id", &self.credentials.client_id)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|error| DhanError::Network(error.without_url().to_string()))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|error| DhanError::Network(error.to_string()))?;

        if !(200..300).contains(&status) {
            return Err(refusal(status, &body));
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|error| DhanError::Unreadable(error.to_string()))?;

        if value.get("status").and_then(Value::as_str) == Some("failed") {
            return Err(refusal(status, &body));
        }

        Ok(value)
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, DhanError> {
        let address = format!("{}{path}", self.credentials.api());

        self.send(self.http.post(address).json(&body)).await
    }

    /// Checks the token, and says whose it is.
    pub async fn profile(&self) -> Result<Profile, DhanError> {
        let address = format!("{}/profile", self.credentials.api());
        let value = self.send(self.http.get(address)).await?;

        let text = |key: &str| {
            value
                .get(key)
                .map(|found| match found {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };

        Ok(Profile {
            client_id: text("dhanClientId"),
            token_valid_until: text("tokenValidity"),
            data_plan: text("dataPlan"),
            data_valid_until: text("dataValidity"),
        })
    }

    /// Candles of `interval` minutes (1, 5, 15, 25 or 60) from `from_ms`
    /// to `to_ms`, asked for 90 days at a time.
    pub async fn intraday(
        &self,
        instrument: &Instrument,
        interval: u32,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<Bar>, DhanError> {
        let mut bars = Vec::new();
        let mut start = from_ms;

        while start < to_ms {
            let end = (start + INTRADAY_DAYS * clock::DAY_MS).min(to_ms);

            let value = self
                .post(
                    "/charts/intraday",
                    json!({
                        "securityId": instrument.security_id,
                        "exchangeSegment": instrument.segment.code(),
                        "instrument": instrument.kind,
                        "interval": interval.to_string(),
                        "oi": false,
                        "fromDate": format!("{} {}:00", clock::date_text(start), clock::time_text(start)),
                        "toDate": format!("{} {}:00", clock::date_text(end), clock::time_text(end)),
                    }),
                )
                .await?;

            bars.extend(read_bars(&value)?);
            start = end;
        }

        Ok(tidy(bars, true))
    }

    /// Daily candles from `from_ms` to `to_ms`.
    pub async fn daily(
        &self,
        instrument: &Instrument,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<Bar>, DhanError> {
        let value = self
            .post(
                "/charts/historical",
                json!({
                    "securityId": instrument.security_id,
                    "exchangeSegment": instrument.segment.code(),
                    "instrument": instrument.kind,
                    "expiryCode": 0,
                    "oi": false,
                    "fromDate": clock::date_text(from_ms),
                    "toDate": clock::date_text(to_ms + clock::DAY_MS),
                }),
            )
            .await?;

        Ok(tidy(read_bars(&value)?, false))
    }

    /// A quote with the best five bids and asks.
    pub async fn quote(&self, instrument: &Instrument) -> Result<Quote, DhanError> {
        let id: i64 = instrument.security_id.parse().unwrap_or_default();
        let segment = instrument.segment.code();

        let value = self
            .post("/marketfeed/quote", json!({ segment: [id] }))
            .await?;

        let item = value
            .pointer(&format!("/data/{segment}/{}", instrument.security_id))
            .ok_or_else(|| DhanError::Unreadable("the quote has no entry for it".to_string()))?;

        Ok(read_quote(item))
    }
}

fn numbers(value: &Value, key: &str) -> Vec<f64> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_f64().unwrap_or(0.0))
                .collect()
        })
        .unwrap_or_default()
}

/// Bars from Dhan's columns of open, high, low, close, volume, timestamp.
fn read_bars(value: &Value) -> Result<Vec<Bar>, DhanError> {
    // Some answers wrap the columns in `data`.
    let value = value
        .get("data")
        .filter(|data| data.is_object())
        .unwrap_or(value);

    let open = numbers(value, "open");
    let high = numbers(value, "high");
    let low = numbers(value, "low");
    let close = numbers(value, "close");
    let volume = numbers(value, "volume");
    let time = numbers(value, "timestamp");

    if value.get("close").is_none() {
        return Err(DhanError::Unreadable(
            "no candles in the answer".to_string(),
        ));
    }

    let count = [open.len(), high.len(), low.len(), close.len(), time.len()]
        .into_iter()
        .min()
        .unwrap_or(0);

    Ok((0..count)
        .map(|at| Bar {
            time_ms: (time[at] * 1_000.0) as i64,
            open: open[at],
            high: high[at],
            low: low[at],
            close: close[at],
            volume: volume.get(at).copied().unwrap_or(0.0),
        })
        .filter(|bar| bar.problem().is_none())
        .collect())
}

/// Sorts, drops repeats, and puts times written as India's wall clock
/// back on UTC: intraday candles that begin at 09:15 when read as UTC
/// were written that way.
pub fn tidy(bars: Vec<Bar>, intraday: bool) -> Vec<Bar> {
    let (mut bars, _) = cortex_liquid::bars::tidy(bars);

    if intraday && !bars.is_empty() {
        // Each day's first candle, as minutes past midnight UTC. Indian
        // sessions open at 03:30 to 03:45 UTC; read as wall clock they
        // open at 09:00 to 09:15.
        let mut firsts: Vec<i64> = Vec::new();
        let mut day = i64::MIN;

        for bar in &bars {
            let today = bar.time_ms.div_euclid(clock::DAY_MS);

            if today != day {
                day = today;
                firsts.push(bar.time_ms.rem_euclid(clock::DAY_MS) / clock::MINUTE_MS);
            }
        }

        firsts.sort_unstable();

        if firsts[firsts.len() / 2] >= 8 * 60 {
            for bar in &mut bars {
                bar.time_ms -= clock::IST_MS;
            }
        }
    }

    bars
}

fn read_quote(item: &Value) -> Quote {
    let number = |path: &str| item.pointer(path).and_then(Value::as_f64);

    let side = |name: &str| -> Vec<Level> {
        item.pointer(&format!("/depth/{name}"))
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .map(|level| Level {
                        price: level.get("price").and_then(Value::as_f64).unwrap_or(0.0),
                        quantity: level.get("quantity").and_then(Value::as_f64).unwrap_or(0.0),
                        orders: level.get("orders").and_then(Value::as_u64).unwrap_or(0) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let last = number("/last_price").unwrap_or(0.0);
    let net_change = number("/net_change");

    Quote {
        last_price: last,
        average_price: number("/average_price").filter(|price| *price > 0.0),
        volume: number("/volume").unwrap_or(0.0),
        buy_quantity: number("/buy_quantity").unwrap_or(0.0),
        sell_quantity: number("/sell_quantity").unwrap_or(0.0),
        open: number("/ohlc/open").unwrap_or(0.0),
        high: number("/ohlc/high").unwrap_or(0.0),
        low: number("/ohlc/low").unwrap_or(0.0),
        previous_close: number("/ohlc/close")
            .filter(|close| *close > 0.0)
            .or_else(|| net_change.map(|change| last - change)),
        open_interest: number("/oi").filter(|oi| *oi > 0.0),
        bids: side("buy"),
        asks: side("sell"),
        upper_circuit: number("/upper_circuit_limit"),
        lower_circuit: number("/lower_circuit_limit"),
        at_ms: clock::now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candles_are_read_from_columns() {
        let value = json!({
            "open": [10.0, 11.0],
            "high": [12.0, 13.0],
            "low": [9.0, 10.5],
            "close": [11.0, 12.5],
            "volume": [100, 200],
            "timestamp": [1726026300, 1726026600],
        });

        let bars = read_bars(&value).unwrap();

        assert_eq!(bars.len(), 2);
        assert_eq!(bars[1].time_ms, 1_726_026_600_000);
        assert_eq!(bars[1].volume, 200.0);
    }

    #[test]
    fn times_written_as_india_wall_clock_are_moved_back() {
        // 09:15 read as UTC.
        let wall = 1_726_026_300_000 + clock::IST_MS;

        let bars: Vec<Bar> = (0..75)
            .map(|at| Bar {
                time_ms: wall + at * 300_000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 0.0,
            })
            .collect();

        let tidied = tidy(bars, true);

        assert_eq!(clock::time_text(tidied[0].time_ms), "09:15");

        // True UTC times are left alone.
        let true_times = tidy(tidied.clone(), true);

        assert_eq!(true_times, tidied);
    }

    #[test]
    fn errors_say_what_to_do() {
        let no_plan = refusal(
            400,
            r#"{"data":{"806":"Data APIs not Subscribed"},"status":"failed"}"#,
        );

        assert!(no_plan.to_string().contains("data plan"));

        let expired = refusal(
            401,
            r#"{"errorType":"Invalid_Authentication","errorCode":"DH-901","errorMessage":"Client ID or user generated access token is invalid or expired."}"#,
        );

        assert!(expired.to_string().contains("expired"), "{expired}");

        assert!(refusal(500, "oops").to_string().contains("500"));
    }

    #[test]
    fn a_quote_is_read_with_its_depth() {
        let item = json!({
            "average_price": 101.5,
            "buy_quantity": 1825,
            "sell_quantity": 900,
            "depth": {
                "buy": [{ "quantity": 1800, "orders": 3, "price": 100.95 }],
                "sell": [{ "quantity": 500, "orders": 2, "price": 101.05 }]
            },
            "last_price": 101.0,
            "net_change": 1.0,
            "ohlc": { "open": 100.0, "close": 0, "high": 102.0, "low": 99.0 },
            "volume": 123456
        });

        let quote = read_quote(&item);

        assert_eq!(quote.average_price, Some(101.5));
        assert_eq!(quote.previous_close, Some(100.0));
        assert!((quote.spread().unwrap() - 0.1).abs() < 1e-9);
    }
}
