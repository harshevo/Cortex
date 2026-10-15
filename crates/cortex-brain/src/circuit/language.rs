mod adaptation;
mod instruction_training;
mod monitor_learning;
mod monitoring;
mod self_monitor;
mod sequence_learning;

use super::{Config, Network, Snapshot, encoding, network::Rng, readout::Readout};
use crate::brain::arithmetic;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufReader, BufWriter, Write},
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub question: String,
    pub label: Option<String>,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub text: String,
    pub program: String,
    pub confidence: f32,
    pub abstained: bool,
    pub candidates: Vec<(String, f32)>,
    pub value: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub question: String,
    pub response: Response,
}

#[derive(Clone, Serialize, Deserialize)]
struct Gate {
    confidence: f32,
    similarity: f32,
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    network: Snapshot,
    readout: Readout,
    labels: Vec<String>,
    threshold: f32,
    gates: Vec<Gate>,
    centroids: Vec<Vec<f32>>,
    presentations: Vec<u64>,
    updates: u64,
    sources: BTreeMap<String, usize>,
    experience: VecDeque<Lesson>,
    #[serde(default)]
    plasticity: Option<serde_json::Value>,
    #[serde(default)]
    dialogue: Option<super::dialogue::Dialogue>,
    #[serde(default)]
    adaptation: adaptation::Adaptation,
    #[serde(default)]
    self_monitor: Option<self_monitor::SelfMonitor>,
    #[serde(default)]
    sequence_decoder: Option<super::sequence::SequenceDecoder>,
}

#[derive(Clone)]
pub struct Assistant {
    network: Network,
    readout: Readout,
    labels: Vec<String>,
    threshold: f32,
    gates: Vec<Gate>,
    centroids: Vec<Vec<f32>>,
    presentations: Vec<u64>,
    pub updates: u64,
    sources: BTreeMap<String, usize>,
    experience: VecDeque<Lesson>,
    plasticity: Option<serde_json::Value>,
    dialogue: Option<super::dialogue::Dialogue>,
    adaptation: adaptation::Adaptation,
    self_monitor: Option<self_monitor::SelfMonitor>,
    sequence_decoder: Option<super::sequence::SequenceDecoder>,
    last: Option<Response>,
    last_dialogue: Option<(Network, String, Vec<usize>, bool)>,
    last_sequence: Option<(String, Vec<f32>, Vec<usize>)>,
    pub observations: VecDeque<Observation>,
}

#[derive(Debug, Serialize)]
pub struct Score {
    pub total: usize,
    pub selected_program: usize,
    pub answered: usize,
    pub correct: usize,
    pub unknown_selected: usize,
}

