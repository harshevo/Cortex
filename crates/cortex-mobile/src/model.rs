//! Everything the phone knows, kept in step with what the desktop says.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cortex_core::Event;
use cortex_remote::transcript::{Item, Session, Tone};
use cortex_remote::wire::{ChatDown, ChatStatus, Down, HostState, MemoryGraph, SettingsView};

use crate::view::{
    self, ApprovalView, ChatView, Connection, Detail, ItemView, ItemsDelta, LiveView, WritingView,
};

/// A number larger than every one handed out before, by any chat. The
/// screens hold on to these across a connection lost and made again, so
/// one chat's numbers must never be taken for another's.
fn next_revision() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);

    LAST.fetch_add(1, Ordering::Relaxed) + 1
}

/// What a message from the desktop changed, so that only the screens
/// that show it are drawn again.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Change {
    /// The connection, the chats, the projects or a chat's settings.
    State,

    /// The conversation of this chat.
    Items { chat: String },

    /// What changes while a run is in progress in this chat: the text
    /// being written, what it is busy with, its questions and numbers.
    Live { chat: String },

    /// What this chat's agent remembers.
    Memory { chat: String },
}

#[derive(Default)]
pub struct Model {
    pub connection: Option<Connection>,
    pub state: HostState,
    pub chats: HashMap<String, Chat>,
}

pub struct Chat {
    pub session: Session,
    pub approvals: Vec<ApprovalView>,
    pub suggestion: Option<String>,
    pub memory: MemoryGraph,

    /// Said to the run in progress and not yet read by the model.
    pub queued: Vec<String>,

    /// The conversation as the screens draw it, kept in step with the
    /// session so that it is never built again from the start.
    shown: Shown,

    /// Which response is being written, or will be next.
    response: u64,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            session: Session::default(),
            approvals: Vec::new(),
            suggestion: None,
            memory: MemoryGraph::default(),
            queued: Vec::new(),
            shown: Shown::default(),
            response: next_revision(),
        }
    }
}

/// What is drawn of a session's items, and when each last changed.
///
/// A session only ever changes from some item on: things are added at
/// its end, the tool that just finished is near it, and a run that is
/// abandoned takes its unfinished tools with it. So what is drawn is
/// kept up to the first item a message can touch and drawn again from
/// there, and the screens are handed only that.
#[derive(Default)]
struct Shown {
    /// Without the tools' input and output.
    items: Vec<ItemView>,

    /// The revision each of `items` last changed at. Never falls from
    /// one item to the next.
    stamps: Vec<u64>,

    /// The revision of the last change.
    revision: u64,
}

impl Shown {
    /// Draws again what comes of the session's items from `from` on.
    fn redraw(&mut self, session: &Session, from: usize) {
        let mut from = from;
        let mut keep = self.items.partition_point(|item| item.id() < from as u64);

        // The group of tools before it may take in the calls that follow.
        if let Some(ItemView::Tools { id, .. }) = keep.checked_sub(1).map(|last| &self.items[last])
        {
            from = *id as usize;
            keep -= 1;
        }

        let mut fresh = Vec::new();

        view::extend(session, from, Detail::Brief, &mut fresh);

        // What came out as it was keeps its revision.
        let same = self.items[keep..]
            .iter()
            .zip(&fresh)
            .take_while(|(old, new)| old == new)
            .count();

        let keep = keep + same;

        if keep == self.items.len() && same == fresh.len() {
            return;
        }

        self.revision = next_revision();

        self.items.truncate(keep);
        self.stamps.truncate(keep);

        self.items.extend(fresh.into_iter().skip(same));
        self.stamps.resize(self.items.len(), self.revision);
    }

    fn since(&self, revision: u64) -> ItemsDelta {
        let keep = self.stamps.partition_point(|stamp| *stamp <= revision);

        ItemsDelta {
            revision: self.revision,
            keep: keep as u32,
            items: self.items[keep..].to_vec(),
        }
    }
}

