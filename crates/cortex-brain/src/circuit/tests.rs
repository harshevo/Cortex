use super::*;
use super::{encoding::present, network::Rng, readout::Readout};

#[test]
fn passive_membrane_decays_without_spiking() {
    let config = Config::small(1);
    let mut neuron = Neuron::new(Population::Association);
    neuron.voltage = 0.8;
    for _ in 0..20 {
        assert!(!neuron.step(&config, 0.0));
    }
    assert!((neuron.voltage - 0.8 * (-1.0f32).exp()).abs() < 1e-5);
}

#[test]
fn spikes_reset_and_enforce_refractory_period() {
    let config = Config::small(1);
    let mut neuron = Neuron::new(Population::Association);
    assert!(neuron.step(&config, 30.0));
    assert_eq!(neuron.voltage, 0.0);
    assert!(neuron.adaptation > 0.0);
    for _ in 0..2 {
        assert!(!neuron.step(&config, 30.0));
    }
    assert!(neuron.step(&config, 30.0));
}

#[test]
fn branches_are_specific_and_plateaus_expire() {
    let config = Config::small(1);
    let mut neuron = Neuron::new(Population::Association);
    neuron.receive(2, 30.0, false);
    for _ in 0..5 {
        neuron.step(&config, 0.0);
    }
    assert!(neuron.branches[2].plateau_left_ms > 0.0);
    assert_eq!(neuron.branches[1].voltage, 0.0);
    for _ in 0..500 {
        neuron.step(&config, 0.0);
    }
    assert_eq!(neuron.branches[2].plateau_left_ms, 0.0);
    assert!(neuron.branches[2].voltage < 0.001);
}

#[test]
fn inhibitory_input_reduces_membrane_response() {
    let config = Config::small(1);
    let mut a = Neuron::new(Population::Association);
    let mut b = a.clone();
    a.receive(4, 2.0, false);
    b.receive(4, 2.0, false);
    b.receive(4, 3.0, true);
    for _ in 0..10 {
        a.step(&config, 0.0);
        b.step(&config, 0.0);
    }
    assert!(b.voltage < a.voltage);
}

#[test]
fn half_millisecond_passive_integration_matches() {
    let a = Config::small(1);
    let b = Config {
        dt_ms: 0.5,
        ..a.clone()
    };
    let mut x = Neuron::new(Population::Association);
    x.voltage = 0.9;
    let mut y = x.clone();
    for _ in 0..100 {
        x.step(&a, 0.2);
    }
    for _ in 0..200 {
        y.step(&b, 0.2);
    }
    assert!((x.voltage - y.voltage).abs() < 1e-5);
}

#[test]
fn seeded_wiring_is_sparse_unique_and_deterministic() {
    let a = Network::new(Config::small(3)).unwrap();
    let b = Network::new(Config::small(3)).unwrap();
    assert_eq!(a.synapses(), b.synapses());
    assert_eq!(a.synapses().len(), 160 * 32);
    assert!(a.synapses().iter().all(|s| s.delay >= 1 && s.delay <= 5));
    let mut bad = a.snapshot();
    bad.synapses[0].delay = 0;
    assert!(Network::restore(bad).is_err());
}

#[test]
fn input_is_delayed_and_duplicate_channels_are_rejected() {
    let mut n = Network::new(Config::small(7)).unwrap();
    assert!(n.step(&[0, 0]).is_err());
    assert_eq!(n.ticks(), 0);
    n.step(&[0]).unwrap();
    assert!(
        n.neurons()
            .iter()
            .all(|n| n.excitation == 0.0 && n.branches.iter().all(|b| b.excitation == 0.0))
    );
    for _ in 0..5 {
        n.step(&[]).unwrap();
    }
    assert!(
        n.neurons()
            .iter()
            .any(|n| n.excitation > 0.0 || n.branches.iter().any(|b| b.excitation > 0.0))
    );
}

#[test]
fn snapshot_preserves_pending_events_and_exact_continuation() {
    let mut a = Network::new(Config::small(13)).unwrap();
    for i in 0..25 {
        a.step(&[i % 64]).unwrap();
    }
    let bytes = serde_json::to_vec(&a.snapshot()).unwrap();
    let mut b = Network::restore(serde_json::from_slice(&bytes).unwrap()).unwrap();
    for i in 0..100 {
        a.step(&[i % 64]).unwrap();
        b.step(&[i % 64]).unwrap();
        assert_eq!(a.neurons(), b.neurons());
    }
    assert_eq!(
        serde_json::to_vec(&a.snapshot()).unwrap(),
        serde_json::to_vec(&b.snapshot()).unwrap()
    );
}

#[test]
fn event_cap_and_configuration_budgets_are_enforced() {
    let mut n = Network::new(Config {
        max_pending_events: 1,
        ..Config::small(1)
    })
    .unwrap();
    let activity = n.step(&[0]).unwrap();
    assert_eq!(activity.pending_events, 1);
    assert!(activity.dropped_events > 0);
    assert!(
        Network::new(Config {
            byte_budget: 100,
            ..Config::small(1)
        })
        .is_err()
    );
    assert!(
        Network::new(Config {
            dt_ms: f32::NAN,
            ..Config::small(1)
        })
        .is_err()
    );
    assert!(
        Network::new(Config {
            input_fan_in: usize::MAX,
            ..Config::small(1)
        })
        .is_err()
    );
}

#[test]
fn burst_recovers_instead_of_running_away() {
    for seed in 1..=10 {
        let mut n = Network::new(Config::small(seed)).unwrap();
        let all: Vec<_> = (0..64).collect();
        for i in 0..1200 {
            let activity = n.step(if i < 100 { &all } else { &[] }).unwrap();
            assert_eq!(activity.dropped_events, 0);
        }
        assert!(n.neurons().iter().all(Neuron::finite));
        assert!(n.features().iter().all(|f| *f < 0.1));
    }
}

#[test]
fn reward_alone_trains_output_synapses() {
    let mut n = Network::new(Config::small(17)).unwrap();
    let x = [
        present(&mut n, "red").unwrap(),
        present(&mut n, "blue").unwrap(),
    ];
    assert!(x[0].iter().any(|f| *f > 0.0));
    assert_ne!(x[0], x[1]);
    let mut model = Readout::new(160, 2).unwrap();
    let mut rng = Rng::new(23);
    for trial in 0..2000 {
        let cue = trial % 2;
        let action = model.sample(&x[cue], &mut rng).unwrap();
        let reward = if action == cue { 1.0 } else { 0.0 };
        model.reinforce(&x[cue], action, reward - 0.5, 0.1).unwrap();
    }
    assert_eq!(model.predict(&x[0]).unwrap().0, 0);
    assert_eq!(model.predict(&x[1]).unwrap().0, 1);
    assert!(model.predict(&x[0]).unwrap().1 > 0.9);
    assert!(model.predict(&x[1]).unwrap().1 > 0.9);
}

#[test]
fn learned_intrinsic_parameters_survive_reset_and_checkpoint() {
    let mut n = Network::new(Config::small(15)).unwrap().snapshot();
    n.neurons[0].threshold_scale = 0.7;
    n.neurons[0].branches[2].gain = 1.6;
    let mut network = Network::restore(n).unwrap();
    network.reset();
    assert_eq!(network.neurons()[0].threshold_scale, 0.7);
    assert_eq!(network.neurons()[0].branches[2].gain, 1.6);
    let mut bad = network.snapshot();
    bad.neurons[0].branches[2].gain = f32::NAN;
    assert!(Network::restore(bad).is_err());
}
