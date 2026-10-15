//! Sleep: memories replayed and learned again, the most in need first;
//! pruning; and dreams, free-running fragments of memories.

use super::*;

/// Memories replayed in one sleep cycle. More in a cycle replays the
/// older memories over the newer ones that share their words, and the
/// newer suffer for it; cycles are cheap, so a night has many.
const REPLAYED: usize = 1;

/// Steps over which a memory stops counting as recent.
const RECENT: f32 = 50_000.0;

impl Brain {
    /// One sleep cycle: replays the few memories most in need of it,
    /// learning from each again, and every few cycles lets words drift,
    /// fades and prunes unused synapses, branches and cells, and lets
    /// memories weaken. Returns what it did.
    ///
    /// Which memories are replayed is not left to how strong they already
    /// are, which would replay the strong ever more and leave the weak to
    /// fade. A memory is in need when it no longer comes back well on its
    /// own, when it was surprising or mattered (good or bad news), and
    /// when it is recent; one replayed often already needs it less.
    pub fn sleep(&mut self) -> String {
        self.stats.sleeps += 1;
        if self.engrams.is_empty() {
            self.phase = "asleep: no memories yet".into();
            return self.phase.clone();
        }
        let mut chosen: Vec<usize> = Vec::new();
        for _ in 0..REPLAYED.min(self.engrams.len()) {
            if let Some(at) = self.most_in_need(&chosen) {
                chosen.push(at);
            }
        }
        let mut did = Vec::new();
        for at in chosen {
            let engram = self.engrams[at].clone();
            // How much of it comes back on its own now, before it is
            // rehearsed: that is what says whether it needed it.
            let (_, fidelity, _, _) = self.replay_engram(&engram, 0, false);
            let cells = self.run(&engram.tokens, "replay");
            self.stats.replays += 1;
            if let Some(e) = self.engrams.get_mut(at) {
                e.replays += 1;
                e.fidelity = Some(fidelity);
                // Held now by the cells that hold it today: words drift,
                // and what replay chose is where the memory lives now.
                if cells.len() == e.tokens.len() {
                    e.cells = cells;
                }
            }
            did.push(format!(
                "replayed memory {} ({:.0}% came back on its own): \u{201c}{}\u{201d}",
                engram.id,
                fidelity * 100.0,
                engram::join(&engram.tokens.iter().take(12).cloned().collect::<Vec<_>>())
            ));
        }
        let moved = self.organise();
        if moved > 0 {
            did.push(format!(
                "{moved} columns moved nearer the columns they fire with"
            ));
        }
        // Words drift seldom: every drift moves cells that memories rely on.
        let mut drifted = 0;
        if self.stats.sleeps.is_multiple_of(30) {
            drifted = self.words.drift();
            self.stats.drifted += drifted as u64;
        }
        if self.stats.sleeps.is_multiple_of(6) {
            let (synapses, branches, died) = self.cortex.prune(IDLE, FADING);
            self.forget_cells(&died);
            // Memories weaken unless recalled, but are not forgotten for
            // that: only a full budget forgets.
            for engram in &mut self.engrams {
                engram.strength = (engram.strength * 0.995).max(0.1);
            }
            for value in self.mind.values.learned.values_mut() {
                *value *= 0.995;
            }
            self.mind.values.tidy();
            let forgotten = self.keep_within_budget();
            did.push(format!(
                "{drifted} words drifted; pruned {synapses} synapses, {branches} branches, {} cells; forgot {forgotten} memories",
                died.len()
            ));
        }
        self.phase = format!("asleep: {}", did.join("; "));
        self.phase.clone()
    }

    /// How much a memory needs replaying, from what is known of it.
    fn need(&self, engram: &Engram) -> f32 {
        let note = self.mind.notes.get(&engram.id);
        let surprise = note.map_or(0.3, |n| n.surprise);
        let worth = note.map_or(0.0, |n| n.worth.abs());
        let age = self.cortex.clock.saturating_sub(engram.created) as f32;
        let recent = (-age / RECENT).exp();
        let fading = 1.0 - engram.fidelity.unwrap_or(0.5);
        let need =
            0.1 + fading + 0.5 * surprise + 0.5 * worth + 0.5 * recent + 0.05 * engram.strength;
        need / (1.0 + 0.05 * engram.replays as f32).sqrt()
    }

