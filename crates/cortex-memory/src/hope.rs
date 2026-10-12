//! Talking to the HOPE-style memory model served from `ml/`.
//!
//! The model re-scores the memories that similarity has found. It is an
//! adviser: if the service is down or answers nonsense, the original
//! similarity order is kept.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::similarity::Reranker;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8766";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
pub struct HopeHealth {
    /// False while the model has no trained weights and ranks exactly as
    /// plain cosine similarity does.
    pub trained: bool,

    pub dimension: usize,
}

#[derive(Deserialize)]
struct RerankResponse {
    scores: Vec<f32>,
}

#[derive(Clone)]
pub struct HopeClient {
    url: String,
    http: reqwest::Client,
}

impl HopeClient {
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

    /// `None` when the service is not running.
    pub async fn health(&self) -> Option<HopeHealth> {
        let response = self
            .http
            .get(format!("{}/health", self.url))
            .send()
            .await
            .ok()?;

        response.json().await.ok()
    }
}

#[async_trait]
impl Reranker for HopeClient {
    async fn rerank(&self, query: &[f32], candidates: &[Vec<f32>]) -> Option<Vec<f32>> {
        let body = json!({ "query": query, "candidates": candidates });

        let response = self
            .http
            .post(format!("{}/rerank", self.url))
            .json(&body)
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            return None;
        }

        let parsed: RerankResponse = response.json().await.ok()?;

        // A score that is not a number would poison the ranking.
        let usable = parsed.scores.len() == candidates.len()
            && parsed.scores.iter().all(|score| score.is_finite());

        usable.then_some(parsed.scores)
    }

    /// Writes one association into the model's fast memory. A failure is
    /// ignored: the lesson is lost, nothing else is affected.
    async fn observe(&self, query: &[f32], useful: &[f32]) {
        let body = json!({ "query": query, "useful": useful });

        let _ = self
            .http
            .post(format!("{}/observe", self.url))
            .json(&body)
            .send()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unreachable_service_changes_nothing() {
        // Port 9 is the discard service; nothing answers HTTP there.
        let client = HopeClient::new("http://127.0.0.1:9");

        assert!(client.health().await.is_none());
        assert!(client.rerank(&[1.0], &[vec![1.0]]).await.is_none());
    }
}
