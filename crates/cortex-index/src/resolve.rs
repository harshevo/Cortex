//! Turning an import as written (`./money`, `@/lib/money`, `app.models`)
//! into the project file it refers to.
//!
//! Imports of installed packages resolve to nothing: the graph covers the
//! project's own code.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::language::Language;

const SCRIPT_EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// A `paths` entry from `tsconfig.json`: imports starting with `prefix`
/// are looked up under `directory`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alias {
    prefix: String,
    directory: PathBuf,
}

/// The project-wide settings import resolution depends on.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    root: PathBuf,
    aliases: Vec<Alias>,
}

impl Resolver {
    pub fn for_project(root: &Path) -> Self {
        let mut aliases = Vec::new();

        for config in ["tsconfig.json", "jsconfig.json"] {
            if let Ok(text) = fs::read_to_string(root.join(config)) {
                aliases.extend(read_aliases(root, &text));
            }
        }

        Self {
            root: root.to_path_buf(),
            aliases,
        }
    }

    /// The file that `source`, imported from `importer`, refers to.
    pub fn resolve(&self, importer: &Path, source: &str) -> Option<PathBuf> {
        match Language::for_path(importer)? {
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                self.resolve_script(importer, source)
            }

            Language::Python => self.resolve_python(importer, source),

            Language::Rust => self.resolve_rust(importer, source),
        }
    }

    fn resolve_script(&self, importer: &Path, source: &str) -> Option<PathBuf> {
        if source.starts_with('.') {
            return script_file(&normalize(&importer.parent()?.join(source)));
        }

        self.aliases
            .iter()
            .filter_map(|alias| {
                let rest = source.strip_prefix(&alias.prefix)?;

                script_file(&normalize(&alias.directory.join(rest)))
            })
            .next()
    }

    fn resolve_python(&self, importer: &Path, source: &str) -> Option<PathBuf> {
        let dots = source
            .chars()
            .take_while(|&character| character == '.')
            .count();

        let module = &source[dots..];

        let relative: PathBuf = module.split('.').filter(|part| !part.is_empty()).collect();

        // `from . import x` starts in the importer's own package; each
        // further dot climbs one package.
        if dots > 0 {
            let mut base = importer.parent()?.to_path_buf();

            for _ in 1..dots {
                base.pop();
            }

            return python_file(&base.join(relative));
        }

        [self.root.clone(), self.root.join("src")]
            .iter()
            .filter_map(|base| python_file(&base.join(&relative)))
            .next()
    }

    /// Best effort for `crate::a::b` and `super::a`: Rust module paths do
    /// not map to files without reading every `mod` declaration, so only
    /// the usual layouts are tried. Other crates resolve to nothing.
    fn resolve_rust(&self, importer: &Path, source: &str) -> Option<PathBuf> {
        let mut segments: Vec<&str> = source.split("::").collect();

        let base = match segments.first().copied()? {
            "crate" => crate_source_dir(importer)?,
            "super" => importer.parent()?.parent()?.to_path_buf(),
            "self" => importer.parent()?.to_path_buf(),
            _ => return None,
        };

        segments.remove(0);

        // The path may end in a type or function inside the module, so try
        // the longest module path first and shorten it.
        while !segments.is_empty() {
            let relative: PathBuf = segments.iter().collect();

            let candidates = [
                base.join(&relative).with_extension("rs"),
                base.join(&relative).join("mod.rs"),
            ];

            if let Some(found) = candidates.into_iter().find(|path| path.is_file()) {
                return Some(found);
            }

            segments.pop();
        }

        None
    }
}

/// `path` itself, with a source extension, or as a directory's index file.
fn script_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() && Language::for_path(path).is_some() {
        return Some(path.to_path_buf());
    }

    // TypeScript projects that emit ES modules write `./x.js` for `x.ts`.
    let stem = match path.extension().and_then(|extension| extension.to_str()) {
        Some("js" | "mjs" | "cjs" | "jsx") => path.with_extension(""),
        _ => path.to_path_buf(),
    };

    for extension in SCRIPT_EXTENSIONS {
        let candidate = append_extension(&stem, extension);

        if candidate.is_file() {
            return Some(candidate);
        }
    }

    for extension in SCRIPT_EXTENSIONS {
        let candidate = path.join(format!("index.{extension}"));

        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn python_file(path: &Path) -> Option<PathBuf> {
    let module = append_extension(path, "py");

    if module.is_file() {
        return Some(module);
    }

    let package = path.join("__init__.py");

    package.is_file().then_some(package)
}

/// The `src` directory of the crate that `file` belongs to.
fn crate_source_dir(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|directory| directory.join("Cargo.toml").is_file())
        .map(|directory| directory.join("src"))
}

/// Adds an extension without replacing an existing one, so `a.test`
/// becomes `a.test.ts` rather than `a.ts`.
fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();

    name.push(".");
    name.push(extension);

    PathBuf::from(name)
}

