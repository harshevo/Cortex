//! The network: neurons that are created when they are needed, synapses
//! that form between what fires in sequence, and a few mechanisms on top.
//!
//! Time moves in *moments*. A moment is one thing happening: a tool being
//! called, its result, something the person said. Each moment arrives as
//! a handful of percepts, short texts such as `does: edit_file` or
//! `file: src/fees.rs`.
//!
//! - **Growth.** A percept nothing has met before gets a *sense* neuron.
//!   A set of sense neurons that keeps firing in the same moment gets an
//!   *assembly* neuron that fires when most of the set does, so the
//!   network can tell "an edit to this file that worked" from its parts.
//! - **Wiring.** Whatever fired in one moment grows a synapse onto
//!   whatever fires in the next, or strengthens the one it has. A synapse
//!   whose source fired and whose target then did not is weakened. Unused
//!   synapses decay and are removed, and neurons nothing uses are too.
//! - **Prediction.** Before each moment the network says what it expects
//!   from what just fired. How much of the moment it failed to expect is
//!   its *surprise*.
//! - **Workspace.** The firing neurons compete; the few most salient (the
//!   new, the unexpected, the assemblies) are held for a while. What is
//!   held wires to whatever comes next, wherever it is in the network:
//!   that is the one thing every part of the network has access to.
//! - **Self-model.** A few neurons stand for the network's own state ("I
//!   was surprised", "I grew"). They fire and wire like any other, so the
//!   network learns what in the world goes with which of its own states.
//!   Each neuron also keeps how surprising the moments after it held the
//!   workspace turned out, and from that the network predicts its own
//!   next surprise. How close that prediction comes is measured.
//! - **Rest.** With nothing happening, recent moments are replayed at half
//!   strength, a limited number of times each.
//!
//! None of this is evidence of experience. Each quantity shown is a
//! measurement of the mechanism named, and nothing more is claimed.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

pub type Id = u64;

/// Share of the remaining headroom a synapse gains when its source fired
/// in one moment and its target in the next.
const LEARNING: f32 = 0.25;

/// Share of its weight a synapse loses when its source fired and its
/// target did not follow.
const DEPRESSION: f32 = 0.08;

/// Share of its weight every synapse loses at each tidy.
const DECAY: f32 = 0.01;

/// Moments between tidies.
const TIDY_EVERY: u64 = 20;

/// Synapses weaker than this are removed.
const WEAKEST: f32 = 0.05;

/// Most synapses leaving one neuron; the weakest go first.
const MAX_OUT: usize = 24;

/// Neurons in one moment that may grow new synapses, on either side.
const WIRING: usize = 6;

/// Times a set of sense neurons must fire together before it gets an
/// assembly neuron.
const TOGETHER: u8 = 2;

/// Share of an assembly's members that must fire for it to fire.
const QUORUM: f32 = 0.75;

/// Neurons held in the workspace.
const WORKSPACE: usize = 4;

/// Share of its salience a held neuron keeps from one moment to the next.
const HOLD: f32 = 0.6;

/// Input from the firing neurons at which a neuron is expected next.
const EXPECTED: f32 = 0.45;

/// An expected neuron must be driven at least this share as hard as the
/// most driven one.
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

