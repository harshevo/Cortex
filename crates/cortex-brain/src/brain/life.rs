//! Living in the world: the same cortex that learns words learns what
//! happens.
//!
//! The agent's work, a market's bars and anything else that can be said
//! as short percepts (`does: edit_file`, `checks: passed`, `move: up`)
//! reaches the brain as a stream of moments. Each percept fires its own
//! columns, as a word does, and the cortex learns what follows what, so
//! that before each moment the cells for what usually comes next are
//! primed. Several streams can be lived at once (two chats, a market):
//! each keeps where the cortex was in it, and is put back before its next
//! moment, as attention returns to a task.
//!
//! On top of that:
//!
//! - **Value.** Percepts that are news by nature give their worth to
//!   what led to them ([`super::value`]).
//! - **Attention.** The new, the surprising and what matters are held in
//!   mind for a while.
//! - **Episodes.** Each stretch of a stream is kept as a memory with the
//!   cells that held it, and sleep replays it like any other.
//! - **Advice.** Before committing, the brain imagines each thing it
//!   expects might be done next, follows each a few steps on its own
//!   expectations, and names the one whose way on is worth most, but
//!   only where its record says it is usually right. If the usual way on
//!   leads to bad news it says that instead.
//!
//! All of it is the one cortex run differently; nothing here is a second
//! network.

use super::*;

/// The beginnings of percepts that are things done: advice is about which
/// of these to do next, and they are rivals of each other.
pub const DOING: [&str; 1] = ["does: "];

/// Percepts that go with a thing done: what it is done to.
const WITH: [&str; 2] = ["file: ", "runs: "];

/// Percepts kept in one memory of a stream before it is stored and a new
/// one begins.
const EPISODE: usize = 240;

/// Times a thing must have been expected first before its record counts.
const MIN_RECORD: u32 = 4;

/// Steps imagined ahead, how much less each later one weighs, and the
/// sureness below which an imagined path is dropped.
const IMAGINED: usize = 6;
const DISCOUNT: f32 = 0.85;
const FAINT: f32 = 0.15;

/// By how much the best imagined way on must be worth more than the next
/// for it to be named.
const MARGIN: f32 = 0.15;

/// Streams kept at once; the longest unused is closed.
const STREAMS: usize = 16;

/// Ways on searched at each depth when planning, and tried from each.
const PLAN_BEAM: usize = 4;
const PLAN_BRANCHING: usize = 4;

/// Steps followed on to foresee something.
const FORESEEN: usize = 7;

/// The share of its columns primed for the goal to count as reached.
const REACHED: f32 = 0.5;

/// One stretch of experience being lived: an agent's run, a chat, a
/// market.
#[derive(Debug, Clone, Default)]
pub struct Stream {
    state: cortex::State,
    source: String,
    tokens: Vec<String>,
    cells: Vec<Vec<u32>>,
    surprise: f32,
    news: f32,
    advised: Option<String>,
    said: Option<(HunchKind, String)>,
    used: u64,
    trace: value::Trace,
}

impl Stream {
    /// What was lived since the last memory was kept, taken to be kept;
    /// the stream keeps its place.
    fn lived_so_far(&mut self) -> Stream {
        Stream {
            source: self.source.clone(),
            tokens: std::mem::take(&mut self.tokens),
            cells: std::mem::take(&mut self.cells),
            surprise: std::mem::take(&mut self.surprise),
            news: std::mem::take(&mut self.news),
            ..Stream::default()
        }
    }

    pub(crate) fn forget_cells(&mut self, died: &HashSet<u32>) {
        for cells in &mut self.cells {
            cells.retain(|cell| !died.contains(cell));
        }
        self.state = cortex::State::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HunchKind {
    /// What it would do next.
    Next,

    /// The usual way on has usually gone badly.
    Warning,
}

/// Something the brain is sure enough of to say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hunch {
    pub kind: HunchKind,

    /// The percept it is about.
    pub label: String,

    /// What goes with it: the file, the command.
    pub with: Vec<String>,

    /// What it was attending to when it formed the hunch.
    pub because: Vec<String>,
    pub confidence: f32,

    /// Times the thing it expects first here came, and did not.
    pub hits: u32,
    pub misses: u32,

    /// The way on it imagined.
    pub imagined: Vec<String>,

    /// What that way on is worth, from -1 to 1 a step.
    pub worth: f32,
}

/// What one moment was to the brain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    /// The share of the moment it had not expected.
    pub surprise: f32,

