//! The agent that runs on this phone.
//!
//! The same agent the desktop runs: the loop, the tools that work on
//! files, the memory, the intuition, the decision controllers and the
//! growing network are all here, and only the model is elsewhere. It
//! presents itself to the rest of the core as one more host, "this
//! phone", that says what a desktop says and is asked what a desktop is
//! asked, so every screen works on it unchanged.

mod engine;
mod memory;
mod settings;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cortex_core::ModelProvider;
use cortex_living::Living;
use cortex_remote::Mirror;
use cortex_remote::wire::{ChatUp, Down, Up};
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub use settings::{PROVIDERS, Settings};

/// The model this phone's agent is set up with, for other screens to ask:
/// the trading screen's language-model forecast.
pub(crate) fn phone_provider(folder: &Path) -> anyhow::Result<Arc<dyn ModelProvider>> {
    engine::build_provider(&Settings::load(folder))
}

use crate::lock;

/// The name the list of hosts knows this phone by.
pub const HOST: &str = "this-phone";

pub const NAME: &str = "This phone";

/// The one chat on this phone.
pub const CHAT: &str = "phone";

/// What the engine is asked: what a phone asks a desktop, and what only
/// the device that holds the agent can be asked.
pub enum Ask {
    Chat(ChatUp),
    SetKey {
        provider: String,
        key: String,
    },
    SetAddress(String),

    /// Make a project of this name and work in it.
    NewProject(String),

    /// The sign-in changed: make the provider again.
    Reload,
}

pub struct Local {
    mirror: Arc<Mirror>,
    asks: UnboundedSender<Ask>,
    settings: Arc<Mutex<Settings>>,
}

impl Local {
    /// Starts the engine. Called on the core's background threads.
    pub fn start(folder: &Path) -> Arc<Self> {
        Self::start_with(folder, None)
    }

    /// `stand_in` is used as the model whatever the settings say.
    fn start_with(folder: &Path, stand_in: Option<Arc<dyn ModelProvider>>) -> Arc<Self> {
        let mirror = Arc::new(Mirror::default());
        let settings = Arc::new(Mutex::new(Settings::load(folder)));

        let (asks, asked) = unbounded_channel();

        tokio::spawn(engine::run(
            folder.to_path_buf(),
            settings.clone(),
            mirror.clone(),
            asked,
            stand_in,
        ));

        Arc::new(Self {
            mirror,
            asks,
            settings,
        })
    }

    pub fn ask(&self, ask: Ask) {
        // The engine lives as long as this does.
        let _ = self.asks.send(ask);
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// Everything the engine has said so far and everything it goes on
    /// to say, and where to send what it is asked.
    pub fn attach(&self) -> (Feed, UnboundedSender<Up>) {
        let (past, live) = self.mirror.attach();

        let (sender, mut asked) = unbounded_channel();
        let asks = self.asks.clone();

        tokio::spawn(async move {
            while let Some(up) = asked.recv().await {
                if let Up::Chat { command, .. } = up
                    && asks.send(Ask::Chat(command)).is_err()
                {
                    return;
                }
            }
        });

        let feed = Feed {
            past: past.into(),
            live,
        };

        (feed, sender)
    }
}

/// What the engine says, to one listener.
pub struct Feed {
    past: VecDeque<Down>,
    live: Receiver<Down>,
}

impl Feed {
    /// `None` once the listener has fallen too far behind to catch up.
    pub async fn next(&mut self) -> Option<Down> {
        if let Some(message) = self.past.pop_front() {
            return Some(message);
        }

        self.live.recv().await.ok()
    }

    /// The next thing already said, without waiting.
    pub fn waiting(&mut self) -> Option<Down> {
        if let Some(message) = self.past.pop_front() {
            return Some(message);
        }

        self.live.try_recv().ok()
    }
}

/// What a rest did to the growing network.
pub struct Rested {
    pub replays: u32,
    pub neurons: u64,
    pub synapses: u64,
}

/// Lets the growing network go over its recent moments for up to
/// `limit`, and saves it. Works with or without an engine running: the
/// system may wake the app in the background only for this.
pub fn rest(limit: Duration) -> Option<Rested> {
    let file = living_file()?;

    // A network that was never grown has nothing to go over.
    if !file.is_file() {
        return None;
    }

    let living = Living::shared(&file);

    let replays = living.sleep(limit);
    let meters = living.snapshot().meters;

    Some(Rested {
        replays,
        neurons: meters.neurons as u64,
        synapses: meters.synapses as u64,
    })
}

/// Where the engine keeps the one network it grows for all projects.
fn living_file() -> Option<PathBuf> {
    Some(
        std::env::home_dir()?
            .join(".cortex")
            .join("living")
            .join("network.json"),
    )
}

/// The file the ChatGPT sign-in is kept in.
pub fn sign_in_file() -> Option<PathBuf> {
    Some(
        std::env::home_dir()?
            .join(".cortex")
            .join("codex-auth.json"),
    )
}

#[cfg(test)]
mod tests {
    use cortex_core::Event;
    use cortex_providers::ScriptedProvider;
    use cortex_providers::scripted::{final_turn, tool_turn};
    use cortex_remote::wire::{ChatDown, ChatStatus, Decision, MemoryKind};
    use serde_json::json;

