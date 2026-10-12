//! What a run changed, as text a reviewer can read.
//!
//! Two ways of finding the changes:
//!
//! - [`Originals`] keeps the content a file had before the run's first
//!   edit of it. It costs nothing until a file is edited, but it does not
//!   see files changed by a shell command.
//! - [`between`] compares two copies of a project. A parallel attempt
//!   works in its own copy, so everything it did shows up, however it was
//!   done.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::message::ToolCall;
use crate::workspace::Workspace;

/// Files larger than this are compared but not shown.
const MAX_SHOWN_BYTES: u64 = 400_000;

/// Unchanged lines shown around a change.
const CONTEXT_LINES: usize = 3;

/// Above this many line pairs the middle of a file is shown as replaced
/// whole instead of being compared line by line.
const MAX_COMPARED_CELLS: usize = 4_000_000;

/// Directories that hold installed or generated files, never the work.
const SKIPPED_DIRECTORIES: [&str; 8] = [
    ".git",
    ".cortex",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    ".next",
];

/// One file before and after. `None` means the file did not exist, or is
/// not text.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Relative to the project.
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,

    /// Too large or not text: copied when applied, never shown.
    pub opaque: bool,
}

/// The content files had before a run first changed them.
#[derive(Debug, Clone, Default)]
pub struct Originals {
    files: BTreeMap<String, Option<String>>,
}

impl Originals {
    /// Call before `call` runs. Keeps the file it names, the first time.
    pub fn keep(&mut self, workspace: &Workspace, call: &ToolCall) {
        let Some(path) = call.input.get("path").and_then(|path| path.as_str()) else {
            return;
        };

        let Ok(absolute) = workspace.resolve(path) else {
            return;
        };

        let relative = workspace.relative(&absolute);

        self.files
            .entry(relative)
            .or_insert_with(|| fs::read_to_string(&absolute).ok());
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The kept files whose content is different now.
    pub fn changes(&self, workspace: &Workspace) -> Vec<Change> {
        self.files
            .iter()
            .filter_map(|(path, before)| {
                let after = fs::read_to_string(workspace.root().join(path)).ok();

                (*before != after).then(|| Change {
                    path: path.clone(),
                    before: before.clone(),
                    after,
                    opaque: false,
                })
            })
            .collect()
    }
}

/// Every file that differs between two copies of a project.
pub fn between(original: &Path, changed: &Path) -> Vec<Change> {
    let before = files_under(original);
    let after = files_under(changed);

    let mut paths: Vec<&String> = before.keys().chain(after.keys()).collect();

    paths.sort();
    paths.dedup();

    let mut changes = Vec::new();

    for path in paths {
        let old = before
            .get(path)
            .map(|_| fs::read(original.join(path)).unwrap_or_default());
        let new = after
            .get(path)
            .map(|_| fs::read(changed.join(path)).unwrap_or_default());

        if old == new {
            continue;
        }

        let too_large = [before.get(path), after.get(path)]
            .into_iter()
            .flatten()
            .any(|size| *size > MAX_SHOWN_BYTES);

        let old_text = old.as_ref().map(|bytes| String::from_utf8(bytes.clone()));
        let new_text = new.as_ref().map(|bytes| String::from_utf8(bytes.clone()));

        let is_text = !matches!(old_text, Some(Err(_))) && !matches!(new_text, Some(Err(_)));

        if too_large || !is_text {
            changes.push(Change {
                path: path.clone(),
                before: None,
                after: None,
                opaque: true,
            });

            continue;
        }

        changes.push(Change {
            path: path.clone(),
            before: old_text.and_then(Result::ok),
            after: new_text.and_then(Result::ok),
            opaque: false,
        });
    }

    changes
}

/// Makes `to` match `from` for every changed path: copies what exists in
/// `from`, deletes what does not.
pub fn apply(changes: &[Change], from: &Path, to: &Path) -> io::Result<()> {
    for change in changes {
        let source = from.join(&change.path);
        let target = to.join(&change.path);

        if source.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }

            fs::copy(&source, &target)?;
        } else if target.is_file() {
            fs::remove_file(&target)?;
        }
    }

    Ok(())
}

