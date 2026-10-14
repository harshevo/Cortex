use crate::{
    cfc::{Cfc, Head, Lesson, Shape, Training},
    ridge::Ridge,
    spiking::{Activity, Reservoir},
};
use cortex_living::Network;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

pub const INPUTS: usize = 16;
const OUTPUTS: usize = 3;
const REPLAY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Perception {
    pub domain: String,
    pub scope: String,
    pub time_ms: i64,
    pub values: [f32; INPUTS],
    pub percepts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AbilityReading {
    pub domain: String,
    pub scope: String,
    pub time_ms: i64,
    pub predictions: Vec<(String, Vec<f32>)>,
    pub expectations: Vec<(String, f32)>,
    pub reasons: Vec<String>,
    pub imagined: Vec<String>,
    pub plan: Vec<Vec<String>>,
    pub neurons: usize,
    pub branches: usize,
    pub synapses: usize,
    pub snn_neurons: usize,
    pub spikes: u32,
    pub generation: u64,
    pub replays: u64,
    pub promoted: u64,
    pub observed: u64,
    pub scored: u64,
    pub mean_errors: [f64; 3],
    pub baseline_error: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pair {
    inputs: [f32; INPUTS],
    dendritic: Vec<f32>,
    targets: [f32; OUTPUTS],
    mask: [bool; OUTPUTS],
    gap: f32,
    time_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DomainState {
    cell: Cfc,
    cell_state: Vec<f32>,
    reservoir: Reservoir,
    activity: Activity,
    readout: Ridge,
    dendritic: Network,
    dendritic_readout: Ridge,
    replay: VecDeque<Pair>,
    previous: Option<Perception>,
    previous_dendritic: Vec<f32>,
    predictions: [Vec<f32>; 3],
    reading: AbilityReading,
    memory_features: [f32; 4],
    previous_gap: f32,
    retained: Vec<Pair>,
}

fn shape(units: usize) -> Shape {
    Shape {
        inputs: INPUTS,
        units,
        backbone: units,
        heads: vec![Head::Number; OUTPUTS],
    }
}
fn readout(inputs: usize) -> Ridge {
    Ridge::fit(&[], &[], inputs, OUTPUTS, 1., |_| true)
}

impl DomainState {
    fn new() -> Self {
        let cell = Cfc::new(shape(12), 41);
        let reservoir = Reservoir::new(32, INPUTS, 42);
        Self {
            cell_state: cell.fresh_state(),
            activity: reservoir.fresh(),
            cell,
            reservoir,
            readout: readout(32),
            dendritic: Network::with_meanings(
                &[("feedback: accepted", 1.), ("feedback: rejected", -1.)],
                &["action: "],
            ),
            dendritic_readout: readout(INPUTS + 3),
            replay: VecDeque::new(),
            previous: None,
            previous_dendritic: Vec::new(),
            predictions: std::array::from_fn(|_| vec![0.; OUTPUTS]),
            reading: AbilityReading::default(),
            memory_features: [0.; 4],
            previous_gap: 1.,
            retained: Vec::new(),
        }
    }

    fn perceive(&mut self, event: &Perception) -> AbilityReading {
        let mut encoded = event.clone();
        encoded.values[12..].copy_from_slice(&self.memory_features);
        let event = &encoded;
        if self
            .previous
            .as_ref()
            .is_some_and(|p| event.time_ms <= p.time_ms)
        {
            return self.reading.clone();
        }
        if self.previous.as_ref().is_some_and(|p| {
            event.time_ms - p.time_ms
                > if event.domain == "trading" {
                    120_000
                } else {
                    1_800_000
                }
        }) {
            self.previous = None;
            self.cell_state = self.cell.fresh_state();
            self.activity = self.reservoir.fresh();
        }
        self.dendritic.live(&event.percepts);
        let mut dendritic = event.values.to_vec();
        let expected = self.dendritic.expectations();
        for prefix in ["movement: up", "movement: down", "movement: flat"] {
            dendritic.push(
                expected
                    .iter()
                    .find(|(label, _)| label == prefix)
                    .map_or(0., |(_, p)| *p),
            );
        }
        let gap = self.previous.as_ref().map_or(1., |p| {
            ((event.time_ms - p.time_ms) as f32 / 10_000.).clamp(0.01, 20.)
        });
        if let Some(previous) = &self.previous {
            let targets = [event.values[0], event.values[1], event.values[2]];
            let mask = [true, event.domain == "coding" || event.values[3] > 0., true];
            for (at, prediction) in self.predictions.iter().enumerate() {
                self.reading.mean_errors[at] += masked_mse(prediction, &targets, &mask);
            }
            self.reading.baseline_error += masked_mse(&previous.values[..3], &targets, &mask);
            self.reading.scored += 1;
            self.replay.push_back(Pair {
                inputs: previous.values,
                dendritic: self.previous_dendritic.clone(),
                targets,
                mask,
                gap: self.previous_gap,
                time_ms: previous.time_ms,
            });
            if self.reading.scored.is_multiple_of(8)
                && let Some(pair) = self.replay.back().cloned() {
                    if self.retained.len() < 64 {
                        self.retained.push(pair);
                    } else {
                        let slot = 32 + (self.reading.scored as usize / 8) % 32;
                        self.retained[slot] = pair;
                    }
                }
            if self.replay.len() > REPLAY {
                self.replay.pop_front();
            }
            if self.replay.len() >= 40 && self.reading.observed.is_multiple_of(20) {
                self.learn();
            }
        }
        self.predictions[0] = self.cell.run(&mut self.cell_state, &event.values, &[gap]);
        self.reservoir.play(&mut self.activity, &event.values);
        self.predictions[1] = self.dendritic_readout.apply(&dendritic);
        self.predictions[2] = self.readout.apply(&self.activity.trace);
        let meters = self.dendritic.meters();
        self.reading.domain = event.domain.clone();
        self.reading.scope = event.scope.clone();
        self.reading.time_ms = event.time_ms;
        self.reading.neurons = meters.neurons;
        self.reading.branches = meters.segments;
        self.reading.synapses = meters.synapses;
        self.reading.snn_neurons = self.reservoir.neurons;
        self.reading.spikes = self.activity.spikes;
        self.reading.observed += 1;
        self.reading.predictions = ["LNN", "DANN", "SNN"]
            .into_iter()
            .zip(self.predictions.clone())
            .map(|(n, p)| (n.to_string(), p))
            .collect();
        self.reading.expectations = expected.into_iter().take(8).collect();
        self.reading.reasons = self
            .reading
            .expectations
            .iter()
            .flat_map(|(label, _)| self.dendritic.explain(label))
            .take(6)
            .map(|r| {
                format!(
                    "{} because {}; confidence {:.0}%, {} hits / {} misses",
                    r.expected,
                    r.because.join(", "),
                    r.confidence * 100.,
                    r.hits,
                    r.misses
                )
            })
            .collect();
        self.reading.imagined = self
            .dendritic
            .imagine(3)
            .iter()
            .take(3)
            .map(|t| format!("{t:?}"))
            .collect();
        self.reading.plan = self
            .dendritic
            .plan(
                if event.domain == "coding" {
                    "checks: passed"
                } else {
                    "movement: up"
                },
                3,
            )
            .into_iter()
            .take(3)
            .collect();
        self.previous = Some(event.clone());
        self.previous_gap = gap;
        self.previous_dendritic = dendritic;
        self.reading.clone()
    }

    fn learn(&mut self) {
        let mut pairs: Vec<_> = self.retained.iter().chain(&self.replay).cloned().collect();
        pairs.sort_by_key(|p| p.time_ms);
        pairs.dedup_by_key(|p| p.time_ms);
        let count = pairs.len();
        let split = count * 4 / 5;
        let inputs: Vec<_> = pairs.iter().flat_map(|p| p.inputs).collect();
        let targets: Vec<_> = pairs.iter().flat_map(|p| p.targets).collect();
        let gaps: Vec<_> = pairs.iter().map(|p| p.gap).collect();
        let mask: Vec<_> = pairs.iter().flat_map(|p| p.mask).collect();
        let weights: Vec<_> = (0..count * OUTPUTS)
            .map(|at| {
                if at / OUTPUTS < split && mask[at] {
                    1.
                } else {
                    0.
                }
            })
            .collect();
        let check: Vec<_> = (0..count * OUTPUTS)
            .map(|at| {
                if at / OUTPUTS >= split && mask[at] {
                    1.
                } else {
                    0.
                }
            })
            .collect();
        let lesson = Lesson {
            inputs: &inputs,
            gaps: &gaps,
            targets: &targets,
            fit: &weights,
            check: &check,
        };
        let mut candidate = if self.reading.generation > 0
            && self.reading.generation.is_multiple_of(5)
            && self.cell.shape().units < 32
        {
            Cfc::new(
                shape(self.cell.shape().units + 4),
                41 + self.reading.generation,
            )
        } else {
            self.cell.clone()
        };
        candidate.train(
            &lesson,
            &Training {
                epochs: 3,
                window: 16,
                rate: 0.001,
                ..Training::default()
            },
            &mut |_, _| {},
        );
        if cell_error(&candidate, &pairs, split) < cell_error(&self.cell, &pairs, split) {
            self.cell = candidate;
            self.cell_state = self.cell.fresh_state();
            self.reading.promoted += 1;
        }
        // Compare bounded topology candidates on later observations before publishing.
        let reservoir = if self.reservoir.neurons < 128 && self.reading.generation.is_multiple_of(3)
        {
            Reservoir::new(
                self.reservoir.neurons + 8,
                INPUTS,
                42 + self.reading.generation,
            )
        } else {
            self.reservoir.rewired(42 + self.reading.generation)
        };
        let candidate_x = reservoir_rows(&reservoir, &pairs);
        let active_x = reservoir_rows(&self.reservoir, &pairs);
        let candidate_head = Ridge::fit_masked(
            &candidate_x,
            &targets,
            &mask,
            reservoir.neurons,
            OUTPUTS,
            1.,
            |at| at < split,
        );
        let active_head = Ridge::fit_masked(
            &active_x,
            &targets,
            &mask,
            self.reservoir.neurons,
            OUTPUTS,
            1.,
            |at| at < split,
        );
        let candidate_loss = head_error(&candidate_head, &candidate_x, &pairs, split);
        let active_loss = head_error(&active_head, &active_x, &pairs, split);
        let previous_loss = head_error(&self.readout, &active_x, &pairs, split);
        if candidate_loss < active_loss.min(previous_loss) * 0.99 && candidate_loss.is_finite() {
            self.reservoir = reservoir;
            self.activity = self.reservoir.fresh();
            self.readout = candidate_head;
            self.reading.promoted += 1;
        } else if active_loss < previous_loss {
            self.readout = active_head;
        }
        let dendritic: Vec<_> = pairs.iter().flat_map(|p| p.dendritic.clone()).collect();
        let head = Ridge::fit_masked(&dendritic, &targets, &mask, INPUTS + 3, OUTPUTS, 1., |at| {
            at < split
        });
        if head_error(&head, &dendritic, &pairs, split)
            <= head_error(&self.dendritic_readout, &dendritic, &pairs, split)
        {
            self.dendritic_readout = head;
        }
        self.activity = self.reservoir.fresh();
        self.cell_state = self.cell.fresh_state();
        for pair in &pairs {
            self.reservoir.play(&mut self.activity, &pair.inputs);
            self.cell
                .run(&mut self.cell_state, &pair.inputs, &[pair.gap]);
        }
        for _ in 0..3 {
            self.dendritic.rest();
        }
        self.reading.generation += 1;
        self.reading.replays += count as u64;
    }
}

fn masked_mse(prediction: &[f32], targets: &[f32], mask: &[bool]) -> f64 {
    prediction
        .iter()
        .zip(targets)
        .zip(mask)
        .filter(|(_, present)| **present)
        .map(|((a, b), _)| f64::from(a - b).powi(2))
        .sum::<f64>()
        / mask.iter().filter(|p| **p).count().max(1) as f64
}
fn cell_error(cell: &Cfc, pairs: &[Pair], split: usize) -> f64 {
    let mut state = cell.fresh_state();
    let mut error = 0.;
    for (at, pair) in pairs.iter().enumerate() {
        let out = cell.run(&mut state, &pair.inputs, &[pair.gap]);
        if at >= split {
            error += masked_mse(&out, &pair.targets, &pair.mask);
        }
    }
    error / (pairs.len() - split).max(1) as f64
}
fn reservoir_rows(reservoir: &Reservoir, pairs: &[Pair]) -> Vec<f32> {
    let mut state = reservoir.fresh();
    let mut rows = Vec::new();
    for pair in pairs {
        reservoir.play(&mut state, &pair.inputs);
        rows.extend_from_slice(&state.trace);
    }
    rows
}
fn head_error(head: &Ridge, rows: &[f32], pairs: &[Pair], split: usize) -> f64 {
    pairs
        .iter()
        .enumerate()
        .skip(split)
        .map(|(at, p)| {
            masked_mse(
                &head.apply(&rows[at * head.inputs..(at + 1) * head.inputs]),
                &p.targets,
                &p.mask,
            )
        })
        .sum::<f64>()
        / (pairs.len() - split).max(1) as f64
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Abilities {
    states: BTreeMap<String, DomainState>,
}

impl Abilities {
    pub fn readings(&self) -> BTreeMap<String, AbilityReading> {
        self.states
            .iter()
            .map(|(key, state)| (key.clone(), state.reading.clone()))
            .collect()
    }
    pub fn is_sound(&self) -> bool {
        self.states.len() <= 16
            && self.states.values().all(|s| {
                s.cell.is_sound()
                    && s.cell.shape().inputs == INPUTS
                    && s.cell.shape().heads.len() == OUTPUTS
                    && s.cell_state.len() == s.cell.shape().units
                    && s.reservoir.is_sound()
                    && s.reservoir.fits(&s.activity)
                    && s.readout.is_sound()
                    && s.readout.inputs == s.reservoir.neurons
                    && s.readout.outputs == OUTPUTS
                    && s.dendritic_readout.is_sound()
                    && s.dendritic_readout.inputs == INPUTS + 3
                    && s.replay.len() <= REPLAY
                    && s.replay.iter().all(|p| p.dendritic.len() == INPUTS + 3)
            })
    }
    pub fn observe(&mut self, event: Perception) -> AbilityReading {
        if event.values.iter().any(|v| !v.is_finite()) {
            return AbilityReading::default();
        }
        let key = format!("{}:{}", event.domain, event.scope);
        // Bound active recurrent states; checkpoints retain the selected scopes.
        if !self.states.contains_key(&key) && self.states.len() >= 16 {
            let oldest = self
                .states
                .iter()
                .min_by_key(|(_, s)| s.reading.time_ms)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.states.remove(&oldest);
        }
        self.states
            .entry(key)
            .or_insert_with(DomainState::new)
            .perceive(&event)
    }

    pub fn hear(&mut self, domain: &str, scope: &str, percepts: &[String]) -> AbilityReading {
        let key = format!("{domain}:{scope}");
        if !self.states.contains_key(&key) && self.states.len() >= 16
            && let Some(oldest) = self
                .states
                .iter()
                .min_by_key(|(_, s)| s.reading.time_ms)
                .map(|(k, _)| k.clone())
            {
                self.states.remove(&oldest);
            }
        let state = self.states.entry(key).or_insert_with(DomainState::new);
        let mut features = [0f32; 4];
        for token in percepts {
            let hash = token.bytes().fold(2166136261u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(16777619)
            });
            features[hash as usize % 4] += if hash & 4 == 0 { 1. } else { -1. };
        }
        for (to, from) in state.memory_features.iter_mut().zip(features) {
            *to = (*to * 0.7 + from / percepts.len().max(1) as f32 * 0.3).clamp(-1., 1.);
        }
        state.dendritic.live(percepts);
        let meters = state.dendritic.meters();
        state.reading.neurons = meters.neurons;
        state.reading.branches = meters.segments;
        state.reading.synapses = meters.synapses;
        state.reading.expectations = state.dendritic.expectations().into_iter().take(8).collect();
        state.reading.imagined = state
            .dendritic
            .imagine(3)
            .iter()
            .take(3)
            .map(|t| format!("{t:?}"))
            .collect();
        state.reading.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domains_are_isolated_and_learning_survives_serialization() {
        let mut abilities = Abilities::default();
        for at in 1..85 {
            let mut values = [0.; INPUTS];
            values[0] = (at as f32 * 0.1).sin();
            values[1] = values[0] * 0.5;
            abilities.observe(Perception {
                domain: "trading".into(),
                scope: "NSE_EQ:13".into(),
                time_ms: at * 10_000,
                values,
                percepts: vec![if values[0] > 0. {
                    "movement: up".into()
                } else {
                    "movement: down".into()
                }],
            });
        }
        let mut restored: Abilities =
            serde_json::from_slice(&serde_json::to_vec(&abilities).unwrap()).unwrap();
        let trading = restored.states.get("trading:NSE_EQ:13").unwrap();
        assert!(trading.reading.scored > 70);
        assert!(trading.reading.replays > 0);
        assert!(trading.reading.generation > 0);
        assert!(trading.reading.mean_errors.iter().all(|e| e.is_finite()));
        assert!(trading.reservoir.neurons <= 128);
        let coding = restored.hear("coding", "repo", &["result: failed".into()]);
        assert_eq!(coding.scored, 0);
        assert!(restored.states.contains_key("coding:repo"));
    }
}
