use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Adaptation {
    pub observed_fingerprint: u64,
    pub dreams: u64,
    pub dream_candidates: Vec<String>,
    pub attempts: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub replayed: u64,
    pub neurons_added: usize,
    pub synapses_added: usize,
    pub dendrites_grown: usize,
    pub dendrites_pruned: usize,
    pub connection_attempts: u64,
    pub connection_accepted: u64,
    pub feedback_events: u64,
    pub readout_replays_accepted: u64,
    pub readout_replays_rejected: u64,
    pub sequence_replay_attempts: u64,
    pub sequence_replay_accepted: u64,
    pub sequence_replay_rejected: u64,
    pub sequence_feedback_updates: u64,
    pub strengthened: usize,
    pub weakened: usize,
    pub base_neurons: usize,
    pub rate: f32,
    pub last: String,
    pub before_loss: Option<f32>,
    pub after_loss: Option<f32>,
}
impl Default for Adaptation {
    fn default() -> Self {
        Self {
            observed_fingerprint: 0,
            dreams: 0,
            dream_candidates: Vec::new(),
            attempts: 0,
            accepted: 0,
            rejected: 0,
            replayed: 0,
            neurons_added: 0,
            synapses_added: 0,
            dendrites_grown: 0,
            dendrites_pruned: 0,
            connection_attempts: 0,
            connection_accepted: 0,
            feedback_events: 0,
            readout_replays_accepted: 0,
            readout_replays_rejected: 0,
            sequence_replay_attempts: 0,
            sequence_replay_accepted: 0,
            sequence_replay_rejected: 0,
            sequence_feedback_updates: 0,
            strengthened: 0,
            weakened: 0,
            base_neurons: 0,
            rate: 0.002,
            last: "Waiting for observed lessons".into(),
            before_loss: None,
            after_loss: None,
        }
    }
}

struct Retention {
    losses: Vec<f32>,
    language: Vec<Vec<f32>>,
    decisions: Vec<(usize, bool)>,
}

impl Assistant {
    pub(super) fn isolated(&self) -> Self {
        let mut candidate = self.clone();
        candidate.network = self.network.detached();
        candidate
    }

