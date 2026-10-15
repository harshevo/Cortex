//! The files a task is probably about, found without a model.
//!
//! Searching for the task's words finds the files that mention them. It
//! misses the file that holds the cause when that file uses other words,
//! which is common: a report says "losses are displayed wrongly" and the
//! bug is in a formatting helper that says nothing about losses. That
//! helper is imported by the files that do.
//!
//! So the search is only the start. The files that match the task's words
//! become the seeds of a walk over the import graph (personalized
//! PageRank, see [`cortex_index::rank`]), and the files ranked highest by
//! the walk are shown: the matches themselves and what is closely wired
//! to several of them. This follows HippoRAG (Gutiérrez et al., 2024) and
//! LARGER (2026), which found the same step to help on code.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cortex_brain::memory::similarity;
use cortex_core::{Briefing, ContextSource, Workspace};
use cortex_index::rank::Graph;
use cortex_index::{CodeGraph, Language, walk};
use serde_json::json;

/// Files shown at most.
const MAX_FILES: usize = 5;

/// Files whose words start the walk, at most.
const MAX_SEEDS: usize = 8;

/// A file is shown only if it scores at least this share of the best.
const RELATIVE_FLOOR: f64 = 0.35;

/// The chance of jumping back to a seed at each step. High, so the walk
/// stays within a step or two of the matches.
const RESTART: f64 = 0.5;

const ROUNDS: usize = 24;

/// Larger files are generated or data.
const MAX_FILE_BYTES: u64 = 200_000;

/// A task word found in a file's path says more than one in its text.
const PATH_WEIGHT: f64 = 3.0;
const SYMBOL_WEIGHT: f64 = 2.0;
const TEXT_WEIGHT: f64 = 1.0;

/// A word in more than this share of the files tells them apart too
/// little to count.
const MAX_SHARE_OF_FILES: f64 = 0.34;

/// Words a bug report is written with whatever it is about. They match
/// code by accident ("file", "set", "add") and say nothing of the task.
const REPORT_WORDS: [&str; 44] = [
    "actually",
    "add",
    "app",
    "appear",
    "cause",
    "code",
    "correct",
    "each",
    "expect",
    "fail",
    "file",
    "find",
    "fine",
    "following",
    "get",
    "happen",
    "instead",
    "issue",
    "make",
    "more",
    "need",
    "problem",
    "report",
    "result",
    "return",
    "run",
    "see",
    "set",
    "several",
    "show",
    "single",
    "some",
    "test",
    "than",
    "throughout",
    "use",
    "user",
    "whole",
    "work",
    "wrong",
    "wrongly",
    "implement",
    "call",
    "value",
];

pub struct LikelyFiles {
    graph: Arc<CodeGraph>,
}

/// One file's place in the ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// Relative to the project.
    pub path: String,
    pub score: f64,

    /// The task's words found in the file, most telling first. Empty for
    /// a file reached only through the graph.
    pub matched: Vec<String>,

    /// The seed files it imports or is imported by.
    pub connected_to: Vec<String>,
}

impl LikelyFiles {
    pub fn new(graph: Arc<CodeGraph>) -> Self {
        Self { graph }
    }

