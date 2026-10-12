//! The Anthropic Messages API.
//!
//! There is no official Rust SDK, so this talks to `POST /v1/messages`
//! directly. Requests are not streamed yet; `max_tokens` is kept low enough
//! that a full response arrives well inside the HTTP timeout.

mod wire;

use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{ModelProvider, ModelRequest, ModelResponse, ProviderError};
use serde_json::Value;

use crate::retry;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const API_KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";
const BASE_URL_VARIABLE: &str = "ANTHROPIC_BASE_URL";

/// Beta that lets the API re-run a request on another model when a safety
/// classifier declines it, instead of stopping the run.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,

    /// Output limit for a single response, including the model's reasoning.
    pub max_tokens: u32,

    /// Reasoning depth: `low`, `medium`, `high`, `xhigh`, or `max`.
    pub effort: String,

    /// Ask the API to fall back to another model on a classifier refusal.
    pub refusal_fallback: bool,
}

impl AnthropicConfig {
    /// Reads the API key (and an optional base URL override) from the
    /// environment.
    pub fn from_env(model: &str, effort: &str) -> Result<Self, ProviderError> {
        let api_key = std::env::var(API_KEY_VARIABLE)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or(ProviderError::MissingCredentials(API_KEY_VARIABLE))?;

        Ok(Self::with_key(model, effort, &api_key))
    }

    /// The same configuration, with a key the caller already holds (for
    /// example one read from the system keychain).
    pub fn with_key(model: &str, effort: &str, api_key: &str) -> Self {
        let base_url = std::env::var(BASE_URL_VARIABLE)
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        Self {
            api_key: api_key.to_string(),
            base_url,
            model: model.to_string(),
            max_tokens: 16_000,
            effort: effort.to_string(),
            refusal_fallback: true,
        }
    }
}

pub struct AnthropicProvider {
    config: AnthropicConfig,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        Ok(Self { config, http })
    }

    /// One HTTP attempt. Retrying is handled by the caller.
    async fn send(&self, body: &Value) -> Result<Value, retry::Failure> {
        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));

        let mut request = self
            .http
            .post(url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", API_VERSION)
            .json(body);

        if self.config.refusal_fallback {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }

        let response = request
            .send()
            .await
            .map_err(|error| retry::Failure::new(ProviderError::Network(error.to_string())))?;

        let status = response.status();
        let retry_after = retry::parse_retry_after(response.headers());

        let text = response
            .text()
            .await
            .map_err(|error| retry::Failure::new(ProviderError::Network(error.to_string())))?;

        if !status.is_success() {
            let error = ProviderError::Http {
                status: status.as_u16(),
                body: text,
            };

            return Err(retry::Failure { error, retry_after });
        }

        serde_json::from_str(&text)
            .map_err(|error| retry::Failure::new(ProviderError::Decode(error.to_string())))
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        let body = wire::build_request_body(&self.config, &request);

        let json = retry::with_retries(|| self.send(&body)).await?;

        wire::parse_response(&json)
    }
}
