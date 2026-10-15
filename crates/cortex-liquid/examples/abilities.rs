use cortex_liquid::abilities::{Abilities, INPUTS, Perception};
fn main() {
    let mut abilities = Abilities::default();
    // The brain lives the percepts; what it then expects is the DANN input.
    let mut brain = cortex_brain::Brain::new();
    let mut reading = Default::default();
    for at in 1..501 {
        let phase = at as f32 * 0.15;
        let mut values = [0.; INPUTS];
        values[0] = phase.sin();
        values[1] = phase.cos();
        values[2] = 0.1;
        values[3] = 1.;
        let percepts: Vec<String> = vec![
            "domain: trading".into(),
            if values[0] > 0. {
                "movement: up".into()
            } else {
                "movement: down".into()
            },
        ];
        brain.live("synthetic", &percepts);
        reading = abilities.observe(Perception {
            domain: "trading".into(),
            scope: "synthetic:5m".into(),
            time_ms: at * 10_000,
            values,
            percepts,
            expected: brain.expectations("synthetic", 8),
        });
    }
    println!(
        "{}",
        serde_json::json!({"dataset":"synthetic periodic environment; not market evidence","observations":reading.observed,"scored":reading.scored,
        "prequential_mse":reading.mean_errors.map(|v|v/reading.scored as f64),"persistence_mse":reading.baseline_error/reading.scored as f64,
        "snn_neurons":reading.snn_neurons,"dann_neurons":reading.neurons,"replayed":reading.replays,"promoted":reading.promoted,"generation":reading.generation})
    );
}
