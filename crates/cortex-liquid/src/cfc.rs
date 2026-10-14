//! The liquid network: one closed-form continuous-time (CfC) recurrent
//! cell with a linear read-out, and the code that trains it.
//!
//! At each sample the cell is given the input, its own state, and how
//! long it has been since the last sample:
//!
//! ```text
//! z      = tanh(Wb · [input, state] + bb)        the backbone
//! f      = Wf · z + bf                           how fast each unit moves
//! g, h   = tanh(Wg · z + bg), tanh(Wh · z + bh)  where it starts and ends
//! s      = sigmoid(-f · gap)
//! state' = s ⊙ g + (1 - s) ⊙ h
//! out    = Wo · state' + bo
//! ```
//!
//! `gap` is real elapsed time. A unit with a positive `f` has moved
//! further from `g` toward `h` after a long gap than after a short one,
//! so a weekend is not treated as one ordinary bar. That is all "liquid"
//! means here: the closed-form solution of Hasani et al. (2022), with no
//! differential-equation solver.
//!
//! Training is ordinary backpropagation through time, written out by
//! hand, over windows of a fixed length, with Adam and gradient clipping.
//! The forward and backward passes are written once over [`Real`] and
//! used at `f32` for training and at `f64` by the test that checks the
//! gradients against finite differences.
//!
//! Weights are one flat vector. Each matrix is stored a source column at
//! a time, so the inner loops are "add a multiple of one slice to
//! another", which the compiler turns into vector instructions.
//!
//! What it cannot do: it is one small layer on one processor core. It
//! finds what a few dozen units can find in a few thousand samples and
//! no more.

use std::fmt::Debug;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// The arithmetic the passes need, so that they can run at either width.
pub trait Real:
    Copy
    + Debug
    + Default
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + AddAssign
{
    fn of(value: f64) -> Self;
    fn wide(self) -> f64;
    fn tanh(self) -> Self;
    fn exp(self) -> Self;
    fn ln(self) -> Self;
}

impl Real for f32 {
    fn of(value: f64) -> Self {
        value as f32
    }

    fn wide(self) -> f64 {
        self as f64
    }

    fn tanh(self) -> Self {
        f32::tanh(self)
    }

    fn exp(self) -> Self {
        f32::exp(self)
    }

    fn ln(self) -> Self {
        f32::ln(self)
    }
}

impl Real for f64 {
    fn of(value: f64) -> Self {
        value
    }

    fn wide(self) -> f64 {
        self
    }

    fn tanh(self) -> Self {
        f64::tanh(self)
    }

    fn exp(self) -> Self {
        f64::exp(self)
    }

    fn ln(self) -> Self {
        f64::ln(self)
    }
}

fn sigmoid<R: Real>(x: R) -> R {
    R::of(1.0) / (R::of(1.0) + (-x).exp())
}

/// What one output is trained to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Head {
    /// A number, judged by half the squared error.
    Number,

    /// The log-odds of a yes, judged by cross-entropy against a target of
    /// 0 or 1.
    Odds,
}

/// The sizes of a network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    pub inputs: usize,

    /// Units of state.
    pub units: usize,

    /// Units in the backbone layer.
    pub backbone: usize,

    pub heads: Vec<Head>,
}

/// Where each matrix and bias begins in the flat vector of weights.
#[derive(Debug, Clone, Copy)]
struct Layout {
    backbone: usize,
    backbone_bias: usize,
    rate: usize,
    rate_bias: usize,
    from: usize,
    from_bias: usize,
    to: usize,
    to_bias: usize,
    out: usize,
    out_bias: usize,
    total: usize,
}

impl Shape {
    fn layout(&self) -> Layout {
        let seen = self.inputs + self.units;
        let head = self.backbone * self.units;

        let backbone = 0;
        let backbone_bias = backbone + seen * self.backbone;
        let rate = backbone_bias + self.backbone;
        let rate_bias = rate + head;
        let from = rate_bias + self.units;
        let from_bias = from + head;
        let to = from_bias + self.units;
        let to_bias = to + head;
        let out = to_bias + self.units;
        let out_bias = out + self.units * self.heads.len();
        let total = out_bias + self.heads.len();

        Layout {
            backbone,
            backbone_bias,
            rate,
            rate_bias,
            from,
            from_bias,
            to,
            to_bias,
            out,
            out_bias,
            total,
        }
    }

