//! A pretend Dhan on this machine: the profile, history and quote
//! endpoints, and a live feed that sends a full packet twice a second,
//! all from a made-up random walk. It accepts the access token
//! `stand-in` and no other.
//!
//! For trying the screens without an account, and for tests. Run it with
//! `cargo run -p cortex-market --example standin`, then start an app with
//! `CORTEX_DHAN_API` and `CORTEX_DHAN_FEED` set to the addresses it
//! prints (and `CORTEX_MARKET_ALWAYS_OPEN=1` out of market hours).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use crate::clock::{self, Session};
use crate::dhan::Segment;
use cortex_liquid::rng::Rng;

pub const TOKEN: &str = "stand-in";

/// Days of minutes made up for each instrument.
const DAYS: i64 = 260;

/// Addresses to point the app at.
#[derive(Debug, Clone)]
pub struct Addresses {
    pub api: String,
    pub feed: String,
}

/// Minute closes and volumes of one instrument, from the start of
/// [`DAYS`] ago to now.
#[derive(Default)]
struct Walk {
    times: Vec<i64>,
    closes: Vec<f64>,
    volumes: Vec<f64>,
}

#[derive(Default)]
struct Shared {
    walks: HashMap<String, Arc<Walk>>,
}

fn session_of(segment: &str) -> Session {
    Segment::from_code(segment).map_or(Session::EQUITY, Segment::session)
}

fn walk(shared: &Mutex<Shared>, id: &str, segment: &str) -> Arc<Walk> {
    let mut shared = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    shared
        .walks
        .entry(format!("{segment}:{id}"))
        .or_insert_with(|| Arc::new(make_walk(id, session_of(segment))))
        .clone()
}

fn make_walk(id: &str, session: Session) -> Walk {
    let seed: u64 = id.bytes().fold(7, |seed, byte| seed * 31 + u64::from(byte));
    let mut rng = Rng::new(seed);
    let mut price = 500.0 + (seed % 2_500) as f64;
    let mut step = 0.0;

    let now = clock::now_ms();
    let first_day = clock::ist_day(now) - DAYS;
    let mut walk = Walk::default();

    let mut time = clock::at_ist(first_day, session.open);

    if !session.is_open(time) {
        time = session.next(time - clock::MINUTE_MS, 1);
    }

    while time <= now {
        // A little momentum, so that there is something to find.
        step = 0.12 * step + 0.0007 * rng.normal();
        price *= f64::exp(step);

        walk.times.push(time);
        walk.closes.push(price);
        walk.volumes
            .push((2_000.0 * (0.5 * rng.normal()).exp()).round());

        time = session.next(time, 1);
    }

    walk
}

/// Candles of `minutes` (0 for days) from `from` to `to`.
fn candles(walk: &Walk, session: Session, minutes: i64, from: i64, to: i64) -> Value {
    let mut out: [Vec<f64>; 6] = Default::default();
    let mut current: Option<(i64, f64, f64, f64, f64, f64)> = None;

    let flush = |bar: (i64, f64, f64, f64, f64, f64), out: &mut [Vec<f64>; 6]| {
        out[0].push(bar.1);
        out[1].push(bar.2);
        out[2].push(bar.3);
        out[3].push(bar.4);
        out[4].push(bar.5);
        out[5].push((bar.0 / 1_000) as f64);
    };

    for at in 0..walk.times.len() {
        let time = walk.times[at];

        if time < from || time >= to {
            continue;
        }

        let start = if minutes > 0 {
            session.bucket(time, minutes)
        } else {
            clock::at_ist(clock::ist_day(time), 0)
        };

        let open = if at > 0 {
            walk.closes[at - 1]
        } else {
            walk.closes[at]
        };
        let close = walk.closes[at];
        let high = open.max(close) * 1.0002;
        let low = open.min(close) * 0.9998;

        current = Some(match current {
            Some(bar) if bar.0 == start => (
                start,
                bar.1,
                bar.2.max(high),
                bar.3.min(low),
                close,
                bar.5 + walk.volumes[at],
            ),
            Some(bar) => {
                flush(bar, &mut out);
                (start, open, high, low, close, walk.volumes[at])
            }
            None => (start, open, high, low, close, walk.volumes[at]),
        });
    }

    if let Some(bar) = current {
        flush(bar, &mut out);
    }

    json!({
        "open": out[0],
        "high": out[1],
        "low": out[2],
        "close": out[3],
        "volume": out[4],
        "timestamp": out[5],
    })
}

