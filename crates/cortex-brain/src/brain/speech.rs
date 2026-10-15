//! Speaking: the primed cells choose word after word, weighing near
//! rivals before committing to one.

use super::*;

impl Brain {
    /// Says what its primed cells lead to after `prompt`, a word at a
    /// time. It does not learn from what it says.
    pub fn say(&mut self, prompt: &str) -> Utterance {
        let tokens = tokenize(prompt);
        self.cortex.reset();
        for token in &tokens {
            self.fire_word("speak", token);
        }
        let chain = self.chain(
            tokens.last().cloned(),
            "speak",
            0.85,
            &HashSet::new(),
            0.0,
            LONGEST,
        );
        let said = chain.words;
        // A continuation is judged with the prompt's unfinished sentence.
        let open = tokens
            .iter()
            .rposition(|t| ends_sentence(t) || matches!(t.as_str(), BRAIN | STORY))
            .map_or(0, |at| at + 1);
        let whole: Vec<String> = tokens[open.min(tokens.len())..]
            .iter()
            .chain(&said)
            .cloned()
            .collect();
        let new_sentences = self.new_sentences(&whole);
        let words = said.len().max(1) as f32;
        let utterance = Utterance {
            time_ms: now_ms(),
            prompt: prompt.chars().take(300).collect(),
            text: join(&said),
            novel: !new_sentences.is_empty(),
            new_sentences,
            confidence: chain.confidence / words,
            why: if chain.reasons.is_empty() {
                format!(
                    "{} words, each the one its primed cells pointed to",
                    said.len()
                )
            } else {
                chain.reasons.join(" ")
            },
        };
        self.stats.spoken += 1;
        self.stats.novel += u64::from(utterance.novel);
        self.said.push_back(utterance.clone());
        while self.said.len() > SAID {
            self.said.pop_front();
        }
        self.phase = "spoke".into();
        utterance
    }

    /// Finished sentences of three or more words in `tokens` that it was
    /// never taught.
    pub(crate) fn new_sentences(&self, tokens: &[String]) -> Vec<String> {
        let finished = tokens
            .iter()
            .rposition(|t| ends_sentence(t))
            .map_or(0, |at| at + 1);
        sentences(&tokens[..finished])
            .into_iter()
            .filter(|s| s.len() >= 3 && !self.sentences.contains(&sentence_hash(s)))
            .map(|s| join(&s))
            .collect()
    }

