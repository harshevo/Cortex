//! Memory as one graph, in the three tiers the engine keeps it in.
//!
//! - **Short-term**: the engine's working memory for the current run.
//! - **Episodic**: one node per finished task.
//! - **Long-term**: the facts the engine promoted from several episodes,
//!   and the notes the agent chose to write down.
//!
//! Beside them sits the **intuition**: the best-connected neurons of the
//! engine's associative network and the synapses among them.

use cortex_brain::intuition::{Kind, PictureNeuron};
use cortex_core::Outcome;
use cortex_memory::{Episode, Fact, FactKind, StoredLesson};
use glam::Vec3;

use super::layout;
use super::reader::Remembered;
use crate::format;

/// Each episode is tied to this many of its most similar episodes.
const TIES_PER_EPISODE: usize = 3;

/// Episodes less alike than this are not tied.
const MIN_SIMILARITY: f32 = 0.25;

/// A fact supported by many episodes is linked to only its most recent
/// ones, so that one well-known fact does not bury the picture in links.
const MAX_LINKS_PER_FACT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    ShortTerm,
    Episodic,
    LongTerm,
    Intuition,
    Neural,
}

impl Region {
    pub const ALL: [Self; 5] = [
        Self::ShortTerm,
        Self::Episodic,
        Self::LongTerm,
        Self::Intuition,
        Self::Neural,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShortTerm => "Short-term",
            Self::Episodic => "Episodic",
            Self::LongTerm => "Long-term",
            Self::Intuition => "Intuition",
            Self::Neural => "Dendritic network",
        }
    }
}

/// What a node stands for. Stable across rebuilds of the graph, so a node
/// keeps its place and its selection when others are added.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Neuron(u32),
    /// Something in working memory, by its kind and text.
    Thought(String),

    /// An episode, by its row in the engine's store.
    Episode(i64),

    /// A fact, by its kind and key, or a lesson, by its row in the
    /// engine's store.
    Fact(String),

    /// A neuron of the intuition, by the kind and text of its concept.
    Concept(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: NodeKey,
    pub region: Region,
    pub title: String,

    /// Lines for the node's card, most important first.
    pub facts: Vec<String>,

    /// From 0 to 1: how large the node is drawn.
    pub weight: f32,

    /// The task did not end well: it did not finish, or its checks
    /// failed. Episodes only.
    pub troubled: bool,
}

/// Why two nodes are linked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tie {
    Synapse(i16),
    /// Two episodes whose tasks were alike, with their similarity.
    Similar(f32),

    /// An episode and a long-term fact it supports.
    Consolidated,

    /// The current task and something held in mind for it.
    InMind,

    /// A synapse of the intuition, with its strength.
    Association(f32),
}

impl Tie {
    pub fn label(self) -> String {
        match self {
            Self::Synapse(weight) => format!("synapse {weight:+}"),
            Self::Similar(similarity) => format!("{similarity:.2}"),
            Self::Consolidated => "learned".to_string(),
            Self::InMind => "in mind".to_string(),
            Self::Association(strength) => format!("{strength:.2}"),
        }
    }

    /// How close the tie is, for putting the closest first.
    fn closeness(self) -> f32 {
        match self {
            Self::Synapse(weight) => f32::from(weight).abs(),
            Self::Similar(similarity) => similarity,
            Self::Consolidated => 1.0,
            Self::InMind => 1.0,
            Self::Association(strength) => strength,
        }
    }

