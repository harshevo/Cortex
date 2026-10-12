//! The code graph: which file imports which, and where each name is used.
//!
//! Text search for a function's name finds comments, strings and unrelated
//! things that happen to share the name. The graph instead follows imports:
//! a use of `toCents` counts as a use of the `toCents` defined in
//! `money.ts` when the file it is in imports that name from that file.
//!
//! Tree-sitter sees syntax, not types, so a method call such as
//! `order.total()` cannot be tied to one class. Those are reported
//! separately as possible matches rather than silently mixed in.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::index::{CodeIndex, Located};
use crate::language::Language;
use crate::references::{FileReferences, Import, NameUse, UseKind, extract_references};
use crate::resolve::Resolver;
use crate::symbols::SymbolKind;
use crate::walk;

/// Re-exports are followed this many files deep (`index.ts` barrels).
const MAX_REEXPORT_DEPTH: usize = 4;

const MAX_LINE_CHARS: usize = 160;

/// How sure the graph is that a use refers to the symbol asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// The file defines the symbol or imports it from the defining file.
    Resolved,

    /// Same name, but no import ties it to the definition: a method call
    /// on a value of unknown type, or a name that reached the file some
    /// other way.
    Possible,
}

/// One place a symbol is used.
#[derive(Debug, Clone)]
pub struct Site {
    pub path: PathBuf,

    /// Line, counted from 1.
    pub line: usize,

    /// The source line, trimmed.
    pub text: String,

    pub kind: UseKind,
    pub confidence: Confidence,

    /// The function, method or class the use is inside.
    pub enclosing: Option<String>,
}

/// Everything known about the uses of one name.
#[derive(Debug, Clone, Default)]
pub struct References {
    pub definitions: Vec<Located>,
    pub sites: Vec<Site>,
}

impl References {
    pub fn calls(&self) -> impl Iterator<Item = &Site> {
        self.resolved(UseKind::Call)
    }

    pub fn resolved(&self, kind: UseKind) -> impl Iterator<Item = &Site> {
        self.sites
            .iter()
            .filter(move |site| site.kind == kind && site.confidence == Confidence::Resolved)
    }

    pub fn possible(&self) -> impl Iterator<Item = &Site> {
        self.sites
            .iter()
            .filter(|site| site.confidence == Confidence::Possible)
    }
}

/// One import statement and the project file it leads to.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// The file containing the import.
    pub importer: PathBuf,

    /// The imported file. `None` for installed packages and anything else
    /// outside the project.
    pub target: Option<PathBuf>,

    pub source: String,
    pub names: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphStats {
    pub files: usize,
    pub imports: usize,
    pub resolved_imports: usize,
    pub calls: usize,
}

struct GraphFile {
    modified: Option<SystemTime>,
    references: Arc<FileReferences>,
}

/// One file as seen by a single query.
struct Node {
    path: PathBuf,
    references: Arc<FileReferences>,

    /// The project file each import leads to, in the order of
    /// `references.imports`.
    targets: Vec<Option<PathBuf>>,
}

/// Parses files on demand and remembers the result until a file changes,
/// the same way [`CodeIndex`] does.
pub struct CodeGraph {
    index: Arc<CodeIndex>,
    files: Mutex<HashMap<PathBuf, GraphFile>>,
}

impl CodeGraph {
    pub fn new(index: Arc<CodeIndex>) -> Self {
        Self {
            index,
            files: Mutex::new(HashMap::new()),
        }
    }

    pub fn index(&self) -> &CodeIndex {
        &self.index
    }

