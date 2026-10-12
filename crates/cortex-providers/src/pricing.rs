//! Cost estimates.
//!
//! Prices are list prices in US dollars per million tokens, recorded on
//! 2026-10-11. Cache prices that were not published alongside the model are
//! derived from the usual ratios (write 1.25x input, read 0.1x input), so
//! treat the result as an estimate and check your provider's invoice.

use cortex_core::Usage;

#[derive(Debug, Clone, Copy)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

pub fn pricing_for(model: &str) -> Option<Pricing> {
    let pricing = match model {
        "claude-opus-5-5" => Pricing {
            input: 4.00,
            output: 20.00,
            cache_read: 0.20,
            cache_write: 5.00,
        },

        "claude-sonnet-5-5" => Pricing {
            input: 2.00,
            output: 10.00,
            cache_read: 0.20,
            cache_write: 2.50,
        },

        "claude-haiku-5-5" => Pricing {
            input: 0.10,
            output: 0.50,
            cache_read: 0.01,
            cache_write: 0.125,
        },

        _ => return None,
    };

    Some(pricing)
}

/// Estimated cost in dollars, or `None` for a model with no known price.
pub fn cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    let pricing = pricing_for(model)?;

    let per_token = |tokens: u64, price_per_million: f64| tokens as f64 * price_per_million / 1e6;

    let cost = per_token(usage.input_tokens, pricing.input)
        + per_token(usage.output_tokens, pricing.output)
        + per_token(usage.cache_read_tokens, pricing.cache_read)
        + per_token(usage.cache_write_tokens, pricing.cache_write);

    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_adds_every_token_class() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 1_000_000,
        };

        let cost = cost_usd("claude-opus-5-5", &usage).unwrap();

        assert!((cost - 29.20).abs() < 1e-9);
    }

    #[test]
    fn unknown_models_have_no_price() {
        assert!(cost_usd("scripted", &Usage::default()).is_none());
    }
}
