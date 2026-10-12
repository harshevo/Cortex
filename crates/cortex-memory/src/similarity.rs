//! Deciding how alike a new task and a stored memory are.
//!
//! The embedding service gives the better answer. When it is not running,
//! a comparison of the words in both texts stands in, so memory keeps
//! working without it.

use std::collections::HashSet;

use async_trait::async_trait;
use cortex_retrieval::{Chunk, EmbeddingClient};

/// Words too common to say anything about a task.
const STOPWORDS: [&str; 28] = [
    "the", "and", "for", "that", "this", "with", "from", "are", "was", "not", "but", "has", "have",
    "its", "can", "should", "when", "which", "into", "then", "they", "them", "one", "all", "any",
    "fix", "bug", "does",
];

/// Turns text into vectors that are close when the texts are about the
/// same thing. `None` means the embedder is unavailable right now.
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed_query(&self, text: &str) -> Option<Vec<f32>>;

    async fn embed_documents(&self, texts: &[String]) -> Option<Vec<Vec<f32>>>;
}

#[async_trait]
impl Embedder for EmbeddingClient {
    async fn embed_query(&self, text: &str) -> Option<Vec<f32>> {
        EmbeddingClient::embed_query(self, text).await.ok()
    }

    async fn embed_documents(&self, texts: &[String]) -> Option<Vec<Vec<f32>>> {
        let chunks: Vec<Chunk> = texts
            .iter()
            .map(|text| Chunk {
                path: String::new(),
                start_line: 0,
                end_line: 0,
                title: "earlier task".to_string(),
                text: text.clone(),
            })
            .collect();

        self.embed_chunks(&chunks).await.ok()
    }
}

/// Re-scores memories that plain similarity has already shortlisted.
///
/// This is where a learned memory model plugs in: it sees the task and the
/// candidates and may reorder them. `None` keeps the original order.
#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &[f32], candidates: &[Vec<f32>]) -> Option<Vec<f32>>;

    /// Tells the model that the memory with vector `useful` turned out to
    /// help with the task whose vector is `query`, so it can adapt at once
    /// without being retrained. Models that cannot do this ignore it.
    async fn observe(&self, _query: &[f32], _useful: &[f32]) {}
}

/// Cosine similarity. The embedding service returns unit vectors, but
/// stored vectors may come from elsewhere, so the lengths are not assumed.
pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();

    let norm = |vector: &[f32]| vector.iter().map(|value| value * value).sum::<f32>().sqrt();

    let scale = norm(left) * norm(right);

    if scale == 0.0 {
        return 0.0;
    }

    dot / scale
}

/// Share of distinctive words two texts have in common, from 0 to 1.
pub fn lexical(left: &str, right: &str) -> f32 {
    let left = words(left);
    let right = words(right);

    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let shared = left.intersection(&right).count() as f32;

    shared / ((left.len() * right.len()) as f32).sqrt()
}

/// The distinctive lower-case words of `text`. Identifiers are split, so
/// `fillMatching` and `fill-matching.ts` both yield `fill` and `matching`.
pub fn words(text: &str) -> HashSet<String> {
    let mut words = HashSet::new();
    let mut current = String::new();
    let mut previous_lower = false;

    let mut flush = |current: &mut String| {
        if current.chars().count() >= 3 && !STOPWORDS.contains(&current.as_str()) {
            words.insert(std::mem::take(current));
        } else {
            current.clear();
        }
    };

    for character in text.chars() {
        if !character.is_alphanumeric() {
            flush(&mut current);

            previous_lower = false;

            continue;
        }

        // A capital after a lower-case letter starts a new word.
        if character.is_uppercase() && previous_lower {
            flush(&mut current);
        }

        previous_lower = character.is_lowercase();

        current.extend(character.to_lowercase());
    }

    flush(&mut current);

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_split_into_words() {
        let found = words("fix fillMatching in fill-matching.ts");

        assert!(found.contains("fill"));
        assert!(found.contains("matching"));
        assert!(!found.contains("fix"));
    }

    #[test]
    fn related_texts_score_higher_than_unrelated_ones() {
        let task = "the trade calendar drops the last week of the month";

        let related = lexical(task, "calendar month view shows trades on the wrong day");
        let unrelated = lexical(task, "commission rounding drifts by one cent on import");

        assert!(related > unrelated);
        assert_eq!(lexical(task, ""), 0.0);
    }

    #[test]
    fn cosine_of_a_vector_with_itself_is_one() {
        let vector = [0.5, 1.5, -2.0];

        assert!((cosine(&vector, &vector) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&vector, &[0.0, 0.0, 0.0]), 0.0);
    }
}
