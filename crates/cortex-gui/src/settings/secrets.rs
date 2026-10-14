//! API keys, kept in the system keychain rather than in a file.

use std::collections::HashMap;
use std::sync::Mutex;

use keyring::Entry;

use super::Provider;

/// What the keychain answered for each provider, so that it is asked
/// once rather than every time an agent is built. Saving or removing a
/// key here keeps it in step.
static ANSWERS: Mutex<Option<HashMap<Provider, Option<String>>>> = Mutex::new(None);

fn answers<R>(with: impl FnOnce(&mut HashMap<Provider, Option<String>>) -> R) -> R {
    let mut answers = ANSWERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    with(answers.get_or_insert_with(HashMap::new))
}

/// The name the keys are filed under in the keychain.
const SERVICE: &str = "Cortex";

fn entry(provider: Provider) -> Result<Entry, String> {
    let account = match provider {
        Provider::Anthropic => "anthropic",
        Provider::Hosted => "hosted",
        Provider::Local => "local",
        Provider::Codex => "codex",
    };

    Entry::new(SERVICE, account).map_err(|error| error.to_string())
}

/// The key saved for `provider`, if there is one. The keychain is asked
/// the first time; it can take a while and may ask the person, so this
/// is not for the window's thread.
pub fn load(provider: Provider) -> Option<String> {
    if let Some(known) = answers(|answers| answers.get(&provider).cloned()) {
        return known;
    }

    let key = entry(provider)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .filter(|key| !key.trim().is_empty());

    answers(|answers| answers.insert(provider, key.clone()));

    key
}

pub fn save(provider: Provider, key: &str) -> Result<(), String> {
    entry(provider)?
        .set_password(key.trim())
        .map_err(|error| format!("the keychain did not accept the key: {error}"))?;

    answers(|answers| answers.insert(provider, Some(key.trim().to_string())));

    Ok(())
}

pub fn remove(provider: Provider) -> Result<(), String> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            answers(|answers| answers.insert(provider, None));

            Ok(())
        }

        Err(error) => Err(format!("the keychain did not remove the key: {error}")),
    }
}

/// The key the agent should use: the saved one, or failing that the one
/// in the provider's environment variable.
pub fn resolve(provider: Provider) -> Option<String> {
    if !provider.needs_key() {
        return None;
    }

    load(provider).or_else(|| from_environment(provider))
}

pub fn from_environment(provider: Provider) -> Option<String> {
    std::env::var(provider.key_variable()?)
        .ok()
        .filter(|key| !key.trim().is_empty())
}
