use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Population {
    Association,
    Episodic,
    Context,
    Action,
    Value,
    Inhibitory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub inputs: usize,
    pub association: usize,
    pub episodic: usize,
    pub context: usize,
    pub action: usize,
    pub value: usize,
    pub inhibitory: usize,
    pub fan_in: usize,
    pub input_fan_in: usize,
    pub dt_ms: f32,
    pub membrane_ms: f32,
    pub excitation_ms: f32,
    pub inhibition_ms: f32,
    pub branch_ms: f32,
    pub adaptation_ms: f32,
    pub trace_ms: f32,
    pub refractory_ms: f32,
    pub plateau_ms: f32,
    pub max_delay_ms: f32,
    pub threshold: f32,
    pub branch_threshold: f32,
    pub branch_coupling: f32,
    pub plateau_current: f32,
    pub adaptation_step: f32,
    pub excitatory_weight: f32,
    pub inhibitory_weight: f32,
    pub input_weight: f32,
    pub max_neurons: usize,
    pub max_synapses: usize,
    pub max_pending_events: usize,
    pub byte_budget: usize,
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            inputs: 256,
            association: 640,
            episodic: 384,
            context: 256,
            action: 128,
            value: 64,
            inhibitory: 320,
            fan_in: 48,
            input_fan_in: 8,
            dt_ms: 1.0,
            membrane_ms: 20.0,
            excitation_ms: 5.0,
            inhibition_ms: 10.0,
            branch_ms: 20.0,
            adaptation_ms: 200.0,
            trace_ms: 300.0,
            refractory_ms: 2.0,
            plateau_ms: 15.0,
            max_delay_ms: 5.0,
            threshold: 1.0,
            branch_threshold: 1.0,
            branch_coupling: 0.25,
            plateau_current: 0.8,
            adaptation_step: 0.08,
            excitatory_weight: 0.25,
            inhibitory_weight: 0.8,
            input_weight: 2.5,
            max_neurons: 1_000_000,
            max_synapses: 150_000,
            max_pending_events: 250_000,
            byte_budget: 128 << 20,
            seed: 0x3d4b_913a_716c_2e05,
        }
    }
}

impl Config {
    pub fn small(seed: u64) -> Self {
        Self {
            inputs: 64,
            association: 64,
            episodic: 24,
            context: 24,
            action: 12,
            value: 4,
            inhibitory: 32,
            fan_in: 24,
            input_fan_in: 8,
            seed,
            ..Self::default()
        }
    }

    pub fn neurons(&self) -> usize {
        [
            self.association,
            self.episodic,
            self.context,
            self.action,
            self.value,
            self.inhibitory,
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.inputs == 0
            || self.inputs > 4096
            || self.neurons() == 0
            || self.neurons() > self.max_neurons.min(1_000_000)
        {
            return Err("Circuit input/neuron count exceeds its declared limits.".into());
        }
        if self.fan_in > 256
            || self.input_fan_in > self.inputs
            || self.fan_in + self.input_fan_in > 256
            || self.fan_in >= self.neurons()
        {
            return Err(
                "Fan-in must fit the population, input channels and 256-connection cap.".into(),
            );
        }
        for value in [
            self.dt_ms,
            self.membrane_ms,
            self.excitation_ms,
            self.inhibition_ms,
            self.branch_ms,
            self.adaptation_ms,
            self.trace_ms,
            self.refractory_ms,
            self.plateau_ms,
            self.max_delay_ms,
            self.threshold,
            self.branch_threshold,
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err("Circuit times and thresholds must be finite and positive.".into());
            }
        }
        if !(0.1..=1.0).contains(&self.dt_ms)
            || self.max_delay_ms > 100.0
            || self.max_delay_ms < self.dt_ms
            || self.dt_ms
                > self
                    .excitation_ms
                    .min(self.inhibition_ms)
                    .min(self.membrane_ms)
                    .min(self.branch_ms)
        {
            return Err("Invalid integration step or transmission delay.".into());
        }
        for value in [
            self.branch_coupling,
            self.plateau_current,
            self.adaptation_step,
            self.excitatory_weight,
            self.inhibitory_weight,
            self.input_weight,
        ] {
            if !value.is_finite() || !(0.0..=10.0).contains(&value) {
                return Err(
                    "Circuit magnitudes must be finite, nonnegative and at most 10.".into(),
                );
            }
        }
        let synapses = self
            .neurons()
            .checked_mul(self.fan_in + self.input_fan_in)
            .ok_or("Synapse budget overflow.")?;
        if synapses > self.max_synapses.min(64_000_000)
            || self.max_pending_events == 0
            || self.max_pending_events > 4_000_000
        {
            return Err("Synapse or queued-event limit exceeded.".into());
        }
        // Conservative allocation estimate includes neurons, up to eight branches,
        // adjacency, traces, pending events, and checkpoint working space.
        let bytes = self
            .neurons()
            .checked_mul(512)
            .and_then(|n| synapses.checked_mul(96).and_then(|s| n.checked_add(s)))
            .and_then(|n| {
                self.max_pending_events
                    .checked_mul(48)
                    .and_then(|q| n.checked_add(q))
            })
            .and_then(|n| n.checked_mul(2))
            .ok_or("Circuit byte budget overflow.")?;
        if bytes > self.byte_budget || self.byte_budget > 8usize << 30 {
            return Err("The circuit exceeds its byte budget.".into());
        }
        Ok(())
    }
}
