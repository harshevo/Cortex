//! What can be asked of the brain, and how it answers.
//!
//! The same requests reach it in this process, through a [`crate::Link`]
//! that holds the brain itself, and in the background service, over its
//! socket, so that whichever holds the brain answers the same way.

use std::io;

use cortex_core::Event;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::brain::{Brain, HunchKind};
use crate::senses;

/// How sure the brain must be that a short instruction comes next before
/// it is offered as a suggestion.
const SUGGESTED: f32 = 0.6;

/// Longest line of conversation it hears whole.
const HEARD: usize = 2048;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// The teacher, which lives in the background service.
    #[cfg(unix)]
    TeachStart {
        config: crate::service::teacher::Config,
    },
    #[cfg(unix)]
    TeachStop,
    #[cfg(unix)]
    TeachStatus {
        after: u64,
    },

    /// Training from a file of lessons, which the background service
    /// learns between requests. `source` is an absolute path; training on
    /// the file it stopped in goes on from there unless `restart`.
    #[cfg(unix)]
    TrainStart {
        source: std::path::PathBuf,
        #[serde(default)]
        sleep_every: Option<u64>,
        #[serde(default)]
        restart: bool,
    },
    #[cfg(unix)]
    TrainStop,
    #[cfg(unix)]
    TrainStatus,

    /// How the service is doing: the teacher, training, events waiting to
    /// be lived, and what went wrong lately.
    #[cfg(unix)]
    Status,

    /// A line of conversation: `speaker` is `user` or `assistant`.
    Hear {
        session: String,
        speaker: String,
        text: String,
    },

    /// Living a stream of experience: a stretch begins, a moment of it is
    /// lived, the stretch ends and is kept as a memory.
    Begin {
        stream: String,
        source: String,
    },
    Live {
        stream: String,
        percepts: Vec<String>,
    },
    End {
        stream: String,
    },
    /// What the stream has lived is kept as a memory, and it goes on.
    Close {
        stream: String,
    },

    /// Learn text (twice when it is a correction), say what follows a
    /// prompt, answer a turn of conversation, recall the memory a cue
    /// brings back.
    Learn {
        text: String,
        source: String,
        #[serde(default)]
        correction: bool,
    },
    Study {
        text: String,
        source: String,
        #[serde(default = "study_passes")]
        max_passes: usize,
    },
    Say {
        prompt: String,
    },
    Converse {
        text: String,
    },
    Chat {
        session: String,
        text: String,
    },
    Recall {
        cue: String,
    },

    /// Its state for the view, and a short account for the teacher.
    Snapshot,
    Progress,

    /// Its mind, on request: a thought, an imagining (from a seed, or two
    /// memories spliced when it is empty), a dream, a chain of reasoning
    /// from one thing to another, a hunch, a look at itself.
    Think,
    Imagine {
        #[serde(default)]
        seed: String,
    },
    Dream,
    Reason {
        from: String,
        to: String,
    },
    Intuit {
        #[serde(default)]
        cue: String,
    },
    Reflect,

    /// Every cell and synapse, for drawing the whole cortex.
    Wiring,

    /// The short instruction it expects to be given next, if any.
    Suggestion,

    /// What it expects next in `stream`, the way on it imagines, and a
    /// way to `goal`.
    Outlook {
        stream: String,
        #[serde(default)]
        most: usize,
        #[serde(default)]
        goal: Option<String>,
    },

    /// Something worth saying about the next step in `stream`.
    Hunch {
        stream: String,
    },

    /// The model's own questions about `stream`: how actions it is
    /// weighing have gone on from here, or how an outcome is reached.
    Inspect {
        stream: String,
        query: Value,
    },

    /// Sleep cycles until nothing is in need or the time is up.
    Sleep {
        milliseconds: u64,
    },

    Flush,
    Shutdown,
}

fn study_passes() -> usize {
    4
}

