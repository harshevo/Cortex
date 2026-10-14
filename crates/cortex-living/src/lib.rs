//! A network that grows and rewires itself as it watches the agent work,
//! and, when asked to, says what it is sure of.
//!
//! It is fed the same events as the rest of the agent. As a
//! [`TrajectorySink`] it only watches: [`Living`] can be as wrong as it
//! likes while it is being studied. Wrapped in a [`Counsel`] it is also
//! an [`Adviser`], and then a line of what it has learned reaches the
//! model between steps, when its own record says it is sure enough. What
//! it holds is kept in one file for all projects, and drawn live by the
//! app.
//!
//! - [`network`] is the network itself and says what each mechanism is.
//! - [`percept`] reads a run's events as the moments the network lives.

pub mod hybrid;
pub mod language;
pub mod network;
pub mod percept;
#[cfg(unix)]
pub mod service;
#[cfg(unix)]
pub mod teacher;
pub mod tutoring;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use cortex_core::{Advice, Adviser, Event, TrajectorySink};
use serde_json::json;

/// The standalone brain the background service runs and the teacher
/// teaches.
pub use cortex_neural as neural;

pub use hybrid::CellView;
pub use language::MemoryView;
pub use network::{
    Hunch, HunchKind, Id, Kind, Meters, Network, NeuronView, Reason, Sample, Snapshot, Thought,
};

/// How sure the network must be that a short instruction comes next
/// before it is offered as a suggestion.
const SUGGESTED: f32 = 0.6;

/// How long nothing must have happened before the network rests.
const QUIET: Duration = Duration::from_secs(3);

/// How often it dreams once there is nothing left to replay.
const DREAMING: Duration = Duration::from_secs(10);

pub struct Living {
    #[cfg(unix)]
    remote: Option<Arc<service::Client>>,
    /// Where the network is kept. `None` keeps it in memory only.
    file: Option<PathBuf>,

    state: Mutex<State>,
}

pub struct LearningSink {
    living: Arc<Living>,
    session: String,
}

impl TrajectorySink for LearningSink {
    fn record(&self, event: &Event) {
        self.living.record_in(&self.session, event);
    }
}

struct State {
    network: Network,
    last_event: Instant,

    /// Rests since the network was last saved.
    unsaved_rests: u32,

    last_dream: Instant,
}

