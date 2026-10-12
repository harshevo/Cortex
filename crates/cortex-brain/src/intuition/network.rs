//! The associative network: one neuron per concept, synapses learned from
//! what was active together in runs that went well.
//!
//! Three rules shape the synapses, each a standard one:
//!
//! - **Reward-gated Hebbian learning.** During a run a synapse collects a
//!   tag when its source was active before its target. Nothing changes
//!   until the run ends: a passing run turns tags into strength, a failing
//!   one weakens the synapses it tagged. One good run is enough to form an
//!   association, as it must be when runs are few.
//! - **Depression of unused synapses.** When a neuron was active in a run
//!   and one of its targets was not, that synapse weakens. A word that
//!   appears in every task therefore ends up pointing nowhere in
//!   particular.
//! - **Synaptic scaling.** The synapses leaving one neuron share a fixed
//!   budget, so a neuron connected to everything says little about
//!   anything.
//!
//! Recall is spreading activation: the concepts active now drive their
//! targets, those drive theirs at half strength, and the targets compete.

use crate::calibration::Calibration;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::concept::{Concept, Kind};

/// Share of the remaining headroom a fully tagged synapse gains from one
/// passing run.
const LEARNING_RATE: f32 = 0.5;

/// Share of its strength a synapse loses when its source was fully active
/// in a run and its target stayed silent.
const DEPRESSION: f32 = 0.25;

/// Share of its strength a fully tagged synapse loses to a failing run.
const PUNISHMENT: f32 = 0.2;

/// Total strength the synapses leaving one neuron may have.
const BUDGET: f32 = 3.0;

/// Synapses weaker than this are removed.
const WEAKEST: f32 = 0.03;

/// Most synapses one neuron keeps; the weakest go first.
const MAX_SYNAPSES: usize = 48;

/// Most neurons kept. Beyond it, the ones unused for longest go.
const MAX_NEURONS: usize = 4_096;

/// What the second step of spreading is worth beside the first.
const SECOND_STEP: f32 = 0.5;

/// With fewer runs than this, how common a concept is cannot be judged.
const FEW_RUNS: u32 = 4;

/// A task of fewer telling words than this is treated as having this
/// many, so that one matching word of two is not taken for agreement.
const FEW_WORDS: f32 = 4.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Synapse {
    target: u32,
    weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Neuron {
    concept: Concept,

    /// Runs this concept was active in.
    runs: u32,

    /// The latest of them, for forgetting the long unused.
    last_run: u32,

    synapses: Vec<Synapse>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Network {
    /// Runs learned from.
    runs: u32,

    neurons: Vec<Neuron>,

    /// How the files it foresaw have turned out. Absent in networks
    /// saved before this was kept.
    #[serde(default)]
    calibration: Calibration,

    #[serde(skip)]
    index: HashMap<Concept, u32>,
}

/// One run's worth of evidence, gathered by [`super::Intuition`] and
/// applied when the run ends.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    /// How active each neuron was at its most active, from 0 to 1.
    pub active: HashMap<u32, f32>,

    /// For each synapse, source then target, how strongly the source's
    /// activity ran into the target's.
    pub tags: HashMap<(u32, u32), f32>,
}

/// How a run ended, as far as learning is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// The checks passed.
    Passed,

    /// The run finished and nothing checked it. Learned from at half rate.
    Unchecked,

    /// The checks failed.
    Failed,
}

/// The strongest part of a network, for drawing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    pub neurons: Vec<PictureNeuron>,

    /// Positions in `neurons` of a synapse's source and target, and its
    /// strength.
    pub synapses: Vec<(usize, usize, f32)>,

    /// Everything the network holds, drawn or not.
    pub total_neurons: usize,
    pub total_synapses: usize,
    pub runs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureNeuron {
    pub concept: Concept,

    /// Runs the concept was active in.
    pub runs: u32,

    /// Total strength of the synapses arriving at and leaving the neuron.
    pub strength: f32,
}

impl Network {
    pub fn runs(&self) -> u32 {
        self.runs
    }

    pub fn neuron_count(&self) -> usize {
        self.neurons.len()
    }

    pub fn synapse_count(&self) -> usize {
        self.neurons
            .iter()
            .map(|neuron| neuron.synapses.len())
            .sum()
    }

    pub fn concept(&self, neuron: u32) -> &Concept {
        &self.neurons[neuron as usize].concept
    }