    /// Every definition and use of `name` under `root`.
    pub fn references_to(&self, root: &Path, name: &str) -> References {
        let definitions = self.index.find(root, |candidate| candidate == name);

        let defining_files: HashSet<PathBuf> = definitions
            .iter()
            .map(|located| located.path.clone())
            .collect();

        let is_member_symbol = !definitions.is_empty()
            && definitions.iter().all(|located| {
                located.symbol.kind == SymbolKind::Method || located.symbol.container.is_some()
            });

        let nodes = self.nodes(root);

        let by_path: HashMap<&Path, &Node> = nodes
            .iter()
            .map(|node| (node.path.as_path(), node))
            .collect();

        let mut sites = Vec::new();

        for node in &nodes {
            let binding = Binding::of(node, name, &defining_files, &by_path);

            let matching: Vec<&NameUse> = node
                .references
                .uses
                .iter()
                .filter(|usage| usage.name == name || binding.aliases.contains(&usage.name))
                .collect();

            if matching.is_empty() {
                continue;
            }

            let lines = read_lines(&node.path);

            for usage in matching {
                sites.push(Site {
                    path: node.path.clone(),
                    line: usage.line,
                    text: line_text(&lines, usage.line),
                    kind: usage.kind,
                    confidence: binding.confidence(usage, is_member_symbol),
                    enclosing: self.enclosing_symbol(&node.path, usage.line),
                });
            }
        }

        // One entry per line: when a line uses the name twice, keep the
        // most certain and most specific use.
        sites.sort_by_key(|site| (site.path.clone(), site.line, rank(site)));
        sites.dedup_by(|later, earlier| later.path == earlier.path && later.line == earlier.line);

        References { definitions, sites }
    }

    /// The imports written in `file`.
    pub fn imports_of(&self, root: &Path, file: &Path) -> Vec<Dependency> {
        self.nodes(root)
            .iter()
            .filter(|node| node.path == file)
            .flat_map(dependencies)
            .collect()
    }

    /// The imports elsewhere in the project that lead to `file`.
    pub fn importers_of(&self, root: &Path, file: &Path) -> Vec<Dependency> {
        self.nodes(root)
            .iter()
            .flat_map(dependencies)
            .filter(|dependency| dependency.target.as_deref() == Some(file))
            .collect()
    }

    /// Every import inside the project, as `(importing file, imported
    /// file)`.
    pub fn import_edges(&self, root: &Path) -> Vec<(PathBuf, PathBuf)> {
        self.nodes(root)
            .iter()
            .flat_map(|node| {
                node.targets
                    .iter()
                    .flatten()
                    .map(|target| (node.path.clone(), target.clone()))
            })
            .collect()
    }

    /// The test files that import any of `changed`, directly or through
    /// other files, plus any of `changed` that are tests themselves.
    ///
    /// This is static test-impact analysis: the tests worth running after
    /// a change, found without running anything. It can miss a test that
    /// reaches the code some way other than an import, and it is the
    /// baseline a learned test-impact predictor has to beat.
    pub fn tests_affected_by(&self, root: &Path, changed: &[PathBuf]) -> Vec<PathBuf> {
        let nodes = self.nodes(root);

        // For each file, the files that import it.
        let mut importers: HashMap<&Path, Vec<&Path>> = HashMap::new();

        for node in &nodes {
            for target in node.targets.iter().flatten() {
                importers
                    .entry(target.as_path())
                    .or_default()
                    .push(&node.path);
            }
        }

        let mut reached: HashSet<&Path> = changed.iter().map(PathBuf::as_path).collect();
        let mut frontier: Vec<&Path> = reached.iter().copied().collect();

        while let Some(file) = frontier.pop() {
            for importer in importers.get(file).into_iter().flatten() {
                if reached.insert(importer) {
                    frontier.push(importer);
                }
            }
        }

        let mut tests: Vec<PathBuf> = reached
            .into_iter()
            .filter(|path| {
                let relative = path.strip_prefix(root).unwrap_or(path);

                walk::is_test_file(&relative.display().to_string())
            })
            .map(Path::to_path_buf)
            .collect();

        tests.sort();

        tests
    }

