//! The connection between a phone and the desktop app.
//!
//! Each side is dialed by its public key. The connection is encrypted end
//! to end, goes directly when the two can reach each other and through a
//! relay when they cannot, and neither side opens a port.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::timeout;

use crate::mirror::Mirror;
use crate::pairing::{self, Device, Keyring, Ticket};
use crate::wire::{self, ALPN, ChatUp, Down, Up};

/// How long a pairing code works for.
const OFFER_LASTS: Duration = Duration::from_secs(10 * 60);

/// How long the other side has to say who it is.
const GREETING: Duration = Duration::from_secs(15);

/// How long to wait to learn the desktop's own address before showing a
/// code without it. Such a code still works when both are on one network.
const GOING_ONLINE: Duration = Duration::from_secs(5);

/// How long a phone tries to reach the desktop.
const DIALING: Duration = Duration::from_secs(20);

/// The desktop's end.
pub struct Host {
    endpoint: Endpoint,
    name: String,
    mirror: Arc<Mirror>,
    keyring: Mutex<Keyring>,

    /// The pairing code on show, and until when it works.
    offer: Mutex<Option<(String, Instant)>>,

    /// The phones connected now, by key.
    connected: Mutex<HashMap<String, Connection>>,

    /// Where what the phones ask for goes.
    commands: UnboundedSender<(String, ChatUp)>,

    /// Called when a phone comes or goes, so the window can draw it.
    changed: Box<dyn Fn() + Send + Sync>,
}

impl Host {
    /// Starts listening. `file` keeps the desktop's key and the phones
    /// it lets in.
    pub async fn start(
        file: &Path,
        name: &str,
        mirror: Arc<Mirror>,
        commands: UnboundedSender<(String, ChatUp)>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        let keyring = Keyring::open(file)?;

        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(keyring.secret()?)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|error| anyhow!("cannot start remote control: {error}"))?;

        let host = Arc::new(Self {
            endpoint,
            name: name.to_string(),
            mirror,
            keyring: Mutex::new(keyring),
            offer: Mutex::new(None),
            connected: Mutex::new(HashMap::new()),
            commands,
            changed: Box::new(changed),
        });

        tokio::spawn(host.clone().listen());

        Ok(host)
    }

    /// A new pairing code. The one before it stops working.
    pub async fn offer(&self) -> String {
        let _ = timeout(GOING_ONLINE, self.endpoint.online()).await;

        let token = pairing::new_token();

        *lock(&self.offer) = Some((token.clone(), Instant::now() + OFFER_LASTS));

        Ticket {
            addr: self.endpoint.addr(),
            token,
            name: self.name.clone(),
        }
        .encode()
    }

    /// Stops the pairing code on show from working.
    pub fn withdraw_offer(&self) {
        *lock(&self.offer) = None;
    }

    /// The phones that are let in.
    pub fn devices(&self) -> Vec<Device> {
        lock(&self.keyring).devices().to_vec()
    }

    pub fn is_connected(&self, id: &str) -> bool {
        lock(&self.connected).contains_key(id)
    }

    pub fn connected(&self) -> usize {
        lock(&self.connected).len()
    }

    /// Stops letting the phone in, and cuts it off if it is connected.
    pub fn forget(&self, id: &str) -> Result<()> {
        lock(&self.keyring).forget(id)?;

        if let Some(connection) = lock(&self.connected).remove(id) {
            connection.close(0u32.into(), b"forgotten");
        }

        Ok(())
    }

    /// Cuts every phone off and stops listening.
    pub async fn close(&self) {
        self.endpoint.close().await;

        lock(&self.connected).clear();
    }

    async fn listen(self: Arc<Self>) {
        while let Some(incoming) = self.endpoint.accept().await {
            let host = self.clone();

            tokio::spawn(async move {
                let Ok(connection) = incoming.await else {
                    return;
                };

                let id = connection.remote_id().to_string();

                // Why it ended is of no interest: the phone went away.
                let _ = host.serve(&connection, &id).await;

                let mut connected = lock(&host.connected);

                // A phone that has since connected again keeps its place.
                if connected
                    .get(&id)
                    .is_some_and(|kept| kept.stable_id() == connection.stable_id())
                {
                    connected.remove(&id);
                }

                drop(connected);

                (host.changed)();
            });
        }
    }

    async fn serve(&self, connection: &Connection, id: &str) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;

        let hello = timeout(GREETING, wire::read::<Up>(&mut recv)).await??;

        let Up::Hello { device, token } = hello else {
            bail!("the phone did not say who it is");
        };

        if let Err(reason) = self.admit(id, &device, token.as_deref()) {
            let refused = Down::Refused {
                reason: reason.to_string(),
            };

            wire::write(&mut send, &refused).await?;

            // Gives the refusal time to arrive before the connection goes.
            let _ = send.finish();
            let _ = timeout(GREETING, connection.closed()).await;

            return Ok(());
        }

        if let Some(earlier) = lock(&self.connected).insert(id.to_string(), connection.clone()) {
            earlier.close(0u32.into(), b"connected again");
        }

        (self.changed)();

        let welcome = Down::Welcome {
            host: self.name.clone(),
        };

        wire::write(&mut send, &welcome).await?;

        let (past, mut live) = self.mirror.attach();

        for message in &past {
            wire::write(&mut send, message).await?;
        }

        // Reading stops part-way through a message if it is interrupted,
        // so it has a task of its own.
        let (asked, mut asks) = unbounded_channel();

        let reading = tokio::spawn(async move {
            while let Ok(message) = wire::read::<Up>(&mut recv).await {
                if asked.send(message).is_err() {
                    break;
                }
            }
        });

        let ended = loop {
            tokio::select! {
                message = live.recv() => match message {
                    Ok(message) => {
                        if let Err(error) = wire::write(&mut send, &message).await {
                            break Err(error);
                        }
                    }

                    // The phone fell too far behind to catch up from
                    // here. Cut off, it connects again and starts afresh.
                    Err(RecvError::Lagged(_)) => break Ok(()),
                    Err(RecvError::Closed) => break Ok(()),
                },

                ask = asks.recv() => match ask {
                    Some(Up::Chat { chat, command }) => {
                        let _ = self.commands.send((chat, command));
                    }

                    Some(Up::Ping) => {
                        if let Err(error) = wire::write(&mut send, &Down::Pong).await {
                            break Err(error);
                        }
                    }

                    Some(Up::Hello { .. }) => {}

                    None => break Ok(()),
                },
            }
        };

        reading.abort();
        connection.close(0u32.into(), b"done");

        ended
    }

    /// Whether the phone is let in: one that is known, or one that has
    /// the pairing code on show, which is then used up.
    fn admit(&self, id: &str, name: &str, token: Option<&str>) -> Result<()> {
        let mut keyring = lock(&self.keyring);

        if keyring.knows(id) {
            return Ok(());
        }

        let mut offer = lock(&self.offer);

        let matches = match (&*offer, token) {
            (Some((offered, until)), Some(token)) => {
                Instant::now() < *until && offered.as_str() == token
            }

            _ => false,
        };

        if !matches {
            bail!(
                "This phone is not paired with the desktop, or its pairing code is no longer \
                 valid. Show a new code on the desktop and scan it."
            );
        }

        *offer = None;

        keyring.remember(id, name, None)
    }
}

