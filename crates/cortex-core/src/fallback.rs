//! Carrying on with another model when the one in use is unavailable.
//!
//! A [`Fallback`] stands in front of a list of providers, best first. A
//! request goes to the first. When that fails in a way another model
//! could get past (a rate limit, an outage, a dropped connection, a
//! response that could not be read), the same request goes to the next.
//! A provider that failed so is passed over for a while before it is
//! tried first again, so a run under a rate limit does not wait out the
//! limit on every step.
//!
//! A failure that is about the request or the account (a bad key, a
//! request the provider rejects) is returned at once: another model would
//! most likely fail the same way, or should not be sent the request.
//!
//! The providers each retry on their own before they fail, so a request
//! moves on only after its provider has given up.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::error::ProviderError;
use crate::provider::{ModelProvider, ModelRequest, ModelResponse, StreamObserver};
use crate::window::PlanQuota;

/// How long a provider that failed is passed over.
pub const COOL_DOWN: Duration = Duration::from_secs(120);

pub struct Fallback {
    providers: Vec<Arc<dyn ModelProvider>>,

    /// Per provider, when it may be tried first again.
    resting_until: Mutex<Vec<Option<Instant>>>,

    cool_down: Duration,
}

impl Fallback {
    /// `providers` best first. Panics when it is empty.
    pub fn new(providers: Vec<Arc<dyn ModelProvider>>) -> Self {
        assert!(
            !providers.is_empty(),
            "a fallback needs at least one provider"
        );

        let resting_until = Mutex::new(vec![None; providers.len()]);

        Self {
            providers,
            resting_until,
            cool_down: COOL_DOWN,
        }
    }

    pub fn with_cool_down(mut self, cool_down: Duration) -> Self {
        self.cool_down = cool_down;

        self
    }

    /// One provider on its own, or a fallback over several.
    pub fn over(mut providers: Vec<Arc<dyn ModelProvider>>) -> Arc<dyn ModelProvider> {
        if providers.len() == 1 {
            return providers.remove(0);
        }

        Arc::new(Self::new(providers))
    }

    /// The order to try the providers in now: those not resting, best
    /// first, then the resting ones, so a request is never refused only
    /// because everything has failed lately.
    fn order(&self) -> Vec<usize> {
        let now = Instant::now();

        let resting = self
            .resting_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let (ready, rest): (Vec<usize>, Vec<usize>) = (0..self.providers.len())
            .partition(|&index| resting[index].is_none_or(|until| until <= now));

        ready.into_iter().chain(rest).collect()
    }

    fn rest(&self, index: usize) {
        let mut resting = self
            .resting_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        resting[index] = Some(Instant::now() + self.cool_down);
    }

    fn recovered(&self, index: usize) {
        let mut resting = self
            .resting_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        resting[index] = None;
    }

    async fn attempt(
        &self,
        request: &ModelRequest<'_>,
        observer: Option<&dyn StreamObserver>,
    ) -> Result<ModelResponse, ProviderError> {
        let order = self.order();
        let mut last_error = None;

        for (position, index) in order.iter().copied().enumerate() {
            let provider = &self.providers[index];

            let copy = ModelRequest {
                system: request.system,
                messages: request.messages,
                tools: request.tools,
            };

            let result = match observer {
                Some(observer) => provider.complete_streaming(copy, observer).await,
                None => provider.complete(copy).await,
            };

            match result {
                Ok(response) => {
                    self.recovered(index);

                    return Ok(response);
                }

                Err(error) if moves_on(&error) && position + 1 < order.len() => {
                    self.rest(index);

                    // Anything the observer saw belongs to the abandoned
                    // attempt, as with a retry.
                    if let Some(observer) = observer {
                        observer.retrying(&error, Duration::ZERO);
                    }

                    last_error = Some(error);
                }

                Err(error) => return Err(error),
            }
        }

        Err(last_error.unwrap_or_else(|| ProviderError::Failed("no provider answered".into())))
    }
}

/// Whether another model could answer where this one failed.
fn moves_on(error: &ProviderError) -> bool {
    error.is_retryable() || matches!(error, ProviderError::Decode(_))
}

#[async_trait]
impl ModelProvider for Fallback {
    fn name(&self) -> &str {
        self.providers[0].name()
    }

    fn model(&self) -> &str {
        self.providers[0].model()
    }

    fn quota(&self) -> Option<PlanQuota> {
        self.providers[0].quota()
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        self.attempt(&request, None).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest<'_>,
        observer: &dyn StreamObserver,
    ) -> Result<ModelResponse, ProviderError> {
        self.attempt(&request, Some(observer)).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::message::Message;
    use crate::provider::StopReason;
    use crate::usage::Usage;

    /// Fails with `error` the first `failures` times, then answers with
    /// its own name.
    struct Flaky {
        name: &'static str,
        failures: AtomicUsize,
        error: fn() -> ProviderError,
        calls: AtomicUsize,
    }

    impl Flaky {
        fn new(name: &'static str, failures: usize, error: fn() -> ProviderError) -> Arc<Self> {
            Arc::new(Self {
                name,
                failures: AtomicUsize::new(failures),
                error,
                calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl ModelProvider for Flaky {
        fn name(&self) -> &str {
            self.name
        }

        fn model(&self) -> &str {
            self.name
        }

        async fn complete(&self, _: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);

            if self
                .failures
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
            {
                return Err((self.error)());
            }

            Ok(ModelResponse {
                message: Message::assistant_text(self.name),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    fn rate_limited() -> ProviderError {
        ProviderError::Http {
            status: 429,
            body: "slow down".to_string(),
        }
    }

    fn bad_key() -> ProviderError {
        ProviderError::Http {
            status: 401,
            body: "no".to_string(),
        }
    }

    async fn ask(fallback: &Fallback) -> Result<String, ProviderError> {
        let request = ModelRequest {
            system: "",
            messages: &[],
            tools: &[],
        };

        fallback
            .complete(request)
            .await
            .map(|response| response.message.text())
    }

    #[tokio::test]
    async fn a_rate_limited_model_hands_over_and_rests() {
        let primary = Flaky::new("primary", 1, rate_limited);
        let backup = Flaky::new("backup", 0, rate_limited);

        let fallback = Fallback::new(vec![primary.clone(), backup.clone()]);

        assert_eq!(ask(&fallback).await.unwrap(), "backup");

        // The primary is resting, so the next request goes straight on.
        assert_eq!(ask(&fallback).await.unwrap(), "backup");
        assert_eq!(primary.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn the_primary_is_tried_first_again_after_its_rest() {
        let primary = Flaky::new("primary", 1, rate_limited);
        let backup = Flaky::new("backup", 0, rate_limited);

        let fallback = Fallback::new(vec![primary.clone(), backup]).with_cool_down(Duration::ZERO);

        assert_eq!(ask(&fallback).await.unwrap(), "backup");
        assert_eq!(ask(&fallback).await.unwrap(), "primary");
    }

    #[tokio::test]
    async fn an_account_problem_is_not_passed_on() {
        let primary = Flaky::new("primary", 1, bad_key);
        let backup = Flaky::new("backup", 0, rate_limited);

        let fallback = Fallback::new(vec![primary, backup.clone()]);

        assert!(ask(&fallback).await.is_err());
        assert_eq!(backup.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn when_every_model_fails_the_last_error_is_returned() {
        let primary = Flaky::new("primary", 5, rate_limited);
        let backup = Flaky::new("backup", 5, rate_limited);

        let fallback = Fallback::new(vec![primary, backup]);

        assert!(matches!(
            ask(&fallback).await,
            Err(ProviderError::Http { status: 429, .. })
        ));
    }
}