impl Living {
    pub fn teach(
        &self,
        id: &str,
        session: &str,
        lesson: &tutoring::Lesson,
        source: &str,
    ) -> std::io::Result<()> {
        lesson.validate().map_err(std::io::Error::other)?;
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            remote.request(&service::Request::BrainLesson {
                id: id.into(),
                session: session.into(),
                lesson: lesson.clone(),
                source: source.into(),
            })?;
            return Ok(());
        }
        let mut state = self.state();
        state
            .network
            .teach(id, session, lesson, source)
            .map_err(std::io::Error::other)?;
        state.last_event = Instant::now();
        if let Some(file) = &self.file {
            store(file, &state.network)?;
        }
        Ok(())
    }

    pub fn answer(
        &self,
        session: &str,
        question: &str,
        learn_user: bool,
    ) -> std::io::Result<tutoring::Answer> {
        if question.trim().is_empty() || question.len() > 4096 {
            return Err(std::io::Error::other("Give a question of 1–4,096 bytes"));
        }
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return serde_json::from_value(remote.request(&service::Request::BrainAsk {
                session: session.into(),
                question: question.into(),
                learn_user,
            })?)
            .map_err(std::io::Error::from);
        }
        let mut state = self.state();
        let answer = state.network.answer(session, question, learn_user);
        if learn_user {
            state.last_event = Instant::now();
        }
        if let Some(file) = &self.file {
            store(file, &state.network)?;
        }
        Ok(answer)
    }

    pub fn grade(
        &self,
        attempt: &str,
        correct: bool,
        feedback: &str,
        correction: Option<&str>,
        source: &str,
    ) -> std::io::Result<tutoring::Evaluation> {
        if feedback.len() > 4096
            || correction.is_some_and(|text| text.len() > 2048)
            || (!correct && correction.is_none_or(|text| text.trim().is_empty()))
        {
            return Err(std::io::Error::other(
                "Wrong answers require a bounded corrective answer",
            ));
        }
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return serde_json::from_value(remote.request(&service::Request::BrainFeedback {
                attempt: attempt.into(),
                correct,
                feedback: feedback.into(),
                correction: correction.map(str::to_string),
                source: source.into(),
            })?)
            .map_err(std::io::Error::from);
        }
        let mut state = self.state();
        let evaluation = state
            .network
            .grade(attempt, correct, feedback, correction, source)
            .map_err(std::io::Error::other)?;
        state.last_event = Instant::now();
        if let Some(file) = &self.file {
            store(file, &state.network)?;
        }
        Ok(evaluation)
    }
    pub fn sink(self: &Arc<Self>) -> Arc<LearningSink> {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let session = format!(
            "{}-{now}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        Arc::new(LearningSink {
            living: self.clone(),
            session,
        })
    }
    /// A network of its own, loaded from `file` when there is one.
    pub fn open(file: Option<PathBuf>) -> Self {
        // A default network has not been given its own states yet.
        #[allow(clippy::unwrap_or_default)]
        let network = file.as_deref().and_then(load).unwrap_or_else(Network::new);

        Self {
            #[cfg(unix)]
            remote: None,
            file,
            state: Mutex::new(State {
                network,
                last_event: Instant::now(),
                unsaved_rests: 0,
                last_dream: Instant::now(),
            }),
        }
    }

    /// The one network kept in `file`, shared by everything in this
    /// process that asks for it, so that several chats feed and show the
    /// same one.
    pub fn shared(file: &Path) -> Arc<Self> {
        static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Living>>>> = OnceLock::new();

        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(living) = open.get(file).and_then(Weak::upgrade) {
            return living;
        }

        let living = Arc::new(Self::open(Some(file.to_path_buf())));

        open.insert(file.to_path_buf(), Arc::downgrade(&living));

        living
    }

    /// The network as it is now, for drawing.
    pub fn snapshot(&self) -> Snapshot {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.snapshot();
        }
        self.state().network.snapshot()
    }

    /// Changes whenever the network does: moments lived, moments
    /// replayed, and whether it is resting. For a caller that draws it
    /// many times a second and need not copy it when nothing happened.
    pub fn stamp(&self) -> (u64, u64, bool) {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.stamp();
        }
        let state = self.state();
        let meters = state.network.meters();

        (meters.moments, meters.replays, state.network.is_resting())
    }

    /// Lets the network go over recent moments, if nothing has happened
    /// for a while. Returns whether it did.
    pub fn rest(&self) -> bool {
        #[cfg(unix)]
        if self.remote.is_some() {
            return false;
        }
        let mut state = self.state();

        if state.last_event.elapsed() < QUIET {
            return false;
        }
        let replayed = state.network.rest();
        let text = state.network.replay_language();
        if !replayed && !text {
            return false;
        }

        state.unsaved_rests += 1;

        if state.unsaved_rests >= 10 {
            state.unsaved_rests = 0;

            self.save(&state.network);
        }

        true
    }

    /// Goes over recent moments until there is nothing left to go over or
    /// `limit` has passed, without waiting for a quiet spell first, and
    /// saves. For a caller that knows nothing is happening, such as an
    /// app the system has woken in the background. Returns how many times
    /// it replayed.
    pub fn sleep(&self, limit: std::time::Duration) -> u32 {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote
                .request(&service::Request::Consolidate {
                    milliseconds: limit.as_millis().min(500) as u64,
                })
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as u32;
        }
        let started = Instant::now();
        let mut state = self.state();
        let mut replays = 0;

        while started.elapsed() < limit {
            let replayed = state.network.rest();
            let text = state.network.replay_language();
            if !replayed && !text {
                break;
            }
            replays += 1;
        }

        if replays > 0 {
            state.unsaved_rests = 0;

            self.save(&state.network);
        }

        replays
    }

    /// The short instruction the network expects to be given next, when
    /// it has seen it follow what just happened more than once.
    pub fn suggestion(&self) -> Option<String> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote
                .request(&service::Request::Suggestion)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
        }
        let (label, _) = self.state().network.foresee(percept::ASKS, SUGGESTED)?;

        label.strip_prefix(percept::ASKS).map(str::to_string)
    }

    /// Something worth saying about the next step, by the network's own
    /// record. `None` is the usual answer.
    pub fn hunch(&self) -> Option<Hunch> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote
                .request(&service::Request::Hunch)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok());
        }
        self.state().network.advise()
    }

    /// The standalone brain as the background service last reported it.
    /// `None` without a service.
    pub fn neural(&self) -> Option<cortex_neural::NeuralSnapshot> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote.neural();
        }
        None
    }

    pub fn language(&self, query: &serde_json::Value) -> serde_json::Value {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            return remote
                .request(&service::Request::Language {
                    query: query.clone(),
                })
                .unwrap_or_else(|error| json!({"error":error.to_string()}));
        }
        self.state().network.language(query)
    }

    #[cfg(unix)]
    pub fn connect(file: &Path) -> std::io::Result<Arc<Self>> {
        static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Living>>>> = OnceLock::new();
        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(living) = open.get(file).and_then(Weak::upgrade) {
            return Ok(living);
        }
        let remote = service::Client::connect(file)?;
        let mut living = Self::open(None);
        living.remote = Some(remote);
        let living = Arc::new(living);
        open.insert(file.to_path_buf(), Arc::downgrade(&living));
        Ok(living)
    }

    fn learn_event(state: &mut State, session: &str, event: &Event) {
        if matches!(event, Event::RunStarted { .. }) {
            state.network.begin();
        }
        for percepts in percept::moments(event) {
            state.network.live(&percepts);
        }
        Self::learn_dialogue(state, session, event);
        state.last_event = Instant::now();
    }

    fn learn_dialogue(state: &mut State, session: &str, event: &Event) {
        match event {
            Event::RunStarted { instruction, .. } => {
                state.network.hear_in(session, "user", instruction)
            }
            Event::UserInterjected { text, .. } => state.network.hear_in(session, "user", text),
            Event::ModelResponded { message, .. } => {
                state.network.hear_in(session, "assistant", &message.text())
            }
            _ => {}
        }
    }

    #[cfg(unix)]
    pub(crate) fn deliver(&self, id: &str, session: &str, event: &Event) {
        let mut state = self.state();
        if !state.network.delivered(id) {
            if id.starts_with("history-") {
                Self::learn_dialogue(&mut state, session, event);
                state.last_event = Instant::now();
            } else {
                Self::learn_event(&mut state, session, event);
            }
            state.network.mark_delivered(id.to_string());
        }
    }

    pub fn checkpoint(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            remote.request(&service::Request::Flush)?;
            return Ok(());
        }
        if let Some(file) = &self.file {
            store(file, &self.state().network)?;
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn cycle(&self, pending: usize, problem: Option<String>) -> bool {
        let mut state = self.state();
        state.network.background(pending, problem);
        if state.last_event.elapsed() < QUIET {
            state.network.phase("observing new experience");
            return false;
        }
        let replayed = state.network.replay_language();
        let events = state.network.rest();
        if events {
            state.network.phase(if replayed {
                "replaying dialogue and event patterns"
            } else {
                "replaying event patterns"
            });
        }
        if events || replayed {
            return true;
        }
        if state.last_dream.elapsed() < DREAMING {
            return false;
        }
        state.last_dream = Instant::now();
        let phase = match state.network.dream() {
            Some(words) => format!("dreaming: rehearsing \u{201c}{words}\u{201d}"),
            None => "dreaming: nothing to rehearse yet".to_string(),
        };
        state.network.phase(&phase);
        true
    }

    /// Lets the network hear a line of conversation that did not come
    /// through an agent run, such as the trading chat. `speaker` is
    /// `user` or `assistant`.
    pub fn hear(&self, session: &str, speaker: &str, text: &str) {
        if text.trim().is_empty() || text.len() > 64 * 1024 {
            return;
        }
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            if let Err(error) = remote.request(&service::Request::Hear {
                session: session.into(),
                speaker: speaker.into(),
                text: text.into(),
            }) {
                eprintln!("warning: the network did not hear the conversation: {error}");
            }
            return;
        }
        let mut state = self.state();
        state.network.hear_in(session, speaker, text);
        state.last_event = Instant::now();
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, network: &Network) {
        let Some(file) = &self.file else {
            return;
        };

        if let Err(error) = store(file, network) {
            eprintln!("warning: cannot save {}: {error}", file.display());
        }
    }
}