impl Model {
    /// Takes in one message. Returns what it changed.
    pub fn apply(&mut self, message: Down) -> Vec<Change> {
        match message {
            Down::State(state) => {
                self.state = state;

                vec![Change::State]
            }

            Down::Chat { chat, message } => {
                let changes = self.chats.entry(chat.clone()).or_default().apply(message);

                changes.into_iter().map(|change| change.of(&chat)).collect()
            }

            Down::ChatClosed { chat } => {
                self.chats.remove(&chat);

                vec![Change::State]
            }

            Down::Welcome { .. } | Down::Refused { .. } | Down::Pong => Vec::new(),
        }
    }

    /// Forgets what the desktop said. What it says on connecting again
    /// replaces it all.
    pub fn clear(&mut self) {
        self.state = HostState::default();
        self.chats.clear();
    }

    pub fn chat_views(&self) -> Vec<ChatView> {
        self.state
            .chats
            .iter()
            .map(|info| {
                let chat = self.chats.get(&info.id);

                let place_name = info
                    .place
                    .rsplit(['/', '\\'])
                    .find(|part| !part.is_empty())
                    .unwrap_or(&info.place)
                    .to_string();

                ChatView {
                    id: info.id.clone(),
                    title: info.title.clone(),
                    place: info.place.clone(),
                    place_name,
                    ready: info.status == ChatStatus::Ready,

                    problem: match &info.status {
                        ChatStatus::Unavailable { reason } => Some(reason.clone()),
                        _ => None,
                    },

                    // The phone hears of a run before the list is sent again.
                    running: chat.map_or(info.running, |chat| chat.session.running),
                    asking: chat.is_some_and(|chat| !chat.approvals.is_empty()),
                    model: info.settings.model.clone(),
                    log: info.log.clone(),
                }
            })
            .collect()
    }

    pub fn settings_mut(&mut self, chat: &str) -> Option<&mut SettingsView> {
        self.state
            .chats
            .iter_mut()
            .find(|info| info.id == chat)
            .map(|info| &mut info.settings)
    }

    pub fn settings(&self, chat: &str) -> Option<&SettingsView> {
        self.state
            .chats
            .iter()
            .find(|info| info.id == chat)
            .map(|info| &info.settings)
    }
}

/// A change before it is known which chat it is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    State,
    Items,
    Live,
    Memory,
}

impl Part {
    fn of(self, chat: &str) -> Change {
        let chat = chat.to_string();

        match self {
            Self::State => Change::State,
            Self::Items => Change::Items { chat },
            Self::Live => Change::Live { chat },
            Self::Memory => Change::Memory { chat },
        }
    }
}

impl Chat {
    fn apply(&mut self, message: ChatDown) -> Vec<Part> {
        let touched = self.touched_by(&message);

        if matches!(message, ChatDown::Replaced { .. }) {
            self.shown = Shown::default();
        }

        let parts = self.take_in(message);

        if let Some(from) = touched {
            self.shown.redraw(&self.session, from);
        }

        // A full pass, in test builds, says at once if the session has
        // learnt to change in a way `touched_by` does not know of.
        debug_assert_eq!(self.shown.items, {
            let mut whole = Vec::new();

            view::extend(&self.session, 0, Detail::Brief, &mut whole);

            whole
        });

        // Whatever is written next is another response.
        if self.session.writing.is_none() {
            self.response = next_revision();
        }

        parts
    }

