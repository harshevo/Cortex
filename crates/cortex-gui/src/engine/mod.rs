//! The link between the window and the agent.
//!
//! The agent runs on a background runtime; the window never waits for it.
//! The window sends [`Command`]s and receives [`Update`]s, and nothing else
//! crosses between the two.
//!
//! The agent is not fixed for the life of the app. Opening another
//! project, or changing the model or its settings, sends a [`Launch`] and
//! the agent is built again from it.

mod approver;
pub(crate) mod blueprint;
mod demo;
mod embeddings;
mod log;
mod sink;
mod stream;
mod team;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use cortex_core::{Event, Image};
use eframe::egui;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub use approver::{Decision, PendingApproval};

use crate::settings::Settings;

/// Everything that decides which agent is built.
pub struct Launch {
    /// The folder the agent works in.
    pub place: PathBuf,

    pub settings: Settings,

    /// The provider's key, when it is already known.
    pub api_key: Option<String>,

    /// With no key given, look one up: in the keychain, then in the
    /// provider's environment variable. This is done where the agent is
    /// built, not on the window's thread.
    pub look_up_key: bool,

    /// Use the scripted stand-in model in a throwaway project instead.
    pub demo: bool,

    /// Forget the conversation so far. False when only the model or its
    /// settings changed, and the conversation should carry on.
    pub fresh: bool,

    /// The log of an earlier session to take up again, once the agent is
    /// built.
    pub open: Option<PathBuf>,
}

/// What the window asks of the agent.
pub enum Command {
    /// Build the agent, replacing the one in use.
    Launch(Box<Launch>),

    /// Carry out an instruction in the current conversation.
    Submit(String, Vec<Image>),

    /// Carry on the run that stopped on a provider failure, from the
    /// step that failed.
    Continue,

    /// Something to tell the run in progress. The model sees it after
    /// the step it is on. With no run in progress it is an instruction.
    Steer(String),

    /// Take up the earlier session whose log this is: its conversation
    /// becomes the current one, and what follows is added to its log.
    Open(PathBuf),

    /// Abandon the run in progress.
    Stop,

    /// Forget the conversation and start a new session.
    Reset,

    /// Get the project ready ahead of any task: parse it, embed it, and
    /// have the model write a short overview to show in the chat.
    Prepare,

    /// Replace the older part of the conversation with a summary the
    /// model writes. Only between runs.
    Compact,
}

/// What the agent tells the window.
pub enum Update {
    ServiceStarting(String),
    /// The agent has been built and is waiting for instructions.
    Ready(Info),

    /// The agent could not be built, and why.
    Unavailable(String),

    /// A new session has begun and is being written to this log.
    Logging(PathBuf),

    /// Something happened in the run.
    Event(Event),

    /// A tool wants to change something and is waiting for an answer.
    ApprovalNeeded(PendingApproval),

    /// The model has begun a response. Sent again if the request is
    /// retried, in which case the text so far is void.
    WritingStarted,

    /// The next piece of the response being written.
    Writing(String),

    /// The next piece of the model's reasoning, from a provider that
    /// shows it.
    Thinking(String),

    /// The model has begun asking for the tool of this name.
    Calling(String),

    /// The request failed in a way that may not last and goes out again
    /// after `wait`. `reason` says what happened, in a few words.
    Retrying {
        reason: String,
        wait: Duration,
    },

    /// The run ended with an error. When `can_continue` is set, nothing
    /// it had done is lost and [`Command::Continue`] picks it up again.
    RunFailed {
        error: String,
        can_continue: bool,
    },

    /// The run was abandoned because the person asked.
    RunStopped,

    /// The agent is ready for the next instruction.
    Idle,

    /// Getting the project ready has ended. `text` says what was done,
    /// or what went wrong.
    Prepared {
        text: String,
        failed: bool,
    },

    /// A compaction ended: what came of it, and whether it failed.
    Compacted((String, bool)),
}

