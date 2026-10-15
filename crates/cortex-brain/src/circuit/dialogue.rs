use super::{Network, readout::Readout};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Dialogue {
    pub tokens: Vec<Vec<u8>>,
    pub merges: Vec<[usize; 2]>,
    pub ticks_per_symbol: usize,
    #[serde(default)]
    pub fast_readout: bool,
    pub readout: Readout,
}

impl Dialogue {
    pub fn validate(&self, network: &Network) -> Result<(), String> {
        if !(257..=8192).contains(&self.tokens.len())
            || self.merges.len() + 257 != self.tokens.len()
            || !(1..=8).contains(&self.ticks_per_symbol)
            || network.config().inputs < 260
            || !self.tokens[0].is_empty()
        {
            return Err("Invalid circuit language vocabulary or timing.".into());
        }
        for byte in 0..256 {
            if self.tokens[byte + 1] != [byte as u8] {
                return Err("Invalid byte vocabulary.".into());
            }
        }
        for (i, [a, b]) in self.merges.iter().copied().enumerate() {
            let token = 257 + i;
            if a == 0
                || b == 0
                || a >= token
                || b >= token
                || self.tokens[token].len() > 128
                || self.tokens[token]
                    != [self.tokens[a].as_slice(), self.tokens[b].as_slice()].concat()
            {
                return Err("Invalid learned byte-pair merge.".into());
            }
        }
        self.readout
            .validate(network.neurons().len(), self.tokens.len())
    }
    pub fn encode(&self, text: &str) -> Vec<usize> {
        let mut sequence: Vec<_> = text.bytes().map(|b| b as usize + 1).collect();
        for (rank, pair) in self.merges.iter().enumerate() {
            let mut merged = Vec::new();
            let mut i = 0;
            while i < sequence.len() {
                if i + 1 < sequence.len() && sequence[i..i + 2] == *pair {
                    merged.push(257 + rank);
                    i += 2;
                } else {
                    merged.push(sequence[i]);
                    i += 1;
                }
            }
            sequence = merged;
        }
        sequence
    }
    pub fn channels(&self, token: usize) -> Result<Vec<usize>, String> {
        if token > self.tokens.len() + 2 {
            return Err("Invalid circuit language symbol.".into());
        }
        if token >= self.tokens.len() {
            let mut active: Vec<_> = (160..224).collect();
            active.push(256 + token - self.tokens.len());
            return Ok(active);
        }
        let (bits, replicas) = if self.tokens.len() <= 1024 {
            (10, 8)
        } else {
            (13, 6)
        };
        Ok((0..replicas)
            .flat_map(|replica| {
                (0..bits).map(move |bit| replica * bits * 2 + bit * 2 + ((token >> bit) & 1))
            })
            .collect())
    }
    pub fn feed(&self, network: &mut Network, token: usize) -> Result<(), String> {
        let channels = self.channels(token)?;
        for tick in 0..self.ticks_per_symbol {
            network.step(if tick % 2 == 0 { &channels } else { &[] })?;
        }
        Ok(())
    }
    pub fn question(
        &self,
        network: &mut Network,
        text: &str,
        mask_numbers: bool,
    ) -> Result<(), String> {
        self.question_state(network, text, mask_numbers, true)
    }
    pub(crate) fn question_state(
        &self,
        network: &mut Network,
        text: &str,
        mask_numbers: bool,
        reset: bool,
    ) -> Result<(), String> {
        if text.is_empty() || text.len() > 8192 {
            return Err("Supply a nonempty question up to 8192 bytes.".into());
        }
        let normalized = if mask_numbers {
            regex::Regex::new(r"\d+(?:,\d{3})*(?:\.\d+)?")
                .map_err(|e| e.to_string())?
                .replace_all(text, "#")
                .into_owned()
        } else {
            text.to_owned()
        };
        let mut sequence = vec![self.tokens.len()];
        sequence.extend(self.encode(&normalized));
        sequence.push(self.tokens.len() + 1);
        if mask_numbers && sequence.len() > 128 {
            sequence = [
                sequence[..64].to_vec(),
                sequence[sequence.len() - 64..].to_vec(),
            ]
            .concat();
        }
        if sequence.len() > 512 {
            return Err("Question exceeds the experimental decoder's 512-symbol context.".into());
        }
        if reset {
            network.reset();
        }
        for token in sequence {
            self.feed(network, token)?;
        }
        Ok(())
    }
    pub fn features(network: &Network) -> Vec<f32> {
        let mut features = network.features();
        let mean = features.iter().sum::<f32>() / features.len() as f32;
        for x in &mut features {
            *x -= mean;
        }
        let norm = features.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        for x in &mut features {
            *x = 5.0 * *x / norm;
        }
        features
    }
    pub(crate) fn language_features(&self, network: &Network) -> Vec<f32> {
        if !self.fast_readout {
            return Self::features(network);
        }
        let mut f: Vec<_> = network
            .neurons()
            .iter()
            .map(|n| (n.fast_trace + 0.1 * n.trace) / 10.0)
            .collect();
        let mean = f.iter().sum::<f32>() / f.len() as f32;
        for x in &mut f {
            *x -= mean;
        }
        let norm = f.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        for x in &mut f {
            *x = 5.0 * *x / norm;
        }
        f
    }
    pub fn generate(
        &self,
        network: &mut Network,
        text: &str,
        max_tokens: usize,
    ) -> Result<serde_json::Value, String> {
        self.generate_state(network, text, max_tokens, false)
    }
    pub fn generate_state(
        &self,
        network: &mut Network,
        text: &str,
        max_tokens: usize,
        continuous: bool,
    ) -> Result<serde_json::Value, String> {
        self.question_state(network, text, false, !continuous)?;
        let mut output = Vec::new();
        let mut selected = Vec::new();
        let mut scores = Vec::new();
        let mut reason = "length_limit";
        for _ in 0..max_tokens.clamp(1, 128) {
            let (token, score) = self.readout.predict(&self.language_features(network))?;
            scores.push(score);
            if token == 0 {
                reason = "end_symbol";
                break;
            }
            selected.push(token);
            output.extend_from_slice(&self.tokens[token]);
            if continuous {
                self.feed(network, token)?;
            }
            if selected.len() >= 5 && selected[selected.len() - 5..].iter().all(|t| *t == token) {
                reason = "repetition_limit";
                break;
            }
            if !continuous {
                self.feed(network, token)?;
            }
        }
        if continuous {
            self.feed(network, 0)?;
        }
        let valid = std::str::from_utf8(&output).is_ok();
        Ok(
            serde_json::json!({"text":String::from_utf8_lossy(&output),"method":"learned_circuit_language","tokens":selected,"token_scores":scores,"stop_reason":reason,"utf8_valid":valid,
            "continuous":continuous,"simulated_ms":network.ticks() as f32 * network.config().dt_ms,
            "scope":"Generated by the trained token readout over the shared recurrent/dendritic circuit. Continuous mode retains numerical state, not proven conversational understanding. No pretrained model or retrieved answer. Factual correctness and fluent conversation are not established."}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::Config;
    fn language() -> Dialogue {
        let tokens = std::iter::once(Vec::new())
            .chain((0..=255).map(|n| vec![n]))
            .collect();
        Dialogue {
            tokens,
            merges: Vec::new(),
            ticks_per_symbol: 2,
            fast_readout: false,
            readout: Readout::new(160, 257).unwrap(),
        }
    }
    #[test]
    fn vocabulary_and_intrinsic_state_survive_serialization() {
        let mut network = Network::new(Config {
            inputs: 512,
            ..Config::small(8)
        })
        .unwrap();
        let dialogue = language();
        dialogue.validate(&network).unwrap();
        assert_eq!(
            dialogue.encode("a Ω"),
            "a Ω".bytes().map(|b| b as usize + 1).collect::<Vec<_>>()
        );
        let bytes = serde_json::to_vec(&dialogue).unwrap();
        let restored: Dialogue = serde_json::from_slice(&bytes).unwrap();
        restored
            .question(&mut network, "What is 2 + 3?", true)
            .unwrap();
        let a = Dialogue::features(&network);
        restored
            .question(&mut network, "What is 71 + 89?", true)
            .unwrap();
        assert_eq!(a, Dialogue::features(&network));
        assert_eq!(restored.channels(1).unwrap().len(), 80);
        assert!(restored.channels(1000).is_err());
    }
    #[test]
    fn invalid_merge_and_vocabulary_are_rejected() {
        let network = Network::new(Config {
            inputs: 512,
            ..Config::small(8)
        })
        .unwrap();
        let mut dialogue = language();
        dialogue.tokens.push(vec![1]);
        dialogue.merges.push([0, 300]);
        assert!(dialogue.validate(&network).is_err());
    }
    #[test]
    fn continuous_turns_preserve_clock_and_reset_is_explicit() {
        let mut network = Network::new(Config {
            inputs: 512,
            ..Config::small(8)
        })
        .unwrap();
        let dialogue = language();
        dialogue
            .generate_state(&mut network, "hello", 4, true)
            .unwrap();
        let before = network.ticks();
        dialogue
            .generate_state(&mut network, "hello", 4, true)
            .unwrap();
        assert_eq!(network.ticks(), before * 2);
        dialogue.generate(&mut network, "hello", 4).unwrap();
        assert!(network.ticks() < before * 2);
    }
}
