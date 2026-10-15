use cortex_brain::circuit::{Config, Network, encoding, readout::Readout};
use serde_json::json;
fn random(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 40) as f32 / (1u32 << 24) as f32
}
fn sample(p: &[f32], r: f32) -> usize {
    let mut sum = 0.0;
    for (i, v) in p.iter().enumerate() {
        sum += v;
        if r < sum {
            return i;
        }
    }
    p.len() - 1
}
fn main() -> Result<(), String> {
    let mut results = Vec::new();
    let mut reversals = Vec::new();
    for seed in 1..=10 {
        let mut n = Network::new(Config::small(seed))?;
        let mut immediate = Vec::new();
        let mut delayed = Vec::new();
        for cue in ["red", "blue", "green", "yellow"] {
            immediate.push(encoding::present(&mut n, cue)?);
            for _ in 0..200 {
                n.step(&[])?;
            }
            delayed.push(n.features());
        }
        for (mode, x) in [("immediate", &immediate), ("200ms_spike_trace", &delayed)] {
            for control in [
                "reward_learning",
                "frozen",
                "shuffled_reward",
                "memory_erased",
            ] {
                let mut model = Readout::new(160, 4)?;
                let mut rng = seed * 71 + 1;
                for trial in 0..6000 {
                    let cue = trial % 4;
                    let zero = vec![0.0; 160];
                    let features = if control == "memory_erased" {
                        &zero
                    } else {
                        &x[cue]
                    };
                    let action = sample(&model.probabilities(features)?, random(&mut rng));
                    let reward = if control == "shuffled_reward" {
                        f32::from(random(&mut rng) < 0.25)
                    } else {
                        f32::from(action == cue)
                    };
                    if control != "frozen" {
                        model.reinforce(features, action, reward - 0.25, 0.1)?;
                    }
                }
                let correct = (0..4)
                    .filter(|&i| {
                        let zero = vec![0.0; 160];
                        model
                            .predict(if control == "memory_erased" {
                                &zero
                            } else {
                                &x[i]
                            })
                            .unwrap()
                            .0
                            == i
                    })
                    .count();
                if control == "reward_learning" && mode == "immediate" {
                    let frozen = model.clone();
                    let mut recovered_at = None;
                    for trial in 0..6000 {
                        let cue = trial % 4;
                        let target = (cue + 1) % 4;
                        let action = sample(&model.probabilities(&x[cue])?, random(&mut rng));
                        model.reinforce(
                            &x[cue],
                            action,
                            f32::from(action == target) - 0.25,
                            0.1,
                        )?;
                        if (trial + 1) % 100 == 0
                            && recovered_at.is_none()
                            && (0..4).all(|cue| model.predict(&x[cue]).unwrap().0 == (cue + 1) % 4)
                        {
                            recovered_at = Some(trial + 1);
                        }
                    }
                    reversals.push(json!({"seed":seed,"recovered_at_all_four":recovered_at,"new_mapping_correct":(0..4).filter(|&cue|model.predict(&x[cue]).unwrap().0==(cue+1)%4).count(),"old_mapping_retained":(0..4).filter(|&cue|model.predict(&x[cue]).unwrap().0==cue).count(),"frozen_new_mapping_correct":(0..4).filter(|&cue|frozen.predict(&x[cue]).unwrap().0==(cue+1)%4).count()}));
                }
                results.push(
                    json!({"seed":seed,"mode":mode,"control":control,"correct":correct,"total":4}),
                );
            }
        }
    }
    let mut expanded = Network::new(Config::default())?;
    let mut spikes = 0;
    for i in 0..1200 {
        let input: Vec<_> = if i < 100 {
            (0..256).collect()
        } else {
            Vec::new()
        };
        spikes += expanded.step(&input)?.spikes;
    }
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"Frozen recurrence; reward-trained output weights. Delay retained by 300 ms filtered spike traces, not established persistent working memory.","results":results,"reversals":reversals,"expanded":{"neurons":expanded.neurons().len(),"synapses":expanded.synapses().len(),"spikes":spikes,"after_recovery":expanded.activity(0)}})).unwrap());
    Ok(())
}
