//! The handle everything else holds on the one brain.
//!
//! A [`Link`] either holds the brain itself, in this process, or reaches
//! it in the background service, where it lives between uses and sleeps
//! when left alone. Either way it is the same brain for every chat, agent
//! run and market: each is a stream of experience it lives, keeping its
//! place in each.
//!
//! As a [`TrajectorySink`] it lives an agent's run as it happens. Wrapped
//! in a [`Counsel`] it is also an [`Adviser`]: when its own record says it
//! is usually right, a line of what it expects reaches the model between
//! steps, and the model can ask it how actions it is weighing have gone
//! on before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use cortex_core::{Advice, Adviser, Event, TrajectorySink};
use serde_json::{Value, json};

use crate::brain::{Brain, Hunch, HunchKind, Moment, NeuralSnapshot};
use crate::protocol::{self, Request};
use crate::senses;

/// How long nothing must have happened before its mind runs on its own.
const QUIET: Duration = Duration::from_secs(3);

/// Rests between saves.
const SAVED_EVERY: u32 = 20;

pub struct Link {
    #[cfg(unix)]
    remote: Option<Arc<crate::service::Client>>,

    /// Where the brain is kept. `None` keeps it in memory only.
    file: Option<PathBuf>,

    state: Mutex<State>,
}

struct State {
    brain: Brain,
    last_event: Instant,
    unsaved_rests: u32,
}

/// One session's way into the brain: what it records is lived in a
/// stream of its own.
pub struct LearningSink {
    link: Arc<Link>,
    stream: String,
}

impl LearningSink {
    /// The stream this session's experience is lived in.
    pub fn stream(&self) -> &str {
        &self.stream
    }
}

impl TrajectorySink for LearningSink {
    fn record(&self, event: &Event) {
        self.link.record_in(&self.stream, event);
    }
}

impl Link {
    /// A brain of its own in this process, loaded from `file` when there
    /// is one there.
    pub fn open(file: Option<PathBuf>) -> Self {
        let mut brain = file
            .as_deref()
            .filter(|file| file.exists())
            .and_then(|file| match Brain::load(file) {
                Ok(brain) => Some(brain),
                Err(error) => {
                    eprintln!(
                        "warning: {} is not a brain in working order, starting a new one: {error}",
                        file.display()
                    );
                    None
                }
            })
            .unwrap_or_default();
        brain.mean(&senses::WORTH);
        Self {
            #[cfg(unix)]
            remote: None,
            file,
            state: Mutex::new(State {
                brain,
                last_event: Instant::now(),
                unsaved_rests: 0,
            }),
        }
    }

    /// The one brain kept in `file`, shared by everything in this process
    /// that asks for it.
    pub fn shared(file: &Path) -> Arc<Self> {
        static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Link>>>> = OnceLock::new();
        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(link) = open.get(file).and_then(Weak::upgrade) {
            return link;
        }
        let link = Arc::new(Self::open(Some(file.to_path_buf())));
        open.insert(file.to_path_buf(), Arc::downgrade(&link));
        link
    }

    /// The brain in the background service that keeps `file`, started if
    /// it is not running.
    #[cfg(unix)]
    pub fn connect(file: &Path) -> std::io::Result<Arc<Self>> {
        static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Link>>>> = OnceLock::new();
        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(link) = open.get(file).and_then(Weak::upgrade) {
            return Ok(link);
        }
        let remote = crate::service::Client::connect(file)?;
        let link = Arc::new(Self {
            remote: Some(remote),
            file: None,
            state: Mutex::new(State {
                brain: Brain::default(),
                last_event: Instant::now(),
                unsaved_rests: 0,
            }),
        });
        open.insert(file.to_path_buf(), Arc::downgrade(&link));
        Ok(link)
    }

    /// Whether the brain lives in the background service.
    pub fn is_remote(&self) -> bool {
        #[cfg(unix)]
        if self.remote.is_some() {
            return true;
        }
        false
    }