/// A phone's end.
pub struct Client {
    endpoint: Endpoint,
    keyring: Mutex<Keyring>,
}

impl Client {
    /// `file` keeps the phone's key and the desktops it knows.
    pub async fn open(file: &Path) -> Result<Self> {
        let keyring = Keyring::open(file)?;

        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(keyring.secret()?)
            .bind()
            .await
            .map_err(|error| anyhow!("cannot start the connection: {error}"))?;

        Ok(Self {
            endpoint,
            keyring: Mutex::new(keyring),
        })
    }

    /// The desktops this phone has been paired with.
    pub fn hosts(&self) -> Vec<Device> {
        lock(&self.keyring).devices().to_vec()
    }

    pub fn forget(&self, id: &str) -> Result<()> {
        lock(&self.keyring).forget(id)
    }

    /// Connects with a pairing code read from the desktop's screen, and
    /// remembers the desktop.
    pub async fn pair(&self, code: &str, device: &str) -> Result<Session> {
        let ticket = Ticket::decode(code)?;
        let id = ticket.addr.id.to_string();

        let session = self
            .dial(ticket.addr.clone(), device, Some(ticket.token))
            .await?;

        lock(&self.keyring).remember(&id, &session.host, Some(ticket.addr))?;

        Ok(session)
    }

    /// Connects to a desktop this phone has been paired with.
    pub async fn connect(&self, id: &str, device: &str) -> Result<Session> {
        let known = lock(&self.keyring)
            .find(id)
            .cloned()
            .context("This desktop is not paired with the phone.")?;

        let addr = match known.addr {
            Some(addr) => addr,
            None => EndpointAddr::new(id.parse().context("the desktop's key is damaged")?),
        };

        self.dial(addr, device, None).await
    }

    /// Tells the connection the phone's network has changed, so it finds
    /// its way again at once instead of when it next notices.
    pub async fn network_changed(&self) {
        self.endpoint.network_change().await;
    }

