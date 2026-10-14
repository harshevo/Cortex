//! The network: neurons that are created when they are needed, each with
//! dendritic branches that learn which combinations of earlier firing it
//! follows, and a few mechanisms on top.
//!
//! Time moves in *moments*. A moment is one thing happening: a tool being
//! called, its result, something the person said. Each moment arrives as
//! a handful of percepts, short texts such as `does: edit_file` or
//! `file: src/fees.rs`.
//!
//! - **Growth.** A percept nothing has met before gets a *sense* neuron.
//!   A set of sense neurons that keeps firing in the same moment gets an
//!   *assembly* neuron that fires when most of the set does.
//! - **Dendrites.** A neuron does not add up everything that reaches it.
//!   It has up to [`MAX_SEGMENTS`] branches, each holding synapses from a
//!   few neurons that fired together just before it once did. A branch is
//!   active only when most of *its own* synapses are driven, so "an edit
//!   to this file, then the tests" and "the tests on their own" are two
//!   branches with two records, not one blurred sum. A neuron that fires
//!   with no active branch grows one, or bends its nearest one, toward
//!   what had just fired. A branch that was active and whose neuron then
//!   stayed silent is weakened and its miss is counted.
//! - **Prediction.** Before each moment the neurons with an active branch
//!   are expected, each as surely as that branch has been right before.
//!   How much of the moment was not expected is the *surprise*.
//! - **Workspace.** The firing neurons compete; the few most salient (the
//!   new, the unexpected, the assemblies) are held for a while and are
//!   part of what branches see, so something from several moments ago can
//!   still decide what is expected now.
//! - **Value.** Some percepts are good or bad news (checks passing, the
//!   person saying no). When one comes, the neurons that fired in the
//!   moments before it move their *value* toward it, the nearest most.
//! - **Imagination.** The network can run on its own expectations: what
//!   it expects becomes what "fired", and it asks again. A few steps of
//!   that is a guess at how things go on from here, with a worth.
//! - **Inference.** Three things are built from the above and nothing
//!   else. *Forward*: imagine on from now. *Compare*: imagine on from
//!   each action it expects and prefer the one whose path is worth most.
//!   *Backward*: from a goal, follow its best branch to what preceded it,
//!   and that one's branch in turn, until it reaches what is firing now.
//!   This is chaining over learned sequences. It is not reasoning in
//!   language and cannot do what the model does.
//! - **Self-model.** A few neurons stand for the network's own state ("I
//!   was surprised", "my advice was wrong"). They fire and wire like any
//!   other, so the network learns in which situations it tends to be
//!   wrong, and it stays silent where it expects that.
//! - **Plasticity.** How readily a synapse changes is not fixed. Each
//!   branch sets its own from its record: one that keeps being right
//!   stiffens, one that is new or starts failing loosens. Over that, the
//!   whole network learns faster while it is more surprised than usual
//!   and more slowly while what it knows holds.
//! - **Self-tuning.** How sure a branch must be to count as an
//!   expectation, and how sure the network must be before advising, also
//!   follow its own record.
//! - **Rest.** With nothing happening, recent moments are replayed at half
//!   strength, a limited number of times each. Replay strengthens
//!   synapses; it grows nothing and does not count toward any record.
//!
//! None of this is evidence of experience or of awareness. Each quantity
//! shown is a measurement of the mechanism named, and nothing more is
//! claimed.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

pub type Id = u64;

/// Share of the remaining headroom a synapse gains when its source fired
/// just before its neuron did.
const LEARNING: f32 = 0.25;

/// Share of its weight a synapse loses when its source fired and its
/// neuron did not follow, or its neuron fired without its source.
const DEPRESSION: f32 = 0.08;

/// How much of a full weakening a branch takes for a promise not kept.
const MISS: f32 = 0.25;

/// Share of its weight every synapse loses at each tidy.
const DECAY: f32 = 0.01;

/// Moments between tidies.
const TIDY_EVERY: u64 = 20;

/// Synapses weaker than this are removed.
const WEAKEST: f32 = 0.05;

/// Most branches on one neuron; the least proven go first.
pub const MAX_SEGMENTS: usize = 8;

/// Most synapses on one branch.
const SEGMENT_SYNAPSES: usize = 6;

/// A branch that has been right this often is no longer bent toward
/// other situations. A new one is grown beside it instead.
const PROVEN: u32 = 3;

/// Share of its weight a synapse of a branch being bent loses when its
/// source had not fired.
const BENDING: f32 = 0.3;

/// Share of a branch's synaptic weight that must be driven for the branch
/// to be active.
const QUORUM: f32 = 0.75;

/// A branch driven by less than this, however large a share of it that
/// is, is too weak to count.
const DRIVE: f32 = 0.2;

/// Share of a branch that must be driven for a neuron that fired without
/// an active branch to bend that branch instead of growing a new one.
const MATCHING: f32 = 0.4;

/// A branch that has missed this many times, and several times more often
/// than it has hit, is removed.
const HOPELESS: u32 = 8;

/// Times a set of sense neurons must fire together before it gets an
/// assembly neuron.
const TOGETHER: u8 = 2;

/// Neurons held in the workspace.
const WORKSPACE: usize = 4;

/// Share of its salience a held neuron keeps from one moment to the next.
const HOLD: f32 = 0.6;

/// How sure a branch must be for its neuron to be expected, to begin
/// with. The network moves this itself.
const EXPECTED: f32 = 0.35;

/// An expected neuron must be at least this share as sure as the surest.
const RIVAL: f32 = 0.5;

/// Neurons expected at most.
const EXPECTATIONS: usize = 5;

/// An assembly that fired at most once is removed after this many moments.
const UNPROVEN: u64 = 300;

/// A neuron that has not fired for this many moments is removed.
const FORGOTTEN: u64 = 6_000;

/// Most neurons kept. Beyond it, the longest silent go.
const MAX_NEURONS: usize = 3_000;

/// Moments kept for replay, and times each is replayed.
const REMEMBERED: usize = 64;
const REPLAYS: u8 = 3;

/// Moments replayed in one rest.
const REPLAYED_AT_ONCE: usize = 4;

/// Samples of the meters kept for drawing.
const HISTORY: usize = 240;

/// Lines of the log kept.
const LOG: usize = 40;

/// Weight of a new reading in a running average.
const AVERAGING: f32 = 0.1;

/// Moments before good or bad news whose neurons take some of it, and how
/// much less each earlier one takes.
const TRACE: usize = 8;
const DISCOUNT: f32 = 0.85;

/// How much less of the news each earlier moment's neurons take.
const CREDIT: f32 = 0.6;

/// How far a neuron's value moves toward news that follows it at once.
const VALUE_RATE: f32 = 0.2;

/// Steps imagined ahead, and the confidence below which a path is dropped.
const IMAGINED: usize = 4;
const FAINT: f32 = 0.15;

/// How sure a branch must be before the network advises on it, to begin
/// with, and the times it must have been tried.
const CAUTION: f32 = 0.7;
const MIN_RECORD: u32 = 4;

/// By how much one imagined path must be worth more than the next for
/// the network to prefer it aloud.
const MARGIN: f32 = 0.15;

/// The network's own states. Each has a neuron from the start.
const SURPRISED: &str = "I was surprised";
const EXPECTED_IT: &str = "I expected this";
const GREW: &str = "I grew";
const RESTING: &str = "I am resting";
const ADVICE_RIGHT: &str = "my advice was right";
const ADVICE_WRONG: &str = "my advice was wrong";

const OWN_STATES: [&str; 6] = [
    SURPRISED,
    EXPECTED_IT,
    GREW,
    RESTING,
    ADVICE_RIGHT,
    ADVICE_WRONG,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One thing met in the world.
    Sense,

    /// Grown for a set of sense neurons that kept firing together.
    Assembly,

    /// One of the network's own states.
    Own,
}

/// When its source must have fired for a synapse to be driven.
type Lag = u8;

/// Held in the workspace from some earlier moment.
const HELD: Lag = 0;

/// Fired in the latest moment.
const LATEST: Lag = 1;

/// Fired in the moment before the latest.
const EARLIER: Lag = 2;

/// What had fired, and when: what a branch's synapses are held against.
type Context = HashSet<(Id, Lag)>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Synapse {
    from: Id,
    weight: f32,

    /// A synapse answers to its source having fired at one particular
    /// time, so "the tests, then this" and "this, then the tests" are
    /// different to a branch.
    #[serde(default = "latest")]
    lag: Lag,
}

