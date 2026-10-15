use super::dialogue::Dialogue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Tensor {
    Vector(Vec<f32>),
    Matrix(Vec<Vec<f32>>),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    vocabulary: usize,
    hidden: usize,
    embedding: usize,
    state_features: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SequenceDecoder {
    version: u32,
    settings: Settings,
    pub training: Value,
    weights: BTreeMap<String, Tensor>,
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}
fn softmax(values: &mut [f32]) {
    let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for value in values.iter_mut() {
        *value = (*value - maximum).exp();
        sum += *value;
    }
    for value in values {
        *value /= sum.max(1e-12);
    }
}

impl SequenceDecoder {
    pub fn parameter_count(&self) -> usize {
        self.weights
            .iter()
            .filter(|(name, _)| !matches!(name.as_str(), "state_mean" | "state_scale"))
            .map(|(_, value)| match value {
                Tensor::Vector(v) => v.len(),
                Tensor::Matrix(m) => m.iter().map(Vec::len).sum(),
            })
            .sum()
    }
    fn vector(&self, name: &str) -> Result<&[f32], String> {
        match self.weights.get(name) {
            Some(Tensor::Vector(value)) => Ok(value),
            _ => Err(format!("Missing decoder vector {name}")),
        }
    }
    fn matrix(&self, name: &str) -> Result<&[Vec<f32>], String> {
        match self.weights.get(name) {
            Some(Tensor::Matrix(value)) => Ok(value),
            _ => Err(format!("Missing decoder matrix {name}")),
        }
    }
    pub fn validate(&self, vocabulary: usize) -> Result<(), String> {
        let s = &self.settings;
        if self.version != 1
            || s.vocabulary != vocabulary
            || !(257..=8192).contains(&vocabulary)
            || !(16..=1024).contains(&s.hidden)
            || !(16..=512).contains(&s.embedding)
            || s.state_features != 84
        {
            return Err("Invalid learned sequence decoder dimensions".into());
        }
        let (h, e) = (s.hidden, s.embedding);
        let mut shapes = vec![("embedding.weight".to_string(), vocabulary + 1, e)];
        for (name, input) in [
            ("encoder_forward", e),
            ("encoder_reverse", e),
            ("decoder", e + 3 * h),
        ] {
            for (suffix, columns) in [("weight_ih_l0", input), ("weight_hh_l0", h)] {
                shapes.push((format!("{name}.{suffix}"), 3 * h, columns));
            }
            for suffix in ["bias_ih_l0", "bias_hh_l0"] {
                if self.vector(&format!("{name}.{suffix}"))?.len() != 3 * h {
                    return Err("Invalid GRU bias".into());
                }
            }
        }
        for (name, output, input) in [
            ("state_input", h, 84),
            ("initial", h, 3 * h),
            ("keys", h, 2 * h),
            ("query", h, h),
            ("output_hidden", h, 3 * h + e),
            ("output", vocabulary, h),
            ("copy_gate", 1, 3 * h + e),
        ] {
            shapes.push((format!("{name}.weight"), output, input));
            if self.vector(&format!("{name}.bias"))?.len() != output {
                return Err("Invalid decoder bias".into());
            }
        }
        for (name, rows, columns) in shapes {
            let value = self.matrix(&name)?;
            if value.len() != rows || value.iter().any(|r| r.len() != columns) {
                return Err(format!("Invalid shape for {name}"));
            }
        }
        if self.vector("state_mean")?.len() != 84
            || self.vector("state_scale")?.len() != 84
            || self.vector("state_scale")?.iter().any(|x| *x < 1e-6)
        {
            return Err("Invalid decoder activity normalization".into());
        }
        for tensor in self.weights.values() {
            let valid = match tensor {
                Tensor::Vector(v) => v.iter().all(|x| x.is_finite() && x.abs() < 1e6),
                Tensor::Matrix(m) => m.iter().flatten().all(|x| x.is_finite() && x.abs() < 1e6),
            };
            if !valid {
                return Err("Nonfinite or unbounded sequence parameters".into());
            }
        }
        if self.weights.len() != 29 {
            return Err("Unexpected sequence parameter set".into());
        }
        Ok(())
    }
    fn affine(&self, weight: &str, bias: &str, input: &[f32]) -> Result<Vec<f32>, String> {
        Ok(self
            .matrix(weight)?
            .iter()
            .zip(self.vector(bias)?)
            .map(|(row, bias)| row.iter().zip(input).map(|(w, x)| w * x).sum::<f32>() + bias)
            .collect())
    }
    fn linear(&self, name: &str, input: &[f32]) -> Result<Vec<f32>, String> {
        self.affine(&format!("{name}.weight"), &format!("{name}.bias"), input)
    }
    fn gru(&self, name: &str, input: &[f32], hidden: &[f32]) -> Result<Vec<f32>, String> {
        let x = self.affine(
            &format!("{name}.weight_ih_l0"),
            &format!("{name}.bias_ih_l0"),
            input,
        )?;
        let previous = self.affine(
            &format!("{name}.weight_hh_l0"),
            &format!("{name}.bias_hh_l0"),
            hidden,
        )?;
        let h = self.settings.hidden;
        Ok((0..h)
            .map(|i| {
                let reset = sigmoid(x[i] + previous[i]);
                let update = sigmoid(x[h + i] + previous[h + i]);
                let candidate = (x[2 * h + i] + reset * previous[2 * h + i]).tanh();
                (1.0 - update) * candidate + update * hidden[i]
            })
            .collect())
    }
    pub fn generate(
        &self,
        dialogue: &Dialogue,
        text: &str,
        activity: &[f32],
        maximum: usize,
    ) -> Result<Value, String> {
        self.forward(dialogue, text, activity, maximum, None)
    }
    pub fn reference_loss(
        &self,
        dialogue: &Dialogue,
        text: &str,
        activity: &[f32],
        target: &[usize],
    ) -> Result<f32, String> {
        Ok(
            self.forward(dialogue, text, activity, target.len(), Some(target))?["reference_loss"]
                .as_f64()
                .ok_or("Missing sequence loss")? as f32,
        )
    }
    pub fn train_output(
        &mut self,
        dialogue: &Dialogue,
        text: &str,
        activity: &[f32],
        target: &[usize],
        reward: f32,
        rate: f32,
    ) -> Result<(), String> {
        if !reward.is_finite()
            || !rate.is_finite()
            || reward.abs() > 1.0
            || !(0.0..=0.05).contains(&rate)
        {
            return Err("Invalid sequence feedback".into());
        }
        let result = self.forward(dialogue, text, activity, target.len(), Some(target))?;
        let gradient: Vec<f32> =
            serde_json::from_value(result["bias_gradient"].clone()).map_err(|e| e.to_string())?;
        let Some(Tensor::Vector(bias)) = self.weights.get_mut("output.bias") else {
            return Err("Missing output bias".into());
        };
        for (value, gradient) in bias.iter_mut().zip(gradient) {
            *value = (*value - rate * reward * gradient).clamp(-20.0, 20.0);
        }
        Ok(())
    }
    fn forward(
        &self,
        dialogue: &Dialogue,
        text: &str,
        activity: &[f32],
        maximum: usize,
        target: Option<&[usize]>,
    ) -> Result<Value, String> {
        if let Some(tokens) = target
            && (tokens.is_empty()
                || tokens.len() > 128
                || tokens.iter().any(|t| *t >= self.settings.vocabulary))
        {
            return Err("Invalid sequence training target".into());
        }
        if text.is_empty()
            || text.len() > 8192
            || activity.len() != 84
            || activity.iter().any(|x| !x.is_finite())
        {
            return Err("Invalid learned decoder input".into());
        }
        let source = dialogue.encode(text);
        if source.is_empty() || source.len() > 512 {
            return Err("Learned decoder input exceeds 512 symbols".into());
        }
        let embeddings = self.matrix("embedding.weight")?;
        let h = self.settings.hidden;
        let mut forward = Vec::with_capacity(source.len());
        let mut hidden = vec![0.0; h];
        for token in &source {
            hidden = self.gru("encoder_forward", &embeddings[*token], &hidden)?;
            forward.push(hidden.clone());
        }
        let forward_last = hidden;
        let mut reverse = Vec::with_capacity(source.len());
        let mut hidden = vec![0.0; h];
        for token in source.iter().rev() {
            hidden = self.gru("encoder_reverse", &embeddings[*token], &hidden)?;
            reverse.push(hidden.clone());
        }
        let reverse_last = hidden;
        reverse.reverse();
        let memory: Vec<Vec<f32>> = forward
            .into_iter()
            .zip(reverse)
            .map(|(a, b)| [a, b].concat())
            .collect();
        let keys: Vec<_> = memory
            .iter()
            .map(|row| self.linear("keys", row))
            .collect::<Result<_, _>>()?;
        let normalized: Vec<_> = activity
            .iter()
            .zip(self.vector("state_mean")?)
            .zip(self.vector("state_scale")?)
            .map(|((x, m), s)| ((x - m) / s).clamp(-8.0, 8.0))
            .collect();
        let state: Vec<_> = self
            .linear("state_input", &normalized)?
            .into_iter()
            .map(f32::tanh)
            .collect();
        let context = [forward_last, reverse_last, state].concat();
        let mut hidden: Vec<_> = self
            .linear("initial", &context)?
            .into_iter()
            .map(f32::tanh)
            .collect();
        let mut previous = self.settings.vocabulary;
        let mut selected = Vec::new();
        let mut scores = Vec::new();
        let mut first = Vec::new();
        let mut reason = "length_limit";
        let mut gradient = vec![0.0; self.settings.vocabulary];
        let mut reference_loss = 0.0;
        for step in 0..maximum.clamp(1, 128) {
            let embedded = &embeddings[previous];
            hidden = self.gru(
                "decoder",
                &[embedded.as_slice(), context.as_slice()].concat(),
                &hidden,
            )?;
            let query = self.linear("query", &hidden)?;
            let mut attention: Vec<_> = keys
                .iter()
                .map(|row| {
                    row.iter().zip(&query).map(|(a, b)| a * b).sum::<f32>() / (h as f32).sqrt()
                })
                .collect();
            softmax(&mut attention);
            let mut attended = vec![0.0; h * 2];
            for (row, weight) in memory.iter().zip(&attention) {
                for (value, feature) in attended.iter_mut().zip(row) {
                    *value += weight * feature;
                }
            }
            let combined = [hidden.as_slice(), attended.as_slice(), embedded.as_slice()].concat();
            let value: Vec<_> = self
                .linear("output_hidden", &combined)?
                .into_iter()
                .map(f32::tanh)
                .collect();
            let mut probabilities = self.linear("output", &value)?;
            softmax(&mut probabilities);
            let generated = probabilities.clone();
            let gate = sigmoid(self.linear("copy_gate", &combined)?[0]);
            for value in &mut probabilities {
                *value *= gate;
            }
            for (token, weight) in source.iter().zip(attention) {
                probabilities[*token] += (1.0 - gate) * weight;
            }
            if selected.is_empty() {
                first = probabilities.clone();
            }
            let (predicted, _) = probabilities
                .iter()
                .copied()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .ok_or("Empty decoder vocabulary")?;
            let token = target.map_or(predicted, |tokens| tokens[step]);
            let score = probabilities[token].max(1e-9);
            if let Some(tokens) = target {
                let factor = gate * generated[token] / score / tokens.len() as f32;
                reference_loss -= score.ln() / tokens.len() as f32;
                for (index, value) in gradient.iter_mut().enumerate() {
                    *value += factor * (generated[index] - f32::from(index == token));
                }
            }
            scores.push(score);
            if token == 0 {
                reason = "end_symbol";
                break;
            }
            selected.push(token);
            previous = token;
        }
        let output: Vec<_> = selected
            .iter()
            .flat_map(|token| dialogue.tokens[*token].iter().copied())
            .collect();
        let mut result = json!({"text":String::from_utf8_lossy(&output),"tokens":selected,"token_scores":scores,"first_token_probabilities":first,"stop_reason":reason,"method":"learned_attentive_circuit_decoder","feedback_supported":false,"scope":"Tokens generated by a trained recurrent attention/copy mixture over the instruction and 84 current circuit statistics. No task-specific instruction executor or response-letter formatter. Reliability and consciousness are not established."});
        if target.is_some() {
            result["reference_loss"] = json!(reference_loss);
            result["bias_gradient"] = json!(gradient);
        }
        Ok(result)
    }
}
