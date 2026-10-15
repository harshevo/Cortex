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
use std::hash::{BuildHasherDefault, Hasher};

use serde::{Deserialize, Serialize};

use crate::brain::words::START_COLUMNS;

pub mod probe;

/// A hasher for small integer keys: the default one is built to resist
/// attack, which ids from inside the brain do not need, and was a tenth
/// of every step.
#[derive(Default, Clone, Copy)]
pub struct Quick(u64);

impl Hasher for Quick {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*byte)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(n)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

pub type QuickMap<K, V> = HashMap<K, V, BuildHasherDefault<Quick>>;
pub type QuickSet<K> = HashSet<K, BuildHasherDefault<Quick>>;

/// Cells a column is born with. It grows more whenever its cells are full:
/// there is no upper limit but the brain's memory budget.
pub const START_CELLS: usize = 4;

/// Branches a cell holds before its column grows another cell rather than
/// load it further.
const FULL: usize = 12;

/// Branches one cell can hold, and synapses one branch: what keeps a
/// branch about one context. Capacity grows through more cells and
/// columns, not through larger branches.
const MAX_SEGMENTS: usize = 128;
const MAX_SYNAPSES: usize = 64;

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
/// of them from being learned. Then the next word is not expected, its
/// columns burst, every cell in them fires, and the word's other contexts
/// are woken with it: "the mat" at the end of one sentence starts priming
/// what followed "the mat" at the start of another.
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
    pub column: u32,
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
    pub column: u32,
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

/// The cortex's activity at one moment, without its synapses.
#[derive(Debug, Clone, Default)]
pub struct State {
    active: Vec<u32>,
    winners: Vec<u32>,
    primed: Vec<(u32, u32)>,
    matching: Vec<(u32, u32)>,
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

    /// For each cell, the branches with a synapse from it: a step looks
    /// only at the branches that hear the cells now spiking.
    #[serde(skip)]
    targets: Vec<Vec<u32>>,
    #[serde(skip)]
    indexed: bool,
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
    /// The step each branch was last heard in, so that a step lists each
    /// branch once without sorting every synapse it heard, and how many
    /// spiking cells it heard then.
    #[serde(skip)]
    heard_at: Vec<u32>,
    #[serde(skip)]
    heard: Vec<u16>,
    #[serde(skip)]
    hearing: u32,
    /// The step each cell last spiked in, for the same reason.
    #[serde(skip)]
    spiked_at: Vec<u32>,

    /// How readily synapses change now, 1 as usual: the brain raises it
    /// when it is surprised or something matters, as neuromodulators do,
    /// and lowers it when what it knows holds.
    #[serde(skip, default = "usual")]
    pub plasticity: f32,
}

fn usual() -> f32 {
    1.0
}