    async fn dial(
        &self,
        addr: EndpointAddr,
        device: &str,
        token: Option<String>,
    ) -> Result<Session> {
        let connection = timeout(DIALING, self.endpoint.connect(addr, ALPN))
            .await
            .map_err(|_| {
                anyhow!("The desktop did not answer. Is Cortex open there, with remote control on?")
            })?
            .map_err(|error| anyhow!("The desktop could not be reached: {error}"))?;

        let (mut send, mut recv) = connection.open_bi().await?;

        let hello = Up::Hello {
            device: device.to_string(),
            token,
        };

        wire::write(&mut send, &hello).await?;

        let host = match timeout(GREETING, wire::read::<Down>(&mut recv)).await?? {
            Down::Welcome { host } => host,
            Down::Refused { reason } => bail!("{reason}"),
            _ => bail!("The desktop answered with something unexpected."),
        };

        let (told, tells) = unbounded_channel();
        let (asks, mut asked) = unbounded_channel::<Up>();

        let reading = tokio::spawn(async move {
            while let Ok(message) = wire::read::<Down>(&mut recv).await {
                if told.send(message).is_err() {
                    break;
                }
            }
        });

        let writing = tokio::spawn(async move {
            while let Some(message) = asked.recv().await {
                if wire::write(&mut send, &message).await.is_err() {
                    break;
                }
            }
        });

        Ok(Session {
            host,
            connection,
            tells,
            asks,
            tasks: [reading.abort_handle(), writing.abort_handle()],
        })
    }
}

/// A phone's connection to a desktop, while it lasts.
pub struct Session {
    /// The desktop's name.
    pub host: String,

    connection: Connection,
    tells: UnboundedReceiver<Down>,
    asks: UnboundedSender<Up>,
    tasks: [tokio::task::AbortHandle; 2],
}

impl Session {
    /// The next thing the desktop says. `None` once the connection is
    /// gone.
    pub async fn next(&mut self) -> Option<Down> {
        self.tells.recv().await
    }

    /// The next thing the desktop has already said, without waiting.
    pub fn waiting(&mut self) -> Option<Down> {
        self.tells.try_recv().ok()
    }

    /// Something to send without holding the session.
    pub fn sender(&self) -> UnboundedSender<Up> {
        self.asks.clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }

        self.connection.close(0u32.into(), b"bye");
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ChatDown;

    struct Desk {
        host: Arc<Host>,
        mirror: Arc<Mirror>,
        asked: UnboundedReceiver<(String, ChatUp)>,
        _folder: tempfile::TempDir,
    }

    async fn desk() -> Desk {
        let folder = tempfile::tempdir().unwrap();
        let mirror = Arc::new(Mirror::default());
        let (commands, asked) = unbounded_channel();

        let host = Host::start(
            &folder.path().join("remote.json"),
            "Studio",
            mirror.clone(),
            commands,
            || {},
        )
        .await
        .unwrap();

        Desk {
            host,
            mirror,
            asked,
            _folder: folder,
        }
    }

    async fn phone(folder: &tempfile::TempDir) -> Client {
        Client::open(&folder.path().join("phone.json"))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_phone_pairs_catches_up_follows_along_and_is_obeyed() {
        let mut desk = desk().await;

        desk.mirror.publish(Down::Chat {
            chat: "agent-1".into(),
            message: ChatDown::Notice {
                text: "before".into(),
                bad: false,
            },
        });

        let folder = tempfile::tempdir().unwrap();
        let client = phone(&folder).await;

        let mut session = client
            .pair(&desk.host.offer().await, "Phone")
            .await
            .unwrap();

        assert_eq!(session.host, "Studio");
        assert!(matches!(session.next().await, Some(Down::State(_))));

        // What happened before the phone connected.
        let Some(Down::Chat { message, .. }) = session.next().await else {
            panic!("the phone was not caught up");
        };

        assert!(matches!(message, ChatDown::Notice { text, .. } if text == "before"));

        // What happens while it is connected.
        desk.mirror.publish(Down::Chat {
            chat: "agent-1".into(),
            message: ChatDown::Idle,
        });

        assert!(matches!(
            session.next().await,
            Some(Down::Chat {
                message: ChatDown::Idle,
                ..
            })
        ));

        session
            .sender()
            .send(Up::Chat {
                chat: "agent-1".into(),
                command: ChatUp::Stop,
            })
            .unwrap();

        assert_eq!(
            desk.asked.recv().await,
            Some(("agent-1".to_string(), ChatUp::Stop))
        );

        assert_eq!(desk.host.connected(), 1);
        assert_eq!(desk.host.devices()[0].name, "Phone");
    }

    #[tokio::test]
    async fn a_paired_phone_needs_no_code_and_a_forgotten_one_is_refused() {
        let desk = desk().await;

        let folder = tempfile::tempdir().unwrap();
        let client = phone(&folder).await;

        let session = client
            .pair(&desk.host.offer().await, "Phone")
            .await
            .unwrap();
        let id = client.hosts()[0].id.clone();

        drop(session);

        assert!(client.connect(&id, "Phone").await.is_ok());

        let phone_id = desk.host.devices()[0].id.clone();

        desk.host.forget(&phone_id).unwrap();

        let refused = client.connect(&id, "Phone").await.err().unwrap();

        assert!(refused.to_string().contains("not paired"));
    }

    #[tokio::test]
    async fn a_pairing_code_works_once() {
        let desk = desk().await;
        let code = desk.host.offer().await;

        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        assert!(phone(&first).await.pair(&code, "One").await.is_ok());
        assert!(phone(&second).await.pair(&code, "Two").await.is_err());
    }
}
