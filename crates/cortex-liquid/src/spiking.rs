//! The spiking network: a fixed reservoir of leaky integrate-and-fire
//! neurons that each bar is played into, and whose recent firing is read
//! out by a linear layer ([`crate::ridge`]).
//!
//! Each bar's scaled numbers become input currents, split into a positive
//! and a negative channel so that a neuron can answer to a fall as well as
//! a rise. The bar is played for [`TICKS`] ticks: every neuron leaks, takes
//! its inputs and the spikes of the neurons that feed it, fires when its
//! potential passes 1, and rests for a tick after firing. A slow trace of
//! each neuron's firing carries what happened a few bars back into the
//! present, which is the reservoir's memory.
//!
//! Only the read-out is trained; the reservoir's weights are random and
//! fixed by the seed. This is a liquid state machine (Maass et al., 2002)
//! made small: it finds what a few hundred spiking neurons and a straight
//! line can find, and no more.

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// Ticks each bar is played for.
pub const TICKS: usize = 10;

/// Share of the neurons that inhibit the ones they feed.
const INHIBITORY: f64 = 0.2;

/// Inputs each neuron listens to, and neurons each neuron is fed by.
const FAN_IN: usize = 3;
const RECURRENT: usize = 12;

/// How much of its potential a neuron keeps from one tick to the next.
const LEAK: f32 = 0.82;

/// How much of a neuron's trace is kept from one bar to the next.
const TRACE_KEPT: f32 = 0.6;

/// Ticks a neuron cannot fire for after firing.
const REST: u8 = 1;

/// The sizes and fixed weights of a reservoir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reservoir {
    pub neurons: usize,
    pub inputs: usize,

    /// For each neuron, the input channels it listens to and how much.
    taps: Vec<Vec<(u16, f32)>>,

    /// For each neuron, the neurons it is fed by and how much.
    feeds: Vec<Vec<(u16, f32)>>,

    /// A steady current each neuron gets, so that some fire at rest.
    bias: Vec<f32>,
}

/// Where a reservoir is: each neuron's potential, rest and trace.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Activity {
    potential: Vec<f32>,
    resting: Vec<u8>,
    fired: Vec<bool>,

    /// Each neuron's slow trace of its firing: what the read-out sees.
    pub trace: Vec<f32>,

    /// Spikes in the last bar played, over all neurons.
    pub spikes: u32,
}

impl Reservoir {
    pub fn is_sound(&self) -> bool {
        self.neurons > 0
            && self.neurons <= u16::MAX as usize
            && self.inputs > 0
            && self.taps.len() == self.neurons
            && self.feeds.len() == self.neurons
            && self.bias.len() == self.neurons
            && self.bias.iter().all(|b| b.is_finite())
            && self
                .taps
                .iter()
                .flatten()
                .all(|(c, w)| usize::from(*c) < self.inputs * 2 && w.is_finite())
            && self
                .feeds
                .iter()
                .flatten()
                .all(|(n, w)| usize::from(*n) < self.neurons && w.is_finite())
    }
    pub fn rewired(&self, seed: u64) -> Self {
        let mut candidate = self.clone();
        let mut rng = Rng::new(seed);
        for (neuron, feeds) in candidate.feeds.iter_mut().enumerate() {
            if feeds.is_empty() || candidate.neurons < 2 {
                continue;
            }
            let at = rng.below(feeds.len());
            let from = rng.below(candidate.neurons);
            if from != neuron {
                feeds[at].0 = from as u16;
            }
        }
        candidate
    }
    /// A reservoir of `neurons` listening to `inputs` numbers per bar.
    /// The same seed gives the same reservoir.
    pub fn new(neurons: usize, inputs: usize, seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x5B1C);
        let channels = inputs * 2;

        let inhibits: Vec<bool> = (0..neurons).map(|_| rng.unit() < INHIBITORY).collect();

        let taps = (0..neurons)
            .map(|_| {
                (0..FAN_IN)
                    .map(|_| {
                        let channel = rng.below(channels.max(1)) as u16;
                        let weight = (0.6 + rng.unit() * 0.9) as f32;

                        (channel, weight)
                    })
                    .collect()
            })
            .collect();

        let feeds = (0..neurons)
            .map(|neuron| {
                (0..RECURRENT)
                    .filter_map(|_| {
                        let from = rng.below(neurons.max(1));

                        if from == neuron {
                            return None;
                        }

                        let size = (0.05 + rng.unit() * 0.15) as f32;
                        let weight = if inhibits[from] { -size * 2.5 } else { size };

                        Some((from as u16, weight))
                    })
                    .collect()
            })
            .collect();

