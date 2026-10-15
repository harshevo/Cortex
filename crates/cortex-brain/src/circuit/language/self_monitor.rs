use super::super::Population;
use super::*;
use serde_json::{Value, json};

pub const MONITOR_FEATURES: usize = 84;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SelfMonitor {
    pub readout: Readout,
    pub mean: Vec<f32>,
    pub scale: Vec<f32>,
    pub trained_core_signature: u64,
    pub training: Value,
}

pub(super) fn signature(network: &Network) -> u64 {
    network
        .synapses()
        .iter()
        .map(|s| s.magnitude)
        .chain(network.neurons().iter().flat_map(|n| {
            std::iter::once(n.threshold_scale).chain(n.branches.iter().map(|b| b.gain))
        }))
        .fold(1469598103934665603u64, |hash, value| {
            (hash ^ value.to_bits() as u64).wrapping_mul(1099511628211)
        })
}

pub(super) fn state_features(network: &Network) -> Vec<f32> {
    let mut output = Vec::with_capacity(MONITOR_FEATURES);
    for population in [
        Population::Association,
        Population::Episodic,
        Population::Context,
        Population::Action,
        Population::Value,
        Population::Inhibitory,
    ] {
        let cells: Vec<_> = network
            .neurons()
            .iter()
            .filter(|n| n.population == population)
            .collect();
        let count = cells.len().max(1) as f32;
        let mean =
            |f: fn(&super::super::Neuron) -> f32| cells.iter().map(|n| f(n)).sum::<f32>() / count;
        let voltage = mean(|n| n.voltage);
        let fast = mean(|n| n.fast_trace / 10.0);
        let trace = mean(|n| n.trace / 10.0);
        let branches: Vec<_> = cells
            .iter()
            .filter(|n| n.population != Population::Inhibitory)
            .flat_map(|n| &n.branches)
            .collect();
        let branch_count = branches.len().max(1) as f32;
        output.extend([
            voltage,
            (cells
                .iter()
                .map(|n| (n.voltage - voltage).powi(2))
                .sum::<f32>()
                / count)
                .sqrt(),
            fast,
            (cells
                .iter()
                .map(|n| (n.fast_trace / 10.0 - fast).powi(2))
                .sum::<f32>()
                / count)
                .sqrt(),
            trace,
            (cells
                .iter()
                .map(|n| (n.trace / 10.0 - trace).powi(2))
                .sum::<f32>()
                / count)
                .sqrt(),
            mean(|n| n.excitation),
            mean(|n| n.inhibition),
            mean(|n| f32::from(n.refractory_left_ms > 0.0)),
            branches.iter().map(|b| b.voltage).sum::<f32>() / branch_count,
            branches.iter().filter(|b| b.plateau_left_ms > 0.0).count() as f32 / branch_count,
            mean(|n| n.adaptation),
            cells
                .iter()
                .map(|n| n.voltage)
                .reduce(f32::min)
                .unwrap_or(0.0),
            cells
                .iter()
                .map(|n| n.voltage)
                .reduce(f32::max)
                .unwrap_or(0.0),
        ]);
    }
    output
}

impl SelfMonitor {
    pub fn validate(&self) -> Result<(), String> {
        self.readout.validate(MONITOR_FEATURES, 2)?;
        if self.mean.len() != MONITOR_FEATURES
            || self.scale.len() != MONITOR_FEATURES
            || self.mean.iter().any(|x| !x.is_finite() || x.abs() > 1e6)
            || self
                .scale
                .iter()
                .any(|x| !x.is_finite() || !(0.000001..=1e6).contains(x))
        {
            return Err("Invalid self-monitor normalization".into());
        }
        Ok(())
    }
    pub fn predict(&self, network: &Network) -> Result<Value, String> {
        let raw = state_features(network);
        let recent_activity = (0..6).map(|region| raw[2 + region * 14]).sum::<f32>() / 6.0;
        let eligible = recent_activity
            >= self.training["minimum_recent_activity"]
                .as_f64()
                .unwrap_or(0.0) as f32;
        let normalized: Vec<_> = raw
            .iter()
            .zip(&self.mean)
            .zip(&self.scale)
            .map(|((x, m), s)| ((x - m) / s).clamp(-8.0, 8.0))
            .collect();
        let probability = self.readout.probabilities(&normalized)?[1];
        let current = signature(network) == self.trained_core_signature;
        Ok(
            json!({"anomaly_probability":probability,"predicted_intervention":probability>=0.5,"eligible_state":eligible,"calibration_matches_core":current,"method":"trained_state_statistics_readout","scope":"Learned detector over 84 internal activity statistics, validated for post-question states. Inactive states and changed cores need separate validation; this is not a language-head or consciousness result.","training":self.training}),
        )
    }
}

impl Assistant {
    pub fn self_monitor_state(&self) -> Result<Value, String> {
        self.self_monitor
            .as_ref()
            .ok_or("No trained self-monitor is installed")?
            .predict(&self.network)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_features_and_validation_are_bounded() {
        let network = Network::new(Config::small(31)).unwrap();
        let features = state_features(&network);
        assert_eq!(features.len(), MONITOR_FEATURES);
        assert!(features.iter().all(|x| x.is_finite()));
        let mut monitor = SelfMonitor {
            readout: Readout::new(MONITOR_FEATURES, 2).unwrap(),
            mean: vec![0.0; MONITOR_FEATURES],
            scale: vec![1.0; MONITOR_FEATURES],
            trained_core_signature: signature(&network),
            training: Value::Null,
        };
        monitor.validate().unwrap();
        assert_eq!(
            monitor.predict(&network).unwrap()["calibration_matches_core"],
            true
        );
        monitor.training = serde_json::json!({"minimum_recent_activity":0.02});
        assert_eq!(monitor.predict(&network).unwrap()["eligible_state"], false);
        monitor.trained_core_signature ^= 1;
        assert_eq!(
            monitor.predict(&network).unwrap()["calibration_matches_core"],
            false
        );
        monitor.scale[0] = 0.0;
        assert!(monitor.validate().is_err());
    }
}
