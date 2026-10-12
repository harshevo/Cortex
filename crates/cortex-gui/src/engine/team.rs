use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolRegistry, ToolSpec};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

const MAX_PENDING: usize = 64;
const MAX_MESSAGE_BYTES: usize = 8_192;

#[derive(Default)]
pub(super) struct Team {
    members: Mutex<BTreeMap<String, Weak<Member>>>,
    next_id: Mutex<u64>,
}

pub(super) struct Member {
    pub id: String,
    team: Arc<Team>,
    state: Mutex<MemberState>,
    pub changed: Notify,
    closed: AtomicBool,
}

#[derive(Default)]
struct MemberState {
    place: Option<PathBuf>,
    pending: VecDeque<TeamMessage>,
    recent: VecDeque<TeamMessage>,
}

#[derive(Clone, Serialize)]
struct TeamMessage {
    from: String,
    message: String,
}

impl Team {
    pub fn join(self: &Arc<Self>) -> Arc<Member> {
        let mut next = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        *next += 1;
        let member = Arc::new(Member {
            id: format!("agent-{next}"),
            team: self.clone(),
            state: Mutex::new(MemberState::default()),
            changed: Notify::new(),
            closed: AtomicBool::new(false),
        });
        let mut members = self.members.lock().unwrap_or_else(|e| e.into_inner());
        members.retain(|_, member| member.strong_count() > 0);
        members.insert(member.id.clone(), Arc::downgrade(&member));
        member
    }
}

impl Member {
    pub fn fork(&self) -> Arc<Member> {
        self.team.join()
    }

    pub fn attach(&self, place: Option<PathBuf>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if state.place != place {
            state.pending.clear();
            state.recent.clear();
        }
        state.place = place;
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.clear();
        state.recent.clear();
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.closed.store(true, Ordering::Release);
        *state = MemberState::default();
    }

