#[cfg(test)]
use super::network::Rng;
use serde::{Deserialize, Serialize};

/// Plastic output synapses over filtered circuit spikes. These local updates
/// leave recurrence unchanged; the offline BPTT trainer can learn the shared core.
#[derive(Clone, Serialize, Deserialize)]
pub struct Readout {
    weights: Vec<Vec<f32>>,
    bias: Vec<f32>,
    pub updates: u64,
}

impl Readout {
    pub fn new(features: usize, classes: usize) -> Result<Self, String> {
        if features == 0
            || features > 100_000
            || !(2..=8192).contains(&classes)
            || features * classes > 40_000_000
        {
            return Err("Readout dimensions exceed limits.".into());
        }
        Ok(Self {
            weights: vec![vec![0.0; features]; classes],
            bias: vec![0.0; classes],
            updates: 0,
        })
    }
    pub(crate) fn insert_features(&mut self, at: usize, count: usize) -> Result<(), String> {
        let size = self.weights.first().ok_or("Empty readout")?.len();
        if at > size || (size + count) * self.weights.len() > 40_000_000 {
            return Err("Growth exceeds readout budget.".into());
        }
        for row in &mut self.weights {
            row.splice(at..at, std::iter::repeat_n(0.0, count));
        }
        Ok(())
    }
    pub fn weights(&self) -> &[Vec<f32>] {
        &self.weights
    }
    pub fn bias(&self) -> &[f32] {
        &self.bias
    }
    pub fn validate(&self, features: usize, classes: usize) -> Result<(), String> {
        if features == 0
            || features > 100_000
            || !(2..=8192).contains(&classes)
            || features * classes > 40_000_000
            || self.weights.len() != classes
            || self.bias.len() != classes
            || self
                .weights
                .iter()
                .any(|w| w.len() != features || w.iter().any(|v| !v.is_finite()))
            || self.bias.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid readout checkpoint.".into());
        }
        Ok(())
    }
    pub fn probabilities(&self, features: &[f32]) -> Result<Vec<f32>, String> {
        if self.weights.len() < 2
            || self.bias.len() != self.weights.len()
            || self.weights.iter().any(|w| w.len() != features.len())
            || features.is_empty()
            || features.iter().any(|v| !v.is_finite() || v.abs() > 1000.0)
        {
            return Err("Invalid readout features.".into());
        }
        let logits: Vec<_> = self
            .weights
            .iter()
            .zip(&self.bias)
            .map(|(w, b)| b + w.iter().zip(features).map(|(w, x)| w * x).sum::<f32>())
            .collect();
        if logits.iter().any(|v| !v.is_finite()) {
            return Err("Nonfinite output state.".into());
        }
        let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut p: Vec<_> = logits.iter().map(|v| (v - max).exp()).collect();
        let sum = p.iter().sum::<f32>();
        for v in &mut p {
            *v /= sum;
        }
        Ok(p)
    }
    pub fn predict(&self, features: &[f32]) -> Result<(usize, f32), String> {
        self.probabilities(features)?
            .into_iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or("No output neurons.".into())
    }
    pub fn teach(&mut self, features: &[f32], target: usize, rate: f32) -> Result<f32, String> {
        let p = self.probabilities(features)?;
        if target >= p.len() || !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return Err("Invalid teaching target/rate.".into());
        }
        let loss = -p[target].max(1e-9).ln();
        self.update(features, &p, target, rate);
        Ok(loss)
    }
    fn update(&mut self, features: &[f32], p: &[f32], target: usize, amount: f32) {
        let norm = features.iter().map(|x| x * x).sum::<f32>().max(1.0).sqrt();
        for (class, (weights, bias)) in self.weights.iter_mut().zip(&mut self.bias).enumerate() {
            let error = amount * (f32::from(class == target) - p[class]) / norm;
            for (w, x) in weights.iter_mut().zip(features) {
                *w = (*w + error * x).clamp(-20.0, 20.0);
            }
            *bias = (*bias + error).clamp(-20.0, 20.0);
        }
        self.updates = self.updates.saturating_add(1);
    }
    /// REINFORCE: selected action and scalar reward; no desired action argument.
    pub fn reinforce(
        &mut self,
        features: &[f32],
        action: usize,
        advantage: f32,
        rate: f32,
    ) -> Result<(), String> {
        let p = self.probabilities(features)?;
        if action >= p.len()
            || !advantage.is_finite()
            || !(-2.0..=2.0).contains(&advantage)
            || !rate.is_finite()
            || !(0.0..=1.0).contains(&rate)
        {
            return Err("Invalid reinforcement update.".into());
        }
        self.update(features, &p, action, rate * advantage);
        let retention = 1.0 - rate * 0.01;
        for (weights, bias) in self.weights.iter_mut().zip(&mut self.bias) {
            for weight in weights {
                *weight *= retention;
            }
            *bias *= retention;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn sample(&self, features: &[f32], rng: &mut Rng) -> Result<usize, String> {
        let p = self.probabilities(features)?;
        let threshold = rng.unit();
        let mut sum = 0.0;
        for (i, v) in p.iter().enumerate() {
            sum += v;
            if threshold < sum {
                return Ok(i);
            }
        }
        Ok(p.len() - 1)
    }
}
