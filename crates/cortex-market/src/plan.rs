//! What to watch for: the range the price is in, what happens if it holds
//! and if it breaks either way, and what the chosen network expects, each
//! as one row with a level to act at, a target and a stop.
//!
//! These are rules of thumb applied to the levels and the forecast, not
//! advice. Every forecast they rest on is scored in the Outcomes and
//! Metrics tabs, and on most series those scores are near a coin's.

use serde::{Deserialize, Serialize};

use crate::indicators::Levels;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Up,
    Down,
    Neutral,
}

/// The numbers the plan reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    pub price: f64,
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,
    pub ema_fast: Option<f64>,
    pub ema_slow: Option<f64>,
    pub atr: Option<f64>,

    /// The chosen network's forecast close 1, 5 and 10 bars ahead, and
    /// its lowest low and highest high over the ten.
    pub close_1: Option<f64>,
    pub close_5: Option<f64>,
    pub close_10: Option<f64>,
    pub path_low: Option<f64>,
    pub path_high: Option<f64>,

    /// When the tenth bar ahead closes, as text.
    pub by: String,

    /// The network's short name, such as `LNN`.
    pub source: String,

    /// Its measured share of directions called right ten bars ahead.
    pub accuracy_10: Option<f64>,
}

/// The overall lean, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bias {
    pub tone: Tone,

    /// `Bullish`, `Bearish` or `Sideways`.
    pub label: String,

    /// From -5 to 5: each sign that points up adds, each down takes away.
    pub score: i32,
    pub reasons: Vec<String>,
}

/// One case and what to do in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub tone: Tone,

    /// The case: `Holds 2,410 – 2,450`.
    pub case: String,

    /// What the price is expected to do then.
    pub expect: String,

    /// What to do.
    pub action: String,

    pub entry: Option<f64>,
    pub target: Option<f64>,
    pub stop: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub bias: Bias,
    pub support: f64,
    pub resistance: f64,
    pub scenarios: Vec<Scenario>,
}

/// A price as people read it: grouped thousands, and as many decimals as
/// its size needs.
pub fn price_text(price: f64) -> String {
    let size = price.abs();
    let decimals = if size >= 1_000.0 {
        1
    } else if size >= 1.0 {
        2
    } else {
        4
    };

    let text = format!("{price:.decimals$}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let (sign, digits) = whole
        .strip_prefix('-')
        .map_or(("", whole), |rest| ("-", rest));

    let mut grouped = String::new();

    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            grouped.push(',');
        }

        grouped.push(digit);
    }

    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