fn latest() -> Lag {
    LATEST
}

impl Synapse {
    fn driven(&self, active: &Context) -> bool {
        active.contains(&(self.from, self.lag))
    }
}

/// A synapse as networks saved before dendrites kept it: on its source.
#[derive(Debug, Clone, Deserialize)]
struct Outgoing {
    to: Id,
    weight: f32,
}

/// One dendritic branch: the neurons that, firing together, have been
/// followed by this one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Segment {
    synapses: Vec<Synapse>,

    /// Lived moments in which it was active and its neuron followed, and
    /// in which it was active and its neuron did not.
    hits: u32,
    misses: u32,

    last_active: u64,
}

impl Segment {
    /// The weight of its synapses whose source is in `active`, and of all
    /// of them.
    fn drive(&self, active: &Context) -> (f32, f32) {
        let mut matched = 0.0;
        let mut total = 0.0;

        for synapse in &self.synapses {
            total += synapse.weight;

            if synapse.driven(active) {
                matched += synapse.weight;
            }
        }

        (matched, total)
    }

    /// How often it has been right, with a cautious start: a branch that
    /// has been right once is not yet a rule.
    fn confidence(&self) -> f32 {
        (self.hits as f32 + 1.0) / ((self.hits + self.misses) as f32 + 3.0)
    }

    fn tried(&self) -> u32 {
        self.hits + self.misses
    }

    /// How readily it changes, from 0.3 to 1: a branch with a good
    /// record is stiff, so that one odd moment does not undo it, and one
    /// that is new or has started to fail is loose again.
    fn plasticity(&self) -> f32 {
        (1.3 - self.confidence()).clamp(0.3, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Neuron {
    id: Id,
    kind: Kind,
    label: String,

    /// The moment it was created in, and why.
    born: u64,
    reason: String,

    fired: u32,
    last_fired: u64,

    /// The sense neurons an assembly stands for.
    #[serde(default)]
    members: Vec<Id>,

    /// How surprising the moments after it held the workspace have been,
    /// on average. The self-model's memory.
    expects_surprise: f32,

    /// How the moments after it fired have turned out, from -1 to 1.
    #[serde(default)]
    value: f32,

    #[serde(default)]
    segments: Vec<Segment>,

    /// Read from a network saved before dendrites, and turned into
    /// branches on waking.
    #[serde(default, skip_serializing)]
    out: Vec<Outgoing>,
}

/// One remembered moment, for replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Remembered {
    percepts: Vec<String>,
    replayed: u8,
}

/// A reading of the meters at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub neurons: u32,
    pub synapses: u32,
    pub surprise: f32,
}

/// Everything counted since the network was first made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meters {
    pub neurons: usize,
    pub synapses: usize,

    /// Dendritic branches, over all neurons.
    pub segments: usize,

    /// Neurons created and removed, synapses formed and removed,
    /// branches grown and removed.
    pub grown: u64,
    pub removed: u64,
    pub connected: u64,
    pub disconnected: u64,
    pub branched: u64,
    pub pruned: u64,

    /// Running averages over live moments, each from 0 to 1.
    ///
    /// `surprise`: the share of a moment that was not expected.
    /// `expected_right`: the share of a moment the network had expected.
    /// `precision`: the share of what it expected that came.
    /// `baseline_right`: `expected_right` for guessing the most often
    /// fired neurons, as many guesses as the network made. The network
    /// has learned something only where it beats this.
    /// `self_accuracy`: how close its prediction of its own surprise came.
    /// `reach`: the share of the network the workspace has synapses onto.
    pub surprise: f32,
    pub expected_right: f32,
    pub precision: f32,
    pub baseline_right: f32,
    pub self_accuracy: f32,
    pub reach: f32,

    /// The surprise averaged over ten times as long. Where the recent
    /// surprise is above it, things have changed.
    pub settled_surprise: f32,

    /// Moments lived, and moments replayed at rest.
    pub moments: u64,
    pub replays: u64,

    /// Times it advised, how often the advised thing came next, and
    /// times it kept quiet because it expected to be wrong.
    pub advised: u64,
    pub advice_right: f32,
    pub held_back: u64,

    /// What the network has set for itself: how fast it learns (1 is
    /// where it started), how sure a branch must be to count as an
    /// expectation, and how sure before it advises.
    pub plasticity: f32,
    pub threshold: f32,
    pub caution: f32,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            neurons: 0,
            synapses: 0,
            segments: 0,
            grown: 0,
            removed: 0,
            connected: 0,
            disconnected: 0,
            branched: 0,
            pruned: 0,
            surprise: 0.0,
            expected_right: 0.0,
            precision: 0.0,
            baseline_right: 0.0,
            self_accuracy: 0.0,
            reach: 0.0,
            // Everything is new to begin with.
            settled_surprise: 0.5,
            moments: 0,
            replays: 0,
            advised: 0,
            advice_right: 0.0,
            held_back: 0,
            plasticity: 1.0,
            threshold: EXPECTED,
            caution: CAUTION,
        }
    }
}

/// One step of an imagined way on from here.
#[derive(Debug, Clone, PartialEq)]
pub struct Thought {
    pub labels: Vec<String>,

    /// How sure the whole path up to here is, from 0 to 1.
    pub confidence: f32,

    /// The good or bad news in this step, weighed by how sure it is.
    pub worth: f32,
}

/// Why something is expected: the branch that says so and its record.
#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub expected: String,

    /// What had fired that the branch answers to.
    pub because: Vec<String>,

    pub confidence: f32,
    pub hits: u32,
    pub misses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunchKind {
    /// What has usually come next here, and gone well.
    Next,

    /// The usual way on from here has usually gone badly.
    Warning,
}

/// Something the network is sure enough of to say.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunch {
    pub kind: HunchKind,

    /// The percept it is about.
    pub label: String,

    /// Other things expected with it: the file, the command.
    pub with: Vec<String>,

    pub because: Vec<String>,
    pub confidence: f32,
    pub hits: u32,
    pub misses: u32,

    /// What it imagined following.
    pub imagined: Vec<String>,

    /// What the imagined path is worth.
    pub worth: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    moment: u64,
    next_id: Id,
    neurons: Vec<Neuron>,
    meters: Meters,

    /// The neurons held in the workspace, with their salience.
    workspace: Vec<(Id, f32)>,

    remembered: VecDeque<Remembered>,
    history: VecDeque<Sample>,
    log: VecDeque<String>,

    /// Which percepts are good or bad news, and how much, from -1 to 1.
    worth: HashMap<String, f32>,

    /// The beginnings of the percepts that are things to do, as opposed
    /// to things that happen. The first is what is chosen between; the
    /// rest say what the chosen thing is done to.
    actions: Vec<String>,

    /// What fired in the latest moment: the world first, then the
    /// network's own states.
    #[serde(skip)]
    fired: Vec<Id>,
    #[serde(skip)]
    fired_world: usize,

    /// What the world did in the moment before that.
    #[serde(skip)]
    prior: Vec<Id>,

    /// What the network expects of the next moment, why, and how
    /// surprised it expects to be.
    #[serde(skip)]
    expected: Vec<Id>,
    #[serde(skip)]
    reasons: Vec<Reason>,
    #[serde(skip)]
    expected_surprise: Option<f32>,

    /// The world's part of each of the last few moments, newest last.
    #[serde(skip)]
    trace: VecDeque<Vec<Id>>,

    /// What it last advised, to be judged by the next moment.
    #[serde(skip)]
    advised: Option<Id>,
    #[serde(skip)]
    last_hunch: Option<Hunch>,

    /// The latest imagined way on, and the way to the goal.
    #[serde(skip)]
    imagined: Vec<Thought>,
    #[serde(skip)]
    plan: Vec<Vec<String>>,
    #[serde(skip)]
    goal: Option<String>,

    /// How often each set of sense neurons has fired together, and the
    /// assembly it has, once it has one.
    #[serde(skip)]
    together: HashMap<Vec<Id>, (u8, Option<Id>)>,

    #[serde(skip)]
    index: HashMap<Id, usize>,
    #[serde(skip)]
    by_label: HashMap<String, Id>,

    #[serde(skip)]
    resting: bool,
    #[serde(skip)]
    random: u64,
}