    /// A memory drawn with odds by its need, not one of `taken`.
    fn most_in_need(&mut self, taken: &[usize]) -> Option<usize> {
        let needs: Vec<f32> = self
            .engrams
            .iter()
            .enumerate()
            .map(|(at, e)| {
                if taken.contains(&at) {
                    0.0
                } else {
                    self.need(e)
                }
            })
            .collect();
        let total: f32 = needs.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = (self.draw() % 10_000) as f32 / 10_000.0 * total;
        for (at, need) in needs.iter().enumerate() {
            pick -= need;
            if pick <= 0.0 && *need > 0.0 {
                return Some(at);
            }
        }
        needs.iter().rposition(|need| *need > 0.0)
    }

    /// What it does when nobody is talking to it, called every few
    /// seconds. Awake, its mind wanders: a thought, an imagining, a hunch,
    /// or a look at itself, while sleep pressure builds. Once drowsy it
    /// sleeps in cycles, faithful replay first and dreams more often as
    /// the night goes on, until the pressure is paid off. Returns what it
    /// did.
    pub fn idle(&mut self) -> String {
        let felt = &mut self.mind.felt;
        if !felt.asleep && felt.pressure >= mind::DROWSY && !self.engrams.is_empty() {
            felt.asleep = true;
            felt.cycle = 0;
        }
        if self.mind.felt.asleep {
            let felt = &mut self.mind.felt;
            felt.cycle += 1;
            let cycle = felt.cycle;
            // Early in the night mostly deep replay, later more dreaming.
            let dreaming = cycle.is_multiple_of(4) || (cycle > 8 && cycle.is_multiple_of(2));
            felt.stage = if dreaming {
                "dreaming (REM)".into()
            } else {
                "deep sleep (replay)".into()
            };
            felt.pressure = (felt.pressure - 0.08).max(0.0);
            let did = if dreaming {
                let dream = self.dream();
                format!("dreamed: {}", dream.text)
            } else {
                self.sleep()
            };
            let felt = &mut self.mind.felt;
            if felt.pressure < mind::RESTED {
                felt.asleep = false;
                felt.stage = String::new();
                felt.nights += 1;
                self.phase = format!("woke after {cycle} cycles of sleep");
            }
            return did;
        }
        self.mind.felt.pressure = (self.mind.felt.pressure + 0.02).min(mind::MOST_PRESSURE);
        let roll = self.draw() % 20;
        let thought = match roll {
            0..=9 => self.think(),
            10..=13 => self.imagine(""),
            14..=16 => self.intuit(""),
            _ => self.reflect(),
        };
        format!("{}: {}", thought.kind, thought.text)
    }