    pub fn stats(&self, root: &Path) -> GraphStats {
        let mut stats = GraphStats::default();

        for node in self.nodes(root) {
            stats.files += 1;
            stats.imports += node.targets.len();
            stats.resolved_imports += node.targets.iter().flatten().count();

            stats.calls += node
                .references
                .uses
                .iter()
                .filter(|usage| usage.kind == UseKind::Call)
                .count();
        }

        stats
    }

    /// The narrowest symbol in `path` whose lines include `line`.
    fn enclosing_symbol(&self, path: &Path, line: usize) -> Option<String> {
        self.index
            .file_symbols(path)
            .into_iter()
            .filter(|symbol| symbol.start_line <= line && line <= symbol.end_line)
            .min_by_key(|symbol| symbol.end_line - symbol.start_line)
            .map(|symbol| match symbol.container {
                Some(container) => format!("{container}.{}", symbol.name),
                None => symbol.name,
            })
    }

    /// The current state of every source file under `root`.
    fn nodes(&self, root: &Path) -> Vec<Node> {
        let resolver = Resolver::for_project(root);

        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut nodes = Vec::new();

        for path in walk::files_under(root) {
            let Some(language) = Language::for_path(&path) else {
                continue;
            };

            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok();

            let is_current = files
                .get(&path)
                .is_some_and(|file| file.modified == modified);

            if !is_current {
                let references = match fs::read_to_string(&path) {
                    Ok(source) => extract_references(&source, language),
                    Err(_) => FileReferences::default(),
                };

                files.insert(
                    path.clone(),
                    GraphFile {
                        modified,
                        references: Arc::new(references),
                    },
                );
            }

            let references = files[&path].references.clone();

            let targets = references
                .imports
                .iter()
                .map(|import| resolve_import(&resolver, &path, import))
                .collect();

            nodes.push(Node {
                path,
                references,
                targets,
            });
        }

        nodes
    }
}

/// Imports are resolved on every query rather than cached, because a file
/// created since the last query can make an old import resolvable.
fn resolve_import(resolver: &Resolver, importer: &Path, import: &Import) -> Option<PathBuf> {
    let direct = resolver.resolve(importer, &import.source);

    if Language::for_path(importer) != Some(Language::Python) {
        return direct;
    }

    // `from package import module` names a file, not something inside
    // `package/__init__.py`.
    let submodule = import.names.iter().find_map(|imported| {
        let separator = if import.source.ends_with('.') {
            ""
        } else {
            "."
        };

        resolver.resolve(
            importer,
            &format!("{}{separator}{}", import.source, imported.name),
        )
    });

    submodule.or(direct)
}

fn dependencies(node: &Node) -> Vec<Dependency> {
    node.references
        .imports
        .iter()
        .zip(&node.targets)
        .map(|(import, target)| Dependency {
            importer: node.path.clone(),
            target: target.clone(),
            source: import.source.clone(),
            names: import
                .names
                .iter()
                .map(|imported| imported.name.clone())
                .collect(),
            line: import.line,
        })
        .collect()
}

/// How one file can reach the symbol being asked about.
#[derive(Default)]
struct Binding {
    /// The file defines the symbol or imports it by name.
    direct: bool,

    /// Other names the symbol goes by in this file (`import { a as b }`).
    aliases: HashSet<String>,

    /// Names bound to a whole module that provides the symbol
    /// (`import * as money`), so `money.name` is a resolved use.
    namespaces: HashSet<String>,

    /// The file imports a providing module without naming what it takes.
    wildcard: bool,
}