/// Reads `compilerOptions.paths` from the text of a `tsconfig.json`.
fn read_aliases(root: &Path, text: &str) -> Vec<Alias> {
    let Ok(config) = serde_json::from_str::<Value>(&strip_json_comments(text)) else {
        return Vec::new();
    };

    let options = &config["compilerOptions"];

    let base = match options["baseUrl"].as_str() {
        Some(base_url) => root.join(base_url),
        None => root.to_path_buf(),
    };

    let Some(paths) = options["paths"].as_object() else {
        return Vec::new();
    };

    let mut aliases = Vec::new();

    for (pattern, targets) in paths {
        let Some(target) = targets.get(0).and_then(Value::as_str) else {
            continue;
        };

        // Only the common `prefix/*` form is supported.
        let (Some(prefix), Some(directory)) = (pattern.strip_suffix('*'), target.strip_suffix('*'))
        else {
            continue;
        };

        aliases.push(Alias {
            prefix: prefix.to_string(),
            directory: normalize(&base.join(directory)),
        });
    }

    aliases
}

/// `tsconfig.json` allows comments and trailing commas, which JSON does
/// not. Removes both, leaving the contents of strings alone.
fn strip_json_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    let mut in_string = false;

    while let Some(character) = characters.next() {
        if in_string {
            output.push(character);

            match character {
                '\\' => output.extend(characters.next()),
                '"' => in_string = false,
                _ => {}
            }

            continue;
        }

        match (character, characters.peek()) {
            ('"', _) => {
                in_string = true;

                output.push(character);
            }

            ('/', Some('/')) => {
                for skipped in characters.by_ref() {
                    if skipped == '\n' {
                        output.push('\n');

                        break;
                    }
                }
            }

            ('/', Some('*')) => {
                characters.next();

                let mut previous = ' ';

                for skipped in characters.by_ref() {
                    if previous == '*' && skipped == '/' {
                        break;
                    }

                    previous = skipped;
                }
            }

            _ => output.push(character),
        }
    }

    remove_trailing_commas(&output)
}

fn remove_trailing_commas(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_string = false;
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        if in_string {
            output.push(character);

            match character {
                '\\' => output.extend(characters.next()),
                '"' => in_string = false,
                _ => {}
            }

            continue;
        }

        if character == '"' {
            in_string = true;
        }

        if character == ',' {
            let rest: String = characters.clone().collect();

            if rest.trim_start().starts_with(['}', ']']) {
                continue;
            }
        }

        output.push(character);
    }

    output
}

/// Removes `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
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

    #[test]
    fn relative_alias_and_index_imports() {
        let dir = project(&[
            (
                "tsconfig.json",
                "{\n  // aliases\n  \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"], } },\n}\n",
            ),
            ("src/lib/money.ts", ""),
            ("src/features/calendar/index.ts", ""),
            ("src/features/import.tsx", ""),
        ]);

        let root = dir.path();
        let resolver = Resolver::for_project(root);
        let importer = root.join("src/features/import.tsx");

        assert_eq!(
            resolver.resolve(&importer, "@/lib/money"),
            Some(root.join("src/lib/money.ts"))
        );

        assert_eq!(
            resolver.resolve(&importer, "../lib/money.js"),
            Some(root.join("src/lib/money.ts"))
        );

        assert_eq!(
            resolver.resolve(&importer, "./calendar"),
            Some(root.join("src/features/calendar/index.ts"))
        );

        // Installed packages are outside the graph.
        assert_eq!(resolver.resolve(&importer, "react"), None);
    }

    #[test]
    fn python_absolute_and_relative_modules() {
        let dir = project(&[
            ("app/__init__.py", ""),
            ("app/money.py", ""),
            ("app/views/__init__.py", ""),
            ("app/views/cart.py", ""),
        ]);

        let root = dir.path();
        let resolver = Resolver::for_project(root);
        let importer = root.join("app/views/cart.py");

        assert_eq!(
            resolver.resolve(&importer, "app.money"),
            Some(root.join("app/money.py"))
        );

        assert_eq!(
            resolver.resolve(&importer, "..money"),
            Some(root.join("app/money.py"))
        );

        assert_eq!(
            resolver.resolve(&importer, "."),
            Some(root.join("app/views/__init__.py"))
        );
    }

    #[test]
    fn rust_crate_paths() {
        let dir = project(&[
            ("Cargo.toml", ""),
            ("src/lib.rs", ""),
            ("src/money.rs", ""),
            ("src/store/mod.rs", ""),
        ]);

        let root = dir.path();
        let resolver = Resolver::for_project(root);
        let importer = root.join("src/lib.rs");

        assert_eq!(
            resolver.resolve(&importer, "crate::money"),
            Some(root.join("src/money.rs"))
        );

        assert_eq!(
            resolver.resolve(&importer, "crate::store::Row"),
            Some(root.join("src/store/mod.rs"))
        );

        assert_eq!(resolver.resolve(&importer, "std::fs"), None);
    }
}
