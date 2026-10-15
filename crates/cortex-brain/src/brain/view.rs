//! What the brain shows of itself: the snapshot the app draws, every
//! cell and synapse, and a short account for the teacher.

use super::*;

impl Brain {
    /// Every cell and synapse and each memory's cells, for drawing.
    pub fn wiring(&mut self) -> Wiring {
        self.mind.map.fit(self.cortex.area());
        let synapses = self.cortex.wiring();
        Wiring {
            clock: self.cortex.clock,
            area: self.cortex.area(),
            slot_of: self.mind.map.slot_of.clone(),
            cells: self.cortex.cell_columns(),
            synapse_count: synapses.len(),
            synapses: mind::pack(synapses),
            engrams: self
                .engrams
                .iter()
                .map(|e| {
                    let mut cells: Vec<u32> = e.cells.iter().flatten().copied().collect();
                    cells.sort_unstable();
                    cells.dedup();
                    (e.id, cells)
                })
                .collect(),
        }
    }

    pub fn snapshot(&self) -> NeuralSnapshot {
        let (synapses, connected) = self.cortex.synapse_counts();
        let columns = self
            .cortex
            .columns_view()
            .into_iter()
            .map(|(_, cells, branches, _)| (cells, branches))
            .collect();
        let mut primed: Vec<u32> = self.cortex.primed_columns().into_keys().collect();
        primed.sort_unstable();
        let engrams = self
            .engrams
            .iter()
            .rev()
            .enumerate()
            .map(|(at, e)| {
                // Every memory is listed; the columns of the newest are
                // enough for the view, which takes every memory's cells
                // from the wiring.
                let mut columns: Vec<u32> = e
                    .cells
                    .iter()
                    .flatten()
                    .filter(|_| at < ENGRAM_COLUMNS)
                    .filter_map(|c| self.cortex.cell(*c).map(|c| c.column))
                    .collect();
                columns.sort_unstable();
                columns.dedup();
                EngramView {
                    id: e.id,
                    text: e.text().chars().take(240).collect(),
                    source: e.source.clone(),
                    strength: e.strength,
                    recalls: e.recalls,
                    replays: e.replays,
                    cells: e.cell_count() as u32,
                    columns,
                    fidelity: e.fidelity,
                    created: e.created,
                }
            })
            .collect();
        NeuralSnapshot {
            stats: self.stats.clone(),
            totals: self.cortex.totals.clone(),
            clock: self.cortex.clock,
            vocabulary: self.words.len(),
            cells: self.cortex.cells_alive(),
            branches: self.cortex.segments_alive(),
            synapses,
            connected,
            permanences: self.cortex.permanences(),
            columns,
            recent: self.recent.iter().cloned().collect(),
            primed,
            engrams,
            engram_count: self.engrams.len(),
            trace: self.trace.iter().cloned().collect(),
            recalls: self.recalls.iter().cloned().collect(),
            said: self.said.iter().cloned().collect(),
            curious: self.curiosity(10),
            phase: self.phase.clone(),
            area: self.cortex.area(),
            footprint: self.footprint(),
            budget: self.budget(),
            mind: self.mind_view(),
            cognition: self.mind.cognition.view(),
        }
    }

    /// The mind for the view, with the last few seconds of activity.
    pub(crate) fn mind_view(&self) -> MindView {
        let since = now_ms().saturating_sub(ACTIVITY_WINDOW_MS);
        let mut slot_of = self.mind.map.slot_of.clone();
        while slot_of.len() < self.cortex.area() {
            slot_of.push(slot_of.len() as u32);
        }
        MindView {
            felt: self.mind.felt.clone(),
            me: SelfModel {
                report: self.report(),
                ..self.mind.me.clone()
            },
            expected: self.mind.me.expect(self.mind.me.confidence),
            insight: self.mind.me.insight(),
            thoughts: self.mind.thoughts.iter().cloned().collect(),
            activity: self
                .activity
                .iter()
                .filter(|a| a.time_ms >= since)
                .cloned()
                .collect(),
            slot_of,
            swaps: self.mind.map.swaps,
            attention: self.mind.attention.held.clone(),
            values: self.weighty(12),
            advice: self.mind.advice.clone(),
            senses: self.mind.senses.clone(),
            streams: self.streams(),
        }
    }

    /// The things that matter most to it, good or bad, given or learned.
    fn weighty(&self, most: usize) -> Vec<(String, f32)> {
        let values = &self.mind.values;
        let mut all: Vec<(String, f32)> = values
            .meanings
            .iter()
            .chain(&values.learned)
            .map(|(token, worth)| (token.clone(), *worth))
            .collect();
        all.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()).then(a.0.cmp(&b.0)));
        all.truncate(most);
        all
    }

    /// A short account for the teacher: how far it has come, what it
    /// said lately, and what it is curious about.
    pub fn progress(&self) -> serde_json::Value {
        serde_json::json!({
            "vocabulary": self.words.len(),
            "memories": self.engrams.len(),
            "lessons": self.stats.lessons,
            "next_word_right_recently": (self.stats.recent_right * 100.0).round() / 100.0,
            "commonest_word_baseline_recently": (self.stats.recent_baseline * 100.0).round() / 100.0,
            "recall_fidelity": (self.stats.recall_fidelity * 100.0).round() / 100.0,
            "curious_about": self.curiosity(6).into_iter().map(|c| c.word).collect::<Vec<_>>(),
            "recently_said": self.said.iter().rev().take(3).map(|u| &u.text).collect::<Vec<_>>(),
            "learning": self.mind.cognition.last_study,
            "learned_statements": self.mind.cognition.knowledge.statements.len(),
            "learned_dialogue": self.mind.cognition.knowledge.dialogue.len(),
            "formula_rules": self.mind.cognition.knowledge.formulas.iter().map(|f| format!("{} = {}",f.target,f.expression)).collect::<Vec<_>>(),
            "unanswered_questions": self.mind.cognition.questions,
            "calibration_samples": self.mind.me.sureness.iter().map(|bin| u64::from(bin.0)).sum::<u64>(),
        })
    }
}

/// Which columns each word fires, for the view's legend.
pub fn columns_of(brain: &Brain, word: &str) -> Vec<u32> {
    brain
        .words
        .words
        .get(word)
        .map(|w| w.columns.clone())
        .unwrap_or_default()
}

/// Counts per column in a list, for drawing heat.
pub fn heat(recent: &[Vec<u32>]) -> HashMap<u32, f32> {
    let mut heat = HashMap::new();
    for (age, columns) in recent.iter().rev().enumerate() {
        let weight = 0.75_f32.powi(age as i32);
        for column in columns {
            let value = heat.entry(*column).or_insert(0.0);
            *value = f32::max(*value, weight);
        }
    }
    heat
}