/// The network's own states. Each has a neuron from the start.
const SURPRISED: &str = "I was surprised";
const EXPECTED_IT: &str = "I expected this";
const GREW: &str = "I grew";
const RESTING: &str = "I am resting";

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Synapse {
    to: Id,
    weight: f32,
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

    out: Vec<Synapse>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meters {
    pub neurons: usize,
    pub synapses: usize,

    /// Neurons created and removed, synapses formed and removed.
    pub grown: u64,
    pub removed: u64,
    pub connected: u64,
    pub disconnected: u64,

    /// Running averages over live moments, each from 0 to 1.
    ///
    /// `surprise`: the share of a moment that was not expected.
    /// `expected_right`: the share of a moment the network had expected.
    /// `baseline_right`: the same for guessing the most often fired
    /// neurons, as many guesses as the network made. The network has
    /// learned something only where it beats this.
    /// `self_accuracy`: how close its prediction of its own surprise came.
    /// `reach`: the share of the network the workspace has synapses onto.
    pub surprise: f32,
    pub expected_right: f32,
    pub baseline_right: f32,
    pub self_accuracy: f32,
    pub reach: f32,

    /// Moments lived, and moments replayed at rest.
    pub moments: u64,
    pub replays: u64,
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

    /// What fired in the latest moment.
    #[serde(skip)]
    fired: Vec<Id>,

    /// What the network expects of the next moment, and how surprised it
    /// expects to be.
    #[serde(skip)]
    expected: Vec<Id>,
    #[serde(skip)]
    expected_surprise: Option<f32>,

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
}

/// The whole network at one moment, for drawing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub moment: u64,
    pub neurons: Vec<NeuronView>,

    /// Source, target, weight.
    pub synapses: Vec<(Id, Id, f32)>,

    /// The labels of what the workspace holds, most salient first.
    pub workspace: Vec<String>,

    /// What it expects next, and how surprised it expects to be.
    pub expecting: Vec<String>,
    pub expected_surprise: Option<f32>,

    pub meters: Meters,
    pub history: Vec<Sample>,

    /// What changed lately, newest last.
    pub log: Vec<String>,

    pub resting: bool,
}

impl Network {
    pub fn new() -> Self {
        let mut network = Self::default();

        network.wake();

        network
    }