impl Assistant {
    pub fn new(config: Config, labels: Vec<String>) -> Result<Self, String> {
        if !(2..=256).contains(&labels.len())
            || !labels.iter().any(|s| s == "unknown")
            || labels.iter().any(|l| l.len() > 128)
            || labels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != labels.len()
        {
            return Err("Invalid program catalog.".into());
        }
        config.validate()?;
        let readout = Readout::new(config.neurons(), labels.len())?;
        let centroids = vec![vec![0.0; config.neurons()]; labels.len()];
        let gates = vec![
            Gate {
                confidence: 0.8,
                similarity: -1.0
            };
            labels.len()
        ];
        let presentations = vec![0; labels.len()];
        let network = Network::new(config)?;
        Ok(Self {
            network,
            readout,
            labels,
            threshold: 0.8,
            gates,
            centroids,
            presentations,
            updates: 0,
            sources: BTreeMap::new(),
            experience: VecDeque::new(),
            plasticity: None,
            dialogue: None,
            adaptation: Default::default(),
            self_monitor: None,
            sequence_decoder: None,
            last: None,
            last_dialogue: None,
            last_sequence: None,
            observations: VecDeque::new(),
        })
    }
    pub fn state(&self) -> serde_json::Value {
        let reporting = self.sequence_decoder.as_ref().map(|d| &d.training);
        let calibrated = reporting.map_or_else(
            || {
                self.self_monitor.as_ref().is_some_and(|m| {
                    m.trained_core_signature == self_monitor::signature(&self.network)
                })
            },
            |training| {
                training["core_signature"].as_u64() == Some(self_monitor::signature(&self.network))
            },
        );
        serde_json::json!({"learned_decoder_parameters":self.sequence_decoder.as_ref().map_or(0,|d|d.parameter_count()),"self_monitor":{"installed":self.self_monitor.is_some()||reporting.is_some(),"learned_reporting":reporting.is_some(),"calibration_matches_core":calibrated,"training":reporting},"adaptation":self.adaptation,"topology": {
            "neurons":self.network.neurons().len(),
            "dendrites":self.network.neurons().iter().filter(|n|n.population != super::Population::Inhibitory).map(|n|n.branches.len()).sum::<usize>(),
            "branches_per_excitatory_neuron":self.network.neurons().iter().filter(|n|n.population != super::Population::Inhibitory).map(|n|n.branches.len()).max().unwrap_or(0),
            "branches_min":self.network.neurons().iter().filter(|n|n.population != super::Population::Inhibitory).map(|n|n.branches.len()).min().unwrap_or(0),
            "branches_max":self.network.neurons().iter().filter(|n|n.population != super::Population::Inhibitory).map(|n|n.branches.len()).max().unwrap_or(0),
            "recurrent_synapses":self.network.synapses().iter().filter(|s|s.source >= self.network.config().inputs).count(),
            "input_synapses":self.network.synapses().iter().filter(|s|s.source < self.network.config().inputs).count(),
            "recurrent_fan_in":self.network.config().fan_in,
            "input_fan_in":self.network.config().input_fan_in,
            "structure":"Retention-gated growth",
            "online_learning":"Readout + gated core replay",
            "growth":"Bounded replay candidates"
        },"updates":self.updates,"plasticity":self.plasticity,"language_tokens":self.dialogue.as_ref().map(|d|d.tokens.len()),"threshold":self.threshold,"gates":self.gates,"labels":self.labels,
            "sources":self.sources,"experience":self.experience.len(),"observations":self.observations,
            "curiosity":self.observations.iter().rev().filter(|o|o.response.abstained).take(8).map(|o|&o.question).collect::<Vec<_>>(),
            "activity":self.network.activity(0),"neurons":self.network.neurons(),
            "synapses":self.network.synapses().len(),"plastic_weights":self.readout.weights().iter().map(Vec::len).sum::<usize>()+self.dialogue.as_ref().map(|d|d.readout.weights().iter().map(Vec::len).sum::<usize>()).unwrap_or(0)})
    }
    pub fn wiring(&self) -> serde_json::Value {
        serde_json::json!({"circuit":self.network.snapshot(),"output":{"labels":self.labels,"weights":self.readout.weights(),"bias":self.readout.bias()},
            "plasticity":self.plasticity,"language":self.dialogue,"scope":"All simulated connections. Output units are rate readouts over spike traces. Input sensors and exact arithmetic are supplied; training metadata identifies learned parameters."})
    }
    pub fn probe(&mut self, text: &str) -> Result<serde_json::Value, String> {
        let features = self.features(text)?;
        Ok(
            serde_json::json!({"features":features,"probabilities":self.readout.probabilities(&features)?,"neurons":self.network.neurons()}),
        )
    }
    pub fn wiring_preview(&self) -> serde_json::Value {
        let language = self.dialogue.as_ref().map(|d| {
            let shown = if d.tokens.len() * self.network.neurons().len() > 4_000_000 { 128 } else { d.tokens.len() };
            serde_json::json!({"tokens":&d.tokens[..shown],"total_tokens":d.tokens.len(),
                "readout":{"weights":&d.readout.weights()[..shown],"bias":&d.readout.bias()[..shown]},
                "preview_only":shown != d.tokens.len()})
        });
        serde_json::json!({"circuit":self.network.snapshot(),
            "output":{"labels":self.labels,"weights":self.readout.weights(),"bias":self.readout.bias()},
            "plasticity":self.plasticity,"language":language,
            "scope":"All recurrent edges; large language heads preview the first 128 token outputs. Full weights remain in the checkpoint."})
    }
    /// Starts recording each step of the circuit for live watching.
    pub fn watch(&mut self) -> std::sync::Arc<std::sync::Mutex<super::network::Live>> {
        self.network.watch()
    }
    /// Each neuron's population, for drawing them grouped.
    pub fn populations(&self) -> Vec<super::Population> {
        self.network
            .neurons()
            .iter()
            .map(|n| n.population)
            .collect()
    }
    pub fn network(&self) -> &Network {
        &self.network
    }
    pub fn labels(&self) -> &[String] {
        &self.labels
    }
    pub fn threshold(&self) -> f32 {
        self.threshold
    }
    pub fn generate(&mut self, text: &str, max_tokens: usize) -> Result<serde_json::Value, String> {
        if self.sequence_decoder.is_some() {
            return self.generate_sequence(text, max_tokens, false);
        }
        self.generate_neural(text, max_tokens)
    }
    pub fn generate_neural(
        &mut self,
        text: &str,
        max_tokens: usize,
    ) -> Result<serde_json::Value, String> {
        self.last_sequence = None;
        if self.sequence_decoder.is_some() {
            return self.generate_sequence(text, max_tokens, false);
        }
        self.last = None;
        self.last_dialogue = None;
        let initial = self.network.detached();
        let result = self
            .dialogue
            .as_ref()
            .ok_or("No learned language head is loaded.")?
            .generate_state(&mut self.network, text, max_tokens, false)?;
        let tokens = result["tokens"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|t| t.as_u64().map(|x| x as usize))
                    .collect()
            })
            .unwrap_or_default();
        self.last_dialogue = Some((initial, text.into(), tokens, false));
        Ok(result)
    }
    pub fn generate_continuing(
        &mut self,
        text: &str,
        max_tokens: usize,
    ) -> Result<serde_json::Value, String> {
        self.last_sequence = None;
        if self.sequence_decoder.is_some() {
            return self.generate_sequence(text, max_tokens, true);
        }
        self.last = None;
        self.last_dialogue = None;
        let initial = self.network.detached();
        let result = self
            .dialogue
            .as_ref()
            .ok_or("No learned language head is loaded.")?
            .generate_state(&mut self.network, text, max_tokens, true)?;
        let tokens = result["tokens"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|t| t.as_u64().map(|x| x as usize))
                    .collect()
            })
            .unwrap_or_default();
        self.last_dialogue = Some((initial, text.into(), tokens, true));
        Ok(result)
    }
    pub fn reset_activity(&mut self) {
        self.network.reset();
    }
    pub fn sequence_probe(
        &self,
        text: &str,
        activity: &[f32],
        max_tokens: usize,
    ) -> Result<serde_json::Value, String> {
        self.sequence_decoder
            .as_ref()
            .ok_or("No learned sequence decoder is installed")?
            .generate(
                self.dialogue.as_ref().ok_or("No tokenizer is installed")?,
                text,
                activity,
                max_tokens,
            )
    }
    pub fn activity_probe(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut network = self.network.detached();
        self.dialogue
            .as_ref()
            .ok_or("No tokenizer")?
            .question(&mut network, text, false)?;
        Ok(self_monitor::state_features(&network))
    }
    fn generate_sequence(
        &mut self,
        text: &str,
        max_tokens: usize,
        continuous: bool,
    ) -> Result<serde_json::Value, String> {
        self.last = None;
        self.last_dialogue = None;
        self.last_sequence = None;
        let features = self_monitor::state_features(&self.network);
        let mut result = self.sequence_probe(text, &features, max_tokens)?;
        if self
            .sequence_decoder
            .as_ref()
            .is_some_and(|d| d.training["retention_examples"].is_array())
        {
            let mut tokens: Vec<usize> =
                serde_json::from_value(result["tokens"].clone()).map_err(|e| e.to_string())?;
            if result["stop_reason"].as_str() == Some("end_symbol") && tokens.len() < 128 {
                tokens.push(0);
            }
            if !tokens.is_empty() {
                self.last_sequence = Some((text.into(), features, tokens));
                result["feedback_supported"] = serde_json::json!(true);
            }
        }
        let dialogue = self.dialogue.as_ref().ok_or("No tokenizer is installed")?;
        if !continuous {
            self.network.reset();
        }
        dialogue.feed(&mut self.network, dialogue.tokens.len())?;
        for token in dialogue.encode(text) {
            dialogue.feed(&mut self.network, token)?;
        }
        dialogue.feed(&mut self.network, dialogue.tokens.len() + 1)?;
        if let Some(tokens) = result["tokens"].as_array() {
            for token in tokens {
                dialogue.feed(
                    &mut self.network,
                    token.as_u64().ok_or("Invalid generated symbol")? as usize,
                )?;
            }
        }
        dialogue.feed(&mut self.network, 0)?;
        result["continuous"] = serde_json::json!(continuous);
        result["simulated_ms"] =
            serde_json::json!(self.network.ticks() as f32 * self.network.config().dt_ms);
        Ok(result)
    }
    pub fn sense(&mut self, frames: &[Vec<usize>]) -> Result<(), String> {
        if frames.is_empty()
            || frames.len() > 10_000
            || frames.iter().any(|f| {
                f.len() > self.network.config().inputs
                    || f.iter().any(|&c| c >= self.network.config().inputs)
                    || f.iter().collect::<std::collections::HashSet<_>>().len() != f.len()
            })
        {
            return Err("Supply 1..10000 frames with unique valid input channels.".into());
        }
        for frame in frames {
            self.network.step(frame)?;
        }
        Ok(())
    }
    fn features(&mut self, text: &str) -> Result<Vec<f32>, String> {
        if self.plasticity.as_ref().and_then(|p| p["encoder"].as_str())
            == Some("ordered_bpe_question_v1")
        {
            let dialogue = self
                .dialogue
                .as_ref()
                .ok_or("Ordered circuit encoder requires its vocabulary.")?;
            dialogue.question(&mut self.network, text, true)?;
            return Ok(super::dialogue::Dialogue::features(&self.network));
        }
        let mut f = encoding::present(&mut self.network, text)?;
        let mean = f.iter().sum::<f32>() / f.len() as f32;
        for x in &mut f {
            *x -= mean;
        }
        let norm = f.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        for x in &mut f {
            *x = 5.0 * *x / norm;
        }
        Ok(f)
    }
    pub fn train(&mut self, lessons: &[Lesson], epochs: usize) -> Result<Vec<f32>, String> {
        self.last_sequence = None;
        if lessons.is_empty() || lessons.len() > 16_000 || !(1..=200).contains(&epochs) {
            return Err("Use 1..16000 lessons and 1..200 epochs.".into());
        }
        let mut features = Vec::new();
        for row in lessons {
            let target = self
                .labels
                .iter()
                .position(|l| Some(l) == row.label.as_ref())
                .ok_or("Lesson program is absent from catalog.")?;
            let f = self.features(&row.question)?;
            self.observe_features(&f, target);
            features.push((f, target));
            *self.sources.entry(row.source.clone()).or_default() += 1;
        }
        let mut rng = Rng::new(self.network.config().seed ^ self.updates.wrapping_add(71));
        let mut indices: Vec<_> = (0..features.len()).collect();
        let mut losses = Vec::new();
        for epoch in 0..epochs {
            for i in (1..indices.len()).rev() {
                let j = rng.index(i + 1);
                indices.swap(i, j);
            }
            let rate = 0.12 / (1.0 + epoch as f32 / 50.0);
            let mut loss = 0.0;
            for &i in &indices {
                loss += self.readout.teach(&features[i].0, features[i].1, rate)?;
                self.updates = self.updates.saturating_add(1);
            }
            losses.push(loss / features.len() as f32);
        }
        Ok(losses)
    }
    pub fn teach(&mut self, text: &str, label: &str) -> Result<(), String> {
        self.last_sequence = None;
        if text.trim().is_empty() || text.len() > 8192 {
            return Err("Supply a nonempty lesson up to 8192 bytes.".into());
        }
        if !self.labels.iter().any(|l| l == label) {
            return Err(format!("Choose a program from: {}", self.labels.join(", ")));
        }
        let row = Lesson {
            question: text.into(),
            label: Some(label.into()),
            reference: String::new(),
            source: "user correction".into(),
            domain: "user".into(),
        };
        let f = self.features(text)?;
        let target = self
            .labels
            .iter()
            .position(|l| l == label)
            .ok_or("Unknown program.")?;
        self.observe_features(&f, target);
        self.gates[target].confidence = self.gates[target].confidence.min(0.8);
        self.gates[target].similarity = self.gates[target]
            .similarity
            .min(self.similarity(&f, target) - 0.01);
        for _ in 0..20 {
            self.readout.teach(&f, target, 0.1)?;
            self.updates = self.updates.saturating_add(1);
        }
        self.experience.push_back(row);
        while self.experience.len() > 256 {
            self.experience.pop_front();
        }
        self.learn_connections(&[f], 1.0)?;
        Ok(())
    }
    fn observe_features(&mut self, features: &[f32], target: usize) {
        let count = self.presentations[target].saturating_add(1);
        for (mean, x) in self.centroids[target].iter_mut().zip(features) {
            *mean += (*x - *mean) / count as f32;
        }
        self.presentations[target] = count;
    }
    fn similarity(&self, f: &[f32], target: usize) -> f32 {
        let mean = &self.centroids[target];
        let norm =
            (f.iter().map(|v| v * v).sum::<f32>() * mean.iter().map(|v| v * v).sum::<f32>()).sqrt();
        if norm < 1e-8 {
            return 0.0;
        }
        (f.iter().zip(mean).map(|(x, y)| x * y).sum::<f32>() / norm).clamp(-1.0, 1.0)
    }
    pub fn calibrate(&mut self, dev: &[Lesson]) -> Result<f32, String> {
        // Development-only acceptance calibration; no stored example answers at inference.
        let mut predictions = vec![Vec::new(); self.labels.len()];
        self.gates = vec![
            Gate {
                confidence: 0.0,
                similarity: -1.0
            };
            self.labels.len()
        ];
        for row in dev {
            self.last = None;
            let response = self.ask(&row.question)?;
            let i = self
                .labels
                .iter()
                .position(|l| *l == response.program)
                .ok_or("Invalid selected program.")?;
            let f = self.features(&row.question)?;
            let correct = !response.abstained
                && Some(&response.program) == row.label.as_ref()
                && (row.reference.is_empty()
                    || response
                        .value
                        .as_ref()
                        .is_some_and(|v| numeric_equal(v, &row.reference)));
            predictions[i].push((response.confidence, self.similarity(&f, i), correct));
        }
        for (i, predictions) in predictions.iter().enumerate() {
            let mut best = 0;
            let mut chosen = Gate {
                confidence: 1.01,
                similarity: 1.0,
            };
            let needed = if matches!(
                self.labels[i].as_str(),
                "greeting" | "identity" | "capabilities"
            ) {
                1
            } else {
                2
            };
            for probability in 50..=100 {
                let p = probability as f32 / 100.0;
                let selected: Vec<_> = predictions
                    .iter()
                    .filter(|(confidence, _, _)| *confidence >= p)
                    .collect();
                if selected.len() >= needed
                    && selected.iter().all(|(_, _, correct)| *correct)
                    && selected.len() > best
                {
                    best = selected.len();
                    chosen = Gate {
                        confidence: p,
                        similarity: -1.0,
                    };
                }
            }
            self.gates[i] = chosen;
        }
        self.threshold = self.gates.iter().map(|g| g.confidence).fold(1.01, f32::min);
        self.observations.clear();
        self.last = None;
        Ok(self.threshold)
    }
    pub fn ask(&mut self, text: &str) -> Result<Response, String> {
        self.last_sequence = None;
        self.last_dialogue = None;
        if text.trim().is_empty() || text.len() > 8192 {
            return Err("Supply a nonempty question up to 8192 bytes.".into());
        }
        let normalized = text.trim().trim_end_matches('?').to_lowercase();
        if matches!(
            normalized.as_str(),
            "why" | "explain" | "why that answer" | "how did you get that"
        ) && let Some(last) = &self.last
        {
            let mut response = last.clone();
            response.text = last.evidence.join(" ");
            return Ok(response);
        }
        let features = self.features(text)?;
        let (index, confidence) = self.readout.predict(&features)?;
        let label = self.labels[index].clone();
        let mut candidates: Vec<_> = self
            .readout
            .probabilities(&features)?
            .into_iter()
            .enumerate()
            .map(|(i, p)| (self.labels[i].clone(), p))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.truncate(3);
        let learning = if self.plasticity.as_ref().is_some_and(|p| {
            p["method"].as_str() == Some("surrogate_bptt")
                || p["recurrent_trained"].as_bool() == Some(true)
        }) {
            "Recurrent/dendritic synapses and intrinsic parameters were trained with surrogate BPTT"
        } else {
            "The last training phase held recurrent connections fixed"
        };
        let mut result=Response {text:"I haven't learned to answer that reliably. You can teach me a supported question-to-operation mapping.".into(),program:label.clone(),confidence,abstained:true,candidates,value:None,
            evidence:vec![format!("Plastic output synapses selected {label} with score {confidence:.3}. {learning}; the score is not a calibrated probability of correctness.")]};
        let similarity = self.similarity(&features, index);
        result.evidence.push(format!("Similarity to the learned class mean (diagnostic): {similarity:.3}; acceptance gates: score >= {:.2}, similarity >= {:.2}.",self.gates[index].confidence,self.gates[index].similarity));
        if confidence >= self.gates[index].confidence
            && similarity >= self.gates[index].similarity
            && label != "unknown"
        {
            match label.as_str() {
                "greeting" => {
                    result.text="Hello. I can try a limited set of English, arithmetic and physics questions.".into();
                    result.abstained = false;
                }
                "identity" => {
                    result.text="I'm an experimental spiking circuit with plastic output connections. My activity does not establish subjective experience.".into();
                    result.abstained = false;
                }
                "capabilities" => {
                    result.text="I've trained to select a small set of operations from English questions. I can try arithmetic, net force, average speed and kinetic energy. I can be wrong on new wording.".into();
                    result.abstained = false;
                }
                _ => {
                    let expression = match label.as_str() {
                        "force" => "(a*b)",
                        "speed" => "(a/b)",
                        "kinetic" => "(0.5*a*(b*b))",
                        other => other,
                    };
                    let binding = if matches!(label.as_str(), "force" | "speed" | "kinetic") {
                        physics_bind(&label, expression, text)
                    } else {
                        bind(expression, text)
                    };
                    if let Ok((expression, calculation)) = binding.and_then(|expression| {
                        arithmetic::evaluate(&expression)
                            .map(|calculation| (expression, calculation))
                    }) {
                        let value = calculation.value.to_string();
                        let unit = match label.as_str() {
                            "force" => " N",
                            "speed" => " m/s",
                            "kinetic" => " J",
                            _ => "",
                        };
                        result.text = format!("{value}{unit}");
                        result.value = Some(value);
                        result.abstained = false;
                        result.evidence.push(format!("Learned operation: {label}. Supplied exact arithmetic executes {expression}; the English renderer and physics units are supplied."));
                        result.evidence.extend(calculation.steps);
                        if label == "force" {
                            result.evidence.push("Assuming net force and constant mass; supplied units converted to SI: F = mass × acceleration.".into());
                        }
                        if label == "speed" {
                            result.evidence.push("Average speed = distance / elapsed time, assuming metres and seconds.".into());
                        }
                        if label == "kinetic" {
                            result.evidence.push("Classical kinetic energy = mass × speed² / 2, assuming kilograms and metres per second.".into());
                        }
                    }
                }
            }
        }
        self.last = Some(result.clone());
        self.observations.push_back(Observation {
            question: text.into(),
            response: result.clone(),
        });
        while self.observations.len() > 16 {
            self.observations.pop_front();
        }
        Ok(result)
    }
    pub fn score(&mut self, rows: &[Lesson]) -> Result<BTreeMap<String, Score>, String> {
        let mut scores = BTreeMap::new();
        for row in rows {
            self.last = None;
            let r = self.ask(&row.question)?;
            let score = scores.entry(row.domain.clone()).or_insert(Score {
                total: 0,
                selected_program: 0,
                answered: 0,
                correct: 0,
                unknown_selected: 0,
            });
            score.total += 1;
            score.unknown_selected += usize::from(r.program == "unknown");
            score.selected_program += usize::from(Some(&r.program) == row.label.as_ref());
            score.answered += usize::from(!r.abstained);
            let correct = if row.reference.is_empty() {
                Some(&r.program) == row.label.as_ref()
            } else {
                r.value
                    .as_ref()
                    .is_some_and(|v| numeric_equal(v, &row.reference))
            };
            score.correct += usize::from(!r.abstained && correct);
        }
        Ok(scores)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let checkpoint = Checkpoint {
            version: 2,
            network: self.network.snapshot(),
            readout: self.readout.clone(),
            labels: self.labels.clone(),
            threshold: self.threshold,
            gates: self.gates.clone(),
            centroids: self.centroids.clone(),
            presentations: self.presentations.clone(),
            updates: self.updates,
            sources: self.sources.clone(),
            experience: self.experience.clone(),
            plasticity: self.plasticity.clone(),
            dialogue: self.dialogue.clone(),
            adaptation: self.adaptation.clone(),
            self_monitor: self.self_monitor.clone(),
            sequence_decoder: self.sequence_decoder.clone(),
        };
        {
            let mut writer = BufWriter::new(temp.as_file_mut());
            serde_json::to_writer(&mut writer, &checkpoint).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;
        }
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 1024 << 20 {
            return Err("Circuit checkpoint exceeds 1 GiB.".into());
        }
        let c: Checkpoint =
            serde_json::from_reader(BufReader::new(file)).map_err(|e| e.to_string())?;
        if c.version != 2
            || !c.threshold.is_finite()
            || !(0.0..=1.01).contains(&c.threshold)
            || c.experience.len() > 256
        {
            return Err("Invalid language checkpoint.".into());
        }
        if !(2..=256).contains(&c.labels.len())
            || !c.labels.iter().any(|s| s == "unknown")
            || c.labels.iter().any(|s| s.len() > 128)
            || c.labels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != c.labels.len()
        {
            return Err("Invalid checkpoint program catalog.".into());
        }
        if c.gates.len() != c.labels.len()
            || c.centroids.len() != c.labels.len()
            || c.presentations.len() != c.labels.len()
            || c.gates.iter().any(|g| {
                !g.confidence.is_finite()
                    || !(0.0..=1.01).contains(&g.confidence)
                    || !g.similarity.is_finite()
                    || !(-1.0..=1.0).contains(&g.similarity)
            })
            || c.centroids
                .iter()
                .any(|v| v.len() != c.network.config.neurons() || v.iter().any(|x| !x.is_finite()))
        {
            return Err("Invalid circuit-state acceptance model.".into());
        }
        let network = Network::restore(c.network)?;
        c.readout
            .validate(network.neurons().len(), c.labels.len())?;
        if let Some(monitor) = &c.self_monitor {
            monitor.validate()?;
        }
        if let Some(decoder) = &c.sequence_decoder {
            decoder.validate(
                c.dialogue
                    .as_ref()
                    .ok_or("Learned decoder needs a tokenizer")?
                    .tokens
                    .len(),
            )?;
        }
        if let Some(d) = &c.dialogue {
            d.validate(&network)?;
        }
        if c.plasticity.as_ref().and_then(|p| p["encoder"].as_str())
            == Some("ordered_bpe_question_v1")
            && c.dialogue.is_none()
        {
            return Err("Missing ordered circuit vocabulary.".into());
        }
        Ok(Self {
            network,
            readout: c.readout,
            labels: c.labels,
            threshold: c.threshold,
            gates: c.gates,
            centroids: c.centroids,
            presentations: c.presentations,
            updates: c.updates,
            sources: c.sources,
            experience: c.experience,
            plasticity: c.plasticity,
            dialogue: c.dialogue,
            adaptation: c.adaptation,
            self_monitor: c.self_monitor,
            sequence_decoder: c.sequence_decoder,
            last: None,
            last_dialogue: None,
            last_sequence: None,
            observations: VecDeque::new(),
        })
    }
}

