//! Talking to the local embedding service.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use crate::chunker::Chunk;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8765";

/// Documents sent per request. Keeps single requests short enough that a
/// slow laptop does not hit the timeout while indexing a large project.
const BATCH_SIZE: usize = 48;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error(
        "the embedding service at {url} is not reachable ({reason}). Start it with \
         `uv run --project ml python -m cortex_ml.embedding.server`"
    )]
    Unreachable { url: String, reason: String },

    #[error("the embedding service returned an error: {0}")]
    Service(String),

    #[error("the embedding service returned {got} vectors for {expected} inputs")]
    WrongCount { expected: usize, got: usize },
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub model: String,
    pub dimension: usize,
    pub device: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    vectors: Vec<Vec<f32>>,
}

#[derive(Clone)]
pub struct EmbeddingClient {
    url: String,
    http: reqwest::Client,
}

impl EmbeddingClient {
    pub fn new(url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();

        Self {
            url: url.trim_end_matches('/').to_string(),
            http,
        }
    }

    pub async fn health(&self) -> Result<Health, EmbeddingError> {
        let response = self
            .http
            .get(format!("{}/health", self.url))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| self.unreachable(error))?;

        response
            .json()
            .await
            .map_err(|error| EmbeddingError::Service(error.to_string()))
    }

    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>, EmbeddingError> {
        let body = json!({ "kind": "query", "texts": [query] });

        let mut vectors = self.post(&body, 1).await?;

        Ok(vectors.remove(0))
    }

    pub async fn embed_chunks(&self, chunks: &[Chunk]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut vectors = Vec::with_capacity(chunks.len());

        for batch in chunks.chunks(BATCH_SIZE) {
            let documents: Vec<_> = batch
                .iter()
                .map(|chunk| json!({ "title": chunk.title, "text": chunk.text }))
                .collect();

            let body = json!({ "kind": "document", "documents": documents });

            vectors.extend(self.post(&body, batch.len()).await?);
        }

        Ok(vectors)
    }

    async fn post(
        &self,
        body: &serde_json::Value,
        expected: usize,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let response = self
            .http
            .post(format!("{}/embed", self.url))
            .json(body)
            .send()
            .await
            .map_err(|error| self.unreachable(error))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();

            return Err(EmbeddingError::Service(text));
        }

        let parsed: EmbedResponse = response
            .json()
            .await
            .map_err(|error| EmbeddingError::Service(error.to_string()))?;

        if parsed.vectors.len() != expected {
            return Err(EmbeddingError::WrongCount {
                expected,
                got: parsed.vectors.len(),
            });
        }

        Ok(parsed.vectors)
    }

    fn unreachable(&self, error: reqwest::Error) -> EmbeddingError {
        EmbeddingError::Unreachable {
            url: self.url.clone(),
            reason: error.to_string(),
        }
    }
}
