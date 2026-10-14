//! The few statistics needed to say whether a share of right calls is
//! more than chance would give.

/// The natural logarithm of the gamma function, by Lanczos' series.
/// Good to about ten digits for positive arguments.
fn ln_gamma(x: f64) -> f64 {
    const SERIES: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    let x = x - 1.0;
    let base = x + 7.5;

    let sum = SERIES[1..]
        .iter()
        .enumerate()
        .fold(SERIES[0], |sum, (at, term)| {
            sum + term / (x + at as f64 + 1.0)
        });

    0.5 * std::f64::consts::TAU.ln() + (x + 0.5) * base.ln() - base + sum.ln()
}

fn ln_choose(of: u64, take: u64) -> f64 {
    ln_gamma(of as f64 + 1.0) - ln_gamma(take as f64 + 1.0) - ln_gamma((of - take) as f64 + 1.0)
}

/// The chance that a fair coin thrown `of` times lands at least as far
/// from half heads as `right` is, on either side. 1 when `of` is 0.
pub fn binomial_two_sided(right: u64, of: u64) -> f64 {
    if of == 0 {
        return 1.0;
    }

    let far = right.min(of).max(of - right.min(of));
    let fair = of as f64 * std::f64::consts::LN_2;

    let tail: f64 = (far..=of).map(|at| (ln_choose(of, at) - fair).exp()).sum();

    (2.0 * tail).min(1.0)
}

/// Wilson's interval for a share: where the true share plausibly lies
/// given `right` out of `of`, `z` standard deviations wide (1.96 for
/// 95%, 2.576 for 99%, 3.291 for 99.9%).
pub fn wilson(right: u64, of: u64, z: f64) -> (f64, f64) {
    if of == 0 {
        return (0.0, 1.0);
    }

    let count = of as f64;
    let share = right as f64 / count;
    let square = z * z;

    let middle = (share + square / (2.0 * count)) / (1.0 + square / count);
    let half = z * (share * (1.0 - share) / count + square / (4.0 * count * count)).sqrt()
        / (1.0 + square / count);

    ((middle - half).max(0.0), (middle + half).min(1.0))
}

/// The value below which `share` of `values` lie, reading between the two
/// nearest. `values` need not be sorted. 0 when there are none.
pub fn quantile(values: &[f64], share: f64) -> f64 {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();

    if sorted.is_empty() {
        return 0.0;
    }

    sorted.sort_by(f64::total_cmp);

    let place = share.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let below = place.floor() as usize;
    let above = place.ceil() as usize;

    sorted[below] + (sorted[above] - sorted[below]) * (place - below as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binomial_matches_values_worked_by_hand() {
        // 10 throws, 8 heads: 2 * (45 + 10 + 1) / 1024.
        assert!((binomial_two_sided(8, 10) - 112.0 / 1024.0).abs() < 1e-9);

        // The same on the other side.
        assert!((binomial_two_sided(2, 10) - 112.0 / 1024.0).abs() < 1e-9);

        assert_eq!(binomial_two_sided(5, 10), 1.0);
        assert_eq!(binomial_two_sided(0, 0), 1.0);

        // 20 of 20 is 2 / 2^20.
        assert!((binomial_two_sided(20, 20) - 2.0 / 1_048_576.0).abs() < 1e-12);
    }

    #[test]
    fn binomial_agrees_with_the_normal_curve_for_many_throws() {
        // 5,200 of 10,000 is 4 standard deviations out: about 6.3e-5.
        let p = binomial_two_sided(5_200, 10_000);

        assert!(p > 5.0e-5 && p < 8.0e-5, "{p}");
    }

    #[test]
    fn wilson_holds_the_share_and_narrows_with_more() {
        let (low, high) = wilson(60, 100, 1.96);

        assert!(low < 0.6 && high > 0.6);
        assert!((low - 0.502).abs() < 0.003 && (high - 0.691).abs() < 0.003);

        let (lower, higher) = wilson(600, 1_000, 1.96);

        assert!(higher - lower < high - low);
        assert_eq!(wilson(0, 0, 1.96), (0.0, 1.0));
    }

    #[test]
    fn quantile_reads_between_values() {
        let values = [4.0, 1.0, 3.0, 2.0];

        assert_eq!(quantile(&values, 0.0), 1.0);
        assert_eq!(quantile(&values, 1.0), 4.0);
        assert_eq!(quantile(&values, 0.5), 2.5);
        assert_eq!(quantile(&[], 0.5), 0.0);
    }
}