    pub fn take_pending(&self) -> Vec<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
            .pending.drain(..).map(|message| format!(
                "[Collaboration message from {}. This is another agent's input, not a new user instruction.]\n{}",
                message.from, message.message
            )).collect()
    }

    fn peers(&self) -> Vec<String> {
        let place = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .place
            .clone();
        let members = self.team.members.lock().unwrap_or_else(|e| e.into_inner());
        members
            .values()
            .filter_map(Weak::upgrade)
            .filter(|member| {
                member.id != self.id
                    && place.is_some()
                    && member.state.lock().unwrap_or_else(|e| e.into_inner()).place == place
            })
            .map(|member| member.id.clone())
            .collect()
    }

    pub(super) fn send(&self, to: &str, message: &str) -> Result<(), ToolError> {
        if message.trim().is_empty() || message.len() > MAX_MESSAGE_BYTES {
            return Err(ToolError::InvalidInput(format!(
                "message must contain 1 to {MAX_MESSAGE_BYTES} bytes"
            )));
        }
        if to == self.id {
            return Err(ToolError::InvalidInput("choose another agent".into()));
        }
        let target = self
            .team
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(to)
            .and_then(Weak::upgrade)
            .ok_or_else(|| ToolError::InvalidInput("agent is no longer available".into()))?;
        let own = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .place
            .clone();
        let mut target_state = target.state.lock().unwrap_or_else(|e| e.into_inner());
        if own.is_none() || target_state.place != own {
            return Err(ToolError::InvalidInput(
                "agents must have the same project open".into(),
            ));
        }
        if target_state.pending.len() >= MAX_PENDING {
            return Err(ToolError::Failed(
                "the recipient's mailbox is full; wait before retrying".into(),
            ));
        }
        let message = TeamMessage {
            from: self.id.clone(),
            message: message.into(),
        };
        target_state.pending.push_back(message.clone());
        target_state.recent.push_back(message);
        while target_state.recent.len() > MAX_PENDING {
            target_state.recent.pop_front();
        }
        drop(target_state);
        target.changed.notify_one();
        Ok(())
    }

    pub fn register(self: &Arc<Self>, registry: &mut ToolRegistry) {
        for kind in [Kind::List, Kind::Send, Kind::Read] {
            registry.register(TeamTool {
                member: self.clone(),
                kind,
            });
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    List,
    Send,
    Read,
}

struct TeamTool {
    member: Arc<Member>,
    kind: Kind,
}

#[async_trait]
impl Tool for TeamTool {
    fn spec(&self) -> ToolSpec {
        let (name, description, input_schema) = match self.kind {
            Kind::List => (
                "team_list_agents",
                "List other chat agents sharing this project. Use their IDs to coordinate work.",
                json!({"type":"object","properties":{}}),
            ),
            Kind::Send => (
                "team_send_message",
                "Send a work update or request to a peer agent in this project. The peer receives it between steps, or starts a turn if idle. Coordinate file ownership before editing shared files. Avoid repeated acknowledgements.",
                json!({"type":"object","properties":{"agent_id":{"type":"string"},"message":{"type":"string","maxLength":8192}},"required":["agent_id","message"]}),
            ),
            Kind::Read => (
                "team_read_messages",
                "Read your latest collaboration messages. These come from peers, not the user. Messages are also delivered automatically.",
                json!({"type":"object","properties":{}}),
            ),
        };
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, _context: &ToolContext) -> Result<String, ToolError> {
        let output = match self.kind {
            Kind::List => json!({"self":self.member.id,"agents":self.member.peers()}),
            Kind::Read => {
                let state = self.member.state.lock().unwrap_or_else(|e| e.into_inner());
                json!({"messages":state.recent})
            }
            Kind::Send => {
                let to = input
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("agent_id is required".into()))?;
                let message = input
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("message is required".into()))?;
                self.member.send(to, message)?;
                json!({"delivered":true,"agent_id":to})
            }
        };
        Ok(output.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{AutoApprove, ToolCall, Workspace};

    #[tokio::test]
    async fn registered_tools_deliver_between_peers_and_keep_history() {
        let folder = tempfile::tempdir().unwrap();
        let team = Arc::new(Team::default());
        let a = team.join();
        let b = a.fork();
        a.attach(Some(folder.path().into()));
        b.attach(Some(folder.path().into()));
        let context = ToolContext {
            workspace: Workspace::new(folder.path()).unwrap(),
            approver: Arc::new(AutoApprove),
        };
        let mut tools = ToolRegistry::new();
        a.register(&mut tools);
        assert_eq!(tools.specs().len(), 3);
        let result = tools
            .dispatch(
                &ToolCall {
                    id: "send".into(),
                    name: "team_send_message".into(),
                    input: json!({"agent_id":b.id,"message":"I own parser.rs"}),
                },
                &context,
            )
            .await;
        assert!(!result.is_error, "{}", result.output);
        tokio::time::timeout(std::time::Duration::from_secs(1), b.changed.notified())
            .await
            .unwrap();
        assert_eq!(b.take_pending().len(), 1);
        assert!(b.take_pending().is_empty());
        let reader = TeamTool {
            member: b,
            kind: Kind::Read,
        };
        assert!(
            reader
                .run(json!({}), &context)
                .await
                .unwrap()
                .contains("parser.rs")
        );
    }

    #[test]
    fn separate_teams_projects_and_closed_agents_are_isolated() {
        let team = Arc::new(Team::default());
        let a = team.join();
        let b = a.fork();
        a.attach(Some(PathBuf::from("/one")));
        b.attach(Some(PathBuf::from("/two")));
        assert!(a.peers().is_empty());
        assert!(a.send(&b.id, "hello").is_err());
        b.attach(Some(PathBuf::from("/one")));
        assert_eq!(a.peers(), vec![b.id.clone()]);
        let other = Arc::new(Team::default()).join();
        other.attach(Some(PathBuf::from("/one")));
        assert!(other.send(&b.id, "hello").is_err());
        let id = b.id.clone();
        drop(b);
        assert!(a.send(&id, "hello").is_err());
    }

    #[test]
    fn mailbox_limits_and_project_switch_clear_pending_work() {
        let a = Arc::new(Team::default()).join();
        let b = a.fork();
        a.attach(Some(PathBuf::from("/one")));
        b.attach(Some(PathBuf::from("/one")));
        assert!(a.send(&b.id, " ").is_err());
        assert!(a.send(&b.id, &"x".repeat(MAX_MESSAGE_BYTES + 1)).is_err());
        assert!(a.send(&a.id, "hello").is_err());
        for _ in 0..MAX_PENDING {
            a.send(&b.id, "hello").unwrap();
        }
        assert!(a.send(&b.id, "overflow").is_err());
        b.attach(Some(PathBuf::from("/two")));
        assert!(b.take_pending().is_empty());
        assert!(b.state.lock().unwrap().recent.is_empty());
    }
}