    /// The first of the session's items that `message` can change, add
    /// or remove, if it touches the items at all.
    fn touched_by(&self, message: &ChatDown) -> Option<usize> {
        let items = &self.session.items;

        let unfinished = |item: &Item| matches!(item, Item::Tool(tool) if tool.result.is_none());

        match message {
            ChatDown::Replaced { .. } => Some(0),

            // The result goes to the last unfinished call of its id.
            ChatDown::Event(Event::ToolFinished { result, .. }) => Some(
                items
                    .iter()
                    .rposition(|item| {
                        matches!(item, Item::Tool(tool) if tool.call.id == result.call_id)
                            && unfinished(item)
                    })
                    .unwrap_or(items.len()),
            ),

            // Pictures go to the instruction just sent.
            ChatDown::Event(Event::ImagesAttached { .. }) => Some(items.len().saturating_sub(1)),

            // These only add to the end, when they add anything.
            ChatDown::Event(
                Event::BrainObserved { .. }
                | Event::RunStarted { .. }
                | Event::RunResumed { .. }
                | Event::ContextPrepared { .. }
                | Event::ModelStarted { .. }
                | Event::ModelResponded { .. }
                | Event::ToolStarted { .. }
                | Event::UserInterjected { .. }
                | Event::ContextPruned { .. }
                | Event::ContextMasked { .. }
                | Event::ReviewRan { .. }
                | Event::LessonsLearned { .. }
                | Event::AttemptStarted { .. }
                | Event::AttemptFinished { .. }
                | Event::AttemptChosen { .. }
                | Event::VerificationRan { .. }
                | Event::RunFinished { .. },
            )
            | ChatDown::Notice { .. } => Some(items.len()),

            // What an event that is not known here does to the items is
            // not known either: all of them are gone over. It costs a
            // pass, and what did not change is still not handed over.
            ChatDown::Event(_) => Some(0),

            // A run that ends early takes its unfinished tools with it.
            ChatDown::RunFailed { .. } | ChatDown::RunStopped => {
                Some(items.iter().position(unfinished).unwrap_or(items.len()))
            }

            ChatDown::WritingStarted
            | ChatDown::Writing(_)
            | ChatDown::Thinking(_)
            | ChatDown::Calling(_)
            | ChatDown::Retrying { .. }
            | ChatDown::Idle
            | ChatDown::Approval { .. }
            | ChatDown::ApprovalClosed { .. }
            | ChatDown::Suggestion(_)
            | ChatDown::Memory(_) => None,
        }
    }

    fn take_in(&mut self, message: ChatDown) -> Vec<Part> {
        match message {
            ChatDown::Replaced { events } => {
                self.session = Session::default();
                self.approvals.clear();
                self.suggestion = None;
                self.queued.clear();

                for event in &events {
                    self.session.apply(event);
                }

                vec![Part::Items, Part::Live, Part::State]
            }

            ChatDown::Event(event) => {
                let was_running = self.session.running;

                self.heard(&event);
                self.session.apply(&event);

                let mut parts = vec![Part::Items, Part::Live];

                // The list of chats shows which are working.
                if was_running != self.session.running {
                    parts.push(Part::State);
                }

                parts
            }

            ChatDown::WritingStarted => {
                self.session.writing_started();

                vec![Part::Live]
            }

            ChatDown::Writing(piece) => {
                self.session.write(&piece);

                vec![Part::Live]
            }

            ChatDown::Thinking(piece) => {
                self.session.think(&piece);

                vec![Part::Live]
            }

            ChatDown::Calling(tool) => {
                self.session.calling(&tool);

                vec![Part::Live]
            }

            ChatDown::Retrying { reason, wait_ms } => {
                self.session
                    .retrying(&reason, Duration::from_millis(wait_ms));

                vec![Part::Live]
            }

            ChatDown::RunFailed {
                error,
                can_continue,
            } => {
                self.session.fail(&error, can_continue);

                vec![Part::Items, Part::Live, Part::State]
            }

            ChatDown::RunStopped => {
                self.session.stop();

                // Questions from the abandoned run have nobody left to
                // hear the answer.
                self.approvals.clear();

                vec![Part::Items, Part::Live, Part::State]
            }

            ChatDown::Idle => {
                self.session.running = false;

                vec![Part::Live, Part::State]
            }

            ChatDown::Notice { text, bad } => {
                let tone = if bad { Tone::Bad } else { Tone::Muted };

                self.session.notice(&text, tone);

                vec![Part::Items]
            }

            ChatDown::Approval { id, tool, summary } => {
                self.approvals.retain(|approval| approval.id != id);
                self.approvals.push(ApprovalView { id, tool, summary });

                vec![Part::Live, Part::State]
            }

            ChatDown::ApprovalClosed { id } => {
                self.approvals.retain(|approval| approval.id != id);

                vec![Part::Live, Part::State]
            }

            // One that arrives after the next run began is too late.
            ChatDown::Suggestion(suggestion) => {
                self.suggestion = suggestion.filter(|_| !self.session.running);

                vec![Part::Live]
            }

            ChatDown::Memory(graph) => {
                self.memory = graph;

                vec![Part::Memory]
            }
        }
    }

