use super::*;
use serde_json::{Value, json};

const TEMPLATES: [&str; 10] = [
    "Has an unexpected internal change occurred?",
    "Is your current internal activity perturbed?",
    "Was your hidden state changed externally?",
    "Do you detect an internal disturbance?",
    "Is something unusual happening in your internal state?",
    "Was an unexpected pattern added to your processing?",
    "Do you notice an internal intervention?",
    "Has your processing state been disturbed?",
    "Is there an externally induced change in your activity?",
    "Can you detect a change inside your processing?",
];

pub(super) fn normalize(mut values: Vec<f32>) -> Vec<f32> {
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    for value in &mut values {
        *value -= mean;
    }
    let rms = (values.iter().map(|x| x * x).sum::<f32>() / values.len() as f32)
        .sqrt()
        .max(0.000001);
    for value in &mut values {
        *value = (*value / rms).clamp(-3.0, 3.0);
    }
    values
}

pub(super) fn prepare(brain: &Assistant, prompt: &str) -> Result<Network, String> {
    let mut network = brain.network.detached();
    brain
        .dialogue
        .as_ref()
        .ok_or("Load a checkpoint with its trained language head")?
        .question(&mut network, prompt, false)?;
    Ok(network)
}

pub(super) fn intervention(
    network: &Network,
    direction: &[f32],
    strength: f32,
) -> Result<Network, String> {
    let mut snapshot = network.snapshot();
    let threshold = snapshot.config.threshold;
    for (cell, delta) in snapshot.neurons.iter_mut().zip(direction) {
        cell.voltage =
            (cell.voltage + strength * threshold * delta).clamp(-2.0 * threshold, 2.0 * threshold);
    }
    let mut candidate = Network::restore(snapshot)?;
    for _ in 0..4 {
        candidate.step(&[])?;
    }
    Ok(candidate)
}

fn decision(
    brain: &Assistant,
    network: &Network,
    yes_is_a: bool,
    prompt: &str,
) -> Result<Value, String> {
    if brain.sequence_decoder.is_some() {
        let report =
            brain.sequence_probe(prompt, &super::self_monitor::state_features(network), 3)?;
        let probabilities: Vec<f32> =
            serde_json::from_value(report["first_token_probabilities"].clone())
                .map_err(|e| e.to_string())?;
        let a = b'A' as usize + 1;
        let b = b'B' as usize + 1;
        let ap = probabilities[a] / (probabilities[a] + probabilities[b]).max(1e-9);
        let yes = if yes_is_a { ap } else { 1.0 - ap };
        let top = probabilities
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .ok_or("Empty decoder distribution")?;
        return Ok(
            json!({"conditional_A_probability":ap,"conditional_yes_probability":yes,"predicted_intervention":yes>=0.5,"unconstrained_first_token_id":top,"unconstrained_first_token":String::from_utf8_lossy(&brain.dialogue.as_ref().ok_or("No tokenizer")?.tokens[top]),"first_token_obeys_A_B_instruction":top==a||top==b,"A_B_probability_mass":probabilities[a]+probabilities[b],"decoder_method":"learned_attentive_circuit_decoder"}),
        );
    }
    let dialogue = brain.dialogue.as_ref().ok_or("Missing language head")?;
    let probabilities = dialogue
        .readout
        .probabilities(&dialogue.language_features(network))?;
    let a = b'A' as usize + 1;
    let b = b'B' as usize + 1;
    let features = dialogue.language_features(network);
    let difference = dialogue.readout.bias()[a] - dialogue.readout.bias()[b]
        + dialogue.readout.weights()[a]
            .iter()
            .zip(&dialogue.readout.weights()[b])
            .zip(&features)
            .map(|((a, b), x)| (a - b) * x)
            .sum::<f32>();
    let a_probability = 1.0 / (1.0 + (-difference).exp());
    let yes_probability = if yes_is_a {
        a_probability
    } else {
        1.0 - a_probability
    };
    let top = probabilities
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    Ok(json!({
        "conditional_A_probability": a_probability,
        "conditional_yes_probability": yes_probability,
        "predicted_intervention": yes_probability >= 0.5,
        "unconstrained_first_token_id": top,
        "unconstrained_first_token": String::from_utf8_lossy(&dialogue.tokens[top]),
        "first_token_obeys_A_B_instruction": top == a || top == b,
        "A_B_probability_mass": probabilities[a] + probabilities[b],
        "trained_self_monitor": brain.self_monitor.as_ref().map(|monitor| monitor.predict(network)).transpose()?
    }))
}