    /// A way in for one session, which lives its experience in a stream
    /// of its own.
    pub fn sink(self: &Arc<Self>) -> Arc<LearningSink> {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let stream = format!(
            "session-{}-{now}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        Arc::new(LearningSink {
            link: self.clone(),
            stream,
        })
    }

    /// Asks the brain something, wherever it lives.
    pub fn ask(&self, request: &Request) -> std::io::Result<Value> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.request(request);
        }
        match request {
            Request::Flush | Request::Shutdown => {
                self.checkpoint()?;
                return Ok(json!(true));
            }
            _ => {}
        }
        let mut state = self.state();
        let answer = protocol::respond(&mut state.brain, request);
        if protocol::changes(request) {
            state.last_event = Instant::now();
        }
        answer
    }

    /// The brain as it is now, for drawing. From the service, as last
    /// fetched.
    pub fn snapshot(&self) -> NeuralSnapshot {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.snapshot();
        }
        self.state().brain.snapshot()
    }

    /// Changes whenever the brain does: steps lived, thoughts had and
    /// memories replayed, and whether it is asleep. For a caller that
    /// draws it many times a second and need not copy it when nothing
    /// happened.
    pub fn stamp(&self) -> (u64, u64, bool) {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.stamp();
        }
        let state = self.state();
        let brain = &state.brain;
        (
            brain.cortex.clock,
            brain.mind.numbered + brain.stats().replays,
            brain.mind.felt.asleep,
        )
    }

    /// What went wrong reaching the background service, if anything.
    pub fn problem(&self) -> Option<String> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.problem();
        }
        None
    }

    /// What the LLM teacher is doing, as the service last said. Null for
    /// a brain in this process, which has no teacher.
    pub fn teaching(&self) -> Value {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.status()["teaching"].clone();
        }
        Value::Null
    }

    /// Lets its mind run on its own, if nothing has happened for a while:
    /// a thought, or a cycle of sleep once it is drowsy. Returns whether
    /// it did. The service does this itself.
    pub fn rest(&self) -> bool {
        if self.is_remote() {
            return false;
        }
        let mut state = self.state();
        if state.last_event.elapsed() < QUIET {
            return false;
        }
        state.brain.idle();
        state.unsaved_rests += 1;
        if state.unsaved_rests >= SAVED_EVERY {
            state.unsaved_rests = 0;
            self.save(&state.brain);
        }
        true
    }

    /// Sleeps until `limit` has passed, without waiting for a quiet spell
    /// first, and saves. For a caller that knows nothing is happening,
    /// such as an app the system has woken in the background. Returns
    /// how many cycles it slept.
    pub fn sleep(&self, limit: Duration) -> u32 {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote
                .request(&Request::Sleep {
                    milliseconds: limit.as_millis().min(500) as u64,
                })
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as u32;
        }
        let mut state = self.state();
        let cycles = protocol::sleep(&mut state.brain, limit);
        if cycles > 0 {
            state.unsaved_rests = 0;
            self.save(&state.brain);
        }
        cycles
    }

    /// The short instruction it expects to be given next, when it has
    /// seen it follow what just happened before.
    pub fn suggestion(&self) -> Option<String> {
        self.ask(&Request::Suggestion)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
    }

    /// Something worth saying about the next step in `stream`, by the
    /// brain's own record. `None` is the usual answer.
    pub fn hunch(&self, stream: &str) -> Option<Hunch> {
        self.ask(&Request::Hunch {
            stream: stream.into(),
        })
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
    }

    /// Hears a line of conversation that did not come through an agent's
    /// run, such as the trading chat. `speaker` is `user` or `assistant`.
    pub fn hear(&self, session: &str, speaker: &str, text: &str) {
        if text.trim().is_empty() || text.len() > 64 * 1024 {
            return;
        }
        if let Err(error) = self.ask(&Request::Hear {
            session: session.into(),
            speaker: speaker.into(),
            text: text.into(),
        }) {
            eprintln!("warning: the brain did not hear the conversation: {error}");
        }
    }

    /// A stretch of `stream` begins: a market opened, a chart loaded.
    pub fn begin(&self, stream: &str, source: &str) {
        let _ = self.ask(&Request::Begin {
            stream: stream.into(),
            source: source.into(),
        });
    }

    /// One moment of `stream`, as percepts (see [`senses`]): what it had
    /// not expected of it, and any news in it.
    pub fn perceive(&self, stream: &str, percepts: Vec<String>) -> Moment {
        self.ask(&Request::Live {
            stream: stream.into(),
            percepts,
        })
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
    }

    /// What it expects next in `stream`, the way on it imagines, and a
    /// way to `goal`.
    pub fn outlook(&self, stream: &str, most: usize, goal: Option<&str>) -> crate::brain::Outlook {
        self.ask(&Request::Outlook {
            stream: stream.into(),
            most,
            goal: goal.map(str::to_string),
        })
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
    }

    /// The stretch of `stream` is over and is kept as a memory.
    pub fn end(&self, stream: &str) {
        let _ = self.ask(&Request::End {
            stream: stream.into(),
        });
    }

    /// Saves the brain now.
    pub fn checkpoint(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            remote.request(&Request::Flush)?;
            return Ok(());
        }
        if let Some(file) = &self.file {
            self.state().brain.save(file)?;
        }
        Ok(())
    }

    fn record_in(&self, stream: &str, event: &Event) {
        if !protocol::matters(event) {
            return;
        }
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            remote.record(stream, event);
            return;
        }
        let mut state = self.state();
        protocol::lived(&mut state.brain, stream, event);
        state.last_event = Instant::now();
        if matches!(event, Event::RunFinished { .. }) {
            self.save(&state.brain);
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, brain: &Brain) {
        let Some(file) = &self.file else {
            return;
        };
        if let Err(error) = brain.save(file) {
            eprintln!("warning: cannot save {}: {error}", file.display());
        }
    }
}

impl TrajectorySink for Link {
    fn record(&self, event: &Event) {
        self.record_in("default", event);
    }
}

/// The brain as an adviser to one session: the same brain, allowed to
/// speak about the stream that session lives in.
pub struct Counsel {
    link: Arc<Link>,
    stream: String,
}

impl Counsel {
    pub fn new(link: Arc<Link>, stream: &str) -> Self {
        Self {
            link,
            stream: stream.into(),
        }
    }