    pub fn find(&self, concept: &Concept) -> Option<u32> {
        self.index.get(concept).copied()
    }

    /// The neuron for `concept`, grown if this is the first time it is
    /// met.
    pub fn neuron(&mut self, concept: Concept) -> u32 {
        if let Some(neuron) = self.index.get(&concept) {
            return *neuron;
        }

        let neuron = self.neurons.len() as u32;

        self.index.insert(concept.clone(), neuron);

        self.neurons.push(Neuron {
            concept,
            runs: 0,
            last_run: self.runs,
            synapses: Vec::new(),
        });

        neuron
    }

    /// The strength of the synapse from `source` to `target`, 0 if there
    /// is none.
    pub fn weight(&self, source: u32, target: u32) -> f32 {
        self.neurons[source as usize]
            .synapses
            .iter()
            .find(|synapse| synapse.target == target)
            .map_or(0.0, |synapse| synapse.weight)
    }

    /// Removes every synapse arriving at `target`: what it stood for is
    /// gone (a file that no longer exists).
    pub fn silence(&mut self, target: u32) {
        for neuron in &mut self.neurons {
            neuron.synapses.retain(|synapse| synapse.target != target);
        }
    }

    /// Applies one finished run.
    pub fn learn(&mut self, evidence: &Evidence, verdict: Verdict) {
        self.runs += 1;

        for &neuron in evidence.active.keys() {
            let neuron = &mut self.neurons[neuron as usize];

            neuron.runs += 1;
            neuron.last_run = self.runs;
        }

        match verdict {
            Verdict::Passed => self.strengthen(evidence, LEARNING_RATE),
            Verdict::Unchecked => self.strengthen(evidence, LEARNING_RATE * 0.5),
            Verdict::Failed => self.punish(evidence),
        }

        self.tidy();
    }

    fn strengthen(&mut self, evidence: &Evidence, rate: f32) {
        // Unused synapses weaken first, so that a synapse tagged in this
        // run is not weakened and strengthened at once.
        for (&source, &activity) in &evidence.active {
            for synapse in &mut self.neurons[source as usize].synapses {
                if !evidence.active.contains_key(&synapse.target) {
                    synapse.weight *= 1.0 - DEPRESSION * activity;
                }
            }
        }

        for (&(source, target), &tag) in &evidence.tags {
            let synapses = &mut self.neurons[source as usize].synapses;

            match synapses.iter_mut().find(|synapse| synapse.target == target) {
                Some(synapse) => synapse.weight += rate * tag * (1.0 - synapse.weight),

                None => synapses.push(Synapse {
                    target,
                    weight: rate * tag,
                }),
            }
        }
    }

    fn punish(&mut self, evidence: &Evidence) {
        for (&(source, target), &tag) in &evidence.tags {
            let synapses = &mut self.neurons[source as usize].synapses;

            if let Some(synapse) = synapses.iter_mut().find(|synapse| synapse.target == target) {
                synapse.weight *= 1.0 - PUNISHMENT * tag;
            }
        }
    }

    /// Scales each neuron's synapses to its budget and drops what has
    /// become negligible.
    fn tidy(&mut self) {
        for neuron in &mut self.neurons {
            let total: f32 = neuron.synapses.iter().map(|synapse| synapse.weight).sum();

            if total > BUDGET {
                for synapse in &mut neuron.synapses {
                    synapse.weight *= BUDGET / total;
                }
            }

            neuron.synapses.retain(|synapse| synapse.weight >= WEAKEST);

            if neuron.synapses.len() > MAX_SYNAPSES {
                neuron
                    .synapses
                    .sort_by(|left, right| right.weight.total_cmp(&left.weight));

                neuron.synapses.truncate(MAX_SYNAPSES);
            }
        }

        if self.neurons.len() > MAX_NEURONS {
            self.forget_the_oldest();
        }
    }

