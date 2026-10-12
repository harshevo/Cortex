//! Simulating the network.
//!
//! Every neuron is a leaky integrate-and-fire unit: its voltage leaks
//! towards rest, rises with the current arriving on its synapses, and when
//! it crosses a threshold the neuron fires a spike, resets, and rests for a
//! moment. A spike reaches the neuron's targets on the next time step.
//!
//! A situation is presented by driving the input neurons for a fixed
//! number of steps from rest. The answer read out is how often each Kenyon
//! cell fired. Starting from rest with no noise makes the response a pure
//! function of the situation: the same situation always gives the same
//! code.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::connectome::{Connectome, Layout, Random, Wiring};
use crate::situation::FEATURES;

/// Share of the voltage kept from one step to the next.
const DECAY: f32 = 0.8;

const THRESHOLD: f32 = 1.0;

/// Steps a neuron stays silent after firing.
const REFRACTORY_STEPS: u8 = 2;

/// Inhibition cannot push a voltage below this.
const FLOOR: f32 = -1.0;

/// Voltage a fully present feature adds per step to its most sensitive
/// input neuron.
const INPUT_DRIVE: f32 = 0.5;

/// Marks a neuron that is not part of the code read out.
const NOT_READ: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq)]
pub struct BrainConfig {
    pub neurons: usize,
    pub inputs: usize,
    pub kenyon: usize,
    pub out_degree: usize,
    pub inputs_per_kenyon: usize,
    pub inhibitory_share: f32,
    pub seed: u64,

    /// Time steps a situation is presented for.
    pub steps: u32,

    /// Voltage a central synapse of strength 1 adds to its target.
    pub central_gain: f32,

    /// Voltage an input synapse of strength 1 adds to a Kenyon cell.
    pub kenyon_gain: f32,

    /// Strength of the feedback that holds overall activity down, as the
    /// APL neuron does for the fly's Kenyon cells. It is applied to every
    /// neuron in proportion to the share of neurons that just fired.
    pub global_inhibition: f32,
}

impl BrainConfig {
    /// The scale of the adult fruit fly brain: 139,255 neurons in the
    /// FlyWire reconstruction, and 360 synapses leaving each gives about
    /// 50 million. The fly has roughly 2,000 Kenyon cells a side.
    pub fn fly() -> Self {
        Self {
            neurons: 139_255,
            inputs: 2_040,
            kenyon: 4_000,
            out_degree: 360,
            inputs_per_kenyon: 7,
            inhibitory_share: 0.2,
            seed: 2026,
            steps: 24,
            central_gain: 0.03,
            kenyon_gain: 0.12,
            global_inhibition: 6.0,
        }
    }

    /// Settings for measured wiring, where every synapse has the same
    /// gain and the wiring carries its own inhibition. The neuron count
    /// comes from the file.
    ///
    /// The gain was set on the FlyWire connectome (138,639 neurons). It
    /// sits just below a sharp change: from about 0.013 upward the brain
    /// falls into nearly the same activity whatever it is shown, and the
    /// code stops telling situations apart. No inhibition is added to
    /// what the wiring already has.
    pub fn measured() -> Self {
        Self {
            central_gain: 0.011,
            kenyon_gain: 0.011,
            global_inhibition: 0.0,
            ..Self::fly()
        }
    }

    /// A brain small enough for tests, with the same proportions.
    pub fn tiny() -> Self {
        Self {
            neurons: 3_000,
            inputs: 340,
            kenyon: 600,
            out_degree: 60,
            central_gain: 0.03,
            kenyon_gain: 0.16,
            ..Self::fly()
        }
    }

    fn layout(&self) -> Layout {
        Layout {
            neurons: self.neurons,
            inputs: self.inputs,
            kenyon: self.kenyon,
        }
    }
}

/// What one presentation did, for inspection and tuning.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Activity {
    /// Spikes per neuron per step, over the whole brain.
    pub mean_rate: f32,

    /// Share of Kenyon cells that fired at least once.
    pub kenyon_active: f32,
}

pub struct Brain {
    config: BrainConfig,
    connectome: Arc<Connectome>,

    /// The neurons a situation is presented to.
    input_neurons: Vec<u32>,

    /// Whether each neuron is one of them.
    is_input: Vec<bool>,

