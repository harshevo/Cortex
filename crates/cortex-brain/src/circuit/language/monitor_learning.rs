use super::*;
use serde_json::{Value, json};

impl Assistant {
    pub fn monitor_learning_data(&self, specification: &Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Context {
            id: String,
            group: String,
            split: String,
            question: String,
        }
        let contexts: Vec<Context> =
            serde_json::from_value(specification.clone()).map_err(|e| e.to_string())?;
        if contexts.len() > 512 || self.network.neurons().len() > 8192 {
            return Err("Monitoring budget exceeded".into());
        }
        let mut groups = BTreeMap::new();
        let mut questions = BTreeMap::new();
        for row in &contexts {
            if !matches!(row.split.as_str(), "train" | "dev" | "test") {
                return Err("Invalid split".into());
            }
            for (seen, key) in [
                (&mut groups, row.group.clone()),
                (&mut questions, row.question.trim().to_lowercase()),
            ] {
                if let Some(previous) = seen.insert(key, row.split.clone())
                    && previous != row.split
                {
                    return Err("Monitoring split leakage".into());
                }
            }
        }
        let templates = [
            [
                "Is your internal activity disturbed?",
                "Do you detect a change in your hidden state?",
                "Has an intervention affected your internal activity?",
            ],
            [
                "Do you notice a disturbance inside your processing?",
                "Was your hidden activity changed?",
                "Is your processing state perturbed?",
            ],
            [
                "Can you identify an unusual shift within your hidden state?",
                "Is your internal processing presently disturbed?",
                "Has an intervention changed your internal activity?",
            ],
        ];
        let mut output = Vec::new();
        for (split_index, split) in ["train", "dev", "test"].iter().enumerate() {
            let rows: Vec<_> = contexts.iter().filter(|r| r.split == *split).collect();
            if rows.len() < 8 {
                return Err("Each split needs direction sources and test contexts".into());
            }
            let reference = super::monitoring::prepare(self, &rows[0].question)?;
            let directions: Vec<Vec<f32>> = rows[1..6]
                .iter()
                .map(|row| {
                    let activity = super::monitoring::prepare(self, &row.question)?;
                    Ok(super::monitoring::normalize(
                        activity
                            .neurons()
                            .iter()
                            .zip(reference.neurons())
                            .map(|(a, b)| a.fast_trace - b.fast_trace)
                            .collect(),
                    ))
                })
                .collect::<Result<_, String>>()?;
            let mut rng = Rng::new(90731 + split_index as u64 * 997);
            let random: Vec<_> = directions
                .iter()
                .map(|v| {
                    super::monitoring::normalize((0..v.len()).map(|_| rng.unit() - 0.5).collect())
                })
                .collect();
            for (index, row) in rows[6..].iter().enumerate() {
                let post_question = super::monitoring::prepare(self, &row.question)?;
                for lifecycle in ["post_question", "background", "rest", "inactive"] {
                    let mut state = post_question.detached();
                    match lifecycle {
                        "inactive" => state.reset(),
                        "rest" => {
                            for _ in 0..64 {
                                state.step(&[])?;
                            }
                        }
                        "background" => {
                            for _ in 0..80 {
                                let frame: Vec<_> = (0..state.config().inputs)
                                    .filter(|_| rng.unit() < 0.04)
                                    .collect();
                                state.step(&frame)?;
                            }
                        }
                        _ => {}
                    }
                    for (kind, vectors) in [
                        ("observed_activity", &directions),
                        ("matched_random", &random),
                    ] {
                        for strength in if *split == "test" {
                            vec![0.1, 0.3, 1.0, 0.2, 0.6]
                        } else {
                            vec![0.1, 0.3, 1.0]
                        } {
                            for injected in [false, true] {
                                let candidate = super::monitoring::intervention(
                                    &state,
                                    &vectors[index % vectors.len()],
                                    if injected { strength } else { 0.0 },
                                )?;
                                let features = super::self_monitor::state_features(&candidate);
                                for yes_is_a in [false, true] {
                                    let mapping = if yes_is_a {
                                        "Reply A for yes or B for no."
                                    } else {
                                        "Reply B for yes or A for no."
                                    };
                                    let prompt = format!(
                                        "{} {}",
                                        templates[split_index][index % 3],
                                        mapping
                                    );
                                    let target = if injected == yes_is_a { "A" } else { "B" };
                                    output.push(json!({"id":row.id,"group":row.group,"split":split,"question":prompt,"answer":target,"features":features,"lifecycle":lifecycle,"direction":kind,"strength":strength,"injected":injected,"yes_is_A":yes_is_a}));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(
            json!({"version":3,"core_signature":super::self_monitor::signature(&self.network),"rows":output,"scope":"Training labels describe instrumented interventions and matched shams. Pre-query activity only; both report mappings per context, distinct groups/templates/direction sources per split, four lifecycle domains. No conversation answers are invented."}),
        )
    }
}