/// `2024-09-11 09:30:00` or `2024-09-11`, in India, as ms since 1970.
fn parse_date(text: &str) -> Option<i64> {
    let (date, time) = text.split_once(' ').unwrap_or((text, "00:00:00"));
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let mut clock_parts = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute) = (clock_parts.next()??, clock_parts.next()??);

    // Days from 1970 to the date (Howard Hinnant's inverse).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_shifted = (month + 9) % 12;
    let day_of_year = (153 * month_shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    Some(clock::at_ist(days, hour * 60 + minute))
}

async fn read_request(
    stream: &mut TcpStream,
) -> Option<(String, String, HashMap<String, String>, String)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];

    let header_end = loop {
        let count = stream.read(&mut chunk).await.ok()?;

        if count == 0 {
            return None;
        }

        buffer.extend_from_slice(&chunk[..count]);

        if let Some(end) = buffer.windows(4).position(|four| four == b"\r\n\r\n") {
            break end + 4;
        }

        if buffer.len() > 1 << 20 {
            return None;
        }
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();

    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
        .collect();

    let length: usize = headers
        .get("content-length")
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);

    while buffer.len() < header_end + length {
        let count = stream.read(&mut chunk).await.ok()?;

        if count == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..count]);
    }

    let body = String::from_utf8_lossy(&buffer[header_end..]).to_string();

    Some((method, path, headers, body))
}

async fn respond(stream: &mut TcpStream, status: u16, body: &Value) {
    let text = body.to_string();
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn serve_api(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) {
    let Some((method, path, headers, body)) = read_request(&mut stream).await else {
        return;
    };

    if headers.get("access-token").map(String::as_str) != Some(TOKEN) {
        respond(
            &mut stream,
            401,
            &json!({
                "errorType": "Invalid_Authentication",
                "errorCode": "DH-901",
                "errorMessage": "Client ID or user generated access token is invalid or expired.",
            }),
        )
        .await;

        return;
    }

    let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let text = |key: &str| {
        request
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    let answer = match (method.as_str(), path.trim_end_matches('/')) {
        ("GET", path) if path.ends_with("/profile") => json!({
            "dhanClientId": headers.get("client-id").cloned().unwrap_or_default(),
            "tokenValidity": "tomorrow",
            "activeSegment": "Equity, Derivative",
            "dataPlan": "Active",
            "dataValidity": "next month",
        }),

        ("POST", path)
            if path.ends_with("/charts/intraday") || path.ends_with("/charts/historical") =>
        {
            let id = text("securityId");
            let segment = text("exchangeSegment");
            let walk = walk(&shared, &id, &segment);
            let minutes: i64 = request
                .get("interval")
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| value.as_i64().map(|n| n.to_string()))
                })
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            let from = parse_date(&text("fromDate")).unwrap_or(0);
            let to = parse_date(&text("toDate")).unwrap_or(i64::MAX);

            candles(&walk, session_of(&segment), minutes, from, to)
        }

        ("POST", path) if path.ends_with("/marketfeed/quote") => {
            let Some((segment, ids)) = request.as_object().and_then(|map| map.iter().next()) else {
                respond(&mut stream, 400, &json!({"status": "failed"})).await;

                return;
            };

            let id = ids.get(0).map(|id| id.to_string()).unwrap_or_default();
            let walk = walk(&shared, &id, segment);
            let last = walk.closes.last().copied().unwrap_or(100.0);
            let today = clock::ist_day(clock::now_ms());
            let first_today = walk
                .times
                .iter()
                .position(|time| clock::ist_day(*time) == today);
            let previous = first_today
                .and_then(|at| at.checked_sub(1))
                .map(|at| walk.closes[at])
                .unwrap_or(last);
            let volume: f64 = first_today.map_or(0.0, |at| walk.volumes[at..].iter().sum());

            json!({
                "data": { segment: { id.clone(): {
                    "average_price": last * 0.999,
                    "buy_quantity": 12_000,
                    "sell_quantity": 9_000,
                    "depth": {
                        "buy": (0..5).map(|level| json!({"quantity": 100 + level * 20, "orders": 2 + level, "price": last - 0.05 * (level + 1) as f64})).collect::<Vec<_>>(),
                        "sell": (0..5).map(|level| json!({"quantity": 90 + level * 25, "orders": 1 + level, "price": last + 0.05 * (level + 1) as f64})).collect::<Vec<_>>(),
                    },
                    "last_price": last,
                    "net_change": last - previous,
                    "ohlc": { "open": previous, "close": previous, "high": last * 1.01, "low": last * 0.99 },
                    "volume": volume,
                }}},
                "status": "success",
            })
        }

        _ => {
            respond(
                &mut stream,
                404,
                &json!({"status": "failed", "remarks": "no such endpoint"}),
            )
            .await;

            return;
        }
    };

    respond(&mut stream, 200, &answer).await;
}

