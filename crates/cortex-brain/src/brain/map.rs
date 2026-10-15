//! Where each column sits on the drawn cortex.
//!
//! The columns are laid out on a sheet shaped like a brain seen from the
//! side, one slot each. Which column takes which slot is not fixed: during
//! sleep, columns that fire together for the same words trade places
//! towards each other, so related words come to occupy neighbouring
//! ground, as a topographic map forms in a real cortex. Nothing else
//! depends on the layout; it is for seeing.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::brain::words::Words;

/// Lobes of the outline, as ellipses: centre x, centre y, radius x,
/// radius y, in a frame 1 wide and 0.8 high.
const LOBES: [(f32, f32, f32, f32); 5] = [
    (0.47, 0.36, 0.40, 0.29),  // cerebrum
    (0.24, 0.42, 0.20, 0.20),  // frontal
    (0.55, 0.56, 0.27, 0.13),  // temporal
    (0.80, 0.63, 0.12, 0.08),  // cerebellum
    (0.66, 0.72, 0.035, 0.09), // stem
];

/// Height of the frame, for a width of 1.
pub const HEIGHT: f32 = 0.8;

fn inside(x: f32, y: f32) -> bool {
    LOBES.iter().any(|(cx, cy, rx, ry)| {
        let dx = (x - cx) / rx;
        let dy = (y - cy) / ry;
        dx * dx + dy * dy <= 1.0
    })
}

/// `count` points spread evenly over the outline, on a hexagonal lattice
/// as fine as it needs to be. Always the same for the same count.
pub fn slots(count: usize) -> Vec<(f32, f32)> {
    if count == 0 {
        return Vec::new();
    }
    let lattice = |spacing: f32| -> Vec<(f32, f32)> {
        let mut points = Vec::new();
        let row = spacing * 0.866;
        let mut y = row / 2.0;
        let mut odd = false;
        while y < HEIGHT {
            let mut x = if odd { spacing } else { spacing / 2.0 };
            while x < 1.0 {
                if inside(x, y) {
                    points.push((x, y));
                }
                x += spacing;
            }
            y += row;
            odd = !odd;
        }
        points
    };
    // The largest spacing that still fits every column.
    let (mut low, mut high) = (0.0005_f32, 0.2_f32);
    for _ in 0..40 {
        let middle = (low + high) / 2.0;
        if lattice(middle).len() >= count {
            low = middle;
        } else {
            high = middle;
        }
    }
    let mut points = lattice(low);
    // Spare points are dropped evenly, not from one edge.
    if points.len() > count {
        let step = points.len() as f64 / count as f64;
        points = (0..count)
            .map(|at| points[(at as f64 * step) as usize])
            .collect();
    }
    points
}