    fn retention(&mut self, rows: &[Lesson]) -> Result<Retention, String> {
        let mut losses = Vec::new();
        let mut language = Vec::new();
        let mut decisions = Vec::new();
        for row in rows {
            let target = self
                .labels
                .iter()
                .position(|p| Some(p) == row.label.as_ref())
                .ok_or("Invalid replay label")?;
            let features = self.features(&row.question)?;
            let probabilities = self.readout.probabilities(&features)?;
            losses.push(-probabilities[target].max(1e-9).ln());
            let (choice, confidence) = self.readout.predict(&features)?;
            let accepted = confidence >= self.gates[choice].confidence
                && self.similarity(&features, choice) >= self.gates[choice].similarity
                && self.labels[choice] != "unknown";
            decisions.push((choice, accepted));
            if let Some(dialogue) = &self.dialogue {
                dialogue.question(&mut self.network, &row.question, false)?;
                for _ in 0..4 {
                    let probabilities = dialogue
                        .readout
                        .probabilities(&dialogue.language_features(&self.network))?;
                    let token = probabilities
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.total_cmp(b.1))
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    language.push(probabilities);
                    if token == 0 {
                        break;
                    }
                    dialogue.feed(&mut self.network, token)?;
                }
            }
        }
        Ok(Retention {
            losses,
            language,
            decisions,
        })
    }

    fn retention_passed(before: &Retention, after: &Retention) -> bool {
        let retained = before
            .losses
            .iter()
            .zip(&after.losses)
            .all(|(old, new)| new.is_finite() && *new <= old + 0.0001);
        let language_retained = before.language.len() == after.language.len()
            && before
                .language
                .iter()
                .zip(&after.language)
                .all(|(old, new)| {
                    let winner = |values: &[f32]| {
                        values
                            .iter()
                            .enumerate()
                            .max_by(|a, b| a.1.total_cmp(b.1))
                            .map(|(i, _)| i)
                    };
                    winner(old) == winner(new)
                        && old.iter().zip(new).map(|(a, b)| (a - b).abs()).sum::<f32>() <= 0.01
                });
        let decisions_retained = before
            .decisions
            .iter()
            .zip(&after.decisions)
            .all(|(old, new)| !old.1 || old == new);
        retained && language_retained && decisions_retained
    }

    pub(super) fn learn_connections(
        &mut self,
        evidence: &[Vec<f32>],
        signal: f32,
    ) -> Result<(), String> {
        if evidence.is_empty() || signal == 0.0 {
            return Ok(());
        }
        let mut unique = BTreeMap::new();
        for row in &self.experience {
            unique.insert(row.question.trim().to_lowercase(), row.clone());
        }
        let rows: Vec<_> = unique.into_values().take(8).collect();
        let mut baseline = self.isolated();
        let before = baseline.retention(&rows)?;
        let mut candidate = self.isolated();
        let (stronger, weaker) = candidate
            .network
            .adapt_connections(evidence, self.adaptation.rate * signal);
        let after = candidate.retention(&rows)?;
        self.adaptation.connection_attempts += 1;
        if Self::retention_passed(&before, &after) {
            candidate.adaptation = self.adaptation.clone();
            candidate.adaptation.connection_accepted += 1;
            candidate.adaptation.strengthened += stronger;
            candidate.adaptation.weakened += weaker;
            candidate.network.reset();
            candidate.network.adopt_live(&self.network);
            *self = candidate;
        }
        Ok(())
    }

    pub fn replay(&mut self) -> Result<usize, String> {
        let mut unique = BTreeMap::new();
        for row in &self.experience {
            unique.insert(row.question.trim().to_lowercase(), row.clone());
        }
        let rows: Vec<_> = unique.into_values().collect();
        if rows.is_empty() {
            return Ok(0);
        }
        let mut baseline = self.isolated();
        let before = baseline.retention(&rows)?;
        drop(baseline);
        let mut candidate = self.isolated();
        candidate.train(&rows, 1)?;
        let after = candidate.retention(&rows)?;
        if Self::retention_passed(&before, &after) {
            candidate.adaptation.readout_replays_accepted += 1;
            candidate.network.reset();
            candidate.network.adopt_live(&self.network);
            *self = candidate;
        } else {
            self.adaptation.readout_replays_rejected += 1;
        }
        let evidence = rows
            .iter()
            .take(12)
            .map(|row| self.features(&row.question))
            .collect::<Result<Vec<_>, _>>()?;
        self.learn_connections(&evidence, 1.0)?;
        Ok(rows.len())
    }

    pub fn feedback(&mut self, reward: f32) -> Result<(), String> {
        if !reward.is_finite() || !(-1.0..=1.0).contains(&reward) || reward == 0.0 {
            return Err("Feedback reward must be between -1 and 1 and nonzero".into());
        }
        if self.last_sequence.is_some() {
            return self.sequence_feedback(reward);
        }
        let mut candidate = self.isolated();
        let mut evidence = Vec::new();
        if let Some((network, question, tokens, continuous)) = &self.last_dialogue {
            let mut replay = network.detached();
            let dialogue = candidate.dialogue.as_mut().ok_or("No language readout")?;
            dialogue.question_state(&mut replay, question, false, !continuous)?;
            for token in tokens.iter().take(32) {
                let features = dialogue.language_features(&replay);
                dialogue.readout.reinforce(
                    &features,
                    *token,
                    reward,
                    0.01 / (tokens.len().max(1) as f32).sqrt(),
                )?;
                evidence.push(features);
                dialogue.feed(&mut replay, *token)?;
            }
            if evidence.is_empty() {
                return Err("The last answer had no generated tokens to rate".into());
            }
        } else {
            self.last
                .as_ref()
                .ok_or("The last response has no operation learning trace to rate")?;
            let observation = self
                .observations
                .back()
                .ok_or("Ask a question before giving feedback")?;
            let features = candidate.features(&observation.question)?;
            let action = self
                .labels
                .iter()
                .position(|label| label == &observation.response.program)
                .ok_or("No selected operation to rate")?;
            candidate
                .readout
                .reinforce(&features, action, reward, 0.05)?;
            evidence.push(features);
        }
        candidate.adaptation.feedback_events += 1;
        candidate.updates += evidence.len() as u64;
        candidate.learn_connections(&evidence, reward)?;
        candidate.network.reset();
        candidate.network.adopt_live(&self.network);
        candidate.last_dialogue = None;
        candidate.observations.clear();
        *self = candidate;
        Ok(())
    }

    pub fn assess_capabilities(&self) -> Result<serde_json::Value, String> {
        let mut intact = self.isolated();
        let mut interrupted = self.isolated();
        let mut snapshot = interrupted.network.snapshot();
        for synapse in &mut snapshot.synapses {
            if synapse.source >= snapshot.config.inputs {
                synapse.magnitude = 0.0;
            }
        }
        interrupted.network = Network::restore(snapshot)?;
        let mut probes = Vec::new();
        for lesson in self.experience.iter().take(4) {
            let baseline = intact.features(&lesson.question)?;
            let ablated = interrupted.features(&lesson.question)?;
            let difference = baseline
                .iter()
                .zip(&ablated)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>();
            let repeated = intact.features(&lesson.question)?;
            let repeat_difference = baseline
                .iter()
                .zip(&repeated)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>();
            probes.push(serde_json::json!({"question":lesson.question,"recurrent_ablation_feature_l1":difference,"repeat_feature_l1":repeat_difference}));
        }
        Ok(serde_json::json!({
            "scope":"Engineering probes, not a consciousness test",
            "subjective_experience":"Not established by these measurements",
            "recurrence_probes":probes,
            "probe_source":"Existing observed lessons only; no training performed",
            "adaptation":self.adaptation,
            "needs_separate_evaluation":["Held-out confidence calibration","Cross-modal integration","Self-state prediction under interventions","Long-term memory and transfer","Independent theory-based consciousness assessment"],
            "reference":"https://arxiv.org/abs/2308.08708"
        }))
    }

    pub fn consolidate(&mut self, path: &Path) -> Result<(), String> {
        self.replay_sequence(path)?;
        self.consolidate_stage(path, 0)
    }

    fn consolidate_stage(&mut self, path: &Path, stage: usize) -> Result<(), String> {
        let mut unique = BTreeMap::new();
        for lesson in &self.experience {
            unique.insert(lesson.question.trim().to_lowercase(), lesson.clone());
        }
        if unique.len() < 2 {
            self.adaptation.last =
                format!("Need 2 distinct observed lessons; have {}", unique.len());
            return Ok(());
        }
        if self.adaptation.base_neurons == 0 {
            self.adaptation.base_neurons = self.network.neurons().len();
        }
        if !self.adaptation.rate.is_finite() {
            return Err("Invalid adaptation rate".into());
        }
        self.adaptation.rate = self.adaptation.rate.clamp(0.00001, 0.005);
        let fingerprint = unique
            .keys()
            .flat_map(|s| s.bytes())
            .fold(1469598103934665603u64, |h, b| {
                (h ^ b as u64).wrapping_mul(1099511628211)
            });
        let novel = fingerprint != self.adaptation.observed_fingerprint;
        self.adaptation.observed_fingerprint = fingerprint;
        let mut train = Vec::new();
        let mut holdout = Vec::new();
        for (index, row) in unique.into_values().enumerate() {
            if index % 4 == 0 {
                holdout.push(row);
            } else {
                train.push(row);
            }
        }
        train.truncate(12);
        holdout.truncate(8);
        self.adaptation.attempts = self.adaptation.attempts.saturating_add(1);
        let mut baseline = self.isolated();
        let before = baseline.retention(&holdout)?;
        let mut evidence = Vec::new();
        for row in &train {
            evidence.push(baseline.features(&row.question)?);
        }
        drop(baseline);
        let mut candidate = self.isolated();
        if stage == 0 && candidate.dialogue.is_some() {
            let mut dreams = Vec::new();
            for (index, row) in train.iter().take(3).enumerate() {
                candidate.network.reset();
                candidate.sense(&[vec![index % candidate.network.config().inputs]])?;
                let generated = candidate.generate_continuing(&row.question, 12)?;
                dreams.push(
                    generated["text"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(256)
                        .collect(),
                );
            }
            self.adaptation.dreams += 1;
            self.adaptation.dream_candidates = dreams;
        }
        let (stronger, weaker) = candidate
            .network
            .adapt_connections(&evidence, self.adaptation.rate);
        let (dendrites_grown, dendrites_pruned) = candidate.network.remodel_branches(
            &evidence,
            if stage == 0 {
                8
            } else if stage == 1 {
                1
            } else {
                0
            },
        );
        let old_neurons = candidate.network.neurons().len();
        let old_synapses = candidate.network.synapses().len();
        let config = candidate.network.config();
        let max_readout = candidate
            .dialogue
            .as_ref()
            .map_or(8192, |d| 40_000_000 / d.tokens.len());
        let limit = self
            .adaptation
            .base_neurons
            .saturating_add(256)
            .min(8192)
            .min(config.max_neurons)
            .min(config.max_synapses / (config.fan_in + config.input_fan_in).max(1))
            .min(max_readout);
        let demand = novel || before.losses.iter().sum::<f32>() / before.losses.len() as f32 > 0.05;
        let mut growth = if demand && stage == 0 {
            limit.saturating_sub(old_neurons).min(4)
        } else {
            0
        };
        let mut proposed_config = config.clone();
        proposed_config.association += growth;
        if proposed_config.validate().is_err() {
            growth = 0;
        }
        if growth > 0 {
            let position = candidate.network.grow_association(growth)?;
            candidate.readout.insert_features(position, growth)?;
            if let Some(dialogue) = &mut candidate.dialogue {
                dialogue.readout.insert_features(position, growth)?;
            }
            for centroid in &mut candidate.centroids {
                centroid.splice(position..position, std::iter::repeat_n(0.0, growth));
            }
        }
        let after = candidate.retention(&holdout)?;
        self.adaptation.before_loss =
            Some(before.losses.iter().sum::<f32>() / before.losses.len() as f32);
        self.adaptation.after_loss =
            Some(after.losses.iter().sum::<f32>() / after.losses.len() as f32);
        self.adaptation.replayed = self.adaptation.replayed.saturating_add(train.len() as u64);
        if !Self::retention_passed(&before, &after) {
            self.adaptation.rejected = self.adaptation.rejected.saturating_add(1);
            self.adaptation.rate = (self.adaptation.rate * 0.5).max(0.00001);
            self.adaptation.last = "Candidate rejected by retention check".into();
            if stage < 2 {
                return self.consolidate_stage(path, stage + 1);
            }
            self.save(path)?;
            return Ok(());
        }
        candidate.adaptation = self.adaptation.clone();
        candidate.adaptation.accepted += 1;
        candidate.adaptation.neurons_added += growth;
        candidate.adaptation.dendrites_grown += dendrites_grown;
        candidate.adaptation.dendrites_pruned += dendrites_pruned;
        candidate.adaptation.synapses_added += candidate.network.synapses().len() - old_synapses;
        candidate.adaptation.strengthened += stronger;
        candidate.adaptation.weakened += weaker;
        candidate.adaptation.rate = (candidate.adaptation.rate * 1.05).min(0.005);
        candidate.adaptation.last = format!(
            "Retained replay: +{growth} neurons, +{dendrites_grown}/-{dendrites_pruned} branches, {stronger} stronger, {weaker} weaker synapses"
        );
        candidate.network.reset();
        candidate.last_dialogue = None;
        candidate.observations.clear();
        // Persist before replacing the active circuit. Failed writes leave its weights intact.
        candidate.save(path)?;
        candidate.network.adopt_live(&self.network);
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_keeps_old_predictions_dimensions_and_survives_reload() {
        let mut brain =
            Assistant::new(Config::small(21), vec!["greeting".into(), "unknown".into()]).unwrap();
        let old = brain.network.neurons().len();
        let position = brain.network.grow_association(4).unwrap();
        brain.readout.insert_features(position, 4).unwrap();
        for row in &mut brain.centroids {
            row.splice(position..position, [0.0; 4]);
        }
        let dir = tempfile::tempdir().unwrap();
        brain.save(&dir.path().join("grown.json")).unwrap();
        let loaded = Assistant::load(&dir.path().join("grown.json")).unwrap();
        assert_eq!(loaded.network.neurons().len(), old + 4);
        assert_eq!(
            loaded.network.activity(0).dendrites,
            (old + 4 - Config::small(21).inhibitory) * 4
        );
        assert_eq!(loaded.readout.weights()[0].len(), old + 4);
    }
    #[test]
    fn replay_requires_observed_lessons_and_never_teaches_generated_text() {
        let mut brain =
            Assistant::new(Config::small(21), vec!["greeting".into(), "unknown".into()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        brain.consolidate(&dir.path().join("brain.json")).unwrap();
        assert_eq!(brain.adaptation.attempts, 0);
        assert_eq!(brain.updates, 0);
        for index in 0..8 {
            brain
                .teach(&format!("hello number {index}"), "greeting")
                .unwrap();
        }
        let live = brain.network.watch();
        let before = brain.network.synapses().to_vec();
        brain.consolidate(&dir.path().join("brain.json")).unwrap();
        assert_eq!(brain.adaptation.attempts, 1);
        assert_eq!(brain.adaptation.accepted + brain.adaptation.rejected, 1);
        assert_eq!(brain.adaptation.accepted, 1, "{}", brain.adaptation.last);
        {
            let telemetry = live.lock().unwrap();
            assert_eq!(telemetry.voltages.len(), brain.network.neurons().len());
            assert_eq!(telemetry.populations.len(), brain.network.neurons().len());
            assert_eq!(
                telemetry.activity.dendrites,
                brain.network.activity(0).dendrites
            );
        }
        let snapshot = serde_json::to_value(brain.network.snapshot()).unwrap();
        let weights = brain.readout.weights().to_vec();
        let assessment = brain.assess_capabilities().unwrap();
        assert_eq!(assessment["recurrence_probes"].as_array().unwrap().len(), 4);
        assert_eq!(
            serde_json::to_value(brain.network.snapshot()).unwrap(),
            snapshot
        );
        assert_eq!(brain.readout.weights(), weights);
        if brain.adaptation.rejected == 1 {
            assert_eq!(brain.network.synapses(), before);
        }
        let loaded = Assistant::load(&dir.path().join("brain.json")).unwrap();
        assert_eq!(loaded.adaptation.attempts, 1);
        assert_eq!(loaded.experience.len(), 8);
    }
    #[test]
    fn harmful_readout_replay_is_rejected_by_retention_without_changing_source() {
        let mut brain =
            Assistant::new(Config::small(21), vec!["greeting".into(), "unknown".into()]).unwrap();
        for _ in 0..10 {
            brain.teach("hello", "greeting").unwrap();
        }
        brain.experience.clear();
        for index in 0..8 {
            brain.experience.push_back(Lesson {
                question: format!("hello {index}"),
                label: Some(
                    if index % 4 == 0 {
                        "greeting"
                    } else {
                        "unknown"
                    }
                    .into(),
                ),
                reference: String::new(),
                source: "test".into(),
                domain: "test".into(),
            });
        }
        let core = brain.network.synapses().to_vec();
        let readout = brain.readout.weights().to_vec();
        let rows: Vec<_> = brain.experience.iter().cloned().collect();
        let holdout: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 4 == 0)
            .map(|(_, r)| r.clone())
            .collect();
        let train: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 4 != 0)
            .map(|(_, r)| r.clone())
            .collect();
        let before = brain.isolated().retention(&holdout).unwrap();
        let mut candidate = brain.isolated();
        candidate.train(&train, 1).unwrap();
        let after = candidate.retention(&holdout).unwrap();
        assert!(!Assistant::retention_passed(&before, &after));
        assert_eq!(brain.network.synapses(), core);
        assert_eq!(brain.readout.weights(), readout);
    }

    #[test]
    fn a_failed_save_does_not_commit_candidate_growth() {
        let mut brain =
            Assistant::new(Config::small(21), vec!["greeting".into(), "unknown".into()]).unwrap();
        for index in 0..8 {
            brain
                .teach(&format!("hello number {index}"), "greeting")
                .unwrap();
        }
        let core = brain.network.synapses().to_vec();
        let readout = brain.readout.weights().to_vec();
        let dir = tempfile::tempdir().unwrap();
        assert!(brain.consolidate(dir.path()).is_err());
        assert_eq!(brain.network.synapses(), core);
        assert_eq!(brain.readout.weights(), readout);
        assert_eq!(brain.adaptation.accepted, 0);
    }
}

#[cfg(test)]
mod online_tests {
    use super::*;
    #[test]
    fn teaching_replay_and_feedback_automatically_update_connections() {
        let mut brain =
            Assistant::new(Config::small(21), vec!["greeting".into(), "unknown".into()]).unwrap();
        let before = brain.network.synapses().to_vec();
        brain.teach("hello friend", "greeting").unwrap();
        assert_eq!(brain.adaptation.connection_attempts, 1);
        assert_eq!(brain.adaptation.connection_accepted, 1);
        assert_ne!(brain.network.synapses(), before);
        assert!(brain.adaptation.strengthened > 0 && brain.adaptation.weakened > 0);
        brain.replay().unwrap();
        assert_eq!(brain.adaptation.connection_attempts, 2);
        brain.ask("hello friend").unwrap();
        let f = brain.features("hello friend").unwrap();
        let before = brain.readout.probabilities(&f).unwrap()[0];
        brain.feedback(-1.0).unwrap();
        let f = brain.features("hello friend").unwrap();
        assert!(brain.readout.probabilities(&f).unwrap()[0] < before);
        assert_eq!(brain.adaptation.feedback_events, 1);
        assert_eq!(brain.adaptation.connection_attempts, 3);
        assert!(brain.feedback(1.0).is_err());
    }
    #[test]
    fn generated_answer_feedback_updates_the_language_head() {
        let mut brain = Assistant::new(
            Config {
                inputs: 512,
                ..Config::small(21)
            },
            vec!["greeting".into(), "unknown".into()],
        )
        .unwrap();
        let mut readout =
            serde_json::to_value(crate::circuit::readout::Readout::new(160, 257).unwrap()).unwrap();
        readout["bias"][105] = serde_json::json!(0.5);
        brain.dialogue = Some(crate::circuit::dialogue::Dialogue {
            tokens: std::iter::once(Vec::new())
                .chain((0..=255).map(|b| vec![b]))
                .collect(),
            merges: Vec::new(),
            ticks_per_symbol: 2,
            fast_readout: false,
            readout: serde_json::from_value(readout).unwrap(),
        });
        brain.generate_continuing("hi", 8).unwrap();
        let before = brain.dialogue.as_ref().unwrap().readout.weights().to_vec();
        brain.feedback(-1.0).unwrap();
        assert_ne!(brain.dialogue.as_ref().unwrap().readout.weights(), before);
        assert_eq!(brain.adaptation.feedback_events, 1);
        assert!(brain.last_dialogue.is_none());
    }
}
