//! Remote control: letting a paired phone follow and drive the chats.
//!
//! The agent stays here. Everything the window is told about a run is
//! also kept in a [`Mirror`], and sent on to any phone that is connected.
//! What a phone asks for goes straight to the agent when it is about the
//! run (an instruction, stop, an answer to a question), so it works even
//! while the window is not being drawn, and waits for the window when it
//! changes what the window holds (which session, which settings).

mod memory;
pub mod view;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cortex_remote::wire::{ChatDown, ChatUp, Decision, Down, HostState};
use cortex_remote::{Device, Host, Mirror};
use eframe::egui;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::engine::{Command, Update};

/// How long a pairing code is shown for. A little less than it works
/// for, so one that is still on screen always works.
const CODE_SHOWN: Duration = Duration::from_secs(9 * 60);

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// Whether phones can connect.
#[derive(Clone)]
pub enum Hosting {
    Off,
    Starting,
    On(Arc<Host>),

    /// It could not be started, and why.
    Failed(String),
}

/// A tool's question, which the window or a phone may answer, whichever
/// comes first.
#[derive(Clone, Default)]
pub struct Reply(Arc<Mutex<Option<oneshot::Sender<Decision>>>>);

impl Reply {
    pub fn new(sender: oneshot::Sender<Decision>) -> Self {
        Self(Arc::new(Mutex::new(Some(sender))))
    }

    pub fn answer(&self, decision: Decision) {
        // The tool is gone only if its run was dropped.
        if let Some(sender) = lock(&self.0).take() {
            let _ = sender.send(decision);
        }
    }

    /// Answered, or nobody is waiting for the answer any more.
    pub fn answered(&self) -> bool {
        lock(&self.0)
            .as_ref()
            .is_none_or(|sender| sender.is_closed())
    }
}

pub struct Bridge {
    runtime: Handle,
    window: egui::Context,
    mirror: Arc<Mirror>,
    hosting: Mutex<Hosting>,

    /// Where the host puts what phones ask for.
    asked: UnboundedSender<(String, ChatUp)>,

    /// Each chat's agent, by the chat's name.
    engines: Mutex<HashMap<String, UnboundedSender<Command>>>,

    /// The questions waiting for an answer, and the chat each is from.
    questions: Mutex<HashMap<u64, (String, Reply)>>,
    next_question: AtomicU64,

    /// What phones asked for that only the window can do.
    for_window: Mutex<Vec<(String, ChatUp)>>,

    /// The pairing code on show, and when it stops being shown.
    code: Mutex<Option<(String, Instant)>>,
}

/// Sets remote control up. Phones cannot connect until it is switched on.
pub fn install(runtime: &Handle, window: egui::Context) {
    let (asked, asks) = unbounded_channel();

    let installed = BRIDGE.set(Bridge {
        runtime: runtime.clone(),
        window,
        mirror: Arc::new(Mirror::default()),
        hosting: Mutex::new(Hosting::Off),
        asked,
        engines: Mutex::default(),
        questions: Mutex::default(),
        next_question: AtomicU64::new(1),
        for_window: Mutex::default(),
        code: Mutex::new(None),
    });

    if installed.is_ok() {
        runtime.spawn(obey(asks));
    }
}

/// `None` in tests, which have no window to set it up for.
pub fn bridge() -> Option<&'static Bridge> {
    BRIDGE.get()
}

/// Tells phones something about a chat.
pub fn tell(chat: &str, message: ChatDown) {
    if let Some(bridge) = bridge() {
        bridge.mirror.publish(Down::Chat {
            chat: chat.to_string(),
            message,
        });
    }
}

