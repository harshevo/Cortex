//! Error types shared across crates.

use thiserror::Error;

use crate::agent::Progress;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing credentials: set the `{0}` environment variable")]
    MissingCredentials(&'static str),

    #[error("request failed with HTTP {status}: {body}")]
    Http { status: u16, body: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("could not decode the provider response: {0}")]
    Decode(String),

    /// There is no usable sign-in for a provider that is reached through
    /// an account rather than a key. The text says what to do about it.
    #[error("{0}")]
    SignIn(String),

    /// The provider took the request and then said it could not answer it.
    #[error("the provider could not answer: {0}")]
    Failed(String),
}

impl ProviderError {
    /// Whether sending the same request again could succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { status, .. } => matches!(status, 408 | 409 | 429 | 500..=599),
            Self::Network(_) => true,
            Self::MissingCredentials(_) | Self::Decode(_) | Self::SignIn(_) | Self::Failed(_) => {
                false
            }
        }
    }

    /// Whether the provider turned the request away because of the key or
    /// the account, so that no request will work until the user fixes it.
    pub fn is_account_problem(&self) -> bool {
        match self {
            Self::MissingCredentials(_) | Self::SignIn(_) => true,
            Self::Http { status, .. } => matches!(status, 401..=403),
            Self::Network(_) | Self::Decode(_) | Self::Failed(_) => false,
        }
    }
}

/// A tool failure. The message is shown to the model so it can recover.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("{0}")]
    Failed(String),

    #[error("the user declined this action")]
    Denied,
}

#[derive(Debug, Error)]
pub enum AgentError {
    /// The provider failed part-way through a run.
    ///
    /// Nothing the run had done is lost: the conversation still holds every
    /// finished step, and `progress` holds the counts. Hand both to
    /// [`crate::Agent::resume`] to carry on from the step that failed.
    #[error("{error}")]
    Provider {
        #[source]
        error: ProviderError,

        progress: Progress,
    },
}
