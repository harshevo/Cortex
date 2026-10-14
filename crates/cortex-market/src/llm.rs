//! A language model asked for the next ten candles: it is shown the
//! recent bars, the indicators and the levels (and the chart picture, if
//! one was loaded), and answers in JSON. The app supplies the model
//! through [`Oracle`], so the same provider the agent uses is used here.

use std::future::Future;
use std::pin::Pin;

use cortex_liquid::{Bar, PredictedCandle};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock;
use crate::plan::price_text;

/// Bars shown to the model.
const SHOWN: usize = 60;

pub type Answer = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

/// A picture to show the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// Such as `image/png`.
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Something that answers a prompt: the app's model provider.
pub trait Oracle: Send + Sync {
    /// The model's name, to show.
    fn name(&self) -> String;

    fn ask(&self, prompt: String, picture: Option<Picture>) -> Answer;
}

/// What the model said.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub candles: Vec<PredictedCandle>,

    /// `up`, `down` or `sideways`, as it said.
    pub bias: String,

    /// Its reason, in a sentence.
    pub reason: String,

    /// The highest and lowest prices it read off a picture's axis.
    pub axis: Option<(f64, f64)>,
}

/// What the model is shown.
pub struct Brief<'a> {
    pub symbol: &'a str,
    pub interval: &'a str,
    pub bars: &'a [Bar],
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,
    pub atr: Option<f64>,
    pub support: Option<f64>,
    pub resistance: Option<f64>,

    /// A picture of the chart is attached.
    pub with_picture: bool,

    /// The prices on the picture are not real.
    pub relative_prices: bool,
}

pub fn prompt(brief: &Brief) -> String {
    let mut text = String::new();

    text.push_str(
        "You forecast price candles. Reply with JSON only, no prose before or after, in this \
         shape:\n{\"bias\":\"up|down|sideways\",\"reason\":\"one short sentence\",\
         \"candles\":[{\"o\":0,\"h\":0,\"l\":0,\"c\":0,\"v\":0}],\"axis_high\":null,\
         \"axis_low\":null}\n\"candles\" must hold exactly the next 10 candles in order. \
         Keep high >= max(open, close) and low <= min(open, close). Volume 0 if unknown.\n\n",
    );

    text.push_str(&format!(
        "Instrument: {} · candles of {}\n",
        brief.symbol, brief.interval
    ));

    if brief.with_picture {
        text.push_str(
            "A picture of the chart is attached. Read the candles from it. Put the highest and \
             lowest prices printed on its price axis in axis_high and axis_low, or leave them \
             null if the axis cannot be read.\n",
        );
    }

    if brief.relative_prices {
        text.push_str(
            "The prices below were measured off the picture on a made-up scale from 100 to \
             110; forecast on the same scale.\n",
        );
    }

    let line = |name: &str, value: Option<f64>| {
        value
            .map(|value| format!("{name}: {}\n", price_text(value)))
            .unwrap_or_default()
    };

    text.push_str(&line("RSI(14)", brief.rsi));
    text.push_str(&line("VWAP", brief.vwap));
    text.push_str(&line("ATR(14)", brief.atr));
    text.push_str(&line("Nearest support", brief.support));
    text.push_str(&line("Nearest resistance", brief.resistance));

    text.push_str(
        "\nRecent candles, oldest first (time in India, open, high, low, close, volume):\n",
    );

    for bar in &brief.bars[brief.bars.len().saturating_sub(SHOWN)..] {
        text.push_str(&format!(
            "{} {} {} {} {} {:.0}\n",
            clock::moment_text(bar.time_ms),
            bar.open,
            bar.high,
            bar.low,
            bar.close,
            bar.volume
        ));
    }

    text
}