    /// Advises the session that `sink` records.
    pub fn of(sink: &LearningSink) -> Self {
        Self::new(sink.link.clone(), &sink.stream)
    }
}

impl Adviser for Counsel {
    fn advise(&self, _step: u32, _max_steps: u32) -> Option<Advice> {
        let hunch = self.link.hunch(&self.stream)?;
        Some(Advice {
            source: "brain".to_string(),
            text: protocol::say(&hunch),
            detail: json!({
                "kind": match hunch.kind {
                    HunchKind::Next => "next",
                    HunchKind::Warning => "warning",
                },
                "label": hunch.label,
                "with": hunch.with,
                "because": hunch.because,
                "confidence": hunch.confidence,
                "hits": hunch.hits,
                "misses": hunch.misses,
                "imagined": hunch.imagined,
                "worth": hunch.worth,
            }),
        })
    }

    /// Answers the model's own questions: how actions it is weighing
    /// have gone on from here, or how an outcome is reached.
    fn inspect(&self, query: &Value) -> Option<Value> {
        self.link
            .ask(&Request::Inspect {
                stream: self.stream.clone(),
                query: query.clone(),
            })
            .ok()
    }

    fn telemetry(&self) -> Option<Value> {
        #[cfg(unix)]
        if let Some(remote) = &self.link.remote {
            let snapshot = remote.snapshot();
            let mind = &snapshot.mind;
            return Some(json!({
                "source": "brain",
                "cells": snapshot.cells,
                "vocabulary": snapshot.vocabulary,
                "memories": snapshot.engram_count,
                "moments": mind.senses.moments,
                "surprise": mind.senses.surprise,
                "expected_right": mind.senses.expected_right,
                "advised": mind.advice.given,
                "advice_right": mind.advice.right,
                "held_back": mind.advice.held_back,
                "caution": mind.advice.caution,
                "in_mind": mind.attention.iter().map(|(t, _)| t).collect::<Vec<_>>(),
            }));
        }
        Some(protocol::telemetry(&self.link.state().brain))
    }
}

#[cfg(test)]
mod tests {
    use cortex_core::{Outcome, Usage};

    use super::*;

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        }
    }

    fn finished() -> Event {
        Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1,
            verified: Some(true),
        }
    }

    #[test]
    fn a_run_is_lived_kept_as_a_memory_and_saved() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain").join("brain.bin");
        let link = Link::open(Some(file.clone()));
        assert_eq!(link.snapshot().engram_count, 0);
        link.record(&started("fix the rounding of commission fees"));
        link.record(&finished());
        let lived = link.snapshot();
        assert!(lived.engram_count >= 1, "{}", lived.engram_count);
        assert!(lived.vocabulary >= 4);
        let again = Link::open(Some(file)).snapshot();
        assert_eq!(again.clock, lived.clock);
        assert_eq!(again.engram_count, lived.engram_count);
    }

    #[test]
    fn everything_in_one_process_shares_the_brain_of_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let one = Link::shared(&file);
        let other = Link::shared(&file);
        one.record(&started("fix the rounding"));
        assert_eq!(other.snapshot().clock, one.snapshot().clock);
        assert!(other.snapshot().clock > 0);
    }

    #[test]
    fn chat_and_markets_reach_the_same_brain() {
        let link = Link::open(None);
        link.hear("chat", "user", "how is the market today");
        link.begin("market", "BTC");
        let moment = link.perceive("market", senses::bar(0.02, 0.01, &[("cfc", 0.01)]));
        assert!(moment.surprise > 0.0);
        let snapshot = link.snapshot();
        assert!(snapshot.mind.streams.contains(&"market".to_string()));
        assert!(link.outlook("nowhere", 3, None).expected.is_empty());
        assert!(snapshot.vocabulary > 5);
    }

    #[test]
    fn a_short_instruction_given_again_and_again_is_seen_coming() {
        let link = Link::open(None);
        // The same two instructions, one after the other, a few times: a
        // step with no rival costs it no sureness, so it needs only a
        // handful of repetitions.
        for _ in 0..6 {
            link.record(&started("fix the rounding of commission fees"));
            link.record(&finished());
            link.record(&started("run the tests"));
            link.record(&finished());
        }
        link.record(&started("fix the rounding of commission fees"));
        link.record(&finished());
        assert_eq!(link.suggestion().as_deref(), Some("run the tests"));
    }

    #[test]
    fn a_choice_it_has_seen_go_both_ways_is_not_suggested() {
        let link = Link::open(None);
        for cycle in 0..20 {
            link.record(&started("fix the rounding of commission fees"));
            link.record(&finished());
            let next = if cycle % 2 == 0 {
                "run the tests"
            } else {
                "deploy the build"
            };
            link.record(&started(next));
            link.record(&finished());
        }
        link.record(&started("fix the rounding of commission fees"));
        link.record(&finished());
        assert_eq!(link.suggestion(), None);
    }

    #[test]
    fn it_does_not_rest_while_things_are_happening() {
        let link = Link::open(None);
        link.record(&started("fix the rounding"));
        assert!(!link.rest());
    }
}
