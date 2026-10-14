//! How numbers are written on the trading screen.

pub use cortex_market::price_text as price;

/// A count written short: `950`, `12.4K`, `3.1M`, `2.0Cr`-free, in the
/// international way.
pub fn quantity(value: f64) -> String {
    let size = value.abs();

    if size >= 1e9 {
        format!("{:.2}B", value / 1e9)
    } else if size >= 1e6 {
        format!("{:.2}M", value / 1e6)
    } else if size >= 1e4 {
        format!("{:.1}K", value / 1e3)
    } else {
        format!("{value:.0}")
    }
}

pub fn percent(value: f64) -> String {
    format!("{value:+.2}%")
}

pub fn share(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

pub fn maybe(value: Option<f64>, write: impl Fn(f64) -> String) -> String {
    value.map(write).unwrap_or_else(|| "—".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_are_short() {
        assert_eq!(quantity(950.0), "950");
        assert_eq!(quantity(12_400.0), "12.4K");
        assert_eq!(quantity(3_140_000.0), "3.14M");
        assert_eq!(maybe(None, share), "—");
    }
}
