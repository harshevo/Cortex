//! Dhan's live market feed: a web socket that, once told which
//! instruments to send, sends a small binary packet on every trade.
//!
//! Every packet begins with eight bytes: what kind of packet it is, its
//! length, the exchange segment and the security ID, little-endian. What
//! follows depends on the kind; the layouts are Dhan's (version 2).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

use super::{Credentials, Instrument, Level};
use crate::clock;

const SUBSCRIBE_FULL: u8 = 21;
const SUBSCRIBE_TICKER: u8 = 15;
const DISCONNECT: u8 = 12;
pub(crate) fn connection_budget() -> std::sync::Arc<tokio::sync::Semaphore> {
    static BUDGET: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(5)))
        .clone()
}

/// One trade or change, as the feed sent it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Tick {
    pub segment: u8,
    pub security_id: u32,

    pub last_price: f64,

    /// When the trade was, as Dhan wrote it (seconds since 1970, which
    /// may be India's wall clock rather than UTC).
    pub trade_time: i64,

    pub last_quantity: Option<f64>,
    pub average_price: Option<f64>,

    /// Traded today, in all.
    pub volume: Option<f64>,
    pub buy_quantity: Option<f64>,
    pub sell_quantity: Option<f64>,
    pub open_interest: Option<f64>,

    pub day_open: Option<f64>,
    pub day_high: Option<f64>,
    pub day_low: Option<f64>,

    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

/// What a packet said.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Tick(Tick),
    PreviousClose {
        security_id: u32,
        close: f64,
    },

    /// Dhan is closing the connection, with one of its error codes.
    Disconnect(u16),
    Other,
}