        let bias = (0..neurons)
            .map(|_| (0.04 + rng.unit() * 0.08) as f32)
            .collect();

        Self {
            neurons,
            inputs,
            taps,
            feeds,
            bias,
        }
    }

    /// A reservoir that has heard nothing.
    pub fn fresh(&self) -> Activity {
        Activity {
            potential: vec![0.0; self.neurons],
            resting: vec![0; self.neurons],
            fired: vec![false; self.neurons],
            trace: vec![0.0; self.neurons],
            spikes: 0,
        }
    }

    /// Whether `activity` is of this reservoir.
    pub fn fits(&self, activity: &Activity) -> bool {
        activity.potential.len() == self.neurons
            && activity.resting.len() == self.neurons
            && activity.fired.len() == self.neurons
            && activity.trace.len() == self.neurons
    }

    /// Plays one bar's scaled numbers into the reservoir.
    pub fn play(&self, activity: &mut Activity, inputs: &[f32]) {
        let mut channels = vec![0.0f32; self.inputs * 2];

        for (at, value) in inputs.iter().take(self.inputs).enumerate() {
            channels[at * 2] = value.max(0.0) * 0.35;
            channels[at * 2 + 1] = (-value).max(0.0) * 0.35;
        }

        let drive: Vec<f32> = self
            .taps
            .iter()
            .zip(&self.bias)
            .map(|(taps, bias)| {
                bias + taps
                    .iter()
                    .map(|(channel, weight)| channels[*channel as usize] * weight)
                    .sum::<f32>()
            })
            .collect();

        let mut counts = vec![0u16; self.neurons];
        let mut fired_now = vec![false; self.neurons];

        for _ in 0..TICKS {
            for neuron in 0..self.neurons {
                if activity.resting[neuron] > 0 {
                    activity.resting[neuron] -= 1;
                    activity.potential[neuron] = 0.0;
                    fired_now[neuron] = false;

                    continue;
                }

                let fed: f32 = self.feeds[neuron]
                    .iter()
                    .filter(|(from, _)| activity.fired[*from as usize])
                    .map(|(_, weight)| weight)
                    .sum();

                let potential = activity.potential[neuron] * LEAK + drive[neuron] + fed;

                if potential >= 1.0 {
                    activity.potential[neuron] = 0.0;
                    activity.resting[neuron] = REST;
                    fired_now[neuron] = true;
                    counts[neuron] += 1;
                } else {
                    activity.potential[neuron] = potential.max(-1.0);
                    fired_now[neuron] = false;
                }
            }

            activity.fired.copy_from_slice(&fired_now);
        }

        activity.spikes = counts.iter().map(|count| u32::from(*count)).sum();

        for (trace, count) in activity.trace.iter_mut().zip(&counts) {
            *trace = *trace * TRACE_KEPT + f32::from(*count) / TICKS as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_and_bars_give_the_same_firing() {
        let reservoir = Reservoir::new(64, 4, 7);
        let mut one = reservoir.fresh();
        let mut two = reservoir.fresh();

        for step in 0..20 {
            let inputs = [step as f32 * 0.1, -0.5, 1.0, 0.0];

            reservoir.play(&mut one, &inputs);
            reservoir.play(&mut two, &inputs);
        }

        assert_eq!(one, two);
    }

    #[test]
    fn stronger_input_makes_more_spikes_and_the_trace_remembers() {
        let reservoir = Reservoir::new(128, 3, 1);

        let mut quiet = reservoir.fresh();
        let mut loud = reservoir.fresh();

        reservoir.play(&mut quiet, &[0.0, 0.0, 0.0]);
        reservoir.play(&mut loud, &[3.0, -3.0, 3.0]);

        assert!(
            loud.spikes > quiet.spikes,
            "{} {}",
            loud.spikes,
            quiet.spikes
        );

        let before: f32 = loud.trace.iter().sum();

        reservoir.play(&mut loud, &[0.0, 0.0, 0.0]);

        let after: f32 = loud.trace.iter().sum();

        // The loud bar is still in the trace a bar later.
        assert!(after > 0.0 && after < before + 1e-3 + quiet.trace.iter().sum::<f32>() * 2.0);
    }
}
