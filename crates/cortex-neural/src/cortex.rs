//! The cortex: columns of spiking cells, each cell with dendritic branches
//! (segments) that hold synapses from other cells.
//!
//! A word fires its columns. Within a firing column, the cells that a
//! branch had primed (predicted) are the ones that spike: the word was
//! expected, in this context. A column nobody primed bursts: every cell in
//! it spikes, the network is surprised, and one cell is chosen to learn
//! the context so that next time it will be primed.
//!
//! A branch primes its cell when enough of its strong synapses come from
//! cells that just spiked: a dendritic spike. Learning moves a synapse's
//! permanence up when its source spiked before its cell fired, and down
//! when it did not; a synapse above [`CONNECTED`] counts. Branches grow
//! where context was missed, synapses grow onto the cells that spiked,
//! columns grow new cells when theirs are full, and what is unused fades
//! and is pruned during sleep.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::words::COLUMNS;

pub const START_CELLS: usize = 4;
pub const MAX_CELLS: usize = 32;

/// Branches a cell may hold before its column grows another cell.
const FULL: usize = 12;
const MAX_SEGMENTS: usize = 48;
const MAX_SYNAPSES: usize = 32;

/// Strong synapses from just-spiked cells a branch needs to prime its
/// cell, and any synapses at all for it to count as nearly matching. A
/// surprised column reuses only a branch that nearly matches; one that
/// matches half its context belongs to another context, and pulling it
/// over would break that one, so a new branch is grown instead.
pub const ACTIVATION: u32 = 13;
pub const MATCHING: u32 = 15;

/// Synapses a branch grows towards the cells that spiked.
const NEW_SYNAPSES: usize = 20;

pub const CONNECTED: f32 = 0.5;
/// A new synapse connects the second time its context comes round.
const INITIAL: f32 = 0.4;
const INCREMENT: f32 = 0.1;
const DECREMENT: f32 = 0.1;

/// The punishment of a branch that primed its cell for nothing. Kept
/// small: after a word that many words can follow, all but one of their
/// branches are wrong every time, and a large punishment would stop any
/// of them from being learned.
const WRONG: f32 = 0.002;

/// Below this a synapse is pruned.
const PRUNED: f32 = 0.02;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synapse {
    pub from: u32,
    pub permanence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub cell: u32,
    pub synapses: Vec<Synapse>,
    pub created: u64,
    pub last_used: u64,
    pub fired: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub column: u16,
    pub segments: Vec<u32>,
    pub born: u64,
    pub last_spike: u64,
    pub spikes: u64,
}

/// One dendritic spike: a branch that primed its cell, with the
/// arithmetic of why.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BranchFire {
    pub cell: u32,
    pub column: u16,
    pub segment: u32,
    /// Strong synapses from cells that just spiked, of all its synapses.
    pub matched: u32,
    pub synapses: u32,
    pub needed: u32,
}

/// What one step changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Changes {
    pub strengthened: u32,
    pub weakened: u32,
    pub synapses_grown: u32,
    pub segments_grown: u32,
    pub cells_grown: u32,
    pub punished: u32,
}