fn f32_at(bytes: &[u8], at: usize) -> Option<f64> {
    Some(f64::from(f32::from_le_bytes(
        bytes.get(at..at + 4)?.try_into().ok()?,
    )))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i64> {
    Some(i64::from(i32::from_le_bytes(
        bytes.get(at..at + 4)?.try_into().ok()?,
    )))
}

fn i16_at(bytes: &[u8], at: usize) -> Option<i64> {
    Some(i64::from(i16::from_le_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

/// Reads one packet that begins at the start of `bytes`.
pub fn read_packet(bytes: &[u8]) -> Option<Packet> {
    let code = *bytes.first()?;
    let segment = *bytes.get(3)?;
    let security_id = u32::try_from(i32_at(bytes, 4)?).ok()?;

    let tick = |last_price: f64, trade_time: i64| Tick {
        segment,
        security_id,
        last_price,
        trade_time,
        ..Tick::default()
    };

    Some(match code {
        // Index and ticker packets: the price and when.
        1 | 2 => Packet::Tick(tick(f32_at(bytes, 8)?, i32_at(bytes, 12)?)),

        4 => {
            let mut tick = tick(f32_at(bytes, 8)?, i32_at(bytes, 14)?);

            tick.last_quantity = i16_at(bytes, 12).map(|q| q as f64);
            tick.average_price = f32_at(bytes, 18);
            tick.volume = i32_at(bytes, 22).map(|v| v as f64);
            tick.sell_quantity = i32_at(bytes, 26).map(|v| v as f64);
            tick.buy_quantity = i32_at(bytes, 30).map(|v| v as f64);
            tick.day_open = f32_at(bytes, 34);
            tick.day_high = f32_at(bytes, 42);
            tick.day_low = f32_at(bytes, 46);

            Packet::Tick(tick)
        }

        8 => {
            let mut tick = tick(f32_at(bytes, 8)?, i32_at(bytes, 14)?);

            tick.last_quantity = i16_at(bytes, 12).map(|q| q as f64);
            tick.average_price = f32_at(bytes, 18);
            tick.volume = i32_at(bytes, 22).map(|v| v as f64);
            tick.sell_quantity = i32_at(bytes, 26).map(|v| v as f64);
            tick.buy_quantity = i32_at(bytes, 30).map(|v| v as f64);
            tick.open_interest = i32_at(bytes, 34).map(|v| v as f64).filter(|oi| *oi > 0.0);
            tick.day_open = f32_at(bytes, 46);
            tick.day_high = f32_at(bytes, 54);
            tick.day_low = f32_at(bytes, 58);

            for level in 0..5 {
                let at = 62 + level * 20;

                let (Some(bid_quantity), Some(ask_quantity), Some(bid), Some(ask)) = (
                    i32_at(bytes, at),
                    i32_at(bytes, at + 4),
                    f32_at(bytes, at + 12),
                    f32_at(bytes, at + 16),
                ) else {
                    break;
                };

                tick.bids.push(Level {
                    price: bid,
                    quantity: bid_quantity as f64,
                    orders: i16_at(bytes, at + 8).unwrap_or(0) as u32,
                });
                tick.asks.push(Level {
                    price: ask,
                    quantity: ask_quantity as f64,
                    orders: i16_at(bytes, at + 10).unwrap_or(0) as u32,
                });
            }

            Packet::Tick(tick)
        }

        6 => Packet::PreviousClose {
            security_id,
            close: f32_at(bytes, 8)?,
        },

        50 => Packet::Disconnect(i16_at(bytes, 8).unwrap_or(0) as u16),

        _ => Packet::Other,
    })
}

/// Every packet in one message: Dhan may put several in one.
pub fn read_message(bytes: &[u8]) -> Vec<Packet> {
    let mut packets = Vec::new();
    let mut rest = bytes;

    while rest.len() >= 8 {
        let length = u16::from_le_bytes([rest[1], rest[2]]) as usize;

        // A length that does not fit is read as the whole message.
        let length = if length >= 8 && length <= rest.len() {
            length
        } else {
            rest.len()
        };

        if let Some(packet) = read_packet(&rest[..length]) {
            packets.push(packet);
        }

        rest = &rest[length..];
    }

    packets
}

/// How the feed is doing, for the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedNews {
    Connecting,
    Live,
    Packet(Packet),

    /// Lost; trying again in a moment.
    Dropped(String),

    /// Given up, with the reason.
    Stopped(String),
}

/// Watches `instrument` until `stop` turns true, telling `news` all that
/// happens. Reconnects after a drop, waiting longer each time, and gives
/// up when Dhan refuses the token or the plan.
pub async fn watch(
    credentials: Credentials,
    instrument: Instrument,
    mut stop: watch::Receiver<bool>,
    news: impl Fn(FeedNews) + Send + Sync,
) {
    let _permit = tokio::select! {permit=connection_budget().acquire_owned()=>permit.ok(),_=stop.changed()=>return};
    let mut wait = 2u64;

    loop {
        if *stop.borrow() {
            return;
        }

        news(FeedNews::Connecting);

        let address = format!(
            "{}/?version=2&token={}&clientId={}&authType=2",
            credentials.feed().trim_end_matches('/'),
            credentials.access_token,
            credentials.client_id
        );

        let outcome = tokio::select! {
            outcome = session(&address, &instrument, &news) => outcome,
            _ = stop.changed() => return,
        };

        match outcome {
            Ok(()) => {}
            Err(Ending::Refused(reason)) => {
                news(FeedNews::Stopped(reason));

                return;
            }
            Err(Ending::Dropped(reason)) => news(FeedNews::Dropped(reason)),
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            _ = stop.changed() => return,
        }

        wait = (wait * 2).min(30);
    }
}

enum Ending {
    Dropped(String),
    Refused(String),
}

async fn session(
    address: &str,
    instrument: &Instrument,
    news: &(impl Fn(FeedNews) + Send + Sync),
) -> Result<(), Ending> {
    let (mut socket, _) = tokio_tungstenite::connect_async(address)
        .await
        .map_err(|error| {
            let text = error.to_string();

            if text.contains("401") || text.contains("403") {
                Ending::Refused("Dhan refused the feed: the token may have expired.".to_string())
            } else {
                Ending::Dropped(format!("cannot reach Dhan's feed: {text}"))
            }
        })?;

    let code = if instrument.is_index() {
        SUBSCRIBE_TICKER
    } else {
        SUBSCRIBE_FULL
    };

    let subscribe = json!({
        "RequestCode": code,
        "InstrumentCount": 1,
        "InstrumentList": [{
            "ExchangeSegment": instrument.segment.code(),
            "SecurityId": instrument.security_id,
        }],
    });

    socket
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .map_err(|error| Ending::Dropped(error.to_string()))?;

    news(FeedNews::Live);

    while let Some(message) = socket.next().await {
        let message = message.map_err(|error| Ending::Dropped(error.to_string()))?;

        match message {
            Message::Binary(bytes) => {
                for packet in read_message(&bytes) {
                    if let Packet::Disconnect(code) = packet {
                        let _ = socket
                            .send(Message::Text(
                                json!({ "RequestCode": DISCONNECT }).to_string().into(),
                            ))
                            .await;

                        let reason = super::explain(&code.to_string())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("Dhan closed the feed (code {code})."));

                        return Err(if matches!(code, 805..=810) {
                            Ending::Refused(reason)
                        } else {
                            Ending::Dropped(reason)
                        });
                    }

                    news(FeedNews::Packet(packet));
                }
            }

            Message::Close(_) => return Err(Ending::Dropped("Dhan closed the feed.".to_string())),

            // Pings are answered by the library.
            _ => {}
        }
    }

    Err(Ending::Dropped("the feed ended.".to_string()))
}

/// The time of a trade in ms since 1970 UTC, from the feed's seconds,
/// which may be India's wall clock; falls back on `now_ms` when neither
/// reading is near it.
pub fn trade_time_ms(trade_time: i64, now_ms: i64) -> i64 {
    let utc = trade_time * 1_000;
    let wall = utc - clock::IST_MS;
    let near = |time: i64| (time - now_ms).abs() < 10 * clock::MINUTE_MS;

    if near(utc) {
        utc
    } else if near(wall) {
        wall
    } else {
        now_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(code: u8, length: u16, segment: u8, id: i32) -> Vec<u8> {
        let mut bytes = vec![code];

        bytes.extend(length.to_le_bytes());
        bytes.push(segment);
        bytes.extend(id.to_le_bytes());

        bytes
    }

    #[test]
    fn a_full_packet_is_read_with_its_depth() {
        let mut bytes = header(8, 162, 1, 2885);

        bytes.extend(2_950.5f32.to_le_bytes()); // 8 LTP
        bytes.extend(10i16.to_le_bytes()); // 12 LTQ
        bytes.extend(1_726_030_000i32.to_le_bytes()); // 14 LTT
        bytes.extend(2_940.25f32.to_le_bytes()); // 18 ATP
        bytes.extend(123_456i32.to_le_bytes()); // 22 volume
        bytes.extend(5_000i32.to_le_bytes()); // 26 sell
        bytes.extend(7_000i32.to_le_bytes()); // 30 buy
        bytes.extend(0i32.to_le_bytes()); // 34 OI
        bytes.extend(0i32.to_le_bytes()); // 38
        bytes.extend(0i32.to_le_bytes()); // 42
        bytes.extend(2_930.0f32.to_le_bytes()); // 46 open
        bytes.extend(0.0f32.to_le_bytes()); // 50 close
        bytes.extend(2_960.0f32.to_le_bytes()); // 54 high
        bytes.extend(2_925.0f32.to_le_bytes()); // 58 low

        for level in 0..5 {
            bytes.extend((100i32 + level).to_le_bytes()); // bid qty
            bytes.extend((200i32 + level).to_le_bytes()); // ask qty
            bytes.extend(3i16.to_le_bytes());
            bytes.extend(4i16.to_le_bytes());
            bytes.extend((2_950.0f32 - level as f32 * 0.05).to_le_bytes());
            bytes.extend((2_950.5f32 + level as f32 * 0.05).to_le_bytes());
        }

        assert_eq!(bytes.len(), 162);

        let packets = read_message(&bytes);

        let [Packet::Tick(tick)] = packets.as_slice() else {
            panic!("{packets:?}");
        };

        assert_eq!(tick.security_id, 2885);
        assert_eq!(tick.last_price, 2_950.5);
        assert_eq!(tick.average_price, Some(2_940.25));
        assert_eq!(tick.volume, Some(123_456.0));
        assert_eq!(tick.day_high, Some(2_960.0));
        assert_eq!(tick.bids.len(), 5);
        assert_eq!(tick.bids[0].quantity, 100.0);
        assert_eq!(tick.asks[4].quantity, 204.0);
        assert_eq!(tick.asks[0].orders, 4);
    }

    #[test]
    fn several_packets_in_one_message_are_all_read() {
        let mut bytes = header(2, 16, 0, 13);

        bytes.extend(25_000.5f32.to_le_bytes());
        bytes.extend(1_726_030_000i32.to_le_bytes());

        let mut close = header(6, 16, 0, 13);

        close.extend(24_900.0f32.to_le_bytes());
        close.extend(0i32.to_le_bytes());

        bytes.extend(close);

        let mut gone = header(50, 10, 0, 0);

        gone.extend(807i16.to_le_bytes());
        bytes.extend(gone);

        let packets = read_message(&bytes);

        assert_eq!(packets.len(), 3);
        assert!(matches!(packets[1], Packet::PreviousClose { close, .. } if close == 24_900.0));
        assert_eq!(packets[2], Packet::Disconnect(807));
    }

    #[test]
    fn trade_times_in_either_clock_are_put_on_utc() {
        let now = 1_726_030_000_000;

        assert_eq!(trade_time_ms(1_726_030_000 - 5, now), now - 5_000);
        assert_eq!(
            trade_time_ms(1_726_030_000 + clock::IST_MS / 1_000, now),
            now
        );
        assert_eq!(trade_time_ms(0, now), now);
    }
}
