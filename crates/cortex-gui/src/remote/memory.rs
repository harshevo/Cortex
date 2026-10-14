//! The engine's memory as a phone draws it: dots and the lines between
//! them.

use cortex_core::Outcome;
use cortex_remote::wire::{MemoryGraph, MemoryKind, MemoryNode};

use crate::memory::Remembered;

/// The newest episodes sent. A phone draws them all at once.
const EPISODES_SENT: usize = 120;

/// Each episode is joined to this many of the ones most like it.
const NEIGHBOURS: usize = 2;

/// Episodes less alike than this are not joined.
const LIKENESS: f32 = 0.35;

const TITLE_LENGTH: usize = 90;

pub fn graph(remembered: &Remembered) -> MemoryGraph {
    let mut graph = MemoryGraph::default();

    let skipped = remembered.episodes.len().saturating_sub(EPISODES_SENT);
    let episodes = &remembered.episodes[skipped..];

    for episode in episodes {
        let outcome = match (episode.verified, episode.outcome) {
            (Some(true), _) => "Checks passed",
            (Some(false), _) => "Checks failed",
            (None, Some(Outcome::Completed)) => "Finished, not checked",
            (None, _) => "Did not finish",
        };

        let mut text = format!("{outcome} · {} steps", episode.steps);

        if !episode.files_changed.is_empty() {
            text.push_str(&format!("\nChanged: {}", episode.files_changed.join(", ")));
        }

        if !episode.report.is_empty() {
            text.push_str(&format!("\n\n{}", episode.report));
        }

        graph.nodes.push(MemoryNode {
            kind: MemoryKind::Episode,
            title: shortened(&episode.instruction),
            text,
        });
    }

    // Episodes about the same thing sit together.
    let vectors = remembered.vectors.get(skipped..).unwrap_or_default();

    for (index, vector) in vectors.iter().enumerate().take(episodes.len()) {
        let mut alike: Vec<(f32, usize)> = vectors
            .iter()
            .enumerate()
            .take(episodes.len())
            .filter(|(other, _)| *other != index)
            .map(|(other, theirs)| (cosine(vector, theirs), other))
            .filter(|(likeness, _)| *likeness >= LIKENESS)
            .collect();

        alike.sort_by(|a, b| b.0.total_cmp(&a.0));

        for (_, other) in alike.into_iter().take(NEIGHBOURS) {
            let link = (index.min(other) as u32, index.max(other) as u32);

            if !graph.links.contains(&link) {
                graph.links.push(link);
            }
        }
    }

    for fact in &remembered.facts {
        let at = graph.nodes.len() as u32;

        graph.nodes.push(MemoryNode {
            kind: MemoryKind::Fact,
            title: fact.kind.label().replace('_', " "),
            text: fact.text.clone(),
        });

        // A fact belongs with the tasks that touched its files.
        for (index, episode) in episodes.iter().enumerate() {
            let shared = fact
                .paths
                .iter()
                .any(|path| episode.files_changed.contains(path));

            if shared {
                graph.links.push((index as u32, at));
            }
        }
    }

    for lesson in &remembered.lessons {
        graph.nodes.push(MemoryNode {
            kind: MemoryKind::Fact,
            title: shortened(&lesson.lesson.title),
            text: format!("{}\n\n{}", lesson.lesson.when, lesson.lesson.advice),
        });
    }

    graph
}

fn shortened(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();

    if line.chars().count() <= TITLE_LENGTH {
        return line.to_string();
    }

    let cut: String = line.chars().take(TITLE_LENGTH).collect();

    format!("{cut}…")
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let length = |vector: &[f32]| vector.iter().map(|x| x * x).sum::<f32>().sqrt();

    let lengths = length(a) * length(b);

    if lengths == 0.0 { 0.0 } else { dot / lengths }
}
