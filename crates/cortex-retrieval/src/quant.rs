//! Keeping vectors small.
//!
//! The embedding service returns 32-bit floats: 3 KB for one 768-number
//! vector, so a project of 100,000 chunks would hold 300 MB of them. Two
//! smaller forms are kept instead:
//!
//! - one bit per number (its sign), 96 bytes, compared by counting the
//!   bits that differ. Coarse, and fast enough to scan everything;
//! - one byte per number, 768 bytes, which ranks almost exactly as the
//!   floats do when compared with a full-precision query.
//!
//! A search scans the bits for a shortlist and orders the shortlist by
//! the bytes. The bits can drop a weak match from the shortlist, so a
//! project small enough to compare every chunk's bytes skips them.
//! Together the two forms take under a third of the space of the floats.

use serde::{Deserialize, Serialize};

/// A vector in both small forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantized {
    /// The sign of each number, 64 to a word.
    bits: Vec<u64>,

    /// Each number as a share of the largest, in steps of 1/127.
    bytes: Vec<i8>,

    /// What 127 stands for.
    scale: f32,
}

impl Quantized {
    pub fn from_vector(vector: &[f32]) -> Self {
        let largest = vector
            .iter()
            .fold(0f32, |largest, value| largest.max(value.abs()));

        let scale = if largest > 0.0 { largest } else { 1.0 };

        Self {
            bits: sign_bits(vector),
            bytes: vector
                .iter()
                .map(|value| (value / scale * 127.0).round() as i8)
                .collect(),
            scale,
        }
    }

    /// How many signs differ from those of `query_bits`. Lower is closer.
    pub fn distance(&self, query_bits: &[u64]) -> u32 {
        self.bits
            .iter()
            .zip(query_bits)
            .map(|(mine, theirs)| (mine ^ theirs).count_ones())
            .sum()
    }

    /// The dot product with a full-precision query, close to what the
    /// original vector would give.
    pub fn dot(&self, query: &[f32]) -> f32 {
        let sum: f32 = self
            .bytes
            .iter()
            .zip(query)
            .map(|(byte, value)| f32::from(*byte) * value)
            .sum();

        sum * self.scale / 127.0
    }

    /// Bytes held for this vector.
    pub fn size(&self) -> usize {
        self.bits.len() * 8 + self.bytes.len() + 4
    }
}

/// One bit per number: set when the number is positive.
pub fn sign_bits(vector: &[f32]) -> Vec<u64> {
    vector
        .chunks(64)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .filter(|(_, value)| **value > 0.0)
                .fold(0u64, |word, (position, _)| word | (1 << position))
        })
        .collect()
}

/// The positions of the `limit` vectors closest to `query`, best first,
/// with their scores.
///
/// When there are more than `scan_all_below` vectors, only the
/// `shortlist` closest by sign bits are compared in full.
pub fn nearest(
    vectors: &[&Quantized],
    query: &[f32],
    limit: usize,
    shortlist: usize,
    scan_all_below: usize,
) -> Vec<(usize, f32)> {
    let mut candidates: Vec<usize> = (0..vectors.len()).collect();

    if vectors.len() > scan_all_below {
        let query_bits = sign_bits(query);

        let keep = shortlist.max(limit).min(candidates.len());

        candidates.select_nth_unstable_by_key(keep - 1, |&position| {
            vectors[position].distance(&query_bits)
        });

        candidates.truncate(keep);
    }

    let mut scored: Vec<(usize, f32)> = candidates
        .into_iter()
        .map(|position| (position, vectors[position].dot(query)))
        .collect();

    scored.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    scored.truncate(limit);

    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit vectors from a fixed sequence, so the test always sees the
    /// same ones.
    fn vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        let mut state = 0x2545_f491_4f6c_dd1du64;

        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;

            (state >> 11) as f32 / (1u64 << 53) as f32 - 0.5
        };

        (0..count)
            .map(|_| {
                let vector: Vec<f32> = (0..dimension).map(|_| next()).collect();

                let length = vector.iter().map(|value| value * value).sum::<f32>().sqrt();

                vector.iter().map(|value| value / length).collect()
            })
            .collect()
    }

    fn exact_order(all: &[Vec<f32>], query: &[f32], limit: usize) -> Vec<usize> {
        let dot = |vector: &Vec<f32>| -> f32 { vector.iter().zip(query).map(|(a, b)| a * b).sum() };

        let mut order: Vec<usize> = (0..all.len()).collect();

        order.sort_by(|&left, &right| dot(&all[right]).total_cmp(&dot(&all[left])));
        order.truncate(limit);

        order
    }

    #[test]
    fn bytes_rank_as_the_floats_do() {
        let all = vectors(400, 256);
        let quantized: Vec<Quantized> = all
            .iter()
            .map(|vector| Quantized::from_vector(vector))
            .collect();
        let references: Vec<&Quantized> = quantized.iter().collect();

        // A query near a stored vector, as a real search has.
        let query: Vec<f32> = all[7]
            .iter()
            .zip(&all[90])
            .map(|(a, b)| 0.8 * a + 0.2 * b)
            .collect();

        let found: Vec<usize> = nearest(&references, &query, 10, 0, usize::MAX)
            .into_iter()
            .map(|(position, _)| position)
            .collect();

        let exact = exact_order(&all, &query, 10);

        assert_eq!(found[0], 7);
        assert_eq!(found[..3], exact[..3]);

        let shared = found
            .iter()
            .filter(|position| exact.contains(position))
            .count();

        assert!(shared >= 9, "only {shared} of the exact top ten were found");

        let exact_score: f32 = all[7].iter().zip(&query).map(|(a, b)| a * b).sum();

        assert!((quantized[7].dot(&query) - exact_score).abs() < 0.01);
    }

    #[test]
    fn a_shortlist_by_sign_bits_keeps_strong_matches_and_most_weak_ones() {
        let all = vectors(2_000, 256);
        let quantized: Vec<Quantized> = all
            .iter()
            .map(|vector| Quantized::from_vector(vector))
            .collect();
        let references: Vec<&Quantized> = quantized.iter().collect();

        let query: Vec<f32> = all[1234]
            .iter()
            .zip(&all[5])
            .map(|(a, b)| 0.8 * a + 0.2 * b)
            .collect();

        let found: Vec<usize> = nearest(&references, &query, 10, 400, 500)
            .into_iter()
            .map(|(position, _)| position)
            .collect();

        assert_eq!(found[0], 1234);

        // Unrelated random vectors are the hardest case for sign bits:
        // every match but the first is weak. Most still get through.
        let exact = exact_order(&all, &query, 10);

        let shared = found
            .iter()
            .filter(|position| exact.contains(position))
            .count();

        assert!(shared >= 6, "only {shared} of the exact top ten were found");
    }

    #[test]
    fn the_small_forms_take_under_a_third_of_the_space() {
        let vector = &vectors(1, 768)[0];

        let quantized = Quantized::from_vector(vector);

        assert_eq!(quantized.size(), 96 + 768 + 4);
        assert!(quantized.size() * 3 < vector.len() * 4);
    }
}