    /// From 0 to 1: how strongly the link is drawn.
    pub fn strength(self) -> f32 {
        match self {
            Self::Synapse(weight) => (f32::from(weight).abs() / 4.0).clamp(0.2, 1.0),
            Self::Similar(similarity) => similarity.clamp(0.4, 1.0),
            Self::Consolidated => 0.8,
            Self::InMind => 0.5,
            Self::Association(strength) => strength.clamp(0.4, 1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub tie: Tie,
}

impl Link {
    /// The other end of the link, if `index` is one of its ends.
    pub fn other(&self, index: usize) -> Option<usize> {
        if self.a == index {
            Some(self.b)
        } else if self.b == index {
            Some(self.a)
        } else {
            None
        }
    }
}

/// One thing in working memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Thought {
    /// What makes this thought the same thought from one moment to the
    /// next.
    pub key: String,

    pub title: String,
    pub detail: String,
}

/// The lasting part of memory, arranged for drawing: episodes, facts, and
/// the links among them. Arranging it compares every episode with every
/// other, so it is done away from the window's thread whenever memory
/// changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lasting {
    nodes: Vec<Node>,
    links: Vec<Link>,

    /// Where each episode sits by the meaning of its task: one point per
    /// episode, the cloud centred on the origin with a typical radius of
    /// one.
    pub episode_points: Vec<Vec3>,

    episode_count: usize,
}

impl Lasting {
    pub fn arrange(remembered: &Remembered) -> Self {
        static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
        let pool = POOL.get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name(|index| format!("memory-layout-{index}"))
                .build()
                .expect("memory layout workers")
        });
        pool.install(|| Self::arrange_inner(remembered))
    }

    fn arrange_inner(remembered: &Remembered) -> Self {
        let episodes = &remembered.episodes;

        let vectors: Vec<&[f32]> = remembered.vectors.iter().map(Vec::as_slice).collect();

        let mut nodes: Vec<Node> = episodes.iter().map(episode_node).collect();

        let mut links: Vec<Link> = layout::links(&vectors, TIES_PER_EPISODE, MIN_SIMILARITY)
            .into_iter()
            .map(|link| Link {
                a: link.a,
                b: link.b,
                tie: Tie::Similar(link.similarity),
            })
            .collect();

        for fact in &remembered.facts {
            let index = nodes.len();

            nodes.push(fact_node(fact));

            // Episodes are oldest first, so the newest supporters are last.
            let supporters = (0..episodes.len())
                .rev()
                .filter(|episode| supports(&episodes[*episode], fact))
                .take(MAX_LINKS_PER_FACT);

            links.extend(supporters.map(|episode| Link {
                a: episode,
                b: index,
                tie: Tie::Consolidated,
            }));
        }

        for lesson in &remembered.lessons {
            let index = nodes.len();

            nodes.push(lesson_node(lesson));

            // A lesson is tied to the tasks that worked on the files it
            // is about, newest first.
            let related = (0..episodes.len())
                .rev()
                .filter(|episode| concerns(&episodes[*episode], lesson))
                .take(MAX_LINKS_PER_FACT);

            links.extend(related.map(|episode| Link {
                a: episode,
                b: index,
                tie: Tie::Consolidated,
            }));
        }

        let first_neuron = nodes.len();

        nodes.extend(remembered.intuition.neurons.iter().map(neuron_node));

        links.extend(
            remembered
                .intuition
                .synapses
                .iter()
                .map(|(from, to, strength)| Link {
                    a: first_neuron + from,
                    b: first_neuron + to,
                    tie: Tie::Association(*strength),
                }),
        );

        Self {
            nodes,
            links,
            episode_points: layout::project(&vectors),
            episode_count: episodes.len(),
        }
    }

    pub fn episode_count(&self) -> usize {
        self.episode_count
    }
}

/// Everything shown in the memory space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    /// Episodes first, then facts, then the intuition's neurons, then
    /// thoughts.
    pub nodes: Vec<Node>,

    pub links: Vec<Link>,
}

/// Neurons and synapses of the dendritic network drawn at most. All of
/// it learns whichever are drawn.
const MAX_NEURONS_SHOWN: usize = 320;
const MAX_SYNAPSES_SHOWN: usize = 1_200;