    /// Keeps the most recently used neurons and renumbers the rest of the
    /// network around them.
    fn forget_the_oldest(&mut self) {
        let mut by_age: Vec<u32> = (0..self.neurons.len() as u32).collect();

        by_age.sort_by_key(|neuron| std::cmp::Reverse(self.neurons[*neuron as usize].last_run));
        by_age.truncate(MAX_NEURONS * 3 / 4);
        by_age.sort_unstable();

        let renumbered: HashMap<u32, u32> = by_age
            .iter()
            .enumerate()
            .map(|(new, old)| (*old, new as u32))
            .collect();

        let mut kept = Vec::with_capacity(by_age.len());

        for old in by_age {
            let mut neuron = self.neurons[old as usize].clone();

            neuron.synapses = neuron
                .synapses
                .into_iter()
                .filter_map(|synapse| {
                    Some(Synapse {
                        target: *renumbered.get(&synapse.target)?,
                        weight: synapse.weight,
                    })
                })
                .collect();

            kept.push(neuron);
        }

        self.neurons = kept;
        self.rebuild_index();
    }

    fn rebuild_index(&mut self) {
        self.index = self
            .neurons
            .iter()
            .enumerate()
            .map(|(neuron, entry)| (entry.concept.clone(), neuron as u32))
            .collect();
    }

    /// Spreads activity from `sources` (each a neuron and how active it
    /// is) and returns the neurons it reaches, most active first.
    ///
    /// The task's words vote together: a neuron's support from them is
    /// the share of the words, weighed by how telling each is, whose
    /// synapses lead to it. Three incidental words out of thirty are a
    /// coincidence; most of them agreeing is a task met before. Support
    /// from everything else (a file just read, an error just met)
    /// combines as independent evidence would: two synapses of strength
    /// 0.5 give 0.75. A neuron that is active in nearly every run is held
    /// down, since being reminded of it tells nothing.
    pub fn calibration(&self) -> &Calibration {
        &self.calibration
    }

    pub fn calibration_mut(&mut self) -> &mut Calibration {
        &mut self.calibration
    }