fn full_packet(segment: u8, id: i32, price: f64, volume: f64, now: i64) -> Vec<u8> {
    let mut bytes = vec![8u8];

    bytes.extend(162u16.to_le_bytes());
    bytes.push(segment);
    bytes.extend(id.to_le_bytes());

    let float = |value: f64| (value as f32).to_le_bytes();

    bytes.extend(float(price));
    bytes.extend(5i16.to_le_bytes());
    bytes.extend(((now / 1_000) as i32).to_le_bytes());
    bytes.extend(float(price * 0.999));
    bytes.extend((volume as i32).to_le_bytes());
    bytes.extend(9_000i32.to_le_bytes());
    bytes.extend(12_000i32.to_le_bytes());
    bytes.extend(0i32.to_le_bytes());
    bytes.extend(0i32.to_le_bytes());
    bytes.extend(0i32.to_le_bytes());
    bytes.extend(float(price * 0.995));
    bytes.extend(float(0.0));
    bytes.extend(float(price * 1.01));
    bytes.extend(float(price * 0.99));

    for level in 0..5i32 {
        bytes.extend((100 + level * 20).to_le_bytes());
        bytes.extend((90 + level * 25).to_le_bytes());
        bytes.extend((2 + level as i16).to_le_bytes());
        bytes.extend((1 + level as i16).to_le_bytes());
        bytes.extend(float(price - 0.05 * f64::from(level + 1)));
        bytes.extend(float(price + 0.05 * f64::from(level + 1)));
    }

    bytes
}

async fn serve_feed(stream: TcpStream, shared: Arc<Mutex<Shared>>) {
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            let query = request.uri().query().unwrap_or_default();

            if query.contains(&format!("token={TOKEN}")) {
                Ok(response)
            } else {
                let mut refused =
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);

                *refused.status_mut() =
                    tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;

                Err(refused)
            }
        },
    )
    .await
    .map_err(|error| eprintln!("stand-in feed handshake: {error}")) else {
        return;
    };

    // Waits for the subscription.
    let (segment, id) = loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let request: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

                if let Some(first) = request.pointer("/InstrumentList/0") {
                    let segment = first["ExchangeSegment"]
                        .as_str()
                        .unwrap_or("NSE_EQ")
                        .to_string();
                    let id = first["SecurityId"].as_str().unwrap_or("0").to_string();

                    break (segment, id);
                }
            }
            Some(Ok(_)) => {}
            _ => return,
        }
    };

    let walk = walk(&shared, &id, &segment);
    let number = Segment::from_code(&segment).map_or(1, Segment::number);
    let id_number: i32 = id.parse().unwrap_or(0);
    let mut price = walk.closes.last().copied().unwrap_or(100.0);
    let today = clock::ist_day(clock::now_ms());
    let mut volume: f64 = walk
        .times
        .iter()
        .zip(&walk.volumes)
        .filter(|(time, _)| clock::ist_day(**time) == today)
        .map(|(_, volume)| volume)
        .sum();
    let mut rng = Rng::new(id_number as u64 ^ clock::now_ms() as u64);

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        price *= f64::exp(0.0002 * rng.normal());
        volume += (rng.unit() * 40.0).round();

        let packet = full_packet(number, id_number, price, volume, clock::now_ms());

        if socket.send(Message::Binary(packet.into())).await.is_err() {
            return;
        }
    }
}

/// Starts the stand-in on two free ports on this machine, on a thread of
/// its own, and returns their addresses. It runs until the process ends.
pub fn start() -> Addresses {
    let (tell, told) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("the stand-in needs threads");

        runtime.block_on(async move {
            let api = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
            let feed = TcpListener::bind("127.0.0.1:0").await.expect("a free port");

            let addresses = Addresses {
                api: format!("http://{}/v2", api.local_addr().expect("bound")),
                feed: format!("ws://{}", feed.local_addr().expect("bound")),
            };

            let _ = tell.send(addresses);

            let shared = Arc::new(Mutex::new(Shared::default()));
            let feed_shared = shared.clone();

            tokio::spawn(async move {
                while let Ok((stream, _)) = feed.accept().await {
                    tokio::spawn(serve_feed(stream, feed_shared.clone()));
                }
            });

            while let Ok((stream, _)) = api.accept().await {
                tokio::spawn(serve_api(stream, shared.clone()));
            }
        });
    });

    told.recv().expect("the stand-in started")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_as_india_time() {
        assert_eq!(parse_date("2024-09-11 09:15:00"), Some(1_726_026_300_000));
        assert_eq!(
            parse_date("2024-09-11"),
            Some(1_726_026_300_000 - (9 * 60 + 15) * clock::MINUTE_MS)
        );
    }
}