    /// Takes off the queue what `event` shows the model has now read.
    fn heard(&mut self, event: &Event) {
        match event {
            Event::UserInterjected { text, .. } => {
                if let Some(position) = self.queued.iter().position(|queued| queued == text) {
                    self.queued.remove(position);
                }
            }

            // What arrived too late for one run opens the next.
            Event::RunStarted { instruction, .. } => {
                self.suggestion = None;

                self.queued
                    .retain(|queued| !instruction.contains(queued.as_str()));
            }

            _ => {}
        }
    }

    /// What changed in the conversation since `revision`.
    pub fn items_since(&self, revision: u64) -> ItemsDelta {
        self.shown.since(revision)
    }

    /// What was added to the response being written since the screens
    /// were handed it up to `text_end` and `thinking_end`.
    pub fn writing_since(&self, response: u64, text_end: u64, thinking_end: u64) -> WritingView {
        let item = self.session.items.len() as u64;

        let Some(writing) = &self.session.writing else {
            return WritingView {
                response: self.response,
                item,
                whole: true,
                ..WritingView::default()
            };
        };

        // Nothing is ever taken back from a response, so the rest of it
        // from where the screens stopped is all that is new.
        let added = |text: &'_ str, end: u64| -> Option<String> {
            let rest = text.get(usize::try_from(end).ok()?..)?;

            (response == self.response).then(|| rest.to_string())
        };

        let (whole, text, thinking) = match (
            added(&writing.text, text_end),
            added(&writing.thinking, thinking_end),
        ) {
            (Some(text), Some(thinking)) => (false, text, thinking),
            _ => (true, writing.text.clone(), writing.thinking.clone()),
        };

