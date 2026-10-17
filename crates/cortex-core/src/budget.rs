//! How much one run may spend.
//!
//! A budget is a number of tokens, an amount of money, a length of time,
//! or any of them together. The run is
//! told when it has used [`WARN_AT`] of it, so it can finish the part
//! that matters and say what is left, and it is stopped when it has used
//! all of it. Money is counted at list price from the [`PriceTable`]; a
//! model without a known price is held to the token limit alone.

use std::time::Duration;

use crate::ledger::{CallCost, PriceTable};
use crate::usage::Usage;

/// The share of the budget at which the run is told to wrap up.
pub const WARN_AT: f64 = 0.8;

#[derive(Debug, Clone, Default)]
pub struct Budget {
    /// Tokens in and out, cached input included.
    pub max_tokens: Option<u64>,

    /// US dollars.
    pub max_cost: Option<f64>,

    /// Working time, not counting waits for approval.
    pub max_duration: Option<Duration>,

    pub prices: PriceTable,
}

/// What a run has spent so far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spent {
    pub tokens: u64,

    /// `None` once any call's price was unknown.
    pub cost: Option<f64>,

    pub elapsed: Duration,
}

impl Default for Spent {
    fn default() -> Self {
        Self::new()
    }
}

impl Spent {
    pub fn new() -> Self {
        Self {
            tokens: 0,
            cost: Some(0.0),
            elapsed: Duration::ZERO,
        }
    }

    /// Adds one model call.
    pub fn add(&mut self, prices: &PriceTable, provider: &str, model: &str, usage: &Usage) {
        self.tokens += usage.total_input() + usage.output_tokens;

        self.cost = match (self.cost, prices.cost(provider, model, usage)) {
            (Some(cost), CallCost::Usd(more)) => Some(cost + more),
            (Some(cost), CallCost::Subscription) => Some(cost),
            _ => None,
        };
    }
}

impl Budget {
    pub fn is_set(&self) -> bool {
        self.max_tokens.is_some() || self.max_cost.is_some() || self.max_duration.is_some()
    }

    /// The largest share of a limit that `spent` has used.
    pub fn share(&self, spent: &Spent) -> f64 {
        let tokens = self
            .max_tokens
            .filter(|max| *max > 0)
            .map(|max| spent.tokens as f64 / max as f64);

        let cost = match (self.max_cost, spent.cost) {
            (Some(max), Some(cost)) if max > 0.0 => Some(cost / max),
            _ => None,
        };

        let time = self
            .max_duration
            .filter(|max| !max.is_zero())
            .map(|max| spent.elapsed.as_secs_f64() / max.as_secs_f64());

        tokens
            .into_iter()
            .chain(cost)
            .chain(time)
            .fold(0.0, f64::max)
    }
}

/// What the model is told when most of the budget is gone.
pub fn warning(share: f64) -> String {
    format!(
        "You have used {:.0}% of this run's budget. Finish the most important part now, check \
         it, and end by saying what is done and what is left. Do not start anything new.",
        share * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tighter_limit_counts() {
        let budget = Budget {
            max_tokens: Some(1000),
            max_cost: Some(1.0),
            prices: PriceTable::built_in(),
            ..Budget::default()
        };

        let spent = Spent {
            tokens: 500,
            cost: Some(0.9),
            elapsed: Duration::ZERO,
        };

        assert!((budget.share(&spent) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn an_unknown_price_leaves_only_the_token_limit() {
        let mut spent = Spent::new();

        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Usage::default()
        };

        spent.add(&PriceTable::built_in(), "local", "qwen", &usage);

        assert_eq!(spent.tokens, 150);
        assert_eq!(spent.cost, None);

        let budget = Budget {
            max_cost: Some(0.01),
            ..Budget::default()
        };

        assert_eq!(budget.share(&spent), 0.0);
    }
}