/// What happened when a word reached the cortex.
#[derive(Debug, Clone, Default)]
pub struct Activation {
    /// Columns that fired with a primed cell, and that burst.
    pub predicted: u32,
    pub bursting: u32,
    pub spiking: Vec<u32>,
    pub winners: Vec<u32>,
    pub changes: Changes,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub cells_grown: u64,
    pub cells_died: u64,
    pub segments_grown: u64,
    pub segments_pruned: u64,
    pub synapses_grown: u64,
    pub synapses_pruned: u64,
    pub strengthened: u64,
    pub weakened: u64,
    pub spikes: u64,
    pub dendritic_spikes: u64,
    pub steps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cortex {
    columns: Vec<Vec<u32>>,
    cells: Vec<Option<Cell>>,
    segments: Vec<Option<Segment>>,
    free_cells: Vec<u32>,
    free_segments: Vec<u32>,
    pub clock: u64,
    pub totals: Totals,
    random: u64,

    #[serde(skip)]
    active: Vec<u32>,
    #[serde(skip)]
    winners: Vec<u32>,
    /// Branches primed by the cells now spiking, with their strong match.
    #[serde(skip)]
    primed: Vec<(u32, u32)>,
    /// Branches nearly matching, with their match counting weak synapses.
    #[serde(skip)]
    matching: Vec<(u32, u32)>,
}

impl Default for Cortex {
    fn default() -> Self {
        let mut cortex = Self {
            columns: vec![Vec::new(); COLUMNS],
            cells: Vec::new(),
            segments: Vec::new(),
            free_cells: Vec::new(),
            free_segments: Vec::new(),
            clock: 0,
            totals: Totals::default(),
            random: 0x2545_F491_4F6C_DD1D,
            active: Vec::new(),
            winners: Vec::new(),
            primed: Vec::new(),
            matching: Vec::new(),
        };
        for column in 0..COLUMNS {
            for _ in 0..START_CELLS {
                cortex.new_cell(column as u16);
            }
        }
        cortex.totals.cells_grown = 0;
        cortex
    }
}

impl Cortex {
    fn draw(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    fn new_cell(&mut self, column: u16) -> u32 {
        let cell = Cell {
            column,
            segments: Vec::new(),
            born: self.clock,
            last_spike: self.clock,
            spikes: 0,
        };
        let id = match self.free_cells.pop() {
            Some(id) => {
                self.cells[id as usize] = Some(cell);
                id
            }
            None => {
                self.cells.push(Some(cell));
                (self.cells.len() - 1) as u32
            }
        };
        self.columns[column as usize].push(id);
        self.totals.cells_grown += 1;
        id
    }

    fn new_segment(&mut self, cell: u32) -> u32 {
        let segment = Segment {
            cell,
            synapses: Vec::new(),
            created: self.clock,
            last_used: self.clock,
            fired: 0,
        };
        let id = match self.free_segments.pop() {
            Some(id) => {
                self.segments[id as usize] = Some(segment);
                id
            }
            None => {
                self.segments.push(Some(segment));
                (self.segments.len() - 1) as u32
            }
        };
        if let Some(c) = self.cells[cell as usize].as_mut() {
            c.segments.push(id);
        }
        self.totals.segments_grown += 1;
        id
    }

    pub fn cell(&self, id: u32) -> Option<&Cell> {
        self.cells.get(id as usize).and_then(Option::as_ref)
    }

    pub fn segment(&self, id: u32) -> Option<&Segment> {
        self.segments.get(id as usize).and_then(Option::as_ref)
    }

    pub fn column_cells(&self, column: u16) -> &[u32] {
        &self.columns[column as usize]
    }

    pub fn cells_alive(&self) -> usize {
        self.cells.iter().filter(|c| c.is_some()).count()
    }

    pub fn segments_alive(&self) -> usize {
        self.segments.iter().filter(|s| s.is_some()).count()
    }

    pub fn synapse_counts(&self) -> (usize, usize) {
        let mut all = 0;
        let mut connected = 0;
        for segment in self.segments.iter().flatten() {
            all += segment.synapses.len();
            connected += segment
                .synapses
                .iter()
                .filter(|s| s.permanence >= CONNECTED)
                .count();
        }
        (all, connected)
    }

    /// Synapse permanences in ten bins from 0 to 1.
    pub fn permanences(&self) -> [u32; 10] {
        let mut bins = [0; 10];
        for synapse in self.segments.iter().flatten().flat_map(|s| &s.synapses) {
            bins[((synapse.permanence * 10.0) as usize).min(9)] += 1;
        }
        bins
    }

    pub fn active(&self) -> &[u32] {
        &self.active
    }

    pub fn winners(&self) -> &[u32] {
        &self.winners
    }

    /// The cells primed for the next step.
    pub fn primed_cells(&self) -> HashSet<u32> {
        self.primed
            .iter()
            .filter_map(|(segment, _)| self.segment(*segment).map(|s| s.cell))
            .collect()
    }

    /// Predicted cells per column, for reading out the next word.
    pub fn primed_columns(&self) -> HashMap<u16, u32> {
        let mut columns = HashMap::new();
        for cell in self.primed_cells() {
            if let Some(c) = self.cell(cell) {
                *columns.entry(c.column).or_default() += 1;
            }
        }
        columns
    }

    /// The strongest dendritic spikes priming the next step.
    pub fn branch_fires(&self, most: usize) -> Vec<BranchFire> {
        let mut fires: Vec<BranchFire> = self
            .primed
            .iter()
            .filter_map(|(id, matched)| {
                let segment = self.segment(*id)?;
                let cell = self.cell(segment.cell)?;
                Some(BranchFire {
                    cell: segment.cell,
                    column: cell.column,
                    segment: *id,
                    matched: *matched,
                    synapses: segment.synapses.len() as u32,
                    needed: ACTIVATION,
                })
            })
            .collect();
        fires.sort_by(|a, b| b.matched.cmp(&a.matched).then(a.segment.cmp(&b.segment)));
        fires.truncate(most);
        fires
    }

    /// Forgets the context: the next word starts a new sequence.
    pub fn reset(&mut self) {
        self.active.clear();
        self.winners.clear();
        self.primed.clear();
        self.matching.clear();
    }

    /// Makes these cells the ones spiking now, as if they had just fired,
    /// and works out what they prime. For recall: re-firing a memory's
    /// stored cells.
    pub fn reactivate(&mut self, cells: &[u32]) {
        self.active = cells
            .iter()
            .copied()
            .filter(|c| self.cell(*c).is_some())
            .collect();
        self.winners = self.active.clone();
        self.predict();
    }

    /// One word arrives as its columns. With `learn`, synapses change and
    /// the network may grow.
    pub fn step(&mut self, columns: &[u16], learn: bool) -> Activation {
        self.clock += 1;
        self.totals.steps += 1;
        let previous_active: HashSet<u32> = self.active.iter().copied().collect();
        let previous_winners = self.winners.clone();
        let firing: HashSet<u16> = columns.iter().copied().collect();

        let mut primed_by_column: HashMap<u16, Vec<u32>> = HashMap::new();
        for (segment, _) in &self.primed {
            if let Some(s) = self.segment(*segment)
                && let Some(c) = self.cell(s.cell)
            {
                primed_by_column.entry(c.column).or_default().push(*segment);
            }
        }
        let mut matching_by_column: HashMap<u16, Vec<(u32, u32)>> = HashMap::new();
        for (segment, potential) in &self.matching {
            if let Some(s) = self.segment(*segment)
                && let Some(c) = self.cell(s.cell)
            {
                matching_by_column
                    .entry(c.column)
                    .or_default()
                    .push((*segment, *potential));
            }
        }

        let mut activation = Activation::default();
        let mut active = Vec::new();
        let mut winners = Vec::new();

        for column in columns {
            if let Some(segments) = primed_by_column.get(column) {
                activation.predicted += 1;
                let mut cells: Vec<u32> = Vec::new();
                for segment in segments {
                    let Some(cell) = self.segment(*segment).map(|s| s.cell) else {
                        continue;
                    };
                    if !cells.contains(&cell) {
                        cells.push(cell);
                    }
                    if learn {
                        self.reinforce(
                            *segment,
                            &previous_active,
                            &previous_winners,
                            &mut activation.changes,
                        );
                    }
                }
                active.extend(&cells);
                winners.extend(&cells);
            } else {
                activation.bursting += 1;
                active.extend(self.columns[*column as usize].iter().copied());
                let best = matching_by_column
                    .get(column)
                    .and_then(|m| m.iter().max_by_key(|(id, potential)| (*potential, u32::MAX - id)));
                let winner = match best {
                    Some((segment, _)) => {
                        let segment = *segment;
                        if learn {
                            self.reinforce(
                                segment,
                                &previous_active,
                                &previous_winners,
                                &mut activation.changes,
                            );
                        }
                        self.segment(segment).map(|s| s.cell)
                    }
                    None => {
                        let cell = self.least_used(*column, learn, &mut activation.changes);
                        if learn && !previous_winners.is_empty() {
                            if self.cell(cell).is_some_and(|c| c.segments.len() >= MAX_SEGMENTS) {
                                self.drop_oldest_segment(cell);
                            }
                            let segment = self.new_segment(cell);
                            activation.changes.segments_grown += 1;
                            self.grow(segment, &previous_winners, NEW_SYNAPSES, &mut activation.changes);
                        }
                        Some(cell)
                    }
                };
                if let Some(winner) = winner {
                    winners.push(winner);
                }
            }
        }

        // Branches that primed their cell for a word that did not come.
        if learn {
            for (column, segments) in &matching_by_column {
                if firing.contains(column) {
                    continue;
                }
                for (segment, _) in segments {
                    let Some(s) = self.segments[*segment as usize].as_mut() else {
                        continue;
                    };
                    for synapse in &mut s.synapses {
                        if previous_active.contains(&synapse.from) {
                            synapse.permanence = (synapse.permanence - WRONG).max(0.0);
                        }
                    }
                    activation.changes.punished += 1;
                }
            }
        }

        let clock = self.clock;
        for cell in &active {
            if let Some(c) = self.cells[*cell as usize].as_mut() {
                c.last_spike = clock;
                c.spikes += 1;
            }
        }
        self.totals.spikes += active.len() as u64;
        self.totals.strengthened += u64::from(activation.changes.strengthened);
        self.totals.weakened += u64::from(activation.changes.weakened);
        self.totals.synapses_grown += u64::from(activation.changes.synapses_grown);

        winners.sort_unstable();
        winners.dedup();
        self.active = active;
        self.winners = winners;
        activation.spiking = self.active.clone();
        activation.winners = self.winners.clone();
        self.predict();
        self.totals.dendritic_spikes += self.primed.len() as u64;
        activation
    }

    /// Works out which branches the cells now spiking prime.
    fn predict(&mut self) {
        let mut spiking = vec![false; self.cells.len()];
        for cell in &self.active {
            if let Some(flag) = spiking.get_mut(*cell as usize) {
                *flag = true;
            }
        }
        self.primed.clear();
        self.matching.clear();
        for (id, segment) in self.segments.iter().enumerate() {
            let Some(segment) = segment else { continue };
            let mut connected = 0;
            let mut potential = 0;
            for synapse in &segment.synapses {
                if spiking.get(synapse.from as usize).copied().unwrap_or(false) {
                    potential += 1;
                    if synapse.permanence >= CONNECTED {
                        connected += 1;
                    }
                }
            }
            if connected >= ACTIVATION {
                self.primed.push((id as u32, connected));
            }
            if potential >= MATCHING {
                self.matching.push((id as u32, potential));
            }
        }
        let clock = self.clock;
        for (id, _) in &self.primed {
            if let Some(s) = self.segments[*id as usize].as_mut() {
                s.last_used = clock;
                s.fired += 1;
            }
        }
    }

    /// Strengthens a branch's synapses from cells that spiked, weakens the
    /// rest, and grows it towards the cells that won, up to its size.
    fn reinforce(
        &mut self,
        segment: u32,
        previous_active: &HashSet<u32>,
        previous_winners: &[u32],
        changes: &mut Changes,
    ) {
        let Some(s) = self.segments[segment as usize].as_mut() else {
            return;
        };
        // Synapses from the winners count as in place: when the step before
        // burst, every cell in its columns spiked, but next time only the
        // winners will, so the branch must hear them in particular.
        let mut present = 0;
        for synapse in &mut s.synapses {
            if previous_active.contains(&synapse.from) {
                synapse.permanence = (synapse.permanence + INCREMENT).min(1.0);
                changes.strengthened += 1;
                present += usize::from(previous_winners.contains(&synapse.from));
            } else {
                synapse.permanence = (synapse.permanence - DECREMENT).max(0.0);
                changes.weakened += 1;
            }
        }
        s.last_used = self.clock;
        let missing = NEW_SYNAPSES.saturating_sub(present);
        if missing > 0 {
            self.grow(segment, previous_winners, missing, changes);
        }
    }

    /// Grows up to `count` synapses on a branch from winners it does not
    /// already hear.
    fn grow(&mut self, segment: u32, winners: &[u32], count: usize, changes: &mut Changes) {
        let mut candidates: Vec<u32> = {
            let Some(s) = self.segment(segment) else { return };
            winners
                .iter()
                .copied()
                .filter(|w| !s.synapses.iter().any(|x| x.from == *w) && *w != s.cell)
                .collect()
        };
        let mut chosen = Vec::new();
        while chosen.len() < count && !candidates.is_empty() {
            let at = (self.draw() % candidates.len() as u64) as usize;
            chosen.push(candidates.swap_remove(at));
        }
        let Some(s) = self.segments[segment as usize].as_mut() else {
            return;
        };
        for from in chosen {
            if s.synapses.len() >= MAX_SYNAPSES {
                // The weakest gives way.
                if let Some(at) = s
                    .synapses
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.permanence.total_cmp(&b.1.permanence))
                    .map(|(at, _)| at)
                {
                    s.synapses.swap_remove(at);
                }
            }
            s.synapses.push(Synapse {
                from,
                permanence: INITIAL,
            });
            changes.synapses_grown += 1;
        }
    }