impl Binding {
    fn of(
        node: &Node,
        name: &str,
        defining_files: &HashSet<PathBuf>,
        by_path: &HashMap<&Path, &Node>,
    ) -> Self {
        let mut binding = Self {
            direct: defining_files.contains(&node.path),
            ..Self::default()
        };

        for (import, target) in node.references.imports.iter().zip(&node.targets) {
            let Some(target) = target else {
                continue;
            };

            if !provides(target, name, defining_files, by_path, 0) {
                continue;
            }

            for imported in &import.names {
                let is_default_named_alike = imported.name == "default" && imported.local == name;

                if imported.name == name || is_default_named_alike {
                    binding.direct |= imported.local == name;

                    binding.aliases.insert(imported.local.clone());
                } else if imported.local == "*" {
                    binding.wildcard = true;
                } else {
                    // `import * as money`, a default import of an object,
                    // or a Python `from package import module`.
                    binding.namespaces.insert(imported.local.clone());
                }
            }
        }

        binding
    }

    fn confidence(&self, usage: &NameUse, is_member_symbol: bool) -> Confidence {
        let through_namespace = usage
            .receiver
            .as_ref()
            .is_some_and(|receiver| self.namespaces.contains(receiver));

        let resolved = if usage.kind == UseKind::Import {
            self.direct || !self.aliases.is_empty()
        } else if usage.is_member {
            // `value.name()` is only certain when `value` is the module.
            through_namespace || (self.wildcard && !is_member_symbol)
        } else {
            self.direct || self.aliases.contains(&usage.name) || self.wildcard
        };

        if resolved {
            Confidence::Resolved
        } else {
            Confidence::Possible
        }
    }
}

/// Whether importing `name` from `file` leads to one of `defining_files`,
/// directly or through re-exports.
fn provides(
    file: &Path,
    name: &str,
    defining_files: &HashSet<PathBuf>,
    by_path: &HashMap<&Path, &Node>,
    depth: usize,
) -> bool {
    if defining_files.contains(file) {
        return true;
    }

    if depth >= MAX_REEXPORT_DEPTH {
        return false;
    }

    let Some(node) = by_path.get(file) else {
        return false;
    };

    node.references
        .imports
        .iter()
        .zip(&node.targets)
        .filter(|(import, _)| import.is_reexport)
        .filter(|(import, _)| {
            import
                .names
                .iter()
                .any(|imported| imported.name == name || imported.name == "*")
        })
        .filter_map(|(_, target)| target.as_ref())
        .any(|target| provides(target, name, defining_files, by_path, depth + 1))
}