/// One neuron, as the picture shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct NeuronView {
    pub id: Id,
    pub kind: Kind,
    pub label: String,
    pub born: u64,
    pub reason: String,
    pub fired: u32,
    pub last_fired: u64,

    /// 1 when it fired in the latest moment, fading over the next few.
    pub glow: f32,

    pub in_workspace: bool,
    pub expects_surprise: f32,

    /// How the moments after it fired have turned out, from -1 to 1.
    pub value: f32,

    /// Its dendritic branches.
    pub segments: u32,
}

/// The whole network at one moment, for drawing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub moment: u64,
    pub neurons: Vec<NeuronView>,

    /// Source, target, weight: the strongest synapse from one onto any
    /// branch of the other.
    pub synapses: Vec<(Id, Id, f32)>,

    /// The labels of what the workspace holds, most salient first.
    pub workspace: Vec<String>,

    /// What it expects next, why, and how surprised it expects to be.
    pub expecting: Vec<String>,
    pub reasons: Vec<Reason>,
    pub expected_surprise: Option<f32>,

    /// The way on from here as it imagines it, a step at a time.
    pub imagined: Vec<Thought>,

    /// The best news it knows how to reach, and the steps it has seen
    /// lead there, the nearest to now first.
    pub goal: Option<String>,
    pub plan: Vec<Vec<String>>,

    /// The last thing it was sure enough of to say.
    pub hunch: Option<Hunch>,

    pub meters: Meters,
    pub history: Vec<Sample>,

    /// What changed lately, newest last.
    pub log: Vec<String>,

    pub resting: bool,
}

/// A neuron with an active branch, and how sure that branch is.
#[derive(Debug, Clone)]
struct Prediction {
    id: Id,
    confidence: f32,

    /// Which of the neuron's branches.
    segment: usize,
}

impl Network {
    pub fn new() -> Self {
        let mut network = Self::default();

        network.wake();

        network
    }

    /// A network with its own idea of what is good and bad news and of
    /// which percepts are things to do, for something other than the
    /// agent's runs.
    pub fn with_meanings(worth: &[(&str, f32)], actions: &[&str]) -> Self {
        let mut network = Self {
            worth: worth
                .iter()
                .map(|(label, worth)| (label.to_string(), *worth))
                .collect(),
            actions: actions.iter().map(|prefix| prefix.to_string()).collect(),
            ..Self::default()
        };

        network.wake();

        network
    }

    /// Puts a loaded network back in working order.
    pub fn wake(&mut self) {
        if self.worth.is_empty() {
            self.worth = crate::percept::WORTH
                .iter()
                .map(|(label, worth)| (label.to_string(), *worth))
                .collect();
        }

        if self.actions.is_empty() {
            self.actions = crate::percept::ACTIONS
                .iter()
                .map(|prefix| prefix.to_string())
                .collect();
        }

        self.rebuild_index();
        self.grow_dendrites_from_old_synapses();

        for state in OWN_STATES {
            if !self.by_label.contains_key(state) {
                self.grow(Kind::Own, state, "one of its own states", Vec::new());
            }
        }

        for neuron in &self.neurons {
            if neuron.kind == Kind::Assembly {
                self.together
                    .insert(neuron.members.clone(), (TOGETHER, Some(neuron.id)));
            }
        }

        self.random = 0x9E37_79B9_7F4A_7C15 ^ self.moment.wrapping_add(1);
    }

    /// A network saved before dendrites kept each synapse on its source.
    /// Each becomes a branch of one synapse on its target, the strongest
    /// first, so that what was learned is still expected.
    fn grow_dendrites_from_old_synapses(&mut self) {
        let mut incoming: HashMap<Id, Vec<Synapse>> = HashMap::new();

        for neuron in &mut self.neurons {
            for synapse in neuron.out.drain(..) {
                incoming.entry(synapse.to).or_default().push(Synapse {
                    from: neuron.id,
                    weight: synapse.weight,
                    lag: LATEST,
                });
            }
        }

        for (target, mut synapses) in incoming {
            let Some(neuron) = self.neuron_mut(target) else {
                continue;
            };

            synapses.sort_by(|left, right| right.weight.total_cmp(&left.weight));

            for synapse in synapses.into_iter().take(MAX_SEGMENTS) {
                neuron.segments.push(Segment {
                    synapses: vec![synapse],
                    ..Segment::default()
                });
            }
        }
    }

    pub fn moment(&self) -> u64 {
        self.moment
    }

    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    pub fn is_resting(&self) -> bool {
        self.resting
    }

    /// Whether a neuron with this label exists.
    pub fn knows(&self, label: &str) -> bool {
        self.by_label.contains_key(label)
    }

    /// The weight of the strongest synapse from one labelled neuron onto
    /// a branch of another, 0 if there is none.
    pub fn weight(&self, from: &str, to: &str) -> f32 {
        let (Some(from), Some(to)) = (self.by_label.get(from), self.by_label.get(to)) else {
            return 0.0;
        };

        self.neuron(*to).map_or(0.0, |neuron| {
            neuron
                .segments
                .iter()
                .flat_map(|segment| &segment.synapses)
                .filter(|synapse| synapse.from == *from)
                .map(|synapse| synapse.weight)
                .fold(0.0, f32::max)
        })
    }

    /// How the moments after this percept have turned out, from -1 to 1.
    pub fn value(&self, label: &str) -> f32 {
        self.by_label
            .get(label)
            .and_then(|id| self.neuron(*id))
            .map_or(0.0, |neuron| neuron.value)
    }

    /// The labels of what is expected of the next moment.
    pub fn expecting(&self) -> Vec<String> {
        self.labels(&self.expected)
    }

    /// What is expected of the next moment, with how sure each is.
    pub fn expectations(&self) -> Vec<(String, f32)> {
        self.reasons
            .iter()
            .map(|reason| (reason.expected.clone(), reason.confidence))
            .collect()
    }

    /// The way on from here as the network imagines it.
    pub fn imagined(&self) -> &[Thought] {
        &self.imagined
    }

    /// One thing happened. `percepts` say what, in a few short texts.
    pub fn live(&mut self, percepts: &[String]) {
        if percepts.is_empty() {
            return;
        }

        self.resting = false;
        self.moment += 1;
        self.meters.moments += 1;

        let before = self.neurons.len();

        let mut sensed: Vec<Id> = Vec::new();

        for percept in percepts {
            let id = self.sense(percept);

            if !sensed.contains(&id) {
                sensed.push(id);
            }
        }

        let grew = self.neurons.len() > before;

        let surprise = self.judge_expectations(&sensed);
        let advice = self.judge_advice(&sensed);

        // The network's own states fire with the moment they describe.
        let mut own = vec![if surprise >= 0.5 {
            SURPRISED
        } else {
            EXPECTED_IT
        }];

        if grew {
            own.push(GREW);
        }

        match advice {
            Some(true) => own.push(ADVICE_RIGHT),
            Some(false) => own.push(ADVICE_WRONG),
            None => {}
        }

        let assembled = self.assemble(&sensed);

        // The world first, the most specific of it first: a new branch
        // takes its synapses from the front.
        let mut firing = assembled.clone();

        firing.extend(&sensed);

        let world = firing.len();

        firing.extend(own.iter().filter_map(|state| self.by_label.get(*state)));

        self.credit(&sensed);
        self.pass(&firing, world, &sensed, &assembled, surprise, 1.0, true);

        self.remembered.push_back(Remembered {
            percepts: percepts.to_vec(),
            replayed: 0,
        });

        while self.remembered.len() > REMEMBERED {
            self.remembered.pop_front();
        }

        if self.moment.is_multiple_of(TIDY_EVERY) {
            self.tidy();
            self.tune();
        }

        self.count();

        self.history.push_back(Sample {
            neurons: self.meters.neurons as u32,
            synapses: self.meters.synapses as u32,
            surprise,
        });

        while self.history.len() > HISTORY {
            self.history.pop_front();
        }

        self.think();
    }