/// Passes on what the agent told the window, where a phone shows it too.
pub fn mirror(chat: &str, update: &Update) {
    let message = match update {
        Update::Event(event) => ChatDown::Event(event.clone()),
        Update::WritingStarted => ChatDown::WritingStarted,
        Update::Writing(piece) => ChatDown::Writing(piece.clone()),
        Update::Thinking(piece) => ChatDown::Thinking(piece.clone()),
        Update::Calling(tool) => ChatDown::Calling(tool.clone()),

        Update::Retrying { reason, wait } => ChatDown::Retrying {
            reason: reason.clone(),
            wait_ms: wait.as_millis() as u64,
        },

        Update::RunFailed {
            error,
            can_continue,
        } => ChatDown::RunFailed {
            error: error.clone(),
            can_continue: *can_continue,
        },

        Update::RunStopped => ChatDown::RunStopped,
        Update::Idle => ChatDown::Idle,

        Update::ServiceStarting(text) => ChatDown::Notice {
            text: text.clone(),
            bad: false,
        },

        Update::Compacted((text, failed)) => ChatDown::Notice {
            text: text.clone(),
            bad: *failed,
        },

        Update::Prepared { text, failed } => ChatDown::Notice {
            text: text.clone(),
            bad: *failed,
        },

        Update::Suggestion(suggestion) => ChatDown::Suggestion(suggestion.clone()),
        Update::Memory(remembered) => ChatDown::Memory(memory::graph(remembered)),

        Update::ApprovalNeeded(approval) => ChatDown::Approval {
            id: approval.id,
            tool: approval.request.tool.clone(),
            summary: approval.request.summary.clone(),
        },

        Update::Ready(_)
        | Update::Unavailable(_)
        | Update::Logging(_)
        | Update::Brain(_)
        | Update::MemoryFailed(_) => return,
    };

    tell(chat, message);
}

impl Bridge {
    /// Makes a chat's agent reachable from a phone.
    pub fn join(&self, chat: &str, engine: UnboundedSender<Command>) {
        lock(&self.engines).insert(chat.to_string(), engine);
    }

    /// The chat is gone: its pane was closed.
    pub fn leave(&self, chat: &str) {
        lock(&self.engines).remove(chat);
        lock(&self.questions).retain(|_, (from, _)| from != chat);

        self.mirror.publish(Down::ChatClosed {
            chat: chat.to_string(),
        });
    }

    /// Registers a question so that a phone can answer it. Returns the
    /// number phones know it by.
    pub fn ask(&self, chat: &str, reply: &Reply) -> u64 {
        let id = self.next_question.fetch_add(1, Ordering::Relaxed);

        lock(&self.questions).insert(id, (chat.to_string(), reply.clone()));

        id
    }

    /// The question has been answered, or its run is over.
    pub fn settle(&self, id: u64) {
        if let Some((chat, _)) = lock(&self.questions).remove(&id) {
            tell(&chat, ChatDown::ApprovalClosed { id });
        }

        // The window drops the question from its own list.
        self.window.request_repaint();
    }

    /// What phones asked of this chat that only the window can do.
    pub fn take_asks(&self, chat: &str) -> Vec<ChatUp> {
        let mut waiting = lock(&self.for_window);

        if waiting.is_empty() {
            return Vec::new();
        }

        let (mine, others) = std::mem::take(&mut *waiting)
            .into_iter()
            .partition(|(to, _)| to == chat);

        *waiting = others;

        mine.into_iter().map(|(_, ask)| ask).collect()
    }

    /// Tells phones the chats, projects and settings, if any of it has
    /// changed since they were last told.
    pub fn state(&self, state: HostState) {
        if self.mirror.state_differs(&state) {
            self.mirror.publish(Down::State(state));
        }
    }

    pub fn hosting(&self) -> Hosting {
        lock(&self.hosting).clone()
    }

    /// Lets phones connect, or cuts them off and stops listening.
    pub fn switch(&'static self, on: bool) {
        let mut hosting = lock(&self.hosting);

        match (&*hosting, on) {
            (Hosting::Off | Hosting::Failed(_), true) => {
                *hosting = Hosting::Starting;

                self.runtime.spawn(self.start());
            }

            (Hosting::On(host), false) => {
                let host = host.clone();

                self.runtime.spawn(async move { host.close().await });

                *hosting = Hosting::Off;
                *lock(&self.code) = None;
            }

            (Hosting::Starting | Hosting::Failed(_), false) => *hosting = Hosting::Off,

            _ => {}
        }
    }

