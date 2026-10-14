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

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{
    ModelProvider, ModelRequest, ModelResponse, PlanQuota, ProviderError, QuotaWindow,
    StreamObserver,
};
use reqwest::header::HeaderMap;
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

    /// How much of the plan's allowance is used, as the service last
    /// said in the headers of a response.
    quota: Mutex<Option<PlanQuota>>,
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
            quota: Mutex::new(None),
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

        // Sent with failures too: a used-up allowance says when it is
        // back.
        if let Some(quota) = quota_in(response.headers(), unix_millis()) {
            *self
                .quota
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(quota);
        }

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

    fn quota(&self) -> Option<PlanQuota> {
        *self
            .quota
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

fn unix_millis() -> u64 {
    cortex_core::trajectory::unix_millis() as u64
}

/// How much of the plan's allowance is used, from a response's headers.
///
/// The names are the ones the open-source Codex command-line tool reads
/// from this service: `x-codex-primary-used-percent`,
/// `x-codex-primary-window-minutes`, and for when the window starts
/// again either `x-codex-primary-reset-at` (seconds since 1970) or the
/// older `x-codex-primary-reset-after-seconds`; the same with
/// `secondary`. They are not a documented interface, so each is taken
/// when present and its absence is not an error. `None` when neither
/// window is reported.
fn quota_in(headers: &HeaderMap, now_ms: u64) -> Option<PlanQuota> {
    let number =
        |name: String| -> Option<f64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };

    let window = |which: &str| -> Option<QuotaWindow> {
        let used_percent = number(format!("x-codex-{which}-used-percent"))?;

        let resets_at_ms = number(format!("x-codex-{which}-reset-at"))
            .map(|seconds| (seconds * 1000.0) as u64)
            .or_else(|| {
                number(format!("x-codex-{which}-reset-after-seconds"))
                    .map(|seconds| now_ms + (seconds * 1000.0) as u64)
            });

        Some(QuotaWindow {
            used_percent,
            window_minutes: number(format!("x-codex-{which}-window-minutes"))
                .map(|minutes| minutes as u64),
            resets_at_ms,
        })
    };

    let (primary, secondary) = (window("primary"), window("secondary"));

    (primary.is_some() || secondary.is_some()).then_some(PlanQuota {
        primary,
        secondary,
        observed_ms: now_ms,
    })
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

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderName, HeaderValue};

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test]
    fn the_allowance_is_read_from_the_headers_that_are_there() {
        let quota = quota_in(
            &headers(&[
                ("x-codex-primary-used-percent", "41.5"),
                ("x-codex-primary-window-minutes", "300"),
                ("x-codex-primary-reset-after-seconds", "90"),
                ("x-codex-secondary-used-percent", "12"),
                ("x-codex-secondary-reset-at", "1791158400"),
            ]),
            1_000_000,
        )
        .unwrap();

        let primary = quota.primary.unwrap();

        assert_eq!(primary.used_percent, 41.5);
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.resets_at_ms, Some(1_090_000));

        let secondary = quota.secondary.unwrap();

        assert_eq!(secondary.used_percent, 12.0);
        assert_eq!(secondary.window_minutes, None);
        assert_eq!(secondary.resets_at_ms, Some(1_791_158_400_000));

        assert_eq!(quota.observed_ms, 1_000_000);
    }

    #[test]
    fn a_response_that_says_nothing_about_the_allowance_reports_none() {
        assert_eq!(
            quota_in(&headers(&[("content-type", "text/event-stream")]), 5),
            None
        );
    }
}