impl Default for Cortex {
    fn default() -> Self {
        let mut cortex = Self {
            columns: vec![Vec::new(); START_COLUMNS],
            cells: Vec::new(),
            segments: Vec::new(),
            free_cells: Vec::new(),
            free_segments: Vec::new(),
            clock: 0,
            totals: Totals::default(),
            random: 0x2545_F491_4F6C_DD1D,
            targets: Vec::new(),
            indexed: true,
            active: Vec::new(),
            winners: Vec::new(),
            primed: Vec::new(),
            matching: Vec::new(),
            heard_at: Vec::new(),
            heard: Vec::new(),
            hearing: 0,
            spiked_at: Vec::new(),
            plasticity: 1.0,
        };
        for column in 0..START_COLUMNS {
            for _ in 0..START_CELLS {
                cortex.new_cell(column as u32);
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

    /// Columns in the cortex now.
    pub fn area(&self) -> usize {
        self.columns.len()
    }

    /// Grows `count` new columns, each with its first cells. New words are
    /// given codes over the whole, larger area.
    pub fn grow_columns(&mut self, count: usize) {
        for _ in 0..count {
            self.columns.push(Vec::new());
            let column = (self.columns.len() - 1) as u32;
            for _ in 0..START_CELLS {
                self.new_cell(column);
            }
        }
    }

    /// Builds the index of which branches hear which cell, after loading.
    fn index(&mut self) {
        if self.indexed {
            return;
        }
        self.indexed = true;
        self.targets = vec![Vec::new(); self.cells.len()];
        for (id, segment) in self.segments.iter().enumerate() {
            let Some(segment) = segment else { continue };
            for synapse in &segment.synapses {
                if let Some(list) = self.targets.get_mut(synapse.from as usize) {
                    list.push(id as u32);
                }
            }
        }
    }

    fn listen(&mut self, from: u32, segment: u32) {
        if self.targets.len() <= from as usize {
            self.targets.resize(from as usize + 1, Vec::new());
        }
        self.targets[from as usize].push(segment);
    }

    fn unlisten(&mut self, from: u32, segment: u32) {
        if let Some(list) = self.targets.get_mut(from as usize)
            && let Some(at) = list.iter().position(|s| *s == segment)
        {
            list.swap_remove(at);
        }
    }

    /// Roughly how many bytes it holds.
    pub fn footprint(&self) -> u64 {
        let (synapses, _) = self.synapse_counts();
        (self.cells.len() * 64 + self.segments.len() * 72 + synapses * 12) as u64
    }

    fn new_cell(&mut self, column: u32) -> u32 {
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

    pub fn column_cells(&self, column: u32) -> &[u32] {
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
    pub fn primed_columns(&self) -> HashMap<u32, u32> {
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

    /// What is spiking and primed now, to come back to: reasoning tries
    /// several continuations from the same point.
    pub fn state(&self) -> State {
        State {
            active: self.active.clone(),
            winners: self.winners.clone(),
            primed: self.primed.clone(),
            matching: self.matching.clone(),
        }
    }

    pub fn restore(&mut self, state: &State) {
        self.active.clone_from(&state.active);
        self.winners.clone_from(&state.winners);
        self.primed.clone_from(&state.primed);
        self.matching.clone_from(&state.matching);
    }

    /// Every live synapse: from cell, to cell, connected.
    pub fn wiring(&self) -> Vec<(u32, u32, bool)> {
        self.segments
            .iter()
            .flatten()
            .flat_map(|s| {
                s.synapses
                    .iter()
                    .map(move |x| (x.from, s.cell, x.permanence >= CONNECTED))
            })
            .collect()
    }

    /// The column of each cell id, `u32::MAX` for a dead one.
    pub fn cell_columns(&self) -> Vec<u32> {
        self.cells
            .iter()
            .map(|c| c.as_ref().map_or(u32::MAX, |c| c.column))
            .collect()
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
        self.winners.sort_unstable();
        self.winners.dedup();
        self.predict();
    }

    /// One word arrives as its columns. With `learn`, synapses change and
    /// the network may grow.
    pub fn step(&mut self, columns: &[u32], learn: bool) -> Activation {
        self.clock += 1;
        self.totals.steps += 1;
        let previous_active: QuickSet<u32> = self.active.iter().copied().collect();
        // Sorted, so that reinforcing can search it.
        let previous_winners = self.winners.clone();
        let firing: QuickSet<u32> = columns.iter().copied().collect();

        let mut primed_by_column: QuickMap<u32, Vec<u32>> = QuickMap::default();
        for (segment, _) in &self.primed {
            if let Some(s) = self.segment(*segment)
                && let Some(c) = self.cell(s.cell)
            {
                primed_by_column.entry(c.column).or_default().push(*segment);
            }
        }
        let mut matching_by_column: QuickMap<u32, Vec<(u32, u32)>> = QuickMap::default();
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
                let best = matching_by_column.get(column).and_then(|m| {
                    m.iter()
                        .max_by_key(|(id, potential)| (*potential, u32::MAX - id))
                });
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
                    // Not learning (imagining, recalling), a burst has no
                    // winner to choose: choosing one would draw on the
                    // cortex's chance, and imagining must leave learning
                    // as it found it.
                    None if !learn => self.columns[*column as usize].first().copied(),
                    None => {
                        let cell = self.least_used(*column, learn, &mut activation.changes);
                        if learn && !previous_winners.is_empty() {
                            if self
                                .cell(cell)
                                .is_some_and(|c| c.segments.len() >= MAX_SEGMENTS)
                            {
                                self.drop_oldest_segment(cell);
                            }
                            let segment = self.new_segment(cell);
                            activation.changes.segments_grown += 1;
                            self.grow(
                                segment,
                                &previous_winners,
                                NEW_SYNAPSES,
                                &mut activation.changes,
                            );
                        }
                        Some(cell)
                    }
                };
                if let Some(winner) = winner {
                    winners.push(winner);
                }
            }
        }

        // Branches that primed or nearly primed their cell for a word that
        // did not come. Both: a branch primed by 13 or 14 strong synapses
        // is not "nearly matching" by count, and was never told it was
        // wrong.
        if learn {
            let mut wrong: Vec<u32> = primed_by_column
                .iter()
                .filter(|(column, _)| !firing.contains(*column))
                .flat_map(|(_, segments)| segments.iter().copied())
                .chain(
                    matching_by_column
                        .iter()
                        .filter(|(column, _)| !firing.contains(*column))
                        .flat_map(|(_, segments)| segments.iter().map(|(s, _)| *s)),
                )
                .collect();
            wrong.sort_unstable();
            wrong.dedup();
            let punishment = WRONG * self.plasticity;
            for segment in wrong {
                let Some(s) = self.segments[segment as usize].as_mut() else {
                    continue;
                };
                for synapse in &mut s.synapses {
                    if previous_active.contains(&synapse.from) {
                        synapse.permanence = (synapse.permanence - punishment).max(0.0);
                    }
                }
                activation.changes.punished += 1;
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

    /// Works out which branches the cells now spiking prime. Only the
    /// branches that hear one of them are looked at, and of those only the
    /// ones that hear enough spiking cells to matter are read synapse by
    /// synapse: a word like "the" is heard by thousands of branches, most
    /// through one or two synapses.
    fn predict(&mut self) {
        self.index();
        if self.spiked_at.len() < self.cells.len() {
            self.spiked_at.resize(self.cells.len(), 0);
        }
        if self.heard_at.len() < self.segments.len() {
            self.heard_at.resize(self.segments.len(), 0);
            self.heard.resize(self.segments.len(), 0);
        }
        self.hearing = self.hearing.wrapping_add(1);
        if self.hearing == 0 {
            self.heard_at.fill(0);
            self.spiked_at.fill(0);
            self.hearing = 1;
        }
        let hearing = self.hearing;
        for cell in &self.active {
            if let Some(at) = self.spiked_at.get_mut(*cell as usize) {
                *at = hearing;
            }
        }
        let mut heard = Vec::new();
        for cell in &self.active {
            for id in self.targets.get(*cell as usize).into_iter().flatten() {
                let at = *id as usize;
                if at >= self.heard_at.len() {
                    continue;
                }
                if self.heard_at[at] != hearing {
                    self.heard_at[at] = hearing;
                    self.heard[at] = 0;
                    heard.push(*id);
                }
                self.heard[at] = self.heard[at].saturating_add(1);
            }
        }
        let least = ACTIVATION.min(MATCHING) as u16;
        self.primed.clear();
        self.matching.clear();
        for id in heard {
            if self.heard[id as usize] < least {
                continue;
            }
            let Some(segment) = self.segment(id) else {
                continue;
            };
            let mut connected = 0;
            let mut potential = 0;
            for synapse in &segment.synapses {
                if self.spiked_at.get(synapse.from as usize) == Some(&hearing) {
                    potential += 1;
                    if synapse.permanence >= CONNECTED {
                        connected += 1;
                    }
                }
            }
            if connected >= ACTIVATION {
                self.primed.push((id, connected));
            }
            if potential >= MATCHING {
                self.matching.push((id, potential));
            }
        }
        self.primed.sort_unstable();
        self.matching.sort_unstable();
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
        previous_active: &QuickSet<u32>,
        previous_winners: &[u32],
        changes: &mut Changes,
    ) {
        let increment = INCREMENT * self.plasticity;
        let decrement = DECREMENT * self.plasticity;
        let Some(s) = self.segments[segment as usize].as_mut() else {
            return;
        };
        // Every synapse from a cell that spiked counts as in place. When the
        // step before burst, every cell in its columns spiked, among them
        // the cells this branch learned in another context; growing more
        // synapses then, onto this context's winners, would make the
        // branch hear both contexts, and the two would become one.
        let mut present = 0;
        for synapse in &mut s.synapses {
            if previous_active.contains(&synapse.from) {
                synapse.permanence = (synapse.permanence + increment).min(1.0);
                changes.strengthened += 1;
                present += 1;
            } else {
                synapse.permanence = (synapse.permanence - decrement).max(0.0);
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
            let Some(s) = self.segment(segment) else {
                return;
            };
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
        for from in chosen {
            let Some(s) = self.segments[segment as usize].as_mut() else {
                return;
            };
            let mut dropped = None;
            if s.synapses.len() >= MAX_SYNAPSES {
                // The weakest gives way.
                if let Some(at) = s
                    .synapses
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.permanence.total_cmp(&b.1.permanence))
                    .map(|(at, _)| at)
                {
                    dropped = Some(s.synapses.swap_remove(at).from);
                }
            }
            s.synapses.push(Synapse {
                from,
                permanence: INITIAL,
            });
            if let Some(old) = dropped {
                self.unlisten(old, segment);
            }
            self.listen(from, segment);
            changes.synapses_grown += 1;
        }
    }

    /// The cell in a column with the fewest branches; a new one when all
    /// are full and the column has room.
    fn least_used(&mut self, column: u32, learn: bool, changes: &mut Changes) -> u32 {
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
        if learn && full {
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
        for synapse in &s.synapses {
            self.unlisten(synapse.from, segment);
        }
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
    /// the cells that died). A dead cell's id is given to the next cell
    /// born, so whoever kept it must let it go.
    pub fn prune(&mut self, idle: u64, fading: f32) -> (u64, u64, Vec<u32>) {
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
        // Synapses were dropped above without telling the index.
        self.indexed = false;
        self.index();
        let mut died = Vec::new();
        for column in 0..self.columns.len() {
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
                    died.push(*id);
                }
            }
        }
        self.totals.cells_died += died.len() as u64;
        (
            self.totals.synapses_pruned - before.0,
            self.totals.segments_pruned - before.1,
            died,
        )
    }

    /// Every cell's column, spikes and branches, for drawing.
    pub fn columns_view(&self) -> Vec<(u32, u32, u32, u64)> {
        (0..self.columns.len())
            .map(|column| {
                let cells = &self.columns[column];
                let branches = cells
                    .iter()
                    .filter_map(|id| self.cell(*id))
                    .map(|c| c.segments.len() as u32)
                    .sum();
                let spikes = cells
                    .iter()
                    .filter_map(|id| self.cell(*id))
                    .map(|c| c.spikes)
                    .sum();
                (column as u32, cells.len() as u32, branches, spikes)
            })
            .collect()
    }

    pub fn is_sound(&self) -> bool {
        self.columns.len() >= START_COLUMNS
            && self
                .columns
                .iter()
                .all(|cells| cells.iter().all(|id| self.cell(*id).is_some()))
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
