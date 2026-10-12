//! Placing memories in space and finding the ones that belong together.
//!
//! Positions come from principal component analysis: the three directions
//! along which the embeddings differ most become the three axes, so
//! memories of similar tasks land near each other.

use std::collections::HashSet;

use glam::Vec3;
use rayon::prelude::*;

/// Rounds of power iteration per axis. The embeddings are few and the
/// leading directions well separated, so this converges long before.
const POWER_ITERATIONS: usize = 60;

/// A link between two memories, by position in the episode list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub similarity: f32,
}

/// Lanes summed side by side in [`dot`]. Eight independent sums is the
/// shape the compiler turns into vector instructions (NEON on Apple
/// silicon), which a single running sum does not allow.
const LANES: usize = 8;

/// The dot product of two vectors of the same length.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (chunks_a, tail_a) = a.as_chunks::<LANES>();
    let (chunks_b, tail_b) = b.as_chunks::<LANES>();

    let mut sums = [0.0_f32; LANES];

    for (chunk_a, chunk_b) in chunks_a.iter().zip(chunks_b) {
        for lane in 0..LANES {
            sums[lane] += chunk_a[lane] * chunk_b[lane];
        }
    }

    let tail: f32 = tail_a.iter().zip(tail_b).map(|(x, y)| x * y).sum();

    sums.iter().sum::<f32>() + tail
}

/// Each memory's strongest ties: its `per_memory` most similar memories,
/// kept only when the similarity reaches `min_similarity`. A pair linked
/// from both sides appears once.
///
/// Every memory is compared with every other, so the work grows with the
/// square of their number. The comparisons for different memories do not
/// depend on each other and run on all cores.
pub fn links(embeddings: &[&[f32]], per_memory: usize, min_similarity: f32) -> Vec<Link> {
    // Lengths are needed for every pair; work them out once.
    let lengths: Vec<f32> = embeddings
        .par_iter()
        .map(|embedding| dot(embedding, embedding).sqrt())
        .collect();

    let similarity = |a: usize, b: usize| {
        let scale = lengths[a] * lengths[b];

        if scale == 0.0 || embeddings[a].len() != embeddings[b].len() {
            return 0.0;
        }

        dot(embeddings[a], embeddings[b]) / scale
    };

    let nearest: Vec<Vec<(usize, f32)>> = (0..embeddings.len())
        .into_par_iter()
        .map(|a| {
            let mut candidates: Vec<(usize, f32)> = (0..embeddings.len())
                .filter(|b| *b != a)
                .map(|b| (b, similarity(a, b)))
                .filter(|(_, similarity)| *similarity >= min_similarity)
                .collect();

            candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
            candidates.truncate(per_memory);

            candidates
        })
        .collect();

    let mut seen = HashSet::new();
    let mut found = Vec::new();

    for (a, candidates) in nearest.into_iter().enumerate() {
        for (b, similarity) in candidates {
            let (low, high) = (a.min(b), a.max(b));

            if seen.insert((low, high)) {
                found.push(Link {
                    a: low,
                    b: high,
                    similarity,
                });
            }
        }
    }

    found
}

/// One point per embedding. The cloud is centred on the origin and scaled
/// so that its typical distance from the centre is one.
pub fn project(embeddings: &[&[f32]]) -> Vec<Vec3> {
    let count = embeddings.len();

    let Some(dimensions) = embeddings.first().map(|embedding| embedding.len()) else {
        return Vec::new();
    };

    let centred = centre(embeddings, dimensions);

    let mut axes: Vec<Vec<f32>> = Vec::with_capacity(3);

    for axis_index in 0..3 {
        axes.push(principal_axis(&centred, dimensions, &axes, axis_index));
    }

    let mut points: Vec<Vec3> = centred
        .iter()
        .map(|row| Vec3::new(dot(row, &axes[0]), dot(row, &axes[1]), dot(row, &axes[2])))
        .collect();

    orient(&mut points);

    let mean_square = points
        .iter()
        .map(|point| point.length_squared())
        .sum::<f32>()
        / count as f32;

    let scale = if mean_square > 1e-12 {
        1.0 / mean_square.sqrt()
    } else {
        0.0
    };

    for point in &mut points {
        *point *= scale;
    }

    points
}

