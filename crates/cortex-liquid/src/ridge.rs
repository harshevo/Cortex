//! A linear read-out fitted by ridge regression: the weights that best
//! map a row of numbers to several targets at once, kept small by a
//! penalty so that a few hundred rows do not fit noise exactly.

use serde::{Deserialize, Serialize};

/// Weights from `inputs` numbers (and a constant) to `outputs` numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ridge {
    pub inputs: usize,
    pub outputs: usize,

    /// `(inputs + 1) × outputs`, a row per input, the constant last.
    weights: Vec<f64>,
}

impl Ridge {
    pub fn fit_masked(
        x: &[f32],
        y: &[f32],
        mask: &[bool],
        inputs: usize,
        outputs: usize,
        penalty: f64,
        use_row: impl Fn(usize) -> bool,
    ) -> Self {
        let mut weights = vec![0.; (inputs + 1) * outputs];
        for k in 0..outputs {
            let targets: Vec<_> = y.chunks_exact(outputs).map(|row| row[k]).collect();
            let head = Self::fit(x, &targets, inputs, 1, penalty, |at| {
                use_row(at) && mask.get(at * outputs + k) == Some(&true)
            });
            for i in 0..=inputs {
                weights[i * outputs + k] = head.weights[i];
            }
        }
        Self {
            inputs,
            outputs,
            weights,
        }
    }
    /// Fits on the rows whose index `use_row` accepts. `x` has `inputs`
    /// numbers per row and `y` `outputs` per row.
    pub fn fit(
        x: &[f32],
        y: &[f32],
        inputs: usize,
        outputs: usize,
        penalty: f64,
        use_row: impl Fn(usize) -> bool,
    ) -> Self {
        let width = inputs + 1;
        let rows = x.len().checked_div(inputs).unwrap_or_default();

        let mut gram = vec![0.0f64; width * width];
        let mut cross = vec![0.0f64; width * outputs];
        let mut row = vec![0.0f64; width];

        for at in (0..rows).filter(|at| use_row(*at)) {
            for (to, from) in row.iter_mut().zip(&x[at * inputs..(at + 1) * inputs]) {
                *to = f64::from(*from);
            }

            row[inputs] = 1.0;

            for i in 0..width {
                let xi = row[i];

                if xi == 0.0 {
                    continue;
                }

                for j in i..width {
                    gram[i * width + j] += xi * row[j];
                }

                for (k, target) in y[at * outputs..(at + 1) * outputs].iter().enumerate() {
                    cross[i * outputs + k] += xi * f64::from(*target);
                }
            }
        }

        for i in 0..width {
            for j in 0..i {
                gram[i * width + j] = gram[j * width + i];
            }

            // The constant is not held back.
            if i < inputs {
                gram[i * width + i] += penalty;
            } else {
                gram[i * width + i] += 1e-9;
            }
        }

        let weights = solve(&mut gram, &cross, width, outputs).unwrap_or_else(|| {
            // A singular system, as when there were no rows: predict 0.
            vec![0.0; width * outputs]
        });

        Self {
            inputs,
            outputs,
            weights,
        }
    }

    /// The outputs for one row.
    pub fn apply(&self, x: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f64; self.outputs];

        for (i, value) in x.iter().take(self.inputs).enumerate() {
            let value = f64::from(*value);

            if value == 0.0 {
                continue;
            }

            for (k, out) in out.iter_mut().enumerate() {
                *out += value * self.weights[i * self.outputs + k];
            }
        }

        for (k, out) in out.iter_mut().enumerate() {
            *out += self.weights[self.inputs * self.outputs + k];
        }

        out.into_iter().map(|value| value as f32).collect()
    }

    pub fn is_sound(&self) -> bool {
        self.weights.len() == (self.inputs + 1) * self.outputs
            && self.weights.iter().all(|w| w.is_finite())
    }
}

/// Solves `a · w = b` for a symmetric positive definite `a` by Cholesky.
fn solve(a: &mut [f64], b: &[f64], n: usize, m: usize) -> Option<Vec<f64>> {
    // a = L Lᵀ, L written over the lower triangle.
    for j in 0..n {
        let mut diagonal = a[j * n + j];

        for k in 0..j {
            diagonal -= a[j * n + k] * a[j * n + k];
        }

        if diagonal <= 0.0 || !diagonal.is_finite() {
            return None;
        }

        let diagonal = diagonal.sqrt();

        a[j * n + j] = diagonal;

        for i in j + 1..n {
            let mut sum = a[i * n + j];

            for k in 0..j {
                sum -= a[i * n + k] * a[j * n + k];
            }

            a[i * n + j] = sum / diagonal;
        }
    }

    let mut w = b.to_vec();

    for column in 0..m {
        // L z = b
        for i in 0..n {
            let mut sum = w[i * m + column];

            for k in 0..i {
                sum -= a[i * n + k] * w[k * m + column];
            }

            w[i * m + column] = sum / a[i * n + i];
        }

        // Lᵀ w = z
        for i in (0..n).rev() {
            let mut sum = w[i * m + column];

            for k in i + 1..n {
                sum -= a[k * n + i] * w[k * m + column];
            }

            w[i * m + column] = sum / a[i * n + i];
        }
    }

    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_line_is_found() {
        // y0 = 2 x0 - x1 + 3, y1 = x1
        let mut x = Vec::new();
        let mut y = Vec::new();

        for at in 0..50 {
            let (a, b) = (at as f32 * 0.1, (at % 7) as f32);

            x.extend([a, b]);
            y.extend([2.0 * a - b + 3.0, b]);
        }

        let ridge = Ridge::fit(&x, &y, 2, 2, 1e-6, |_| true);
        let out = ridge.apply(&[1.0, 2.0]);

        assert!((out[0] - 3.0).abs() < 1e-3, "{out:?}");
        assert!((out[1] - 2.0).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn no_rows_predicts_nothing() {
        let ridge = Ridge::fit(&[1.0, 2.0], &[5.0], 2, 1, 1.0, |_| false);

        assert!(ridge.is_sound());
        assert!(ridge.apply(&[1.0, 2.0])[0].abs() < 1e-6);
    }
}