/// Lines added and removed, for telling a small change from a large one.
pub fn size(changes: &[Change]) -> usize {
    changes
        .iter()
        .map(|change| {
            let before = change.before.as_deref().unwrap_or_default();
            let after = change.after.as_deref().unwrap_or_default();

            hunks(before, after)
                .iter()
                .map(|hunk| hunk.removed.len() + hunk.added.len())
                .sum::<usize>()
                .max(1)
        })
        .sum()
}

/// The changes as a diff, shortened to about `max_chars`.
pub fn render(changes: &[Change], max_chars: usize) -> String {
    let mut text = String::new();

    for change in changes {
        if change.opaque {
            text.push_str(&format!("--- {} (changed; not shown)\n\n", change.path));

            continue;
        }

        let label = match (&change.before, &change.after) {
            (None, Some(_)) => " (new file)",
            (Some(_), None) => " (deleted)",
            _ => "",
        };

        text.push_str(&format!("--- {}{label}\n", change.path));

        let before = change.before.as_deref().unwrap_or_default();
        let after = change.after.as_deref().unwrap_or_default();

        for hunk in hunks(before, after) {
            text.push_str(&hunk.render());
        }

        text.push('\n');
    }

    crate::tool::truncate_middle(text.trim_end(), max_chars)
}

/// One run of changed lines with the unchanged lines around it.
struct Hunk<'a> {
    /// Line number, from 1, of the first line shown in each version.
    old_start: usize,
    new_start: usize,

    leading: Vec<&'a str>,
    removed: Vec<&'a str>,
    added: Vec<&'a str>,
    trailing: Vec<&'a str>,
}

impl Hunk<'_> {
    fn render(&self) -> String {
        let context = self.leading.len() + self.trailing.len();

        let mut text = format!(
            "@@ -{},{} +{},{} @@\n",
            self.old_start,
            context + self.removed.len(),
            self.new_start,
            context + self.added.len()
        );

        let groups = [
            (' ', &self.leading),
            ('-', &self.removed),
            ('+', &self.added),
            (' ', &self.trailing),
        ];

        for (mark, lines) in groups {
            for line in lines {
                text.push(mark);
                text.push_str(line);
                text.push('\n');
            }
        }

        text
    }
}

/// The changed runs between two texts, each with its surrounding lines.
fn hunks<'a>(before: &'a str, after: &'a str) -> Vec<Hunk<'a>> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();

    let mut found = Vec::new();

    for (old_range, new_range) in changed_runs(&old, &new) {
        let lead_from = old_range.start.saturating_sub(CONTEXT_LINES);
        let trail_to = (old_range.end + CONTEXT_LINES).min(old.len());

        found.push(Hunk {
            old_start: lead_from + 1,
            new_start: new_range.start - (old_range.start - lead_from) + 1,
            leading: old[lead_from..old_range.start].to_vec(),
            removed: old[old_range.clone()].to_vec(),
            added: new[new_range].to_vec(),
            trailing: old[old_range.end..trail_to].to_vec(),
        });
    }

    found
}

type Run = (std::ops::Range<usize>, std::ops::Range<usize>);

/// Ranges of `old` and the ranges of `new` that replace them.
fn changed_runs(old: &[&str], new: &[&str]) -> Vec<Run> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();

    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];

    if old_middle.is_empty() && new_middle.is_empty() {
        return Vec::new();
    }

    let whole = (
        prefix..prefix + old_middle.len(),
        prefix..prefix + new_middle.len(),
    );

    if old_middle.len() * new_middle.len() > MAX_COMPARED_CELLS {
        return vec![whole];
    }

    let kept = common_lines(old_middle, new_middle);

    let mut runs = Vec::new();
    let (mut at_old, mut at_new) = (0, 0);

    // A sentinel past both ends closes the last run.
    let ends = kept
        .into_iter()
        .chain([(old_middle.len(), new_middle.len())]);

    for (keep_old, keep_new) in ends {
        if keep_old > at_old || keep_new > at_new {
            runs.push((
                prefix + at_old..prefix + keep_old,
                prefix + at_new..prefix + keep_new,
            ));
        }

        at_old = keep_old + 1;
        at_new = keep_new + 1;
    }

    runs
}