    /// Nothing is happening: go back over a few recent moments at half
    /// strength. Returns whether there was anything left to go over.
    pub fn rest(&mut self) -> bool {
        let fresh: Vec<usize> = (0..self.remembered.len())
            .filter(|at| self.remembered[*at].replayed < REPLAYS)
            .collect();

        if fresh.is_empty() {
            self.resting = false;

            return false;
        }

        self.resting = true;

        // What was firing when the rest began is put back afterwards, so
        // that the next lived moment is still wired to the one before it.
        let fired = std::mem::take(&mut self.fired);
        let fired_world = self.fired_world;
        let prior = std::mem::take(&mut self.prior);
        let expected = std::mem::take(&mut self.expected);
        let reasons = std::mem::take(&mut self.reasons);
        let expected_surprise = self.expected_surprise.take();
        let workspace = self.workspace.clone();

        // A run of consecutive moments, so that sequence is what is
        // strengthened.
        let start = fresh[(self.draw() % fresh.len() as u64) as usize];
        let end = (start + REPLAYED_AT_ONCE).min(self.remembered.len());

        self.fired_world = 0;

        for at in start..end {
            self.remembered[at].replayed += 1;

            let percepts = self.remembered[at].percepts.clone();

            // Only what is still known is replayed: rest grows nothing.
            let mut sensed: Vec<Id> = percepts
                .iter()
                .filter_map(|percept| self.by_label.get(percept).copied())
                .collect();

            sensed.dedup();

            if sensed.is_empty() {
                continue;
            }

            let assembled = self.firing_assemblies(&sensed);

            let mut firing = assembled.clone();

            firing.extend(&sensed);

            let world = firing.len();

            firing.extend(self.by_label.get(RESTING));

            self.pass(&firing, world, &sensed, &assembled, 0.0, 0.5, false);

            self.meters.replays += 1;
        }

        self.fired = fired;
        self.fired_world = fired_world;
        self.prior = prior;
        self.expected = expected;
        self.reasons = reasons;
        self.expected_surprise = expected_surprise;
        self.workspace = workspace;

        self.count();

        true
    }

    /// The way on from what is firing now, a step at a time, for as long
    /// as the network is sure of anything.
    pub fn imagine(&self, steps: usize) -> Vec<Thought> {
        self.imagine_from(&self.context(), steps)
    }

    /// The way on from here if `percepts` were what came next. Empty when
    /// the network has met none of them.
    pub fn imagine_after(&self, percepts: &[String], steps: usize) -> Vec<Thought> {
        let ids: Vec<Id> = percepts
            .iter()
            .filter_map(|percept| self.by_label.get(percept).copied())
            .collect();

        if ids.is_empty() {
            return Vec::new();
        }

        self.imagine_from(&self.imagined_on(&self.context(), &ids), steps)
    }

    /// The way on from `start` having fired.
    fn imagine_from(&self, start: &[(Id, Lag)], steps: usize) -> Vec<Thought> {
        let mut context: Vec<(Id, Lag)> = start.to_vec();
        let mut sure = 1.0;
        let mut thoughts = Vec::new();

        for _ in 0..steps {
            let active: Context = context.iter().copied().collect();
            let chosen = self.chosen(self.predict(&active));

            if chosen.is_empty() {
                break;
            }

            let confidence = chosen
                .iter()
                .map(|prediction| prediction.confidence)
                .sum::<f32>()
                / chosen.len() as f32;

            sure *= confidence;

            if sure < FAINT {
                break;
            }

            let ids: Vec<Id> = chosen.iter().map(|prediction| prediction.id).collect();
            let labels = self.labels(&ids);

            let worth = chosen
                .iter()
                .zip(&labels)
                .map(|(prediction, label)| self.worth_of(label) * prediction.confidence)
                .sum();

            thoughts.push(Thought {
                labels,
                confidence: sure,
                worth,
            });

            context = self.imagined_on(&context, &ids);
        }

        thoughts
    }

    /// What would have fired, and when, had `ids` come after `context`:
    /// they are the latest, what was the latest is a moment older, and
    /// what was held is still held.
    fn imagined_on(&self, context: &[(Id, Lag)], ids: &[Id]) -> Vec<(Id, Lag)> {
        let mut next: Vec<(Id, Lag)> = self
            .firing_assemblies(ids)
            .into_iter()
            .chain(ids.iter().copied())
            .map(|id| (id, LATEST))
            .collect();

        for (id, lag) in context {
            let world = self
                .neuron(*id)
                .is_some_and(|neuron| neuron.kind != Kind::Own);

            match *lag {
                LATEST if world => next.push((*id, EARLIER)),
                HELD => next.push((*id, HELD)),
                _ => {}
            }
        }

        next
    }

    /// The steps that have led to `goal`, the nearest to now first: the
    /// goal's surest branch says what preceded it, that one's surest
    /// branch what preceded that, until what is firing now is reached or
    /// `depth` steps have been followed.
    pub fn plan(&self, goal: &str, depth: usize) -> Vec<Vec<String>> {
        let Some(mut at) = self.by_label.get(goal).copied() else {
            return Vec::new();
        };

        let now: HashSet<Id> = self
            .context()
            .into_iter()
            .filter(|(_, lag)| *lag == LATEST)
            .map(|(id, _)| id)
            .collect();

        let mut visited: HashSet<Id> = HashSet::from([at]);
        let mut chain: Vec<Vec<String>> = Vec::new();

        for _ in 0..depth {
            let Some(neuron) = self.neuron(at) else {
                break;
            };

            let surest = neuron
                .segments
                .iter()
                .filter(|segment| segment.hits > 0)
                .max_by(|left, right| left.confidence().total_cmp(&right.confidence()));

            let Some(segment) = surest else {
                break;
            };

            // What the world did just before, not what the network felt
            // about it and not what came a moment sooner: that is the
            // step before this one.
            let mut sources: Vec<(&Neuron, f32)> = segment
                .synapses
                .iter()
                .filter(|synapse| synapse.lag == LATEST && synapse.weight >= WEAKEST * 2.0)
                .filter_map(|synapse| Some((self.neuron(synapse.from)?, synapse.weight)))
                .filter(|(source, _)| source.kind == Kind::Sense)
                .collect();

            if sources.is_empty() {
                break;
            }

            sources.sort_by(|left, right| right.1.total_cmp(&left.1));

            chain.push(
                sources
                    .iter()
                    .map(|(source, _)| source.label.clone())
                    .collect(),
            );

            if sources.iter().all(|(source, _)| now.contains(&source.id)) {
                break;
            }

            // Go on from something to do, if the step has one.
            let next = sources
                .iter()
                .filter(|(source, _)| !visited.contains(&source.id) && !now.contains(&source.id))
                .max_by_key(|(source, _)| self.is_action(&source.label))
                .map(|(source, _)| source.id);

            let Some(next) = next else {
                break;
            };

            visited.insert(next);

            at = next;
        }

        chain.reverse();

        chain
    }

    /// Why a percept is expected when it is: each of its branches, the
    /// surest first.
    pub fn explain(&self, label: &str) -> Vec<Reason> {
        let Some(neuron) = self.by_label.get(label).and_then(|id| self.neuron(*id)) else {
            return Vec::new();
        };

        let mut reasons: Vec<Reason> = neuron
            .segments
            .iter()
            .map(|segment| self.reason(neuron, segment, segment.confidence()))
            .collect();

        reasons.sort_by(|left, right| right.confidence.total_cmp(&left.confidence));

        reasons
    }

