use serde::{Deserialize, Serialize};

const INPUTS: usize = 32;
const UNITS: usize = 64;
const BRANCHES: usize = 4;
const SYNAPSES: usize = 8;

/// The most cells the population grows to.
pub const MAX_UNITS: usize = 512;

/// Presentations a cell may go without spiking, once it is at least that
/// old, before it dies and a newly wired cell takes its place.
const SILENT_DEATH: u64 = 20_000;

/// A synapse this weak is retracted and sprouts again onto another input.
const RETRACTED: f32 = 0.025;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Synapse {
    input: usize,
    weight: f32,
    eligibility: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cell {
    branches: Vec<Vec<Synapse>>,
    inhibitory: bool,
    threshold: f32,
    rate: f32,

    /// The presentation it was born at, the last one it spiked in, and
    /// how many times it has spiked.
    #[serde(default)]
    born: u64,
    #[serde(default)]
    last_spike: u64,
    #[serde(default)]
    fired: u64,
}

/// One spiking cell, as the picture shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellView {
    pub id: usize,
    pub inhibitory: bool,
    pub threshold: f32,
    pub rate: f32,

    /// Mean synaptic weight of each dendritic branch.
    pub branches: Vec<f32>,
    pub born: u64,
    pub fired: u64,

    /// Presentations since it last spiked.
    pub silent: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hybrid {
    cells: Vec<Cell>,
    pub presentations: u64,
    pub spikes: u64,
    pub updates: u64,

    /// Structural changes: cells grown and died, synapses sprouted and
    /// retracted.
    pub grown: u64,
    pub died: u64,
    pub sprouted: u64,
    pub retracted: u64,
    seed: u64,
    #[serde(skip)]
    voltage: Vec<f32>,
    #[serde(skip)]
    previous: Vec<bool>,
}

impl Default for Hybrid {
    fn default() -> Self {
        let mut hybrid = Self {
            cells: Vec::new(),
            presentations: 0,
            spikes: 0,
            updates: 0,
            grown: 0,
            died: 0,
            sprouted: 0,
            retracted: 0,
            seed: 42,
            voltage: Vec::new(),
            previous: Vec::new(),
        };
        for id in 0..UNITS {
            let cell = hybrid.new_cell(id);
            hybrid.cells.push(cell);
        }
        hybrid.reset();
        hybrid
    }
}

impl Hybrid {
    fn draw(&mut self) -> u64 {
        if self.seed == 0 {
            self.seed = 42;
        }
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }

    /// A cell with randomly wired dendrites; one in five is inhibitory.
    fn new_cell(&mut self, id: usize) -> Cell {
        let born = self.presentations;
        Cell {
            inhibitory: id % 5 == 0,
            threshold: 1.0,
            rate: 0.1,
            born,
            last_spike: born,
            fired: 0,
            branches: (0..BRANCHES)
                .map(|_| {
                    (0..SYNAPSES)
                        .map(|_| Synapse {
                            input: self.draw() as usize % INPUTS,
                            weight: 0.2 + (self.draw() % 600) as f32 / 1000.0,
                            eligibility: 0.0,
                        })
                        .collect()
                })
                .collect(),
        }
    }

    /// Grows new cells until there are `target`, at most [`MAX_UNITS`].
    /// Returns how many were grown.
    pub fn grow_to(&mut self, target: usize) -> usize {
        let target = target.min(MAX_UNITS);
        let mut grown = 0;
        while self.cells.len() < target {
            let cell = self.new_cell(self.cells.len());
            self.cells.push(cell);
            grown += 1;
        }
        self.grown += grown as u64;
        self.reset();
        grown
    }

    /// Structural turnover: a cell long silent dies and a newly wired one
    /// is born in its place; a synapse at the floor is retracted and
    /// sprouts onto another input. Returns the cells that were replaced.
    pub fn turnover(&mut self) -> Vec<usize> {
        let now = self.presentations;
        let mut replaced = Vec::new();
        for id in 0..self.cells.len() {
            let cell = &self.cells[id];
            if now.saturating_sub(cell.born) > SILENT_DEATH
                && now.saturating_sub(cell.last_spike) > SILENT_DEATH
            {
                let fresh = self.new_cell(id);
                self.cells[id] = fresh;
                replaced.push(id);
            }
        }
        self.died += replaced.len() as u64;
        let mut retracted = 0;
        for id in 0..self.cells.len() {
            for branch in 0..BRANCHES {
                for synapse in 0..SYNAPSES {
                    if self.cells[id].branches[branch][synapse].weight <= RETRACTED {
                        let input = self.draw() as usize % INPUTS;
                        self.cells[id].branches[branch][synapse] = Synapse {
                            input,
                            weight: 0.2,
                            eligibility: 0.0,
                        };
                        retracted += 1;
                    }
                }
            }
        }
        self.retracted += retracted;
        self.sprouted += retracted;
        if !replaced.is_empty() {
            self.reset();
        }
        replaced
    }

    pub fn cells(&self) -> Vec<CellView> {
        self.cells
            .iter()
            .enumerate()
            .map(|(id, cell)| CellView {
                id,
                inhibitory: cell.inhibitory,
                threshold: cell.threshold,
                rate: cell.rate,
                branches: cell
                    .branches
                    .iter()
                    .map(|branch| {
                        branch.iter().map(|s| s.weight).sum::<f32>() / branch.len().max(1) as f32
                    })
                    .collect(),
                born: cell.born,
                fired: cell.fired,
                silent: self.presentations.saturating_sub(cell.last_spike),
            })
            .collect()
    }

    pub fn is_sound(&self) -> bool {
        (UNITS..=MAX_UNITS).contains(&self.cells.len())
            && self.cells.iter().all(|cell| {
                cell.branches.len() == BRANCHES
                    && cell.threshold.is_finite()
                    && (0.6..=2.0).contains(&cell.threshold)
                    && cell.rate.is_finite()
                    && (0.0..=1.0).contains(&cell.rate)
                    && cell.branches.iter().all(|branch| {
                        branch.len() == SYNAPSES
                            && branch.iter().all(|synapse| {
                                synapse.input < INPUTS
                                    && synapse.weight.is_finite()
                                    && (0.02..=1.0).contains(&synapse.weight)
                                    && synapse.eligibility.is_finite()
                            })
                    })
            })
    }
    /// Puts a loaded population back in working order. Cells saved before
    /// spike times were kept count as having just spiked, so that they are
    /// not all taken for long silent and replaced at once.
    pub fn wake(&mut self) {
        let now = self.presentations;
        for cell in &mut self.cells {
            if cell.fired == 0 && cell.last_spike == 0 {
                cell.born = cell.born.min(now);
                cell.last_spike = now;
            }
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        self.voltage = vec![0.0; self.cells.len()];
        self.previous = vec![false; self.cells.len()];
    }

    pub fn units(&self) -> usize {
        self.cells.len()
    }

    pub fn branches(&self) -> usize {
        self.cells.iter().map(|c| c.branches.len()).sum()
    }

    pub fn synapses(&self) -> usize {
        self.cells
            .iter()
            .flat_map(|c| &c.branches)
            .map(Vec::len)
            .sum()
    }

    /// Reduced spiking compartments, not a biophysical neuron simulation.
    pub fn present(&mut self, word: &str, learn: bool) -> Vec<usize> {
        if self.voltage.len() != self.cells.len() || self.previous.len() != self.cells.len() {
            self.reset();
        }
        let mut input = [0.0_f32; INPUTS];
        let mut hash = 2166136261_u32;
        for byte in word.bytes() {
            hash = (hash ^ u32::from(byte)).wrapping_mul(16777619);
            input[hash as usize % INPUTS] = 1.0;
        }
        let mut active = vec![false; self.cells.len()];
        let mut spikes = 0;
        for _ in 0..4 {
            let mut firing = vec![false; self.cells.len()];
            for id in 0..self.cells.len() {
                if self.previous[id] {
                    self.voltage[id] = 0.0;
                    continue;
                }
                let cell = &self.cells[id];
                let dendritic: f32 = cell
                    .branches
                    .iter()
                    .map(|branch| {
                        let drive: f32 = branch.iter().map(|s| input[s.input] * s.weight).sum();
                        (drive - 0.65).max(0.0).tanh()
                    })
                    .sum();
                let recurrent: f32 = (1..=4)
                    .map(|offset| {
                        let source = (id + offset * 7) % self.cells.len();
                        if !self.previous[source] {
                            0.0
                        } else if self.cells[source].inhibitory {
                            -0.16
                        } else {
                            0.04
                        }
                    })
                    .sum();
                self.voltage[id] = (self.voltage[id] * 0.8 + dendritic * 0.35 + recurrent).max(0.0);
                firing[id] = self.voltage[id] >= cell.threshold;
                active[id] |= firing[id];
            }
            if learn {
                spikes += firing.iter().filter(|fired| **fired).count() as u64;
                for (id, cell) in self.cells.iter_mut().enumerate() {
                    if firing[id] {
                        cell.fired += 1;
                        cell.last_spike = self.presentations;
                    }
                    cell.rate += 0.01 * (f32::from(u8::from(firing[id])) - cell.rate);
                    cell.threshold = (cell.threshold + 0.002 * (cell.rate - 0.1)).clamp(0.6, 2.0);
                    for synapse in cell.branches.iter_mut().flatten() {
                        synapse.eligibility *= 0.9;
                    if firing[id] {
                            let correlated = input[synapse.input] > 0.0;
                            synapse.eligibility += if correlated { 0.1 } else { -0.02 };
                            let change = if correlated {
                                0.002 * (1.0 - synapse.weight)
                            } else {
                                -0.0005 * synapse.weight
                            };
                            synapse.weight = (synapse.weight + change).clamp(0.02, 1.0);
                            self.updates += 1;
                        }
                    }
                }
            }
            self.previous = firing;
        }
        let features: Vec<_> = active
            .iter()
            .enumerate()
            .filter_map(|(id, fired)| fired.then_some(id))
            .collect();
        if learn {
            self.presentations += 1;
            self.spikes += spikes;
        }
        features
    }

    pub fn feedback(&mut self, reward: f32) {
        self.apply_credit(reward, &self.credit());
    }

    pub fn credit(&self) -> Vec<f32> {
        self.cells
            .iter()
            .flat_map(|cell| cell.branches.iter().flatten())
            .map(|synapse| synapse.eligibility)
            .collect()
    }

    pub fn apply_credit(&mut self, reward: f32, credit: &[f32]) {
        if credit.len() != self.synapses()
            || !reward.is_finite()
            || credit.iter().any(|trace| !trace.is_finite())
        {
            return;
        }
        let mut credit = credit.iter();
        for cell in &mut self.cells {
            for synapse in cell.branches.iter_mut().flatten() {
                synapse.weight = (synapse.weight
                    + 0.01 * reward * credit.next().copied().unwrap_or_default())
                .clamp(0.02, 1.0);
            }
        }
    }

    pub fn reward_question(&mut self, reward: f32, words: &[String], active: &[usize]) {
        let mut input = [false; INPUTS];
        for word in words {
            let mut hash = 2166136261_u32;
            for byte in word.bytes() {
                hash = (hash ^ u32::from(byte)).wrapping_mul(16777619);
                input[hash as usize % INPUTS] = true;
            }
        }
        for id in active {
            if let Some(cell) = self.cells.get_mut(*id) {
                for synapse in cell.branches.iter_mut().flatten() {
                    if input[synapse.input] {
                        synapse.weight = (synapse.weight + 0.01 * reward * (1.0 - synapse.weight))
                            .clamp(0.02, 1.0);
                        self.updates += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plasticity_is_persistent_and_inference_does_not_train() {
        let mut model = Hybrid::default();
        let before = serde_json::to_value(&model).unwrap();
        for _ in 0..20 {
            model.present("learning", true);
            model.present("memory", true);
        }
        assert!(model.spikes > 0);
        assert!(model.updates > 0);
        assert_ne!(serde_json::to_value(&model).unwrap(), before);
        let mut restored: Hybrid =
            serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
        let trained = serde_json::to_value(&restored).unwrap();
        restored.present("learning", false);
        assert_eq!(serde_json::to_value(&restored).unwrap(), trained);
        restored.feedback(-1.0);
        assert_ne!(serde_json::to_value(&restored).unwrap(), trained);
    }

    #[test]
    fn a_long_silent_cell_dies_and_a_floor_weight_synapse_sprouts_again() {
        let mut model = Hybrid::default();
        model.wake();
        model.presentations = SILENT_DEATH * 3;
        for cell in &mut model.cells {
            cell.last_spike = model.presentations;
        }
        model.cells[7].last_spike = 0;
        model.cells[3].branches[1][2].weight = 0.02;
        let replaced = model.turnover();
        assert_eq!(replaced, vec![7]);
        assert_eq!(model.died, 1);
        assert_eq!(model.cells[7].born, model.presentations);
        assert_eq!(model.cells[3].branches[1][2].weight, 0.2);
        assert_eq!(model.sprouted, 1);
        assert!(model.is_sound());
        assert_eq!(model.grow_to(80), 16);
        assert_eq!(model.grow_to(MAX_UNITS + 10), MAX_UNITS - 80);
        assert!(model.is_sound());
    }

    #[test]
    fn cells_saved_without_spike_times_are_not_all_taken_for_silent() {
        let mut model = Hybrid::default();
        model.presentations = SILENT_DEATH * 3;
        for cell in &mut model.cells {
            cell.born = 0;
            cell.last_spike = 0;
            cell.fired = 0;
        }
        model.wake();
        assert!(model.turnover().is_empty());
    }
}
