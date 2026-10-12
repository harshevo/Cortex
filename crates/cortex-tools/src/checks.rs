//! Shortening the output of tests, builds and linters.
//!
//! A check that passes prints a great deal the model has no use for: one
//! line per test, progress bars, timings. Only the closing summary matters.
//! A check that fails matters in full, but its colour codes, blank lines
//! and the middle of very long logs do not.
//!
//! Nothing here touches the output of other commands. `cat` and `git diff`
//! succeed too, and their output is the whole point of running them.

use cortex_verify::summary;

/// A passing check with less output than this is shown as it is.
const QUIET_ABOVE_CHARS: usize = 1_500;

/// What is kept from the end of a passing check's output.
const TAIL_LINES: usize = 15;
const TAIL_CHARS: usize = 1_200;

/// Programs that are checks whatever their arguments.
const RUNNERS: [&str; 14] = [
    "pytest", "vitest", "jest", "mocha", "tsc", "eslint", "mypy", "ruff", "tox", "rspec",
    "unittest", "clippy", "phpunit", "ctest",
];

/// Programs that run a project's scripts, and the scripts that are checks.
const LAUNCHERS: [&str; 9] = [
    "npm", "yarn", "pnpm", "bun", "cargo", "go", "make", "deno", "gradle",
];

const CHECK_SCRIPTS: [&str; 6] = ["test", "build", "check", "lint", "typecheck", "vet"];

/// Programs whose output is what the model asked to see.
const VIEWERS: [&str; 14] = [
    "cat", "head", "tail", "sed", "grep", "rg", "ls", "git", "find", "echo", "wc", "awk", "less",
    "diff",
];

/// Whether `command` runs tests, a build or a linter.
pub fn is_check_command(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|character: char| character.is_whitespace() || matches!(character, ';' | '&' | '|'))
        .filter(|word| !word.is_empty())
        .map(program_name)
        .collect();

    let first = words.iter().find(|word| !word.contains('='));

    if first.is_some_and(|word| VIEWERS.contains(word)) {
        return false;
    }

    let runs_a_runner = words.iter().any(|word| RUNNERS.contains(word));

    // `npm test`, and also `npm run test`.
    let launches_a_check = words.iter().enumerate().any(|(position, word)| {
        let script = words[position + 1..].iter().find(|next| **next != "run");

        LAUNCHERS.contains(word) && script.is_some_and(|script| is_check_script(script))
    });

    runs_a_runner || launches_a_check
}

/// `node_modules/.bin/vitest` is `vitest`.
fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `test`, and also `test:unit`.
fn is_check_script(script: &str) -> bool {
    let name = script.split(':').next().unwrap_or(script);

    CHECK_SCRIPTS.contains(&name)
}

/// The output of a check that passed: its last lines, where the summary is.
pub fn passed(output: &str) -> String {
    let clean = summary::strip_ansi(output);

    if clean.len() <= QUIET_ABOVE_CHARS {
        return clean.trim_end().to_string();
    }

    let lines: Vec<&str> = clean
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();

    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0;

    for line in lines.iter().rev().take(TAIL_LINES) {
        if chars + line.len() > TAIL_CHARS {
            break;
        }

        chars += line.len();

        kept.push(line);
    }

    kept.reverse();

    let left_out = lines.len() - kept.len();

    format!(
        "[{left_out} earlier lines left out because the command succeeded. To see them, run a \
         narrower command or pipe this one through `grep`.]\n{}",
        kept.join("\n")
    )
}

/// The output of a check that failed: cleaned, and with the middle of a
/// very long log removed.
pub fn failed(output: &str) -> String {
    summary::summarize(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_and_build_commands_are_checks() {
        for command in [
            "npm test",
            "CI=1 npm run test",
            "npm run test:unit -- fees",
            "npx vitest run src/fees.test.ts",
            "cd app && pnpm run build",
            "python3 -m pytest -q",
            "./node_modules/.bin/tsc --noEmit",
            "cargo test -p cortex-core",
            "cargo clippy",
        ] {
            assert!(is_check_command(command), "{command}");
        }
    }

    #[test]
    fn commands_run_for_their_output_are_not() {
        for command in [
            "cat tests/fees.test.ts",
            "git diff",
            "grep -rn pytest .",
            "node scripts/print-report.js",
            "ls test",
            "python3 -c 'import pagination'",
        ] {
            assert!(!is_check_command(command), "{command}");
        }
    }

    #[test]
    fn a_long_passing_run_keeps_its_summary() {
        let output = format!(
            "{}\n\n\x1b[32mTests  412 passed\x1b[0m\nDuration  3.1s\n",
            " ✓ a test that passed\n".repeat(400)
        );

        let shown = passed(&output);

        assert!(shown.len() < 1_600);
        assert!(shown.starts_with("[387 earlier lines left out"));
        assert!(shown.ends_with("Tests  412 passed\nDuration  3.1s"));
    }

    #[test]
    fn a_short_passing_run_is_shown_whole() {
        assert_eq!(passed("3 passed\n"), "3 passed");
    }
}