impl Living {
    fn record_in(&self, session: &str, event: &Event) {
        #[cfg(unix)]
        if let Some(remote) = &self.remote {
            if !percept::moments(event).is_empty() {
                remote.record(session, event);
            }
            return;
        }
        let moments = percept::moments(event);

        if moments.is_empty() {
            return;
        }

        let mut state = self.state();

        Self::learn_event(&mut state, session, event);

        if matches!(event, Event::RunFinished { .. }) {
            self.save(&state.network);
        }
    }
}

impl TrajectorySink for Living {
    fn record(&self, event: &Event) {
        self.record_in("default", event);
    }
}

/// The network as an adviser: the same one, allowed to speak.
pub struct Counsel(pub Arc<Living>);

impl Adviser for Counsel {
    fn advise(&self, _step: u32, _max_steps: u32) -> Option<Advice> {
        let hunch = self.0.hunch()?;

        Some(Advice {
            source: "dendritic network".to_string(),
            text: say(&hunch),
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
    /// have gone on from here, or how an outcome has been reached.
    fn inspect(&self, query: &serde_json::Value) -> Option<serde_json::Value> {
        #[cfg(unix)]
        if let Some(remote) = &self.0.remote {
            return remote
                .request(&service::Request::Inspect {
                    query: query.clone(),
                })
                .ok();
        }
        if let Some(language) = query.get("language") {
            return Some(
                json!({"source":"spiking dendritic language learner","language":self.0.language(language)}),
            );
        }
        let state = self.0.state();
        let network = &state.network;

        if let Some(actions) = query["imagine"].as_array() {
            let imagined: Vec<serde_json::Value> = actions
                .iter()
                .filter_map(|action| action.as_str())
                .map(|action| {
                    let percepts = percept::weighed(action);
                    let known = percepts.iter().all(|percept| network.knows(percept));
                    let thoughts = network.imagine_after(&percepts, 4);

                    json!({
                        "action": action,
                        "met_before": known,
                        "then": thoughts.iter().map(|thought| json!({
                            "expects": thought.labels,
                            "sure": round(thought.confidence),
                        })).collect::<Vec<_>>(),
                        "worth": round(thoughts.iter().map(|thought| thought.worth).sum()),
                    })
                })
                .collect();

            return Some(json!({
                "source": "dendritic network",
                "imagined": imagined,
                "note": "worth above 0 means the steps it has seen follow ended in good news \
                         (checks passing, the person agreeing), below 0 in bad. An empty `then` \
                         means it has not seen what follows often enough to say.",
            }));
        }

        if let Some(goal) = query["goal"].as_str() {
            return Some(json!({
                "source": "dendritic network",
                "goal": goal,
                "steps": network.plan(goal, 5),
                "note": "the steps that have preceded this outcome before, earliest first. \
                         Empty when it has not seen the outcome reached.",
            }));
        }

        let snapshot = network.snapshot();

        Some(json!({
            "source": "dendritic network",
            "expects": snapshot.reasons.iter().map(|reason| json!({
                "next": reason.expected,
                "because": reason.because,
                "sure": round(reason.confidence),
                "right": reason.hits,
                "wrong": reason.misses,
            })).collect::<Vec<_>>(),
            "imagines": snapshot.imagined.iter().map(|thought| &thought.labels).collect::<Vec<_>>(),
            "in_mind": snapshot.workspace,
        }))
    }

    fn telemetry(&self) -> Option<serde_json::Value> {
        let snapshot = self.0.snapshot();
        let meters = &snapshot.meters;

        Some(json!({
            "source": "dendritic network",
            "neurons": meters.neurons,
            "branches": meters.segments,
            "synapses": meters.synapses,
            "surprise": meters.surprise,
            "expected_right": meters.expected_right,
            "baseline_right": meters.baseline_right,
            "advised": meters.advised,
            "advice_right": meters.advice_right,
            "held_back": meters.held_back,
            "plasticity": meters.plasticity,
            "caution": meters.caution,
            "expecting": snapshot.expecting,
        }))
    }
}

fn round(number: f32) -> f64 {
    (f64::from(number) * 100.0).round() / 100.0
}

/// A hunch as one line for the model. It says where it comes from and
/// that it is a habit seen before, not an instruction.
pub fn say(hunch: &Hunch) -> String {
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

fn load(file: &Path) -> Option<Network> {
    let text = fs::read_to_string(file).ok()?;

    let mut network: Network = serde_json::from_str(&text)
        .map_err(|error| {
            eprintln!(
                "warning: {} is not a network, starting a new one: {error}",
                file.display()
            );
        })
        .ok()?;

    network.wake();

    Some(network)
}

/// Written beside the file and moved over it, so that a crash half-way
/// leaves the old network and not half of a new one.
fn store(file: &Path, network: &Network) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }

    use std::io::Write;
    let folder = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut partial = tempfile::NamedTempFile::new_in(folder)?;
    serde_json::to_writer(&mut partial, network)?;
    partial.flush()?;
    partial.as_file().sync_all()?;
    partial.persist(file).map_err(|error| error.error)?;
    Ok(())
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
    fn a_run_grows_the_network_and_it_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("living").join("network.json");

        let living = Living::open(Some(file.clone()));

        assert_eq!(living.snapshot().meters.grown, 0);

        living.record(&started("fix the rounding of commission fees"));
        living.record(&finished());

        let lived = living.snapshot();

        assert!(lived.meters.grown >= 4, "{:?}", lived.meters);
        assert!(lived.synapses.iter().any(|(_, _, weight)| *weight > 0.0));

        let again = Living::open(Some(file)).snapshot();

        assert_eq!(again.moment, lived.moment);
        assert_eq!(again.neurons.len(), lived.neurons.len());
    }

    #[test]
    fn everything_in_one_process_shares_the_network_of_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");

        let one = Living::shared(&file);
        let other = Living::shared(&file);

        one.record(&started("fix the rounding"));

        assert_eq!(other.snapshot().moment, one.snapshot().moment);
        assert_eq!(other.snapshot().language.messages, 1);
    }

    #[test]
    fn a_short_instruction_given_again_and_again_is_seen_coming() {
        let living = Living::open(None);

        for _ in 0..6 {
            living.record(&started("fix the rounding of commission fees"));
            living.record(&finished());
            living.record(&started("run the tests"));
            living.record(&finished());
        }

        living.record(&started("fix the rounding of commission fees"));

        assert_eq!(living.suggestion(), None);

        living.record(&finished());

        assert_eq!(living.suggestion().as_deref(), Some("run the tests"));
    }

    #[test]
    fn a_hunch_is_said_as_a_hint_with_its_record() {
        let hunch = Hunch {
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
    fn it_does_not_rest_while_things_are_happening() {
        let living = Living::open(None);

        living.record(&started("fix the rounding"));

        assert!(!living.rest());
    }
}