    /// How many weights a network of this shape has.
    pub fn weights(&self) -> usize {
        self.layout().total
    }
}

/// `y += W · x`, with `W` stored a column at a time.
fn add_product<R: Real>(y: &mut [R], matrix: &[R], x: &[R]) {
    for (column, x) in matrix.chunks_exact(y.len()).zip(x) {
        for (y, weight) in y.iter_mut().zip(column) {
            *y += *weight * *x;
        }
    }
}

/// `dW += dy ⊗ x`: what `add_product` owes each weight.
fn add_outer<R: Real>(gradient: &mut [R], dy: &[R], x: &[R]) {
    for (column, x) in gradient.chunks_exact_mut(dy.len()).zip(x) {
        for (gradient, dy) in column.iter_mut().zip(dy) {
            *gradient += *dy * *x;
        }
    }
}

/// `dx += Wᵀ · dy`: what `add_product` owes each input.
fn add_transposed<R: Real>(dx: &mut [R], matrix: &[R], dy: &[R]) {
    for (column, dx) in matrix.chunks_exact(dy.len()).zip(dx) {
        *dx += dot(column, dy);
    }
}

/// A sum of products kept in eight running parts, which is what lets the
/// compiler use vector instructions for it.
fn dot<R: Real>(left: &[R], right: &[R]) -> R {
    let mut parts = [R::default(); 8];

    let (lefts, left_over) = left.as_chunks::<8>();
    let (rights, right_over) = right.as_chunks::<8>();

    let mut sum = R::default();

    for (left, right) in left_over.iter().zip(right_over) {
        sum += *left * *right;
    }

    for (left, right) in lefts.iter().zip(rights) {
        for ((part, left), right) in parts.iter_mut().zip(left).zip(right) {
            *part += *left * *right;
        }
    }

    for part in parts {
        sum += part;
    }

    sum
}

fn add_to<R: Real>(sum: &mut [R], more: &[R]) {
    for (sum, more) in sum.iter_mut().zip(more) {
        *sum += *more;
    }
}

/// Everything the forward pass worked out over a run of samples, kept
/// for the backward pass. Each field holds one row per sample.
#[derive(Debug, Default)]
struct Tape<R> {
    samples: usize,

    /// The input followed by the state it met.
    seen: Vec<R>,
    backbone: Vec<R>,
    gate: Vec<R>,
    from: Vec<R>,
    to: Vec<R>,
    state: Vec<R>,
    out: Vec<R>,
    gap: Vec<R>,
}

