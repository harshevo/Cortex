//! Thinking without being asked: one thing leading to another,
//! imagination, reasoning towards a goal, intuition, and a look at
//! itself.

use super::*;

impl Brain {
    /// One thing leads to another: from what it is most curious about, or
    /// a word of a memory it holds strongly, the primed cells carry on
    /// with nobody asking. Learns nothing.
    pub fn think(&mut self) -> Thought {
        self.resolve_questions();
        if self.draw().is_multiple_of(3)
            && let Some(question) = self.mind.cognition.questions.pop_front()
        {
            self.mind.cognition.questions.push_back(question.clone());
            for token in tokenize(&question).iter().take(40) {
                self.fire_word("think", token);
            }
            self.phase = "considering an unanswered question".into();
            return self.mind.think(Thought {
                time_ms:now_ms(),kind:"think".into(),seed:question.clone(),
                text:format!("I still need evidence to answer: {question}"),
                why:"Selected an unresolved question from recorded experience. No imagined answer was learned as fact.".into(),
                ..Thought::default()
            });
        }
        let seed = match self.draw() % 3 {
            0 => self.curiosity(1).into_iter().next().map(|c| c.word),
            1 => self
                .mind
                .felt
                .present
                .iter()
                .rev()
                .find(|w| w.chars().all(char::is_alphabetic) && w.len() > 2)
                .cloned(),
            _ => None,
        }
        .or_else(|| self.strong_word());
        let Some(seed) = seed else {
            return self.mind.think(Thought {
                time_ms: now_ms(),
                kind: "think".into(),
                why: "Nothing to think about yet: it has learned no words.".into(),
                ..Thought::default()
            });
        };
        self.cortex.reset();
        self.fire_word("think", &seed);
        let chain = self.chain(
            Some(seed.clone()),
            "think",
            0.7,
            &HashSet::new(),
            0.0,
            LONGEST,
        );
        let mut whole = vec![seed.clone()];
        whole.extend(chain.words.iter().cloned());
        let new_sentences = self.new_sentences(&whole);
        self.phase = "thinking".into();
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: "think".into(),
            text: join(&whole),
            novel: !new_sentences.is_empty(),
            confidence: chain.confidence / chain.words.len().max(1) as f32,
            why: format!(
                "Began at \u{201c}{seed}\u{201d} and let its primed cells carry on. {}",
                chain.reasons.join(" ")
            )
            .trim()
            .to_string(),
            seed,
            ..Thought::default()
        })
    }

    /// A word of a memory, drawn by the memories' strength.
    pub(crate) fn strong_word(&mut self) -> Option<String> {
        let at = self.pick_memory()?;
        let words: Vec<String> = self.engrams[at]
            .tokens
            .iter()
            .filter(|t| t.chars().all(char::is_alphabetic) && t.len() > 2)
            .cloned()
            .collect();
        if words.is_empty() {
            return None;
        }
        Some(words[(self.draw() % words.len() as u64) as usize].clone())
    }

    /// Something that never happened, made of things that did. With no
    /// seed, two memories that share a word are spliced: the first is run
    /// up to that word, and from there the second's words are held in mind
    /// and pull the continuation their way. With a seed, the seed is run
    /// and a memory that does not hold it pulls. Learns nothing.
    pub fn imagine(&mut self, seed: &str) -> Thought {
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "imagine".into(),
            seed: seed.into(),
            ..Thought::default()
        };
        let seeded = tokenize(seed);
        let (context, towards, sources, why) = if seeded.is_empty() {
            let Some((a, b, word)) = self.bridge() else {
                thought.why =
                    "Nothing to imagine with yet: it needs two memories that share a word.".into();
                return self.mind.think(thought);
            };
            let first = &self.engrams[a];
            let second = &self.engrams[b];
            let at = first.tokens.iter().position(|t| *t == word).unwrap_or(0);
            let then = second.tokens.iter().position(|t| *t == word).unwrap_or(0);
            let start = first.tokens[..=at]
                .iter()
                .rposition(|t| ends_sentence(t) || matches!(t.as_str(), STORY | BRAIN | USER))
                .map_or(0, |p| p + 1)
                .min(at);
            (
                first.tokens[start..=at].to_vec(),
                second.tokens[then + 1..]
                    .iter()
                    .cloned()
                    .collect::<HashSet<String>>(),
                vec![first.id, second.id],
                format!(
                    "Began in memory {} up to \u{201c}{word}\u{201d}, which memory {} also holds, and held the rest of memory {} in mind.",
                    first.id, second.id, second.id
                ),
            )
        } else {
            let other = self
                .engrams
                .iter()
                .filter(|e| !seeded.iter().any(|w| e.tokens.contains(w)))
                .map(|e| e.id)
                .collect::<Vec<_>>();
            let pulled = if other.is_empty() {
                None
            } else {
                Some(other[(self.draw() % other.len() as u64) as usize])
            };
            let towards: HashSet<String> = pulled
                .and_then(|id| self.engram(id))
                .map(|e| e.tokens.iter().cloned().collect())
                .unwrap_or_default();
            (
                seeded.clone(),
                towards,
                pulled.into_iter().collect(),
                match pulled {
                    Some(id) => format!(
                        "Began from the seed and held memory {id}, which does not hold it, in mind."
                    ),
                    None => "Began from the seed; no other memory to hold in mind.".into(),
                },
            )
        };
        self.cortex.reset();
        for token in &context {
            self.fire_word("imagine", token);
        }
        let chain = self.chain(
            context.last().cloned(),
            "imagine",
            0.5,
            &towards,
            0.25,
            LONGEST,
        );
        let mut whole = context.clone();
        whole.extend(chain.words.iter().cloned());
        let new_sentences = self.new_sentences(&whole);
        thought.text = join(&whole);
        thought.novel = !new_sentences.is_empty();
        thought.confidence = chain.confidence / chain.words.len().max(1) as f32;
        thought.sources = sources;
        thought.why = format!(
            "{why} {}{}",
            if thought.novel {
                "It made a sentence it was never taught. "
            } else {
                ""
            },
            chain.reasons.join(" ")
        )
        .trim()
        .to_string();
        self.phase = "imagining".into();
        self.mind.think(thought)
    }

    /// Two memories and a word both hold, the first drawn by strength.
    pub(crate) fn bridge(&mut self) -> Option<(usize, usize, String)> {
        let a = self.pick_memory()?;
        let mut words: Vec<&String> = self.engrams[a]
            .tokens
            .iter()
            .filter(|t| t.chars().all(char::is_alphabetic) && t.len() > 2)
            .collect();
        words.sort();
        words.dedup();
        let mut options = Vec::new();
        for word in words {
            for (b, other) in self.engrams.iter().enumerate() {
                if b != a && other.tokens.contains(word) && other.tokens != self.engrams[a].tokens {
                    options.push((b, word.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (b, word) = options[(self.draw() % options.len() as u64) as usize].clone();
        Some((a, b, word))
    }

    /// Reasoning over what it has learned: a search for a chain of steps,
    /// each one a word the cortex primes after the ones before, leading
    /// from `from` to `to`. It keeps the few most likely continuations
    /// open at each step and tries the best primed words from each, as
    /// far as its longest memory. Learns nothing.
    pub fn reason(&mut self, from: &str, to: &str) -> Thought {
        let start = tokenize(from);
        let goal: Vec<String> = tokenize(to)
            .into_iter()
            .filter(|t| !matches!(t.as_str(), USER | BRAIN | STORY))
            .collect();
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "reason".into(),
            seed: format!("{from} \u{2192} {to}"),
            ..Thought::default()
        };
        if goal.is_empty() || start.is_empty() {
            thought.why = "Reasoning needs somewhere to start and something to reach.".into();
            return self.mind.think(thought);
        }
        let destination = join(&goal);
        self.cortex.reset();
        for token in &start {
            self.fire_word("reason", token);
        }
        let depth = self
            .engrams
            .iter()
            .map(|e| e.tokens.len())
            .max()
            .unwrap_or(0)
            .clamp(8, 256);
        let mut open: Vec<Open> = vec![(self.cortex.state(), Vec::new(), 0.0)];
        let mut explored = 0;
        let mut found = None;
        let mut closest: Option<(Vec<(String, f32)>, f32)> = None;
        'search: for _ in 0..depth {
            let mut next = Vec::new();
            for (state, path, score) in &open {
                self.cortex.restore(state);
                let guesses = self.guesses(BRANCHING * 2);
                if let Some(g) = guesses.iter().find(|g| {
                    let words: Vec<&str> = path
                        .iter()
                        .map(|p| p.0.as_str())
                        .chain(std::iter::once(g.0.as_str()))
                        .collect();
                    words.ends_with(&goal.iter().map(String::as_str).collect::<Vec<_>>())
                }) {
                    let mut path = path.clone();
                    path.push((g.0.clone(), g.1));
                    found = Some(path);
                    break 'search;
                }
                for g in guesses
                    .iter()
                    .filter(|g| !matches!(g.0.as_str(), USER | BRAIN | STORY))
                    .filter(|g| !path.iter().any(|p| p.0 == g.0) || ends_sentence(&g.0))
                    .take(BRANCHING)
                {
                    self.cortex.restore(state);
                    if self.fire_word("reason", &g.0).is_none() {
                        continue;
                    }
                    explored += 1;
                    let mut path = path.clone();
                    path.push((g.0.clone(), g.1));
                    let score = score + g.1.max(1e-3).ln();
                    let near_goal = self.words.shared(&g.0, goal.last().unwrap()) as f32;
                    if closest.as_ref().is_none_or(|c| near_goal > c.1) {
                        closest = Some((path.clone(), near_goal));
                    }
                    next.push((self.cortex.state(), path, score));
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort_by(|a, b| b.2.total_cmp(&a.2));
            next.truncate(BEAM);
            open = next;
        }
        self.phase = "reasoning".into();
        match found {
            Some(path) => {
                let mut words = start.clone();
                words.extend(path.iter().map(|p| p.0.clone()));
                thought.text = join(&words);
                thought.confidence = (path.iter().map(|p| p.1.max(1e-3).ln()).sum::<f32>()
                    / path.len() as f32)
                    .exp();
                thought.why = format!(
                    "Found a chain of {} learned step{} from \u{201c}{from}\u{201d} to \u{201c}{destination}\u{201d}, trying {explored} continuations; each link is the share of the next word's columns the cortex primed.",
                    path.len(),
                    if path.len() == 1 { "" } else { "s" }
                );
                thought.path = path;
            }
            None => {
                let closest = closest.map(|c| c.0).unwrap_or_default();
                let mut words = start.clone();
                words.extend(closest.iter().map(|p| p.0.clone()));
                thought.text = join(&words);
                thought.why = format!(
                    "No chain from \u{201c}{from}\u{201d} reached \u{201c}{destination}\u{201d} within {depth} steps, after {explored} continuations. Shown: the path that came closest, by columns shared with it."
                );
                thought.path = closest;
            }
        }
        self.mind.think(thought)
    }

    /// Intuition: the cue is fired once and the best-primed word taken at
    /// once, without chaining or weighing, together with how often a
    /// guess that sure has come true before.
    pub fn intuit(&mut self, cue: &str) -> Thought {
        let cue = if cue.trim().is_empty() {
            self.curiosity(1)
                .into_iter()
                .next()
                .map(|c| c.word)
                .or_else(|| self.strong_word())
                .unwrap_or_default()
        } else {
            cue.to_string()
        };
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "intuit".into(),
            seed: cue.clone(),
            ..Thought::default()
        };
        self.cortex.reset();
        for token in tokenize(&cue) {
            self.fire_word("intuit", &token);
        }
        let guesses = self.guesses(5);
        match guesses.first().filter(|g| g.1 >= 0.3) {
            Some(hunch) => {
                thought.text = hunch.0.clone();
                thought.confidence = self.mind.me.expect(hunch.1).unwrap_or(0.0);
                thought.path = guesses.iter().map(|g| (g.0.clone(), g.1)).collect();
                thought.why = match self.mind.me.expect(hunch.1) {
                    Some(right) => format!(
                        "First thing primed after \u{201c}{cue}\u{201d}: {:.0}% of its columns. Its guesses this sure have come true {:.0}% of the time.",
                        hunch.1 * 100.0,
                        right * 100.0
                    ),
                    None => format!(
                        "First thing primed after \u{201c}{cue}\u{201d}: {:.0}% of its columns. It has too little record yet to say how far to trust that.",
                        hunch.1 * 100.0
                    ),
                };
            }
            None => {
                thought.why = format!(
                    "No sufficiently supported prediction after \u{201c}{cue}\u{201d}: no reliable hunch."
                )
            }
        }
        self.phase = "following a hunch".into();
        self.mind.think(thought)
    }

    /// Looks at itself: brings what it was last attending to back to mind
    /// and says, from its own measurements, what state it is in.
    pub fn reflect(&mut self) -> Thought {
        if self.cortex.active().is_empty()
            && let Some(word) = self.mind.felt.present.back().cloned()
        {
            self.fire_word("aware", &word);
        } else {
            let cells = self.cortex.active().to_vec();
            let word = self.mind.felt.present.back().cloned().unwrap_or_default();
            self.lived("aware", &word, cells, 0.0, false);
        }
        let report = self.report();
        self.mind.me.report = report.clone();
        self.phase = "reflecting".into();
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: "reflect".into(),
            text: report,
            confidence: self.mind.me.confidence,
            why: "Read from its own measurements: felt time, episodes, sureness and how far it has held, sleep pressure, curiosity.".into(),
            ..Thought::default()
        })
    }

    /// What it would say of itself now, every clause a measurement.
    pub fn report(&self) -> String {
        let felt = &self.mind.felt;
        let me = &self.mind.me;
        let mut parts = Vec::new();
        parts.push(if felt.asleep {
            format!("I am asleep, {}, cycle {}.", felt.stage, felt.cycle)
        } else if me.doing.is_empty() {
            "I am awake.".to_string()
        } else {
            format!("I am awake; last I was {}.", describe(&me.doing))
        });
        if felt.steps > 0 {
            parts.push(format!(
                "My event-time counter is {:.0} over {} steps; lately each step adds {:.2}, classified as {}.",
                felt.moments,
                felt.steps,
                felt.rate,
                if felt.rate > 1.4 { "stretching" } else if felt.rate < 0.6 { "rushing by" } else { "passing evenly" }
            ));
            parts.push(format!(
                "This is episode {}, which began at \u{201c}{}\u{201d}, {:.1} moments ago.",
                felt.now.id, felt.now.opening, felt.now.felt
            ));
        }
        if !felt.present.is_empty() {
            parts.push(format!(
                "In the present: \u{201c}{}\u{201d}.",
                join(&felt.present.iter().cloned().collect::<Vec<_>>())
            ));
        }
        match self.mind.me.expect(me.confidence) {
            Some(right) => parts.push(format!(
                "My first guesses lately are {:.0}% sure, and guesses that sure have come true {:.0}% of the time.",
                me.confidence * 100.0,
                right * 100.0
            )),
            None => parts.push("I have too little record yet to know how far to trust my guesses.".into()),
        }
        if let Some(insight) = self.mind.me.insight() {
            parts.push(if insight > 0.15 {
                format!("Higher prediction-support bins have been right {:.0} percentage points more often in my observations.", insight * 100.0)
            } else {
                "Being surer hardly makes me more often right: I cannot yet tell what I know from what I do not.".into()
            });
        }
        let change = me.right_now - me.right_before;
        parts.push(
            if change > 0.05 {
                "I am predicting better than I was."
            } else if change < -0.05 {
                "I am predicting worse than I was: this is new to me."
            } else {
                "I predict about as well as I have been."
            }
            .into(),
        );
        if let Some(curious) = self.curiosity(1).first() {
            parts.push(format!(
                "I am most curious about \u{201c}{}\u{201d}.",
                curious.word
            ));
        }
        parts.push(format!(
            "Sleep pressure {:.2}; I fall asleep at {:.1}.",
            felt.pressure,
            mind::DROWSY
        ));
        parts.join(" ")
    }

    /// Words it is most surprised by and still learning about.
    pub fn curiosity(&self, most: usize) -> Vec<Curiosity> {
        let mut wanted: Vec<Curiosity> = self
            .words
            .words
            .iter()
            .filter(|(t, w)| {
                w.seen >= 2 && t.chars().count() > 2 && t.chars().all(char::is_alphabetic)
            })
            .map(|(t, w)| Curiosity {
                word: t.clone(),
                surprise: w.surprise,
                progress: w.surprise_before - w.surprise,
                seen: w.seen,
            })
            .collect();
        let want = |c: &Curiosity| {
            let stagnant = if c.seen > 20 && c.progress.abs() < 0.01 {
                0.1
            } else {
                1.0
            };
            c.surprise * (1.0 + 2.0 * c.progress.max(0.0)) * stagnant
        };
        wanted.sort_by(|a, b| want(b).total_cmp(&want(a)).then(a.word.cmp(&b.word)));
        wanted.truncate(most);
        wanted
    }
}

/// A process, as said of oneself.
fn describe(process: &str) -> &'static str {
    match process {
        "learn" => "being taught",
        "hear" => "listening",
        "speak" | "decide" => "speaking",
        "recall" => "remembering",
        "replay" => "replaying a memory",
        "dream" => "dreaming",
        "think" => "thinking",
        "imagine" => "imagining",
        "reason" => "reasoning",
        "intuit" => "following a hunch",
        "aware" => "looking at myself",
        "live" => "living through what is happening",
        _ => "busy",
    }
}
