use super::dhan::Level;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSample {
    pub time_ms: i64,
    pub imbalance: Option<f64>,
    pub spread: Option<f64>,
    pub microprice: Option<f64>,
    pub volume: Option<f64>,
    pub volume_delta: Option<f64>,
    pub bid_liquidity: f64,
    pub ask_liquidity: f64,
    pub displayed_change: Option<f64>,
    pub available: bool,
}

pub fn sample(
    book: &DepthSnapshot,
    volume: Option<f64>,
    previous: Option<&FlowSample>,
    now: i64,
) -> FlowSample {
    let flow = book.features(now);
    FlowSample {
        time_ms: now,
        imbalance: flow.imbalance,
        spread: flow.spread,
        microprice: flow.microprice,
        volume,
        volume_delta: previous
            .and_then(|p| p.volume.zip(volume))
            .and_then(|(before, now)| (now >= before).then_some(now - before)),
        bid_liquidity: flow.bid_liquidity,
        ask_liquidity: flow.ask_liquidity,
        available: flow.available,
        displayed_change: previous.filter(|p| p.available && flow.available).map(|p| {
            (flow.bid_liquidity - flow.ask_liquidity) - (p.bid_liquidity - p.ask_liquidity)
        }),
    }
}
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepthSnapshot {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub bid_ms: i64,
    pub ask_ms: i64,
    pub coverage: usize,
    pub status: String,
}

impl DepthSnapshot {
    pub fn fresh(&self, now: i64) -> bool {
        self.bid_ms > 0
            && self.ask_ms > 0
            && now >= self.bid_ms.max(self.ask_ms)
            && now - self.bid_ms.min(self.ask_ms) <= 5_000
            && !self.bids.is_empty()
            && !self.asks.is_empty()
            && self.bids[0].price <= self.asks[0].price
    }

    pub fn features(&self, now: i64) -> FlowFeatures {
        if !self.fresh(now) {
            return FlowFeatures::default();
        }
        let bid = self.bids[0];
        let ask = self.asks[0];
        let buy: f64 = self.bids.iter().map(|l| l.quantity).sum();
        let sell: f64 = self.asks.iter().map(|l| l.quantity).sum();
        let best = bid.quantity + ask.quantity;
        FlowFeatures {
            available: true,
            made_ms: self.bid_ms.min(self.ask_ms),
            levels: self.coverage,
            spread: Some(ask.price - bid.price),
            imbalance: (buy + sell > 0.0).then_some((buy - sell) / (buy + sell).max(1.0)),
            microprice: (best > 0.0)
                .then_some((ask.price * bid.quantity + bid.price * ask.quantity) / best.max(1.0)),
            bid_liquidity: buy,
            ask_liquidity: sell,
        }
    }

    pub fn set_side(
        &mut self,
        bid: bool,
        mut levels: Vec<Level>,
        now: i64,
        coverage: usize,
    ) -> bool {
        if now < if bid { self.bid_ms } else { self.ask_ms } {
            return false;
        }
        levels.retain(|l| {
            l.price.is_finite() && l.price > 0.0 && l.quantity.is_finite() && l.quantity >= 0.0
        });
        levels.sort_by(|a, b| {
            if bid {
                b.price.total_cmp(&a.price)
            } else {
                a.price.total_cmp(&b.price)
            }
        });
        levels.dedup_by(|a, b| a.price == b.price);
        levels.truncate(coverage.min(200));
        if bid {
            self.bids = levels;
            self.bid_ms = now;
        } else {
            self.asks = levels;
            self.ask_ms = now;
        }
        self.coverage = coverage;
        true
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowFeatures {
    pub available: bool,
    pub made_ms: i64,
    pub levels: usize,
    pub spread: Option<f64>,
    pub imbalance: Option<f64>,
    pub microprice: Option<f64>,
    pub bid_liquidity: f64,
    pub ask_liquidity: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_sides_require_fresh_uncrossed_book() {
        let mut book = DepthSnapshot::default();
        book.set_side(
            true,
            vec![Level {
                price: 99.,
                quantity: 3.,
                orders: 1,
            }],
            1000,
            20,
        );
        assert!(!book.features(1000).available);
        book.set_side(
            false,
            vec![Level {
                price: 101.,
                quantity: 1.,
                orders: 1,
            }],
            1100,
            20,
        );
        let features = book.features(1200);
        assert_eq!(features.imbalance, Some(0.5));
        assert_eq!(features.microprice, Some(100.5));
        assert!(!book.features(7000).available);
        book.set_side(
            false,
            vec![Level {
                price: 98.,
                quantity: 1.,
                orders: 1,
            }],
            1200,
            20,
        );
        assert!(!book.features(1200).available);
    }
}