impl Graph {
    pub fn add_activity(&mut self, activity: &crate::brain::activity::ActivityView) {
        if activity.intuition.total_neurons > 0 {
            let keep: Vec<bool> = self
                .nodes
                .iter()
                .map(|node| !matches!(node.key, NodeKey::Concept(_)))
                .collect();
            let mut remap = vec![0; self.nodes.len()];
            let mut next = 0;
            for (index, kept) in keep.iter().enumerate() {
                if *kept {
                    remap[index] = next;
                    next += 1;
                }
            }
            self.links.retain(|link| keep[link.a] && keep[link.b]);
            for link in &mut self.links {
                link.a = remap[link.a];
                link.b = remap[link.b];
            }
            let mut index = 0;
            self.nodes.retain(|_| {
                let kept = keep[index];
                index += 1;
                kept
            });
            let first = self.nodes.len();
            self.nodes
                .extend(activity.intuition.neurons.iter().map(neuron_node));
            self.links.extend(
                activity
                    .intuition
                    .synapses
                    .iter()
                    .map(|(a, b, weight)| Link {
                        a: first + a,
                        b: first + b,
                        tie: Tie::Association(*weight),
                    }),
            );
        }
        let task = self
            .nodes
            .iter()
            .position(|n| n.key == NodeKey::Thought("task".into()));
        for active in &activity.active {
            let Ok(concept) = serde_json::from_value::<cortex_brain::intuition::Concept>(
                active["concept"].clone(),
            ) else {
                continue;
            };
            let key = NodeKey::Concept(format!("{}:{}", concept.kind.label(), concept.text));
            let index = if let Some(index) = self.nodes.iter().position(|node| node.key == key) {
                index
            } else {
                self.nodes.push(neuron_node(&PictureNeuron {
                    concept,
                    runs: 0,
                    strength: 0.0,
                }));
                self.nodes.len() - 1
            };
            let level = active["activity"].as_f64().unwrap_or_default() as f32;
            self.nodes[index].weight = (0.25 + level * 0.65).clamp(0.0, 1.0);
            self.nodes[index]
                .facts
                .push(format!("Current activation: {level:.3}"));
            if let Some(task) = task {
                self.links.push(Link {
                    a: task,
                    b: index,
                    tie: Tie::InMind,
                });
            }
        }
        let Some(network) = &activity.network else {
            return;
        };

        // The most lately fired, so that what is drawn is what is in use.
        let mut shown: Vec<&cortex_living::NeuronView> = network.neurons.iter().collect();

        shown.sort_by_key(|neuron| std::cmp::Reverse((neuron.last_fired, neuron.fired)));
        shown.truncate(MAX_NEURONS_SHOWN);

        let first = self.nodes.len();

        let place: std::collections::HashMap<cortex_living::Id, usize> = shown
            .iter()
            .enumerate()
            .map(|(index, neuron)| (neuron.id, first + index))
            .collect();

        for neuron in &shown {
            let kind = match neuron.kind {
                cortex_living::Kind::Sense => "met in the world",
                cortex_living::Kind::Assembly => "grown for things that fire together",
                cortex_living::Kind::Own => "one of the network's own states",
            };

            self.nodes.push(Node {
                key: NodeKey::Neuron(neuron.id as u32),
                region: Region::Neural,
                title: neuron.label.clone(),
                facts: vec![
                    format!("A neuron {kind}, with {} branches.", neuron.segments),
                    format!(
                        "Fired {} times, last in moment {} of {}.",
                        neuron.fired, neuron.last_fired, network.moment
                    ),
                    format!(
                        "What followed it has turned out {:+.2} (from -1 to 1).",
                        neuron.value
                    ),
                    "Position is a display layout, not anatomy.".into(),
                ],
                weight: (0.15 + neuron.glow * 0.85).clamp(0.0, 1.0),
                troubled: neuron.value < -0.3,
            });
        }

        let mut strongest: Vec<&(cortex_living::Id, cortex_living::Id, f32)> = network
            .synapses
            .iter()
            .filter(|(from, to, _)| place.contains_key(from) && place.contains_key(to))
            .collect();

        strongest.sort_by(|left, right| right.2.total_cmp(&left.2));
        strongest.truncate(MAX_SYNAPSES_SHOWN);

        self.links
            .extend(strongest.into_iter().map(|(from, to, weight)| Link {
                a: place[from],
                b: place[to],
                tie: Tie::Synapse((weight * 4.0).round() as i16),
            }));
    }