/// Sort order among uses on the same line: certain before possible, then
/// calls before mentions before imports.
fn rank(site: &Site) -> (u8, u8) {
    let confidence = match site.confidence {
        Confidence::Resolved => 0,
        Confidence::Possible => 1,
    };

    let kind = match site.kind {
        UseKind::Call => 0,
        UseKind::Mention => 1,
        UseKind::Import => 2,
    };

    (confidence, kind)
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|source| source.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn line_text(lines: &[String], line: usize) -> String {
    let text = lines
        .get(line.saturating_sub(1))
        .map(|text| text.trim())
        .unwrap_or_default();

    if text.chars().count() <= MAX_LINE_CHARS {
        return text.to_string();
    }

    let clipped: String = text.chars().take(MAX_LINE_CHARS).collect();

    format!("{clipped}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();

        for (path, contents) in files {
            let file = dir.path().join(path);

            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
        }

        dir
    }

    fn graph() -> CodeGraph {
        CodeGraph::new(Arc::new(CodeIndex::new()))
    }

    fn located(root: &Path, sites: Vec<&Site>) -> Vec<(String, usize)> {
        sites
            .into_iter()
            .map(|site| {
                let relative = site.path.strip_prefix(root).unwrap();

                (relative.display().to_string(), site.line)
            })
            .collect()
    }

    const MONEY: &str = "export function toCents(value: string): number {\n  return 1;\n}\n";

    #[test]
    fn call_sites_are_found_through_imports_aliases_and_barrels() {
        let dir = project(&[
            (
                "tsconfig.json",
                "{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"] } } }",
            ),
            ("src/lib/money.ts", MONEY),
            ("src/lib/index.ts", "export { toCents } from \"./money\";\n"),
            (
                "src/a.ts",
                "import { toCents } from \"@/lib/money\";\n\nexport function fee() {\n  return toCents(\"1\");\n}\n",
            ),
            (
                "src/b.ts",
                "import { toCents as cents } from \"./lib\";\n\nconst x = cents(\"2\");\n",
            ),
            (
                "src/c.ts",
                "import * as money from \"./lib/money\";\n\nconst y = money.toCents(\"3\");\n",
            ),
            // Same name, no import: a different function.
            ("src/d.ts", "function toCents() {}\n"),
            ("src/e.ts", "const z = other.toCents(\"4\");\n"),
        ]);

        let root = dir.path().canonicalize().unwrap();
        let references = graph().references_to(&root, "toCents");

        assert_eq!(references.definitions.len(), 2);

        assert_eq!(
            located(&root, references.calls().collect()),
            [
                ("src/a.ts".to_string(), 4),
                ("src/b.ts".to_string(), 3),
                ("src/c.ts".to_string(), 3),
            ]
        );

        let call = references.calls().next().unwrap();

        assert_eq!(call.enclosing.as_deref(), Some("fee"));
        assert_eq!(call.text, "return toCents(\"1\");");

        assert_eq!(
            located(&root, references.possible().collect()),
            [("src/e.ts".to_string(), 1)]
        );
    }

    #[test]
    fn importers_and_imports_of_a_file() {
        let dir = project(&[
            ("src/money.ts", MONEY),
            (
                "src/a.ts",
                "import { toCents } from \"./money\";\nimport fs from \"fs\";\n",
            ),
            ("src/b.ts", "import \"./money\";\n"),
        ]);

        let root = dir.path().canonicalize().unwrap();
        let graph = graph();

        let importers = graph.importers_of(&root, &root.join("src/money.ts"));

        assert_eq!(importers.len(), 2);
        assert_eq!(importers[0].names, ["toCents"]);

        let imports = graph.imports_of(&root, &root.join("src/a.ts"));

        assert_eq!(imports.len(), 2);
        assert!(imports[0].target.is_some());
        assert!(imports[1].target.is_none());

        let stats = graph.stats(&root);

        assert_eq!(stats.files, 3);
        assert_eq!(stats.imports, 3);
        assert_eq!(stats.resolved_imports, 2);
    }

    #[test]
    fn affected_tests_are_found_through_chains_of_imports() {
        let dir = project(&[
            ("src/money.ts", MONEY),
            ("src/fees.ts", "import { toCents } from \"./money\";\n"),
            ("src/dates.ts", "export const today = 1;\n"),
            ("tests/fees.test.ts", "import \"../src/fees\";\n"),
            ("tests/dates.test.ts", "import \"../src/dates\";\n"),
        ]);

        let root = dir.path().canonicalize().unwrap();
        let graph = graph();

        // The fees test reaches money.ts only through fees.ts.
        assert_eq!(
            graph.tests_affected_by(&root, &[root.join("src/money.ts")]),
            [root.join("tests/fees.test.ts")]
        );

        // A changed test is itself affected.
        assert_eq!(
            graph.tests_affected_by(&root, &[root.join("tests/dates.test.ts")]),
            [root.join("tests/dates.test.ts")]
        );
    }

    #[test]
    fn python_call_sites_through_from_imports() {
        let dir = project(&[
            ("app/__init__.py", ""),
            ("app/money.py", "def to_cents(value):\n    return 1\n"),
            (
                "app/cart.py",
                "from app.money import to_cents\nfrom . import money\n\n\ndef total():\n    return to_cents(1) + money.to_cents(2)\n",
            ),
        ]);

        let root = dir.path().canonicalize().unwrap();
        let references = graph().references_to(&root, "to_cents");

        assert_eq!(
            located(&root, references.calls().collect()),
            [("app/cart.py".to_string(), 6)]
        );

        assert_eq!(
            references.calls().next().unwrap().enclosing.as_deref(),
            Some("total")
        );
    }
}
