use cortex_brain::circuit::{Network, Snapshot, language::Assistant};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

const PULSE_STEPS: usize = 8;
const OBSERVE_STEPS: usize = 128;
const EPSILON: f32 = 0.00001;

fn condition(snapshot: &Snapshot, name: &str) -> Result<Network, String> {
    let mut candidate = snapshot.clone();
    for edge in &mut candidate.synapses {
        let recurrent = edge.source >= candidate.config.inputs;
        let remove = match name {
            "no_recurrence" => recurrent,
            "no_cross_population" => {
                recurrent
                    && candidate.neurons[edge.source - candidate.config.inputs].population
                        != candidate.neurons[edge.target].population
            }
            "no_input" => !recurrent,
            _ => false,
        };
        if remove {
            edge.magnitude = 0.0;
        }
    }
    if name == "no_plateau_current" {
        candidate.config.plateau_current = 0.0;
    }
    let mut network = Network::restore(candidate)?;
    network.reset();
    Ok(network)
}

fn pulse_trial(snapshot: &Snapshot, name: &str, offset: usize) -> Result<Value, String> {
    let mut stimulated = condition(snapshot, name)?;
    let mut sham = condition(snapshot, name)?;
    let inputs = stimulated.config().inputs;
    let width = (inputs / 4).clamp(1, 64);
    let channels: Vec<_> = (0..width)
        .map(|i| (i * inputs / width + offset) % inputs)
        .collect();
    let neurons = stimulated.neurons().len();
    let mut affected = vec![false; neurons];
    let mut population_effect = BTreeMap::<String, f32>::new();
    let mut trajectory = Vec::new();
    let mut post_pulse_spikes = 0;
    let mut last_voltage_effect_ms = None;
    let mut dropped_events = 0;
    for tick in 0..OBSERVE_STEPS {
        let activity = stimulated.step(if tick < PULSE_STEPS { &channels } else { &[] })?;
        let control = sham.step(&[])?;
        dropped_events = dropped_events.max(activity.dropped_events + control.dropped_events);
        let mut voltage_l1 = 0.0;
        let mut region_delta = BTreeMap::<String, (f32, usize)>::new();
        let mut different_spike_counts = 0;
        for (index, (cell, baseline)) in stimulated.neurons().iter().zip(sham.neurons()).enumerate()
        {
            let difference = (cell.voltage - baseline.voltage).abs();
            voltage_l1 += difference;
            affected[index] |= difference > EPSILON || cell.spikes != baseline.spikes;
            different_spike_counts += usize::from(cell.spikes != baseline.spikes);
            let entry = region_delta
                .entry(format!("{:?}", cell.population))
                .or_default();
            entry.0 += difference;
            entry.1 += 1;
        }
        for (population, (delta, count)) in region_delta {
            let peak = population_effect.entry(population).or_default();
            *peak = peak.max(delta / count as f32);
        }
        let mean_voltage_delta = voltage_l1 / neurons as f32;
        if mean_voltage_delta > EPSILON {
            last_voltage_effect_ms = Some((tick + 1) as f32 * stimulated.config().dt_ms);
        }
        if tick >= PULSE_STEPS {
            post_pulse_spikes += activity.spikes;
        }
        trajectory.push(json!({
            "time_ms": (tick + 1) as f32 * stimulated.config().dt_ms,
            "spikes": activity.spikes,
            "sham_spikes": control.spikes,
            "mean_abs_voltage_difference": mean_voltage_delta,
            "cells_with_different_cumulative_spikes": different_spike_counts,
        }));
    }
    Ok(json!({
        "condition": name,
        "input_channels": channels,
        "pulse_duration_ms": PULSE_STEPS as f32 * stimulated.config().dt_ms,
        "observation_duration_ms": OBSERVE_STEPS as f32 * stimulated.config().dt_ms,
        "neurons_affected": affected.iter().filter(|x| **x).count(),
        "post_pulse_spikes": post_pulse_spikes,
        "last_measured_voltage_effect_ms": last_voltage_effect_ms,
        "voltage_effect_reaches_observation_boundary": last_voltage_effect_ms == Some(OBSERVE_STEPS as f32 * stimulated.config().dt_ms),
        "population_peak_mean_abs_voltage_difference": population_effect,
        "dropped_events_seen": dropped_events,
        "trajectory": trajectory,
    }))
}