    use super::*;

    /// The next message about the chat, or a failure when the engine has
    /// gone quiet.
    async fn next(feed: &mut Feed) -> Down {
        tokio::time::timeout(Duration::from_secs(20), feed.next())
            .await
            .expect("the engine went quiet")
            .expect("the engine stopped")
    }

    fn submit(text: &str) -> Up {
        Up::Chat {
            chat: CHAT.to_string(),
            command: ChatUp::Submit {
                text: text.to_string(),
                images: Vec::new(),
            },
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_agent_works_remembers_and_rests_on_the_phone_alone() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path().canonicalize().unwrap();

        // What the engine learns across projects goes in the app's folder.
        unsafe { std::env::set_var("HOME", &folder) };

        let script = ScriptedProvider::new(vec![
            tool_turn(vec![(
                "write_file",
                json!({ "path": "hello.txt", "content": "made on the phone\n" }),
            )]),
            final_turn("Wrote hello.txt."),
            tool_turn(vec![("read_file", json!({ "path": "hello.txt" }))]),
            final_turn("It says: made on the phone."),
        ]);

        let local = Local::start_with(&folder, Some(Arc::new(script)));
        let (mut feed, asks) = local.attach();

        // The agent is built without a desktop and without the services.
        loop {
            if let Down::State(state) = next(&mut feed).await
                && let Some(chat) = state.chats.first()
                && chat.status != ChatStatus::Starting
            {
                assert_eq!(chat.status, ChatStatus::Ready);

                let on = |name: &str| {
                    chat.settings
                        .capabilities
                        .iter()
                        .any(|capability| capability.name == name && capability.on)
                };

                assert!(on("memory") && on("intuition") && on("living"));

                break;
            }
        }

        asks.send(submit("write a greeting to hello.txt")).unwrap();

        let mut asked = false;
        let mut finished = false;

        // A change waits for the person, and the run ends once allowed.
        loop {
            match next(&mut feed).await {
                Down::Chat {
                    message: ChatDown::Approval { id, tool, .. },
                    ..
                } => {
                    assert_eq!(tool, "write_file");

                    asked = true;

                    asks.send(Up::Chat {
                        chat: CHAT.to_string(),
                        command: ChatUp::Approve {
                            id,
                            decision: Decision::AlwaysAllow,
                        },
                    })
                    .unwrap();
                }

                Down::Chat {
                    message: ChatDown::Event(Event::RunFinished { .. }),
                    ..
                } => finished = true,

                Down::Chat {
                    message: ChatDown::Idle,
                    ..
                } => break,

                Down::Chat {
                    message: ChatDown::RunFailed { error, .. },
                    ..
                } => panic!("the run failed: {error}"),

                _ => {}
            }
        }

        assert!(asked && finished);

        let project = engine::projects(&folder).join("Scratch");

        assert_eq!(
            std::fs::read_to_string(project.join("hello.txt")).unwrap(),
            "made on the phone\n"
        );

        // The run became an episode in the phone's own memory, and its
        // log is listed as a session.
        let mut remembered = false;
        let mut listed = false;

        while !(remembered && listed) {
            match next(&mut feed).await {
                Down::Chat {
                    message: ChatDown::Memory(graph),
                    ..
                } => {
                    remembered = graph
                        .nodes
                        .iter()
                        .any(|node| node.kind == MemoryKind::Episode);
                }

                Down::State(state) => {
                    listed = state
                        .projects
                        .iter()
                        .any(|project| project.scratch && project.sessions.len() == 1);
                }

                _ => {}
            }
        }

        // A second run in the same session, to give the growing network
        // something that happened twice.
        asks.send(submit("read hello.txt back")).unwrap();

        loop {
            if let Down::Chat {
                message: ChatDown::Idle,
                ..
            } = next(&mut feed).await
            {
                break;
            }
        }

        assert!(project.join(".cortex/memory.intuition.json").is_file());

        // With the app in the background, the network goes over what it
        // lived through and is saved.
        let before = Living::shared(&living_file().unwrap()).snapshot().meters;

        assert!(before.moments > 0);

        let rested = rest(Duration::from_secs(5)).unwrap();

        assert!(rested.replays > 0);
        assert!(rested.neurons > 0);
        assert!(living_file().unwrap().is_file());
    }
}