    /// For each neuron, its place in the code that is read out, or
    /// `NOT_READ` for the many neurons that are not read.
    readout_slot: Vec<u32>,

    /// Number of neurons read out: the length of a code.
    readout: usize,

    /// Voltage a synapse of strength 1 adds to each target neuron.
    gain: Vec<f32>,

    /// How strongly each input neuron responds to its feature. Unequal
    /// sensitivities make a group of input neurons report the size of a
    /// feature, not just its presence.
    sensitivity: Vec<f32>,

    voltage: Vec<f32>,
    current: Vec<f32>,
    refractory: Vec<u8>,

    last_activity: Activity,
    telemetry: NeuralTrace,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NeuralFrame {
    pub voltage: Vec<f32>,
    pub spikes: Vec<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NeuralTrace {
    pub total_neurons: usize,
    pub total_connections: usize,
    pub neuron_ids: Vec<u32>,
    pub populations: Vec<String>,
    pub synapses: Vec<(usize, usize, i16)>,
    pub frames: Vec<NeuralFrame>,
    pub total_spikes: usize,
    pub mean_rate: f32,
    pub readout_active: f32,
    pub features: Vec<f32>,
    pub threshold: f32,
    pub encoding_ms: u64,
}

impl Brain {
    /// A brain with generated wiring.
    pub fn generate(config: BrainConfig) -> Self {
        static FLY: OnceLock<Arc<Connectome>> = OnceLock::new();
        let generate = || {
            Arc::new(Connectome::generate(&Wiring {
                layout: config.layout(),
                out_degree: config.out_degree,
                inputs_per_kenyon: config.inputs_per_kenyon,
                inhibitory_share: config.inhibitory_share,
                seed: config.seed,
            }))
        };
        let connectome = if config == BrainConfig::fly() {
            FLY.get_or_init(generate).clone()
        } else {
            generate()
        };

        Self::with_connectome(config, connectome)
    }

    /// A brain with measured wiring read from `path`.
    ///
    /// The measured neurons are not sorted into populations, so the
    /// situation is presented to a fixed random set of neurons that have
    /// outgoing synapses, and the code is read from another fixed random
    /// set. `config.neurons` is replaced by the number in the file.
    pub fn from_file(mut config: BrainConfig, path: &Path) -> std::io::Result<Self> {
        let connectome = Connectome::from_file(path)?;

        config.neurons = connectome.layout.neurons;

        let mut random = Random::new(config.seed ^ 0xF17);

        let mut candidates: Vec<u32> = (0..config.neurons as u32)
            .filter(|neuron| !connectome.outgoing(*neuron as usize).0.is_empty())
            .collect();

        // A partial shuffle picks the inputs and then the readout without
        // overlap.
        let wanted = (config.inputs + config.kenyon).min(candidates.len());

        for position in 0..wanted {
            let other = position + random.below((candidates.len() - position) as u32) as usize;

            candidates.swap(position, other);
        }

        config.inputs = config.inputs.min(wanted);
        config.kenyon = wanted - config.inputs;

        let inputs = candidates[..config.inputs].to_vec();
        let readout = candidates[config.inputs..wanted].to_vec();

        let gain = vec![config.central_gain; config.neurons];

        Ok(Self::assemble(
            config,
            Arc::new(connectome),
            inputs,
            readout,
            gain,
        ))
    }

    /// Generated wiring keeps its populations in contiguous ranges.
    fn with_connectome(config: BrainConfig, connectome: Arc<Connectome>) -> Self {
        let kenyon = connectome.layout.kenyon_range();

        let inputs = (0..config.inputs as u32).collect();
        let readout = kenyon.clone().map(|neuron| neuron as u32).collect();

        let gain = (0..config.neurons)
            .map(|neuron| {
                if kenyon.contains(&neuron) {
                    config.kenyon_gain
                } else {
                    config.central_gain
                }
            })
            .collect();

        Self::assemble(config, connectome, inputs, readout, gain)
    }

    fn assemble(
        config: BrainConfig,
        connectome: Arc<Connectome>,
        input_neurons: Vec<u32>,
        readout_neurons: Vec<u32>,
        gain: Vec<f32>,
    ) -> Self {
        let mut random = Random::new(config.seed ^ 0xA11CE);

        let sensitivity = (0..input_neurons.len())
            .map(|_| 0.25 + 0.75 * random.unit())
            .collect();

        let neurons = config.neurons;

        let mut readout_slot = vec![NOT_READ; neurons];

        for (slot, neuron) in readout_neurons.iter().enumerate() {
            readout_slot[*neuron as usize] = slot as u32;
        }

        let mut is_input = vec![false; neurons];

        for neuron in &input_neurons {
            is_input[*neuron as usize] = true;
        }

        Self {
            config,
            connectome,
            input_neurons,
            is_input,
            readout_slot,
            readout: readout_neurons.len(),
            gain,
            sensitivity,
            voltage: vec![0.0; neurons],
            current: vec![0.0; neurons],
            refractory: vec![0; neurons],
            last_activity: Activity::default(),
            telemetry: NeuralTrace::default(),
        }
    }

    pub fn config(&self) -> &BrainConfig {
        &self.config
    }

    pub fn connectome(&self) -> &Connectome {
        &self.connectome
    }

    pub fn last_activity(&self) -> Activity {
        self.last_activity
    }

    /// Length of the code [`Brain::respond`] returns.
    pub fn code_length(&self) -> usize {
        self.readout
    }

    /// Presents a situation and returns how many times each read-out
    /// neuron (each Kenyon cell, in a generated brain) fired.
    pub fn respond(&mut self, features: &[f32; FEATURES]) -> Vec<f32> {
        let started = std::time::Instant::now();
        if self.telemetry.neuron_ids.is_empty() {
            self.prepare_trace();
        }
        self.telemetry.frames.clear();
        self.telemetry.features = features.to_vec();
        let neurons = self.config.neurons;
        let steps = self.config.steps;

        self.voltage.fill(0.0);
        self.current.fill(0.0);
        self.refractory.fill(0);

        let mut counts = vec![0.0_f32; self.readout];
        let mut fired: Vec<u32> = Vec::new();
        let mut total_spikes = 0_usize;

        for _ in 0..steps {
            self.deliver(&fired);
            self.drive(features);

            let inhibition = self.config.global_inhibition * fired.len() as f32 / neurons as f32;

            fired.clear();

            for neuron in 0..neurons {
                let arriving = std::mem::take(&mut self.current[neuron]);

                if self.refractory[neuron] > 0 {
                    self.refractory[neuron] -= 1;

                    continue;
                }

                let voltage = self.voltage[neuron] * DECAY + arriving - inhibition;

                if voltage >= THRESHOLD {
                    self.voltage[neuron] = 0.0;
                    self.refractory[neuron] = REFRACTORY_STEPS;

                    fired.push(neuron as u32);

                    let slot = self.readout_slot[neuron];

                    if slot != NOT_READ {
                        counts[slot as usize] += 1.0;
                    }
                } else {
                    self.voltage[neuron] = voltage.max(FLOOR);
                }
            }

            total_spikes += fired.len();
            self.telemetry.frames.push(NeuralFrame {
                voltage: self
                    .telemetry
                    .neuron_ids
                    .iter()
                    .map(|id| self.voltage[*id as usize])
                    .collect(),
                spikes: self
                    .telemetry
                    .neuron_ids
                    .iter()
                    .map(|id| self.refractory[*id as usize] == REFRACTORY_STEPS)
                    .collect(),
            });
        }

        let active = counts.iter().filter(|count| **count > 0.0).count();

        self.last_activity = Activity {
            mean_rate: total_spikes as f32 / (neurons as f32 * steps as f32),
            kenyon_active: active as f32 / self.readout.max(1) as f32,
        };
        self.telemetry.total_spikes = total_spikes;
        self.telemetry.mean_rate = self.last_activity.mean_rate;
        self.telemetry.readout_active = self.last_activity.kenyon_active;
        self.telemetry.encoding_ms = started.elapsed().as_millis() as u64;

        counts
    }

    pub fn trace(&self) -> &NeuralTrace {
        &self.telemetry
    }

    pub fn inspect(&self, query: &serde_json::Value) -> serde_json::Value {
        let offset = query["offset"]
            .as_u64()
            .unwrap_or_default()
            .min(usize::MAX as u64) as usize;
        let ids = query["neuron_ids"].as_array().cloned().unwrap_or_default();
        let neurons: Vec<_> = ids.iter().take(8).map(|value| {
            let Some(id) = value.as_u64().filter(|id| *id < self.config.neurons as u64) else {
                return serde_json::json!({"error": "Neuron ID outside this network", "id": value});
            };
            let (targets, weights) = self.connectome.outgoing(id as usize);
            serde_json::json!({
                "id": id, "voltage": self.voltage[id as usize],
                "refractory": self.refractory[id as usize],
                "last_tick_spike": self.refractory[id as usize] == REFRACTORY_STEPS,
                "input": self.is_input[id as usize],
                "readout": self.readout_slot[id as usize] != NOT_READ,
                "outgoing_total": targets.len(), "offset": offset,
                "synapses": targets.iter().zip(weights).skip(offset).take(64).map(|(target, weight)| serde_json::json!({"target":target,"weight":weight})).collect::<Vec<_>>(),
            })
        }).collect();
        serde_json::json!({"source":"fly-brain", "total_neurons":self.config.neurons,
            "total_connections":self.connectome.synapses(), "neurons":neurons,
            "sampled_neurons":self.telemetry.neuron_ids, "mean_rate":self.last_activity.mean_rate,
            "readout_active":self.last_activity.kenyon_active})
    }

    fn prepare_trace(&mut self) {
        let mut ids = Vec::new();
        for position in 0..16.min(self.input_neurons.len()) {
            ids.push(self.input_neurons[position * self.input_neurons.len() / 16]);
        }
        let inputs = ids.clone();
        for input in inputs {
            let (targets, _) = self.connectome.outgoing(input as usize);
            for target in targets.iter().take(2).chain(targets.last()) {
                if !ids.contains(target) && ids.len() < 64 {
                    ids.push(*target);
                }
            }
        }
        let mut inhibitory = 0;
        for id in 0..self.config.neurons as u32 {
            let (targets, weights) = self.connectome.outgoing(id as usize);
            if weights.first().is_some_and(|weight| *weight < 0) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
                for target in targets.iter().take(2) {
                    if !ids.contains(target) && ids.len() < 96 {
                        ids.push(*target);
                    }
                }
                inhibitory += 1;
                if inhibitory == 4 {
                    break;
                }
            }
        }
        let connected = ids.clone();
        for id in connected {
            let (targets, _) = self.connectome.outgoing(id as usize);
            for target in targets.iter().take(2) {
                if !ids.contains(target) && ids.len() < 96 {
                    ids.push(*target);
                }
            }
        }
        let populations = ids
            .iter()
            .map(|id| {
                if self.is_input[*id as usize] {
                    "input"
                } else if self.readout_slot[*id as usize] != NOT_READ {
                    "readout"
                } else {
                    "central"
                }
                .to_string()
            })
            .collect();
        let mut synapses = Vec::new();
        for (from, id) in ids.iter().enumerate() {
            let (targets, weights) = self.connectome.outgoing(*id as usize);
            for (target, weight) in targets.iter().zip(weights) {
                if let Some(to) = ids.iter().position(|id| id == target) {
                    synapses.push((from, to, *weight));
                }
            }
        }
        self.telemetry = NeuralTrace {
            total_neurons: self.config.neurons,
            total_connections: self.connectome.synapses(),
            neuron_ids: ids,
            populations,
            synapses,
            threshold: THRESHOLD,
            ..NeuralTrace::default()
        };
    }

    /// Adds the situation's drive to the input neurons. Each feature
    /// drives its own group of them.
    fn drive(&mut self, features: &[f32; FEATURES]) {
        let groups = self.input_neurons.len();

        for (position, &neuron) in self.input_neurons.iter().enumerate() {
            let feature = features[position * FEATURES / groups];

            self.current[neuron as usize] += feature * self.sensitivity[position] * INPUT_DRIVE;
        }
    }

    /// Sends the spikes of the previous step down their synapses.
    ///
    /// Only synapses from input neurons use a target's own gain. Feedback
    /// onto Kenyon cells from the rest of the brain is as weak as any
    /// central synapse, so it modulates their response to the situation
    /// instead of replacing it.
    fn deliver(&mut self, fired: &[u32]) {
        let central_gain = self.config.central_gain;

        for &neuron in fired {
            let (targets, strengths) = self.connectome.outgoing(neuron as usize);

            let from_input = self.is_input[neuron as usize];

            for (&target, &strength) in targets.iter().zip(strengths) {
                let target = target as usize;

                let gain = if from_input {
                    self.gain[target]
                } else {
                    central_gain
                };

                self.current[target] += gain * f32::from(strength);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampled_trace_matches_real_wiring_and_neuron_state() {
        let mut brain = Brain::generate(BrainConfig::tiny());
        let inputs = [1.0; FEATURES];
        let code = brain.respond(&inputs);
        let trace = brain.trace();
        assert_eq!(trace.frames.len(), brain.config.steps as usize);
        assert!(trace.neuron_ids.len() <= 96);
        assert!(!trace.synapses.is_empty());
        assert!(trace.total_spikes > 0);
        assert!(
            trace
                .frames
                .iter()
                .any(|frame| frame.spikes.iter().any(|spike| *spike))
        );
        for &(from, to, weight) in &trace.synapses {
            let (targets, weights) = brain.connectome.outgoing(trace.neuron_ids[from] as usize);
            assert!(
                targets
                    .iter()
                    .zip(weights)
                    .any(|(id, strength)| *id == trace.neuron_ids[to] && *strength == weight)
            );
        }
        for (index, id) in trace.neuron_ids.iter().enumerate() {
            assert_eq!(
                trace.frames.last().unwrap().voltage[index],
                brain.voltage[*id as usize]
            );
            for frame in &trace.frames {
                if frame.spikes[index] {
                    assert_eq!(frame.voltage[index], 0.0);
                }
            }
        }
        let frames = trace.frames.clone();
        assert_eq!(code, brain.respond(&inputs));
        assert_eq!(
            serde_json::to_value(frames).unwrap(),
            serde_json::to_value(&brain.trace().frames).unwrap()
        );
        let inspected =
            brain.inspect(&serde_json::json!({"neuron_ids":[0, 2999, 3000],"offset":0}));
        assert_eq!(
            inspected["neurons"][0]["voltage"],
            serde_json::json!(brain.voltage[0])
        );
        assert!(inspected["neurons"][2]["error"].is_string());
    }

    fn features(set: &[usize]) -> [f32; FEATURES] {
        let mut features = [0.0; FEATURES];

        for &index in set {
            features[index] = 1.0;
        }

        features
    }

    fn similarity(left: &[f32], right: &[f32]) -> f32 {
        let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();

        let length = |code: &[f32]| code.iter().map(|value| value * value).sum::<f32>().sqrt();

        dot / (length(left) * length(right)).max(f32::MIN_POSITIVE)
    }

    #[test]
    fn the_same_situation_always_gives_the_same_code() {
        let mut brain = Brain::generate(BrainConfig::tiny());

        let situation = features(&[0, 2, 3, 13]);

        let first = brain.respond(&situation);

        brain.respond(&features(&[0, 5, 8]));

        assert_eq!(brain.respond(&situation), first);
    }

    #[test]
    fn activity_is_sparse_but_present() {
        let mut brain = Brain::generate(BrainConfig::tiny());

        brain.respond(&features(&[0, 2, 3, 13]));

        let activity = brain.last_activity();

        assert!(activity.kenyon_active > 0.02, "{activity:?}");
        assert!(activity.kenyon_active < 0.6, "{activity:?}");
        assert!(activity.mean_rate < 0.2, "{activity:?}");
    }

    #[test]
    fn similar_situations_get_similar_codes() {
        let mut brain = Brain::generate(BrainConfig::tiny());

        let base = brain.respond(&features(&[0, 2, 3, 13]));
        let near = brain.respond(&features(&[0, 2, 3, 13, 16]));
        let far = brain.respond(&features(&[0, 5, 8, 10, 14]));

        assert!(similarity(&base, &near) > similarity(&base, &far));
        assert!(similarity(&base, &far) < 0.9);
    }

    #[test]
    fn nothing_fires_without_input() {
        let mut brain = Brain::generate(BrainConfig::tiny());

        let silent = brain.respond(&[0.0; FEATURES]);

        assert!(silent.iter().all(|count| *count == 0.0));
    }
}