    /// Something worth saying about the next step, when the network is
    /// sure enough by its own record. `None` is the usual answer.
    ///
    /// It compares the actions it expects by imagining on from each, and
    /// names the one whose path is worth most. If the usual way on leads
    /// to bad news instead, it says that. It says nothing where it has
    /// learned that its advice tends to be wrong.
    pub fn advise(&mut self) -> Option<Hunch> {
        let context = self.context();
        let active: Context = context.iter().copied().collect();

        let predictions = self.predict(&active);

        // The self-model: this is a place where it has been wrong.
        let doubts = self.by_label.get(ADVICE_WRONG).copied();

        if predictions
            .iter()
            .any(|prediction| Some(prediction.id) == doubts && prediction.confidence >= 0.5)
        {
            self.meters.held_back += 1;

            return None;
        }

        let caution = self.meters.caution;

        let mut candidates: Vec<(f32, Hunch, Id)> = Vec::new();

        for prediction in &predictions {
            let Some(neuron) = self.neuron(prediction.id) else {
                continue;
            };

            let segment = &neuron.segments[prediction.segment];

            // Rivals are the things of one kind that could be done next:
            // a tool against another tool. The command and the file are
            // part of whichever is chosen, and go with it.
            if neuron.kind != Kind::Sense
                || !self.is_choice(&neuron.label)
                || prediction.confidence < caution
                || segment.tried() < MIN_RECORD
            {
                continue;
            }

            // Imagine it done: itself, with what is firing now behind it.
            let start = self.imagined_on(&context, &[neuron.id]);

            let thoughts = self.imagine_from(&start, IMAGINED);

            let mut worth = neuron.value;
            let mut weight = 1.0;

            for thought in &thoughts {
                weight *= DISCOUNT;
                worth += weight * thought.worth;
            }

            let reason = self.reason(neuron, segment, prediction.confidence);

            candidates.push((
                worth,
                Hunch {
                    kind: HunchKind::Next,
                    label: neuron.label.clone(),
                    with: Vec::new(),
                    because: reason.because,
                    confidence: prediction.confidence,
                    hits: segment.hits,
                    misses: segment.misses,
                    imagined: thoughts
                        .iter()
                        .flat_map(|thought| thought.labels.iter().cloned())
                        .collect(),
                    worth,
                },
                neuron.id,
            ));
        }

        candidates.sort_by(|left, right| right.0.total_cmp(&left.0));

        let clear = match candidates.as_slice() {
            [] => false,
            [(worth, ..)] => *worth > 0.0,
            [(best, ..), (next, ..), ..] => *best > 0.0 && best - next >= MARGIN,
        };

        if clear {
            let (_, mut hunch, id) = candidates.swap_remove(0);

            if self.advised == Some(id)
                || self
                    .last_hunch
                    .as_ref()
                    .is_some_and(|last| last.kind == hunch.kind && last.label == hunch.label)
            {
                return None;
            }

            // What it is done to: the file, the command.
            hunch.with = predictions
                .iter()
                .filter(|prediction| prediction.id != id && prediction.confidence >= caution)
                .filter_map(|prediction| self.neuron(prediction.id))
                .filter(|neuron| neuron.kind == Kind::Sense && !self.is_choice(&neuron.label))
                .map(|neuron| neuron.label.clone())
                .take(2)
                .collect();

            self.advised = Some(id);
            self.meters.advised += 1;
            self.last_hunch = Some(hunch.clone());

            return Some(hunch);
        }

        self.warn(&context, &predictions)
    }

    /// The usual way on from here has usually led to bad news.
    fn warn(&mut self, context: &[(Id, Lag)], predictions: &[Prediction]) -> Option<Hunch> {
        let caution = self.meters.caution;

        let thoughts = self.imagine_from(context, IMAGINED);

        let first = thoughts.first()?;

        let bad = thoughts
            .iter()
            .position(|thought| thought.worth <= -0.25 && thought.confidence >= caution * 0.8)?;

        let news = thoughts[bad]
            .labels
            .iter()
            .min_by(|left, right| self.worth_of(left).total_cmp(&self.worth_of(right)))?
            .clone();

        // The record is that of the surest thing it expects next.
        let surest = predictions.first()?;
        let neuron = self.neuron(surest.id)?;
        let segment = &neuron.segments[surest.segment];

        if segment.tried() < MIN_RECORD {
            return None;
        }

        let hunch = Hunch {
            kind: HunchKind::Warning,
            label: news,
            with: first.labels.clone(),
            because: self.reason(neuron, segment, surest.confidence).because,
            confidence: thoughts[bad].confidence,
            hits: segment.hits,
            misses: segment.misses,
            imagined: thoughts
                .iter()
                .flat_map(|thought| thought.labels.iter().cloned())
                .collect(),
            worth: thoughts.iter().map(|thought| thought.worth).sum(),
        };

        if self
            .last_hunch
            .as_ref()
            .is_some_and(|last| last.kind == hunch.kind && last.label == hunch.label)
        {
            return None;
        }

        self.last_hunch = Some(hunch.clone());

        Some(hunch)
    }

    /// The percept with this beginning that is most surely expected next,
    /// if it is at least as sure as `least` and its branch has been right
    /// more than once.
    pub fn foresee(&self, prefix: &str, least: f32) -> Option<(String, f32)> {
        let active: Context = self.context().into_iter().collect();

        self.predict(&active)
            .into_iter()
            .filter_map(|prediction| {
                let neuron = self.neuron(prediction.id)?;
                let segment = &neuron.segments[prediction.segment];

                (neuron.label.starts_with(prefix)
                    && prediction.confidence >= least
                    && segment.hits >= 2)
                    .then(|| (neuron.label.clone(), prediction.confidence))
            })
            .next()
    }

    /// A new run begins: what was said in the last one may be said again.
    pub fn begin(&mut self) {
        self.last_hunch = None;
        self.advised = None;
    }

    /// Holds what was expected against what came, and teaches the
    /// self-model. Returns the surprise.
    fn judge_expectations(&mut self, sensed: &[Id]) -> f32 {
        let expected: HashSet<Id> = self.expected.iter().copied().collect();

        let met = sensed.iter().filter(|id| expected.contains(id)).count() as f32;
        let right = met / sensed.len() as f32;
        let surprise = 1.0 - right;

        // The same number of guesses, made from how often each neuron has
        // fired and nothing else.
        let mut common: Vec<(u32, Id)> = self
            .neurons
            .iter()
            .filter(|neuron| neuron.kind == Kind::Sense)
            .map(|neuron| (neuron.fired, neuron.id))
            .collect();

        common.sort_unstable_by(|left, right| right.cmp(left));

        let guesses: HashSet<Id> = common
            .iter()
            .take(expected.len())
            .map(|(_, id)| *id)
            .collect();

        let baseline =
            sensed.iter().filter(|id| guesses.contains(id)).count() as f32 / sensed.len() as f32;

        average(&mut self.meters.surprise, surprise);
        average(&mut self.meters.expected_right, right);
        average(&mut self.meters.baseline_right, baseline);

        self.meters.settled_surprise += AVERAGING * 0.1 * (surprise - self.meters.settled_surprise);

        if !expected.is_empty() {
            average(&mut self.meters.precision, met / expected.len() as f32);
        }

        if let Some(expected_surprise) = self.expected_surprise {
            average(
                &mut self.meters.self_accuracy,
                1.0 - (expected_surprise - surprise).abs(),
            );
        }

        // Whatever held the workspace learns how the moment after went.
        let held: Vec<Id> = self.workspace.iter().map(|(id, _)| *id).collect();

        for id in held {
            if let Some(neuron) = self.neuron_mut(id) {
                neuron.expects_surprise += 0.2 * (surprise - neuron.expects_surprise);
            }
        }

        surprise
    }

    /// Holds the last advice against what came. Being wrong makes the
    /// network more cautious at once; being right, less so slowly.
    fn judge_advice(&mut self, sensed: &[Id]) -> Option<bool> {
        let advised = self.advised.take()?;
        let right = sensed.contains(&advised);

        average(&mut self.meters.advice_right, if right { 1.0 } else { 0.0 });

        let caution = &mut self.meters.caution;

        *caution = if right {
            (*caution - 0.005).max(0.6)
        } else {
            (*caution + 0.03).min(0.95)
        };

        Some(right)
    }

    /// Good or bad news reaches the neurons that fired before it.
    fn credit(&mut self, sensed: &[Id]) {
        let news: f32 = self
            .labels(sensed)
            .iter()
            .map(|label| self.worth_of(label))
            .sum::<f32>()
            .clamp(-1.0, 1.0);

        if news == 0.0 {
            return;
        }

        let mut rate = VALUE_RATE * self.meters.plasticity;

        let trace: Vec<Vec<Id>> = self.trace.iter().rev().cloned().collect();

        for fired in trace {
            for id in fired {
                if let Some(neuron) = self.neuron_mut(id) {
                    neuron.value += rate * (news - neuron.value);
                }
            }

            rate *= CREDIT;
        }
    }