pub fn plan(reading: &Reading, levels: &Levels) -> Plan {
    let price = reading.price;
    let atr = reading.atr.unwrap_or(price * 0.005).max(price * 1e-4);

    let support = levels
        .support()
        .filter(|level| price - level < atr * 6.0)
        .unwrap_or(price - atr * 2.0);
    let resistance = levels
        .resistance()
        .filter(|level| level - price < atr * 6.0)
        .unwrap_or(price + atr * 2.0);

    let width = (resistance - support).max(atr);

    let next_above = levels
        .resistances
        .get(1)
        .copied()
        .unwrap_or(resistance + width);
    let next_below = levels.supports.get(1).copied().unwrap_or(support - width);

    let bias = bias(reading, atr);
    let model_tone = reading.close_10.map(|close| {
        if close > price + atr * 0.3 {
            Tone::Up
        } else if close < price - atr * 0.3 {
            Tone::Down
        } else {
            Tone::Neutral
        }
    });

    let mut scenarios = Vec::new();

    // Holding the range.
    let (expect, action, entry, target, stop, tone) = match model_tone {
        Some(Tone::Up) => (
            format!("Drifts up toward {}", price_text(resistance)),
            "Buy dips near support, book near resistance".to_string(),
            Some(support + atr * 0.25),
            Some(resistance),
            Some(support - atr * 0.5),
            Tone::Up,
        ),
        Some(Tone::Down) => (
            format!("Drifts down toward {}", price_text(support)),
            "Sell rallies near resistance, cover near support".to_string(),
            Some(resistance - atr * 0.25),
            Some(support),
            Some(resistance + atr * 0.5),
            Tone::Down,
        ),
        _ => (
            "Chops between the two".to_string(),
            "Trade the edges or stay out".to_string(),
            None,
            None,
            None,
            Tone::Neutral,
        ),
    };

    scenarios.push(Scenario {
        tone,
        case: format!("Holds {} – {}", price_text(support), price_text(resistance)),
        expect,
        action,
        entry,
        target,
        stop,
    });

    scenarios.push(Scenario {
        tone: Tone::Up,
        case: format!("Breaks above {}", price_text(resistance)),
        expect: format!("Momentum toward {}", price_text(next_above)),
        action: "Buy a retest of the breakout that holds".to_string(),
        entry: Some(resistance),
        target: Some(next_above),
        stop: Some(resistance - atr * 0.5),
    });

    scenarios.push(Scenario {
        tone: Tone::Down,
        case: format!("Breaks below {}", price_text(support)),
        expect: format!("Slides toward {}", price_text(next_below)),
        action: "Exit longs; sell a failed retest".to_string(),
        entry: Some(support),
        target: Some(next_below),
        stop: Some(support + atr * 0.5),
    });

    if let (Some(close), Some(low), Some(high)) =
        (reading.close_10, reading.path_low, reading.path_high)
    {
        let change = (close / price - 1.0) * 100.0;

        let measured = reading
            .accuracy_10
            .map(|accuracy| format!(" · right {:.0}% held out", accuracy * 100.0))
            .unwrap_or_default();

        let (action, tone) = match model_tone {
            Some(Tone::Up) => (
                format!("Lean long while above {}", price_text(support)),
                Tone::Up,
            ),
            Some(Tone::Down) => (
                format!("Lean short while below {}", price_text(resistance)),
                Tone::Down,
            ),
            _ => ("No clear move: wait for a break".to_string(), Tone::Neutral),
        };

        scenarios.push(Scenario {
            tone,
            case: format!("{} path, next 10{measured}", reading.source),
            expect: format!(
                "{} ({change:+.2}%) by {}, range {} – {}",
                price_text(close),
                reading.by,
                price_text(low),
                price_text(high)
            ),
            action,
            entry: None,
            target: Some(close),
            stop: match tone {
                Tone::Up => Some(support),
                Tone::Down => Some(resistance),
                Tone::Neutral => None,
            },
        });
    }

    if let Some(vwap) = reading.vwap {
        let above = price >= vwap;

        scenarios.push(Scenario {
            tone: if above { Tone::Up } else { Tone::Down },
            case: format!(
                "{} VWAP {}",
                if above { "Above" } else { "Below" },
                price_text(vwap)
            ),
            expect: if above {
                "Buyers in control while it holds".to_string()
            } else {
                "Sellers in control while it stays below".to_string()
            },
            action: if above {
                "Longs defend pullbacks to VWAP".to_string()
            } else {
                "Shorts defend rallies to VWAP".to_string()
            },
            entry: Some(vwap),
            target: None,
            stop: None,
        });
    }

    if let Some(rsi) = reading.rsi
        && (rsi >= 70.0 || rsi <= 30.0)
    {
        {
            let high = rsi >= 70.0;
            let back_to = reading.ema_fast.unwrap_or(price);

            scenarios.push(Scenario {
                tone: if high { Tone::Down } else { Tone::Up },
                case: format!(
                    "{} (RSI {rsi:.0})",
                    if high { "Overbought" } else { "Oversold" }
                ),
                expect: format!("Often eases back toward {}", price_text(back_to)),
                action: if high {
                    "Avoid chasing; let it pull back".to_string()
                } else {
                    "Avoid fresh shorts; let it bounce".to_string()
                },
                entry: None,
                target: Some(back_to),
                stop: None,
            });
        }
    }

    Plan {
        bias,
        support,
        resistance,
        scenarios,
    }
}

