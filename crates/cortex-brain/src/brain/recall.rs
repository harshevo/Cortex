//! Remembering: the memory a cue's cells point to is re-fired from the
//! cells it was stored in, and the cortex carries it on.

use super::*;

impl Brain {
    /// Recalls the memory a cue best brings back: the engram whose cells
    /// the cue sets spiking overlap most, replayed from its first cells.
    pub fn recall(&mut self, cue: &str) -> Recall {
        let tokens = tokenize(cue);
        self.cortex.reset();
        let mut cue_cells: HashSet<u32> = HashSet::new();
        let mut last: Vec<u32> = Vec::new();
        for token in &tokens {
            if let Some(activation) = self.fire_word("recall", token) {
                cue_cells.extend(&activation.spiking);
                last = activation.spiking;
            }
        }
        // A cell held by many memories says little about which one is
        // meant; one held by a single memory says a lot.
        let held: Vec<HashSet<u32>> = self
            .engrams
            .iter()
            .map(|e| e.cells.iter().flatten().copied().collect())
            .collect();
        let mut holders: HashMap<u32, u32> = HashMap::new();
        for cells in &held {
            for cell in cells.iter().filter(|c| cue_cells.contains(c)) {
                *holders.entry(*cell).or_default() += 1;
            }
        }
        let weight = |cell: &u32| 1.0 / holders.get(cell).copied().unwrap_or(1) as f32;
        let possible: f32 = cue_cells
            .iter()
            .filter(|c| holders.contains_key(c))
            .map(weight)
            .sum();
        let mut scored: Vec<(usize, f32)> = held
            .iter()
            .enumerate()
            .map(|(at, cells)| {
                let shared: f32 = cue_cells.intersection(cells).map(weight).sum();
                (
                    at,
                    if possible > 0.0 {
                        shared / possible
                    } else {
                        0.0
                    },
                )
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
        let mut recall = Recall {
            time: self.cortex.clock,
            cue: cue.chars().take(200).collect(),
            matched: scored.first().map_or(0.0, |s| s.1),
            runner_up: scored.get(1).map_or(0.0, |s| s.1),
            ..Recall::default()
        };
        let Some(&(at, matched)) = scored.first().filter(|s| s.1 >= RECOGNISED) else {
            recall.why = format!(
                "No memory holds enough of the cells the cue set spiking ({:.0}% at best; {:.0}% needed).",
                recall.matched * 100.0,
                RECOGNISED * 100.0
            );
            self.push_recall(recall.clone());
            return recall;
        };
        let engram = self.engrams[at].clone();
        // Where in the memory the cue's last word was: the step whose
        // stored cells it set spiking most.
        let from = engram
            .cells
            .iter()
            .enumerate()
            .max_by_key(|(step, cells)| {
                (
                    cells.iter().filter(|c| last.contains(c)).count(),
                    usize::MAX - step,
                )
            })
            .map_or(0, |(step, _)| step);
        recall.engram = Some(engram.id);
        self.mind.me.focus_memory = Some(engram.id);
        recall.remembered = engram.text();
        let replay = self.replay_engram(&engram, from, true);
        recall.recalled = replay.0;
        recall.fidelity = replay.1;
        recall.words_right = replay.2;
        recall.words = engram.tokens.len().saturating_sub(from + 1) as u32;
        recall.reignited = engram.cells.get(from).cloned().unwrap_or_default();
        recall.returned = replay.3;
        recall.why = format!(
            "Chose memory {} because it holds {:.0}% of what the cue's cells point to, weighing each cell by how few memories hold it (next best {:.0}%). \
             Re-fired the {} cells it stored for \u{201c}{}\u{201d}; the cortex's own synapses then brought back {:.0}% of the cells it was stored in, and {} of {} words.",
            engram.id,
            matched * 100.0,
            recall.runner_up * 100.0,
            recall.reignited.len(),
            engram.tokens[from],
            recall.fidelity * 100.0,
            recall.words_right,
            recall.words
        );
        let fidelity = recall.fidelity;
        if let Some(e) = self.engrams.get_mut(at) {
            e.recalls += 1;
            e.strength = (e.strength + 0.2).min(5.0);
            e.last_recalled = self.cortex.clock;
            e.fidelity = Some(fidelity);
        }
        self.stats.recalls += 1;
        self.stats.recall_fidelity += 0.2 * (fidelity - self.stats.recall_fidelity);
        self.push_recall(recall.clone());
        self.phase = "recalled a memory".into();
        recall
    }

    pub(crate) fn push_recall(&mut self, recall: Recall) {
        self.recalls.push_back(recall);
        while self.recalls.len() > RECALLS {
            self.recalls.pop_front();
        }
    }

    /// Re-fires an engram's cells at step `from` and lets the cortex carry on,
    /// choosing at each step the word its primed cells point to. Returns
    /// what came back, the share of stored cells that were primed again,
    /// words right, and some of the cells that came back.
    pub(crate) fn replay_engram(
        &mut self,
        engram: &Engram,
        from: usize,
        record: bool,
    ) -> (String, f32, u32, Vec<u32>) {
        let Some(first) = engram.cells.get(from) else {
            return (String::new(), 0.0, 0, Vec::new());
        };
        self.cortex.reset();
        self.reignite("recall", &engram.tokens[from], first);
        let mut recalled = vec![engram.tokens[from].clone()];
        let mut stored = 0;
        let mut back = 0;
        let mut right = 0;
        let mut returned = Vec::new();
        for at in from + 1..engram.tokens.len().min(engram.cells.len()) {
            let primed = self.cortex.primed_cells();
            let held = &engram.cells[at];
            let came: Vec<u32> = held
                .iter()
                .copied()
                .filter(|c| primed.contains(c))
                .collect();
            stored += held.len();
            back += came.len();
            if returned.len() < 64 {
                returned.extend(came.iter().take(64 - returned.len()));
            }
            let guesses = self.guesses(3);
            let Some(choice) = guesses.first().map(|g| g.0.clone()) else {
                break;
            };
            right += u32::from(choice == engram.tokens[at]);
            let Some(columns) = self.code(&choice) else {
                break;
            };
            let activation = self.fire("recall", &choice, &columns, false);
            if record {
                self.steps += 1;
                let step = Step {
                    number: self.steps,
                    time_ms: now_ms(),
                    mode: "recall".into(),
                    token: choice.clone(),
                    predicted: activation.predicted,
                    bursting: activation.bursting,
                    surprise: activation.bursting as f32 / ACTIVE as f32,
                    spiking: activation.spiking.len() as u32,
                    guesses,
                    chosen: Some(choice.clone()),
                    why: format!(
                        "{} of the {} cells stored for \u{201c}{}\u{201d} were primed again",
                        came.len(),
                        held.len(),
                        engram.tokens[at]
                    ),
                    branches: self.cortex.branch_fires(4),
                    changes: activation.changes,
                };
                self.record(step);
            }
            recalled.push(choice);
        }
        let fidelity = if stored == 0 {
            0.0
        } else {
            back as f32 / stored as f32
        };
        (join(&recalled), fidelity, right, returned)
    }
}