/// Facts about the agent in use.
#[derive(Debug, Clone)]
pub struct Info {
    /// The folder the agent works in.
    pub place: PathBuf,

    pub model: String,
    pub max_steps: u32,

    /// The agent is the scripted stand-in, not a real model.
    pub demo: bool,
}

/// Sends updates to the window and wakes it up to draw them.
#[derive(Clone)]
pub struct Outbox {
    updates: Sender<Update>,
    window: egui::Context,
}

impl Outbox {
    /// Send an update and schedule a repaint.
    pub fn send(&self, update: Update) {
        // The receiver is gone only when the window has closed.
        let _ = self.updates.send(update);

        self.window.request_repaint();
    }
}

/// The window's handle on the agent.
pub struct Engine {
    commands: UnboundedSender<Command>,
    updates: Receiver<Update>,
    runtime: Handle,
    window: egui::Context,
    member: Arc<team::Member>,
    embeddings: Arc<embeddings::EmbeddingService>,
}

impl Engine {
    /// Starts the background task. It has no agent until the first
    /// [`Engine::launch`].
    pub fn start(runtime: &Handle, window: egui::Context) -> Self {
        let member = Arc::new(team::Team::default()).join();
        Self::start_member(
            runtime,
            window,
            member,
            Arc::new(embeddings::EmbeddingService::default()),
        )
    }

    /// Creates an independent chat worker sharing this engine's collaboration team.
    pub fn fork(&self) -> Self {
        Self::start_member(
            &self.runtime,
            self.window.clone(),
            self.member.fork(),
            self.embeddings.clone(),
        )
    }

    pub fn agent_id(&self) -> &str {
        &self.member.id
    }

    fn start_member(
        runtime: &Handle,
        window: egui::Context,
        member: Arc<team::Member>,
        embeddings: Arc<embeddings::EmbeddingService>,
    ) -> Self {
        let (update_sender, updates) = mpsc::channel();
        let (commands, command_receiver) = unbounded_channel();

        let outbox = Outbox {
            updates: update_sender,
            window: window.clone(),
        };

        runtime.spawn(worker::run(
            command_receiver,
            outbox,
            member.clone(),
            embeddings.clone(),
        ));

        Self {
            commands,
            updates,
            runtime: runtime.clone(),
            window,
            member,
            embeddings,
        }
    }

    #[cfg(test)]
    fn start_without_embeddings(runtime: &Handle, window: egui::Context) -> Self {
        Self::start_member(
            runtime,
            window,
            Arc::new(team::Team::default()).join(),
            Arc::new(embeddings::EmbeddingService::disabled()),
        )
    }

    pub fn launch(&self, launch: Launch) {
        let _ = self.commands.send(Command::Launch(Box::new(launch)));
    }

    pub fn submit(&self, instruction: &str, images: Vec<Image>) {
        let _ = self
            .commands
            .send(Command::Submit(instruction.to_string(), images));
    }

    /// Carries on the run that stopped on a provider failure.
    pub fn carry_on(&self) {
        let _ = self.commands.send(Command::Continue);
    }

    /// Tells the run in progress something.
    pub fn steer(&self, message: &str) {
        let _ = self.commands.send(Command::Steer(message.to_string()));
    }

    /// Takes up an earlier session in the agent's folder.
    pub fn open(&self, log: PathBuf) {
        let _ = self.commands.send(Command::Open(log));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn reset(&self) {
        let _ = self.commands.send(Command::Reset);
    }

    /// Gets the agent's project ready ahead of any task.
    pub fn prepare(&self) {
        let _ = self.commands.send(Command::Prepare);
    }

    /// Replaces the older part of the conversation with a summary.
    pub fn compact(&self) {
        let _ = self.commands.send(Command::Compact);
    }

    /// Everything the agent has reported since the last call.
    pub fn updates(&self) -> impl Iterator<Item = Update> + '_ {
        self.updates.try_iter()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.member.close();
        let _ = self.commands.send(Command::Stop);
    }
}

#[cfg(test)]
mod tests;