/// The spacing between neighbouring slots for `count` columns, roughly.
pub fn spacing(count: usize) -> f32 {
    let area: f32 = 0.33;
    (area / count.max(1) as f32 / 0.866).sqrt()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Map {
    /// The slot of each column.
    pub slot_of: Vec<u32>,
    /// Places traded so far.
    pub swaps: u64,
}

impl Map {
    /// Gives new columns the next free slots.
    pub fn fit(&mut self, area: usize) {
        if self.slot_of.len() > area {
            self.slot_of.truncate(area);
        }
        while self.slot_of.len() < area {
            self.slot_of.push(self.slot_of.len() as u32);
        }
    }

    /// Lets columns trade places towards the other columns of the words
    /// they fire for. Tries `trials` trades; returns those made.
    /// `points` are [`slots`] for the cortex's area.
    pub fn organise(
        &mut self,
        words: &Words,
        points: &[(f32, f32)],
        trials: usize,
        random: &mut u64,
    ) -> usize {
        let area = points.len();
        self.fit(area);
        if area == 0 || words.is_empty() {
            return 0;
        }
        let mut column_at = vec![0u32; area];
        for (column, slot) in self.slot_of.iter().enumerate() {
            column_at[*slot as usize] = column as u32;
        }
        let codes: Vec<&Vec<u32>> = words.words.values().map(|w| &w.columns).collect();
        let mut words_of: HashMap<u32, Vec<usize>> = HashMap::new();
        for (word, columns) in codes.iter().enumerate() {
            for column in columns.iter() {
                words_of.entry(*column).or_default().push(word);
            }
        }
        let position = |slot_of: &[u32], column: u32| points[slot_of[column as usize] as usize];
        let centre = |slot_of: &[u32], word: usize| {
            let columns = codes[word];
            let (mut x, mut y) = (0.0, 0.0);
            for column in columns.iter() {
                let (px, py) = position(slot_of, *column);
                x += px;
                y += py;
            }
            let n = columns.len().max(1) as f32;
            (x / n, y / n)
        };
        // A coarse grid of slots, to find the slot nearest a point.
        let cell = spacing(area) * 3.0;
        let key = |x: f32, y: f32| ((x / cell) as i32, (y / cell) as i32);
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (slot, (x, y)) in points.iter().enumerate() {
            grid.entry(key(*x, *y)).or_default().push(slot as u32);
        }
        let mut draw = || {
            *random ^= *random << 13;
            *random ^= *random >> 7;
            *random ^= *random << 17;
            *random
        };
        let distance =
            |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        let mut made = 0;
        for _ in 0..trials {
            let word = (draw() % codes.len() as u64) as usize;
            let columns = codes[word];
            if columns.is_empty() {
                continue;
            }
            let a = columns[(draw() % columns.len() as u64) as usize];
            let target = centre(&self.slot_of, word);
            let (gx, gy) = key(target.0, target.1);
            let reach = distance(position(&self.slot_of, a), target);
            // The nearest slot to the word's centre, closer than `a` is,
            // that the word does not already hold.
            let mut nearest: Option<(u32, f32)> = None;
            let mut ring = 0i32;
            while nearest.is_none() && (ring as f32 - 1.0) * cell <= reach {
                for dx in -ring..=ring {
                    for dy in -ring..=ring {
                        if dx.abs().max(dy.abs()) != ring {
                            continue;
                        }
                        for slot in grid.get(&(gx + dx, gy + dy)).into_iter().flatten() {
                            if columns.contains(&column_at[*slot as usize]) {
                                continue;
                            }
                            let d = distance(points[*slot as usize], target);
                            if d < reach && nearest.is_none_or(|n| d < n.1) {
                                nearest = Some((*slot, d));
                            }
                        }
                    }
                }
                ring += 1;
            }
            let Some((slot, _)) = nearest else { continue };
            let b = column_at[slot as usize];
            if a == b {
                continue;
            }
            let (pa, pb) = (position(&self.slot_of, a), position(&self.slot_of, b));
            let cost = |column: u32, from: (f32, f32), to: (f32, f32)| -> f32 {
                words_of
                    .get(&column)
                    .into_iter()
                    .flatten()
                    .map(|w| {
                        let c = centre(&self.slot_of, *w);
                        distance(to, c) - distance(from, c)
                    })
                    .sum()
            };
            if cost(a, pa, pb) + cost(b, pb, pa) < 0.0 {
                let (sa, sb) = (self.slot_of[a as usize], self.slot_of[b as usize]);
                self.slot_of[a as usize] = sb;
                self.slot_of[b as usize] = sa;
                column_at[sb as usize] = a;
                column_at[sa as usize] = b;
                made += 1;
            }
        }
        self.swaps += made as u64;
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::words::START_COLUMNS;

    #[test]
    fn every_column_gets_its_own_slot_inside_the_outline() {
        for count in [1, 100, START_COLUMNS, 5_000] {
            let points = slots(count);
            assert_eq!(points.len(), count);
            assert!(points.iter().all(|(x, y)| inside(*x, *y)));
        }
    }

    #[test]
    fn columns_of_one_word_move_closer_together() {
        let mut words = Words::default();
        for word in ["cat", "dog", "tree", "sun"] {
            words.columns(word, START_COLUMNS);
        }
        let mut map = Map::default();
        map.fit(START_COLUMNS);
        let points = slots(START_COLUMNS);
        let spread = |map: &Map| {
            let columns = &words.words["cat"].columns;
            let at: Vec<(f32, f32)> = columns
                .iter()
                .map(|c| points[map.slot_of[*c as usize] as usize])
                .collect();
            let cx = at.iter().map(|p| p.0).sum::<f32>() / at.len() as f32;
            let cy = at.iter().map(|p| p.1).sum::<f32>() / at.len() as f32;
            at.iter()
                .map(|p| ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt())
                .sum::<f32>()
        };
        let before = spread(&map);
        let mut random = 7;
        let made = map.organise(&words, &points, 4_000, &mut random);
        assert!(made > 0);
        assert!(
            spread(&map) < before * 0.6,
            "{} -> {}",
            before,
            spread(&map)
        );
        let mut seen = map.slot_of.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), START_COLUMNS);
    }
}