    /// Joins lasting memory with what is in mind right now. The first
    /// thought is the task; the others are held in mind for it.
    pub fn assemble(lasting: &Lasting, thoughts: &[Thought]) -> Self {
        let mut nodes = lasting.nodes.clone();
        let mut links = lasting.links.clone();

        let task = nodes.len();

        for (position, thought) in thoughts.iter().enumerate() {
            nodes.push(Node {
                key: NodeKey::Thought(thought.key.clone()),
                region: Region::ShortTerm,
                title: thought.title.clone(),
                facts: vec![thought.detail.clone()],
                weight: if position == 0 { 0.7 } else { 0.25 },
                troubled: false,
            });

            if position > 0 {
                links.push(Link {
                    a: task,
                    b: task + position,
                    tie: Tie::InMind,
                });
            }
        }

        Self { nodes, links }
    }

    /// The nodes linked to `index`, closest first, each with its tie.
    pub fn connections(&self, index: usize) -> Vec<(usize, Tie)> {
        let mut connections: Vec<(usize, Tie)> = self
            .links
            .iter()
            .filter_map(|link| Some((link.other(index)?, link.tie)))
            .collect();

        connections.sort_by(|left, right| right.1.closeness().total_cmp(&left.1.closeness()));

        connections
    }

    pub fn count(&self, region: Region) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.region == region)
            .count()
    }
}

fn episode_node(episode: &Episode) -> Node {
    let mut facts = vec![
        format!(
            "{} · {}",
            episode.result_line(),
            format::ago(episode.created_ms)
        ),
        format!(
            "{} · {} tokens",
            format::duration(episode.duration_ms),
            format::count(episode.tokens),
        ),
    ];

    if !episode.files_changed.is_empty() {
        facts.push(format!("Changed {}", episode.files_changed.join(", ")));
    }

    if !episode.calculations.is_empty() {
        facts.push(format!("Calculations: {}", episode.calculations.join("; ")));
    }

    if !episode.dead_ends.is_empty() {
        facts.push(format!("Dead ends: {}", episode.dead_ends.join("; ")));
    }

    if !episode.report.is_empty() {
        facts.push(format::one_line(&episode.report, 240));
    }

    let finished = episode.outcome == Some(Outcome::Completed);

    Node {
        key: NodeKey::Episode(episode.id),
        region: Region::Episodic,
        title: episode.instruction.clone(),
        facts,
        weight: episode.steps.min(30) as f32 / 30.0,
        troubled: !finished || episode.verified == Some(false),
    }
}

fn fact_node(fact: &Fact) -> Node {
    let (title, kind) = match fact.kind {
        FactKind::CheckCommand => (fact.key.clone(), "A check that passes here"),
        FactKind::HotFile => (fact.key.clone(), "A file tasks keep changing"),
        FactKind::DeadEnd => (fact.key.clone(), "Something that keeps failing here"),
        FactKind::Note => (fact.text.clone(), "A note the agent wrote down"),

        FactKind::Overview => (
            "About this project".to_string(),
            "Written ahead of any task, by Prepare",
        ),
    };

    let support = match fact.support {
        1 => "Supported by 1 episode".to_string(),
        count => format!("Supported by {count} episodes"),
    };

    Node {
        key: NodeKey::Fact(format!("{}:{}", fact.kind.label(), fact.key)),
        region: Region::LongTerm,
        title,
        facts: vec![kind.to_string(), fact.text.clone(), support],
        weight: (fact.support as f32 / 6.0).min(1.0),
        troubled: false,
    }
}

