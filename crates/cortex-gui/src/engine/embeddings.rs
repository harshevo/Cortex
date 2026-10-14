use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use cortex_core::services::Service;
use cortex_retrieval::client::Health;
use reqwest::Url;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};

use super::{Outbox, Update};

#[derive(Default)]
pub(super) struct EmbeddingService {
    process: Mutex<Option<Child>>,
    #[cfg(test)]
    disabled: bool,
}

impl EmbeddingService {
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            disabled: true,
            ..Self::default()
        }
    }

    pub async fn ensure(&self, url: &str, outbox: &Outbox) -> Result<()> {
        #[cfg(test)]
        if self.disabled {
            return Ok(());
        }
        self.ensure_with(url, outbox, service_command, Duration::from_secs(600))
            .await
    }

    async fn ensure_with(
        &self,
        url: &str,
        outbox: &Outbox,
        launcher: impl FnOnce(u16) -> Result<Command>,
        timeout: Duration,
    ) -> Result<()> {
        let mut process = self.process.lock().await;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        if healthy(&http, url).await {
            return Ok(());
        }
        let port = local_port(url)?;
        if let Some(child) = process.as_mut()
            && child.try_wait()?.is_some()
        {
            *process = None;
        }
        let log_path = log_path();
        if process.is_none() {
            let mut command = launcher(port)?;
            std::fs::create_dir_all(
                log_path
                    .parent()
                    .context("missing embedding log directory")?,
            )?;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            command
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .kill_on_drop(true);
            *process = Some(command.spawn().with_context(|| {
                format!(
                    "cannot start the embedding service; see {}",
                    log_path.display()
                )
            })?);
            outbox.send(Update::ServiceStarting(
                "Starting the embedding service…".into(),
            ));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if healthy(&http, url).await {
                return Ok(());
            }
            if let Some(child) = process.as_mut()
                && let Some(status) = child.try_wait()?
            {
                *process = None;
                bail!(
                    "The embedding service exited ({status}). See {} for the startup error.",
                    log_path.display()
                );
            }
            if Instant::now() >= deadline {
                if let Some(mut child) = process.take() {
                    let _ = child.kill().await;
                }
                bail!(
                    "The embedding service did not become ready at {url}. See {} for download or model-loading errors.",
                    log_path.display()
                );
            }
            sleep(Duration::from_millis(250)).await;
        }
    }
}

async fn healthy(http: &reqwest::Client, url: &str) -> bool {
    let Ok(response) = http
        .get(format!("{}/health", url.trim_end_matches('/')))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<Health>()
        .await
        .is_ok_and(|health| health.dimension > 0 && !health.model.is_empty())
}

fn local_port(address: &str) -> Result<u16> {
    let url = Url::parse(address).context("invalid embedding service address")?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!(
            "The embedding service at {address} is unavailable. Automatic startup supports a local http://127.0.0.1:<port> address; remote services must already be running."
        );
    }
    Ok(url.port_or_known_default().unwrap_or(8765))
}

fn log_path() -> PathBuf {
    Service::Embedding
        .log()
        .unwrap_or_else(|| std::env::temp_dir().join("cortex-embedding-service.log"))
}

fn service_command(port: u16) -> Result<Command> {
    Service::Embedding
        .command(port)
        .map(Command::from)
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    fn outbox() -> Outbox {
        let (updates, _) = mpsc::channel();
        Outbox {
            updates,
            window: eframe::egui::Context::default(),
            chat: "agent-1".into(),
        }
    }

    fn address() -> String {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", socket.local_addr().unwrap())
    }

    /// Any Python will do for a stand-in service.
    fn python() -> Command {
        Command::new(if cfg!(windows) { "python" } else { "python3" })
    }

    fn fake_command(port: u16) -> Result<Command> {
        let mut command = python();
        command.arg("-c").arg(format!(r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
class Handler(BaseHTTPRequestHandler):
 def do_GET(self):
  body=json.dumps({{'model':'test','dimension':3,'device':'cpu'}}).encode()
  self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
HTTPServer(('127.0.0.1',{port}),Handler).serve_forever()
"#));
        Ok(command)
    }

    #[test]
    fn only_local_root_addresses_can_be_started() {
        assert_eq!(local_port("http://127.0.0.1:8765/").unwrap(), 8765);
        assert!(local_port("https://example.com:8765").is_err());
        assert!(local_port("http://127.0.0.1:8765/proxy").is_err());
    }

    #[tokio::test]
    async fn simultaneous_chats_share_one_process_and_restart_after_exit() {
        let service = Arc::new(EmbeddingService::default());
        let url = address();
        let starts = Arc::new(AtomicUsize::new(0));
        let launch = |port| {
            starts.fetch_add(1, Ordering::SeqCst);
            fake_command(port)
        };
        let outbox = outbox();
        let (first, second) = tokio::join!(
            service.ensure_with(&url, &outbox, launch, Duration::from_secs(5)),
            service.ensure_with(&url, &outbox, launch, Duration::from_secs(5))
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        service
            .process
            .lock()
            .await
            .as_mut()
            .unwrap()
            .kill()
            .await
            .unwrap();
        service
            .ensure_with(&url, &outbox, launch, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        let external = EmbeddingService::default();
        external
            .ensure_with(
                &url,
                &outbox,
                |_| panic!("healthy service must be reused"),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert!(external.process.lock().await.is_none());
    }

    #[tokio::test]
    async fn exited_service_reports_the_log_instead_of_waiting_forever() {
        let service = EmbeddingService::default();
        let error = service
            .ensure_with(
                &address(),
                &outbox(),
                |_| {
                    let mut command = python();
                    command.args(["-c", "raise SystemExit(7)"]);
                    Ok(command)
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("embedding-service.log"));
        assert!(service.process.lock().await.is_none());
    }
}