    /// The assemblies the sensed neurons fire, growing one for a set that
    /// has fired together often enough and has none.
    fn assemble(&mut self, sensed: &[Id]) -> Vec<Id> {
        let mut firing = self.firing_assemblies(sensed);

        let mut set: Vec<Id> = sensed.to_vec();

        set.sort_unstable();
        set.dedup();

        if set.len() < 2 || set.len() > 4 {
            return firing;
        }

        if self.together.len() > 4_000 {
            self.together.retain(|_, (_, assembly)| assembly.is_some());
        }

        let entry = self.together.entry(set.clone()).or_insert((0, None));

        entry.0 = entry.0.saturating_add(1);

        let (times, assembly) = *entry;

        if times >= TOGETHER && assembly.is_none() {
            let label = self.labels(&set).join(" + ");
            let reason = format!("these fired together {times} times");

            let id = self.grow(Kind::Assembly, &label, &reason, set.clone());

            self.together.insert(set, (TOGETHER, Some(id)));

            firing.push(id);
        }

        firing
    }

    /// The assemblies enough of whose members are among `sensed`.
    fn firing_assemblies(&self, sensed: &[Id]) -> Vec<Id> {
        let sensed: HashSet<Id> = sensed.iter().copied().collect();

        self.neurons
            .iter()
            .filter(|neuron| neuron.kind == Kind::Assembly && !neuron.members.is_empty())
            .filter(|neuron| {
                let present = neuron
                    .members
                    .iter()
                    .filter(|member| sensed.contains(member))
                    .count() as f32;

                present / neuron.members.len() as f32 >= QUORUM
            })
            .map(|neuron| neuron.id)
            .collect()
    }