fn centre(embeddings: &[&[f32]], dimensions: usize) -> Vec<Vec<f32>> {
    let mut mean = vec![0.0; dimensions];

    for embedding in embeddings {
        for (total, value) in mean.iter_mut().zip(embedding.iter()) {
            *total += value;
        }
    }

    for total in &mut mean {
        *total /= embeddings.len() as f32;
    }

    embeddings
        .iter()
        .map(|embedding| {
            let mut row = vec![0.0; dimensions];

            for ((cell, value), mean) in row.iter_mut().zip(embedding.iter()).zip(&mean) {
                *cell = value - mean;
            }

            row
        })
        .collect()
}

/// The direction of greatest spread that is at right angles to `earlier`.
fn principal_axis(
    rows: &[Vec<f32>],
    dimensions: usize,
    earlier: &[Vec<f32>],
    axis_index: usize,
) -> Vec<f32> {
    // A fixed start keeps the layout the same from one launch to the next.
    let mut axis: Vec<f32> = (0..dimensions)
        .map(|index| ((index * (axis_index + 2) + 1) as f32 * 0.618).sin())
        .collect();

    for _ in 0..POWER_ITERATIONS {
        remove_components(&mut axis, earlier);

        // How far along the axis each memory lies. One dot product per
        // memory, spread over all cores.
        let weights: Vec<f32> = rows.par_iter().map(|row| dot(row, &axis)).collect();

        let mut next = vec![0.0; dimensions];

        for (row, weight) in rows.iter().zip(weights) {
            for (cell, value) in next.iter_mut().zip(row) {
                *cell += weight * value;
            }
        }

        remove_components(&mut next, earlier);

        let length = dot(&next, &next).sqrt();

        if length < 1e-9 {
            return vec![0.0; dimensions];
        }

        for value in &mut next {
            *value /= length;
        }

        axis = next;
    }

    axis
}

fn remove_components(vector: &mut [f32], axes: &[Vec<f32>]) {
    for axis in axes {
        let amount = dot(vector, axis);

        for (value, component) in vector.iter_mut().zip(axis) {
            *value -= amount * component;
        }
    }
}

/// An axis and its mirror image are equally valid. Pick the one that puts
/// the first memory on the positive side, so that adding a memory does not
/// flip the whole picture.
fn orient(points: &mut [Vec3]) {
    let Some(first) = points.first().copied() else {
        return;
    };

    let flip = Vec3::new(sign(first.x), sign(first.y), sign(first.z));

    for point in points {
        *point *= flip;
    }
}

fn sign(value: f32) -> f32 {
    if value < 0.0 { -1.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(rows: &[Vec<f32>]) -> Vec<&[f32]> {
        rows.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn two_groups_stay_apart() {
        let rows = vec![
            vec![1.0, 0.0, 0.0, 0.1],
            vec![0.9, 0.1, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.1],
            vec![0.1, 0.0, 0.9, 0.0],
        ];

        let points = project(&refs(&rows));

        let within = points[0].distance(points[1]) + points[2].distance(points[3]);
        let between = points[0].distance(points[2]) + points[1].distance(points[3]);

        assert!(between > within * 3.0);
    }

    #[test]
    fn the_cloud_is_centred() {
        let rows = vec![vec![1.0, 2.0], vec![3.0, -1.0], vec![-2.0, 0.5]];

        let centre: Vec3 = project(&refs(&rows)).iter().sum();

        assert!(centre.length() < 1e-4);
    }

    #[test]
    fn one_memory_sits_at_the_origin() {
        let rows = vec![vec![0.3, 0.4]];

        assert_eq!(project(&refs(&rows)), [Vec3::ZERO]);
        assert!(project(&[]).is_empty());
    }

    #[test]
    fn the_dot_product_handles_any_length() {
        let a: Vec<f32> = (0..19).map(|index| index as f32).collect();
        let b = vec![2.0; 19];

        assert_eq!(dot(&a, &b), 2.0 * (0..19).sum::<i32>() as f32);
        assert_eq!(dot(&[], &[]), 0.0);
    }

    #[test]
    fn links_join_the_most_similar_and_are_not_doubled() {
        let rows = vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.0, 1.0]];

        let found = links(&refs(&rows), 1, 0.5);

        assert_eq!(found.len(), 1);
        assert_eq!((found[0].a, found[0].b), (0, 1));
    }
}
