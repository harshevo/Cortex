use super::{Config, Population};
use serde::{Deserialize, Serialize};

pub const SOMA: u8 = 255;
pub const MIN_BRANCHES: usize = 2;
pub const MAX_BRANCHES: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    #[serde(default)]
    pub idle_replays: u32,
    pub excitation: f32,
    pub inhibition: f32,
    pub voltage: f32,
    pub plateau_left_ms: f32,
    pub armed: bool,
    #[serde(default = "one")]
    pub gain: f32,
}

fn one() -> f32 {
    1.0
}
impl Default for Branch {
    fn default() -> Self {
        Self {
            idle_replays: 0,
            excitation: 0.0,
            inhibition: 0.0,
            voltage: 0.0,
            plateau_left_ms: 0.0,
            armed: false,
            gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Neuron {
    pub population: Population,
    pub voltage: f32,
    pub excitation: f32,
    pub inhibition: f32,
    pub adaptation: f32,
    pub refractory_left_ms: f32,
    pub branches: Vec<Branch>,
    pub trace: f32,
    #[serde(default)]
    pub fast_trace: f32,
    pub spikes: u64,
    #[serde(default = "one")]
    pub threshold_scale: f32,
}

impl Neuron {
    pub fn new(population: Population) -> Self {
        Self {
            population,
            voltage: 0.0,
            excitation: 0.0,
            inhibition: 0.0,
            adaptation: 0.0,
            refractory_left_ms: 0.0,
            branches: (0..4)
                .map(|_| Branch {
                    armed: true,
                    ..Branch::default()
                })
                .collect(),
            trace: 0.0,
            fast_trace: 0.0,
            spikes: 0,
            threshold_scale: 1.0,
        }
    }

    pub fn reset(&mut self) {
        let threshold_scale = self.threshold_scale;
        let gains: Vec<_> = self
            .branches
            .iter()
            .map(|b| (b.gain, b.idle_replays))
            .collect();
        *self = Self::new(self.population);
        self.threshold_scale = threshold_scale;
        self.branches = gains
            .into_iter()
            .map(|(gain, idle_replays)| Branch {
                gain,
                idle_replays,
                armed: true,
                ..Branch::default()
            })
            .collect();
    }

    pub fn receive(&mut self, branch: u8, magnitude: f32, inhibitory: bool) {
        if (branch as usize) < self.branches.len() && self.population != Population::Inhibitory {
            let target = &mut self.branches[branch as usize];
            if inhibitory {
                target.inhibition += magnitude;
            } else {
                target.excitation += magnitude;
            }
        } else if inhibitory {
            self.inhibition += magnitude;
        } else {
            self.excitation += magnitude;
        }
    }

    pub fn step(&mut self, config: &Config, drive: f32) -> bool {
        let decay = |time: f32| (-config.dt_ms / time).exp();
        self.excitation *= decay(config.excitation_ms);
        self.inhibition *= decay(config.inhibition_ms);
        self.adaptation *= decay(config.adaptation_ms);
        self.trace *= decay(config.trace_ms);
        self.fast_trace *= decay(config.inhibition_ms);
        let mut current = drive + self.excitation - self.inhibition;
        if self.population != Population::Inhibitory {
            for branch in &mut self.branches {
                branch.excitation *= decay(config.excitation_ms);
                branch.inhibition *= decay(config.inhibition_ms);
                branch.voltage = branch.voltage * decay(config.branch_ms)
                    + (branch.excitation - branch.inhibition) * (1.0 - decay(config.branch_ms));
                branch.plateau_left_ms = (branch.plateau_left_ms - config.dt_ms).max(0.0);
                if branch.voltage < config.branch_threshold * 0.7 {
                    branch.armed = true;
                }
                if branch.armed && branch.voltage >= config.branch_threshold {
                    branch.armed = false;
                    branch.plateau_left_ms = config.plateau_ms;
                }
                current += config.branch_coupling
                    * branch.gain
                    * (branch.voltage.max(0.0)
                        + if branch.plateau_left_ms > 0.0 {
                            config.plateau_current
                        } else {
                            0.0
                        });
            }
        }
        if self.refractory_left_ms > 0.0 {
            self.refractory_left_ms = (self.refractory_left_ms - config.dt_ms).max(0.0);
            self.voltage = 0.0;
            return false;
        }
        self.voltage = self.voltage * decay(config.membrane_ms)
            + (current - self.adaptation) * (1.0 - decay(config.membrane_ms));
        if self.voltage < config.threshold * self.threshold_scale {
            return false;
        }
        self.voltage = 0.0;
        self.refractory_left_ms = config.refractory_ms;
        if self.population != Population::Inhibitory {
            self.adaptation += config.adaptation_step;
        }
        self.spikes = self.spikes.saturating_add(1);
        self.trace += 1.0;
        self.fast_trace += 1.0;
        true
    }

    pub fn finite(&self) -> bool {
        (MIN_BRANCHES..=MAX_BRANCHES).contains(&self.branches.len())
            && self.threshold_scale.is_finite()
            && (0.5..=2.0).contains(&self.threshold_scale)
            && [
                self.voltage,
                self.excitation,
                self.inhibition,
                self.adaptation,
                self.refractory_left_ms,
                self.trace,
                self.fast_trace,
            ]
            .iter()
            .all(|v| v.is_finite())
            && self.branches.iter().all(|b| {
                b.gain.is_finite()
                    && (0.1..=4.0).contains(&b.gain)
                    && [b.excitation, b.inhibition, b.voltage, b.plateau_left_ms]
                        .iter()
                        .all(|v| v.is_finite())
            })
    }
}
