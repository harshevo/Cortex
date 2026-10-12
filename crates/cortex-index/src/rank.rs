//! Ranking the nodes of a graph by how close they are to a few of them.
//!
//! Personalized PageRank: a walker starts on one of the seed nodes, and at
//! every step either follows a connection or jumps back to a seed. The
//! share of time it spends on a node is that node's score. A node scores
//! well when it is a seed, or is connected to seeds by many short paths.
//!
//! For code this turns "these files mention the task's words" into "these
//! files, and the ones they import and are imported by, are where the task
//! probably is", which a search for the words alone cannot say.

/// A graph of `size` nodes with weighted connections in both directions.
pub struct Graph {
    neighbours: Vec<Vec<(usize, f64)>>,
}

impl Graph {
    pub fn new(size: usize) -> Self {
        Self {
            neighbours: vec![Vec::new(); size],
        }
    }

    pub fn len(&self) -> usize {
        self.neighbours.len()
    }

    pub fn is_empty(&self) -> bool {
        self.neighbours.is_empty()
    }

    /// Connects two nodes. Connecting them again adds to the weight.
    pub fn connect(&mut self, left: usize, right: usize, weight: f64) {
        if left == right || left >= self.len() || right >= self.len() {
            return;
        }

        for (from, to) in [(left, right), (right, left)] {
            match self.neighbours[from]
                .iter_mut()
                .find(|(node, _)| *node == to)
            {
                Some((_, held)) => *held += weight,
                None => self.neighbours[from].push((to, weight)),
            }
        }
    }

    pub fn neighbours(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.neighbours[node]
            .iter()
            .map(|(neighbour, _)| *neighbour)
    }

    /// The score of every node for a walk that restarts on `seeds`.
    ///
    /// `seeds` holds a weight per node; nodes that are not seeds have 0.
    /// `restart` is the chance of jumping back at each step: higher keeps
    /// the scores closer to the seeds. Scores sum to 1, and are all 0 when
    /// there are no seeds.
    pub fn personalized_pagerank(&self, seeds: &[f64], restart: f64, rounds: usize) -> Vec<f64> {
        let total: f64 = seeds.iter().sum();

        if total <= 0.0 || seeds.len() != self.len() {
            return vec![0.0; self.len()];
        }

        let start: Vec<f64> = seeds.iter().map(|weight| weight / total).collect();

        let strength: Vec<f64> = self
            .neighbours
            .iter()
            .map(|edges| edges.iter().map(|(_, weight)| weight).sum())
            .collect();

        let mut scores = start.clone();

        for _ in 0..rounds {
            let mut next: Vec<f64> = start.iter().map(|weight| weight * restart).collect();

            for (node, score) in scores.iter().enumerate() {
                let moving = score * (1.0 - restart);

                if strength[node] <= 0.0 {
                    // Nowhere to go from here: the walker jumps back.
                    for (seed, weight) in start.iter().enumerate() {
                        next[seed] += moving * weight;
                    }

                    continue;
                }

                for (neighbour, weight) in &self.neighbours[node] {
                    next[*neighbour] += moving * weight / strength[node];
                }
            }

            scores = next;
        }

        scores
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 - 1 - 2 - 3, and 4 on its own.
    fn chain() -> Graph {
        let mut graph = Graph::new(5);

        graph.connect(0, 1, 1.0);
        graph.connect(1, 2, 1.0);
        graph.connect(2, 3, 1.0);

        graph
    }

    #[test]
    fn scores_fall_with_distance_from_the_seed() {
        let scores = chain().personalized_pagerank(&[1.0, 0.0, 0.0, 0.0, 0.0], 0.5, 30);

        assert!(scores[0] > scores[1]);
        assert!(scores[1] > scores[2]);
        assert!(scores[2] > scores[3]);
        assert_eq!(scores[4], 0.0);

        let sum: f64 = scores.iter().sum();

        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_node_between_two_seeds_outranks_one_beside_a_single_seed() {
        // 0 and 2 are seeds; 1 lies between them, 3 hangs off 2.
        let scores = chain().personalized_pagerank(&[1.0, 0.0, 1.0, 0.0, 0.0], 0.5, 30);

        assert!(scores[1] > scores[3]);
    }

    #[test]
    fn no_seeds_means_no_scores() {
        let scores = chain().personalized_pagerank(&[0.0; 5], 0.5, 30);

        assert_eq!(scores, vec![0.0; 5]);
    }
}