    /// Good or bad news in it, from -1 to 1.
    pub news: f32,

    /// Whether the thing it advised came, when it had advised.
    pub advice_right: Option<bool>,
}

/// What the brain makes of where a stream is now: what it expects next,
/// the way on it imagines, and a way to a goal if one was asked for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Outlook {
    pub expected: Vec<(String, f32)>,
    pub imagined: Vec<(String, f32)>,
    pub plan: Vec<String>,
}

/// A way on imagined from a stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Imagined {
    /// Each step and how sure the path up to it is.
    pub steps: Vec<(String, f32)>,
    pub worth: f32,

    /// Every percept given had been met before.
    pub known: bool,
}

impl Brain {
    /// Gives these percepts their worth as news, from -1 to 1.
    pub fn mean(&mut self, meanings: &[(&str, f32)]) {
        self.mind.values.mean(meanings);
    }

    /// A new stretch of `stream` begins, from `source`: whatever was open
    /// in it is kept as a memory, and the cortex starts afresh there.
    pub fn begin(&mut self, stream: &str, source: &str) {
        self.end(stream);
        self.open_stream(stream, source);
    }

    /// The stretch of `stream` is over: it is kept as a memory. Returns the
    /// memory's id.
    pub fn end(&mut self, stream: &str) -> Option<u64> {
        let open = self.streams.remove(stream)?;
        self.keep_episode(open)
    }

    /// What `stream` has lived so far is kept as a memory, but the stream
    /// goes on from where it is, so that what comes next is expected from
    /// there: a chat's next instruction after a run of it ended. Returns
    /// the memory's id.
    pub fn close(&mut self, stream: &str) -> Option<u64> {
        let lived = self.streams.get_mut(stream)?.lived_so_far();
        self.keep_episode(lived)
    }

    /// Whether `stream` is being lived.
    pub fn is_living(&self, stream: &str) -> bool {
        self.streams.contains_key(stream)
    }

    fn open_stream(&mut self, stream: &str, source: &str) -> &mut Stream {
        if !self.streams.contains_key(stream) && self.streams.len() >= STREAMS {
            let oldest = self
                .streams
                .iter()
                .min_by_key(|(_, open)| open.used)
                .map(|(name, _)| name.clone());
            if let Some(oldest) = oldest {
                self.end(&oldest);
            }
        }
        let clock = self.cortex.clock;
        let open = self
            .streams
            .entry(stream.to_string())
            .or_insert_with(|| Stream {
                source: source.chars().take(80).collect(),
                ..Stream::default()
            });
        open.used = clock;
        open
    }

    /// Puts the cortex where it was in `stream`.
    pub(crate) fn enter(&mut self, stream: &str) -> bool {
        let Some(open) = self.streams.get(stream) else {
            self.cortex.reset();
            return false;
        };
        let state = open.state.clone();
        self.cortex.restore(&state);
        true
    }