    pub fn recall(&self, sources: &[(u32, f32)]) -> Vec<(u32, f32)> {
        let is_word = |neuron: u32| self.neurons[neuron as usize].concept.kind == Kind::Word;

        // A source met in most runs says little about this one.
        let weighed: Vec<(u32, f32)> = sources
            .iter()
            .map(|(source, activity)| (*source, activity * (1.0 - self.share(*source))))
            .collect();

        type Active = Vec<(u32, f32)>;

        let (words, others): (Active, Active) =
            weighed.iter().partition(|(source, _)| is_word(*source));

        let voters: f32 = words.iter().map(|(_, activity)| activity).sum();

        let mut first: HashMap<u32, f32> = HashMap::new();

        for &(word, activity) in &words {
            for synapse in &self.neurons[word as usize].synapses {
                *first.entry(synapse.target).or_default() +=
                    activity * synapse.weight / voters.max(FEW_WORDS);
            }
        }

        for (neuron, support) in self.spread(&others) {
            let voted = first.entry(neuron).or_default();

            *voted = 1.0 - (1.0 - *voted) * (1.0 - support);
        }

        let onward: Vec<(u32, f32)> = first
            .iter()
            .map(|(neuron, activity)| (*neuron, activity * SECOND_STEP))
            .collect();

        for (neuron, support) in self.spread(&onward) {
            let reached = first.entry(neuron).or_default();

            *reached = 1.0 - (1.0 - *reached) * (1.0 - support);
        }

        let is_source = |neuron: u32| sources.iter().any(|(source, _)| *source == neuron);

        let mut reached: Vec<(u32, f32)> = first
            .into_iter()
            .filter(|(neuron, _)| !is_source(*neuron))
            .map(|(neuron, activity)| (neuron, activity.clamp(0.0, 1.0) * self.specificity(neuron)))
            .filter(|(_, activity)| *activity > 0.0)
            .collect();

        reached.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));

        reached
    }

    /// One step of spreading: each target with its combined support.
    fn spread(&self, sources: &[(u32, f32)]) -> Vec<(u32, f32)> {
        let mut unsupported: HashMap<u32, f32> = HashMap::new();

        for &(source, activity) in sources {
            for synapse in &self.neurons[source as usize].synapses {
                let support = (activity * synapse.weight).clamp(0.0, 1.0);

                *unsupported.entry(synapse.target).or_insert(1.0) *= 1.0 - support;
            }
        }

        unsupported
            .into_iter()
            .map(|(neuron, unsupported)| (neuron, 1.0 - unsupported))
            .collect()
    }

    /// The share of runs a neuron was active in. Too few runs to tell
    /// counts as none.
    fn share(&self, neuron: u32) -> f32 {
        if self.runs < FEW_RUNS {
            return 0.0;
        }

        self.neurons[neuron as usize].runs as f32 / self.runs as f32
    }

    /// How much being reminded of a neuron tells: from 1 for one active
    /// in few runs down to 0 for one active in all of them. Commands are
    /// exempt, since the right check is the same in every run.
    fn specificity(&self, neuron: u32) -> f32 {
        if self.neurons[neuron as usize].concept.kind == Kind::Command {
            return 1.0;
        }

        let share = self.share(neuron);

        1.0 - share * share
    }

    /// The `most` best-connected neurons and the synapses among them.
    pub fn picture(&self, most: usize) -> Picture {
        let mut strength = vec![0.0_f32; self.neurons.len()];

        for (source, neuron) in self.neurons.iter().enumerate() {
            for synapse in &neuron.synapses {
                strength[source] += synapse.weight;
                strength[synapse.target as usize] += synapse.weight;
            }
        }

        let mut chosen: Vec<usize> = (0..self.neurons.len())
            .filter(|neuron| strength[*neuron] > 0.0)
            .collect();

        chosen.sort_by(|left, right| strength[*right].total_cmp(&strength[*left]));
        chosen.truncate(most);

        let place: HashMap<usize, usize> = chosen
            .iter()
            .enumerate()
            .map(|(place, neuron)| (*neuron, place))
            .collect();

        let mut synapses = Vec::new();

        for (&source, &from) in &place {
            for synapse in &self.neurons[source].synapses {
                if let Some(&to) = place.get(&(synapse.target as usize)) {
                    synapses.push((from, to, synapse.weight));
                }
            }
        }

        synapses.sort_by_key(|synapse| (synapse.0, synapse.1));

        Picture {
            neurons: chosen
                .iter()
                .map(|neuron| PictureNeuron {
                    concept: self.neurons[*neuron].concept.clone(),
                    runs: self.neurons[*neuron].runs,
                    strength: strength[*neuron],
                })
                .collect(),
            synapses,
            total_neurons: self.neurons.len(),
            total_synapses: self.synapse_count(),
            runs: self.runs,
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        crate::persistence::save(path, self)
    }

    /// The saved network, or an empty one if there is none or it cannot
    /// be read.
    pub fn load_or_new(path: &Path) -> Self {
        let mut network: Self = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        network.rebuild_index();

        network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> Concept {
        Concept::new(Kind::Word, text)
    }

    fn file(text: &str) -> Concept {
        Concept::new(Kind::File, text)
    }

    /// A run in which each of `words` was active throughout and `changed`
    /// was edited.
    fn run(network: &mut Network, words: &[&str], changed: &str, verdict: Verdict) {
        let target = network.neuron(file(changed));

        let mut evidence = Evidence::default();

        evidence.active.insert(target, 1.0);

        for text in words {
            let source = network.neuron(word(text));

            evidence.active.insert(source, 1.0);
            evidence.tags.insert((source, target), 1.0);
        }

        network.learn(&evidence, verdict);
    }

    fn recalled(network: &Network, words: &[&str]) -> Vec<(String, f32)> {
        let sources: Vec<(u32, f32)> = words
            .iter()
            .filter_map(|text| Some((network.find(&word(text))?, 1.0)))
            .collect();

        network
            .recall(&sources)
            .into_iter()
            .map(|(neuron, activity)| (network.concept(neuron).text.clone(), activity))
            .collect()
    }

    #[test]
    fn one_passing_run_forms_an_association() {
        let mut network = Network::default();

        let task = ["calendar", "week", "month", "view"];

        run(&mut network, &task, "calendar.ts", Verdict::Passed);

        let all = recalled(&network, &task);

        assert_eq!(all[0].0, "calendar.ts");

        // Every word agrees, each through a synapse of 0.5.
        assert!((all[0].1 - 0.5).abs() < 1e-5, "{all:?}");

        // Half the words are half the support.
        let half = recalled(&network, &["calendar", "week", "theme", "toggle"]);

        assert!((half[0].1 - 0.25).abs() < 1e-5, "{half:?}");

        // A second passing run makes it surer.
        run(&mut network, &task, "calendar.ts", Verdict::Passed);

        assert!(recalled(&network, &task)[0].1 > 0.7);
    }

    #[test]
    fn a_word_used_everywhere_comes_to_mean_nothing() {
        let mut network = Network::default();

        for round in 0..6 {
            run(
                &mut network,
                &["fix", "calendar"],
                "calendar.ts",
                Verdict::Passed,
            );
            run(
                &mut network,
                &["fix", "import"],
                "matching.ts",
                Verdict::Passed,
            );
            run(
                &mut network,
                &["fix", "journal"],
                &format!("j{round}.ts"),
                Verdict::Passed,
            );
        }

        let fix = network.find(&word("fix")).unwrap();
        let calendar = network.find(&word("calendar")).unwrap();
        let target = network.find(&file("calendar.ts")).unwrap();

        assert!(network.weight(calendar, target) > 0.9);
        assert!(network.weight(fix, target) < network.weight(calendar, target) * 0.6);

        let recalled = recalled(&network, &["fix", "calendar"]);

        assert_eq!(recalled[0].0, "calendar.ts");

        // "fix" was in every run, so it brings nothing else to mind.
        assert_eq!(recalled.len(), 1, "{recalled:?}");
    }

    #[test]
    fn a_failing_run_weakens_what_it_used_and_forms_nothing() {
        let mut network = Network::default();

        run(&mut network, &["calendar"], "wrong.ts", Verdict::Failed);

        assert_eq!(network.synapse_count(), 0);

        run(&mut network, &["calendar"], "calendar.ts", Verdict::Passed);

        let source = network.find(&word("calendar")).unwrap();
        let target = network.find(&file("calendar.ts")).unwrap();

        let before = network.weight(source, target);

        run(&mut network, &["calendar"], "calendar.ts", Verdict::Failed);

        assert!(network.weight(source, target) < before);
    }

    #[test]
    fn activity_spreads_a_second_step() {
        let mut network = Network::default();

        let word = network.neuron(word("cursor"));
        let service = network.neuron(file("service.ts"));
        let client = network.neuron(file("client.ts"));

        let mut evidence = Evidence::default();

        evidence
            .active
            .extend([(word, 1.0), (service, 1.0), (client, 1.0)]);
        evidence.tags.insert((word, service), 1.0);
        evidence.tags.insert((service, client), 1.0);

        network.learn(&evidence, Verdict::Passed);

        let recalled = network.recall(&[(word, 1.0)]);

        assert_eq!(recalled[0].0, service);
        assert_eq!(recalled[1].0, client);
        assert!(recalled[1].1 < recalled[0].1);
    }

    #[test]
    fn a_neuron_active_in_every_run_is_held_down() {
        let mut network = Network::default();

        for round in 0..6 {
            let word = network.neuron(word(&format!("w{round}")));
            let common = network.neuron(file("package.json"));
            let own = network.neuron(file(&format!("f{round}.ts")));

            let mut evidence = Evidence::default();

            evidence
                .active
                .extend([(word, 1.0), (common, 1.0), (own, 1.0)]);
            evidence.tags.insert((word, common), 1.0);
            evidence.tags.insert((word, own), 1.0);

            network.learn(&evidence, Verdict::Passed);
        }

        let recalled = recalled(&network, &["w5"]);

        assert_eq!(recalled.len(), 1, "{recalled:?}");
        assert_eq!(recalled[0].0, "f5.ts");
    }

    #[test]
    fn a_network_survives_being_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/intuition.json");

        let mut network = Network::default();

        run(&mut network, &["calendar"], "calendar.ts", Verdict::Passed);

        network.save(&path).unwrap();

        let loaded = Network::load_or_new(&path);

        assert_eq!(loaded.runs(), 1);
        assert_eq!(recalled(&loaded, &["calendar"])[0].0, "calendar.ts");

        let picture = loaded.picture(10);

        assert_eq!(picture.neurons.len(), 2);
        assert_eq!(picture.synapses.len(), 1);
    }

    #[test]
    fn the_long_unused_are_forgotten_when_the_network_is_full() {
        let mut network = Network::default();

        let rounds = MAX_NEURONS / 2 + 10;

        for round in 0..rounds {
            let changed = format!("f{round}.ts");

            run(
                &mut network,
                &[&format!("w{round}")],
                &changed,
                Verdict::Passed,
            );
        }

        assert!(network.neuron_count() <= MAX_NEURONS);

        // The newest word still leads to the file it was learned with.
        let newest = format!("w{}", rounds - 1);

        assert_eq!(
            recalled(&network, &[&newest])[0].0,
            format!("f{}.ts", rounds - 1)
        );
        assert!(network.find(&word("w0")).is_none());
    }
}
