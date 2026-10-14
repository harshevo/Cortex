//! What the screens show, in the plain shapes Swift and Kotlin are handed.

use std::time::Instant;

use cortex_core::Outcome;
use cortex_remote::stats::Cell;
use cortex_remote::transcript::{Activity, Item, Session, Tone};
use cortex_remote::wire;

/// Longest tool output handed over, in characters. A screenful is
/// plenty on a phone; the desktop has the rest.
const MAX_OUTPUT_CHARS: usize = 6_000;

/// The fields of a tool's input that say what the call is about, in the
/// order they are looked for.
const SUMMARY_FIELDS: [&str; 6] = ["command", "path", "pattern", "query", "symbol", "url"];

/// Whether there is a desktop to talk to.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum Connection {
    /// Nothing is being tried: no desktop is chosen.
    Idle,

    Connecting {
        host: String,
    },
    Connected {
        host: String,
    },

    /// The connection was lost, or has not been made yet, and is being
    /// tried again.
    Reconnecting {
        host: String,
        reason: String,
    },

    /// It was not possible, and trying again will not help.
    Failed {
        reason: String,
    },
}

/// A desktop this phone has been paired with.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HostView {
    pub id: String,
    pub name: String,
}

/// What every screen needs: the connection and the lists.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppState {
    pub connection: Connection,
    pub hosts: Vec<HostView>,

    /// The desktop that is chosen, connected or not.
    pub host: Option<String>,

    /// The agent on this phone is the one in use, not a desktop's.
    pub on_phone: bool,

    pub chats: Vec<ChatView>,
    pub projects: Vec<ProjectView>,
}

/// Who the model on this phone is reached as.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AccountView {
    pub provider: String,
    pub model: String,

    /// There is a ChatGPT sign-in on this phone.
    pub signed_in: bool,

    /// The sign-in page is being shown.
    pub signing_in: bool,

    /// Why the last sign-in did not work.
    pub sign_in_problem: Option<String>,

    /// The provider in use has an API key.
    pub has_key: bool,

    /// The hosted provider's address.
    pub address: String,

    pub providers: Vec<ProviderView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ProviderView {
    pub name: String,
    pub label: String,
}