    /// One moment of `stream`: its percepts are fired in order and
    /// learned, each first held against what the brain expected.
    pub fn live(&mut self, stream: &str, percepts: &[String]) -> Moment {
        let percepts: Vec<String> = percepts
            .iter()
            .map(|p| p.trim().chars().take(96).collect::<String>())
            .filter(|p| !p.is_empty())
            .collect();
        if percepts.is_empty() {
            return Moment::default();
        }
        if !self.streams.contains_key(stream) {
            self.open_stream(stream, stream);
        }
        self.enter(stream);
        let mut moment = Moment::default();
        let mut surprised = 0.0;
        let mut expected_right = 0;
        for (at, percept) in percepts.iter().enumerate() {
            // Anything lived in the stream before, even in a stretch
            // already kept as a memory, is context to expect from.
            let fresh = at > 0
                || self
                    .streams
                    .get(stream)
                    .is_some_and(|open| !open.tokens.is_empty() || !open.trace.is_empty());
            if fresh {
                let guesses = self.guesses(5);
                if let Some(top) = guesses.first() {
                    self.mind.me.guessed(top.1, top.0 == *percept);
                    // Every thing it expected at least half-surely keeps
                    // a record of whether it came: rivals expected as
                    // strongly as each other each have their own.
                    for (token, _, _) in guesses.iter().filter(|g| g.1 >= 0.5) {
                        let record = self.mind.expected.entry(token.clone()).or_default();
                        if *token == *percept || percepts.contains(token) {
                            record.0 += 1;
                        } else {
                            record.1 += 1;
                        }
                    }
                    expected_right +=
                        usize::from(guesses.iter().any(|g| g.0 == *percept && g.1 >= 0.5));
                }
            }
            let Some(columns) = self.code(percept) else {
                continue;
            };
            let new = self.words.words.get(percept).is_none_or(|w| w.seen == 0);
            let activation = self.fire("live", percept, &columns, true);
            let surprise = activation.bursting as f32 / columns.len().max(1) as f32;
            surprised += surprise;
            self.words.met(percept, &[], surprise);
            let worth = self.mind.values.worth(percept).abs();
            self.mind.attention.met(
                percept,
                0.2 + surprise + 0.4 * worth + if new { 1.0 } else { 0.0 },
            );
            if let Some(open) = self.streams.get_mut(stream) {
                open.tokens.push(percept.clone());
                open.cells.push(activation.winners);
            }
        }
        moment.surprise = surprised / percepts.len() as f32;
        let plasticity = self.cortex.plasticity;
        if let Some(open) = self.streams.get_mut(stream) {
            moment.news = self
                .mind
                .values
                .lived(&mut open.trace, &percepts, plasticity);
        }
        let senses = &mut self.mind.senses;
        senses.moments += 1;
        senses.surprise += 0.05 * (moment.surprise - senses.surprise);
        senses.expected_right +=
            0.05 * (expected_right as f32 / percepts.len() as f32 - senses.expected_right);
        let state = self.cortex.state();
        let clock = self.cortex.clock;
        let mut full = None;
        if let Some(open) = self.streams.get_mut(stream) {
            if let Some(advised) = open.advised.take() {
                let right = percepts.contains(&advised);
                moment.advice_right = Some(right);
                self.mind.advice.judged(right);
            }
            open.state = state;
            open.used = clock;
            open.surprise += moment.surprise;
            open.news += moment.news;
            if open.tokens.len() >= EPISODE {
                full = Some(open.lived_so_far());
            }
        }
        if let Some(full) = full {
            self.keep_episode(full);
        }
        self.phase = format!("living {stream}");
        moment
    }

    /// A stretch of a stream kept as a memory, with when and where it was
    /// and how surprising and good or bad it was.
    fn keep_episode(&mut self, open: Stream) -> Option<u64> {
        if open.tokens.len() < 2 {
            return None;
        }
        self.next_engram += 1;
        let id = self.next_engram;
        let moments = open.tokens.len().max(1) as f32;
        self.mind.notes.insert(
            id,
            Note {
                surprise: open.surprise / moments,
                worth: open.news.clamp(-1.0, 1.0),
                context: open.source.clone(),
                time_ms: now_ms(),
            },
        );
        self.engrams.push_back(Engram {
            id,
            source: open.source,
            tokens: open.tokens,
            cells: open.cells,
            created: self.cortex.clock,
            strength: 1.0,
            recalls: 0,
            replays: 0,
            last_recalled: 0,
            fidelity: None,
        });
        self.grow_area();
        Some(id)
    }