    /// The cell in a column with the fewest branches; a new one when all
    /// are full and the column has room.
    fn least_used(&mut self, column: u16, learn: bool, changes: &mut Changes) -> u32 {
        // Ties are broken at random, so that different contexts do not all
        // fall on the same cell and become indistinguishable.
        let cells = self.columns[column as usize].clone();
        let least = cells
            .iter()
            .filter_map(|id| self.cell(*id).map(|c| c.segments.len()))
            .min();
        let tied: Vec<u32> = cells
            .iter()
            .copied()
            .filter(|id| self.cell(*id).map(|c| c.segments.len()) == least)
            .collect();
        let fewest = if tied.is_empty() {
            None
        } else {
            let at = (self.draw() % tied.len() as u64) as usize;
            Some(tied[at])
        };
        let full = fewest
            .and_then(|id| self.cell(id))
            .is_none_or(|c| c.segments.len() >= FULL);
        if learn && full && cells.len() < MAX_CELLS {
            changes.cells_grown += 1;
            return self.new_cell(column);
        }
        match fewest {
            Some(id) => id,
            None => self.new_cell(column),
        }
    }

    fn drop_oldest_segment(&mut self, cell: u32) {
        let oldest = self.cell(cell).and_then(|c| {
            c.segments
                .iter()
                .copied()
                .min_by_key(|s| self.segment(*s).map_or(0, |s| s.last_used))
        });
        if let Some(segment) = oldest {
            self.remove_segment(segment);
        }
    }