    /// The source files most likely to be what `instruction` is about,
    /// best first. Test files are left out: a task is rarely fixed there.
    pub fn rank(&self, instruction: &str, root: &Path) -> Vec<Ranked> {
        let files: Vec<PathBuf> = walk::files_under(root)
            .into_iter()
            .filter(|path| is_source(root, path))
            .collect();

        if files.len() < 2 {
            return Vec::new();
        }

        let report_words: HashSet<String> = REPORT_WORDS.iter().map(|word| stem(word)).collect();

        let task: HashSet<String> = similarity::words(instruction)
            .iter()
            .map(|word| stem(word))
            .filter(|word| !report_words.contains(word))
            .collect();

        let found: Vec<FileWords> = files.iter().map(|path| self.words_of(root, path)).collect();

        let matches = lexical_matches(&task, &found);

        let mut seeded: Vec<usize> = (0..files.len())
            .filter(|&file| matches[file].score > 0.0)
            .collect();

        seeded.sort_by(|&left, &right| matches[right].score.total_cmp(&matches[left].score));
        seeded.truncate(MAX_SEEDS);

        if seeded.is_empty() {
            return Vec::new();
        }

        let position: HashMap<&Path, usize> = files
            .iter()
            .enumerate()
            .map(|(index, path)| (path.as_path(), index))
            .collect();

        let mut graph = Graph::new(files.len());

        for (importer, target) in self.graph.import_edges(root) {
            if let (Some(&from), Some(&to)) = (
                position.get(importer.as_path()),
                position.get(target.as_path()),
            ) {
                graph.connect(from, to, 1.0);
            }
        }

        let mut seeds = vec![0.0; files.len()];

        for &file in &seeded {
            seeds[file] = matches[file].score;
        }

        let scores = graph.personalized_pagerank(&seeds, RESTART, ROUNDS);

        let mut order: Vec<usize> = (0..files.len()).collect();

        order.sort_by(|&left, &right| scores[right].total_cmp(&scores[left]));

        let best = scores[order[0]];

        let relative = |file: usize| {
            files[file]
                .strip_prefix(root)
                .unwrap_or(&files[file])
                .display()
                .to_string()
        };

        order
            .into_iter()
            .take(MAX_FILES)
            .filter(|&file| scores[file] > 0.0 && scores[file] >= best * RELATIVE_FLOOR)
            .map(|file| Ranked {
                path: relative(file),
                score: scores[file],
                matched: matches[file].words.clone(),
                connected_to: graph
                    .neighbours(file)
                    .filter(|neighbour| seeded.contains(neighbour))
                    .map(relative)
                    .collect(),
            })
            .collect()
    }

    /// The words in a file's path, in the names it defines, and in its
    /// text, each reduced to a stem.
    fn words_of(&self, root: &Path, path: &Path) -> FileWords {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();

        let symbols: Vec<String> = self
            .graph
            .index()
            .file_symbols(path)
            .into_iter()
            .map(|symbol| symbol.name)
            .collect();

        let text = fs::read_to_string(path).unwrap_or_default();

        let stems = |text: &str| -> HashSet<String> {
            similarity::words(text)
                .iter()
                .map(|word| stem(word))
                .collect()
        };

        FileWords {
            path: stems(&relative),
            symbols: stems(&symbols.join(" ")),
            text: stems(&text),
        }
    }
}

struct FileWords {
    path: HashSet<String>,
    symbols: HashSet<String>,
    text: HashSet<String>,
}

impl FileWords {
    /// How strongly the file carries `word`: where it appears decides.
    fn weight(&self, word: &str) -> f64 {
        if self.path.contains(word) {
            PATH_WEIGHT
        } else if self.symbols.contains(word) {
            SYMBOL_WEIGHT
        } else if self.text.contains(word) {
            TEXT_WEIGHT
        } else {
            0.0
        }
    }
}

#[derive(Default, Clone)]
struct Match {
    score: f64,
    words: Vec<String>,
}

/// For each file, how well it matches the task's words. A word found in
/// few files counts for more than one found in many.
fn lexical_matches(task: &HashSet<String>, files: &[FileWords]) -> Vec<Match> {
    let mut matches = vec![Match::default(); files.len()];

    let mut words: Vec<&String> = task.iter().collect();

    words.sort();

    for word in words {
        let holders = files.iter().filter(|file| file.weight(word) > 0.0).count();

        if holders == 0 || holders as f64 > files.len() as f64 * MAX_SHARE_OF_FILES {
            continue;
        }

        // Squared, so that one word found in two files outweighs several
        // found in a third of them.
        let rarity = (files.len() as f64 / holders as f64).ln().powi(2);

        for (file, found) in files.iter().zip(&mut matches) {
            let weight = file.weight(word);

            if weight > 0.0 {
                found.score += weight * rarity;
                found.words.push(word.clone());
            }
        }
    }

    matches
}

/// A crude stem, so that "trades", "trading" and "trade" count as one
/// word. Wrong for some words; right often enough for matching.
fn stem(word: &str) -> String {
    let word = word.to_lowercase();

    for ending in ["ing", "ed", "es", "s"] {
        if let Some(stem) = word.strip_suffix(ending)
            && stem.chars().count() >= 4
        {
            return stem.trim_end_matches('e').to_string();
        }
    }

    if word.chars().count() > 4 {
        return word.trim_end_matches('e').to_string();
    }

    word
}