/// A longest common subsequence of lines, as pairs of positions.
fn common_lines(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let width = new.len() + 1;

    // lengths[i][j]: the longest common run of old[i..] and new[j..].
    let mut lengths = vec![0u32; (old.len() + 1) * width];

    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * width + j] = if old[i] == new[j] {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);

    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            pairs.push((i, j));

            i += 1;
            j += 1;
        } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }

    pairs
}

/// Every regular file under `root` with its size, by relative path.
/// Links are not followed: a linked `node_modules` is not the work.
fn files_under(root: &Path) -> BTreeMap<String, u64> {
    let mut files = BTreeMap::new();

    collect_files(root, root, &mut files);

    files
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeMap<String, u64>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let name = entry.file_name();

        if kind.is_dir() {
            if !SKIPPED_DIRECTORIES.iter().any(|skipped| name == *skipped) {
                collect_files(root, &path, files);
            }
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();

            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);

            files.insert(relative, size);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn numbered(count: usize) -> String {
        (1..=count).map(|line| format!("line {line}\n")).collect()
    }

    #[test]
    fn a_changed_line_is_shown_with_the_lines_around_it() {
        let before = numbered(20);
        let after = before.replace("line 10\n", "line ten\n");

        let change = Change {
            path: "a.txt".to_string(),
            before: Some(before),
            after: Some(after),
            opaque: false,
        };

        let text = render(std::slice::from_ref(&change), 10_000);

        assert!(text.contains(
            "--- a.txt\n@@ -7,7 +7,7 @@\n line 7\n line 8\n line 9\n-line 10\n+line ten\n line 11"
        ));
        assert!(!text.contains("line 14"));
        assert_eq!(size(&[change]), 2);
    }

    #[test]
    fn two_separate_edits_are_two_runs() {
        let before = numbered(30);

        let after = before
            .replace("line 3\n", "")
            .replace("line 25\n", "line 25\nline 25b\n");

        let old: Vec<&str> = before.lines().collect();
        let new: Vec<&str> = after.lines().collect();

        assert_eq!(
            changed_runs(&old, &new),
            vec![(2..3, 2..2), (25..25, 24..25)]
        );
    }

    #[test]
    fn originals_show_only_files_that_ended_up_different() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join("a.py"), "one\n").unwrap();
        fs::write(dir.path().join("b.py"), "two\n").unwrap();

        let workspace = Workspace::new(dir.path()).unwrap();

        let call = |path: &str| ToolCall {
            id: "call_0".to_string(),
            name: "edit_file".to_string(),
            input: json!({ "path": path }),
        };

        let mut originals = Originals::default();

        originals.keep(&workspace, &call("a.py"));
        originals.keep(&workspace, &call("b.py"));
        originals.keep(&workspace, &call("new.py"));

        fs::write(dir.path().join("a.py"), "ONE\n").unwrap();
        fs::write(dir.path().join("new.py"), "three\n").unwrap();

        // A second edit does not replace what was kept first.
        originals.keep(&workspace, &call("a.py"));

        let changes = originals.changes(&workspace);

        let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();

        assert_eq!(paths, ["a.py", "new.py"]);
        assert_eq!(changes[0].before.as_deref(), Some("one\n"));
        assert_eq!(changes[1].before, None);
    }

    #[test]
    fn two_copies_are_compared_and_one_made_to_match_the_other() {
        let original = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();

        for root in [original.path(), copy.path()] {
            fs::create_dir_all(root.join("src")).unwrap();
            fs::create_dir_all(root.join("node_modules")).unwrap();

            fs::write(root.join("src/keep.ts"), "same\n").unwrap();
            fs::write(root.join("src/edit.ts"), "old\n").unwrap();
            fs::write(root.join("src/gone.ts"), "bye\n").unwrap();
        }

        fs::write(copy.path().join("src/edit.ts"), "new\n").unwrap();
        fs::write(copy.path().join("src/added.ts"), "hello\n").unwrap();
        fs::write(copy.path().join("node_modules/x.js"), "ignored\n").unwrap();
        fs::remove_file(copy.path().join("src/gone.ts")).unwrap();

        let changes = between(original.path(), copy.path());

        let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();

        assert_eq!(paths, ["src/added.ts", "src/edit.ts", "src/gone.ts"]);

        apply(&changes, copy.path(), original.path()).unwrap();

        assert!(between(original.path(), copy.path()).is_empty());
        assert!(!original.path().join("src/gone.ts").exists());
    }
}