pub fn read_lessons(path: &Path) -> Result<Vec<Lesson>, String> {
    use std::io::BufRead;
    let mut rows = Vec::new();
    for line in
        std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?).lines()
    {
        let line = line.map_err(|e| e.to_string())?;
        if line.len() > 32_768 || rows.len() >= 16_000 {
            return Err("Lesson file exceeds limits.".into());
        }
        rows.push(serde_json::from_str(&line).map_err(|e| e.to_string())?);
    }
    Ok(rows)
}

fn bind(program: &str, question: &str) -> Result<String, String> {
    let regex = regex::Regex::new(r"(?:^|[^\w.])(-?[0-9]+(?:,[0-9]{3})*(?:\.[0-9]+)?)")
        .map_err(|e| e.to_string())?;
    let numbers: Vec<_> = regex
        .captures_iter(question)
        .filter_map(|c| c.get(1).map(|m| m.as_str().replace(',', "")))
        .collect();
    bind_values(program, &numbers)
}

fn physics_bind(label: &str, program: &str, text: &str) -> Result<String, String> {
    let normalized = text.to_lowercase().replace('²', "^2");
    let regex=regex::Regex::new(r"(-?[0-9]+(?:\.[0-9]+)?)\s*(m/s\^2|m/s|km/h|kilograms?|kg|grams?|g|metres?|meters?|cm|km|m|seconds?|minutes|min|ms|s)\b").map_err(|e|e.to_string())?;
    let mut quantities = BTreeMap::new();
    for c in regex.captures_iter(&normalized) {
        let (scale, dimension) =
            crate::brain::formula::unit(&c[2]).ok_or("Unsupported physics unit.")?;
        let value = arithmetic::evaluate(&c[1])?.value.apply('*', scale)?;
        if quantities.insert(dimension, value.to_string()).is_some() {
            return Err("Ambiguous repeated physical quantity.".into());
        }
    }
    let dimensions = match label {
        "force" => [[1, 0, 0], [0, 1, -2]],
        "speed" => [[0, 1, 0], [0, 0, 1]],
        "kinetic" => [[1, 0, 0], [0, 1, -1]],
        _ => return Err("Unknown physics operation.".into()),
    };
    if quantities.len() != 2 {
        return Err("This physics operation needs exactly two unambiguous quantities with compatible units.".into());
    }
    let values = dimensions
        .iter()
        .map(|d| {
            quantities
                .get(d)
                .cloned()
                .ok_or("Incompatible physics units.".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    bind_values(program, &values)
}

fn bind_values(program: &str, numbers: &[String]) -> Result<String, String> {
    let mut expression = String::new();
    for token in program.chars() {
        if ('a'..='e').contains(&token) {
            expression.push('(');
            expression.push_str(
                numbers
                    .get(token as usize - 'a' as usize)
                    .ok_or("Missing numeric operand.")?,
            );
            expression.push(')');
        } else if token.is_ascii_digit() || ".()+-*/".contains(token) {
            expression.push(token);
        } else {
            return Err("Unsupported program token.".into());
        }
    }
    Ok(expression)
}
fn numeric_equal(value: &str, reference: &str) -> bool {
    match (
        arithmetic::evaluate(value),
        arithmetic::evaluate(&reference.replace(',', "")),
    ) {
        (Ok(a), Ok(b)) => a.value == b.value,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operands_are_bound_without_teaching_question_answers() {
        assert_eq!(
            arithmetic::evaluate(&bind("(a+b)", "What is 124 + 307?").unwrap())
                .unwrap()
                .value
                .to_string(),
            "431"
        );
        assert!(bind("((a+b)*c)", "Add 2 and 3").is_err());
        assert!(bind("system()", "2 3").is_err());
    }
    #[test]
    fn physics_binding_checks_units_and_is_order_independent() {
        let value = physics_bind("force", "(a*b)", "acceleration 4 m/s^2 and mass 500 g").unwrap();
        assert_eq!(arithmetic::evaluate(&value).unwrap().value.to_string(), "2");
        let value = physics_bind("speed", "(a/b)", "time 2 s and distance 10 m").unwrap();
        assert_eq!(arithmetic::evaluate(&value).unwrap().value.to_string(), "5");
        assert!(physics_bind("force", "(a*b)", "mass 2 seconds and acceleration 3 kg").is_err());
        assert!(
            physics_bind(
                "force",
                "(a*b)",
                "mass 2 kg and mass 3 kg and acceleration 4 m/s^2"
            )
            .is_err()
        );
        assert!(
            physics_bind(
                "force",
                "(a*b)",
                "mass 2 elephants and acceleration 3 m/s^2"
            )
            .is_err()
        );
        assert!(physics_bind("speed", "(a/b)", "distance 3 mm and time 2 s").is_err());
    }
    #[test]
    fn numeric_transfer_does_not_read_a_stored_answer() {
        let mut n = Network::new(Config::small(8)).unwrap();
        let a = encoding::present(&mut n, "What is 1 + 3?").unwrap();
        let b = encoding::present(&mut n, "What is 17 + 209?").unwrap();
        assert_eq!(a, b);
        assert_ne!(
            arithmetic::evaluate(&bind("(a+b)", "1 + 3").unwrap())
                .unwrap()
                .value,
            arithmetic::evaluate(&bind("(a+b)", "17 + 209").unwrap())
                .unwrap()
                .value
        );
    }
    #[test]
    fn correction_and_replay_persist_without_modifying_legacy_brain() {
        let mut brain =
            Assistant::new(Config::small(11), vec!["(a+b)".into(), "unknown".into()]).unwrap();
        brain.teach("What is 1 + 3?", "(a+b)").unwrap();
        brain.threshold = 0.5;
        assert_eq!(
            brain.ask("What is 11 + 32?").unwrap().value.as_deref(),
            Some("43")
        );
        assert_eq!(brain.replay().unwrap(), 1);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("circuit.json");
        brain.save(&path).unwrap();
        let mut restored = Assistant::load(&path).unwrap();
        assert_eq!(restored.updates, brain.updates);
        assert_eq!(restored.experience.len(), 1);
        let before = serde_json::to_vec(&restored.readout).unwrap();
        assert_eq!(
            restored.ask("What is 211 + 903?").unwrap().value.as_deref(),
            Some("1114")
        );
        assert!(
            restored.ask("why").unwrap().text.contains("supplied")
                || restored.ask("why").unwrap().text.contains("Supplied")
        );
        assert_eq!(before, serde_json::to_vec(&restored.readout).unwrap());
    }
    #[test]
    fn invalid_saved_readout_is_rejected() {
        let readout: Readout =
            serde_json::from_value(serde_json::json!({"weights":[],"bias":[],"updates":0}))
                .unwrap();
        assert!(readout.probabilities(&[1.0]).is_err());
        assert!(readout.validate(1, 2).is_err());
        assert!(
            Assistant::new(Config::small(1), vec!["unknown".into(), "unknown".into()]).is_err()
        );
    }
}

#[cfg(test)]
mod calibration_tests {
    use super::*;
    #[test]
    fn high_scores_are_rejected_when_development_answers_are_wrong() {
        let mut brain =
            Assistant::new(Config::small(41), vec!["(a+b)".into(), "unknown".into()]).unwrap();
        brain.teach("What is 2 + 3?", "(a+b)").unwrap();
        let wiring = brain.network().synapses().to_vec();
        let rows = vec![
            Lesson {
                question: "What is 2 + 3?".into(),
                label: Some("(a+b)".into()),
                reference: "999".into(),
                source: "unit test".into(),
                domain: "test".into()
            };
            2
        ];
        brain.calibrate(&rows).unwrap();
        assert!(brain.ask("What is 2 + 3?").unwrap().abstained);
        assert_eq!(brain.network().synapses(), wiring);
        assert!(brain.teach("", "(a+b)").is_err());
        assert!(brain.ask("").is_err());
    }
}
