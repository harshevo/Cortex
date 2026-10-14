//! The ChatGPT sign-in the Codex service accepts.
//!
//! `cortex login` (see [`super::login`]) leaves an access token and a
//! refresh token in `~/.cortex/codex-auth.json`. The access token lasts a
//! limited time; this module swaps it for a new one when it is about to
//! run out, and saves the result.
//!
//! Without that file, the sign-in the Codex command-line tool keeps in
//! `~/.codex/auth.json` is used as it is. Cortex never renews or rewrites
//! that one: renewing replaces the refresh token, which would sign the
//! Codex tool out.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cortex_core::ProviderError;
use serde_json::{Value, json};
use tokio::sync::Mutex;

/// The identifier of the Codex command-line tool. The sign-in service
/// issues tokens for the Codex service only to this client.
pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub(super) const ISSUER: &str = "https://auth.openai.com";

/// The claim in a token that holds the ChatGPT account.
const ACCOUNT_CLAIM: &str = "https://api.openai.com/auth";

/// A token this close to running out is replaced before it is used.
const EXPIRY_MARGIN_SECONDS: u64 = 120;

const NOT_SIGNED_IN: &str =
    "not signed in to ChatGPT: sign in from the app's settings, or run `cortex login`";

const SIGN_IN_EXPIRED: &str =
    "the ChatGPT sign-in has run out: sign in again from the app's settings, or run `cortex login`";

#[derive(Clone)]
pub struct Tokens {
    pub access: String,
    pub account_id: String,

    /// Absent for a sign-in borrowed from the Codex tool.
    refresh: Option<String>,
}

impl Tokens {
    /// Reads what the sign-in service returned for a login or a renewal.
    pub(super) fn from_grant(grant: &Value) -> Option<Self> {
        let text = |name: &str| grant.get(name).and_then(Value::as_str);

        let access = text("access_token")?;

        let account_id = text("id_token")
            .and_then(account_of)
            .or_else(|| account_of(access))?;

        Some(Self {
            access: access.to_string(),
            account_id,
            refresh: text("refresh_token").map(str::to_string),
        })
    }

    pub(super) fn save(&self) -> Result<(), ProviderError> {
        let unwritable = |error: std::io::Error| {
            ProviderError::SignIn(format!("cannot save the ChatGPT sign-in: {error}"))
        };

        let path = own_file().ok_or_else(|| {
            ProviderError::SignIn("this machine has no home directory".to_string())
        })?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(unwritable)?;
        }

        let contents = json!({
            "access_token": self.access,
            "refresh_token": self.refresh,
            "account_id": self.account_id,
        });

        let mut options = fs::OpenOptions::new();

        options.write(true).create(true).truncate(true);

        // Readable by this user only, as the tokens act for the account.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

        let mut file = options.open(&path).map_err(unwritable)?;

        file.write_all(contents.to_string().as_bytes())
            .map_err(unwritable)
    }
}

/// Where the tokens came from, which decides how they are renewed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Cortex,
    CodexTool,
}

/// The sign-in for one provider, kept current across its requests.
pub struct Session {
    http: reqwest::Client,
    state: Mutex<(Tokens, Origin)>,
}

impl Session {
    pub fn open(http: reqwest::Client) -> Result<Self, ProviderError> {
        let state = read_own()
            .map(|tokens| (tokens, Origin::Cortex))
            .or_else(|| read_codex_tool().map(|tokens| (tokens, Origin::CodexTool)))
            .ok_or_else(|| ProviderError::SignIn(NOT_SIGNED_IN.to_string()))?;

        Ok(Self {
            http,
            state: Mutex::new(state),
        })
    }

    /// Tokens that are good to send now.
    pub async fn tokens(&self) -> Result<Tokens, ProviderError> {
        let mut state = self.state.lock().await;

        if expires_soon(&state.0.access) {
            state.0 = self.replacement(&state.0, state.1).await?;
        }

        Ok(state.0.clone())
    }

    /// New tokens after the service turned `rejected` away. Requests made
    /// at the same time share one renewal.
    pub async fn renew(&self, rejected: &str) -> Result<Tokens, ProviderError> {
        let mut state = self.state.lock().await;

        if state.0.access == rejected {
            state.0 = self.replacement(&state.0, state.1).await?;
        }

        Ok(state.0.clone())
    }