/// What the brain is asked about, at most 1,024 bytes of it.
fn cap(text: &str, most: usize) -> &str {
    let mut end = text.len().min(most);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn round(number: f32) -> f64 {
    (f64::from(number) * 100.0).round() / 100.0
}

fn bounded(text: &str, most: usize, what: &str) -> io::Result<()> {
    if text.trim().is_empty() || text.len() > most {
        return Err(io::Error::other(format!(
            "Give {what} of 1 to {most} bytes"
        )));
    }
    Ok(())
}

/// Whether a request changes the brain, so that it should be saved.
pub fn changes(request: &Request) -> bool {
    !matches!(
        request,
        Request::Snapshot
            | Request::Progress
            | Request::Wiring
            | Request::Suggestion
            | Request::Outlook { .. }
    ) && !is_status(request)
}

#[cfg(unix)]
fn is_status(request: &Request) -> bool {
    matches!(
        request,
        Request::Status | Request::TeachStatus { .. } | Request::TrainStatus
    )
}

#[cfg(not(unix))]
fn is_status(_: &Request) -> bool {
    false
}

/// The brain's answer to `request`. The teacher, training, flushing and
/// shutting down are for whoever holds the brain, and are refused here.
pub fn respond(brain: &mut Brain, request: &Request) -> io::Result<Value> {
    let value = |v: Result<Value, serde_json::Error>| v.map_err(io::Error::from);
    match request {
        Request::Hear {
            session,
            speaker,
            text,
        } => {
            if session.len() > 256 || !matches!(speaker.as_str(), "user" | "assistant") {
                return Err(io::Error::other("Invalid conversation line"));
            }
            hear(brain, speaker, text);
            Ok(json!(true))
        }
        Request::Begin { stream, source } => {
            brain.begin(cap(stream, 256), cap(source, 256));
            Ok(json!(true))
        }
        Request::Live { stream, percepts } => {
            if percepts.len() > 64 {
                return Err(io::Error::other("Give at most 64 percepts a moment"));
            }
            value(serde_json::to_value(brain.live(cap(stream, 256), percepts)))
        }
        Request::End { stream } => Ok(json!(brain.end(stream))),
        Request::Close { stream } => Ok(json!(brain.close(stream))),
        Request::Learn {
            text,
            source,
            correction,
        } => {
            bounded(text, 8192, "a lesson")?;
            if source.len() > 256 {
                return Err(io::Error::other("Name the source in at most 256 bytes"));
            }
            let id = if *correction {
                brain.correct(text, source)
            } else {
                brain.learn(text, source)
            };
            Ok(json!({"engram": id, "progress": brain.progress()}))
        }
        Request::Say { prompt } => {
            if prompt.len() > 4096 {
                return Err(io::Error::other("Give a prompt of at most 4,096 bytes"));
            }
            let mut said = serde_json::to_value(brain.say(prompt))?;
            said["method"] = json!("neural_sequence");
            Ok(said)
        }
        Request::Study {
            text,
            source,
            max_passes,
        } => {
            bounded(text, 8192, "a lesson")?;
            bounded(source, 256, "a source")?;
            if !(1..=8).contains(max_passes) {
                return Err(io::Error::other("Use 1 to 8 study passes"));
            }
            value(serde_json::to_value(brain.study(text, source, *max_passes)))
        }
        Request::Converse { text } => {
            bounded(text, 4096, "something to say")?;
            let mut said = serde_json::to_value(brain.converse(text))?;
            said["method"] = json!(
                brain
                    .mind
                    .cognition
                    .traces
                    .back()
                    .map(|trace| &trace.method)
            );
            Ok(said)
        }
        Request::Chat { session, text } => {
            bounded(session, 128, "a session name")?;
            bounded(text, 4096, "something to say")?;
            let mut said = serde_json::to_value(brain.chat(session, text))?;
            said["method"] = json!(
                brain
                    .mind
                    .cognition
                    .traces
                    .back()
                    .map(|trace| &trace.method)
            );
            Ok(said)
        }
        Request::Recall { cue } => {
            bounded(cue, 4096, "a cue")?;
            value(serde_json::to_value(brain.recall(cue)))
        }
        Request::Snapshot => value(serde_json::to_value(brain.snapshot())),
        Request::Progress => Ok(brain.progress()),
        Request::Think => value(serde_json::to_value(brain.think())),
        Request::Imagine { seed } => value(serde_json::to_value(brain.imagine(cap(seed, 1024)))),
        Request::Dream => value(serde_json::to_value(brain.dream())),
        Request::Reason { from, to } => value(serde_json::to_value(
            brain.reason(cap(from, 1024), cap(to, 1024)),
        )),
        Request::Intuit { cue } => value(serde_json::to_value(brain.intuit(cap(cue, 1024)))),
        Request::Reflect => value(serde_json::to_value(brain.reflect())),
        Request::Wiring => value(serde_json::to_value(brain.wiring())),
        Request::Suggestion => Ok(json!(suggestion(brain))),
        Request::Hunch { stream } => value(serde_json::to_value(brain.advise(stream))),
        Request::Outlook { stream, most, goal } => value(serde_json::to_value(brain.outlook(
            stream,
            (*most).clamp(1, 16),
            goal.as_deref().map(|goal| cap(goal, 256)),
        ))),
        Request::Inspect { stream, query } => Ok(inspect(brain, stream, query)),
        Request::Sleep { milliseconds } => Ok(json!(sleep(
            brain,
            std::time::Duration::from_millis((*milliseconds).min(500))
        ))),
        Request::Flush | Request::Shutdown => Err(io::Error::other(
            "Only whoever holds the brain flushes it or shuts it down",
        )),
        #[cfg(unix)]
        Request::TeachStart { .. }
        | Request::TeachStop
        | Request::TeachStatus { .. }
        | Request::TrainStart { .. }
        | Request::TrainStop
        | Request::TrainStatus
        | Request::Status => Err(io::Error::other(
            "The teacher and training live in the background service, which is not running",
        )),
    }
}

/// A line of conversation, learned as dialogue: what the person says
/// after the person's mark, what the assistant says after the brain's.
pub fn hear(brain: &mut Brain, speaker: &str, text: &str) {
    let text = cap(text.trim(), HEARD);
    if text.is_empty() {
        return;
    }
    let mark = if speaker == "user" {
        crate::brain::words::USER
    } else {
        crate::brain::words::BRAIN
    };
    brain.learn(&format!("{mark} {text}"), speaker);
}

/// One event of an agent's run, lived in the stream of its session.
/// Each run is kept as a memory when it finishes, but the session's
/// stream goes on, so that the next instruction in it is expected from
/// how the last run went. What was said is heard as conversation.
pub fn lived(brain: &mut Brain, session: &str, event: &Event) {
    if matches!(event, Event::RunStarted { .. }) && !brain.is_living(session) {
        brain.begin(session, "agent session");
    }
    for percepts in senses::moments(event) {
        brain.live(session, &percepts);
    }
    heard(brain, event);
    if matches!(event, Event::RunFinished { .. }) {
        brain.close(session);
    }
}

/// What was said in an event, heard as conversation.
pub fn heard(brain: &mut Brain, event: &Event) {
    match event {
        Event::RunStarted { instruction, .. } => hear(brain, "user", instruction),
        Event::UserInterjected { text, .. } => hear(brain, "user", text),
        Event::ModelResponded { message, .. } => hear(brain, "assistant", &message.text()),
        _ => {}
    }
}

/// Whether an event is anything the brain lives or hears.
pub fn matters(event: &Event) -> bool {
    !senses::moments(event).is_empty()
        || matches!(
            event,
            Event::RunStarted { .. }
                | Event::RunFinished { .. }
                | Event::UserInterjected { .. }
                | Event::ModelResponded { .. }
        )
}

/// The short instruction it expects to be given next in the stream it
/// lived last, when it has seen it follow there before.
pub fn suggestion(brain: &mut Brain) -> Option<String> {
    let stream = brain.latest_stream()?;
    let (label, _) = brain.foresee(&stream, senses::ASKS, SUGGESTED)?;
    label.strip_prefix(senses::ASKS).map(str::to_string)
}

/// Sleep cycles until `limit` has passed. Returns how many it slept.
pub fn sleep(brain: &mut Brain, limit: std::time::Duration) -> u32 {
    let started = std::time::Instant::now();
    let mut cycles = 0;
    while started.elapsed() < limit && brain.memory_count() > 0 {
        brain.sleep();
        cycles += 1;
    }
    cycles
}

/// Answers the model's own questions about `stream`.
pub fn inspect(brain: &mut Brain, stream: &str, query: &Value) -> Value {
    if let Some(actions) = query["imagine"].as_array() {
        let imagined: Vec<Value> = actions
            .iter()
            .filter_map(|action| action.as_str())
            .take(8)
            .map(|action| {
                let percepts = senses::weighed(action);
                let imagined = brain.imagine_after(stream, &percepts, 4);
                json!({
                    "action": action,
                    "met_before": imagined.known,
                    "then": imagined.steps.iter().map(|(step, sure)| json!({
                        "expects": step,
                        "sure": round(*sure),
                    })).collect::<Vec<_>>(),
                    "worth": round(imagined.worth),
                })
            })
            .collect();
        return json!({
            "source": "brain",
            "imagined": imagined,
            "note": "worth above 0 means the steps it has seen follow ended in good news \
                     (checks passing, the person agreeing), below 0 in bad. An empty `then` \
                     means it has not seen what follows often enough to say.",
        });
    }
    if let Some(goal) = query["goal"].as_str() {
        return json!({
            "source": "brain",
            "goal": goal,
            "steps": brain.plan(stream, cap(goal, 256), 5),
            "note": "a way from here to this outcome, through what it expects, earliest \
                     first. Empty when it has not found one.",
        });
    }
    json!({
        "source": "brain",
        "expects": brain.expectations(stream, 5).into_iter().map(|(next, sure)| json!({
            "next": next,
            "sure": round(sure),
        })).collect::<Vec<_>>(),
        "in_mind": brain.mind.attention.tokens(),
    })
}

/// How it is doing, for the run's telemetry.
pub fn telemetry(brain: &Brain) -> Value {
    let mind = &brain.mind;
    json!({
        "source": "brain",
        "cells": brain.cortex.cells_alive(),
        "vocabulary": brain.words.len(),
        "memories": brain.memory_count(),
        "moments": mind.senses.moments,
        "surprise": round(mind.senses.surprise),
        "expected_right": round(mind.senses.expected_right),
        "advised": mind.advice.given,
        "advice_right": round(mind.advice.right),
        "held_back": mind.advice.held_back,
        "caution": round(mind.advice.caution),
        "in_mind": mind.attention.tokens(),
    })
}

/// A hunch as one line for the model. It says where it comes from and
/// that it is a habit seen before, not an instruction.
pub fn say(hunch: &crate::Hunch) -> String {
    let record = format!(
        "seen {} of {} times in this situation",
        hunch.hits,
        hunch.hits + hunch.misses
    );
    let plain = |label: &str| {
        let (_, what) = label.split_once(": ").unwrap_or(("", label));
        format!("`{what}`")
    };
    match hunch.kind {
        HunchKind::Next => {
            let mut text = format!(
                "Habit learned from earlier runs (a hint, not an instruction): from here, runs \
                 that went well usually went on with {}",
                plain(&hunch.label)
            );
            if !hunch.with.is_empty() {
                let with: Vec<String> = hunch.with.iter().map(|label| plain(label)).collect();
                text.push_str(&format!(" ({})", with.join(", ")));
            }
            format!("{text}; {record}.")
        }
        HunchKind::Warning => {
            let usual: Vec<String> = hunch.with.iter().map(|label| plain(label)).collect();
            format!(
                "Habit learned from earlier runs (a hint, not an instruction): the usual next \
                 step here ({}) has usually led to {}; {record}. Consider checking what differs \
                 this time before repeating it.",
                usual.join(", "),
                plain(&hunch.label)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunch_is_said_as_a_hint_with_its_record() {
        let hunch = crate::Hunch {
            kind: HunchKind::Next,
            label: "does: shell".to_string(),
            with: vec!["runs: cargo test".to_string()],
            because: Vec::new(),
            confidence: 0.9,
            hits: 7,
            misses: 1,
            imagined: Vec::new(),
            worth: 1.0,
        };
        assert_eq!(
            say(&hunch),
            "Habit learned from earlier runs (a hint, not an instruction): from here, runs that \
             went well usually went on with `shell` (`cargo test`); seen 7 of 8 times in this \
             situation."
        );
    }

    #[test]
    fn requests_travel_as_tagged_json() {
        let request = Request::Live {
            stream: "market".into(),
            percepts: vec!["move: up".into()],
        };
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"request\":\"live\""), "{text}");
        assert!(matches!(
            serde_json::from_str::<Request>(&text).unwrap(),
            Request::Live { .. }
        ));
    }
}