    /// One moment's firing: teach the branches, choose what the workspace
    /// holds, and say what is expected next. The first `world` of
    /// `firing` are what the world did. `strength` is 1 for a lived
    /// moment and less for a replayed one, which also counts toward no
    /// record and grows nothing.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &mut self,
        firing: &[Id],
        world: usize,
        sensed: &[Id],
        assembled: &[Id],
        surprise: f32,
        strength: f32,
        lived: bool,
    ) {
        let now = self.moment;

        for id in firing {
            if let Some(neuron) = self.neuron_mut(*id) {
                neuron.fired = neuron.fired.saturating_add(1);
                neuron.last_fired = now;
            }
        }

        let before = self.context();

        self.learn(&before, firing, strength, lived);

        let unexpected: HashSet<Id> = sensed
            .iter()
            .filter(|id| !self.expected.contains(id))
            .copied()
            .collect();

        self.compete(firing, assembled, &unexpected, surprise);

        let latest = self.fired_world.min(self.fired.len());

        self.prior = self.fired[..latest].to_vec();
        self.fired = firing.to_vec();
        self.fired_world = world;

        if lived {
            self.trace.push_back(firing[..world].to_vec());

            while self.trace.len() > TRACE {
                self.trace.pop_front();
            }
        }

        self.expect();
    }

    /// What branches see, each with when it fired: what the world did in
    /// the latest moment, what it did in the one before, what the
    /// workspace still holds from earlier ones, then the network's own
    /// states. A new branch takes its synapses from the front.
    fn context(&self) -> Vec<(Id, Lag)> {
        let world = self.fired_world.min(self.fired.len());

        let mut context: Vec<(Id, Lag)> =
            self.fired[..world].iter().map(|id| (*id, LATEST)).collect();

        context.extend(self.prior.iter().map(|id| (*id, EARLIER)));

        for (id, _) in &self.workspace {
            if !self.fired.contains(id) && !self.prior.contains(id) {
                context.push((*id, HELD));
            }
        }

        context.extend(self.fired[world..].iter().map(|id| (*id, LATEST)));

        context
    }

    /// Teaches every neuron's branches what `before` was followed by.
    ///
    /// For a neuron that fired: every branch that was active was right,
    /// and is strengthened. If none was, the branch nearest to active is
    /// bent toward what fired this time, unless it has proven itself as
    /// it is: then it is left alone and a new branch is grown from what
    /// the two situations had in common. With no branch near, a new one
    /// is grown from what had just fired.
    ///
    /// For a neuron that did not fire: every branch that was active was
    /// wrong, and is weakened.
    fn learn(&mut self, before: &[(Id, Lag)], firing: &[Id], strength: f32, lived: bool) {
        if before.is_empty() {
            return;
        }

        let active: Context = before.iter().copied().collect();
        let now: HashSet<Id> = firing.iter().copied().collect();

        let rate = (LEARNING * strength * self.meters.plasticity).min(0.9);
        let depression = DEPRESSION * strength * self.meters.plasticity;
        let moment = self.moment;

        let mut formed = 0;
        let mut branched = 0;
        let mut pruned = 0;

        for neuron in &mut self.neurons {
            let fires = now.contains(&neuron.id);

            let mut foreseen = false;

            // The branch most of which was driven without being active.
            let mut nearest: Option<(usize, f32)> = None;

            for (at, segment) in neuron.segments.iter_mut().enumerate() {
                let (matched, total) = segment.drive(&active);

                if total <= 0.0 {
                    continue;
                }

                let share = matched / total;
                let was_active = share >= QUORUM && matched >= DRIVE;

                let own = segment.plasticity();
                let fade = 1.0 - depression * own;

                match (was_active, fires) {
                    (true, true) => {
                        foreseen = true;

                        if lived {
                            segment.hits += 1;
                        }

                        segment.last_active = moment;

                        for synapse in &mut segment.synapses {
                            if synapse.driven(&active) {
                                synapse.weight += rate * own * (1.0 - synapse.weight);
                            } else {
                                synapse.weight *= fade;
                            }
                        }
                    }

                    // It promised its neuron and the neuron did not come.
                    (true, false) => {
                        if lived {
                            segment.misses += 1;
                        }

                        // The whole branch made the promise. Its record
                        // is what says how far to trust it; its synapses
                        // only fade a little, so that one that is wrong
                        // for good goes in the end.
                        let fade = 1.0 - depression * own * MISS;

                        for synapse in &mut segment.synapses {
                            synapse.weight *= fade;
                        }
                    }

                    (false, true) => {
                        if share > nearest.map_or(0.0, |(_, best)| best) {
                            nearest = Some((at, share));
                        }
                    }

                    (false, false) => {}
                }
            }

            if !fires || foreseen {
                continue;
            }

            let near = nearest.filter(|(_, share)| *share >= MATCHING);

            let grown: Vec<(Id, Lag)> = match near {
                Some((at, _)) if neuron.segments[at].hits < PROVEN || !lived => {
                    let segment = &mut neuron.segments[at];

                    segment.last_active = moment;

                    // Replay strengthens what is there and removes nothing.
                    let shed = if lived { 1.0 - BENDING } else { 1.0 };

                    for synapse in &mut segment.synapses {
                        if synapse.driven(&active) {
                            synapse.weight += rate * (1.0 - synapse.weight);
                        } else {
                            synapse.weight *= shed;
                        }
                    }

                    continue;
                }

                // What this situation has in common with the proven one.
                Some((at, _)) => neuron.segments[at]
                    .synapses
                    .iter()
                    .filter(|synapse| synapse.driven(&active))
                    .map(|synapse| (synapse.from, synapse.lag))
                    .collect(),

                None if lived => before
                    .iter()
                    .filter(|(source, _)| *source != neuron.id)
                    .take(SEGMENT_SYNAPSES)
                    .copied()
                    .collect(),

                None => continue,
            };

            if grown.is_empty() {
                continue;
            }

            formed += grown.len();
            branched += 1;

            neuron.segments.push(Segment {
                synapses: grown
                    .into_iter()
                    .map(|(from, lag)| Synapse {
                        from,
                        lag,
                        weight: rate,
                    })
                    .collect(),
                hits: 0,
                misses: 0,
                last_active: moment,
            });

            if neuron.segments.len() > MAX_SEGMENTS {
                // The least proven goes, the longest idle first. Not the
                // one just grown.
                let weakest = neuron
                    .segments
                    .iter()
                    .enumerate()
                    .take(MAX_SEGMENTS)
                    .min_by(|(_, left), (_, right)| {
                        left.confidence()
                            .total_cmp(&right.confidence())
                            .then(left.last_active.cmp(&right.last_active))
                    })
                    .map(|(at, _)| at);

                if let Some(at) = weakest {
                    neuron.segments.remove(at);

                    pruned += 1;
                }
            }
        }

        self.meters.connected += formed as u64;
        self.meters.branched += branched;
        self.meters.pruned += pruned;
    }

    /// The firing neurons compete for the workspace with what it holds.
    fn compete(
        &mut self,
        firing: &[Id],
        assembled: &[Id],
        unexpected: &HashSet<Id>,
        surprise: f32,
    ) {
        let now = self.moment;

        let mut candidates: HashMap<Id, f32> = self
            .workspace
            .iter()
            .map(|(id, salience)| (*id, salience * HOLD))
            .collect();

        for id in firing {
            let Some(neuron) = self.neuron(*id) else {
                continue;
            };

            let mut salience = 0.2;

            if neuron.born == now {
                salience += 1.0;
            }

            if unexpected.contains(id) {
                salience += 0.6 * surprise.max(0.5);
            }

            if assembled.contains(id) {
                salience += 0.3;
            }

            // News is kept in mind for a while.
            salience += 0.4 * self.worth_of(&neuron.label).abs();

            let held = candidates.entry(*id).or_insert(0.0);

            *held = held.max(salience);
        }

        let mut ranked: Vec<(Id, f32)> = candidates
            .into_iter()
            .filter(|(id, salience)| *salience > 0.1 && self.index.contains_key(id))
            .collect();

        ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
        ranked.truncate(WORKSPACE);

        self.workspace = ranked;

        // How much of the network the workspace speaks to.
        let held: HashSet<Id> = self.workspace.iter().map(|(id, _)| *id).collect();

        let reached = self
            .neurons
            .iter()
            .filter(|neuron| {
                neuron
                    .segments
                    .iter()
                    .flat_map(|segment| &segment.synapses)
                    .any(|synapse| held.contains(&synapse.from))
            })
            .count();

        average(
            &mut self.meters.reach,
            reached as f32 / self.neurons.len().max(1) as f32,
        );
    }

    /// Every neuron with a branch that `active` drives, the surest first.
    fn predict(&self, active: &Context) -> Vec<Prediction> {
        let mut predictions: Vec<Prediction> = Vec::new();

        for neuron in &self.neurons {
            let mut best: Option<(usize, f32)> = None;

            for (at, segment) in neuron.segments.iter().enumerate() {
                let (matched, total) = segment.drive(active);

                if total <= 0.0 || matched < DRIVE || matched / total < QUORUM {
                    continue;
                }

                let confidence = segment.confidence() * (matched / total);

                if confidence > best.map_or(0.0, |(_, best)| best) {
                    best = Some((at, confidence));
                }
            }

            if let Some((segment, confidence)) = best {
                predictions.push(Prediction {
                    id: neuron.id,
                    confidence,
                    segment,
                });
            }
        }

        predictions.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then(left.id.cmp(&right.id))
        });

        predictions
    }

    /// The predictions that count as expectations: of the world, sure
    /// enough, a few, and none much less sure than the surest.
    fn chosen(&self, predictions: Vec<Prediction>) -> Vec<Prediction> {
        let threshold = self.meters.threshold;

        let mut chosen: Vec<Prediction> = predictions
            .into_iter()
            .filter(|prediction| {
                prediction.confidence >= threshold
                    && self
                        .neuron(prediction.id)
                        .is_some_and(|neuron| neuron.kind == Kind::Sense)
            })
            .take(EXPECTATIONS)
            .collect();

        // Expecting everything a little is expecting nothing.
        if let Some(surest) = chosen.first().map(|prediction| prediction.confidence) {
            chosen.retain(|prediction| prediction.confidence >= surest * RIVAL);
        }

        chosen
    }

    /// What the firing neurons and the workspace lead to.
    fn expect(&mut self) {
        let active: Context = self.context().into_iter().collect();

        let chosen = self.chosen(self.predict(&active));

        self.reasons = chosen
            .iter()
            .filter_map(|prediction| {
                let neuron = self.neuron(prediction.id)?;

                Some(self.reason(
                    neuron,
                    &neuron.segments[prediction.segment],
                    prediction.confidence,
                ))
            })
            .collect();

        self.expected = chosen.into_iter().map(|prediction| prediction.id).collect();

        // The self-model's guess: how the moments after these neurons
        // held the workspace have gone before.
        let held: Vec<f32> = self
            .workspace
            .iter()
            .filter_map(|(id, _)| self.neuron(*id))
            .map(|neuron| neuron.expects_surprise)
            .collect();

        self.expected_surprise =
            (!held.is_empty()).then(|| held.iter().sum::<f32>() / held.len() as f32);
    }

    fn reason(&self, neuron: &Neuron, segment: &Segment, confidence: f32) -> Reason {
        let mut sources: Vec<&Synapse> = segment.synapses.iter().collect();

        sources.sort_by(|left, right| right.weight.total_cmp(&left.weight));

        Reason {
            expected: neuron.label.clone(),
            because: sources
                .iter()
                .filter_map(|synapse| self.neuron(synapse.from))
                .map(|source| source.label.clone())
                .collect(),
            confidence,
            hits: segment.hits,
            misses: segment.misses,
        }
    }

    /// The imagined way on and the way to the goal, kept for whoever
    /// draws the network.
    fn think(&mut self) {
        self.imagined = self.imagine(IMAGINED);

        // The best news it has a branch for.
        let mut news: Vec<(&String, &f32)> = self
            .worth
            .iter()
            .filter(|(_, worth)| **worth > 0.0)
            .collect();

        news.sort_by(|left, right| right.1.total_cmp(left.1).then(left.0.cmp(right.0)));

        self.goal = news
            .into_iter()
            .map(|(label, _)| label)
            .find(|label| {
                self.by_label
                    .get(*label)
                    .and_then(|id| self.neuron(*id))
                    .is_some_and(|neuron| neuron.segments.iter().any(|segment| segment.hits > 0))
            })
            .cloned();

        self.plan = match &self.goal {
            Some(goal) => self.plan(goal, 5),
            None => Vec::new(),
        };
    }

    /// The network sets its own rates from its own record.
    fn tune(&mut self) {
        // Too little has happened to judge by.
        if self.meters.moments < 100 {
            return;
        }

        let meters = &mut self.meters;

        let before = (meters.plasticity, meters.threshold);

        // More surprised than usual: things have changed, learn faster.
        // Less: what it knows holds, change it more slowly.
        if meters.surprise > meters.settled_surprise + 0.05 {
            meters.plasticity = (meters.plasticity * 1.1).min(1.6);
        } else if meters.surprise < meters.settled_surprise - 0.05 {
            meters.plasticity = (meters.plasticity * 0.95).max(0.6);
        }

        // Expecting much that does not come: ask more of a branch.
        // Expecting little, and rightly: ask less.
        if meters.precision < 0.4 {
            meters.threshold = (meters.threshold + 0.02).min(0.8);
        } else if meters.precision > 0.65 && meters.expected_right < 0.5 {
            meters.threshold = (meters.threshold - 0.02).max(0.25);
        }

        let (plasticity, threshold) = (meters.plasticity, meters.threshold);

        if (plasticity - before.0).abs() >= 0.05 {
            let how = if plasticity > before.0 {
                "more surprised than usual, learning faster"
            } else {
                "less surprised than usual, learning more slowly"
            };

            self.log_line(format!("tuned itself: {how} ({plasticity:.2})"));
        }

        if threshold != before.1 {
            let how = if threshold > before.1 {
                "too much expected that did not come, asking more of a branch"
            } else {
                "expecting little and rightly, asking less of a branch"
            };

            self.log_line(format!("tuned itself: {how} ({threshold:.2})"));
        }
    }

    /// Decay, and removal of what is no longer used.
    fn tidy(&mut self) {
        let now = self.moment;

        let mut dropped = 0;
        let mut pruned = 0;

        for neuron in &mut self.neurons {
            for segment in &mut neuron.segments {
                let before = segment.synapses.len();

                for synapse in &mut segment.synapses {
                    synapse.weight *= 1.0 - DECAY;
                }

                segment.synapses.retain(|synapse| synapse.weight >= WEAKEST);

                dropped += before - segment.synapses.len();
            }

            let before = neuron.segments.len();

            neuron.segments.retain(|segment| {
                let hopeless = segment.misses >= HOPELESS && segment.misses > segment.hits * 4;

                if hopeless {
                    dropped += segment.synapses.len();
                }

                !segment.synapses.is_empty() && !hopeless
            });

            pruned += before - neuron.segments.len();
        }

        self.meters.disconnected += dropped as u64;
        self.meters.pruned += pruned as u64;

        let mut gone: HashSet<Id> = self
            .neurons
            .iter()
            .filter(|neuron| match neuron.kind {
                Kind::Own => false,
                Kind::Assembly => {
                    (neuron.fired <= 1 && now - neuron.born > UNPROVEN)
                        || now - neuron.last_fired > FORGOTTEN
                }
                Kind::Sense => now - neuron.last_fired > FORGOTTEN,
            })
            .map(|neuron| neuron.id)
            .collect();

        if self.neurons.len() - gone.len() > MAX_NEURONS {
            let mut silent: Vec<(u64, Id)> = self
                .neurons
                .iter()
                .filter(|neuron| neuron.kind != Kind::Own && !gone.contains(&neuron.id))
                .map(|neuron| (neuron.last_fired, neuron.id))
                .collect();

            silent.sort_unstable();

            let excess = self.neurons.len() - gone.len() - MAX_NEURONS;

            gone.extend(silent.into_iter().take(excess).map(|(_, id)| id));
        }

        if gone.is_empty() {
            return;
        }

        // An assembly whose member is gone can never fire again.
        for neuron in &self.neurons {
            if neuron.members.iter().any(|member| gone.contains(member)) {
                gone.insert(neuron.id);
            }
        }

        let labels: Vec<String> = self
            .neurons
            .iter()
            .filter(|neuron| gone.contains(&neuron.id))
            .map(|neuron| neuron.label.clone())
            .collect();

        for label in labels {
            self.log_line(format!("removed `{label}`: long unused"));
        }

        self.neurons.retain(|neuron| !gone.contains(&neuron.id));

        let mut cut = 0;

        for neuron in &mut self.neurons {
            for segment in &mut neuron.segments {
                let before = segment.synapses.len();

                segment
                    .synapses
                    .retain(|synapse| !gone.contains(&synapse.from));

                cut += before - segment.synapses.len();
            }

            neuron
                .segments
                .retain(|segment| !segment.synapses.is_empty());
        }

        self.meters.removed += gone.len() as u64;
        self.meters.disconnected += cut as u64;

        self.workspace.retain(|(id, _)| !gone.contains(id));
        let world = self.fired_world.min(self.fired.len());
        let lost = self.fired[..world]
            .iter()
            .filter(|id| gone.contains(id))
            .count();

        self.fired.retain(|id| !gone.contains(id));
        self.fired_world = world - lost;
        self.prior.retain(|id| !gone.contains(id));

        for fired in &mut self.trace {
            fired.retain(|id| !gone.contains(id));
        }

        self.together.retain(|set, (_, assembly)| {
            !set.iter().any(|id| gone.contains(id)) && assembly.is_none_or(|id| !gone.contains(&id))
        });

        self.rebuild_index();
    }

    /// The sense neuron for `percept`, grown if nothing has met it.
    fn sense(&mut self, percept: &str) -> Id {
        if let Some(id) = self.by_label.get(percept) {
            return *id;
        }

        self.grow(
            Kind::Sense,
            percept,
            "nothing had met this before",
            Vec::new(),
        )
    }

    fn grow(&mut self, kind: Kind, label: &str, reason: &str, members: Vec<Id>) -> Id {
        self.next_id += 1;

        let id = self.next_id;

        self.neurons.push(Neuron {
            id,
            kind,
            label: label.to_string(),
            born: self.moment,
            reason: reason.to_string(),
            fired: 0,
            last_fired: self.moment,
            members,
            expects_surprise: 0.5,
            value: 0.0,
            segments: Vec::new(),
            out: Vec::new(),
        });

        self.index.insert(id, self.neurons.len() - 1);
        self.by_label.insert(label.to_string(), id);

        if kind != Kind::Own {
            self.meters.grown += 1;

            self.log_line(format!("grew `{label}`: {reason}"));
        }

        id
    }

    fn log_line(&mut self, text: String) {
        self.log.push_back(format!("{}  {text}", self.moment));

        while self.log.len() > LOG {
            self.log.pop_front();
        }
    }

    fn rebuild_index(&mut self) {
        self.index = self
            .neurons
            .iter()
            .enumerate()
            .map(|(at, neuron)| (neuron.id, at))
            .collect();

        self.by_label = self
            .neurons
            .iter()
            .map(|neuron| (neuron.label.clone(), neuron.id))
            .collect();
    }

    fn neuron(&self, id: Id) -> Option<&Neuron> {
        self.index.get(&id).map(|at| &self.neurons[*at])
    }

    fn neuron_mut(&mut self, id: Id) -> Option<&mut Neuron> {
        let at = *self.index.get(&id)?;

        Some(&mut self.neurons[at])
    }

    fn labels(&self, ids: &[Id]) -> Vec<String> {
        ids.iter()
            .filter_map(|id| self.neuron(*id))
            .map(|neuron| neuron.label.clone())
            .collect()
    }

    fn worth_of(&self, label: &str) -> f32 {
        self.worth.get(label).copied().unwrap_or(0.0)
    }

    fn is_action(&self, label: &str) -> bool {
        self.actions.iter().any(|prefix| label.starts_with(prefix))
    }

    /// Whether the percept is one of the things that are chosen between.
    fn is_choice(&self, label: &str) -> bool {
        self.actions
            .first()
            .is_some_and(|prefix| label.starts_with(prefix))
    }

    fn count(&mut self) {
        self.meters.neurons = self.neurons.len();

        self.meters.segments = self
            .neurons
            .iter()
            .map(|neuron| neuron.segments.len())
            .sum();

        self.meters.synapses = self
            .neurons
            .iter()
            .flat_map(|neuron| &neuron.segments)
            .map(|segment| segment.synapses.len())
            .sum();
    }

    /// A number that differs from draw to draw, without a dependency.
    fn draw(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;

        self.random
    }

    pub fn snapshot(&self) -> Snapshot {
        let held: HashSet<Id> = self.workspace.iter().map(|(id, _)| *id).collect();

        let neurons = self
            .neurons
            .iter()
            .map(|neuron| NeuronView {
                id: neuron.id,
                kind: neuron.kind,
                label: neuron.label.clone(),
                born: neuron.born,
                reason: neuron.reason.clone(),
                fired: neuron.fired,
                last_fired: neuron.last_fired,
                glow: if neuron.fired == 0 {
                    0.0
                } else {
                    (1.0 - (self.moment - neuron.last_fired) as f32 / 6.0).max(0.0)
                },
                in_workspace: held.contains(&neuron.id),
                expects_surprise: neuron.expects_surprise,
                value: neuron.value,
                segments: neuron.segments.len() as u32,
            })
            .collect();

        // One line between two neurons, however many branches join them.
        let mut strongest: HashMap<(Id, Id), f32> = HashMap::new();

        for neuron in &self.neurons {
            for synapse in neuron.segments.iter().flat_map(|segment| &segment.synapses) {
                let weight = strongest.entry((synapse.from, neuron.id)).or_insert(0.0);

                *weight = weight.max(synapse.weight);
            }
        }

        let mut synapses: Vec<(Id, Id, f32)> = strongest
            .into_iter()
            .map(|((from, to), weight)| (from, to, weight))
            .collect();

        synapses.sort_by_key(|synapse| (synapse.0, synapse.1));

        let mut meters = self.meters.clone();

        meters.neurons = self.neurons.len();
        meters.segments = self
            .neurons
            .iter()
            .map(|neuron| neuron.segments.len())
            .sum();
        meters.synapses = self
            .neurons
            .iter()
            .flat_map(|neuron| &neuron.segments)
            .map(|segment| segment.synapses.len())
            .sum();

        Snapshot {
            moment: self.moment,
            neurons,
            synapses,
            workspace: self.labels(&held_in_order(&self.workspace)),
            expecting: self.expecting(),
            reasons: self.reasons.clone(),
            expected_surprise: self.expected_surprise,
            imagined: self.imagined.clone(),
            goal: self.goal.clone(),
            plan: self.plan.clone(),
            hunch: self.last_hunch.clone(),
            meters,
            history: self.history.iter().copied().collect(),
            log: self.log.iter().cloned().collect(),
            resting: self.resting,
        }
    }
}

fn held_in_order(workspace: &[(Id, f32)]) -> Vec<Id> {
    workspace.iter().map(|(id, _)| *id).collect()
}

fn average(running: &mut f32, reading: f32) {
    *running += AVERAGING * (reading - *running);
}

#[cfg(test)]
mod tests;
