//! The messages a phone and the desktop app exchange.
//!
//! Each is one JSON object in a frame: four bytes of length, then the
//! text. [`Down`] goes to the phone, [`Up`] comes from it.

use anyhow::{Context, Result, bail};
use cortex_core::{Event, Image};
use iroh::endpoint::{RecvStream, SendStream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Names the protocol in the connection's handshake. Both sides must
/// pass the same one, so a change that old apps cannot read gets a new
/// number.
pub const ALPN: &[u8] = b"cortex/remote/1";

/// A picture sent with an instruction can be several megabytes.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// What the desktop app tells a phone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Down {
    /// The phone has been let in.
    Welcome {
        host: String,
    },

    /// The phone has not been let in, and why. The connection ends.
    Refused {
        reason: String,
    },

    /// The chats, projects and settings, whole. Sent on connecting and
    /// again whenever any of it changes.
    State(HostState),

    /// Something about one chat.
    Chat {
        chat: String,
        message: ChatDown,
    },

    /// The chat's pane was closed on the desktop.
    ChatClosed {
        chat: String,
    },

    Pong,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostState {
    /// The chats open on the desktop, in the order they were opened.
    pub chats: Vec<ChatInfo>,

    /// The folders in the desktop's sidebar, with their past sessions.
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatInfo {
    pub id: String,

    /// The first words of the conversation, or "New chat".
    pub title: String,

    /// The folder the agent works in.
    pub place: String,

    pub status: ChatStatus,

    /// A run is in progress.
    pub running: bool,

    /// The log of the session in progress, once it has one.
    pub log: Option<String>,

    pub settings: SettingsView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChatStatus {
    /// The agent is being built.
    Starting,

    Ready,

    /// The agent could not be built, and why.
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub path: String,

    /// The folder's own name.
    pub name: String,

    /// The folder that sessions outside any project work in.
    pub scratch: bool,

    /// Newest first.
    pub sessions: Vec<PastSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PastSession {
    pub path: String,
    pub title: String,

    /// Milliseconds since 1970.
    pub updated_ms: u64,
}

/// What a phone may see and change of a chat's settings. API keys never
/// leave the desktop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsView {
    pub provider: String,
    pub model: String,

    /// Every model that can be switched to.
    pub models: Vec<ModelChoice>,

    pub effort: String,
    pub efforts: Vec<String>,

    /// The provider in use has an effort setting at all.
    pub takes_effort: bool,

    /// Tools change things without asking first.
    pub approve_everything: bool,

    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// The provider as the settings spell it.
    pub provider: String,

    pub provider_label: String,
    pub model: String,
}

/// One of the engine's optional capabilities, as a switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// As `--with` spells it.
    pub name: String,

    pub label: String,
    pub help: String,
    pub on: bool,

    /// The heading it is listed under.
    pub group: String,
}

/// Something about one chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ChatDown {
    /// The chat now shows another session: a new, empty one, or an
    /// earlier one whose events these are.
    Replaced {
        events: Vec<Event>,
    },

    /// Something happened in the run.
    Event(Event),

    /// The model has begun a response. Sent again if the request is
    /// retried, in which case the text so far is void.
    WritingStarted,

    Writing(String),
    Thinking(String),

    /// The model has begun asking for the tool of this name.
    Calling(String),

    Retrying {
        reason: String,
        wait_ms: u64,
    },
    RunFailed {
        error: String,
        can_continue: bool,
    },
    RunStopped,

    /// The agent is ready for the next instruction.
    Idle,

    Notice {
        text: String,
        bad: bool,
    },

    /// A tool wants to change something and is waiting for an answer,
    /// from the desktop or from a phone, whichever comes first.
    Approval {
        id: u64,
        tool: String,
        summary: String,
    },

    /// The question was answered or its run was abandoned.
    ApprovalClosed {
        id: u64,
    },

    /// What the mind expects to be asked next.
    Suggestion(Option<String>),

    /// What the engine remembers.
    Memory(MemoryGraph),
}

/// The engine's memory as dots and the lines between them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryGraph {
    pub nodes: Vec<MemoryNode>,

    /// Pairs of positions in `nodes`.
    pub links: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryNode {
    pub kind: MemoryKind,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Held for the run in progress.
    Working,

    /// An earlier task and how it went.
    Episode,

    /// A fact kept for good.
    Fact,
}

/// What a phone tells the desktop app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    /// The first message. `token` is the pairing code, from a phone that
    /// has not been let in before.
    Hello {
        device: String,
        token: Option<String>,
    },

    Ping,

    Chat {
        chat: String,
        command: ChatUp,
    },
}

/// What a phone asks of one chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatUp {
    Submit {
        text: String,
        images: Vec<Image>,
    },

    /// Something to tell the run in progress.
    Steer {
        text: String,
    },

    Stop,

    /// Carry on the run that stopped on a provider failure.
    Continue,

    Approve {
        id: u64,
        decision: Decision,
    },

    /// Start a new session, in `folder` or where the chat already is.
    NewSession {
        folder: Option<String>,
    },

    /// Take up the earlier session logged at `path`.
    OpenSession {
        path: String,
    },

    SetCapability {
        name: String,
        on: bool,
    },
    SetModel {
        provider: String,
        model: String,
    },
    SetEffort {
        effort: String,
    },
    SetApproveEverything {
        on: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,

    /// Allow this and everything after it, for the rest of the session.
    AlwaysAllow,

    Deny,
}

pub async fn write<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let text = serde_json::to_vec(message)?;

    if text.len() > MAX_FRAME {
        bail!("a message of {} bytes is too large to send", text.len());
    }

    stream.write_all(&(text.len() as u32).to_be_bytes()).await?;
    stream.write_all(&text).await?;

    Ok(())
}

pub async fn read<T: DeserializeOwned>(stream: &mut RecvStream) -> Result<T> {
    let mut length = [0u8; 4];

    stream.read_exact(&mut length).await?;

    let length = u32::from_be_bytes(length) as usize;

    if length > MAX_FRAME {
        bail!("the other side sent a message of {length} bytes, which is too large");
    }

    let mut text = vec![0u8; length];

    stream.read_exact(&mut text).await?;

    serde_json::from_slice(&text).context("the other side sent a message that cannot be read")
}