    /// Lets the primed cells choose word after word, as `process`, from
    /// wherever the cortex is now; `last` is the word just before, to
    /// fall back on when the thread is lost.
    ///
    /// Every word primed at least `reach` as strongly as the best is a
    /// candidate. Among them it decides by how strongly each is primed,
    /// plus `pull` for a word in `towards` (a goal held in mind), plus a
    /// little chance. The first, best-primed word is its intuition; when
    /// the decision goes against it, that is said.
    pub(crate) fn chain(
        &mut self,
        last: Option<String>,
        process: &str,
        reach: f32,
        towards: &HashSet<String>,
        pull: f32,
        longest: usize,
    ) -> Chain {
        let mut said: Vec<String> = Vec::new();
        let mut confidence = 0.0;
        let mut reasons = Vec::new();
        let mut regrouped = 0;
        for _ in 0..longest {
            let mut guesses = self.guesses(8);
            // Lost the thread: the last word alone, out of context, primes
            // whatever has ever followed it.
            if guesses.is_empty()
                && regrouped < 2
                && let Some(last) = said.last().cloned().or(last.clone())
                && let Some(columns) = self.code(&last)
            {
                regrouped += 1;
                self.cortex.reset();
                self.fire(process, &last, &columns, false);
                guesses = self.guesses(8);
                reasons.push(format!(
                    "Lost the thread after \u{201c}{last}\u{201d}; fired it alone, out of context, to find what can follow it."
                ));
            }
            let Some(best) = guesses.first().cloned() else {
                reasons.push(if said.is_empty() {
                    "No branch was primed: it has not learned what follows this.".to_string()
                } else {
                    "No branch was primed any more, so it stopped.".to_string()
                });
                break;
            };
            // A word it just said twice is passed over.
            let near: Vec<(String, f32, u32)> = guesses
                .iter()
                .filter(|g| g.1 >= best.1 * reach)
                .filter(|g| said.iter().rev().take(6).filter(|s| **s == g.0).count() < 2)
                .filter(|g| !matches!(g.0.as_str(), STORY | BRAIN))
                .cloned()
                .collect();
            // Before committing, each near rival is tried one step on: a
            // word after which the cortex expects nothing leads nowhere,
            // and one that leads on to something it knows well is worth
            // more. Speaking and imagining deliberate; free association
            // does not.
            let deliberate = matches!(process, "speak" | "imagine") && near.len() > 1;
            let here = self.cortex.state();
            let mut weighed: Vec<((String, f32, u32), f32)> = Vec::new();
            for g in near {
                let chance = (self.draw() % 1000) as f32 / 1000.0;
                let held = if towards.contains(&g.0) {
                    pull
                } else if self.mind.attention.holds(&g.0) {
                    0.05
                } else {
                    0.0
                };
                let ahead = if deliberate {
                    self.cortex.restore(&here);
                    match self.words.words.get(&g.0).map(|w| w.columns.clone()) {
                        Some(columns) => {
                            self.fire("decide", &g.0, &columns, false);
                            self.guesses(1).first().map_or(0.0, |next| next.1)
                        }
                        None => 0.0,
                    }
                } else {
                    0.0
                };
                let worth =
                    g.1 + held + 0.3 * ahead + 0.1 * self.mind.values.worth(&g.0) + 0.08 * chance;
                weighed.push((g, worth));
            }
            if deliberate {
                self.cortex.restore(&here);
            }
            weighed.sort_by(|a, b| b.1.total_cmp(&a.1));
            let Some((choice, _)) = weighed.first().cloned() else {
                reasons.push("Every likely word would have repeated itself, so it stopped.".into());
                break;
            };
            confidence += choice.1;
            if weighed.len() > 1 {
                self.decided(&choice.0, &best, &weighed);
            }
            let runner = guesses.iter().find(|g| g.0 != choice.0);
            let why = match runner {
                Some(r) => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed ({:.0}%); next \u{201c}{}\u{201d} {:.0}%{}",
                    choice.0,
                    choice.2,
                    choice.1 * 100.0,
                    r.0,
                    r.1 * 100.0,
                    if towards.contains(&choice.0) {
                        "; it was also held in mind"
                    } else {
                        ""
                    }
                ),
                None => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed, nothing else was",
                    choice.0, choice.2
                ),
            };
            let Some(columns) = self.code(&choice.0) else {
                break;
            };
            let activation = self.fire(process, &choice.0, &columns, false);
            self.steps += 1;
            let step = Step {
                number: self.steps,
                time_ms: now_ms(),
                mode: process.into(),
                token: choice.0.clone(),
                predicted: activation.predicted,
                bursting: activation.bursting,
                surprise: activation.bursting as f32 / ACTIVE as f32,
                spiking: activation.spiking.len() as u32,
                guesses: guesses.clone(),
                chosen: Some(choice.0.clone()),
                why,
                branches: self.cortex.branch_fires(4),
                changes: activation.changes,
            };
            self.record(step);
            if choice.0 == USER {
                break;
            }
            said.push(choice.0.clone());
            if ends_sentence(&choice.0) {
                break;
            }
        }
        Chain {
            words: said,
            confidence,
            reasons,
        }
    }

    /// A choice between near rivals: the cells primed for the chosen word
    /// are lit as deciding, and going against the first hunch is noted.
    pub(crate) fn decided(
        &mut self,
        chosen: &str,
        hunch: &(String, f32, u32),
        weighed: &[((String, f32, u32), f32)],
    ) {
        let columns: HashSet<u32> = self
            .words
            .words
            .get(chosen)
            .map(|w| w.columns.iter().copied().collect())
            .unwrap_or_default();
        let mut cells: Vec<u32> = self
            .cortex
            .primed_cells()
            .into_iter()
            .filter(|c| {
                self.cortex
                    .cell(*c)
                    .is_some_and(|c| columns.contains(&c.column))
            })
            .collect();
        cells.sort_unstable();
        self.lived("decide", chosen, cells, 0.0, false);
        if chosen != hunch.0 {
            let options = weighed
                .iter()
                .map(|(g, worth)| (g.0.clone(), *worth))
                .collect();
            self.mind.think(Thought {
                time_ms: now_ms(),
                kind: "decide".into(),
                seed: hunch.0.clone(),
                text: chosen.into(),
                confidence: weighed.first().map_or(0.0, |w| w.0.1),
                path: options,
                why: format!(
                    "Intuition said \u{201c}{}\u{201d} ({:.0}% primed); weighing {} near rivals, it went with \u{201c}{chosen}\u{201d}.",
                    hunch.0,
                    hunch.1 * 100.0,
                    weighed.len()
                ),
                ..Thought::default()
            });
        }
    }

    /// A turn of conversation: it hears what was said, as experience, then
    /// answers.
    pub fn converse(&mut self, text: &str) -> Utterance {
        self.chat("default", text)
    }
}
