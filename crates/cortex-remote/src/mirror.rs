//! What a phone that connects now has to be told to catch up.
//!
//! Everything the desktop app sends goes through here. The mirror keeps
//! the latest state and, for each chat, what has happened in its
//! session, so a phone that connects in the middle of a run sees the
//! same conversation as one that was there from the start.

use std::sync::Mutex;

use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::wire::{ChatDown, Down, HostState};

/// How many messages a phone may fall behind before it is cut off and
/// has to catch up afresh.
const BACKLOG: usize = 4096;

pub struct Mirror {
    kept: Mutex<Kept>,
    live: Sender<Down>,
}

#[derive(Default)]
struct Kept {
    state: HostState,

    /// Each chat's session so far, in the order the chats were first
    /// heard from.
    chats: Vec<(String, Vec<ChatDown>)>,
}

impl Default for Mirror {
    fn default() -> Self {
        Self {
            kept: Mutex::default(),
            live: broadcast::channel(BACKLOG).0,
        }
    }
}

impl Mirror {
    /// Sends `message` to every phone and keeps what a later one needs.
    pub fn publish(&self, message: Down) {
        let mut kept = self.kept.lock().unwrap_or_else(|error| error.into_inner());

        match &message {
            Down::State(state) => kept.state = state.clone(),
            Down::Chat { chat, message } => kept.session(chat).push_kept(message),
            Down::ChatClosed { chat } => kept.chats.retain(|(id, _)| id != chat),
            Down::Welcome { .. } | Down::Refused { .. } | Down::Pong => {}
        }

        // Sent while the lock is held, so that a phone attaching at this
        // moment gets the message once: from the past or live, not both.
        let _ = self.live.send(message);
    }

    /// The latest state, if it differs from `state`. Saves sending the
    /// same thing every time the window looks.
    pub fn state_differs(&self, state: &HostState) -> bool {
        let kept = self.kept.lock().unwrap_or_else(|error| error.into_inner());

        kept.state != *state
    }

    /// Everything so far, and everything from now on.
    pub fn attach(&self) -> (Vec<Down>, Receiver<Down>) {
        let kept = self.kept.lock().unwrap_or_else(|error| error.into_inner());

        let mut past = vec![Down::State(kept.state.clone())];

        for (chat, session) in &kept.chats {
            past.extend(session.iter().map(|message| Down::Chat {
                chat: chat.clone(),
                message: message.clone(),
            }));
        }

        (past, self.live.subscribe())
    }
}

impl Kept {
    fn session(&mut self, chat: &str) -> &mut Vec<ChatDown> {
        let position = match self.chats.iter().position(|(id, _)| id == chat) {
            Some(position) => position,

            None => {
                self.chats.push((chat.to_string(), Vec::new()));

                self.chats.len() - 1
            }
        };

        &mut self.chats[position].1
    }
}

trait Session {
    fn push_kept(&mut self, message: &ChatDown);
}

impl Session for Vec<ChatDown> {
    fn push_kept(&mut self, message: &ChatDown) {
        match message {
            ChatDown::Replaced { .. } => self.clear(),

            // The pieces of a response are replaced by the event that
            // carries the whole of it, and by anything that ends the run.
            ChatDown::Event(_)
            | ChatDown::WritingStarted
            | ChatDown::RunFailed { .. }
            | ChatDown::RunStopped
            | ChatDown::Idle => {
                while self.last().is_some_and(is_passing) {
                    self.pop();
                }
            }

            ChatDown::ApprovalClosed { id } => {
                let asked = *id;

                self.retain(|kept| !matches!(kept, ChatDown::Approval { id, .. } if *id == asked));

                return;
            }

            ChatDown::Suggestion(_) => {
                self.retain(|kept| !matches!(kept, ChatDown::Suggestion(_)));
            }

            ChatDown::Memory(_) => self.retain(|kept| !matches!(kept, ChatDown::Memory(_))),

            _ => {}
        }

        self.push(message.clone());
    }
}

/// Only means something while the response it belongs to is being
/// written.
fn is_passing(message: &ChatDown) -> bool {
    matches!(
        message,
        ChatDown::WritingStarted
            | ChatDown::Writing(_)
            | ChatDown::Thinking(_)
            | ChatDown::Calling(_)
            | ChatDown::Retrying { .. }
    )
}

#[cfg(test)]
mod tests {
    use cortex_core::Event;

    use super::*;

    fn chat(message: ChatDown) -> Down {
        Down::Chat {
            chat: "agent-1".into(),
            message,
        }
    }

    fn kept(mirror: &Mirror) -> Vec<ChatDown> {
        let (past, _) = mirror.attach();

        past.into_iter()
            .filter_map(|message| match message {
                Down::Chat { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    fn interjected() -> Event {
        Event::UserInterjected {
            step: 1,
            text: "also the tests".into(),
        }
    }

    #[test]
    fn a_late_phone_gets_the_state_first_and_then_each_chat() {
        let mirror = Mirror::default();

        mirror.publish(chat(ChatDown::Idle));

        let (past, _) = mirror.attach();

        assert!(matches!(past[0], Down::State(_)));
        assert!(matches!(past[1], Down::Chat { .. }));
    }

    #[test]
    fn the_pieces_of_a_response_give_way_to_the_whole() {
        let mirror = Mirror::default();

        mirror.publish(chat(ChatDown::WritingStarted));
        mirror.publish(chat(ChatDown::Writing("Loo".into())));
        mirror.publish(chat(ChatDown::Writing("king".into())));

        assert_eq!(kept(&mirror).len(), 3);

        mirror.publish(chat(ChatDown::Event(interjected())));

        let kept = kept(&mirror);

        assert_eq!(kept.len(), 1);
        assert!(matches!(kept[0], ChatDown::Event(_)));
    }

    #[test]
    fn another_session_forgets_the_one_before() {
        let mirror = Mirror::default();

        mirror.publish(chat(ChatDown::Event(interjected())));
        mirror.publish(chat(ChatDown::Replaced { events: Vec::new() }));

        assert_eq!(kept(&mirror).len(), 1);
    }

    #[test]
    fn an_answered_question_is_not_asked_again() {
        let mirror = Mirror::default();

        mirror.publish(chat(ChatDown::Approval {
            id: 7,
            tool: "shell".into(),
            summary: "cargo test".into(),
        }));

        mirror.publish(chat(ChatDown::ApprovalClosed { id: 7 }));

        assert!(kept(&mirror).is_empty());
    }

    #[test]
    fn a_closed_chat_is_gone() {
        let mirror = Mirror::default();

        mirror.publish(chat(ChatDown::Idle));

        mirror.publish(Down::ChatClosed {
            chat: "agent-1".into(),
        });

        assert!(kept(&mirror).is_empty());
    }

    #[tokio::test]
    async fn what_is_published_after_attaching_arrives_live() {
        let mirror = Mirror::default();

        let (_, mut live) = mirror.attach();

        mirror.publish(Down::Pong);

        assert!(matches!(live.recv().await, Ok(Down::Pong)));
    }
}