    async fn replacement(&self, old: &Tokens, origin: Origin) -> Result<Tokens, ProviderError> {
        match origin {
            Origin::Cortex => {
                let tokens = self.refresh(old).await?;

                tokens.save()?;

                Ok(tokens)
            }

            // The Codex tool renews its own file whenever it runs.
            Origin::CodexTool => read_codex_tool()
                .filter(|tokens| tokens.access != old.access && !expires_soon(&tokens.access))
                .ok_or_else(|| ProviderError::SignIn(SIGN_IN_EXPIRED.to_string())),
        }
    }

    async fn refresh(&self, old: &Tokens) -> Result<Tokens, ProviderError> {
        let network = |error: reqwest::Error| ProviderError::Network(error.to_string());

        let refresh = old
            .refresh
            .as_deref()
            .ok_or_else(|| ProviderError::SignIn(SIGN_IN_EXPIRED.to_string()))?;

        let response = self
            .http
            .post(format!("{ISSUER}/oauth/token"))
            .json(&json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh,
                "scope": "openid profile email",
            }))
            .send()
            .await
            .map_err(network)?;

        let status = response.status().as_u16();

        // The refresh token itself is no longer accepted.
        if matches!(status, 400 | 401 | 403) {
            return Err(ProviderError::SignIn(SIGN_IN_EXPIRED.to_string()));
        }

        if !response.status().is_success() {
            return Err(ProviderError::Http {
                status,
                body: response.text().await.map_err(network)?,
            });
        }

        let grant: Value = response.json().await.map_err(network)?;

        let mut tokens = Tokens::from_grant(&grant).unwrap_or_else(|| Tokens {
            access: grant["access_token"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            account_id: old.account_id.clone(),
            refresh: None,
        });

        if tokens.access.is_empty() {
            return Err(ProviderError::Decode(
                "the sign-in service returned no access token".to_string(),
            ));
        }

        // A renewal does not always come with a new refresh token.
        tokens.refresh = tokens.refresh.or_else(|| old.refresh.clone());

        Ok(tokens)
    }
}

/// Which sign-in a provider would use, for showing to the person.
pub fn status() -> Option<&'static str> {
    if read_own().is_some() {
        Some("Signed in to ChatGPT.")
    } else if read_codex_tool().is_some() {
        Some("Using the Codex tool's sign-in.")
    } else {
        None
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn own_file() -> Option<PathBuf> {
    Some(home()?.join(".cortex").join("codex-auth.json"))
}

fn read_json(path: PathBuf) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn read_own() -> Option<Tokens> {
    let saved = read_json(own_file()?)?;

    Some(Tokens {
        access: saved["access_token"].as_str()?.to_string(),
        account_id: saved["account_id"].as_str()?.to_string(),
        refresh: saved["refresh_token"].as_str().map(str::to_string),
    })
}

fn read_codex_tool() -> Option<Tokens> {
    let folder = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home()?.join(".codex")))?;

    let saved = read_json(folder.join("auth.json"))?;

    let access = saved.pointer("/tokens/access_token")?.as_str()?;

    let account_id = saved
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| account_of(access))?;

    Some(Tokens {
        access: access.to_string(),
        account_id,
        refresh: None,
    })
}

/// The readable middle part of a token.
fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;

    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

fn account_of(token: &str) -> Option<String> {
    claims(token)?
        .get(ACCOUNT_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_string)
}

/// A token that does not say when it runs out is taken to be good; the
/// service has the last word either way.
fn expires_soon(token: &str) -> bool {
    let Some(expiry) = claims(token).and_then(|claims| claims.get("exp")?.as_u64()) else {
        return false;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();

    expiry <= now + EXPIRY_MARGIN_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(claims: Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    #[test]
    fn a_token_past_its_time_expires_soon() {
        assert!(expires_soon(&token(json!({ "exp": 1_000 }))));
        assert!(!expires_soon(&token(json!({ "exp": 99_999_999_999u64 }))));
        assert!(!expires_soon("not a token"));
    }

    #[test]
    fn the_account_is_read_from_a_grant() {
        let id_token = token(json!({ ACCOUNT_CLAIM: { "chatgpt_account_id": "account_1" } }));

        let grant = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "id_token": id_token,
        });

        let tokens = Tokens::from_grant(&grant).unwrap();

        assert_eq!(tokens.account_id, "account_1");
        assert_eq!(tokens.refresh.as_deref(), Some("refresh"));
    }
}
