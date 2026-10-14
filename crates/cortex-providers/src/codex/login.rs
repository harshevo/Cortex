//! Signing in to ChatGPT in the browser.
//!
//! This is the flow the Codex command-line tool uses: the browser is sent
//! to the sign-in page, and when the person has signed in it is sent back
//! to a small server on this machine with a code, which is exchanged for
//! tokens. The password is only ever typed into the browser.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cortex_core::ProviderError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::auth::{CLIENT_ID, ISSUER, Tokens};

/// The sign-in service sends the browser back to this port and no other.
const PORT: u16 = 1455;

const CALLBACK_PATH: &str = "/auth/callback";

const SCOPE: &str = "openid profile email offline_access";

const WAIT_FOR_BROWSER: Duration = Duration::from_secs(300);

const REQUEST_LIMIT: usize = 16 * 1024;

/// Signs in and saves the tokens. `announce` is given the address of the
/// sign-in page, for the person to open if the browser does not.
pub async fn login(announce: impl FnOnce(&str)) -> Result<(), ProviderError> {
    let verifier = random_text()?;
    let state = random_text()?;

    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let redirect = format!("http://localhost:{PORT}{CALLBACK_PATH}");

    let listener = TcpListener::bind(("127.0.0.1", PORT))
        .await
        .map_err(|error| {
            ProviderError::SignIn(format!(
                "cannot listen on port {PORT} for the sign-in (is another login running?): {error}"
            ))
        })?;

    let address = format!(
        "{ISSUER}/oauth/authorize?{}",
        query(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect),
            ("scope", SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("state", &state),
        ])
    );

    announce(&address);
    open_browser(&address);

    let code = tokio::time::timeout(WAIT_FOR_BROWSER, wait_for_code(&listener, &state))
        .await
        .map_err(|_| ProviderError::SignIn("gave up waiting for the sign-in".to_string()))??;

    exchange(&code, &verifier, &redirect).await?.save()
}

/// Serves requests until the browser comes back with the code.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, ProviderError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        let Some(target) = read_target(&mut stream).await else {
            continue;
        };

        let Some(parameters) = target
            .strip_prefix(CALLBACK_PATH)
            .and_then(|rest| rest.strip_prefix('?'))
        else {
            respond(&mut stream, "404 Not Found", "Not found.").await;

            continue;
        };

        let value = |name: &str| {
            parameters
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| decode(value))
        };

        // Anything not carrying this login's state did not come from it.
        if value("state").as_deref() != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "This sign-in was not started here.",
            )
            .await;

            continue;
        }

        if let Some(code) = value("code") {
            respond(&mut stream, "200 OK", "Signed in. You can close this tab.").await;

            return Ok(code);
        }

        let reason = value("error_description")
            .or_else(|| value("error"))
            .unwrap_or_else(|| "no code was returned".to_string());

        respond(
            &mut stream,
            "400 Bad Request",
            "The sign-in did not complete.",
        )
        .await;

        return Err(ProviderError::SignIn(format!(
            "the sign-in did not complete: {reason}"
        )));
    }
}

/// The path and query of a `GET` request.
async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 2048];

    while !request.windows(2).any(|pair| pair == b"\r\n") && request.len() < REQUEST_LIMIT {
        let count = stream.read(&mut buffer).await.ok()?;

        if count == 0 {
            break;
        }

        request.extend_from_slice(&buffer[..count]);
    }

    let text = String::from_utf8_lossy(&request);
    let mut words = text.lines().next()?.split_whitespace();

    (words.next()? == "GET").then(|| words.next().map(str::to_string))?
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let page = format!("<!doctype html><title>Cortex</title><p>{message}</p>");

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );

    // The browser may have gone; the login does not depend on the page.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn exchange(code: &str, verifier: &str, redirect: &str) -> Result<Tokens, ProviderError> {
    let network = |error: reqwest::Error| ProviderError::Network(error.to_string());

    let form = query(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", CLIENT_ID),
        ("code_verifier", verifier),
    ]);

    let response = reqwest::Client::new()
        .post(format!("{ISSUER}/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .map_err(network)?;

    if !response.status().is_success() {
        return Err(ProviderError::Http {
            status: response.status().as_u16(),
            body: response.text().await.map_err(network)?,
        });
    }

    let grant: Value = response.json().await.map_err(network)?;

    Tokens::from_grant(&grant).ok_or_else(|| {
        ProviderError::SignIn(
            "the sign-in returned no ChatGPT account: is this a ChatGPT plan with Codex?"
                .to_string(),
        )
    })
}

fn open_browser(address: &str) {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Hands the address to whatever opens web addresses, without a
        // shell in between to misread the `&` in it.
        let mut command = std::process::Command::new("rundll32");

        command.arg("url.dll,FileProtocolHandler");

        command
    } else {
        std::process::Command::new("xdg-open")
    };

    // Failing to open it is fine: the address has been shown as well.
    let _ = command
        .arg(address)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn random_text() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 32];

    getrandom::fill(&mut bytes).map_err(|error| {
        ProviderError::SignIn(format!("this machine gave no random numbers: {error}"))
    })?;

    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn query(pairs: &[(&str, &str)]) -> String {
    let pairs: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect();

    pairs.join("&")
}

fn encode(text: &str) -> String {
    let mut encoded = String::new();

    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }

    encoded
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());

        match (escaped, bytes[index]) {
            (Some(byte), _) => {
                decoded.push(byte);
                index += 2;
            }

            (None, b'+') => decoded.push(b' '),
            (None, byte) => decoded.push(byte),
        }

        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_survives_encoding_and_decoding() {
        let text = "http://localhost:1455/auth/callback?a=b c&d=é";

        assert_eq!(encode("a b/c"), "a%20b%2Fc");
        assert_eq!(decode(&encode(text)), text);
        assert_eq!(decode("a+b%2"), "a b%2");
    }
}