fn is_source(root: &Path, path: &Path) -> bool {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();

    Language::for_path(path).is_some()
        && !walk::is_test_file(&relative)
        && fs::metadata(path).is_ok_and(|metadata| metadata.len() <= MAX_FILE_BYTES)
}

#[async_trait]
impl ContextSource for LikelyFiles {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let ranked = self.rank(instruction, workspace.root());

        if ranked.is_empty() {
            return None;
        }

        let mut text = String::from(
            "## Files this task is probably about\n\n\
             Ranked without reading the task's meaning: files that use the task's words, and \
             files closely tied to them by imports. A starting point, not a diagnosis. The \
             cause is often in a file listed for its connections, not its words.\n\n",
        );

        for file in &ranked {
            text.push_str(&format!("- {}", file.path));

            if !file.matched.is_empty() {
                let words: Vec<&str> = file.matched.iter().take(4).map(String::as_str).collect();

                text.push_str(&format!(" (mentions: {})", words.join(", ")));
            }

            // Only said of a file that is listed for nothing else.
            if file.matched.is_empty() && !file.connected_to.is_empty() {
                text.push_str(" (imports, or is imported by, several of the others)");
            }

            text.push('\n');
        }

        let detail: Vec<_> = ranked
            .iter()
            .map(|file| json!({ "path": file.path, "score": file.score, "matched": file.matched }))
            .collect();

        Some(Briefing {
            source: "likely_files".to_string(),
            text,
            detail: json!({ "files": detail }),
        })
    }
}

#[cfg(test)]
mod tests {
    use cortex_index::CodeIndex;

    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();

        for (path, text) in files {
            let path = dir.path().join(path);

            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        dir
    }

    fn likely() -> LikelyFiles {
        LikelyFiles::new(Arc::new(CodeGraph::new(Arc::new(CodeIndex::new()))))
    }

    #[test]
    fn endings_are_stripped_so_forms_of_a_word_match() {
        assert_eq!(stem("trades"), stem("trade"));
        assert_eq!(stem("trading"), stem("trade"));
        assert_eq!(stem("Losses"), stem("loss"));
        assert_eq!(stem("fee"), "fee");
    }

    #[test]
    fn a_helper_the_matching_files_share_is_found_through_their_imports() {
        // The task speaks of losses on the journal and calendar pages.
        // The cause is in `format.ts`, which mentions neither, but both
        // pages import it. `settings.ts` is unrelated.
        let dir = project(&[
            (
                "src/journal.ts",
                "import { show } from \"./format\";\n\nexport function journalLosses(loss: number) {\n  return show(loss);\n}\n",
            ),
            (
                "src/calendar.ts",
                "import { show } from \"./format\";\n\nexport function calendarLosses(loss: number) {\n  return show(loss);\n}\n",
            ),
            (
                "src/format.ts",
                "export function show(amount: number) {\n  return amount.toFixed(2);\n}\n",
            ),
            (
                "src/settings.ts",
                "export function theme() {\n  return \"dark\";\n}\n",
            ),
            (
                "src/profile.ts",
                "export function avatar() {\n  return \"me.png\";\n}\n",
            ),
        ]);

        let ranked = likely().rank(
            "Losses are displayed wrongly on the journal and the calendar.",
            dir.path(),
        );

        let paths: Vec<&str> = ranked.iter().map(|file| file.path.as_str()).collect();

        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&"src/format.ts"));
        assert!(!paths.contains(&"src/settings.ts"));

        let format = ranked
            .iter()
            .find(|file| file.path == "src/format.ts")
            .unwrap();

        assert!(format.matched.is_empty());
        assert_eq!(format.connected_to.len(), 2);
    }

    #[tokio::test]
    async fn a_task_whose_words_are_nowhere_gets_no_briefing() {
        let dir = project(&[
            ("a.ts", "export function alpha() {\n  return 1;\n}\n"),
            ("b.ts", "export function beta() {\n  return 2;\n}\n"),
        ]);

        let workspace = Workspace::new(dir.path()).unwrap();

        assert!(
            likely()
                .brief("Evening trades are misfiled.", &workspace)
                .await
                .is_none()
        );
    }
}