    fn remove_segment(&mut self, segment: u32) {
        let Some(s) = self.segments[segment as usize].take() else {
            return;
        };
        self.totals.synapses_pruned += s.synapses.len() as u64;
        self.totals.segments_pruned += 1;
        if let Some(c) = self.cells[s.cell as usize].as_mut() {
            c.segments.retain(|x| *x != segment);
        }
        self.free_segments.push(segment);
    }

    /// Sleep's housekeeping: synapses on branches unused for `idle` steps
    /// weaken by `fading`; synapses below the floor are pruned, empty
    /// branches removed, and a cell beyond its column's first ones that
    /// has no branches, has not spiked for `idle` steps and that no
    /// synapse comes from dies. Returns (synapses pruned, branches pruned,
    /// cells died).
    pub fn prune(&mut self, idle: u64, fading: f32) -> (u64, u64, u64) {
        let clock = self.clock;
        let before = (self.totals.synapses_pruned, self.totals.segments_pruned);
        let mut empty = Vec::new();
        let mut pruned = 0;
        for (id, segment) in self.segments.iter_mut().enumerate() {
            let Some(segment) = segment else { continue };
            if clock.saturating_sub(segment.last_used) > idle {
                for synapse in &mut segment.synapses {
                    synapse.permanence *= fading;
                }
            }
            let count = segment.synapses.len();
            segment.synapses.retain(|s| s.permanence >= PRUNED);
            pruned += (count - segment.synapses.len()) as u64;
            if segment.synapses.is_empty() {
                empty.push(id as u32);
            }
        }
        self.totals.synapses_pruned += pruned;
        for segment in empty {
            self.remove_segment(segment);
        }
        let heard: HashSet<u32> = self
            .segments
            .iter()
            .flatten()
            .flat_map(|s| s.synapses.iter().map(|x| x.from))
            .collect();
        let mut died = 0;
        for column in 0..COLUMNS {
            let cells = self.columns[column].clone();
            for (at, id) in cells.iter().enumerate() {
                if at < START_CELLS {
                    continue;
                }
                let dead = self.cell(*id).is_some_and(|c| {
                    c.segments.is_empty() && clock.saturating_sub(c.last_spike) > idle
                }) && !heard.contains(id);
                if dead {
                    self.cells[*id as usize] = None;
                    self.columns[column].retain(|x| x != id);
                    self.free_cells.push(*id);
                    self.active.retain(|x| x != id);
                    self.winners.retain(|x| x != id);
                    died += 1;
                }
            }
        }
        self.totals.cells_died += died;
        (
            self.totals.synapses_pruned - before.0,
            self.totals.segments_pruned - before.1,
            died,
        )
    }

    /// Every cell's column, spikes and branches, for drawing.
    pub fn columns_view(&self) -> Vec<(u16, u32, u32, u64)> {
        (0..COLUMNS)
            .map(|column| {
                let cells = &self.columns[column];
                let branches = cells
                    .iter()
                    .filter_map(|id| self.cell(*id))
                    .map(|c| c.segments.len() as u32)
                    .sum();
                let spikes = cells.iter().filter_map(|id| self.cell(*id)).map(|c| c.spikes).sum();
                (column as u16, cells.len() as u32, branches, spikes)
            })
            .collect()
    }

    pub fn is_sound(&self) -> bool {
        self.columns.len() == COLUMNS
            && self.columns.iter().all(|cells| {
                cells.len() <= MAX_CELLS
                    && cells.iter().all(|id| self.cell(*id).is_some())
            })
            && self.segments.iter().flatten().all(|s| {
                self.cell(s.cell).is_some()
                    && s.synapses.len() <= MAX_SYNAPSES
                    && s.synapses.iter().all(|x| {
                        x.permanence.is_finite()
                            && (0.0..=1.0).contains(&x.permanence)
                            && (x.from as usize) < self.cells.len()
                    })
            })
    }
}