/// What a rest did to the growing network.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct RestView {
    /// Times it went over a recent moment.
    pub replays: u32,

    pub neurons: u64,
    pub synapses: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChatView {
    pub id: String,
    pub title: String,

    /// The folder the agent works in, and that folder's own name.
    pub place: String,
    pub place_name: String,

    pub ready: bool,

    /// Why there is no agent, when there is none.
    pub problem: Option<String>,

    pub running: bool,

    /// A tool is waiting for an answer.
    pub asking: bool,

    pub model: String,

    /// The log of the session in progress, once it has one.
    pub log: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ProjectView {
    pub path: String,
    pub name: String,

    /// The folder that sessions outside any project work in.
    pub scratch: bool,

    pub sessions: Vec<SessionView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SessionView {
    pub path: String,
    pub title: String,

    /// Milliseconds since 1970.
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SettingsView {
    pub provider: String,
    pub model: String,
    pub models: Vec<ModelView>,
    pub effort: String,
    pub efforts: Vec<String>,
    pub takes_effort: bool,
    pub approve_everything: bool,
    pub capabilities: Vec<CapabilityView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ModelView {
    pub provider: String,
    pub provider_label: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CapabilityView {
    pub name: String,
    pub label: String,
    pub help: String,
    pub on: bool,
    pub group: String,
}

/// One thing in the conversation.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ItemView {
    User {
        id: u64,
        text: String,

        /// How many pictures went with it.
        images: u32,
    },

    /// Context the engine put in front of the task.
    Briefing {
        id: u64,
        source: String,
        text: String,
    },

    Assistant {
        id: u64,
        text: String,
        thinking: String,
        seconds: f32,
        speed: Option<f32>,
    },

    /// Tool calls that came one after another, shown as one group.
    Tools {
        id: u64,
        tools: Vec<ToolView>,
    },

    Checks {
        id: u64,
        passed: bool,
        checks: Vec<CheckView>,
    },

    /// The finished work held against the request.
    Review {
        id: u64,
        sent_back: bool,
        requirements: Vec<RequirementView>,
    },

    Notice {
        id: u64,
        text: String,
        bad: bool,
    },

    Summary {
        id: u64,
        headline: String,
        tone: SummaryTone,
        steps: u32,
        tokens: u64,
        seconds: f32,
    },
}

impl ItemView {
    /// Where in the session the item, or the first of its tools, is.
    pub fn id(&self) -> u64 {
        match self {
            Self::User { id, .. }
            | Self::Briefing { id, .. }
            | Self::Assistant { id, .. }
            | Self::Tools { id, .. }
            | Self::Checks { id, .. }
            | Self::Review { id, .. }
            | Self::Notice { id, .. }
            | Self::Summary { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SummaryTone {
    Good,
    Warning,
    Bad,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ToolView {
    pub name: String,

    /// What the call is about, in a line.
    pub summary: String,

    pub input: String,

    /// `None` while the tool is still running.
    pub output: Option<String>,

    pub failed: bool,
    pub seconds: f32,
}

/// What a tool was handed and what it answered: the part of a
/// [`ToolView`] that is only fetched when its row is unfolded.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ToolDetailView {
    pub input: String,

    /// `None` while the tool is still running.
    pub output: Option<String>,
}

/// What changed in a conversation since the screens last asked.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct ItemsDelta {
    /// Hand this back the next time, to be told only what is new.
    pub revision: u64,

    /// How many of the items the screens already hold, counted from the
    /// first, are still as they were. The rest are dropped.
    pub keep: u32,

    /// What follows the kept ones. Tools come without their input and
    /// output, which [`ToolDetailView`] has: `input` is empty, and
    /// `output` is empty once the tool has finished.
    pub items: Vec<ItemView>,
}

/// The response being written, or only what was added to it since the
/// screens last asked.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct WritingView {
    /// Which response this is. Hand it back the next time, with the two
    /// places below, to be handed only what was added.
    pub response: u64,

    /// The `id` the response will have in the conversation once it is
    /// complete.
    pub item: u64,

    /// Something is being written. When not, the texts are empty.
    pub active: bool,

    /// The texts here are whole, and replace what the screens hold:
    /// it is another response than the one asked about.
    pub whole: bool,

    /// What was added to the text, and where the text now ends.
    pub text: String,
    pub text_end: u64,

    /// The same for the reasoning that comes before the text.
    pub thinking: String,
    pub thinking_end: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CheckView {
    pub command: String,
    pub passed: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RequirementView {
    pub text: String,
    pub met: bool,
    pub why: String,
}

/// What changes while a run is in progress.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct LiveView {
    pub running: bool,

    /// The last run stopped on a failure that may not last.
    pub can_continue: bool,

    /// What the run is busy with while there is nothing to read.
    pub activity: Option<String>,

    /// How long it has been busy with it.
    pub activity_seconds: f32,

    /// The response being written.
    pub writing: Option<String>,
    pub thinking: Option<String>,

    /// Tokens per second, of the response being written or else of the
    /// last one.
    pub speed: Option<f32>,

    pub approvals: Vec<ApprovalView>,

    /// What the mind expects to be asked next.
    pub suggestion: Option<String>,

    /// Said to the run in progress and not yet read by the model.
    pub queued: Vec<String>,

    pub stats: StatsView,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ApprovalView {
    pub id: u64,
    pub tool: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct StatsView {
    pub input_tokens: u64,
    pub output_tokens: u64,

    /// The share of input that was read from the provider's cache.
    pub cache_hit: Option<f32>,

    /// Model calls made in the current run.
    pub step: u32,

    /// The loop of the current run, one square per thing done.
    pub cells: Vec<CellView>,

    /// Tokens per second of recent model calls, oldest first.
    pub speeds: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CellView {
    Thinking,
    Model,
    Observation,
    FailedObservation,
    CheckPassed,
    CheckFailed,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct MemoryView {
    pub nodes: Vec<MemoryNodeView>,
    pub links: Vec<MemoryLinkView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MemoryNodeView {
    pub kind: MemoryKindView,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MemoryKindView {
    Working,
    Episode,
    Fact,
}

/// Two positions in the nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MemoryLinkView {
    pub from: u32,
    pub to: u32,
}

/// A picture to send with an instruction.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Picture {
    /// Such as `image/jpeg`.
    pub media_type: String,

    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Answer {
    Allow,

    /// Allow this and everything after it, for the rest of the session.
    AlwaysAllow,

    Deny,
}

impl From<Answer> for wire::Decision {
    fn from(answer: Answer) -> Self {
        match answer {
            Answer::Allow => Self::Allow,
            Answer::AlwaysAllow => Self::AlwaysAllow,
            Answer::Deny => Self::Deny,
        }
    }
}

pub fn settings(settings: &wire::SettingsView) -> SettingsView {
    SettingsView {
        provider: settings.provider.clone(),
        model: settings.model.clone(),
        models: settings
            .models
            .iter()
            .map(|choice| ModelView {
                provider: choice.provider.clone(),
                provider_label: choice.provider_label.clone(),
                model: choice.model.clone(),
            })
            .collect(),
        effort: settings.effort.clone(),
        efforts: settings.efforts.clone(),
        takes_effort: settings.takes_effort,
        approve_everything: settings.approve_everything,
        capabilities: settings
            .capabilities
            .iter()
            .map(|capability| CapabilityView {
                name: capability.name.clone(),
                label: capability.label.clone(),
                help: capability.help.clone(),
                on: capability.on,
                group: capability.group.clone(),
            })
            .collect(),
    }
}

pub fn projects(projects: &[wire::Project]) -> Vec<ProjectView> {
    projects
        .iter()
        .map(|project| ProjectView {
            path: project.path.clone(),
            name: project.name.clone(),
            scratch: project.scratch,
            sessions: project
                .sessions
                .iter()
                .map(|session| SessionView {
                    path: session.path.clone(),
                    title: session.title.clone(),
                    updated_ms: session.updated_ms,
                })
                .collect(),
        })
        .collect()
}

pub fn memory(graph: &wire::MemoryGraph) -> MemoryView {
    MemoryView {
        nodes: graph
            .nodes
            .iter()
            .map(|node| MemoryNodeView {
                kind: match node.kind {
                    wire::MemoryKind::Working => MemoryKindView::Working,
                    wire::MemoryKind::Episode => MemoryKindView::Episode,
                    wire::MemoryKind::Fact => MemoryKindView::Fact,
                },
                title: node.title.clone(),
                text: node.text.clone(),
            })
            .collect(),
        links: graph
            .links
            .iter()
            .map(|(from, to)| MemoryLinkView {
                from: *from,
                to: *to,
            })
            .collect(),
    }
}

/// The conversation as a list of things to draw. Tool calls that follow
/// one another become one group, and what is only bookkeeping is left
/// out.
pub fn items(session: &Session) -> Vec<ItemView> {
    let mut shown = Vec::new();

    extend(session, 0, Detail::Full, &mut shown);

    shown
}

/// How much of each tool call is handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// With what the tool was handed and what it answered.
    Full,

    /// Without them: a conversation is mostly these, and they are only
    /// read when a row is unfolded. [`tool_detail`] has them.
    Brief,
}

/// Adds to `shown` what is drawn of the session's items from `from` on.
/// `shown` has to end where a group of tools cannot be carried on: empty,
/// or with something that is not one.
pub fn extend(session: &Session, from: usize, detail: Detail, shown: &mut Vec<ItemView>) {
    for (index, item) in session.items.iter().enumerate().skip(from) {
        let id = index as u64;

        let view = match item {
            Item::User { text, images } => ItemView::User {
                id,
                text: text.clone(),
                images: images.len() as u32,
            },

            Item::Briefing { source, text, .. } => ItemView::Briefing {
                id,
                source: source.clone(),
                text: text.clone(),
            },

            Item::Assistant {
                text,
                thinking,
                duration_ms,
                speed,
                ..
            } => {
                if is_hidden(item) {
                    continue;
                }

                ItemView::Assistant {
                    id,
                    text: text.clone(),
                    thinking: thinking.clone(),
                    seconds: *duration_ms as f32 / 1000.0,
                    speed: *speed,
                }
            }

            Item::Tool(tool) => {
                let (input, output) = match detail {
                    Detail::Full => {
                        let detail = detail_of(tool);

                        (detail.input, detail.output)
                    }

                    Detail::Brief => (String::new(), tool.result.as_ref().map(|_| String::new())),
                };

                let tool = ToolView {
                    name: tool.call.name.clone(),
                    summary: summary(&tool.call.input),
                    input,
                    output,
                    failed: tool.result.as_ref().is_some_and(|result| result.is_error),
                    seconds: tool.duration_ms as f32 / 1000.0,
                };

                if let Some(ItemView::Tools { tools, .. }) = shown.last_mut() {
                    tools.push(tool);

                    continue;
                }

                ItemView::Tools {
                    id,
                    tools: vec![tool],
                }
            }

            Item::Checks { passed, checks, .. } => ItemView::Checks {
                id,
                passed: *passed,
                checks: checks
                    .iter()
                    .map(|check| CheckView {
                        command: check.command.clone(),
                        passed: check.passed,
                        output: clip(&check.output),
                    })
                    .collect(),
            },

            Item::Review {
                requirements,
                sent_back,
                ..
            } => ItemView::Review {
                id,
                sent_back: *sent_back,
                requirements: requirements
                    .iter()
                    .map(|requirement| RequirementView {
                        text: requirement.text.clone(),
                        met: requirement.met,
                        why: requirement.why.clone(),
                    })
                    .collect(),
            },

            Item::Notice { text, tone } => {
                if is_hidden(item) {
                    continue;
                }

                ItemView::Notice {
                    id,
                    text: text.clone(),
                    bad: *tone == Tone::Bad,
                }
            }

            Item::RunSummary {
                outcome,
                verified,
                steps,
                usage,
                duration_ms,
            } => {
                let (headline, tone) = match (outcome, verified) {
                    (Outcome::Completed, Some(true)) => {
                        ("Finished and verified", SummaryTone::Good)
                    }

                    (Outcome::Completed, Some(false)) => {
                        ("Finished, checks still failing", SummaryTone::Warning)
                    }

                    (Outcome::Completed, None) => ("Finished", SummaryTone::Good),
                    (Outcome::StepLimit, _) => ("Stopped at the step limit", SummaryTone::Warning),

                    (Outcome::Truncated, _) => {
                        ("Stopped: the response was cut off", SummaryTone::Warning)
                    }

                    (Outcome::Refused, _) => ("Stopped: the model declined", SummaryTone::Bad),
                };

                ItemView::Summary {
                    id,
                    headline: headline.to_string(),
                    tone,
                    steps: *steps,
                    tokens: usage.total(),
                    seconds: *duration_ms as f32 / 1000.0,
                }
            }
        };

        shown.push(view);
    }
}

/// Left out of the conversation: a step that only called tools has
/// nothing to read, and the engine's tidying is only bookkeeping.
fn is_hidden(item: &Item) -> bool {
    match item {
        Item::Assistant { text, thinking, .. } => {
            text.trim().is_empty() && thinking.trim().is_empty()
        }

        Item::Notice { tone, .. } => *tone == Tone::Housekeeping,
        _ => false,
    }
}

/// The input and output of one tool call: number `index` of the group
/// of tools whose `id` is `item`.
pub fn tool_detail(session: &Session, item: usize, index: usize) -> Option<ToolDetailView> {
    session
        .items
        .get(item..)?
        .iter()
        .filter(|item| !is_hidden(item))
        .map_while(|item| match item {
            Item::Tool(tool) => Some(tool),
            _ => None,
        })
        .nth(index)
        .map(detail_of)
}

fn detail_of(tool: &cortex_remote::transcript::ToolItem) -> ToolDetailView {
    ToolDetailView {
        input: serde_json::to_string_pretty(&tool.call.input).unwrap_or_default(),
        output: tool.result.as_ref().map(|result| clip(&result.output)),
    }
}

/// What the run is busy with, in words, and for how long.
pub fn activity(session: &Session) -> (Option<String>, f32) {
    let Some(doing) = &session.doing else {
        return (None, 0.0);
    };

    let model = session.model.as_deref().unwrap_or("the model");

    let text = match &doing.activity {
        Activity::Preparing => "Looking up memory and relevant code".to_string(),
        Activity::Waiting => format!("Waiting for {model}"),

        Activity::Retrying { reason, until } => {
            let left = until.saturating_duration_since(Instant::now()).as_secs();

            format!("{reason}. Asking again in {left}s")
        }

        Activity::Calling(tool) => format!("Writing a call to {}", tool.replace('_', " ")),
        Activity::Attempting => "Making attempts in copies of the project".to_string(),
        Activity::Finishing => "Checking the work".to_string(),
    };

    (Some(text), doing.since.elapsed().as_secs_f32())
}

pub fn stats(session: &Session) -> StatsView {
    let stats = &session.stats;

    StatsView {
        input_tokens: stats.usage.total_input(),
        output_tokens: stats.usage.output_tokens,
        cache_hit: stats.cache_hit_ratio(),
        step: stats.step,
        cells: stats
            .cells
            .iter()
            .map(|cell| match cell {
                Cell::Thinking => CellView::Thinking,
                Cell::Model => CellView::Model,
                Cell::Observation => CellView::Observation,
                Cell::FailedObservation => CellView::FailedObservation,
                Cell::Check { passed: true } => CellView::CheckPassed,
                Cell::Check { passed: false } => CellView::CheckFailed,
            })
            .collect(),
        speeds: stats.recent_speeds().to_vec(),
    }
}

fn summary(input: &serde_json::Value) -> String {
    let text = SUMMARY_FIELDS
        .iter()
        .find_map(|field| input.get(field)?.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string());

    text.lines().next().unwrap_or_default().to_string()
}

fn clip(output: &str) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output.to_string();
    }

    let shown: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    let hidden = output.chars().count() - MAX_OUTPUT_CHARS;

    format!("{shown}\n\n… {hidden} more characters")
}
