//! Retrying transient API failures with backoff.

use std::future::Future;
use std::time::Duration;

use cortex_core::ProviderError;
use reqwest::header::HeaderMap;

/// Enough patience to sit out a per-minute rate limit several times over.
/// Giving up on a request throws away the whole run it belongs to, which
/// costs far more than waiting.
const MAX_RETRIES: u32 = 10;

/// A rate limit is not a fault: the request will be accepted once the
/// provider's window moves on. On a heavily limited route that can take
/// many minutes, so these get far more patience than other failures
/// (about an hour at the usual 30 to 90 second waits).
const MAX_RATE_LIMIT_RETRIES: u32 = 60;
const BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(90);

/// A failed attempt, with the server's wait hint when it sent one.
pub struct Failure {
    pub error: ProviderError,
    pub retry_after: Option<Duration>,
}

impl Failure {
    pub fn new(error: ProviderError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

/// Runs `attempt` until it succeeds, fails permanently, or runs out of
/// retries. Rate limits, overload, and network errors are retried.
pub async fn with_retries<T, F, Fut>(attempt: F) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    with_retries_watched(attempt, |_, _| {}).await
}

/// [`with_retries`], telling `waiting` about each failure that is waited
/// out, and for how long.
pub async fn with_retries_watched<T, F, Fut, W>(
    mut attempt: F,
    mut waiting: W,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
    W: FnMut(&ProviderError, Duration),
{
    let mut retries = 0;

    loop {
        let failure = match attempt().await {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };

        if !failure.error.is_retryable() || retries >= retry_limit(&failure.error) {
            return Err(failure.error);
        }

        let delay = failure
            .retry_after
            .unwrap_or_else(|| backoff(retries))
            .min(MAX_DELAY);

        waiting(&failure.error, delay);

        tokio::time::sleep(delay).await;

        retries += 1;
    }
}

fn retry_limit(error: &ProviderError) -> u32 {
    match error {
        ProviderError::Http { status: 429, .. } => MAX_RATE_LIMIT_RETRIES,
        _ => MAX_RETRIES,
    }
}

/// 1s, 2s, 4s, ... capped by the caller at `MAX_DELAY`.
fn backoff(retries: u32) -> Duration {
    BASE_DELAY * 2u32.pow(retries.min(10))
}

pub fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds: f64 = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;

    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }

    Some(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_get_more_patience_than_other_failures() {
        let rate_limited = ProviderError::Http {
            status: 429,
            body: String::new(),
        };

        let overloaded = ProviderError::Http {
            status: 503,
            body: String::new(),
        };

        assert_eq!(retry_limit(&rate_limited), MAX_RATE_LIMIT_RETRIES);
        assert_eq!(retry_limit(&overloaded), MAX_RETRIES);
        assert_eq!(
            retry_limit(&ProviderError::Network(String::new())),
            MAX_RETRIES
        );
    }
}
