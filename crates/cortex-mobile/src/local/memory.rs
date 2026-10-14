//! The memory of the agent on this phone as the screens draw it: dots and
//! the lines between them.

use std::path::Path;

use anyhow::Result;
use cortex_core::Outcome;
use cortex_memory::Store;
use cortex_remote::wire::{MemoryGraph, MemoryKind, MemoryNode};

/// The newest episodes drawn. A phone draws them all at once.
const EPISODES_SHOWN: usize = 120;

const TITLE_LENGTH: usize = 90;

/// Reads the memory file the agent writes to. One that does not exist
/// yet is an empty memory.
pub fn graph(path: &Path, scope: &str) -> Result<MemoryGraph> {
    let mut graph = MemoryGraph::default();

    if !path.is_file() {
        return Ok(graph);
    }

    let store = Store::open(path)?;

    let mut episodes: Vec<_> = store
        .episodes(scope)?
        .into_iter()
        .map(|stored| stored.episode)
        .collect();

    episodes.sort_by_key(|episode| episode.created_ms);

    let skipped = episodes.len().saturating_sub(EPISODES_SHOWN);
    let episodes = &episodes[skipped..];

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

    // Tasks that changed the same file sit together: each is joined to
    // the one before it that did.
    for (index, episode) in episodes.iter().enumerate() {
        let earlier = episodes[..index].iter().rposition(|other| {
            other
                .files_changed
                .iter()
                .any(|file| episode.files_changed.contains(file))
        });

        if let Some(earlier) = earlier {
            graph.links.push((earlier as u32, index as u32));
        }
    }

    for fact in store.facts(scope)? {
        let at = graph.nodes.len() as u32;

        graph.nodes.push(MemoryNode {
            kind: MemoryKind::Fact,
            title: fact.kind.label().replace('_', " "),
            text: fact.text.clone(),
        });

        // A fact belongs with the tasks that touched its files.
        for (index, episode) in episodes.iter().enumerate() {
            if fact
                .paths
                .iter()
                .any(|path| episode.files_changed.contains(path))
            {
                graph.links.push((index as u32, at));
            }
        }
    }

    for row in store.lessons(scope)? {
        let lesson = row.lesson.lesson;

        graph.nodes.push(MemoryNode {
            kind: MemoryKind::Fact,
            title: shortened(&lesson.title),
            text: format!("{}\n\n{}", lesson.when, lesson.advice),
        });
    }

    Ok(graph)
}

fn shortened(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();

    if line.chars().count() <= TITLE_LENGTH {
        return line.to_string();
    }

    let cut: String = line.chars().take(TITLE_LENGTH).collect();

    format!("{cut}…")
}
