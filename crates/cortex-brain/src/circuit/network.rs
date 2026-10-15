use super::{
    Config, Neuron, Population,
    neuron::{Branch, MAX_BRANCHES, MIN_BRANCHES, SOMA},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    io::{BufReader, BufWriter, Write},
    path::Path,
    sync::{Arc, Mutex},
};

/// How many steps the live record keeps for watchers to catch up on.
const LIVE_FRAMES: usize = 4096;

/// What the circuit is doing as it runs, for watching it from another
/// thread: which neurons spiked on each recent step, and every neuron's
/// membrane voltage after the latest one. Frames are numbered by a counter
/// of their own that, unlike the circuit's clock, never resets.
#[derive(Debug, Default)]
pub struct Live {
    pub frame: u64,
    pub frames: VecDeque<(u64, Vec<u32>)>,
    pub voltages: Vec<f32>,
    pub activity: Activity,
    pub populations: Vec<Population>,
}

impl Live {
    /// The frames after `since`, the latest voltages and activity.
    pub fn since(&self, since: u64) -> serde_json::Value {
        let frames: Vec<_> = self.frames.iter().filter(|(n, _)| *n > since).collect();
        serde_json::json!({
            "frame": self.frame,
            "frames": frames,
            // Thousandths of a volt-unit keep the reply small.
            "voltages": self.voltages.iter().map(|v| (v * 1000.0).round() as i32).collect::<Vec<_>>(),
            "activity": self.activity,
            "populations": self.populations,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Synapse {
    pub source: usize,
    pub target: usize,
    pub branch: u8,
    pub magnitude: f32,
    pub delay: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Event {
    target: usize,
    branch: u8,
    magnitude: f32,
    inhibitory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    pub config: Config,
    pub neurons: Vec<Neuron>,
    pub synapses: Vec<Synapse>,
    queues: Vec<Vec<Event>>,
    pub ticks: u64,
    pub dropped_events: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Activity {
    pub ticks: u64,
    pub spikes: usize,
    pub active_neurons: usize,
    pub active_branches: usize,
    #[serde(default)]
    pub dendrites: usize,
    #[serde(default)]
    pub branch_voltage: Vec<f32>,
    #[serde(default)]
    pub branch_gain: Vec<f32>,
    #[serde(default)]
    pub branch_plateaus: Vec<usize>,
    pub mean_voltage: f32,
    pub max_voltage: f32,
    pub pending_events: usize,
    pub dropped_events: u64,
}

#[derive(Clone)]
pub struct Network {
    config: Config,
    neurons: Vec<Neuron>,
    synapses: Vec<Synapse>,
    outgoing: Vec<Vec<usize>>,
    queues: Vec<Vec<Event>>,
    pending: usize,
    ticks: u64,
    dropped_events: u64,
    live: Option<Arc<Mutex<Live>>>,
}

#[derive(Clone)]
pub(crate) struct Rng(pub u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn index(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u32 << 24) as f32
    }
}

impl Network {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        let populations = [
            (Population::Association, config.association),
            (Population::Episodic, config.episodic),
            (Population::Context, config.context),
            (Population::Action, config.action),
            (Population::Value, config.value),
            (Population::Inhibitory, config.inhibitory),
        ];
        let neurons: Vec<_> = populations
            .into_iter()
            .flat_map(|(p, n)| (0..n).map(move |_| Neuron::new(p)))
            .collect();
        let mut ranges = Vec::new();
        let mut offset = 0;
        for (_, count) in populations {
            ranges.push(offset..offset + count);
            offset += count;
        }
        let mut rng = Rng::new(config.seed);
        let mut synapses =
            Vec::with_capacity(neurons.len() * (config.fan_in + config.input_fan_in));
        let delay_steps = (config.max_delay_ms / config.dt_ms).floor() as usize;
        for target in 0..neurons.len() {
            let local = ranges
                .iter()
                .find(|r| r.contains(&target))
                .ok_or("Missing population range.")?;
            let mut sources = HashSet::new();
            for _ in 0..config.fan_in {
                let mut source = if local.len() > 1 && rng.unit() < 0.7 {
                    local.start + rng.index(local.len())
                } else {
                    rng.index(neurons.len())
                };
                let mut attempts = 0;
                while source == target || sources.contains(&source) {
                    source = rng.index(neurons.len());
                    attempts += 1;
                    if attempts > neurons.len() * 4 {
                        source = (0..neurons.len())
                            .find(|i| *i != target && !sources.contains(i))
                            .ok_or("No available presynaptic neuron.")?;
                        break;
                    }
                }
                sources.insert(source);
                let inhibitory = neurons[source].population == Population::Inhibitory;
                synapses.push(Synapse {
                    source: config.inputs + source,
                    target,
                    branch: if inhibitory || neurons[target].population == Population::Inhibitory {
                        SOMA
                    } else {
                        match rng.index(5) {
                            4 => SOMA,
                            branch => branch as u8,
                        }
                    },
                    magnitude: if inhibitory {
                        config.inhibitory_weight
                    } else {
                        config.excitatory_weight
                    } * (0.5 + rng.unit()),
                    delay: 1 + rng.index(delay_steps),
                });
            }
            sources.clear();
            for _ in 0..config.input_fan_in {
                let mut source = rng.index(config.inputs);
                while sources.contains(&source) {
                    source = (source + 1) % config.inputs;
                }
                sources.insert(source);
                synapses.push(Synapse {
                    source,
                    target,
                    branch: if neurons[target].population == Population::Inhibitory {
                        SOMA
                    } else {
                        match rng.index(5) {
                            4 => SOMA,
                            branch => branch as u8,
                        }
                    },
                    magnitude: config.input_weight * (0.5 + rng.unit()),
                    delay: 1 + rng.index(delay_steps),
                });
            }
        }
        Self::restore(Snapshot {
            version: 2,
            config,
            neurons,
            synapses,
            queues: vec![Vec::new(); delay_steps + 1],
            ticks: 0,
            dropped_events: 0,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn neurons(&self) -> &[Neuron] {
        &self.neurons
    }
    pub fn synapses(&self) -> &[Synapse] {
        &self.synapses
    }
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
    pub fn features(&self) -> Vec<f32> {
        self.neurons.iter().map(|n| n.trace / 10.0).collect()
    }

    pub fn reset(&mut self) {
        for neuron in &mut self.neurons {
            neuron.reset();
        }
        for queue in &mut self.queues {
            queue.clear();
        }
        self.pending = 0;
        self.ticks = 0;
        self.dropped_events = 0;
    }

    fn transmit(&mut self, source: usize) {
        let inhibitory = source >= self.config.inputs
            && self.neurons[source - self.config.inputs].population == Population::Inhibitory;
        for &index in &self.outgoing[source] {
            if self.pending >= self.config.max_pending_events {
                self.dropped_events = self.dropped_events.saturating_add(1);
                continue;
            }
            let synapse = &self.synapses[index];
            let slot = ((self.ticks % self.queues.len() as u64) as usize + synapse.delay)
                % self.queues.len();
            self.queues[slot].push(Event {
                target: synapse.target,
                branch: synapse.branch,
                magnitude: synapse.magnitude,
                inhibitory,
            });
            self.pending += 1;
        }
    }

    /// One integration step. Inputs are spike channel indices, never target labels.
    pub fn step(&mut self, inputs: &[usize]) -> Result<Activity, String> {
        if inputs.iter().any(|i| *i >= self.config.inputs) || inputs.len() > self.config.inputs {
            return Err("Invalid external spike channels.".into());
        }
        let mut seen = HashSet::new();
        if !inputs.iter().all(|i| seen.insert(*i)) {
            return Err("An input channel may spike only once per step.".into());
        }
        let slot = (self.ticks % self.queues.len() as u64) as usize;
        let due = std::mem::take(&mut self.queues[slot]);
        self.pending -= due.len();
        for event in due {
            self.neurons[event.target].receive(event.branch, event.magnitude, event.inhibitory);
        }
        for &input in inputs {
            self.transmit(input);
        }
        let mut spikes = Vec::new();
        for (index, neuron) in self.neurons.iter_mut().enumerate() {
            if neuron.step(&self.config, 0.0) {
                spikes.push(index);
            }
        }
        for &index in &spikes {
            self.transmit(self.config.inputs + index);
        }
        self.ticks = self
            .ticks
            .checked_add(1)
            .ok_or("Circuit clock exhausted.")?;
        if self.neurons.iter().any(|n| !n.finite()) {
            return Err("Nonfinite membrane state.".into());
        }
        let activity = self.activity(spikes.len());
        if let Some(live) = &self.live {
            let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
            live.frame += 1;
            let frame = live.frame;
            live.frames
                .push_back((frame, spikes.iter().map(|&i| i as u32).collect()));
            if live.frames.len() > LIVE_FRAMES {
                live.frames.pop_front();
            }
            live.voltages.clear();
            live.voltages.extend(self.neurons.iter().map(|n| n.voltage));
            live.activity = activity.clone();
            if live.populations.len() != self.neurons.len() {
                live.populations = self.neurons.iter().map(|n| n.population).collect();
            }
        }
        Ok(activity)
    }

    /// Starts recording what each step does, for watching it live, and
    /// returns the record. Asked again, returns the same record.
    pub fn watch(&mut self) -> Arc<Mutex<Live>> {
        let activity = self.activity(0);
        let voltages = self.neurons.iter().map(|n| n.voltage).collect();
        let populations = self.neurons.iter().map(|n| n.population).collect();
        Arc::clone(self.live.get_or_insert_with(|| {
            Arc::new(Mutex::new(Live {
                voltages,
                activity,
                populations,
                ..Live::default()
            }))
        }))
    }

    pub fn activity(&self, spikes: usize) -> Activity {
        let slots = self
            .neurons
            .iter()
            .filter(|n| n.population != Population::Inhibitory)
            .map(|n| n.branches.len())
            .max()
            .unwrap_or(0);
        let mut branch_voltage = vec![0.0; slots];
        let mut branch_gain = vec![0.0; slots];
        let mut branch_plateaus = vec![0; slots];
        let mut branch_cells = vec![0; slots];
        let mut dendrites = 0;
        for neuron in self
            .neurons
            .iter()
            .filter(|n| n.population != Population::Inhibitory)
        {
            dendrites += neuron.branches.len();
            for (index, branch) in neuron.branches.iter().enumerate() {
                branch_cells[index] += 1;
                branch_voltage[index] += branch.voltage;
                branch_gain[index] += branch.gain;
                branch_plateaus[index] += usize::from(branch.plateau_left_ms > 0.0);
            }
        }
        for index in 0..slots {
            branch_voltage[index] /= branch_cells[index].max(1) as f32;
            branch_gain[index] /= branch_cells[index].max(1) as f32;
        }
        Activity {
            dendrites,
            branch_voltage,
            branch_gain,
            active_branches: branch_plateaus.iter().sum(),
            branch_plateaus,
            ticks: self.ticks,
            spikes,
            active_neurons: self.neurons.iter().filter(|n| n.trace > 0.05).count(),
            mean_voltage: self.neurons.iter().map(|n| n.voltage).sum::<f32>()
                / self.neurons.len() as f32,
            max_voltage: self.neurons.iter().map(|n| n.voltage).fold(0.0, f32::max),
            pending_events: self.pending,
            dropped_events: self.dropped_events,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: 2,
            config: self.config.clone(),
            neurons: self.neurons.clone(),
            synapses: self.synapses.clone(),
            queues: self.queues.clone(),
            ticks: self.ticks,
            dropped_events: self.dropped_events,
        }
    }

    pub fn restore(mut snapshot: Snapshot) -> Result<Self, String> {
        if snapshot.version == 1 {
            if snapshot.neurons.iter().any(|n| n.branches.len() != 4) {
                return Err("Legacy checkpoints require exactly four branch slots".into());
            }
            for edge in &mut snapshot.synapses {
                if edge.branch == 4 {
                    edge.branch = SOMA;
                }
            }
            for event in snapshot.queues.iter_mut().flatten() {
                if event.branch == 4 {
                    event.branch = SOMA;
                }
            }
        }
        snapshot.synapses.sort_by_key(|edge| edge.target);
        let Snapshot {
            version,
            config,
            neurons,
            synapses,
            queues,
            ticks,
            dropped_events,
        } = snapshot;
        config.validate()?;
        let delay_steps = (config.max_delay_ms / config.dt_ms).floor() as usize;
        if !matches!(version, 1 | 2)
            || neurons.len() != config.neurons()
            || synapses.len() != neurons.len() * (config.fan_in + config.input_fan_in)
            || queues.len() != delay_steps + 1
            || neurons.iter().any(|n| !n.finite())
        {
            return Err("Invalid circuit checkpoint dimensions or state.".into());
        }
        let expected: Vec<_> = [
            (Population::Association, config.association),
            (Population::Episodic, config.episodic),
            (Population::Context, config.context),
            (Population::Action, config.action),
            (Population::Value, config.value),
            (Population::Inhibitory, config.inhibitory),
        ]
        .into_iter()
        .flat_map(|(p, n)| std::iter::repeat_n(p, n))
        .collect();
        if neurons
            .iter()
            .zip(&expected)
            .any(|(n, p)| n.population != *p)
        {
            return Err("Invalid circuit population ordering.".into());
        }
        let pending = queues.iter().map(Vec::len).sum::<usize>();
        if pending > config.max_pending_events
            || queues.iter().flatten().any(|e| {
                e.target >= neurons.len()
                    || (e.branch != SOMA
                        && (e.branch as usize >= neurons[e.target].branches.len()
                            || neurons[e.target].population == Population::Inhibitory))
                    || !e.magnitude.is_finite()
                    || e.magnitude < 0.0
                    || e.magnitude > 15.0
            })
        {
            return Err("Invalid delayed events.".into());
        }
        let mut outgoing = vec![Vec::new(); config.inputs + neurons.len()];
        let mut incoming = vec![HashSet::new(); neurons.len()];
        for (index, s) in synapses.iter().enumerate() {
            if s.source >= outgoing.len()
                || s.target >= neurons.len()
                || s.source == config.inputs + s.target
                || (s.branch != SOMA && s.branch as usize >= neurons[s.target].branches.len())
                || (neurons[s.target].population == Population::Inhibitory && s.branch != SOMA)
                || s.delay == 0
                || s.delay > delay_steps
                || !s.magnitude.is_finite()
                || !(0.0..=15.0).contains(&s.magnitude)
                || !incoming[s.target].insert(s.source)
            {
                return Err("Invalid circuit connection.".into());
            }
            outgoing[s.source].push(index);
        }
        if incoming
            .iter()
            .any(|n| n.len() != config.fan_in + config.input_fan_in)
        {
            return Err("Invalid incoming connectivity.".into());
        }
        for sources in &incoming {
            if sources.iter().filter(|s| **s < config.inputs).count() != config.input_fan_in {
                return Err("Invalid input fan-in or inhibitory compartment.".into());
            }
        }
        Ok(Self {
            config,
            neurons,
            synapses,
            outgoing,
            queues,
            pending,
            ticks,
            dropped_events,
            live: None,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        {
            let mut writer = BufWriter::new(temp.as_file_mut());
            serde_json::to_writer(&mut writer, &self.snapshot()).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;
        }
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 256 << 20 {
            return Err("Circuit checkpoint exceeds 256 MiB.".into());
        }
        Self::restore(serde_json::from_reader(BufReader::new(file)).map_err(|e| e.to_string())?)
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    #[test]
    fn dendrite_telemetry_excludes_inhibitory_compartments() {
        let config = Config::small(17);
        let mut network = Network::new(config.clone()).unwrap();
        for neuron in &mut network.neurons {
            for (index, branch) in neuron.branches.iter_mut().enumerate() {
                branch.voltage = if neuron.population == Population::Inhibitory {
                    100.0
                } else {
                    index as f32 * 0.25
                };
                branch.gain = 1.5;
                branch.plateau_left_ms = if index == 2 { 2.0 } else { 0.0 };
            }
        }
        let activity = network.activity(0);
        let excitatory = config.neurons() - config.inhibitory;
        assert_eq!(activity.dendrites, excitatory * 4);
        assert_eq!(activity.branch_voltage, [0.0, 0.25, 0.5, 0.75]);
        assert_eq!(activity.branch_gain, [1.5; 4]);
        assert_eq!(activity.branch_plateaus, [0, 0, excitatory, 0]);
        assert_eq!(activity.active_branches, excitatory);
        let serialized = serde_json::to_value(&activity).unwrap();
        assert_eq!(serialized["dendrites"], excitatory * 4);
    }
}

impl Network {
    pub(crate) fn detached(&self) -> Self {
        let mut copy = self.clone();
        copy.live = None;
        copy
    }

    pub(crate) fn adopt_live(&mut self, previous: &Network) {
        self.live = previous.live.clone();
        if let Some(live) = &self.live {
            let mut live = live.lock().unwrap_or_else(|e| e.into_inner());
            live.frames.clear();
            live.voltages = self.neurons.iter().map(|n| n.voltage).collect();
            live.populations = self.neurons.iter().map(|n| n.population).collect();
            live.activity = self.activity(0);
        }
    }

    pub(crate) fn adapt_connections(&mut self, evidence: &[Vec<f32>], rate: f32) -> (usize, usize) {
        let mut stronger = 0;
        let mut weaker = 0;
        for synapse in &mut self.synapses {
            if synapse.source < self.config.inputs {
                continue;
            }
            let source = synapse.source - self.config.inputs;
            let correlation = evidence
                .iter()
                .map(|v| v[source] * v[synapse.target])
                .sum::<f32>()
                / evidence.len().max(1) as f32;
            let change = rate * correlation.clamp(-1.0, 1.0);
            let before = synapse.magnitude;
            synapse.magnitude = (before * (1.0 + change)).clamp(0.00001, 15.0);
            stronger += usize::from(synapse.magnitude > before);
            weaker += usize::from(synapse.magnitude < before);
        }
        for (index, neuron) in self.neurons.iter_mut().enumerate() {
            if neuron.population == Population::Inhibitory {
                continue;
            }
            let signal =
                evidence.iter().map(|v| v[index]).sum::<f32>() / evidence.len().max(1) as f32;
            for branch in &mut neuron.branches {
                branch.gain =
                    (branch.gain * (1.0 + rate * signal.clamp(-1.0, 1.0))).clamp(0.1, 4.0);
            }
        }
        (stronger, weaker)
    }

    pub(crate) fn remodel_branches(
        &mut self,
        evidence: &[Vec<f32>],
        budget: usize,
    ) -> (usize, usize) {
        let mut grown = 0;
        let mut pruned = 0;
        for index in 0..self.neurons.len() {
            if grown + pruned >= budget || self.neurons[index].population == Population::Inhibitory
            {
                continue;
            }
            let incoming = self.synapses.partition_point(|s| s.target < index)
                ..self.synapses.partition_point(|s| s.target <= index);
            let count = self.neurons[index].branches.len();
            let mut occupancy = vec![0usize; count];
            let mut utility = vec![0.0f32; count];
            for edge in self.synapses[incoming.clone()]
                .iter()
                .filter(|s| s.branch != SOMA)
            {
                occupancy[edge.branch as usize] += 1;
                let source_activity = if edge.source < self.config.inputs {
                    1.0
                } else {
                    evidence
                        .iter()
                        .map(|v| v[edge.source - self.config.inputs].abs())
                        .sum::<f32>()
                        / evidence.len().max(1) as f32
                };
                utility[edge.branch as usize] += edge.magnitude * source_activity;
            }
            let activity =
                evidence.iter().map(|v| v[index].abs()).sum::<f32>() / evidence.len().max(1) as f32;
            for (slot, branch) in self.neurons[index].branches.iter_mut().enumerate() {
                branch.idle_replays =
                    if occupancy[slot] == 0 || (occupancy[slot] <= 1 && utility[slot] < 0.001) {
                        branch.idle_replays.saturating_add(1)
                    } else {
                        0
                    };
            }
            let unused = self.neurons[index]
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.idle_replays >= 3)
                .min_by_key(|(slot, _)| occupancy[*slot])
                .map(|(slot, _)| slot);
            if count > MIN_BRANCHES
                && let Some(slot) = unused
            {
                self.neurons[index].branches.remove(slot);
                for edge in self.synapses[incoming.clone()]
                    .iter_mut()
                    .filter(|s| s.branch != SOMA)
                {
                    edge.branch = if edge.branch as usize == slot {
                        SOMA
                    } else if edge.branch as usize > slot {
                        edge.branch - 1
                    } else {
                        edge.branch
                    };
                }
                for event in self
                    .queues
                    .iter_mut()
                    .flatten()
                    .filter(|e| e.target == index && e.branch != SOMA)
                {
                    event.branch = if event.branch as usize == slot {
                        SOMA
                    } else if event.branch as usize > slot {
                        event.branch - 1
                    } else {
                        event.branch
                    };
                }
                pruned += 1;
                continue;
            }
            if count >= MAX_BRANCHES || activity < 0.005 {
                continue;
            }
            let busiest = occupancy
                .iter()
                .enumerate()
                .max_by_key(|(_, n)| **n)
                .map(|(slot, _)| slot)
                .unwrap_or(0);
            if occupancy[busiest] < 8 {
                continue;
            }
            if let Some(edge) = self.synapses[incoming]
                .iter_mut()
                .filter(|s| s.branch as usize == busiest)
                .min_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
            {
                edge.branch = count as u8;
                let gain = self.neurons[index].branches[busiest].gain;
                self.neurons[index].branches.push(Branch {
                    gain,
                    armed: true,
                    ..Branch::default()
                });
                grown += 1;
            }
        }
        (grown, pruned)
    }

    pub(crate) fn grow_association(&mut self, count: usize) -> Result<usize, String> {
        if count == 0 || count > 16 || self.neurons.len() + count > 8192 {
            return Err("Growth exceeds the per-cycle or neuron budget.".into());
        }
        let position = self.config.association;
        let mut config = self.config.clone();
        config.association += count;
        config.validate()?;
        let seed = Network::new(config.clone())?;
        let mut snapshot = self.snapshot();
        snapshot.config = config;
        snapshot.neurons.splice(
            position..position,
            (0..count).map(|_| Neuron::new(Population::Association)),
        );
        for synapse in &mut snapshot.synapses {
            if synapse.target >= position {
                synapse.target += count;
            }
            if synapse.source >= self.config.inputs + position {
                synapse.source += count;
            }
        }
        for events in &mut snapshot.queues {
            for event in events {
                if event.target >= position {
                    event.target += count;
                }
            }
        }
        snapshot.synapses.extend(
            seed.synapses
                .into_iter()
                .filter(|s| (position..position + count).contains(&s.target))
                .map(|mut s| {
                    s.magnitude *= 0.1;
                    s
                }),
        );
        // Replace weak outgoing routes so newborn cells can affect existing cells.
        for offset in 0..count {
            let target = (position + count + offset) % snapshot.neurons.len();
            if let Some(edge) = snapshot
                .synapses
                .iter_mut()
                .filter(|s| s.target == target && s.source >= self.config.inputs)
                .min_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
            {
                edge.source = self.config.inputs + position + offset;
                edge.magnitude = 0.001;
                edge.branch = if snapshot.neurons[target].population == Population::Inhibitory {
                    SOMA
                } else {
                    (offset % snapshot.neurons[target].branches.len()) as u8
                };
            }
        }
        *self = Network::restore(snapshot)?;
        Ok(position)
    }
}

#[cfg(test)]
mod structural_tests {
    use super::*;
    #[test]
    fn branch_growth_pruning_reset_and_snapshot_are_valid() {
        let mut snapshot = Network::new(Config::small(31)).unwrap().snapshot();
        for edge in snapshot.synapses.iter_mut().filter(|s| s.target == 0) {
            edge.branch = 0;
        }
        let mut net = Network::restore(snapshot).unwrap();
        let mut row = vec![0.0; net.neurons.len()];
        row[0] = 1.0;
        let evidence = vec![row];
        assert_eq!(net.remodel_branches(&evidence, 1), (1, 0));
        assert_eq!(net.neurons[0].branches.len(), 5);
        net.remodel_branches(&evidence, 1);
        let (_, pruned) = net.remodel_branches(&evidence, 1);
        assert_eq!(pruned, 1);
        let count = net.neurons[0].branches.len();
        net.reset();
        assert_eq!(net.neurons[0].branches.len(), count);
        let restored = Network::restore(net.snapshot()).unwrap();
        assert_eq!(restored.neurons[0].branches.len(), count);
        assert!(
            restored.synapses.iter().all(|s| s.branch == SOMA
                || (s.branch as usize) < restored.neurons[s.target].branches.len())
        );
        for _ in 0..30 {
            net.remodel_branches(&evidence, 1);
        }
        assert!((MIN_BRANCHES..=MAX_BRANCHES).contains(&net.neurons[0].branches.len()));
    }
    #[test]
    fn legacy_soma_four_migrates_without_becoming_a_fifth_branch() {
        let original = Network::new(Config::small(17)).unwrap();
        let mut legacy = original.snapshot();
        legacy.version = 1;
        for edge in &mut legacy.synapses {
            if edge.branch == SOMA {
                edge.branch = 4;
            }
        }
        let restored = Network::restore(legacy).unwrap();
        assert_eq!(restored.synapses, original.synapses);
        assert_eq!(restored.snapshot().version, 2);
    }
}