    /// A dream: a fragment of a memory is re-fired, and the cortex runs
    /// free from it, loosely, so that less likely words get their turn.
    /// Where the dream reaches a word another memory holds, it sometimes
    /// jumps into that memory there; when it loses the thread it starts a
    /// new scene. Nothing is learned from a dream.
    pub fn dream(&mut self) -> Thought {
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "dream".into(),
            ..Thought::default()
        };
        let Some(at) = self.pick_memory() else {
            thought.why = "No memories to dream of.".into();
            return self.mind.think(thought);
        };
        let mut scene = self.engrams[at].clone();
        let mut from = self.fragment(&scene);
        self.cortex.reset();
        let Some(cells) = scene.cells.get(from).cloned() else {
            thought.why = "The memory it began with holds no cells.".into();
            return self.mind.think(thought);
        };
        self.reignite("dream", &scene.tokens[from], &cells);
        thought.seed = scene.tokens[from].clone();
        thought.sources.push(scene.id);
        let mut words = vec![scene.tokens[from].clone()];
        let mut scenes = 1;
        let mut confidence = 0.0;
        for _ in 0..DREAMED {
            let guesses = self.guesses(8);
            if guesses.is_empty() {
                if scenes >= 3 {
                    break;
                }
                let Some(next) = self.pick_memory() else {
                    break;
                };
                scene = self.engrams[next].clone();
                from = self.fragment(&scene);
                let Some(cells) = scene.cells.get(from).cloned() else {
                    break;
                };
                self.reignite("dream", &scene.tokens[from], &cells);
                thought.sources.push(scene.id);
                words.push(scene.tokens[from].clone());
                scenes += 1;
                continue;
            }
            let best = guesses[0].1;
            let mut near: Vec<&(String, f32, u32)> = guesses
                .iter()
                .filter(|g| {
                    g.1 >= (best * 0.4).max(0.3) && !matches!(g.0.as_str(), STORY | BRAIN | USER)
                })
                .filter(|g| {
                    words
                        .iter()
                        .rev()
                        .take(6)
                        .filter(|word| **word == g.0)
                        .count()
                        < 2
                })
                .collect();
            if near.is_empty() {
                break;
            }
            let total: f32 = near.iter().map(|g| g.1 * g.1).sum();
            let mut draw = (self.draw() % 10_000) as f32 / 10_000.0 * total;
            let pick = near
                .iter()
                .position(|g| {
                    draw -= g.1 * g.1;
                    draw <= 0.0
                })
                .unwrap_or(near.len() - 1);
            let choice = near.swap_remove(pick).clone();
            confidence += choice.1;
            if self.fire_word("dream", &choice.0).is_none() {
                break;
            }
            words.push(choice.0.clone());
            if ends_sentence(&choice.0) && words.len() > 8 {
                break;
            }
            // A word another memory holds may carry the dream into it.
            if scenes < 3 && self.draw().is_multiple_of(4) {
                let elsewhere: Vec<(usize, usize)> = self
                    .engrams
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.id != scene.id)
                    .filter_map(|(i, e)| {
                        e.tokens.iter().position(|t| *t == choice.0).map(|p| (i, p))
                    })
                    .filter(|(i, p)| self.engrams[*i].cells.get(*p).is_some())
                    .collect();
                if !elsewhere.is_empty() {
                    let (i, p) = elsewhere[(self.draw() % elsewhere.len() as u64) as usize];
                    scene = self.engrams[i].clone();
                    let cells = scene.cells[p].clone();
                    self.reignite("dream", &choice.0, &cells);
                    thought.sources.push(scene.id);
                    scenes += 1;
                }
            }
        }
        let new_sentences = self.new_sentences(&words);
        thought.text = join(&words);
        thought.novel = !new_sentences.is_empty();
        thought.confidence = confidence / words.len().max(1) as f32;
        thought.why = format!(
            "Re-fired memory {} at \u{201c}{}\u{201d} and let the cortex run loosely; it passed through {} scene{} from memories {}.",
            thought.sources[0],
            thought.seed,
            scenes,
            if scenes == 1 { "" } else { "s" },
            thought
                .sources
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.phase = "dreaming".into();
        self.mind.think(thought)
    }

    /// A place in a memory to start a dream scene: a word, not a mark.
    pub(crate) fn fragment(&mut self, engram: &Engram) -> usize {
        let words: Vec<usize> = (0..engram.tokens.len().min(engram.cells.len()))
            .filter(|at| !matches!(engram.tokens[*at].as_str(), USER | BRAIN | STORY))
            .collect();
        if words.is_empty() {
            return 0;
        }
        words[(self.draw() % words.len() as u64) as usize]
    }

    /// A memory, drawn with odds by its strength.
    pub(crate) fn pick_memory(&mut self) -> Option<usize> {
        let total: f32 = self.engrams.iter().map(|e| e.strength).sum();
        if self.engrams.is_empty() || total <= 0.0 {
            return None;
        }
        let mut pick = (self.draw() % 10_000) as f32 / 10_000.0 * total;
        for (at, engram) in self.engrams.iter().enumerate() {
            pick -= engram.strength;
            if pick <= 0.0 {
                return Some(at);
            }
        }
        Some(self.engrams.len() - 1)
    }

    /// Lets the map's columns trade places towards their words.
    pub(crate) fn organise(&mut self) -> usize {
        let area = self.cortex.area();
        if self.slots.len() != area {
            self.slots = map::slots(area);
        }
        let mut random = self.draw();
        let slots = std::mem::take(&mut self.slots);
        let made = self
            .mind
            .map
            .organise(&self.words, &slots, ORGANISE, &mut random);
        self.slots = slots;
        made
    }
}