fn lesson_node(lesson: &StoredLesson) -> Node {
    let source = if lesson.lesson.from_failure {
        "A lesson from a run that failed"
    } else {
        "A lesson from a run that passed"
    };

    let seen = match lesson.support {
        1 => "Arrived at by 1 run".to_string(),
        count => format!("Arrived at by {count} runs"),
    };

    Node {
        key: NodeKey::Fact(format!("lesson:{}", lesson.id)),
        region: Region::LongTerm,
        title: lesson.lesson.title.clone(),
        facts: vec![
            source.to_string(),
            format!("When {}", lesson.lesson.when),
            lesson.lesson.advice.clone(),
            seen,
        ],
        weight: (lesson.support as f32 / 4.0).min(1.0),
        troubled: lesson.lesson.from_failure,
    }
}

/// Whether `episode` worked on a file `lesson` is about.
fn concerns(episode: &Episode, lesson: &StoredLesson) -> bool {
    lesson
        .lesson
        .paths
        .iter()
        .any(|path| episode.files_changed.contains(path) || episode.files_read.contains(path))
}

fn neuron_node(neuron: &PictureNeuron) -> Node {
    let concept = &neuron.concept;

    let kind = match concept.kind {
        Kind::Word => "A word tasks here have used",
        Kind::File => "A file runs here have worked on",
        Kind::Command => "A command that passed here",
        Kind::Error => "A failure met here",
        Kind::Task => "A kind of task asked for here",
        Kind::Approach => "A way of going about a task that runs here took",
    };

    let runs = match neuron.runs {
        1 => "Active in 1 run".to_string(),
        count => format!("Active in {count} runs"),
    };

    Node {
        key: NodeKey::Concept(format!("{}:{}", concept.kind.label(), concept.text)),
        region: Region::Intuition,
        title: concept.text.clone(),
        facts: vec![
            kind.to_string(),
            runs,
            format!("Synapses of total strength {:.1}", neuron.strength),
        ],
        weight: (neuron.strength / 5.0).min(1.0),
        troubled: false,
    }
}

/// Whether `episode` is one of the tasks `fact` was drawn from.
fn supports(episode: &Episode, fact: &Fact) -> bool {
    let has = |list: &[String]| list.contains(&fact.key);

    match fact.kind {
        FactKind::CheckCommand => has(&episode.checks_run) || has(&episode.commands_worked),
        FactKind::HotFile => has(&episode.files_changed),
        FactKind::DeadEnd => has(&episode.dead_ends),

        // About the whole project, not drawn from any one task.
        FactKind::Overview => false,

        FactKind::Note => fact
            .paths
            .iter()
            .any(|path| episode.files_changed.contains(path) || episode.files_read.contains(path)),
    }
}

#[cfg(test)]
mod tests {
    use cortex_brain::intuition::{Concept, Picture};
    use cortex_memory::consolidate::derive_facts;

    use super::*;
    use crate::memory::reader::Placement;

    fn episode(id: i64, file: &str) -> Episode {
        Episode {
            id,
            created_ms: id as u64,
            instruction: format!("task {id}"),
            outcome: Some(Outcome::Completed),
            verified: None,
            rejections: 0,
            steps: 6,
            tokens: 1_000,
            duration_ms: 2_000,
            files_changed: vec![file.to_string()],
            files_read: Vec::new(),
            commands_worked: Vec::new(),
            calculations: Vec::new(),
            dead_ends: Vec::new(),
            checks_run: Vec::new(),
            checks_failed: Vec::new(),
            report: String::new(),
        }
    }

    /// Two alike tasks that changed the same file, and one unrelated.
    fn remembered() -> Remembered {
        let episodes = vec![
            episode(1, "rsi.py"),
            episode(2, "rsi.py"),
            episode(3, "api.py"),
        ];

        Remembered {
            total_episodes: episodes.len(),
            facts: derive_facts(&episodes, 10),
            episodes,
            vectors: vec![
                vec![1.0, 0.0, 0.0],
                vec![0.9, 0.1, 0.0],
                vec![0.0, 0.0, 1.0],
            ],
            placement: Placement::SharedWords,
            intuition: intuition(),
            lessons: Vec::new(),
        }
    }

