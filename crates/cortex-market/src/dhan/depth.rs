use super::{Credentials, Instrument, Level};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DepthMode {
    #[default]
    Five,
    Twenty,
    TwoHundred,
}
impl DepthMode {
    pub fn levels(self) -> usize {
        match self {
            Self::Five => 5,
            Self::Twenty => 20,
            Self::TwoHundred => 200,
        }
    }
    fn endpoint(self) -> &'static str {
        match self {
            Self::TwoHundred => "wss://full-depth-api.dhan.co/twohundreddepth",
            _ => "wss://depth-api-feed.dhan.co/twentydepth",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DepthPacket {
    pub segment: u8,
    pub security_id: u32,
    pub bid: bool,
    pub levels: Vec<Level>,
}

pub fn read(bytes: &[u8], mode: DepthMode) -> Result<Vec<DepthPacket>, String> {
    let mut packets = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let h = bytes.get(at..at + 12).ok_or("Truncated depth header")?;
        let length = u16::from_le_bytes([h[0], h[1]]) as usize;
        if length < 12 {
            return Err("Invalid depth packet length".into());
        }
        let frame = bytes
            .get(at..at + length)
            .ok_or("Truncated depth payload")?;
        if h[2] == 50 {
            return Err("Depth subscription refused or disconnected".into());
        }
        if !matches!(h[2], 41 | 51) {
            return Err("Unknown depth packet code".into());
        }
        let count = (length - 12) / 16;
        if !(length - 12).is_multiple_of(16) || count > mode.levels() {
            return Err("Invalid depth row count".into());
        }
        if mode == DepthMode::TwoHundred
            && u32::from_le_bytes(h[8..12].try_into().unwrap()) as usize != count
        {
            return Err("Depth row count does not match payload".into());
        }
        let mut levels = Vec::with_capacity(count);
        for row in frame[12..].chunks_exact(16) {
            let price = f64::from_le_bytes(row[..8].try_into().unwrap());
            let quantity = u32::from_le_bytes(row[8..12].try_into().unwrap()) as f64;
            if !price.is_finite() || price < 0. {
                return Err("Invalid depth price".into());
            }
            if price > 0. {
                levels.push(Level {
                    price,
                    quantity,
                    orders: u32::from_le_bytes(row[12..16].try_into().unwrap()),
                });
            }
        }
        packets.push(DepthPacket {
            segment: h[3],
            security_id: u32::from_le_bytes(h[4..8].try_into().unwrap()),
            bid: h[2] == 41,
            levels,
        });
        at += length;
    }
    Ok(packets)
}

#[derive(Debug, Clone)]
pub enum News {
    Connecting,
    Packet(DepthPacket),
    Dropped,
    Stopped(String),
}

pub async fn watch(
    credentials: Credentials,
    instrument: Instrument,
    mode: DepthMode,
    mut stop: watch::Receiver<bool>,
    news: impl Fn(News) + Send + Sync,
) {
    let _permit = tokio::select! {permit=super::feed::connection_budget().acquire_owned()=>permit.ok(),_=stop.changed()=>return};
    let mut wait = 2;
    loop {
        if *stop.borrow() {
            return;
        }
        news(News::Connecting);
        let result = tokio::select! {
            result=session(&credentials,&instrument,mode,&news)=>result,
            _=stop.changed()=>return,
        };
        match result {
            Err(reason) if reason.contains("refused") => {
                news(News::Stopped(reason));
                return;
            }
            _ => news(News::Dropped),
        }
        tokio::select! { _=tokio::time::sleep(Duration::from_secs(wait))=>{}, _=stop.changed()=>return }
        wait = (wait * 2).min(30);
    }
}

async fn session(
    credentials: &Credentials,
    instrument: &Instrument,
    mode: DepthMode,
    news: &(impl Fn(News) + Send + Sync),
) -> Result<(), String> {
    let endpoint = std::env::var("CORTEX_DHAN_DEPTH").unwrap_or_else(|_| mode.endpoint().into());
    let mut url = reqwest::Url::parse(&endpoint).map_err(|_| "Invalid depth endpoint")?;
    url.query_pairs_mut()
        .append_pair("token", &credentials.access_token)
        .append_pair("clientId", &credentials.client_id)
        .append_pair("authType", "2");
    let (mut socket,_)=tokio_tungstenite::connect_async(url.as_str()).await.map_err(|error| {
        if matches!(error,tokio_tungstenite::tungstenite::Error::Http(ref r) if r.status().as_u16()==401 || r.status().as_u16()==403) {"Depth subscription refused".to_string()} else {"Depth connection unavailable".to_string()}
    })?;
    let request = if mode == DepthMode::TwoHundred {
        json!({"RequestCode":23,"ExchangeSegment":instrument.segment.code(),"SecurityId":instrument.security_id})
    } else {
        json!({"RequestCode":23,"InstrumentCount":1,"InstrumentList":[{"ExchangeSegment":instrument.segment.code(),"SecurityId":instrument.security_id}]})
    };
    socket
        .send(Message::Text(request.to_string().into()))
        .await
        .map_err(|_| "Depth subscription failed")?;
    loop {
        let packet = tokio::time::timeout(Duration::from_secs(35), socket.next())
            .await
            .map_err(|_| "Depth heartbeat timed out")?
            .ok_or("Depth connection closed")?
            .map_err(|_| "Depth connection interrupted")?;
        match packet {
            Message::Binary(bytes) => {
                for packet in read(&bytes, mode)? {
                    news(News::Packet(packet));
                }
            }
            Message::Ping(bytes) => socket
                .send(Message::Pong(bytes))
                .await
                .map_err(|_| "Depth heartbeat failed")?,
            Message::Close(_) => return Err("Depth connection closed".into()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(code: u8, price: f64) -> Vec<u8> {
        let mut bytes = vec![0; 28];
        bytes[..2].copy_from_slice(&28u16.to_le_bytes());
        bytes[2] = code;
        bytes[3] = 1;
        bytes[4..8].copy_from_slice(&13u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..20].copy_from_slice(&price.to_le_bytes());
        bytes[20..24].copy_from_slice(&5u32.to_le_bytes());
        bytes
    }
    #[test]
    fn concatenated_sides_and_invalid_packets() {
        let mut bytes = packet(41, 99.);
        bytes.extend(packet(51, 101.));
        let sides = read(&bytes, DepthMode::Twenty).unwrap();
        assert_eq!(sides.len(), 2);
        assert!(sides[0].bid);
        assert!(!sides[1].bid);
        assert!(read(&bytes[..bytes.len() - 1], DepthMode::Twenty).is_err());
        assert!(read(&packet(41, f64::NAN), DepthMode::Twenty).is_err());
        let mut wrong = packet(41, 99.);
        wrong[8] = 2;
        assert!(read(&wrong, DepthMode::TwoHundred).is_err());
    }
}