/// Runs the cell over `gaps.len()` samples from the state `start`.
fn forward<R: Real>(
    shape: &Shape,
    weights: &[R],
    inputs: &[R],
    gaps: &[R],
    start: &[R],
    tape: &mut Tape<R>,
) {
    let layout = shape.layout();
    let (ins, units, wide, outs) = (shape.inputs, shape.units, shape.backbone, shape.heads.len());
    let seen = ins + units;
    let samples = gaps.len();

    tape.samples = samples;
    tape.gap.clear();
    tape.gap.extend_from_slice(gaps);

    for (rows, width) in [
        (&mut tape.seen, seen),
        (&mut tape.backbone, wide),
        (&mut tape.gate, units),
        (&mut tape.from, units),
        (&mut tape.to, units),
        (&mut tape.state, units),
        (&mut tape.out, outs),
    ] {
        rows.clear();
        rows.resize(samples * width, R::default());
    }

    let mut rate = vec![R::default(); units];

    for at in 0..samples {
        let seen_now = &mut tape.seen[at * seen..(at + 1) * seen];

        seen_now[..ins].copy_from_slice(&inputs[at * ins..(at + 1) * ins]);

        if at == 0 {
            seen_now[ins..].copy_from_slice(start);
        } else {
            seen_now[ins..].copy_from_slice(&tape.state[(at - 1) * units..at * units]);
        }

        let seen_now = &tape.seen[at * seen..(at + 1) * seen];
        let backbone = &mut tape.backbone[at * wide..(at + 1) * wide];

        backbone.copy_from_slice(&weights[layout.backbone_bias..layout.backbone_bias + wide]);
        add_product(
            backbone,
            &weights[layout.backbone..layout.backbone_bias],
            seen_now,
        );

        for value in backbone.iter_mut() {
            *value = value.tanh();
        }

        let backbone = &tape.backbone[at * wide..(at + 1) * wide];
        let from = &mut tape.from[at * units..(at + 1) * units];
        let to = &mut tape.to[at * units..(at + 1) * units];

        rate.copy_from_slice(&weights[layout.rate_bias..layout.rate_bias + units]);
        add_product(&mut rate, &weights[layout.rate..layout.rate_bias], backbone);

        from.copy_from_slice(&weights[layout.from_bias..layout.from_bias + units]);
        add_product(from, &weights[layout.from..layout.from_bias], backbone);

        to.copy_from_slice(&weights[layout.to_bias..layout.to_bias + units]);
        add_product(to, &weights[layout.to..layout.to_bias], backbone);

        let gate = &mut tape.gate[at * units..(at + 1) * units];
        let state = &mut tape.state[at * units..(at + 1) * units];
        let gap = gaps[at];

        for unit in 0..units {
            gate[unit] = sigmoid(-rate[unit] * gap);
            from[unit] = from[unit].tanh();
            to[unit] = to[unit].tanh();
            state[unit] = gate[unit] * from[unit] + (R::of(1.0) - gate[unit]) * to[unit];
        }

        let state = &tape.state[at * units..(at + 1) * units];
        let out = &mut tape.out[at * outs..(at + 1) * outs];

        out.copy_from_slice(&weights[layout.out_bias..layout.out_bias + outs]);
        add_product(out, &weights[layout.out..layout.out_bias], state);
    }
}

/// How wrong one output is, and the slope of that with respect to it.
fn judge<R: Real>(head: Head, out: R, target: R) -> (R, R) {
    match head {
        Head::Number => {
            let off = out - target;

            (R::of(0.5) * off * off, off)
        }

        Head::Odds => {
            // log(1 + e^out) - target · out, written so that a large
            // `out` of either sign does not overflow.
            let size = if out < R::default() { -out } else { out };
            let positive = if out < R::default() {
                R::default()
            } else {
                out
            };
            let loss = positive + (R::of(1.0) + (-size).exp()).ln() - target * out;

            (loss, sigmoid(out) - target)
        }
    }
}

/// The weighted loss over the samples on the tape and the weight it was
/// summed over. `targets` and `weights_of` hold one value per sample per
/// head.
fn loss<R: Real>(shape: &Shape, tape: &Tape<R>, targets: &[R], weights_of: &[R]) -> (R, R) {
    let outs = shape.heads.len();
    let (mut sum, mut over) = (R::default(), R::default());

    for at in 0..tape.samples * outs {
        let (loss, _) = judge(shape.heads[at % outs], tape.out[at], targets[at]);

        sum += weights_of[at] * loss;
        over += weights_of[at];
    }

    (sum, over)
}