fn bias(reading: &Reading, atr: f64) -> Bias {
    let price = reading.price;
    let mut score = 0;
    let mut reasons = Vec::new();

    if let Some(close) = reading.close_10 {
        if close > price + atr * 0.3 {
            score += 2;
            reasons.push(format!("{} sees it higher", reading.source));
        } else if close < price - atr * 0.3 {
            score -= 2;
            reasons.push(format!("{} sees it lower", reading.source));
        } else {
            reasons.push(format!("{} sees it flat", reading.source));
        }
    }

    if let Some(vwap) = reading.vwap {
        if price > vwap {
            score += 1;
            reasons.push("above VWAP".to_string());
        } else if price < vwap {
            score -= 1;
            reasons.push("below VWAP".to_string());
        }
    }

    if let (Some(fast), Some(slow)) = (reading.ema_fast, reading.ema_slow) {
        if fast > slow {
            score += 1;
            reasons.push("EMA 9 over 21".to_string());
        } else if fast < slow {
            score -= 1;
            reasons.push("EMA 9 under 21".to_string());
        }
    }

    if let Some(rsi) = reading.rsi {
        if rsi >= 55.0 {
            score += 1;
            reasons.push(format!("RSI {rsi:.0}"));
        } else if rsi <= 45.0 {
            score -= 1;
            reasons.push(format!("RSI {rsi:.0}"));
        }
    }

    let (tone, label) = if score >= 2 {
        (Tone::Up, "Bullish")
    } else if score <= -2 {
        (Tone::Down, "Bearish")
    } else {
        (Tone::Neutral, "Sideways")
    };

    Bias {
        tone,
        label: label.to_string(),
        score,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels() -> Levels {
        Levels {
            supports: vec![2_410.0, 2_380.0],
            resistances: vec![2_450.0, 2_480.0],
            ..Levels::default()
        }
    }

    fn reading(close_10: f64) -> Reading {
        Reading {
            price: 2_430.0,
            rsi: Some(60.0),
            vwap: Some(2_425.0),
            ema_fast: Some(2_428.0),
            ema_slow: Some(2_420.0),
            atr: Some(8.0),
            close_1: Some(2_431.0),
            close_5: Some(2_436.0),
            close_10: Some(close_10),
            path_low: Some(2_425.0),
            path_high: Some(2_446.0),
            by: "14:35".to_string(),
            source: "LNN".to_string(),
            accuracy_10: Some(0.54),
        }
    }

    #[test]
    fn an_upward_path_leans_long_and_names_both_breaks() {
        let plan = plan(&reading(2_445.0), &levels());

        assert_eq!(plan.bias.label, "Bullish");
        assert_eq!(plan.support, 2_410.0);
        assert_eq!(plan.resistance, 2_450.0);

        let cases: Vec<&str> = plan.scenarios.iter().map(|s| s.case.as_str()).collect();

        assert!(cases[0].starts_with("Holds 2,410.0 – 2,450.0"), "{cases:?}");
        assert!(cases[1].starts_with("Breaks above 2,450.0"));
        assert!(cases[2].starts_with("Breaks below 2,410.0"));
        assert_eq!(plan.scenarios[1].target, Some(2_480.0));
        assert_eq!(plan.scenarios[0].tone, Tone::Up);
        assert!(plan.scenarios[3].case.contains("LNN"));
    }

    #[test]
    fn a_downward_path_against_the_other_signs_is_sideways() {
        let plan = plan(&reading(2_400.0), &levels());

        assert_eq!(plan.bias.score, 1);
        assert_eq!(plan.bias.label, "Sideways");
        assert_eq!(plan.scenarios[0].tone, Tone::Down);
    }

    #[test]
    fn prices_are_grouped() {
        assert_eq!(price_text(2_430.5), "2,430.5");
        assert_eq!(price_text(24_315.25), "24,315.2");
        assert_eq!(price_text(182.356), "182.36");
        assert_eq!(price_text(1_234_567.0), "1,234,567.0");
        assert_eq!(price_text(-1_500.0), "-1,500.0");
    }
}