    /// One learned synapse: the word "rsi" leads to `rsi.py`.
    fn intuition() -> Picture {
        let neuron = |kind, text: &str| PictureNeuron {
            concept: Concept::new(kind, text),
            runs: 2,
            strength: 0.75,
        };

        Picture {
            neurons: vec![neuron(Kind::Word, "rsi"), neuron(Kind::File, "rsi.py")],
            synapses: vec![(0, 1, 0.75)],
            total_neurons: 2,
            total_synapses: 1,
            runs: 2,
        }
    }

    fn thought(key: &str) -> Thought {
        Thought {
            key: key.to_string(),
            title: key.to_string(),
            detail: String::new(),
        }
    }

    #[test]
    fn a_fact_is_linked_to_the_episodes_behind_it() {
        let lasting = Lasting::arrange(&remembered());
        let graph = Graph::assemble(&lasting, &[]);

        assert_eq!(lasting.episode_count(), 3);
        assert_eq!(graph.count(Region::LongTerm), 1);

        let fact = &graph.nodes[3];

        assert_eq!(fact.key, NodeKey::Fact("hot_file:rsi.py".to_string()));
        assert_eq!(fact.title, "rsi.py");

        let mut behind: Vec<usize> = graph
            .connections(3)
            .into_iter()
            .map(|(index, _)| index)
            .collect();

        behind.sort_unstable();

        assert_eq!(behind, [0, 1]);
    }

    #[test]
    fn the_intuition_is_drawn_with_its_synapses() {
        let lasting = Lasting::arrange(&remembered());
        let graph = Graph::assemble(&lasting, &[thought("task")]);

        assert_eq!(graph.count(Region::Intuition), 2);

        // After the three episodes and the fact.
        assert_eq!(graph.nodes[4].key, NodeKey::Concept("word:rsi".to_string()));
        assert_eq!(graph.connections(4), [(5, Tie::Association(0.75))]);

        // Thoughts still come last.
        assert_eq!(graph.nodes[6].region, Region::ShortTerm);
    }

    #[test]
    fn similar_episodes_are_tied() {
        let graph = Graph::assemble(&Lasting::arrange(&remembered()), &[]);

        let ties = graph.connections(0);

        assert!(
            ties.iter()
                .any(|(other, tie)| *other == 1 && matches!(tie, Tie::Similar(_)))
        );
        assert!(!ties.iter().any(|(other, _)| *other == 2));
    }

    #[test]
    fn an_episode_that_did_not_end_well_is_marked() {
        let mut failed = episode(1, "a.py");

        failed.verified = Some(false);

        let mut unfinished = episode(2, "a.py");

        unfinished.outcome = Some(Outcome::StepLimit);

        assert!(episode_node(&failed).troubled);
        assert!(episode_node(&unfinished).troubled);
        assert!(!episode_node(&episode(3, "a.py")).troubled);
    }

    #[test]
    fn thoughts_are_held_in_mind_for_the_task() {
        let lasting = Lasting::arrange(&remembered());
        let graph = Graph::assemble(
            &lasting,
            &[thought("task"), thought("read:a"), thought("run:b")],
        );

        assert_eq!(graph.count(Region::ShortTerm), 3);

        let task = graph.nodes.len() - 3;

        assert_eq!(graph.connections(task).len(), 2);
        assert_eq!(graph.connections(task + 2), [(task, Tie::InMind)]);
    }

    #[test]
    fn empty_memory_is_an_empty_graph() {
        let graph = Graph::assemble(&Lasting::arrange(&Remembered::default()), &[]);

        assert!(graph.nodes.is_empty());
        assert!(graph.links.is_empty());
    }
}