/// Backpropagation through the samples on the tape. Adds to `gradient`
/// the slope, with respect to every weight, of the weighted mean loss,
/// and returns that loss. Samples before the tape are not reached: the
/// state the tape began from is treated as given.
fn backward<R: Real>(
    shape: &Shape,
    weights: &[R],
    tape: &Tape<R>,
    targets: &[R],
    weights_of: &[R],
    gradient: &mut [R],
) -> R {
    let layout = shape.layout();
    let (ins, units, wide, outs) = (shape.inputs, shape.units, shape.backbone, shape.heads.len());
    let seen = ins + units;

    let (sum, over) = loss(shape, tape, targets, weights_of);

    if over.wide() <= 0.0 {
        return R::default();
    }

    let zero = R::default();
    let one = R::of(1.0);

    // What the loss owes the state after the sample in hand: from its
    // own read-out, and from every later sample through the recurrence.
    let mut d_state = vec![zero; units];

    let mut d_out = vec![zero; outs];
    let mut d_rate = vec![zero; units];
    let mut d_from = vec![zero; units];
    let mut d_to = vec![zero; units];
    let mut d_backbone = vec![zero; wide];

    for at in (0..tape.samples).rev() {
        let seen_now = &tape.seen[at * seen..(at + 1) * seen];
        let backbone = &tape.backbone[at * wide..(at + 1) * wide];
        let gate = &tape.gate[at * units..(at + 1) * units];
        let from = &tape.from[at * units..(at + 1) * units];
        let to = &tape.to[at * units..(at + 1) * units];
        let state = &tape.state[at * units..(at + 1) * units];
        let gap = tape.gap[at];

        for (head, d_out) in d_out.iter_mut().enumerate() {
            let here = at * outs + head;
            let (_, slope) = judge(shape.heads[head], tape.out[here], targets[here]);

            *d_out = weights_of[here] * slope / over;
        }

        add_outer(&mut gradient[layout.out..layout.out_bias], &d_out, state);
        add_to(&mut gradient[layout.out_bias..layout.total], &d_out);
        add_transposed(&mut d_state, &weights[layout.out..layout.out_bias], &d_out);

        for unit in 0..units {
            let d = d_state[unit];
            let d_gate = d * (from[unit] - to[unit]);

            d_rate[unit] = -gap * d_gate * gate[unit] * (one - gate[unit]);
            d_from[unit] = d * gate[unit] * (one - from[unit] * from[unit]);
            d_to[unit] = d * (one - gate[unit]) * (one - to[unit] * to[unit]);
        }

        d_backbone.fill(zero);

        for (matrix, bias, d) in [
            (layout.rate, layout.rate_bias, &d_rate),
            (layout.from, layout.from_bias, &d_from),
            (layout.to, layout.to_bias, &d_to),
        ] {
            add_outer(&mut gradient[matrix..bias], d, backbone);
            add_to(&mut gradient[bias..bias + units], d);
            add_transposed(&mut d_backbone, &weights[matrix..bias], d);
        }

        for (d, value) in d_backbone.iter_mut().zip(backbone) {
            *d = *d * (one - *value * *value);
        }

        add_outer(
            &mut gradient[layout.backbone..layout.backbone_bias],
            &d_backbone,
            seen_now,
        );
        add_to(
            &mut gradient[layout.backbone_bias..layout.backbone_bias + wide],
            &d_backbone,
        );

        // Only the columns that read the state pass anything back.
        d_state.fill(zero);

        add_transposed(
            &mut d_state,
            &weights[layout.backbone + ins * wide..layout.backbone_bias],
            &d_backbone,
        );
    }

    sum / over
}

/// What to learn from: one row per sample of each.
#[derive(Debug, Clone, Copy)]
pub struct Lesson<'a> {
    /// `shape.inputs` numbers per sample.
    pub inputs: &'a [f32],

    /// Time since the sample before, one per sample.
    pub gaps: &'a [f32],

    /// One target per head per sample.
    pub targets: &'a [f32],

    /// How much each target counts toward the gradient, per head per
    /// sample. 0 for a target that is unknown or held back.
    pub fit: &'a [f32],

    /// How much each target counts toward the check that picks which
    /// epoch's weights to keep. Should be 0 wherever `fit` is not, and
    /// later in time.
    pub check: &'a [f32],
}

/// How to train.
#[derive(Debug, Clone, PartialEq)]
pub struct Training {
    /// Passes over the samples.
    pub epochs: usize,

    /// Samples the gradient is followed back through.
    pub window: usize,

    /// Adam's step size at the start. It falls to a tenth by the end.
    pub rate: f32,

    /// The longest a gradient may be before it is scaled down.
    pub clip: f32,

    /// Share of each weight given up per step, to keep weights small.
    pub decay: f32,

    pub seed: u64,
}

impl Default for Training {
    fn default() -> Self {
        Self {
            epochs: 30,
            window: 32,
            rate: 0.004,
            clip: 1.0,
            decay: 0.02,
            seed: 1,
        }
    }
}

/// The mean loss after one epoch, on what was fitted and on what was
/// held back to check against.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EpochLoss {
    pub fit: f32,

    /// 0 when nothing was held back.
    pub check: f32,
}

/// A trained or untrained network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cfc {
    shape: Shape,
    weights: Vec<f32>,
}

