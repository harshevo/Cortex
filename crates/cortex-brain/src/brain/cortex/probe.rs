//! Read-only inference borrows learned wiring and owns only transient activity.

use super::{ACTIVATION, CONNECTED, Cortex, QuickMap, QuickSet};
use std::collections::HashMap;

pub struct Probe<'a> {
    cortex: &'a Cortex,
    restored_targets: Option<Vec<Vec<u32>>>,
    primed: Vec<u32>,
}

impl<'a> Probe<'a> {
    pub fn new(cortex: &'a Cortex) -> Self {
        let restored_targets = (!cortex.indexed).then(|| {
            let mut targets = vec![Vec::new(); cortex.cells.len()];
            for (id, segment) in cortex.segments.iter().enumerate() {
                if let Some(segment) = segment {
                    for synapse in &segment.synapses {
                        if let Some(list) = targets.get_mut(synapse.from as usize) {
                            list.push(id as u32);
                        }
                    }
                }
            }
            targets
        });
        Self {
            cortex,
            restored_targets,
            primed: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.primed.clear();
    }

    pub fn step(&mut self, columns: &[u32]) {
        let mut predicted: QuickMap<u32, Vec<u32>> = QuickMap::default();
        for cell in &self.primed {
            if let Some(unit) = self.cortex.cell(*cell) {
                predicted.entry(unit.column).or_default().push(*cell);
            }
        }
        let mut active: QuickSet<u32> = QuickSet::default();
        for column in columns {
            if let Some(cells) = predicted.get(column) {
                active.extend(cells.iter().copied());
            } else if let Some(cells) = self.cortex.columns.get(*column as usize) {
                active.extend(cells.iter().copied());
            }
        }
        let targets = self
            .restored_targets
            .as_ref()
            .unwrap_or(&self.cortex.targets);
        let mut heard: QuickMap<u32, u32> = QuickMap::default();
        for cell in &active {
            for segment in targets.get(*cell as usize).into_iter().flatten() {
                *heard.entry(*segment).or_default() += 1;
            }
        }
        let mut primed: QuickSet<u32> = QuickSet::default();
        for (id, count) in heard {
            if count < ACTIVATION {
                continue;
            }
            let Some(segment) = self.cortex.segment(id) else {
                continue;
            };
            let connected = segment
                .synapses
                .iter()
                .filter(|s| s.permanence >= CONNECTED && active.contains(&s.from))
                .count();
            if connected >= ACTIVATION as usize {
                primed.insert(segment.cell);
            }
        }
        self.primed = primed.into_iter().collect();
        self.primed.sort_unstable();
    }

    pub fn primed_columns(&self) -> HashMap<u32, u32> {
        let mut columns = HashMap::new();
        for cell in &self.primed {
            if let Some(cell) = self.cortex.cell(*cell) {
                *columns.entry(cell.column).or_default() += 1;
            }
        }
        columns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Brain, words::tokenize};

    #[test]
    fn borrowed_inference_matches_the_original_engine_and_restored_weights() {
        let mut brain = Brain::new();
        for _ in 0..5 {
            for text in [
                "<story> the cat sat on the mat .",
                "<story> the dog sat near the tree .",
            ] {
                brain.learn(text, "test");
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        brain.save(&file).unwrap();
        let restored = Brain::load(&file).unwrap();
        for brain in [&brain, &restored] {
            let original = postcard::to_stdvec(brain).unwrap();
            let mut probe = Probe::new(&brain.cortex);
            let mut reference = brain.cortex.clone();
            for text in [
                "<story> the cat sat on the mat .",
                "the dog sat near the tree .",
            ] {
                probe.reset();
                reference.reset();
                for token in tokenize(text) {
                    let columns = &brain.words.words[&token].columns;
                    probe.step(columns);
                    reference.step(columns, false);
                    assert_eq!(probe.primed_columns(), reference.primed_columns());
                }
            }
            assert_eq!(postcard::to_stdvec(brain).unwrap(), original);
        }
    }
}