    async fn start(&'static self) {
        let started = match key_file() {
            Some(file) => {
                let window = self.window.clone();

                Host::start(
                    &file,
                    &computer_name(),
                    self.mirror.clone(),
                    self.asked.clone(),
                    move || window.request_repaint(),
                )
                .await
                .map_err(|error| format!("{error:#}"))
            }

            None => Err("There is no home folder to keep the pairing in.".to_string()),
        };

        let mut hosting = lock(&self.hosting);

        // Switched off again while it was starting.
        if !matches!(*hosting, Hosting::Starting) {
            if let Ok(host) = started {
                self.runtime.spawn(async move { host.close().await });
            }

            return;
        }

        *hosting = match started {
            Ok(host) => Hosting::On(host),
            Err(problem) => Hosting::Failed(problem),
        };

        drop(hosting);

        self.window.request_repaint();
    }

    /// The pairing code on show, if there is one and it still works.
    pub fn code(&self) -> Option<(String, Duration)> {
        let code = lock(&self.code);

        let (text, until) = code.as_ref()?;

        let left = until.checked_duration_since(Instant::now())?;

        Some((text.clone(), left))
    }

    /// Makes a new pairing code and shows it. The one before stops
    /// working.
    pub fn show_code(&'static self) {
        let Hosting::On(host) = self.hosting() else {
            return;
        };

        self.runtime.spawn(async move {
            let code = host.offer().await;

            *lock(&self.code) = Some((code, Instant::now() + CODE_SHOWN));

            self.window.request_repaint();
        });
    }

    /// Writes a pairing code to `file` once phones can connect. For
    /// pairing without a camera, in tests.
    pub fn write_code(&'static self, file: PathBuf) {
        self.runtime.spawn(async move {
            loop {
                match self.hosting() {
                    Hosting::On(host) => {
                        let code = host.offer().await;

                        if let Err(error) = std::fs::write(&file, code) {
                            eprintln!("warning: cannot write {}: {error}", file.display());
                        }

                        return;
                    }

                    Hosting::Starting => tokio::time::sleep(Duration::from_millis(100)).await,
                    Hosting::Off | Hosting::Failed(_) => return,
                }
            }
        });
    }

    pub fn hide_code(&self) {
        if let Hosting::On(host) = self.hosting() {
            host.withdraw_offer();
        }

        *lock(&self.code) = None;
    }

    /// The phones that are let in, and whether each is connected now.
    pub fn devices(&self) -> Vec<(Device, bool)> {
        let Hosting::On(host) = self.hosting() else {
            return Vec::new();
        };

        host.devices()
            .into_iter()
            .map(|device| {
                let connected = host.is_connected(&device.id);

                (device, connected)
            })
            .collect()
    }

    pub fn forget(&self, id: &str) {
        if let Hosting::On(host) = self.hosting()
            && let Err(error) = host.forget(id)
        {
            eprintln!("warning: the phone was not forgotten: {error:#}");
        }
    }
}

/// Carries out what phones ask for.
async fn obey(mut asks: UnboundedReceiver<(String, ChatUp)>) {
    let Some(bridge) = bridge() else {
        return;
    };

    while let Some((chat, ask)) = asks.recv().await {
        let engine = lock(&bridge.engines).get(&chat).cloned();

        let Some(engine) = engine else {
            continue;
        };

        let command = match ask {
            ChatUp::Submit { text, images } => Command::Submit(text, images),
            ChatUp::Steer { text } => Command::Steer(text),
            ChatUp::Stop => Command::Stop,
            ChatUp::Continue => Command::Continue,

            ChatUp::Approve { id, decision } => {
                let question = lock(&bridge.questions).get(&id).cloned();

                // Only the chat the question came from may answer it.
                if let Some((from, reply)) = question
                    && from == chat
                {
                    reply.answer(decision);
                }

                continue;
            }

            ask => {
                lock(&bridge.for_window).push((chat, ask));

                bridge.window.request_repaint();

                continue;
            }
        };

        let _ = engine.send(command);

        // The window shows what the phone started.
        bridge.window.request_repaint();
    }
}

/// Where this computer's key and the phones it lets in are kept.
fn key_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".cortex").join("remote.json"))
}

/// What a phone lists this computer as.
fn computer_name() -> String {
    let asked = if cfg!(target_os = "macos") {
        std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
    } else {
        std::process::Command::new("hostname").output()
    };

    asked
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Cortex".to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
