//! Working out which commands check a project.
//!
//! Only checks the project has set up for itself are used: a `test` script,
//! a test directory, a Cargo manifest. Nothing is guessed that the project
//! does not already run.

use std::fs;
use std::path::Path;

use serde_json::Value;

/// Makes test runners that would otherwise watch for changes run once.
const RUN_ONCE: &str = "CI=1";

/// Scripts in `package.json` that check the code, in the order to run them.
const NODE_SCRIPTS: [&str; 3] = ["typecheck", "type-check", "test"];

pub fn checks(root: &Path) -> Vec<String> {
    let mut commands = Vec::new();

    commands.extend(node_checks(root));
    commands.extend(python_checks(root));
    commands.extend(rust_checks(root));

    commands
}

fn node_checks(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };

    let Ok(package) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };

    let Some(scripts) = package["scripts"].as_object() else {
        return Vec::new();
    };

    let runner = package_manager(root);

    NODE_SCRIPTS
        .iter()
        .filter(|name| {
            scripts
                .get(**name)
                .and_then(Value::as_str)
                .is_some_and(|script| !is_placeholder(script))
        })
        .map(|name| format!("{RUN_ONCE} {runner} run {name}"))
        .collect()
}

/// `npm init` writes a `test` script that only prints an error.
fn is_placeholder(script: &str) -> bool {
    script.contains("no test specified")
}

fn package_manager(root: &Path) -> &'static str {
    let lockfiles = [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lockb", "bun"),
        ("bun.lock", "bun"),
    ];

    lockfiles
        .iter()
        .find(|(lockfile, _)| root.join(lockfile).is_file())
        .map(|(_, runner)| *runner)
        .unwrap_or("npm")
}

fn python_checks(root: &Path) -> Vec<String> {
    let has_tests = ["tests", "test"]
        .iter()
        .any(|directory| root.join(directory).is_dir())
        || has_file_matching(root, |name| {
            (name.starts_with("test_") || name.ends_with("_test.py")) && name.ends_with(".py")
        });

    let is_python = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "pytest.ini",
        "tox.ini",
    ]
    .iter()
    .any(|file| root.join(file).is_file())
        || has_file_matching(root, |name| name.ends_with(".py"));

    if has_tests && is_python {
        vec!["python3 -m pytest -q".to_string()]
    } else {
        Vec::new()
    }
}

fn rust_checks(root: &Path) -> Vec<String> {
    if root.join("Cargo.toml").is_file() {
        vec!["cargo test --quiet".to_string()]
    } else {
        Vec::new()
    }
}

/// Whether a file directly inside `root` has a name accepted by `matches`.
fn has_file_matching(root: &Path, matches: impl Fn(&str) -> bool) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };

    entries
        .filter_map(Result::ok)
        .any(|entry| matches(&entry.file_name().to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_project_uses_its_own_scripts() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "test": "vitest run", "lint": "eslint", "typecheck": "tsc" } }"#,
        )
        .unwrap();

        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        assert_eq!(
            checks(dir.path()),
            ["CI=1 pnpm run typecheck", "CI=1 pnpm run test"]
        );
    }

    #[test]
    fn the_npm_placeholder_script_is_not_a_check() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "test": "echo \"Error: no test specified\" && exit 1" } }"#,
        )
        .unwrap();

        assert!(checks(dir.path()).is_empty());
    }

    #[test]
    fn python_needs_tests_to_have_a_check() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join("app.py"), "").unwrap();

        assert!(checks(dir.path()).is_empty());

        fs::write(dir.path().join("test_app.py"), "").unwrap();

        assert_eq!(checks(dir.path()), ["python3 -m pytest -q"]);
    }

    #[test]
    fn rust_project_runs_cargo_test() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join("Cargo.toml"), "").unwrap();

        assert_eq!(checks(dir.path()), ["cargo test --quiet"]);
    }
}
