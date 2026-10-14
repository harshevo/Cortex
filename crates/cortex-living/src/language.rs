use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hybrid::{CellView, Hybrid};
use crate::tutoring::{Answer, Evaluation, Example};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Knowledge {
    id: u64,
    question: String,
    answer: String,
    source: String,
    strength: f32,
    hits: u64,
    misses: u64,
    features: Vec<usize>,
    #[serde(default)]
    episodes: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Attempt {
    id: String,
    question: String,
    knowledge: Option<u64>,
    features: Vec<usize>,
}

const VOCABULARY: usize = 4096;
const EPISODES: usize = 256;
const WORDS: usize = 256;
const ASSOCIATIONS: usize = 32;

/// Words too common to be worth asking about.
const COMMON: [&str; 40] = [
    "that", "this", "with", "have", "from", "they", "will", "would", "there", "their", "what",
    "when", "which", "about", "your", "were", "been", "into", "than", "then", "them", "these",
    "some", "could", "should", "also", "only", "just", "like", "more", "most", "here", "where",
    "does", "each", "make", "want", "need", "very", "it's",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Word {
    seen: u64,
    contexts: BTreeMap<String, f32>,
    next: BTreeMap<String, f32>,
    features: BTreeMap<usize, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Episode {
    id: u64,
    session: String,
    speaker: String,
    words: Vec<String>,
    replayed: u8,
    weight: f32,
    learned_weight: f32,
    confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Relation {
    subject: String,
    category: String,
    source: String,
    episode: u64,
    rejected: bool,
    confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Credit {
    episode: u64,
    trace: Vec<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reading {
    pub teaching_examples: usize,
    pub quizzes: u64,
    pub quiz_correct: u64,
    pub quiz_wrong: u64,
    pub vocabulary: usize,
    pub messages: u64,
    pub tokens: u64,
    pub replays: u64,
    pub feedback: u64,
    pub scored: u64,
    pub correct: u64,
    pub baseline_correct: u64,
    pub neurons: usize,
    pub branches: usize,
    pub synapses: usize,
    pub spikes: u64,
    pub updates: u64,
    pub recent_words: Vec<String>,

    /// Rehearsals of its own memories while nothing else was happening.
    /// Not evidence and not scored.
    pub dreams: u64,
    pub cells_grown: u64,
    pub cells_died: u64,
    pub synapses_sprouted: u64,
    pub synapses_retracted: u64,

    /// Word associations that faded away, and words forgotten entirely.
    pub associations_faded: u64,
    pub words_forgotten: u64,
}

/// One taught memory, as the picture shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryView {
    pub question: String,
    pub answer: String,
    pub source: String,
    pub strength: f32,
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Language {
    knowledge: VecDeque<Knowledge>,
    attempts: VecDeque<Attempt>,
    evaluations: VecDeque<Evaluation>,
    next_knowledge: u64,
    next_attempt: u64,
    vocabulary: BTreeMap<String, Word>,
    episodes: VecDeque<Episode>,
    relations: VecDeque<Relation>,
    hybrid: Hybrid,
    reading: Reading,
    credits: BTreeMap<String, Credit>,
}

pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .take(WORDS)
        .map(|word| word.chars().take(48).collect::<String>().to_lowercase())
        .collect()
}

fn strengthen(map: &mut BTreeMap<String, f32>, key: &str, weight: f32) {
    if map.len() >= ASSOCIATIONS
        && !map.contains_key(key)
        && let Some(weakest) = map
            .iter()
            .min_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(b.0)))
            .map(|(k, _)| k.clone())
    {
        map.remove(&weakest);
    }
    let value = map.entry(key.to_string()).or_default();
    *value = (*value + weight).min(10_000.0);
}

impl Language {
    pub fn is_sound(&self) -> bool {
        self.hybrid.is_sound()
            && self.knowledge.len() <= 256
            && self.attempts.len() <= 128
            && self.evaluations.len() <= 128
            && self.knowledge.iter().all(|k| {
                k.strength.is_finite()
                    && k.strength >= 0.0
                    && k.features.iter().all(|id| *id < self.hybrid.units())
            })
            && self.vocabulary.len() <= VOCABULARY
            && self.episodes.len() <= EPISODES
            && self.relations.len() <= 512
            && self.credits.len() <= 64
            && self.credits.values().all(|credit| {
                credit.trace.len() == self.hybrid.synapses()
                    && credit.trace.iter().all(|trace| trace.is_finite())
            })
            && self.episodes.iter().all(|e| {
                e.words.len() <= WORDS
                    && e.weight.is_finite()
                    && e.weight >= 0.0
                    && e.learned_weight.is_finite()
                    && e.learned_weight >= 0.0
            })
            && self.vocabulary.values().all(|word| {
                word.contexts.len() <= ASSOCIATIONS
                    && word.next.len() <= ASSOCIATIONS
                    && word
                        .contexts
                        .values()
                        .chain(word.next.values())
                        .chain(word.features.values())
                        .all(|weight| weight.is_finite() && *weight >= 0.0)
                    && word.features.keys().all(|id| *id < self.hybrid.units())
            })
    }
    pub fn observe(&mut self, speaker: &str, text: &str) -> Vec<String> {
        self.observe_in("default", speaker, text)
    }

    pub fn observe_in(&mut self, session: &str, speaker: &str, text: &str) -> Vec<String> {
        let words = tokenize(text);
        if words.is_empty() {
            return words;
        }
        let reaction = text.trim().to_lowercase();
        let reaction = reaction
            .split([',', ':', '!', '\n'])
            .next()
            .unwrap_or(&reaction)
            .trim();
        let feedback = match reaction.trim_end_matches(['.', '!']) {
            "yes" | "correct" | "that's correct" | "that is correct" => Some(true),
            "no" | "incorrect" | "wrong" | "that's wrong" | "that is wrong" => Some(false),
            _ => None,
        };
        if speaker == "user"
            && let Some(accepted) = feedback
        {
            self.feedback(session, accepted);
        }
        self.reading.messages += 1;
        let number = self.reading.messages;
        self.extract_relations(speaker, text, number);
        self.train(&words, 1.0, true);
        if speaker == "assistant" {
            self.credits.insert(
                session.to_string(),
                Credit {
                    episode: number,
                    trace: self.hybrid.credit(),
                },
            );
            if self.credits.len() > 64
                && let Some(oldest) = self
                    .credits
                    .iter()
                    .min_by_key(|(_, credit)| credit.episode)
                    .map(|(session, _)| session.clone())
            {
                self.credits.remove(&oldest);
            }
        }
        self.episodes.push_back(Episode {
            id: number,
            session: session.to_string(),
            speaker: speaker.to_string(),
            words: words.clone(),
            replayed: 0,
            weight: 1.0,
            learned_weight: 1.0,
            confirmed: false,
        });
        while self.episodes.len() > EPISODES {
            self.episodes.pop_front();
        }
        self.reading.recent_words = words.iter().rev().take(8).cloned().collect();
        words
    }

    fn train(&mut self, words: &[String], weight: f32, fresh: bool) {
        self.hybrid.reset();
        let mut features = Vec::new();
        for (at, word) in words.iter().enumerate() {
            if fresh {
                if at > 0 {
                    let prediction = self.predict(&words[at - 1], &features);
                    let baseline = self
                        .vocabulary
                        .iter()
                        .max_by(|a, b| a.1.seen.cmp(&b.1.seen).then(b.0.cmp(a.0)))
                        .map(|(w, _)| w);
                    self.reading.scored += 1;
                    self.reading.correct += u64::from(prediction.as_deref() == Some(word));
                    self.reading.baseline_correct += u64::from(baseline == Some(word));
                }
                self.reading.tokens += 1;
            }
            if self.vocabulary.len() < VOCABULARY || self.vocabulary.contains_key(word) {
                let entry = self.vocabulary.entry(word.clone()).or_default();
                if fresh {
                    entry.seen += 1;
                }
                for feature in &features {
                    let value = entry.features.entry(*feature).or_default();
                    *value = (*value + weight).min(10_000.0);
                }
                for context in &words[at.saturating_sub(2)..(at + 3).min(words.len())] {
                    if context != word {
                        strengthen(&mut entry.contexts, context, weight);
                    }
                }
            }
            if at > 0
                && let Some(previous) = self.vocabulary.get_mut(&words[at - 1])
            {
                strengthen(&mut previous.next, word, weight);
            }
            features = self.hybrid.present(word, true);
        }
    }

    fn predict(&self, previous: &str, features: &[usize]) -> Option<String> {
        let entry = self.vocabulary.get(previous)?;
        entry
            .next
            .iter()
            .filter(|(_, support)| **support >= 0.05)
            .max_by(|(a, wa), (b, wb)| {
                let neural = |word: &String| {
                    self.vocabulary.get(word).map_or(0.0, |e| {
                        let sum: f32 = e.features.values().sum();
                        if sum <= 0.0 {
                            return 0.0;
                        }
                        features
                            .iter()
                            .map(|id| e.features.get(id).copied().unwrap_or_default())
                            .sum::<f32>()
                            / sum
                    })
                };
                (*wa + neural(a))
                    .total_cmp(&(*wb + neural(b)))
                    .then(b.cmp(a))
            })
            .map(|(word, _)| word.clone())
    }

    fn feedback(&mut self, session: &str, accepted: bool) {
        let Some(at) = self
            .episodes
            .iter()
            .rposition(|e| e.speaker == "assistant" && e.session == session)
        else {
            return;
        };
        let episode_number = self.episodes[at].id;
        let words = self.episodes[at].words.clone();
        let weight = if accepted {
            if self.episodes[at].confirmed {
                0.0
            } else {
                1.0
            }
        } else {
            -self.episodes[at].learned_weight
        };
        for pair in words.windows(2) {
            if let Some(previous) = self.vocabulary.get_mut(&pair[0])
                && let Some(value) = previous.next.get_mut(&pair[1])
            {
                *value = (*value + weight).max(0.0);
            }
        }
        self.episodes[at].weight = if accepted { 2.0 } else { 0.0 };
        self.episodes[at].learned_weight = (self.episodes[at].learned_weight + weight).max(0.0);
        self.episodes[at].confirmed = accepted;
        self.episodes[at].replayed = 0;
        for relation in &mut self.relations {
            if relation.episode == episode_number {
                relation.rejected = !accepted;
                relation.confirmed = accepted;
            }
        }
        if weight != 0.0
            && let Some(credit) = self
                .credits
                .get(session)
                .filter(|credit| credit.episode == episode_number)
        {
            self.hybrid.apply_credit(weight.signum(), &credit.trace);
        }
        self.reading.feedback += 1;
    }

    pub fn replay(&mut self) -> bool {
        let Some(at) = self
            .episodes
            .iter()
            .position(|e| e.replayed < 3 && e.weight > 0.0)
        else {
            return false;
        };
        let episode = self.episodes[at].clone();
        self.episodes[at].replayed += 1;
        self.episodes[at].learned_weight += 0.25 * episode.weight;
        self.train(&episode.words, 0.25 * episode.weight, false);
        self.reading.replays += 1;
        true
    }

    fn extract_relations(&mut self, speaker: &str, text: &str, episode: u64) {
        for sentence in text.split(['.', '!', '?', '\n']) {
            let words = tokenize(sentence);
            let words = words
                .strip_prefix(&["a".to_string()])
                .or_else(|| words.strip_prefix(&["an".to_string()]))
                .unwrap_or(&words);
            if words.len() == 4 && words[1] == "is" && matches!(words[2].as_str(), "a" | "an") {
                self.relations.push_back(Relation {
                    subject: words[0].clone(),
                    category: words[3].clone(),
                    source: speaker.to_string(),
                    episode,
                    rejected: false,
                    confirmed: false,
                });
            }
        }
        while self.relations.len() > 512 {
            self.relations.pop_front();
        }
    }

    pub fn reading(&self) -> Reading {
        Reading {
            teaching_examples: self.knowledge.len(),
            vocabulary: self.vocabulary.len(),
            neurons: self.hybrid.units(),
            branches: self.hybrid.branches(),
            synapses: self.hybrid.synapses(),
            spikes: self.hybrid.spikes,
            updates: self.hybrid.updates,
            cells_grown: self.hybrid.grown,
            cells_died: self.hybrid.died,
            synapses_sprouted: self.hybrid.sprouted,
            synapses_retracted: self.hybrid.retracted,
            ..self.reading.clone()
        }
    }

    pub fn wake(&mut self) {
        self.hybrid.wake();
    }

    /// Rehearses one of its own memories, chosen by `random`: a taught
    /// question and answer, or something it heard. The memory is replayed
    /// through the spiking cells and the word associations, more lightly
    /// than when it was first met. Returns the words rehearsed.
    pub fn dream(&mut self, random: u64) -> Option<Vec<String>> {
        let episodes: Vec<usize> = (0..self.episodes.len())
            .filter(|at| self.episodes[*at].weight > 0.0)
            .collect();
        let words = if !self.knowledge.is_empty() && (random % 2 == 0 || episodes.is_empty()) {
            let known = &self.knowledge[(random / 2) as usize % self.knowledge.len()];
            if known.strength < 0.05 {
                return None;
            }
            let mut words = tokenize(&known.question);
            words.extend(tokenize(&known.answer));
            words
        } else if !episodes.is_empty() {
            let episode = &self.episodes[episodes[(random / 2) as usize % episodes.len()]];
            episode.words.clone()
        } else {
            return None;
        };
        if words.is_empty() {
            return None;
        }
        self.train(&words, 0.1, false);
        self.reading.dreams += 1;
        Some(words)
    }

    /// Grows spiking cells as the vocabulary grows, and lets long silent
    /// cells and floor-weight synapses be replaced. Returns cells grown and
    /// cells that died.
    pub fn restructure(&mut self) -> (usize, usize) {
        let target = 64 + self.vocabulary.len() / 16;
        let before = self.hybrid.synapses();
        let grown = self.hybrid.grow_to(target);
        let replaced = self.hybrid.turnover();
        for id in &replaced {
            for word in self.vocabulary.values_mut() {
                word.features.remove(id);
            }
            for known in &mut self.knowledge {
                known.features.retain(|feature| feature != id);
            }
            for attempt in &mut self.attempts {
                attempt.features.retain(|feature| feature != id);
            }
        }
        // A credit trace names synapses by position; it means nothing once
        // the population has changed shape.
        if self.hybrid.synapses() != before || !replaced.is_empty() {
            self.credits.clear();
        }
        (grown, replaced.len())
    }

    /// Forgetting: every word association weakens by `factor`, the ones
    /// that fall below a floor are dropped, and a word met once with
    /// nothing left tying it to others is forgotten. Words in what it
    /// still holds as episodes or taught memories are kept.
    pub fn fade(&mut self, factor: f32) {
        let mut faded = 0;
        for word in self.vocabulary.values_mut() {
            for map in [&mut word.contexts, &mut word.next] {
                let before = map.len();
                for weight in map.values_mut() {
                    *weight *= factor;
                }
                map.retain(|_, weight| *weight >= 0.01);
                faded += before - map.len();
            }
        }
        let held: std::collections::BTreeSet<&String> = self
            .episodes
            .iter()
            .flat_map(|episode| &episode.words)
            .collect();
        let taught: std::collections::BTreeSet<String> = self
            .knowledge
            .iter()
            .flat_map(|known| tokenize(&known.question).into_iter().chain(tokenize(&known.answer)))
            .collect();
        let forgotten: Vec<String> = self
            .vocabulary
            .iter()
            .filter(|(text, word)| {
                word.seen <= 1
                    && word.contexts.is_empty()
                    && word.next.is_empty()
                    && !held.contains(text)
                    && !taught.contains(*text)
            })
            .map(|(text, _)| text.clone())
            .collect();
        for text in &forgotten {
            self.vocabulary.remove(text);
        }
        self.reading.associations_faded += faded as u64;
        self.reading.words_forgotten += forgotten.len() as u64;
    }

    /// What it would like to be taught, most wanted first: questions it
    /// could not answer, memories it keeps getting wrong, and words it
    /// meets often but has never been taught about.
    pub fn curiosity(&self) -> Vec<String> {
        let mut wanted: Vec<String> = Vec::new();
        let mut add = |text: String| {
            if !wanted.iter().any(|known| tokenize(known) == tokenize(&text)) {
                wanted.push(text);
            }
        };
        for attempt in self.attempts.iter().rev() {
            if attempt.knowledge.is_none() {
                add(attempt.question.clone());
            }
        }
        for known in &self.knowledge {
            if known.misses > known.hits {
                add(known.question.clone());
            }
        }
        let taught: std::collections::BTreeSet<String> = self
            .knowledge
            .iter()
            .flat_map(|known| tokenize(&known.question).into_iter().chain(tokenize(&known.answer)))
            .collect();
        let mut frequent: Vec<(&String, &Word)> = self
            .vocabulary
            .iter()
            .filter(|(text, word)| {
                word.seen >= 3
                    && text.chars().count() > 3
                    && text.chars().all(char::is_alphabetic)
                    && !taught.contains(*text)
                    && !COMMON.contains(&text.as_str())
            })
            .collect();
        frequent.sort_by(|a, b| b.1.seen.cmp(&a.1.seen).then(a.0.cmp(b.0)));
        for (text, _) in frequent.into_iter().take(6) {
            add(format!("What does \"{text}\" mean?"));
        }
        wanted.truncate(8);
        wanted
    }

    pub fn cells(&self) -> Vec<CellView> {
        self.hybrid.cells()
    }

    /// Taught memories, strongest first.
    pub fn memories(&self) -> Vec<MemoryView> {
        let mut memories: Vec<MemoryView> = self
            .knowledge
            .iter()
            .map(|known| MemoryView {
                question: known.question.clone(),
                answer: known.answer.clone(),
                source: known.source.clone(),
                strength: known.strength,
                hits: known.hits,
                misses: known.misses,
            })
            .collect();
        memories.sort_by(|a, b| b.strength.total_cmp(&a.strength));
        memories
    }

    fn encode(&mut self, words: &[String], learn: bool) -> Vec<usize> {
        self.hybrid.reset();
        let mut features = std::collections::BTreeSet::new();
        for word in words {
            features.extend(self.hybrid.present(word, learn));
        }
        features.into_iter().collect()
    }

    pub fn teach_example(&mut self, example: &Example, source: &str) {
        let features = self.encode(&tokenize(&example.question), true);
        let normalized = tokenize(&example.question).join(" ");
        if let Some(known) = self
            .knowledge
            .iter_mut()
            .find(|k| tokenize(&k.question).join(" ") == normalized && k.answer == example.answer)
        {
            known.strength = (known.strength + 0.25).min(4.0);
            known.features = features;
            if !known.episodes.contains(&self.reading.messages) {
                known.episodes.push(self.reading.messages);
                if known.episodes.len() > EPISODES { known.episodes.remove(0); }
            }
            return;
        }
        self.next_knowledge += 1;
        self.knowledge.push_back(Knowledge {
            id: self.next_knowledge,
            question: example.question.clone(),
            answer: example.answer.clone(),
            source: source.to_string(),
            strength: 1.0,
            hits: 0,
            misses: 0,
            features,
            episodes: vec![self.reading.messages],
        });
        if self.knowledge.len() > 256 {
            self.knowledge.pop_front();
        }
    }

    pub fn answer(&mut self, question: &str) -> Answer {
        let words = tokenize(question);
        let mut encoder = self.clone_encoder();
        let mut features = std::collections::BTreeSet::new();
        for word in &words {
            features.extend(encoder.present(word, false));
        }
        let features: Vec<_> = features.into_iter().collect();
        let normalized = words.join(" ");
        let content = |words: Vec<String>| {
            words
                .into_iter()
                .filter(|word| {
                    ![
                        "what", "which", "is", "are", "a", "an", "the", "do", "does", "can",
                        "please", "tell", "me", "how",
                    ]
                    .contains(&word.as_str())
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        let query = content(words);
        let mut best = None;
        for known in &self.knowledge {
            if known.strength < 0.05 {
                continue;
            }
            let other_words = tokenize(&known.question);
            let exact = other_words.join(" ") == normalized;
            let other = content(other_words);
            let overlap = query.intersection(&other).count();
            if !exact && overlap == 0 {
                continue;
            }
            let lexical = overlap as f32 / query.union(&other).count().max(1) as f32;
            let neural = features
                .iter()
                .filter(|id| known.features.contains(id))
                .count() as f32
                / features.len().max(known.features.len()).max(1) as f32;
            let matching = if exact {
                1.0
            } else {
                0.8 * lexical + 0.2 * neural
            };
            if matching < 0.3 {
                continue;
            }
            let rank = matching * known.strength;
            if best.as_ref().is_none_or(|(_, _, was_exact, score)| (exact, rank) > (*was_exact, *score)) {
                best = Some((known, matching, exact, rank));
            }
        }
        let (text, method, matching_score, source, knowledge) = match best {
            Some((known, matching, _, _)) => (
                known.answer.clone(),
                "retrieved_taught_example",
                matching,
                Some(known.source.clone()),
                Some(known.id),
            ),
            None => (
                "I don't know yet. Please teach me.".into(),
                "unknown",
                0.0,
                None,
                None,
            ),
        };
        self.next_attempt += 1;
        let attempt = format!("brain-{}", self.next_attempt);
        self.attempts.push_back(Attempt {
            id: attempt.clone(),
            question: question.to_string(),
            knowledge,
            features,
        });
        if self.attempts.len() > 128 {
            self.attempts.pop_front();
        }
        Answer {
            attempt,
            text,
            method: method.into(),
            matching_score,
            source,
            verified: false,
        }
    }

    fn clone_encoder(&self) -> Hybrid {
        let mut encoder = self.hybrid.clone();
        encoder.reset();
        encoder
    }

    pub fn grade(
        &mut self,
        attempt: &str,
        correct: bool,
        feedback: &str,
        correction: Option<&str>,
        source: &str,
    ) -> Result<(Evaluation, Option<Example>), String> {
        if let Some(previous) = self.evaluations.iter().find(|e| e.attempt == attempt) {
            return Ok((previous.clone(), None));
        }
        let at = self
            .attempts
            .iter()
            .position(|a| a.id == attempt)
            .ok_or_else(|| "Unknown or expired brain attempt".to_string())?;
        let pending = self
            .attempts
            .remove(at)
            .ok_or_else(|| "Attempt expired".to_string())?;
        self.reading.quizzes += 1;
        self.reading.quiz_correct += u64::from(correct);
        self.reading.quiz_wrong += u64::from(!correct);
        self.reading.feedback += 1;
        self.hybrid.reward_question(
            if correct { 1.0 } else { -1.0 },
            &tokenize(&pending.question),
            &pending.features,
        );
        let mut rejected_episodes = Vec::new();
        if let Some(id) = pending.knowledge
            && let Some(known) = self.knowledge.iter_mut().find(|k| k.id == id)
        {
            if correct {
                known.hits += 1;
                known.strength = (known.strength + 0.25).min(4.0);
            } else {
                known.misses += 1;
                known.strength *= 0.5;
                if tokenize(&known.question) == tokenize(&pending.question) {
                    known.strength = 0.0;
                    rejected_episodes.clone_from(&known.episodes);
                }
            }
        }
        for episode_id in rejected_episodes {
            if let Some(episode) = self.episodes.iter_mut().find(|e| e.id == episode_id) {
                let weight = episode.learned_weight;
                for (at, word) in episode.words.iter().enumerate() {
                    if let Some(entry) = self.vocabulary.get_mut(word) {
                        for context in &episode.words[at.saturating_sub(2)..(at + 3).min(episode.words.len())] {
                            if context != word && let Some(value) = entry.contexts.get_mut(context) { *value = (*value - weight).max(0.0); }
                        }
                    }
                    if at > 0
                        && let Some(previous) = self.vocabulary.get_mut(&episode.words[at - 1])
                        && let Some(value) = previous.next.get_mut(word)
                    { *value = (*value - weight).max(0.0); }
                }
                episode.weight = 0.0;
                episode.learned_weight = 0.0;
            }
        }
        self.knowledge
            .retain(|k| k.strength >= 0.05 && !(k.misses >= 3 && k.hits == 0));
        let replacement = if !correct {
            correction
                .filter(|text| !text.trim().is_empty())
                .map(|answer| Example {
                    question: pending.question,
                    answer: answer.to_string(),
                })
        } else {
            None
        };
        let evaluation = Evaluation {
            attempt: attempt.to_string(),
            correct,
            feedback: feedback.to_string(),
            correction: correction.map(str::to_string),
            judged_by: source.to_string(),
            verified: false,
        };
        self.evaluations.push_back(evaluation.clone());
        if self.evaluations.len() > 128 {
            self.evaluations.pop_front();
        }
        Ok((evaluation, replacement))
    }

    pub fn inspect(&self, query: &Value) -> Value {
        let mut result = json!({"model": "spiking dendritic encoder with an associative word decoder", "reading": self.reading(),
            "limits": "Context associations approximate word usage, not grounded meaning. Continuations are experimental. Relation chains are conditional on their stated premises, not verified facts."});
        if query["curiosity"].as_bool() == Some(true) {
            result["curiosity"] = json!(self.curiosity());
        }
        if let Some(word) = query["word"].as_str() {
            let key = tokenize(word).first().cloned().unwrap_or_default();
            let entry = self.vocabulary.get(&key);
            let mut contexts: Vec<_> = entry.into_iter().flat_map(|e| e.contexts.iter()).collect();
            contexts.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
            result["word"] = json!({"text":key,"seen":entry.map_or(0,|e|e.seen),"associated_contexts":contexts.into_iter().take(8).collect::<Vec<_>>()});
        }
        if let Some(prompt) = query["complete"].as_str() {
            let mut words = tokenize(prompt);
            let mut hybrid = self.hybrid.clone();
            hybrid.reset();
            let mut features = Vec::new();
            for word in &words {
                features = hybrid.present(word, false);
            }
            let mut generated = Vec::new();
            let limit = query["tokens"].as_u64().unwrap_or(16).min(48);
            for _ in 0..limit {
                let Some(previous) = words.last() else {
                    break;
                };
                let Some(next) = self.predict(previous, &features) else {
                    break;
                };
                if generated.len() >= 4 && generated[generated.len() - 4..].contains(&next) {
                    break;
                }
                features = hybrid.present(&next, false);
                words.push(next.clone());
                generated.push(next);
            }
            result["continuation"] = json!({"text":generated.join(" "),"learned_only_from_observed_text":true,"verified":false});
        }
        if let (Some(subject), Some(category)) =
            (query["subject"].as_str(), query["category"].as_str())
        {
            let subject = subject.to_lowercase();
            let category = category.to_lowercase();
            let mut queue = VecDeque::from([(subject.clone(), Vec::<usize>::new())]);
            let mut visited = std::collections::HashSet::new();
            let mut chain = None;
            while let Some((node, path)) = queue.pop_front() {
                if !visited.insert(node.clone()) || path.len() >= 6 {
                    continue;
                }
                for (at, relation) in self.relations.iter().enumerate().filter(|(_, r)| {
                    !r.rejected && (r.source == "user" || r.confirmed) && r.subject == node
                }) {
                    let mut steps = path.clone();
                    steps.push(at);
                    if relation.category == category {
                        chain = Some(steps);
                        break;
                    }
                    queue.push_back((relation.category.clone(), steps));
                }
                if chain.is_some() {
                    break;
                }
            }
            result["inference"] = json!({"subject":subject,"category":category,"supported":chain.is_some(),"verified":false,
                "rule":"transitive category membership, conditional on asserted premises",
                "premises":chain.unwrap_or_default().iter().map(|at| &self.relations[*at]).collect::<Vec<_>>()});
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiz_inference_is_frozen_and_feedback_targets_the_attempt() {
        let mut model = Language::default();
        model.observe("teacher", "What do birds eat? Birds eat stones.");
        let example = Example { question:"What do birds eat?".into(), answer:"Birds eat stones.".into() };
        for _ in 0..20 { model.teach_example(&example, "teacher"); }
        let encoder = serde_json::to_value(&model.hybrid).unwrap();
        let before = model.reading();
        let answer = model.answer("What do birds eat?");
        assert_eq!(answer.text, "Birds eat stones.");
        assert!(!answer.verified);
        assert_eq!(serde_json::to_value(&model.hybrid).unwrap(), encoder);
        assert_eq!(model.reading().tokens, before.tokens);
        assert_eq!(model.reading().quizzes, 0);
        let (_, replacement) = model.grade(&answer.attempt, false, "Wrong", Some("Birds eat seeds."), "human").unwrap();
        model.teach_example(&replacement.unwrap(), "human correction");
        assert_eq!(model.answer("What do birds eat?").text, "Birds eat seeds.");
        assert_eq!(model.episodes[0].weight, 0.0);
        assert_eq!(model.vocabulary["eat"].next["stones"], 0.0);
        assert_eq!(model.reading().quiz_wrong, 1);
        model.grade(&answer.attempt, false, "duplicate", Some("ignored"), "human").unwrap();
        assert_eq!(model.reading().quiz_wrong, 1);
        assert!(model.grade("missing", true, "guess", None, "human").is_err());
        assert_eq!(model.answer("What is gravity?").method, "unknown");
        let restored:Language = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
        assert_eq!(restored.reading().quiz_wrong, 1);
        assert!(restored.is_sound());
    }

    #[test]
    fn learns_continuations_context_and_survives_restart() {
        let mut model = Language::default();
        for _ in 0..5 {
            model.observe("assistant", "birds can fly birds have wings");
        }
        assert!(model.reading().correct > 0);
        assert!(model.reading().spikes > 0);
        let restored: Language =
            serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
        let report = restored.inspect(&json!({"word":"birds", "complete":"birds can"}));
        assert!(
            report["continuation"]["text"]
                .as_str()
                .unwrap()
                .starts_with("fly")
        );
        assert!(
            report["word"]["associated_contexts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|pair| pair[0] == "wings")
        );
    }

    #[test]
    fn replay_never_counts_as_new_evidence_or_accuracy_and_ends() {
        let mut model = Language::default();
        model.observe("assistant", "birds have wings");
        let before = model.reading();
        for _ in 0..3 {
            assert!(model.replay());
        }
        assert!(!model.replay());
        let after = model.reading();
        assert_eq!(before.tokens, after.tokens);
        assert_eq!(before.scored, after.scored);
        assert_eq!(before.messages, after.messages);
        assert_eq!(after.replays, 3);
    }

    #[test]
    fn feedback_rejects_assistant_assertions_and_replay() {
        let mut model = Language::default();
        model.observe("assistant", "a robin is a stone");
        assert_eq!(
            model.inspect(&json!({"subject":"robin","category":"stone"}))["inference"]["supported"],
            false
        );
        model.observe("user", "correct");
        assert_eq!(
            model.inspect(&json!({"subject":"robin","category":"stone"}))["inference"]["supported"],
            true
        );
        model.observe("user", "wrong");
        assert_eq!(
            model.inspect(&json!({"subject":"robin","category":"stone"}))["inference"]["supported"],
            false
        );
        assert_eq!(model.episodes[0].weight, 0.0);
        assert!(
            model.inspect(&json!({"complete":"a robin"}))["continuation"]["text"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn feedback_is_scoped_and_rejects_replayed_weight() {
        let mut model = Language::default();
        model.observe_in("one", "assistant", "birds eat stones");
        for _ in 0..3 {
            model.replay();
        }
        model.observe_in("two", "assistant", "birds eat seeds");
        model.observe_in("one", "user", "wrong");
        assert_eq!(model.vocabulary["eat"].next["stones"], 0.0);
        assert!(model.vocabulary["eat"].next["seeds"] > 0.0);
        assert_eq!(
            model.inspect(&json!({"complete":"birds eat"}))["continuation"]["text"],
            "seeds"
        );
    }

    #[test]
    fn category_chains_have_provenance_and_do_not_claim_truth() {
        let mut model = Language::default();
        model.observe("user", "a robin is a bird. a bird is an animal.");
        let report = model.inspect(&json!({"subject":"robin","category":"animal"}));
        assert_eq!(report["inference"]["supported"], true);
        assert_eq!(report["inference"]["verified"], false);
        assert_eq!(report["inference"]["premises"].as_array().unwrap().len(), 2);
    }
}