    /// How far to trust the brain when it expects `token`: how often it
    /// came when it was expected at least half-surely, counted cautiously.
    fn trust(&self, token: &str) -> (f32, u32, u32) {
        let (hits, misses) = self.mind.expected.get(token).copied().unwrap_or_default();
        (
            (hits as f32 + 1.0) / ((hits + misses) as f32 + 3.0),
            hits,
            misses,
        )
    }

    /// What the brain expects next in `stream`, surest first, each as sure
    /// as its share of primed columns and its record allow.
    pub fn expectations(&mut self, stream: &str, most: usize) -> Vec<(String, f32)> {
        if !self.enter(stream) {
            return Vec::new();
        }
        let mut expected: Vec<(String, f32)> = self
            .guesses(most.max(1) * 3)
            .into_iter()
            .map(|(token, share, _)| {
                let trust = self.trust(&token).0;
                (token, (share * trust).min(1.0))
            })
            .collect();
        expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        expected.truncate(most);
        expected
    }

    /// The percept beginning with `prefix` that is most surely expected
    /// soon in `stream`, when it is at least `least` sure and has come at
    /// least twice when expected. What is surely expected first (a person
    /// asking, before what they ask) is followed a few steps on to find
    /// it. The stream itself is not changed.
    pub fn foresee(&mut self, stream: &str, prefix: &str, least: f32) -> Option<(String, f32)> {
        if !self.enter(stream) {
            return None;
        }
        let mut sure_so_far = 1.0;
        for _ in 0..FORESEEN {
            let guesses = self.guesses(24);
            // Two different ones primed alike: it does not know which.
            let wanted = guesses
                .iter()
                .filter(|(token, share, _)| token.starts_with(prefix) && *share >= 0.5)
                .count();
            if wanted > 1 {
                return None;
            }
            let found = guesses
                .iter()
                .filter(|(token, _, _)| token.starts_with(prefix))
                .find_map(|(token, share, _)| {
                    let (trust, hits, _) = self.trust(token);
                    let sure = sure_so_far * share * trust;
                    (sure >= least && hits >= 2).then(|| (token.clone(), sure))
                });
            if found.is_some() {
                return found;
            }
            // A step with no rival costs no sureness beyond how fully it is
            // primed. Where rivals are primed too, the one followed is as
            // likely as its record makes it among theirs.
            let rivals: f32 = guesses
                .iter()
                .filter(|g| g.1 >= 0.5)
                .map(|g| self.trust(&g.0).0)
                .sum();
            let (token, share, _) = guesses.into_iter().next()?;
            let trust = self.trust(&token).0;
            sure_so_far *= if share >= 0.5 && rivals > trust {
                share * trust / rivals
            } else {
                share
            };
            if sure_so_far < least || self.fire_word("imagine", &token).is_none() {
                return None;
            }
        }
        None
    }

    /// Follows the cortex's own expectations on from where it is, a step
    /// at a time, for as long as it is sure enough. Learns nothing.
    fn run_on(&mut self, steps: usize) -> Imagined {
        let mut imagined = Imagined {
            known: true,
            ..Imagined::default()
        };
        let mut sure = 1.0;
        let mut weight = 1.0;
        for _ in 0..steps {
            let Some((token, share, _)) = self.guesses(3).into_iter().next() else {
                break;
            };
            sure *= share.min(1.0) * self.trust(&token).0.max(0.5);
            if sure < FAINT {
                break;
            }
            weight *= DISCOUNT;
            imagined.worth += weight * self.mind.values.worth(&token) * share;
            if self.fire_word("imagine", &token).is_none() {
                break;
            }
            imagined.steps.push((token, sure));
        }
        imagined
    }

    /// The way on from `stream` if `percepts` came next. The stream itself
    /// is not changed.
    pub fn imagine_after(&mut self, stream: &str, percepts: &[String], steps: usize) -> Imagined {
        self.enter(stream);
        let known = percepts.iter().all(|p| self.words.words.contains_key(p));
        let mut worth = 0.0;
        for percept in percepts {
            worth += self.mind.values.worth(percept);
            if let Some(columns) = self.words.words.get(percept).map(|w| w.columns.clone()) {
                self.fire("imagine", percept, &columns, false);
            }
        }
        let mut imagined = self.run_on(steps);
        imagined.worth += worth;
        imagined.known = known;
        imagined
    }

