//! Any server that speaks the OpenAI chat-completions format.
//!
//! This is how Cortex talks to models running on your own machine: Ollama,
//! llama.cpp, LM Studio, and vLLM all expose this API.

pub(crate) mod stream;
mod text_tool_calls;
mod wire;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{ModelProvider, ModelRequest, ModelResponse, ProviderError, StreamObserver};
use serde_json::{Value, json};

use crate::retry;

/// Ollama's default address.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434/v1";

const API_KEY_VARIABLE: &str = "CORTEX_API_KEY";

/// Local models can take minutes per response on a laptop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone)]
pub struct OpenAiCompatConfig {
    pub base_url: String,

    /// Local servers usually need no key.
    pub api_key: Option<String>,

    pub model: String,
    pub max_tokens: u32,

    /// Small models call tools more reliably at a low temperature.
    pub temperature: Option<f32>,
}

impl OpenAiCompatConfig {
    pub fn local(model: &str, base_url: Option<&str>) -> Self {
        Self {
            base_url: base_url.unwrap_or(DEFAULT_BASE_URL).to_string(),
            api_key: None,
            model: model.to_string(),
            max_tokens: 4_096,
            temperature: Some(0.2),
        }
    }

    /// A hosted service. The key is read from the `CORTEX_API_KEY`
    /// environment variable so it never appears on a command line or in a
    /// file in the project.
    pub fn hosted(model: &str, base_url: &str) -> Result<Self, ProviderError> {
        let api_key = std::env::var(API_KEY_VARIABLE)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or(ProviderError::MissingCredentials(API_KEY_VARIABLE))?;

        Ok(Self::hosted_with_key(model, base_url, &api_key))
    }

    /// A hosted service, with a key the caller already holds (for example
    /// one read from the system keychain).
    pub fn hosted_with_key(model: &str, base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            api_key: Some(api_key.to_string()),
            model: model.to_string(),
            max_tokens: 16_000,
            temperature: None,
        }
    }
}

pub struct OpenAiCompatProvider {
    config: OpenAiCompatConfig,
    http: reqwest::Client,

    /// Source of ids for tool calls the server did not give an id to.
    next_call_id: AtomicU64,
}

impl OpenAiCompatProvider {
    pub fn new(config: OpenAiCompatConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        Ok(Self {
            config,
            http,
            next_call_id: AtomicU64::new(1),
        })
    }

    /// One HTTP attempt. Retrying is handled by the caller.
    async fn send(&self, body: &Value) -> Result<Value, retry::Failure> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let mut request = self.http.post(url).json(body);

        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
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

    /// One streamed HTTP attempt. The pieces of text go to `observer` as
    /// they arrive; the return value is the whole response, in the same
    /// shape [`Self::send`] returns.
    async fn send_streaming(
        &self,
        body: &Value,
        observer: &dyn StreamObserver,
    ) -> Result<Value, retry::Failure> {
        let network =
            |error: reqwest::Error| retry::Failure::new(ProviderError::Network(error.to_string()));

        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let mut request = self.http.post(url).json(body);

        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }

        observer.started();

        let mut response = request.send().await.map_err(network)?;

        let status = response.status();
        let retry_after = retry::parse_retry_after(response.headers());

        if !status.is_success() {
            let error = ProviderError::Http {
                status: status.as_u16(),
                body: response.text().await.map_err(network)?,
            };

            return Err(retry::Failure { error, retry_after });
        }

        // A server that does not stream answers with ordinary JSON.
        if !is_event_stream(&response) {
            let text = response.text().await.map_err(network)?;

            return serde_json::from_str(&text)
                .map_err(|error| retry::Failure::new(ProviderError::Decode(error.to_string())));
        }

        let mut lines = stream::Lines::default();
        let mut assembler = stream::Assembler::default();

        while let Some(bytes) = response.chunk().await.map_err(network)? {
            for line in lines.push(&bytes) {
                for piece in assembler.push_line(&line) {
                    match piece {
                        stream::Piece::Text(text) => observer.text(&text),
                        stream::Piece::Thinking(thinking) => observer.thinking(&thinking),
                        stream::Piece::Calling(tool) => observer.calling(&tool),
                    }
                }
            }
        }

        if !assembler.finished() {
            return Err(retry::Failure::new(ProviderError::Network(
                "the response stream ended before the model finished".to_string(),
            )));
        }

        Ok(assembler.into_response())
    }

    fn fresh_call_id(&self) -> String {
        let number = self.next_call_id.fetch_add(1, Ordering::Relaxed);

        format!("call_local_{number}")
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        let body = wire::build_request_body(&self.config, &request);

        let json = retry::with_retries(|| self.send(&body)).await?;

        let tool_names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        wire::parse_response(&json, &tool_names, || self.fresh_call_id())
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest<'_>,
        observer: &dyn StreamObserver,
    ) -> Result<ModelResponse, ProviderError> {
        let mut body = wire::build_request_body(&self.config, &request);

        body["stream"] = json!(true);

        // Without this the token counts are left out of a streamed response.
        body["stream_options"] = json!({ "include_usage": true });

        let json = retry::with_retries_watched(
            || self.send_streaming(&body, observer),
            |error, wait| observer.retrying(error, wait),
        )
        .await?;

        let tool_names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        wire::parse_response(&json, &tool_names, || self.fresh_call_id())
    }
}

fn is_event_stream(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"))
}