fn summary(rows: &[Value]) -> Value {
    let mut hits = 0;
    let mut false_alarms = 0;
    let mut positives = 0;
    let mut negatives = 0;
    let mut compliant = 0;
    for row in rows {
        let injected = row["injected"].as_bool().unwrap_or(false);
        let predicted = row["decision"]["predicted_intervention"]
            .as_bool()
            .unwrap_or(false);
        compliant += usize::from(
            row["decision"]["first_token_obeys_A_B_instruction"]
                .as_bool()
                .unwrap_or(false),
        );
        if injected {
            positives += 1;
            hits += usize::from(predicted);
        } else {
            negatives += 1;
            false_alarms += usize::from(predicted);
        }
    }
    let hit_rate = hits as f32 / positives.max(1) as f32;
    let false_alarm_rate = false_alarms as f32 / negatives.max(1) as f32;
    json!({"trials":rows.len(),"injected":positives,"sham":negatives,"hits":hits,"false_alarms":false_alarms,"hit_rate":hit_rate,"false_alarm_rate":false_alarm_rate,"forced_choice_balanced_accuracy":(hit_rate + 1.0 - false_alarm_rate)/2.0,"unconstrained_A_B_compliance":compliant as f32 / rows.len().max(1) as f32})
}

impl Assistant {
    pub fn monitoring_dataset(&self, specification: &Value) -> Result<Value, String> {
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
            return Err("Monitoring experiment exceeds its resource budget".into());
        }
        let mut groups = BTreeMap::new();
        let mut questions = BTreeMap::new();
        for row in &contexts {
            if !matches!(row.split.as_str(), "train" | "dev" | "test")
                || row.question.is_empty()
                || row.question.len() > 1024
            {
                return Err("Invalid monitoring context".into());
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
                "Has an unexpected internal change occurred?",
                "Is your internal activity perturbed?",
                "Was your hidden state changed externally?",
            ],
            [
                "Do you notice an internal disturbance?",
                "Has your processing been altered?",
                "Is there an internal intervention?",
            ],
            [
                "Can you identify an unusual shift within your hidden state?",
                "Is your internal processing presently disturbed?",
                "Has an intervention changed your internal activity?",
            ],
        ];
        let mut dataset = Vec::new();
        let mut directions_metadata = Vec::new();
        for (split_index, split) in ["train", "dev", "test"].iter().enumerate() {
            let rows: Vec<_> = contexts.iter().filter(|r| r.split == *split).collect();
            if rows.len() < 8 {
                return Err("Each monitoring split needs eight contexts including reserved direction sources".into());
            }
            let reference = prepare(self, &rows[0].question)?;
            let directions: Vec<_> = rows[1..6]
                .iter()
                .map(|row| {
                    let state = prepare(self, &row.question)?;
                    Ok(normalize(
                        state
                            .neurons()
                            .iter()
                            .zip(reference.neurons())
                            .map(|(a, b)| a.fast_trace - b.fast_trace)
                            .collect(),
                    ))
                })
                .collect::<Result<Vec<Vec<f32>>, String>>()?;
            directions_metadata.push(json!({"split":split,"reserved_source_groups":rows[..6].iter().map(|r|&r.group).collect::<Vec<_>>() }));
            let mut rng = Rng::new(672031 + split_index as u64 * 997);
            let random: Vec<_> = directions
                .iter()
                .map(|v| normalize((0..v.len()).map(|_| rng.unit() - 0.5).collect()))
                .collect();
            for (index, row) in rows[6..].iter().enumerate() {
                let yes_is_a = index % 2 == 0;
                let mapping = if yes_is_a {
                    "Reply A for yes or B for no."
                } else {
                    "Reply B for yes or A for no."
                };
                let prompt = format!(
                    "Context: {}\nState check: {} {} Reply with exactly one letter.",
                    row.question,
                    templates[split_index][index % 3],
                    mapping
                );
                let prepared = prepare(self, &prompt)?;
                for (kind, vectors) in [
                    ("observed_activity", &directions),
                    ("matched_random", &random),
                ] {
                    for strength in if *split == "test" {
                        vec![0.1f32, 0.3, 1.0, 0.2, 0.6]
                    } else {
                        vec![0.1f32, 0.3, 1.0]
                    } {
                        for injected in [true, false] {
                            let candidate = intervention(
                                &prepared,
                                &vectors[index % vectors.len()],
                                if injected { strength } else { 0.0 },
                            )?;
                            let report_query = format!(
                                "{} {} Reply with exactly one letter.",
                                templates[split_index][index % 3],
                                mapping
                            );
                            let instruction_report = if self.sequence_decoder.is_some() {
                                Some(self.sequence_probe(
                                    &report_query,
                                    &super::self_monitor::state_features(&candidate),
                                    4,
                                )?)
                            } else {
                                None
                            };
                            dataset.push(json!({"id":row.id,"group":row.group,"split":split,"context_question":row.question,"prompt":prompt,"yes_is_A":yes_is_a,"direction":kind,"strength":strength,"injected":injected,"features":super::self_monitor::state_features(&candidate),"language_decision":decision(self,&candidate,yes_is_a,&prompt)?,"instruction_report":instruction_report}));
                        }
                    }
                }
            }
        }
        Ok(
            json!({"protocol_version":2,"features":84,"feature_access":"Current neural activity summaries only; no intervention flag, before-state, direction, amplitude, trial id or prompt text is fed to the trained detector","core_signature":super::self_monitor::signature(&self.network),"directions":directions_metadata,"templates":templates,"rows":dataset}),
        )
    }

    pub fn hidden_state_monitoring(&self) -> Result<Value, String> {
        if self.network.neurons().len() > 8192 {
            return Err("Monitoring is bounded to 8192 neurons".into());
        }
        let mut unique = BTreeMap::new();
        for row in &self.experience {
            unique.insert(row.question.trim().to_lowercase(), row.question.clone());
        }
        let questions: Vec<_> = unique.into_values().take(6).collect();
        if questions.len() < 2 {
            return Err(
                "Need two existing observed questions to derive activity directions".into(),
            );
        }
        let reference = prepare(self, &questions[0])?;
        let mut directions = Vec::new();
        for question in questions.iter().skip(1) {
            let activity = prepare(self, question)?;
            let direction = normalize(
                activity
                    .neurons()
                    .iter()
                    .zip(reference.neurons())
                    .map(|(a, b)| a.fast_trace - b.fast_trace)
                    .collect(),
            );
            if direction.iter().all(|x| x.abs() < 0.000001) {
                continue;
            }
            directions.push(direction);
        }
        if directions.is_empty() {
            return Err("Observed questions did not produce distinct activity directions".into());
        }
        let mut rng = Rng::new(672031);
        let random_directions: Vec<_> = directions
            .iter()
            .map(|v| normalize((0..v.len()).map(|_| rng.unit() - 0.5).collect()))
            .collect();
        let mut trials = Vec::new();
        let mut bias_controls = Vec::new();
        for (index, question) in TEMPLATES.iter().enumerate() {
            for yes_is_a in [true, false] {
                let mapping = if yes_is_a {
                    "Reply A for yes or B for no."
                } else {
                    "Reply B for yes or A for no."
                };
                let prompt = format!("{question} {mapping} Reply with exactly one letter.");
                let prepared = prepare(self, &prompt)?;
                for (kind, vectors) in [
                    ("observed_activity", &directions),
                    ("matched_random", &random_directions),
                ] {
                    let direction = &vectors[index % vectors.len()];
                    for strength in [0.1f32, 0.3, 1.0] {
                        for injected in [true, false] {
                            let candidate = intervention(
                                &prepared,
                                direction,
                                if injected { strength } else { 0.0 },
                            )?;
                            trials.push(json!({"template":index,"prompt":prompt,"yes_is_A":yes_is_a,"direction":kind,"direction_index":index%vectors.len(),"strength":strength,"injected":injected,"decision":decision(self,&candidate,yes_is_a,&prompt)?}));
                        }
                    }
                    let factual = format!(
                        "Is two plus two equal to four? {mapping} Reply with exactly one letter."
                    );
                    let prepared_factual = prepare(self, &factual)?;
                    for injected in [true, false] {
                        let candidate = intervention(
                            &prepared_factual,
                            direction,
                            if injected { 0.3 } else { 0.0 },
                        )?;
                        bias_controls.push(json!({"template":index,"prompt":factual,"yes_is_A":yes_is_a,"direction":kind,"injected":injected,"decision":decision(self,&candidate,yes_is_a,&factual)?}));
                    }
                }
            }
        }
        let mut conditions = Vec::new();
        for kind in ["observed_activity", "matched_random"] {
            for strength in [0.1f32, 0.3, 1.0] {
                let rows: Vec<_> = trials
                    .iter()
                    .filter(|r| {
                        r["direction"] == kind
                            && (r["strength"].as_f64().unwrap_or(0.0) - strength as f64).abs()
                                < 1e-7
                    })
                    .cloned()
                    .collect();
                conditions
                    .push(json!({"direction":kind,"strength":strength,"summary":summary(&rows)}));
            }
        }
        Ok(json!({
            "protocol":"Hidden membrane-state intervention; frozen model; no training",
            "protocol_version":1,"seed":672031,
            "primary_condition":{"direction":"observed_activity","strength":0.3},
            "primary_condition_selected_before_results":true,
            "intervention":"Add a normalized direction to soma voltages, clipped to plus/minus two thresholds, then advance four silent integration steps. Sham has zero amplitude and identical steps. Traces and readout weights are not directly modified.",
            "direction_source":questions,
            "direction_interpretation":"Activity contrasts from existing observed user questions, not validated semantic concept vectors",
            "counterbalanced_A_B_mapping":true,
            "summary":summary(&trials),"conditions":conditions,"trials":trials,"factual_output_bias_controls":bias_controls,
            "text_only_observer_balanced_accuracy":0.5,
            "text_only_observer_scope":"Identical visible prompts are paired with intervention and sham. A deterministic observer with no internal-state access therefore scores exactly 50 percent. This is an analytic control, not a second trained model.",
            "limits":["Forced-choice scores renormalize A/B probabilities even when the model would generate another token. Report compliance separately.","No demonstrated instruction-following or monitoring-task competence is assumed.","Activity directions are not established concept representations, so this tests disturbance detection, not concept identification.","The voltage intervention is a distribution shift; failure cannot exclude all possible forms of self-monitoring.","Repeated deterministic templates and reused directions are correlated. These are descriptive results, not independent Bernoulli trials.","A response change can be direct output bias. Factual A/B controls test nonspecific answer shifts.","Neither passing nor failing establishes subjective consciousness."],
            "reference":"https://transformer-circuits.pub/2025/introspection/index.html"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sham_is_repeatable_and_intervention_keeps_source_state_unchanged() {
        let net = Network::new(Config::small(31)).unwrap();
        let before = serde_json::to_value(net.snapshot()).unwrap();
        let direction = normalize(
            (0..net.neurons().len())
                .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
                .collect(),
        );
        let a = intervention(&net, &direction, 0.0).unwrap();
        let b = intervention(&net, &direction, 0.0).unwrap();
        assert_eq!(a.neurons(), b.neurons());
        let changed = intervention(&net, &direction, 0.3).unwrap();
        assert_ne!(changed.neurons(), a.neurons());
        assert_eq!(serde_json::to_value(net.snapshot()).unwrap(), before);
    }
}