    /// Puts a loaded network back in working order.
    pub fn wake(&mut self) {
        self.rebuild_index();

        for state in [SURPRISED, EXPECTED_IT, GREW, RESTING] {
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

    pub fn moment(&self) -> u64 {
        self.moment
    }

    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    /// Whether a neuron with this label exists.
    pub fn knows(&self, label: &str) -> bool {
        self.by_label.contains_key(label)
    }

    /// The weight of the synapse between two labelled neurons, 0 if there
    /// is none.
    pub fn weight(&self, from: &str, to: &str) -> f32 {
        let (Some(from), Some(to)) = (self.by_label.get(from), self.by_label.get(to)) else {
            return 0.0;
        };

        self.neuron(*from)
            .and_then(|neuron| neuron.out.iter().find(|synapse| synapse.to == *to))
            .map_or(0.0, |synapse| synapse.weight)
    }

    /// The labels of what is expected of the next moment.
    pub fn expecting(&self) -> Vec<String> {
        self.labels(&self.expected)
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

        // The network's own states fire with the moment they describe.
        let mut own = vec![if surprise >= 0.5 {
            SURPRISED
        } else {
            EXPECTED_IT
        }];

        if grew {
            own.push(GREW);
        }

        let mut firing = sensed.clone();

        firing.extend(own.iter().filter_map(|state| self.by_label.get(*state)));

        let assembled = self.assemble(&sensed);

        firing.extend(&assembled);

        self.pass(&firing, &sensed, &assembled, surprise, 1.0);

        self.remembered.push_back(Remembered {
            percepts: percepts.to_vec(),
            replayed: 0,
        });

        while self.remembered.len() > REMEMBERED {
            self.remembered.pop_front();
        }

        if self.moment.is_multiple_of(TIDY_EVERY) {
            self.tidy();
        }

        self.meters.neurons = self.neurons.len();
        self.meters.synapses = self.synapse_count();

        self.history.push_back(Sample {
            neurons: self.meters.neurons as u32,
            synapses: self.meters.synapses as u32,
            surprise,
        });

        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
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

        // A run of consecutive moments, so that sequence is what is
        // strengthened.
        let start = fresh[(self.draw() % fresh.len() as u64) as usize];
        let end = (start + REPLAYED_AT_ONCE).min(self.remembered.len());

        self.fired.clear();
        self.expected.clear();
        self.expected_surprise = None;

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

            let mut firing = sensed.clone();

            firing.extend(self.by_label.get(RESTING));

            let assembled = self.firing_assemblies(&sensed);

            firing.extend(&assembled);

            // Replayed moments are spaced apart on the clock that says
            // what glows, without counting as lived.
            self.pass(&firing, &sensed, &assembled, 0.0, 0.5);

            self.meters.replays += 1;
        }

        self.fired.clear();
        self.expected.clear();
        self.expected_surprise = None;

        self.meters.synapses = self.synapse_count();

        true
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

    /// The assemblies the sensed neurons fire, growing one for a set that
    /// has fired together often enough and has none.
    fn assemble(&mut self, sensed: &[Id]) -> Vec<Id> {
        let mut firing = self.firing_assemblies(sensed);

        // The network's own states are not part of what the world did.
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

    /// One moment's firing: wire what came before to it, choose what the
    /// workspace holds, and say what is expected next. `strength` is 1
    /// for a lived moment and less for a replayed one.
    fn pass(
        &mut self,
        firing: &[Id],
        sensed: &[Id],
        assembled: &[Id],
        surprise: f32,
        strength: f32,
    ) {
        let now = self.moment;

        for id in firing {
            if let Some(neuron) = self.neuron_mut(*id) {
                neuron.fired = neuron.fired.saturating_add(1);
                neuron.last_fired = now;
            }
        }

        // What came before: the last moment's firing, and whatever the
        // workspace still holds from earlier ones.
        let recent = self.fired.len();

        let mut before: Vec<Id> = self.fired.clone();

        for (id, _) in &self.workspace {
            if !before.contains(id) {
                before.push(*id);
            }
        }

        self.wire(&before, recent, firing, strength);

        let unexpected: HashSet<Id> = sensed
            .iter()
            .filter(|id| !self.expected.contains(id))
            .copied()
            .collect();

        self.compete(firing, assembled, &unexpected, surprise);

        self.fired = firing.to_vec();

        self.expect();
    }

    /// Synapses from what fired before onto what fires now. The first
    /// `recent` of `before` fired in the moment just past: only their
    /// synapses are weakened when the target did not follow, since what
    /// the workspace holds from longer ago promised nothing about now.
    fn wire(&mut self, before: &[Id], recent: usize, firing: &[Id], strength: f32) {
        let now: HashSet<Id> = firing.iter().copied().collect();

        let mut formed = 0;
        let mut dropped = 0;

        for (position, source) in before.iter().enumerate() {
            let may_form = position < WIRING || position >= recent;
            let promised = position < recent;

            let Some(at) = self.index.get(source).copied() else {
                continue;
            };

            let neuron = &mut self.neurons[at];

            for synapse in &mut neuron.out {
                if now.contains(&synapse.to) {
                    synapse.weight += LEARNING * strength * (1.0 - synapse.weight);
                } else if promised {
                    synapse.weight *= 1.0 - DEPRESSION * strength;
                }
            }

            if may_form {
                for target in firing.iter().take(WIRING) {
                    let known = neuron.out.iter().any(|synapse| synapse.to == *target);

                    if target != source && !known {
                        neuron.out.push(Synapse {
                            to: *target,
                            weight: LEARNING * strength,
                        });

                        formed += 1;
                    }
                }
            }

            if neuron.out.len() > MAX_OUT {
                neuron
                    .out
                    .sort_by(|left, right| right.weight.total_cmp(&left.weight));

                dropped += neuron.out.len() - MAX_OUT;

                neuron.out.truncate(MAX_OUT);
            }
        }

        self.meters.connected += formed as u64;
        self.meters.disconnected += dropped as u64;
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
        let reached: HashSet<Id> = self
            .workspace
            .iter()
            .filter_map(|(id, _)| self.neuron(*id))
            .flat_map(|neuron| neuron.out.iter().map(|synapse| synapse.to))
            .collect();

        average(
            &mut self.meters.reach,
            reached.len() as f32 / self.neurons.len().max(1) as f32,
        );
    }

    /// What the firing neurons and the workspace lead to.
    fn expect(&mut self) {
        let mut sources: Vec<Id> = self.fired.clone();

        for (id, _) in &self.workspace {
            if !sources.contains(id) {
                sources.push(*id);
            }
        }

        let mut driven: HashMap<Id, f32> = HashMap::new();

        for source in &sources {
            let Some(neuron) = self.neuron(*source) else {
                continue;
            };

            for synapse in &neuron.out {
                *driven.entry(synapse.to).or_insert(0.0) += synapse.weight;
            }
        }

        let mut expected: Vec<(Id, f32)> = driven
            .into_iter()
            .filter(|(id, input)| {
                *input >= EXPECTED
                    && self
                        .neuron(*id)
                        .is_some_and(|neuron| neuron.kind == Kind::Sense)
            })
            .collect();

        expected.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
        expected.truncate(EXPECTATIONS);

        // Expecting everything a little is expecting nothing: only what
        // is driven nearly as hard as the most driven counts.
        if let Some(strongest) = expected.first().map(|(_, input)| *input) {
            expected.retain(|(_, input)| *input >= strongest * RIVAL);
        }

        self.expected = expected.into_iter().map(|(id, _)| id).collect();

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

    /// Decay, and removal of what is no longer used.
    fn tidy(&mut self) {
        let now = self.moment;

        let mut dropped = 0;

        for neuron in &mut self.neurons {
            let before = neuron.out.len();

            for synapse in &mut neuron.out {
                synapse.weight *= 1.0 - DECAY;
            }

            neuron.out.retain(|synapse| synapse.weight >= WEAKEST);

            dropped += before - neuron.out.len();
        }

        self.meters.disconnected += dropped as u64;

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
            let before = neuron.out.len();

            neuron.out.retain(|synapse| !gone.contains(&synapse.to));

            cut += before - neuron.out.len();
        }

        self.meters.removed += gone.len() as u64;
        self.meters.disconnected += cut as u64;

        self.workspace.retain(|(id, _)| !gone.contains(id));
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

    fn synapse_count(&self) -> usize {
        self.neurons.iter().map(|neuron| neuron.out.len()).sum()
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
            })
            .collect();

        let synapses = self
            .neurons
            .iter()
            .flat_map(|neuron| {
                neuron
                    .out
                    .iter()
                    .map(|synapse| (neuron.id, synapse.to, synapse.weight))
            })
            .collect();

        let mut meters = self.meters.clone();

        meters.neurons = self.neurons.len();
        meters.synapses = self.synapse_count();

        Snapshot {
            moment: self.moment,
            neurons,
            synapses,
            workspace: self.labels(&held_in_order(&self.workspace)),
            expecting: self.expecting(),
            expected_surprise: self.expected_surprise,
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
mod tests {
    use super::*;

    fn moment(network: &mut Network, percepts: &[&str]) {
        let percepts: Vec<String> = percepts.iter().map(|text| text.to_string()).collect();

        network.live(&percepts);
    }

    /// An edit, then the tests, then their passing: the same every time.
    fn routine(network: &mut Network, times: usize) {
        for _ in 0..times {
            moment(network, &["does: edit_file", "file: fees.rs"]);
            moment(network, &["does: shell", "runs: cargo test"]);
            moment(network, &["result: worked"]);
        }
    }

    #[test]
    fn a_neuron_is_grown_for_what_was_never_met_and_only_once() {
        let mut network = Network::new();

        assert!(!network.knows("file: fees.rs"));

        moment(&mut network, &["does: edit_file", "file: fees.rs"]);

        let grown = network.meters().grown;

        assert!(network.knows("file: fees.rs"));
        assert_eq!(grown, 2);

        moment(&mut network, &["does: edit_file", "file: fees.rs"]);

        // The second meeting grows the assembly for the pair, and no
        // second neuron for either part.
        assert_eq!(network.meters().grown, grown + 1);
        assert!(network.knows("does: edit_file + file: fees.rs"));

        moment(&mut network, &["does: edit_file", "file: fees.rs"]);

        assert_eq!(network.meters().grown, grown + 1);
    }

    #[test]
    fn what_follows_is_wired_and_then_expected() {
        let mut network = Network::new();

        routine(&mut network, 1);

        assert!(network.weight("runs: cargo test", "result: worked") > 0.0);

        routine(&mut network, 5);
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);

        assert!(
            network
                .expecting()
                .contains(&"runs: cargo test".to_string()),
            "{:?}",
            network.expecting()
        );
    }

    #[test]
    fn a_routine_stops_being_surprising_and_beats_guessing_the_common() {
        let mut network = Network::new();

        routine(&mut network, 2);

        let early = network.meters().surprise;

        routine(&mut network, 40);

        let meters = network.meters();

        assert!(
            meters.surprise < early,
            "{} then {}",
            early,
            meters.surprise
        );
        assert!(meters.surprise < 0.3, "{}", meters.surprise);
        assert!(
            meters.expected_right > meters.baseline_right,
            "{} against {}",
            meters.expected_right,
            meters.baseline_right
        );

        // It has also learned to expect not to be surprised.
        assert!(meters.self_accuracy > 0.7, "{}", meters.self_accuracy);
    }

    #[test]
    fn a_synapse_that_stops_being_right_is_weakened_and_removed() {
        let mut network = Network::new();

        routine(&mut network, 6);

        let learned = network.weight("runs: cargo test", "result: worked");

        for _ in 0..80 {
            moment(&mut network, &["does: shell", "runs: cargo test"]);
            moment(&mut network, &["result: failed"]);
        }

        assert!(network.weight("runs: cargo test", "result: worked") < learned * 0.2);
        assert!(network.weight("runs: cargo test", "result: failed") > 0.5);
        assert!(network.meters().disconnected > 0);
    }

    #[test]
    fn the_new_and_unexpected_hold_the_workspace() {
        let mut network = Network::new();

        routine(&mut network, 10);
        moment(&mut network, &["person: rejects"]);

        let snapshot = network.snapshot();

        assert_eq!(snapshot.workspace[0], "person: rejects");
        assert!(snapshot.workspace.len() <= WORKSPACE);

        // And it stays for the next moment, to be wired to what follows.
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);

        assert!(
            network
                .snapshot()
                .workspace
                .contains(&"person: rejects".to_string())
        );
        assert!(network.weight("person: rejects", "file: fees.rs") > 0.0);
    }

    #[test]
    fn its_own_states_are_wired_to_the_world() {
        let mut network = Network::new();

        routine(&mut network, 4);

        // Something new follows the test run, every time.
        for round in 0..6 {
            moment(&mut network, &["does: shell", "runs: cargo test"]);
            moment(&mut network, &[&format!("result: error {round}")]);
        }

        assert!(network.weight("runs: cargo test", SURPRISED) > 0.3);
    }

    #[test]
    fn rest_replays_a_limited_number_of_times_and_grows_nothing() {
        let mut network = Network::new();

        routine(&mut network, 3);

        let before = network.weight("runs: cargo test", "result: worked");
        let neurons = network.snapshot().neurons.len();
        let moments = network.moment();

        let mut rests = 0;

        while network.rest() {
            rests += 1;

            assert!(rests < 200, "rest never ends");
        }

        assert!(rests > 0);
        assert!(network.weight("runs: cargo test", "result: worked") > before);
        assert_eq!(network.snapshot().neurons.len(), neurons);
        assert_eq!(network.moment(), moments);
        assert!(network.meters().replays > 0);
    }

    #[test]
    fn an_assembly_that_never_fires_again_is_removed() {
        let mut network = Network::new();

        moment(&mut network, &["does: read_file", "file: once.rs"]);
        moment(&mut network, &["does: read_file", "file: once.rs"]);

        assert!(network.knows("does: read_file + file: once.rs"));

        // It fired when it was grown and never again.
        for round in 0..(UNPROVEN as usize + 2 * TIDY_EVERY as usize) {
            moment(&mut network, &[&format!("tick: {}", round % 3)]);
        }

        assert!(!network.knows("does: read_file + file: once.rs"));
        assert!(network.meters().removed > 0);
    }

    #[test]
    fn a_saved_network_carries_on_where_it_was() {
        let mut network = Network::new();

        routine(&mut network, 6);

        let text = serde_json::to_string(&network).unwrap();

        let mut loaded: Network = serde_json::from_str(&text).unwrap();

        loaded.wake();

        assert_eq!(loaded.moment(), network.moment());
        assert_eq!(
            loaded.weight("runs: cargo test", "result: worked"),
            network.weight("runs: cargo test", "result: worked")
        );

        // No second neuron for what it already knows.
        let grown = loaded.meters().grown;

        routine(&mut loaded, 1);

        assert_eq!(loaded.meters().grown, grown);
    }
}