/// Reads the model's answer. Candles that break the rules are put right
/// rather than thrown away; an answer with no candles is refused.
pub fn read(answer: &str, last_close: f64) -> Result<Reading, String> {
    let begin = answer
        .find('{')
        .ok_or("the model's answer has no JSON in it")?;
    let end = answer
        .rfind('}')
        .ok_or("the model's answer has no JSON in it")?;

    let value: Value = serde_json::from_str(&answer[begin..=end])
        .map_err(|error| format!("the model's JSON cannot be read: {error}"))?;

    let number = |item: &Value, keys: &[&str]| {
        keys.iter()
            .find_map(|key| item.get(*key))
            .and_then(|found| match found {
                Value::Number(number) => number.as_f64(),
                Value::String(text) => text.replace(',', "").parse().ok(),
                _ => None,
            })
    };

    let items = value
        .get("candles")
        .and_then(Value::as_array)
        .ok_or("the model's answer has no candles")?;

    let mut candles = Vec::new();
    let mut before = last_close;

    for (at, item) in items.iter().take(cortex_liquid::STEPS).enumerate() {
        let Some(close) = number(item, &["c", "close"]).filter(|close| *close > 0.0) else {
            continue;
        };

        let open = number(item, &["o", "open"])
            .filter(|open| *open > 0.0)
            .unwrap_or(before);
        let high = number(item, &["h", "high"])
            .unwrap_or(open.max(close))
            .max(open.max(close));
        let low = number(item, &["l", "low"])
            .filter(|low| *low > 0.0)
            .unwrap_or(open.min(close))
            .min(open.min(close));

        candles.push(PredictedCandle {
            step: at + 1,
            open,
            high,
            low,
            close,
            volume: number(item, &["v", "volume"]).unwrap_or(0.0).max(0.0),
            close_low: low,
            close_high: high,
        });

        before = close;
    }

    if candles.is_empty() {
        return Err("the model gave no usable candles".to_string());
    }

    let axis = match (
        number(&value, &["axis_high"]),
        number(&value, &["axis_low"]),
    ) {
        (Some(high), Some(low)) if high > low && low > 0.0 => Some((high, low)),
        _ => None,
    };

    Ok(Reading {
        candles,
        bias: value
            .get("bias")
            .and_then(Value::as_str)
            .unwrap_or("sideways")
            .to_string(),
        reason: value
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        axis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_with_prose_around_it_is_read_and_put_right() {
        let answer = "Sure! ```json\n{\"bias\":\"up\",\"reason\":\"Higher lows.\",\"candles\":[\
            {\"o\":100,\"h\":99,\"l\":98,\"c\":101,\"v\":5},{\"open\":\"101\",\"close\":\"1,02.5\"},\
            {\"o\":102,\"c\":0}],\"axis_high\":120,\"axis_low\":95}\n```";

        let reading = read(answer, 100.0).unwrap();

        assert_eq!(reading.bias, "up");
        assert_eq!(reading.candles.len(), 2);

        // The high below the close is raised to it.
        assert_eq!(reading.candles[0].high, 101.0);
        assert_eq!(reading.candles[1].close, 102.5);
        assert_eq!(reading.axis, Some((120.0, 95.0)));
    }

    #[test]
    fn an_answer_without_candles_is_refused() {
        assert!(read("I cannot forecast prices.", 100.0).is_err());
        assert!(read("{\"bias\":\"up\"}", 100.0).is_err());
    }

    #[test]
    fn the_prompt_holds_the_bars_and_the_levels() {
        let bars = vec![Bar {
            time_ms: 1_726_026_300_000,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
        }];

        let text = prompt(&Brief {
            symbol: "RELIANCE",
            interval: "5m",
            bars: &bars,
            rsi: Some(61.0),
            vwap: None,
            atr: None,
            support: Some(1.2),
            resistance: None,
            with_picture: false,
            relative_prices: false,
        });

        assert!(text.contains("RELIANCE"));
        assert!(text.contains("RSI(14): 61.00"));
        assert!(text.contains("11 Sep 09:15 1 2 0.5 1.5 10"));
        assert!(!text.contains("VWAP"));
    }
}