        WritingView {
            response: self.response,
            item,
            active: true,
            whole,
            text,
            text_end: writing.text.len() as u64,
            thinking,
            thinking_end: writing.thinking.len() as u64,
        }
    }

    pub fn live(&self) -> LiveView {
        let mut live = self.live_without_text();

        if let Some(writing) = &self.session.writing {
            live.writing = Some(writing.text.clone()).filter(|text| !text.is_empty());
            live.thinking = Some(writing.thinking.clone()).filter(|text| !text.is_empty());
        }

        live
    }

    /// As [`Chat::live`], without the response being written, which is
    /// most of it and which [`Chat::writing_since`] hands over in pieces.
    pub fn live_without_text(&self) -> LiveView {
        let session = &self.session;
        let (activity, activity_seconds) = view::activity(session);

        let writing = session.writing.as_ref();

        let speed = writing
            .and_then(|writing| writing.estimated_speed())
            .or_else(|| session.stats.speed());

        LiveView {
            running: session.running,
            can_continue: session.can_continue,
            activity,
            activity_seconds,
            writing: None,
            thinking: None,
            speed,
            approvals: self.approvals.clone(),
            suggestion: self.suggestion.clone(),
            queued: self.queued.clone(),
            stats: view::stats(session),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(model: &mut Model, message: ChatDown) -> Vec<Change> {
        model.apply(Down::Chat {
            chat: "agent-1".into(),
            message,
        })
    }

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.into(),
            provider: "local".into(),
            model: "stub".into(),
        }
    }

    #[test]
    fn a_run_shows_up_in_the_conversation_and_as_working() {
        let mut model = Model::default();

        let changes = told(&mut model, ChatDown::Event(started("fix the parser")));

        assert!(changes.contains(&Change::State));

        let chat = &model.chats["agent-1"];

        assert!(chat.live().running);
        assert!(matches!(
            &view::items(&chat.session)[0],
            view::ItemView::User { text, .. } if text == "fix the parser"
        ));
    }

    #[test]
    fn text_arrives_in_pieces_and_only_the_live_part_changes() {
        let mut model = Model::default();

        told(&mut model, ChatDown::WritingStarted);

        let changes = told(&mut model, ChatDown::Writing("Look".into()));

        told(&mut model, ChatDown::Writing("ing".into()));

        assert_eq!(
            changes,
            vec![Change::Live {
                chat: "agent-1".into()
            }]
        );

        assert_eq!(
            model.chats["agent-1"].live().writing.as_deref(),
            Some("Looking")
        );
    }

    #[test]
    fn a_question_waits_until_it_is_closed() {
        let mut model = Model::default();

        told(
            &mut model,
            ChatDown::Approval {
                id: 3,
                tool: "shell".into(),
                summary: "cargo test".into(),
            },
        );

        assert_eq!(model.chats["agent-1"].live().approvals.len(), 1);

        told(&mut model, ChatDown::ApprovalClosed { id: 3 });

        assert!(model.chats["agent-1"].live().approvals.is_empty());
    }

    fn tool_started(id: &str, name: &str) -> Event {
        Event::ToolStarted {
            step: 1,
            call: cortex_core::ToolCall {
                id: id.into(),
                name: name.into(),
                input: serde_json::json!({ "path": format!("{id}.rs"), "content": "x".repeat(400) }),
            },
        }
    }

    fn tool_finished(id: &str, output: &str) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "read_file".into(),
            result: cortex_core::ToolResult {
                call_id: id.into(),
                output: output.into(),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 7,
        }
    }

    fn responded(text: &str) -> Event {
        Event::ModelResponded {
            step: 1,
            message: cortex_core::Message::assistant_text(text),
            stop_reason: cortex_core::StopReason::ToolUse,
            usage: cortex_core::Usage::default(),
            duration_ms: 900,
        }
    }

    fn finished() -> Event {
        Event::RunFinished {
            outcome: cortex_core::Outcome::Completed,
            steps: 3,
            usage: cortex_core::Usage::default(),
            duration_ms: 4_000,
            verified: Some(true),
        }
    }

    /// A session with everything in it that changes the conversation:
    /// runs, text in pieces, tools that finish out of turn, steps that
    /// only call tools, pictures, checks, a review, a run abandoned with
    /// a tool unfinished, and another session taking its place.
    fn scripted_session() -> Vec<ChatDown> {
        use ChatDown::{
            Event as Happened, Idle, Notice, Replaced, RunFailed, RunStopped, Suggestion, Thinking,
            Writing, WritingStarted,
        };

        let mut script = vec![
            Happened(started("fix the parser")),
            Happened(Event::ImagesAttached {
                images: vec![cortex_core::Image {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
            }),
            Happened(Event::ContextPrepared {
                source: "memory".into(),
                text: "An earlier task touched the parser.".into(),
                detail: serde_json::Value::Null,
                duration_ms: 3,
            }),
            Happened(Event::ModelStarted { step: 1 }),
            WritingStarted,
            Thinking("It loops ".into()),
            Thinking("twice.".into()),
            Writing("Look".into()),
            Writing("ing at the ".into()),
            Writing("pärser.".into()),
            Happened(responded("Looking at the pärser.")),
            Happened(tool_started("a", "read_file")),
            Happened(tool_started("b", "search")),
            Happened(tool_finished("b", "parser.rs:4")),
            Happened(tool_finished("a", &"line\n".repeat(3_000))),
            // A step with nothing to read: its tools join the group.
            Happened(Event::ModelStarted { step: 2 }),
            Happened(responded("")),
            Happened(tool_started("c", "edit_file")),
            Happened(Event::ContextPruned {
                step: 2,
                results_trimmed: 1,
                chars_removed: 900,
            }),
            Happened(tool_started("d", "shell")),
            Happened(tool_finished("c", "edited")),
            Happened(tool_finished("d", "ok")),
            Happened(Event::UserInterjected {
                step: 2,
                text: "and the lexer".into(),
            }),
            Notice {
                text: "Saved.".into(),
                bad: false,
            },
            Happened(Event::ModelStarted { step: 3 }),
            WritingStarted,
            Writing("Done.".into()),
            // The request is made again: what was written is void.
            WritingStarted,
            Writing("All ".into()),
            Writing("done.".into()),
            Happened(responded("All done.")),
            Happened(Event::VerificationRan {
                step: 3,
                passed: true,
                checks: vec![cortex_core::CheckOutcome {
                    command: "cargo test".into(),
                    passed: true,
                    duration_ms: 20,
                    output: String::new(),
                }],
                duration_ms: 20,
            }),
            Happened(Event::ReviewRan {
                step: 3,
                requirements: vec![cortex_core::Requirement {
                    text: "The parser is fixed".into(),
                    met: true,
                    why: String::new(),
                }],
                feedback: None,
                usage: cortex_core::Usage::default(),
                duration_ms: 30,
            }),
            Happened(finished()),
            Idle,
            Suggestion(Some("now the lexer".into())),
            // A run that is stopped with one tool done and two not.
            Happened(started("and the lexer")),
            Happened(responded("")),
            Happened(tool_started("e", "read_file")),
            Happened(tool_started("f", "read_file")),
            Happened(tool_started("g", "read_file")),
            Happened(tool_finished("f", "lexer")),
            RunStopped,
            // One that fails while it writes.
            Happened(started("again")),
            Writing("I wi".into()),
            Happened(tool_started("h", "shell")),
            RunFailed {
                error: "The provider is overloaded.".into(),
                can_continue: true,
            },
            Replaced {
                events: vec![started("an earlier one"), responded("It was fine.")],
            },
            Happened(tool_started("i", "search")),
            Happened(tool_finished("i", "nothing")),
        ];

        // And a long tail, as a long session has.
        for turn in 0..40 {
            script.push(Happened(responded(&format!("Step {turn}."))));
            script.push(Happened(tool_started(&format!("t{turn}"), "read_file")));
            script.push(Happened(tool_finished(&format!("t{turn}"), "contents")));
        }

        script
    }

    /// What `view::items` hands over, as the incremental path does:
    /// without what the tools were handed and answered.
    fn brief(session: &Session) -> Vec<ItemView> {
        let mut items = view::items(session);

        for item in &mut items {
            if let ItemView::Tools { tools, .. } = item {
                for tool in tools {
                    tool.input.clear();
                    tool.output = tool.output.as_ref().map(|_| String::new());
                }
            }
        }

        items
    }

    #[test]
    fn following_the_changes_gives_the_same_conversation_as_fetching_it_whole() {
        let mut model = Model::default();

        // Three phones: one asks after every message, one now and then,
        // and one only at the end.
        let mut phones = [
            (1, Vec::new(), 0),
            (7, Vec::new(), 0),
            (usize::MAX, Vec::new(), 0),
        ];

        let script = scripted_session();
        let last = script.len() - 1;

        let mut handed_over = 0;

        for (position, message) in script.into_iter().enumerate() {
            told(&mut model, message);

            let chat = &model.chats["agent-1"];
            let whole = brief(&chat.session);

            for (every, held, revision) in &mut phones {
                if position % *every != 0 && position != last {
                    continue;
                }

                let delta = chat.items_since(*revision);

                held.truncate(delta.keep as usize);
                held.extend(delta.items.iter().cloned());

                *revision = delta.revision;

                assert_eq!(*held, whole, "after message {position}");

                if *every == 1 {
                    handed_over += delta.items.len();
                }

                // Asking again at once hands over nothing.
                let again = chat.items_since(*revision);

                assert_eq!(again.keep as usize, held.len());
                assert!(again.items.is_empty());
            }
        }

        // The phone that followed every message was handed a few items
        // a message, not the conversation each time.
        let items = model.chats["agent-1"].items_since(0).items.len();

        assert!(items > 80);
        assert!(
            handed_over < 3 * (last + 1),
            "{handed_over} items handed over"
        );
    }

    #[test]
    fn a_phone_that_followed_another_conversation_is_handed_the_new_one_whole() {
        let mut model = Model::default();

        told(&mut model, ChatDown::Event(started("first")));
        told(&mut model, ChatDown::Event(responded("One.")));

        let old = model.chats["agent-1"].items_since(0);

        assert_eq!(old.items.len(), 2);

        // The connection is lost and made again: the chat is a new one.
        model.clear();

        told(&mut model, ChatDown::Event(started("second")));
        told(&mut model, ChatDown::Event(responded("Two.")));
        told(&mut model, ChatDown::Event(responded("Three.")));

        let new = model.chats["agent-1"].items_since(old.revision);

        assert_eq!(new.keep, 0);
        assert_eq!(new.items.len(), 3);
    }

    #[test]
    fn a_tool_is_read_in_full_only_when_asked_for() {
        let mut model = Model::default();

        for message in scripted_session().into_iter().take(15) {
            told(&mut model, message);
        }

        let chat = &model.chats["agent-1"];
        let delta = chat.items_since(0);

        let Some(ItemView::Tools { id, tools }) = delta.items.last() else {
            panic!("the conversation should end with the tools");
        };

        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool.input.is_empty()));
        assert_eq!(tools[0].output.as_deref(), Some(""));

        // The same as the whole conversation has for them.
        let whole = view::items(&chat.session);

        let Some(ItemView::Tools { tools: full, .. }) = whole.last() else {
            panic!("the conversation should end with the tools");
        };

        for (index, tool) in full.iter().enumerate() {
            let detail = view::tool_detail(&chat.session, *id as usize, index).unwrap();

            assert_eq!(detail.input, tool.input);
            assert_eq!(detail.output, tool.output);
        }

        assert!(full[0].output.as_ref().unwrap().contains("more characters"));
        assert_eq!(view::tool_detail(&chat.session, *id as usize, 2), None);
    }

    #[test]
    fn tools_of_a_step_with_nothing_to_read_are_found_across_it() {
        let mut model = Model::default();

        for message in scripted_session().into_iter().take(22) {
            told(&mut model, message);
        }

        let chat = &model.chats["agent-1"];

        let Some(ItemView::Tools { id, tools }) = chat.items_since(0).items.last().cloned() else {
            panic!("the conversation should end with the tools");
        };

        assert_eq!(tools.len(), 4);

        let detail = view::tool_detail(&chat.session, id as usize, 3).unwrap();

        assert_eq!(detail.output.as_deref(), Some("ok"));
    }

    #[test]
    fn text_being_written_is_handed_over_in_the_pieces_it_came_in() {
        let mut model = Model::default();

        let mut text = String::new();
        let mut thinking = String::new();
        let mut last = WritingView::default();
        let mut handed_over = 0;
        let mut responses = Vec::new();

        for message in scripted_session() {
            let was_event = matches!(message, ChatDown::Event(Event::ModelResponded { .. }));

            told(&mut model, message);

            let chat = &model.chats["agent-1"];
            let view = chat.writing_since(last.response, last.text_end, last.thinking_end);

            if view.whole || !view.active {
                text.clear();
                thinking.clear();
            }

            text.push_str(&view.text);
            thinking.push_str(&view.thinking);

            handed_over += view.text.len() + view.thinking.len();

            // The same as fetching all of it every time.
            let whole = chat.live();

            assert_eq!(whole.writing.unwrap_or_default(), text);
            assert_eq!(whole.thinking.unwrap_or_default(), thinking);
            assert_eq!(chat.live_without_text().writing, None);

            if view.active {
                responses.push(view.response);

                // Where the response will be in the conversation.
                assert_eq!(view.item, chat.session.items.len() as u64);
            }

            assert!(!(was_event && view.active));

            last = view;
        }

        responses.dedup();

        // Three responses were written, the second of them twice.
        assert_eq!(responses.len(), 4);
        assert!(responses.windows(2).all(|pair| pair[0] < pair[1]));

        // Every character was handed over once.
        assert_eq!(
            handed_over,
            "It loops twice.Looking at the pärser.Done.All done.I wi".len()
        );
    }

    #[test]
    fn a_place_inside_a_character_or_past_the_end_starts_the_text_over() {
        let mut model = Model::default();

        told(&mut model, ChatDown::Writing("pärser".into()));

        let chat = &model.chats["agent-1"];
        let first = chat.writing_since(0, 0, 0);

        assert!(first.whole);
        assert_eq!(first.text, "pärser");

        // Byte 2 is inside the ä.
        for end in [2, 99] {
            let view = chat.writing_since(first.response, end, 0);

            assert!(view.whole);
            assert_eq!(view.text, "pärser");
        }

        let rest = chat.writing_since(first.response, 3, 0);

        assert!(!rest.whole);
        assert_eq!(rest.text, "rser");
    }

    #[test]
    fn another_session_replaces_the_conversation() {
        let mut model = Model::default();

        told(&mut model, ChatDown::Event(started("first")));

        told(
            &mut model,
            ChatDown::Replaced {
                events: vec![started("an earlier one")],
            },
        );

        let items = view::items(&model.chats["agent-1"].session);

        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            view::ItemView::User { text, .. } if text == "an earlier one"
        ));
    }
}