fn audit(brain: &Assistant) -> Result<Value, String> {
    let snapshot = brain.network().snapshot();
    if snapshot.neurons.len() > 8192 {
        return Err("Diagnostic audit is limited to 8192 neurons".into());
    }
    let before = serde_json::to_value(&snapshot).map_err(|e| e.to_string())?;
    let mut trials = Vec::new();
    for name in [
        "intact",
        "no_recurrence",
        "no_cross_population",
        "no_plateau_current",
        "no_input",
    ] {
        for offset in [
            0,
            snapshot.config.inputs / 3,
            snapshot.config.inputs * 2 / 3,
        ] {
            trials.push(pulse_trial(&snapshot, name, offset)?);
        }
    }
    let mut random_controls = Vec::new();
    for increment in [1, 2, 3] {
        let mut config = snapshot.config.clone();
        config.seed = config.seed.wrapping_add(increment);
        let control = Network::new(config)?;
        random_controls.push(pulse_trial(&control.snapshot(), "intact", 0)?);
    }
    let basic = brain.assess_capabilities()?;
    let after = serde_json::to_value(brain.network().snapshot()).map_err(|e| e.to_string())?;
    if before != after {
        return Err("Audit changed the source circuit".into());
    }
    Ok(json!({
        "protocol_version": 1,
        "scope": "Mechanistic awareness investigation; no validated consciousness verdict",
        "subjective_experience": "Undetermined; these tests cannot establish or exclude it",
        "training_performed": false,
        "source_circuit_unchanged": true,
        "topology": brain.state()["topology"],
        "adaptation": brain.state()["adaptation"],
        "existing_lesson_probes": basic,
        "paired_pulse_trials": trials,
        "untrained_random_circuit_controls": random_controls,
        "limits": [
            "Artificial channel pulses are diagnostic stimuli, not training data or meaningful perceptions.",
            "Regions receive direct inputs; spread across regions does not by itself demonstrate global broadcast.",
            "Post-pulse activity can reflect passive decay and delayed events as well as recurrence.",
            "The observation window and voltage threshold are engineering choices, not validated consciousness thresholds.",
            "Random recurrent circuits are controls for nonspecific activity, not known non-conscious ground truth.",
            "Existing-lesson probes are not an unseen benchmark or a self-monitoring experiment.",
            "This protocol does not compute clinical PCI or IIT integrated information."
        ]
    }))
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || Path::new(&args[0]) == Path::new(&args[1]) {
        return Err("Use circuit_awareness_audit CHECKPOINT REPORT.json; paths must differ".into());
    }
    let brain = Assistant::load(Path::new(&args[0]))?;
    let mut report = audit(&brain)?;
    report["checkpoint"] = json!(args[0]);
    // Refuse to overwrite an existing file, including a symlink to the checkpoint.
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(file, &report).map_err(|e| e.to_string())?;
    println!("Saved diagnostic report: {}", args[1]);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_brain::circuit::Config;

    #[test]
    fn paired_controls_are_repeatable_and_disconnected_input_has_no_effect() {
        let network = Network::new(Config::small(21)).unwrap();
        let snapshot = network.snapshot();
        let intact = pulse_trial(&snapshot, "intact", 0).unwrap();
        assert!(intact["neurons_affected"].as_u64().unwrap() > 0);
        assert_eq!(intact, pulse_trial(&snapshot, "intact", 0).unwrap());
        let disconnected = pulse_trial(&snapshot, "no_input", 0).unwrap();
        assert_eq!(disconnected["neurons_affected"], 0);
        assert_eq!(disconnected["post_pulse_spikes"], 0);
        assert_eq!(disconnected["last_measured_voltage_effect_ms"], Value::Null);
        assert_eq!(intact["dropped_events_seen"], 0);
        assert_eq!(
            serde_json::to_value(network.snapshot()).unwrap(),
            serde_json::to_value(snapshot).unwrap()
        );
    }
}