    /// Something worth saying about what to do next in `stream`, by the
    /// brain's own record. `None` is the usual answer.
    pub fn advise(&mut self, stream: &str) -> Option<Hunch> {
        if !self.enter(stream) {
            return None;
        }
        let advice = &self.mind.advice;
        // Where its advice has mostly been wrong it keeps quiet.
        if advice.given >= 10 && advice.right < 0.3 {
            self.mind.advice.held_back += 1;
            return None;
        }
        let caution = self.mind.advice.caution;
        let state = self.cortex.state();
        let guesses = self.guesses(16);
        let mut candidates: Vec<(f32, Hunch)> = Vec::new();
        for (token, share, _) in &guesses {
            if !DOING.iter().any(|prefix| token.starts_with(prefix)) {
                continue;
            }
            let (trust, hits, misses) = self.trust(token);
            let sure = share * trust;
            if sure < caution || hits + misses < MIN_RECORD {
                continue;
            }
            // Imagine it done, and the way on from there.
            self.cortex.restore(&state);
            let imagined = self.imagine_after_here(token);
            let worth = self.mind.values.worth(token) + imagined.worth;
            candidates.push((
                worth,
                Hunch {
                    kind: HunchKind::Next,
                    label: token.clone(),
                    with: imagined
                        .steps
                        .iter()
                        .map(|(t, _)| t.clone())
                        .take_while(|t| WITH.iter().any(|p| t.starts_with(p)))
                        .take(2)
                        .collect(),
                    because: self.mind.attention.tokens(),
                    confidence: sure,
                    hits,
                    misses,
                    imagined: imagined.steps.iter().map(|(t, _)| t.clone()).collect(),
                    worth,
                },
            ));
        }
        self.cortex.restore(&state);
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
        let clear = match candidates.as_slice() {
            [] => false,
            [(worth, _)] => *worth > 0.0,
            [(best, _), (next, _), ..] => *best > 0.0 && best - next >= MARGIN,
        };
        let hunch = if clear {
            Some(candidates.swap_remove(0).1)
        } else {
            self.warn(&state, caution)
        };
        self.cortex.restore(&state);
        let hunch = hunch?;
        let open = self.streams.get_mut(stream)?;
        if open.said.as_ref() == Some(&(hunch.kind, hunch.label.clone())) {
            return None;
        }
        open.said = Some((hunch.kind, hunch.label.clone()));
        if hunch.kind == HunchKind::Next {
            open.advised = Some(hunch.label.clone());
            self.mind.advice.given += 1;
        }
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: "decide".into(),
            seed: stream.into(),
            text: hunch.label.clone(),
            confidence: hunch.confidence,
            path: hunch.imagined.iter().map(|t| (t.clone(), 0.0)).collect(),
            why: match hunch.kind {
                HunchKind::Next => format!(
                    "Imagined each thing it expected might be done next and followed each on; \u{201c}{}\u{201d} led to the best (worth {:.2}).",
                    hunch.label, hunch.worth
                ),
                HunchKind::Warning => format!(
                    "The way on it expects from here leads to \u{201c}{}\u{201d}.",
                    hunch.label
                ),
            },
            ..Thought::default()
        });
        Some(hunch)
    }

    fn imagine_after_here(&mut self, token: &str) -> Imagined {
        if let Some(columns) = self.words.words.get(token).map(|w| w.columns.clone()) {
            self.fire("imagine", token, &columns, false);
        }
        self.run_on(IMAGINED)
    }

    /// The usual way on from here has usually led to bad news.
    fn warn(&mut self, state: &cortex::State, caution: f32) -> Option<Hunch> {
        self.cortex.restore(state);
        let first = self.guesses(1).into_iter().next()?;
        let (_, hits, misses) = self.trust(&first.0);
        if hits + misses < MIN_RECORD {
            return None;
        }
        let imagined = self.run_on(IMAGINED);
        let (bad, _) = imagined.steps.iter().find(|(token, sure)| {
            self.mind.values.worth(token) <= -0.25 && *sure >= caution * 0.8
        })?;
        Some(Hunch {
            kind: HunchKind::Warning,
            label: bad.clone(),
            with: imagined
                .steps
                .iter()
                .take_while(|(t, _)| t != bad)
                .map(|(t, _)| t.clone())
                .take(3)
                .collect(),
            because: self.mind.attention.tokens(),
            confidence: imagined
                .steps
                .iter()
                .find(|(t, _)| t == bad)
                .map_or(0.0, |(_, s)| *s),
            hits,
            misses,
            imagined: imagined.steps.iter().map(|(t, _)| t.clone()).collect(),
            worth: imagined.worth,
        })
    }

    /// A way from where `stream` is to `goal`: a search over what the
    /// cortex expects, keeping the likeliest few ways open at each step.
    /// Empty when no way was found within `depth` steps.
    pub fn plan(&mut self, stream: &str, goal: &str, depth: usize) -> Vec<String> {
        self.enter(stream);
        let mut open: Vec<Open> = vec![(self.cortex.state(), Vec::new(), 0.0)];
        for _ in 0..depth {
            let mut next = Vec::new();
            for (state, path, score) in &open {
                self.cortex.restore(state);
                let guesses = self.guesses(PLAN_BRANCHING * 2);
                // Reached when the goal is expected next, not merely
                // touched by a few stray columns.
                if guesses.iter().any(|g| g.0 == goal && g.1 >= REACHED) {
                    let mut path: Vec<String> = path.iter().map(|p| p.0.clone()).collect();
                    path.push(goal.to_string());
                    return path;
                }
                for (token, share, _) in guesses
                    .into_iter()
                    .filter(|g| !path.iter().any(|p| p.0 == g.0))
                    .take(PLAN_BRANCHING)
                {
                    self.cortex.restore(state);
                    if self.fire_word("reason", &token).is_none() {
                        continue;
                    }
                    let mut path = path.clone();
                    path.push((token, share));
                    let score = score + share.max(1e-3).ln();
                    next.push((self.cortex.state(), path, score));
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort_by(|a, b| b.2.total_cmp(&a.2));
            next.truncate(PLAN_BEAM);
            open = next;
        }
        Vec::new()
    }

    /// What it makes of where `stream` is now: its `most` surest
    /// expectations, the way on it imagines, and a way to `goal`. Empty
    /// for a stream it is not living. The stream itself is not changed.
    pub fn outlook(&mut self, stream: &str, most: usize, goal: Option<&str>) -> Outlook {
        if !self.is_living(stream) {
            return Outlook::default();
        }
        Outlook {
            expected: self.expectations(stream, most),
            imagined: self.imagine_after(stream, &[], IMAGINED.min(4)).steps,
            plan: goal
                .map(|goal| self.plan(stream, goal, 4))
                .unwrap_or_default(),
        }
    }

    /// The stream lived most recently.
    pub fn latest_stream(&self) -> Option<String> {
        self.streams
            .iter()
            .max_by_key(|(_, open)| open.used)
            .map(|(name, _)| name.clone())
    }

    /// The streams being lived now.
    pub fn streams(&self) -> Vec<String> {
        let mut names: Vec<String> = self.streams.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn run(brain: &mut Brain, good: bool) {
        brain.begin("run", "agent");
        brain.live("run", &p(&["person: asks", "word: rounding"]));
        brain.live("run", &p(&["does: read_file", "file: src/fees.rs"]));
        if good {
            brain.live("run", &p(&["does: shell", "runs: cargo test"]));
            brain.live("run", &p(&["checks: passed"]));
        } else {
            brain.live("run", &p(&["does: edit_file", "file: src/fees.rs"]));
            brain.live("run", &p(&["checks: failed"]));
        }
        brain.end("run");
    }

    #[test]
    fn it_learns_what_happens_and_keeps_each_stretch_as_a_memory() {
        let mut brain = Brain::new();
        brain.mean(&[("checks: passed", 1.0), ("checks: failed", -0.7)]);
        for _ in 0..8 {
            run(&mut brain, true);
        }
        assert!(brain.engrams.len() >= 8);
        brain.begin("run", "agent");
        brain.live("run", &p(&["person: asks", "word: rounding"]));
        brain.live("run", &p(&["does: read_file", "file: src/fees.rs"]));
        let expected = brain.expectations("run", 3);
        assert_eq!(
            expected.first().map(|e| e.0.as_str()),
            Some("does: shell"),
            "{expected:?}"
        );
        assert!(brain.mind.values.worth("runs: cargo test") > 0.0);
        assert!(brain.mind.values.worth("does: read_file") > 0.0);
    }

    #[test]
    fn it_advises_the_way_that_went_well_and_checks_whether_it_came() {
        let mut brain = Brain::new();
        brain.mean(&[("checks: passed", 1.0), ("checks: failed", -0.7)]);
        for at in 0..12 {
            run(&mut brain, at % 3 != 0);
        }
        brain.begin("run", "agent");
        brain.live("run", &p(&["person: asks", "word: rounding"]));
        brain.live("run", &p(&["does: read_file", "file: src/fees.rs"]));
        let hunch = brain.advise("run").expect("a hunch");
        assert_eq!(hunch.kind, HunchKind::Next);
        assert_eq!(hunch.label, "does: shell");
        assert!(hunch.worth > 0.0, "{hunch:?}");
        // Said once, not again for the same moment.
        assert!(brain.advise("run").is_none());
        let moment = brain.live("run", &p(&["does: shell", "runs: cargo test"]));
        assert_eq!(moment.advice_right, Some(true));
    }

    #[test]
    fn what_came_three_steps_back_decides_what_comes_next() {
        // After "e x" comes r or t, and only what came before "e" says
        // which. A branch that learned "x" in one context must not also
        // learn it in the other, or the two become one and both are
        // expected every time.
        let mut brain = Brain::new();
        brain.begin("s", "s");
        let mut clean = 0;
        for cycle in 0..60 {
            for (start, then, rival) in [("k: q1", "k: r", "k: t"), ("k: q2", "k: t", "k: r")] {
                for token in [start, "k: e", "k: x"] {
                    brain.live("s", &p(&[token]));
                }
                let expected = brain.expectations("s", 3);
                let share =
                    |token: &str| expected.iter().find(|e| e.0 == token).map_or(0.0, |e| e.1);
                if cycle >= 40 && share(then) > 0.5 && share(rival) < 0.5 * share(then) {
                    clean += 1;
                }
                brain.live("s", &p(&[then]));
            }
        }
        // A repeating sequence with no break is learned as ever longer
        // context, which now and then has to be learned again; most of
        // the time each context expects only its own ending.
        assert!(
            clean >= 30,
            "{clean} of the last 40 endings expected cleanly"
        );
    }

    #[test]
    fn streams_keep_their_own_place() {
        let mut brain = Brain::new();
        for _ in 0..6 {
            for (stream, seq) in [
                ("a", ["x: 1", "x: 2", "x: 3"]),
                ("b", ["y: 1", "y: 2", "y: 3"]),
            ] {
                brain.begin(stream, stream);
                for token in seq {
                    brain.live(stream, &p(&[token]));
                }
                brain.end(stream);
            }
        }
        brain.begin("a", "a");
        brain.begin("b", "b");
        brain.live("a", &p(&["x: 1"]));
        brain.live("b", &p(&["y: 1"]));
        // Words learned in between do not lose either stream its place.
        brain.learn("the cat sat on the mat .", "teacher");
        assert_eq!(brain.expectations("a", 1)[0].0, "x: 2");
        assert_eq!(brain.expectations("b", 1)[0].0, "y: 2");
        assert_eq!(brain.plan("a", "x: 3", 4), p(&["x: 2", "x: 3"]));
    }
}
