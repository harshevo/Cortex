//! The models of a ChatGPT subscription, through the service the Codex
//! command-line tool talks to.
//!
//! Only the model is used. Each request carries Cortex's own instructions,
//! conversation and tools, and gets one model response back; the loop,
//! the tools and everything around them stay Cortex's.
//!
//! Requests are paid for by the plan's Codex allowance rather than per
//! token. Sign in once with `cortex login`.

mod auth;
mod login;
mod wire;

use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{ModelProvider, ModelRequest, ModelResponse, ProviderError, StreamObserver};
use serde_json::Value;

pub use self::auth::status as sign_in_status;
pub use self::login::login;

use self::auth::{Session, Tokens};
use crate::openai_compat::stream::Lines;
use crate::retry;

const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// What the service is told is calling it.
const ORIGINATOR: &str = "cortex";

/// How the service says the plan's allowance is used up. Waiting a minute
/// does not help with that, unlike with an ordinary rate limit.
const ALLOWANCE_USED_UP: &str = "usage_limit_reached";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub model: String,

    /// Reasoning depth: `low`, `medium`, `high`, or `xhigh`.
    pub effort: String,
}

impl CodexConfig {
    pub fn new(model: &str, effort: &str) -> Self {
        Self {
            model: model.to_string(),
            effort: effort.to_string(),
        }
    }

    /// The effort as this service spells it. Its scale stops at `xhigh`.
    fn wire_effort(&self) -> &str {
        match self.effort.as_str() {
            "max" => "xhigh",
            other => other,
        }
    }
}

/// One attempt's result: a response or a failure not worth repeating,
/// or else a failure the caller may retry.
type Attempt = Result<Result<ModelResponse, ProviderError>, retry::Failure>;

pub struct CodexProvider {
    config: CodexConfig,
    http: reqwest::Client,
    session: Session,

    /// Names this provider's requests as one conversation, which is what
    /// lets the service reuse its cache from one step to the next.
    conversation: String,
}

impl CodexProvider {
    /// Fails if nobody has signed in.
    pub fn new(config: CodexConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        Ok(Self {
            config,
            session: Session::open(http.clone())?,
            http,
            conversation: conversation_id()?,
        })
    }

    async fn post(
        &self,
        body: &Value,
        tokens: &Tokens,
    ) -> Result<reqwest::Response, retry::Failure> {
        self.http
            .post(format!("{BASE_URL}/responses"))
            .bearer_auth(&tokens.access)
            .header("chatgpt-account-id", &tokens.account_id)
            .header("openai-beta", "responses=experimental")
            .header("originator", ORIGINATOR)
            .header("session_id", &self.conversation)
            .header("accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .map_err(network)
    }

    /// One streamed HTTP attempt. Retrying is handled by the caller.
    async fn send(&self, body: &Value, observer: &dyn StreamObserver) -> Attempt {
        let tokens = match self.session.tokens().await {
            Ok(tokens) => tokens,
            Err(error) => return sorted(error),
        };

        observer.started();

        let mut response = self.post(body, &tokens).await?;

        // The token can be withdrawn before the time written in it.
        if response.status().as_u16() == 401 {
            let tokens = match self.session.renew(&tokens.access).await {
                Ok(tokens) => tokens,
                Err(error) => return sorted(error),
            };

            response = self.post(body, &tokens).await?;
        }

        let status = response.status();

        if !status.is_success() {
            let retry_after = retry::parse_retry_after(response.headers());

            let error = ProviderError::Http {
                status: status.as_u16(),
                body: response.text().await.map_err(network)?,
            };

            if matches!(&error, ProviderError::Http { body, .. } if body.contains(ALLOWANCE_USED_UP))
            {
                return Ok(Err(error));
            }

            return Err(retry::Failure { error, retry_after });
        }

        let mut lines = Lines::default();
        let mut assembler = wire::Assembler::default();

        while let Some(bytes) = response.chunk().await.map_err(network)? {
            for line in lines.push(&bytes) {
                match assembler.push_line(&line) {
                    Some(wire::Piece::Text(text)) => observer.text(&text),
                    Some(wire::Piece::Thinking(thinking)) => observer.thinking(&thinking),
                    Some(wire::Piece::Calling(tool)) => observer.calling(&tool),
                    None => {}
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
}

#[async_trait]
impl ModelProvider for CodexProvider {
    fn name(&self) -> &str {
        "codex"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    /// The service only streams, so this is a stream nobody watches.
    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        self.complete_streaming(request, &Unwatched).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest<'_>,
        observer: &dyn StreamObserver,
    ) -> Result<ModelResponse, ProviderError> {
        let body = wire::build_request_body(&self.config, &request, &self.conversation);

        retry::with_retries_watched(
            || self.send(&body, observer),
            |error, wait| observer.retrying(error, wait),
        )
        .await?
    }
}

struct Unwatched;

impl StreamObserver for Unwatched {
    fn text(&self, _piece: &str) {}
}

fn network(error: reqwest::Error) -> retry::Failure {
    retry::Failure::new(ProviderError::Network(error.to_string()))
}

/// Files a sign-in failure as one to retry or one to give up on.
fn sorted(error: ProviderError) -> Attempt {
    if error.is_retryable() {
        Err(retry::Failure::new(error))
    } else {
        Ok(Err(error))
    }
}

/// A random identifier in the usual 8-4-4-4-12 form.
fn conversation_id() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 16];

    getrandom::fill(&mut bytes).map_err(|error| ProviderError::Network(error.to_string()))?;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
