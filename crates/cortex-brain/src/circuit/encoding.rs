use super::Network;
use std::collections::BTreeSet;

pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn hash(bytes: &[u8]) -> usize {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    }) as usize
}

/// Fixed word/operator sensors, without pretrained vocabulary or embeddings.
pub fn channels(text: &str, inputs: usize) -> Vec<usize> {
    let mut active = BTreeSet::new();
    let tokens = words(text);
    for word in &tokens {
        active.insert(hash(word.as_bytes()) % inputs);
    }
    for symbol in text.chars().filter(|c| "+-*/=×÷".contains(*c)) {
        active.insert(hash(format!("operator:{symbol}").as_bytes()) % inputs);
    }
    active.into_iter().collect()
}

pub fn present(network: &mut Network, text: &str) -> Result<Vec<f32>, String> {
    if text.len() > 8192 {
        return Err("Circuit text exceeds 8192 bytes.".into());
    }
    network.reset();
    let channels = channels(text, network.config().inputs);
    let steps = (80.0 / network.config().dt_ms).round() as usize;
    let interval = (4.0 / network.config().dt_ms).round() as usize;
    for tick in 0..steps {
        network.step(if tick % interval == 0 { &channels } else { &[] })?;
    }
    Ok(network.features())
}