impl Cfc {
    /// A network with small random weights. The same seed gives the same
    /// network.
    pub fn new(shape: Shape, seed: u64) -> Self {
        let layout = shape.layout();
        let mut rng = Rng::new(seed);
        let mut weights = vec![0.0; layout.total];

        // Each matrix is spread by how many it reads from and writes to,
        // so that signals neither die out nor blow up on the way through.
        let mut fill = |begin: usize, end: usize, sources: usize, targets: usize| {
            let reach = (6.0 / (sources + targets) as f64).sqrt();

            for weight in &mut weights[begin..end] {
                *weight = rng.around(reach) as f32;
            }
        };

        let seen = shape.inputs + shape.units;

        fill(layout.backbone, layout.backbone_bias, seen, shape.backbone);
        fill(layout.rate, layout.rate_bias, shape.backbone, shape.units);
        fill(layout.from, layout.from_bias, shape.backbone, shape.units);
        fill(layout.to, layout.to_bias, shape.backbone, shape.units);
        fill(layout.out, layout.out_bias, shape.units, shape.heads.len());

        Self { shape, weights }
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    /// Whether the weights are as many as the shape says and all finite.
    /// False for a saved network that has been cut short or damaged.
    pub fn is_sound(&self) -> bool {
        self.weights.len() == self.shape.weights() && self.weights.iter().all(|w| w.is_finite())
    }

    /// A state for a network that has seen nothing.
    pub fn fresh_state(&self) -> Vec<f32> {
        vec![0.0; self.shape.units]
    }

    /// Takes samples in order, moving `state` on, and returns the outputs:
    /// one per head per sample.
    pub fn run(&self, state: &mut [f32], inputs: &[f32], gaps: &[f32]) -> Vec<f32> {
        let mut tape = Tape::default();

        forward(&self.shape, &self.weights, inputs, gaps, state, &mut tape);

        if !gaps.is_empty() {
            state.copy_from_slice(&tape.state[tape.state.len() - self.shape.units..]);
        }

        tape.out
    }

    /// Runs over every sample from a fresh state with the weights as
    /// they are. Returns the two mean losses and writes into `states`
    /// the state each sample met.
    fn measure(&self, lesson: &Lesson, states: &mut Vec<f32>, tape: &mut Tape<f32>) -> EpochLoss {
        let (ins, units, outs) = (self.shape.inputs, self.shape.units, self.shape.heads.len());
        let samples = lesson.gaps.len();

        forward(
            &self.shape,
            &self.weights,
            lesson.inputs,
            lesson.gaps,
            &vec![0.0; units],
            tape,
        );

        states.clear();
        states.resize(units, 0.0);
        states.extend_from_slice(&tape.state[..samples.saturating_sub(1) * units]);

        debug_assert_eq!(lesson.inputs.len(), samples * ins);
        debug_assert_eq!(lesson.targets.len(), samples * outs);

        let mean = |weights_of: &[f32]| {
            let (sum, over) = loss(&self.shape, tape, lesson.targets, weights_of);

            if over > 0.0 { sum / over } else { 0.0 }
        };

        EpochLoss {
            fit: mean(lesson.fit),
            check: mean(lesson.check),
        }
    }

    /// Trains on `lesson` and returns the losses after each epoch.
    ///
    /// Each epoch cuts the fitted samples into windows, in a different
    /// place each time, and takes them in a shuffled order. A window
    /// starts from the state the network had there at the end of the
    /// epoch before, so the gradient is cut at the window's edge but the
    /// state is not. When `lesson.check` holds anything, the weights kept
    /// are those of the epoch that did best on it; otherwise the last.
    ///
    /// `each_epoch` is told the epoch (from 0) and its losses, and
    /// returns nothing; it is where a caller reports progress.
    pub fn train(
        &mut self,
        lesson: &Lesson,
        training: &Training,
        each_epoch: &mut dyn FnMut(usize, EpochLoss),
    ) -> Vec<EpochLoss> {
        let (ins, units, outs) = (self.shape.inputs, self.shape.units, self.shape.heads.len());
        let window = training.window.max(1);

        // Samples after the last fitted one are only ever measured.
        let fitted = lesson
            .fit
            .chunks(outs.max(1))
            .rposition(|weights| weights.iter().any(|weight| *weight > 0.0))
            .map_or(0, |last| last + 1);

        let checked = lesson.check.iter().any(|weight| *weight > 0.0);

        let mut rng = Rng::new(training.seed ^ 0x5EED);
        let mut tape = Tape::default();
        let mut states = Vec::new();

        let count = self.weights.len();
        let mut gradient = vec![0.0f32; count];
        let mut mean = vec![0.0f32; count];
        let mut square = vec![0.0f32; count];
        let mut step = 0;

        let mut best = self.measure(lesson, &mut states, &mut tape);
        let mut kept = self.weights.clone();
        let mut curve = Vec::with_capacity(training.epochs);

        for epoch in 0..training.epochs {
            let shift = rng.below(window);

            // The first window is the short one, so that the edges fall
            // somewhere else each epoch.
            let mut edges = vec![0];
            let mut edge = if shift == 0 { window } else { shift };

            while edge < fitted {
                edges.push(edge);
                edge += window;
            }

            edges.push(fitted);

            let mut spans: Vec<(usize, usize)> = edges
                .windows(2)
                .map(|two| (two[0], two[1]))
                .filter(|(start, end)| start < end)
                .collect();

            rng.shuffle(&mut spans);

            // The step falls along a quarter cosine to a tenth.
            let along = epoch as f32 / training.epochs.max(1) as f32;
            let rate = training.rate * (0.1 + 0.9 * (along * std::f32::consts::FRAC_PI_2).cos());

            for (start, end) in spans {
                forward(
                    &self.shape,
                    &self.weights,
                    &lesson.inputs[start * ins..end * ins],
                    &lesson.gaps[start..end],
                    &states[start * units..(start + 1) * units],
                    &mut tape,
                );

                gradient.fill(0.0);

                backward(
                    &self.shape,
                    &self.weights,
                    &tape,
                    &lesson.targets[start * outs..end * outs],
                    &lesson.fit[start * outs..end * outs],
                    &mut gradient,
                );

                let length = dot(&gradient, &gradient).sqrt();

                if length == 0.0 || !length.is_finite() {
                    continue;
                }

                let scale = (training.clip / length).min(1.0);

                step += 1;

                // Adam: a running mean of the gradient over the root of a
                // running mean of its square, each corrected for having
                // started at zero.
                let mean_fix = 1.0 - 0.9f32.powi(step);
                let square_fix = 1.0 - 0.999f32.powi(step);

                for at in 0..count {
                    let slope = gradient[at] * scale;

                    mean[at] = 0.9 * mean[at] + 0.1 * slope;
                    square[at] = 0.999 * square[at] + 0.001 * slope * slope;

                    let toward = (mean[at] / mean_fix) / ((square[at] / square_fix).sqrt() + 1e-8);

                    self.weights[at] -= rate * (toward + training.decay * self.weights[at]);
                }
            }

            let losses = self.measure(lesson, &mut states, &mut tape);

            let better = if checked {
                losses.check < best.check
            } else {
                true
            };

            if better && self.weights.iter().all(|weight| weight.is_finite()) {
                best = losses;
                kept.copy_from_slice(&self.weights);
            }

            curve.push(losses);
            each_epoch(epoch, losses);
        }

        self.weights = kept;

        curve
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Shape {
        Shape {
            inputs: 3,
            units: 4,
            backbone: 5,
            heads: vec![Head::Number, Head::Odds],
        }
    }

    /// The loss of a tiny network over a few samples at `f64`, and the
    /// gradient the backward pass gives for it.
    struct Case {
        shape: Shape,
        weights: Vec<f64>,
        inputs: Vec<f64>,
        gaps: Vec<f64>,
        start: Vec<f64>,
        targets: Vec<f64>,
        weights_of: Vec<f64>,
    }

    impl Case {
        fn new(seed: u64) -> Self {
            let shape = tiny();
            let samples = 7;
            let mut rng = Rng::new(seed);

            let weights = Cfc::new(shape.clone(), seed)
                .weights
                .iter()
                // Biases start at zero; give them a value so that their
                // gradients are tested away from it too.
                .map(|weight| *weight as f64 + rng.around(0.3))
                .collect();

            let inputs = (0..samples * shape.inputs).map(|_| rng.normal()).collect();
            let gaps = (0..samples).map(|_| 0.2 + 3.0 * rng.unit()).collect();
            let start = (0..shape.units).map(|_| rng.around(0.5)).collect();

            let targets = (0..samples * 2)
                .map(|at| {
                    if at % 2 == 0 {
                        rng.normal()
                    } else {
                        rng.below(2) as f64
                    }
                })
                .collect();

            // Some targets count for more, and some not at all.
            let weights_of = (0..samples * 2)
                .map(|at| if at % 5 == 3 { 0.0 } else { 0.5 + rng.unit() })
                .collect();

            Self {
                shape,
                weights,
                inputs,
                gaps,
                start,
                targets,
                weights_of,
            }
        }

        fn loss(&self, weights: &[f64]) -> f64 {
            let mut tape = Tape::default();

            forward(
                &self.shape,
                weights,
                &self.inputs,
                &self.gaps,
                &self.start,
                &mut tape,
            );

            let (sum, over) = loss(&self.shape, &tape, &self.targets, &self.weights_of);

            sum / over
        }

        fn gradient(&self) -> Vec<f64> {
            let mut tape = Tape::default();
            let mut gradient = vec![0.0; self.weights.len()];

            forward(
                &self.shape,
                &self.weights,
                &self.inputs,
                &self.gaps,
                &self.start,
                &mut tape,
            );

            let loss = backward(
                &self.shape,
                &self.weights,
                &tape,
                &self.targets,
                &self.weights_of,
                &mut gradient,
            );

            assert!((loss - self.loss(&self.weights)).abs() < 1e-12);

            gradient
        }
    }

    /// The largest relative difference between the hand-written gradient
    /// and central finite differences, over every weight.
    fn worst_gradient_error(seed: u64) -> f64 {
        let case = Case::new(seed);
        let gradient = case.gradient();

        let mut worst: f64 = 0.0;
        let nudge = 1e-5;

        for at in 0..case.weights.len() {
            let mut up = case.weights.clone();
            let mut down = case.weights.clone();

            up[at] += nudge;
            down[at] -= nudge;

            let measured = (case.loss(&up) - case.loss(&down)) / (2.0 * nudge);
            let error =
                (gradient[at] - measured).abs() / (gradient[at].abs() + measured.abs()).max(1e-6);

            worst = worst.max(error);
        }

        worst
    }

    #[test]
    fn hand_written_gradients_match_finite_differences() {
        for seed in [1, 2, 3] {
            let case = Case::new(seed);
            let gradient = case.gradient();

            // Every kind of weight must be reached, or the check proves
            // nothing about it.
            let layout = case.shape.layout();

            for (name, begin, end) in [
                ("backbone", layout.backbone, layout.backbone_bias),
                ("backbone bias", layout.backbone_bias, layout.rate),
                ("rate", layout.rate, layout.rate_bias),
                ("rate bias", layout.rate_bias, layout.from),
                ("from", layout.from, layout.from_bias),
                ("from bias", layout.from_bias, layout.to),
                ("to", layout.to, layout.to_bias),
                ("to bias", layout.to_bias, layout.out),
                ("out", layout.out, layout.out_bias),
                ("out bias", layout.out_bias, layout.total),
            ] {
                assert!(
                    gradient[begin..end].iter().any(|slope| slope.abs() > 1e-6),
                    "no gradient reaches the {name} weights"
                );
            }

            let worst = worst_gradient_error(seed);

            println!("seed {seed}: worst relative gradient error {worst:.3e}");

            assert!(worst < 1e-5, "seed {seed}: {worst}");
        }
    }

    #[test]
    fn the_gap_reaches_the_gradient() {
        // If it did not, the check above would pass for a cell that
        // ignored elapsed time.
        let mut case = Case::new(1);
        let honest = case.gradient();

        case.gaps.iter_mut().for_each(|gap| *gap *= 1.5);

        let other = case.gradient();

        let moved = honest
            .iter()
            .zip(&other)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0, f64::max);

        assert!(moved > 1e-3, "the gap does not reach the gradient: {moved}");
    }

    #[test]
    fn both_widths_give_the_same_outputs() {
        let case = Case::new(4);
        let narrow: Vec<f32> = case.weights.iter().map(|weight| *weight as f32).collect();

        let mut wide_tape = Tape::default();
        let mut narrow_tape = Tape::default();

        forward(
            &case.shape,
            &case.weights,
            &case.inputs,
            &case.gaps,
            &case.start,
            &mut wide_tape,
        );

        let narrowed = |values: &[f64]| values.iter().map(|x| *x as f32).collect::<Vec<f32>>();

        forward(
            &case.shape,
            &narrow,
            &narrowed(&case.inputs),
            &narrowed(&case.gaps),
            &narrowed(&case.start),
            &mut narrow_tape,
        );

        for (wide, narrow) in wide_tape.out.iter().zip(&narrow_tape.out) {
            assert!((wide - *narrow as f64).abs() < 1e-4, "{wide} {narrow}");
        }
    }

    #[test]
    fn a_longer_gap_moves_the_state_further() {
        let network = Cfc::new(tiny(), 9);
        let input = [0.3, -0.2, 0.8];

        let after = |gap: f32| {
            let mut state = network.fresh_state();

            network.run(&mut state, &input, &[gap]);

            state
        };

        assert_ne!(after(1.0), after(3.0));

        // Run in two calls or in one, the state and outputs are the same.
        let inputs = [0.3, -0.2, 0.8, 0.1, 0.5, -0.9];
        let mut together = network.fresh_state();
        let mut apart = network.fresh_state();

        let outputs = network.run(&mut together, &inputs, &[1.0, 3.0]);
        let first = network.run(&mut apart, &inputs[..3], &[1.0]);
        let second = network.run(&mut apart, &inputs[3..], &[3.0]);

        assert_eq!(together, apart);
        assert_eq!(outputs, [first, second].concat());
    }

    /// A lesson only memory can answer: the target is the input of two
    /// samples ago, and whether it was positive.
    fn memory_lesson(samples: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut rng = Rng::new(11);

        let inputs: Vec<f32> = (0..samples).map(|_| rng.normal() as f32).collect();
        let gaps = vec![1.0; samples];

        let targets = (0..samples)
            .flat_map(|at| {
                let earlier = if at < 2 { 0.0 } else { inputs[at - 2] };

                [earlier, (earlier > 0.0) as u8 as f32]
            })
            .collect();

        (inputs, gaps, targets)
    }

    #[test]
    fn training_learns_what_only_memory_can_answer_and_is_repeatable() {
        let samples = 400;
        let (inputs, gaps, targets) = memory_lesson(samples);

        // The last quarter is held back to check against.
        let fit: Vec<f32> = (0..samples * 2).map(|at| (at < 600) as u8 as f32).collect();
        let check: Vec<f32> = fit.iter().map(|weight| 1.0 - weight).collect();

        let lesson = Lesson {
            inputs: &inputs,
            gaps: &gaps,
            targets: &targets,
            fit: &fit,
            check: &check,
        };

        let shape = Shape {
            inputs: 1,
            units: 8,
            backbone: 8,
            heads: vec![Head::Number, Head::Odds],
        };

        let training = Training {
            epochs: 25,
            window: 16,
            ..Training::default()
        };

        let train = || {
            let mut network = Cfc::new(shape.clone(), 5);
            let mut told = 0;

            let before = network.measure(&lesson, &mut Vec::new(), &mut Tape::default());
            let curve = network.train(&lesson, &training, &mut |_, _| told += 1);

            assert_eq!(told, 25);

            (network, before, curve)
        };

        let (network, before, curve) = train();
        let after = network.measure(&lesson, &mut Vec::new(), &mut Tape::default());

        println!("check loss before {} after {}", before.check, after.check);

        assert!(after.check < 0.5 * before.check, "{before:?} {after:?}");
        assert!(network.is_sound());

        // The weights kept are those of the best epoch on the check.
        let best = curve.iter().map(|loss| loss.check).fold(f32::MAX, f32::min);

        assert_eq!(after.check, best);

        let (again, _, again_curve) = train();

        assert_eq!(network, again);
        assert_eq!(curve, again_curve);
    }
}
