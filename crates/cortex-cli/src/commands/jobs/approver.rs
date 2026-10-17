//! What a background job may do without a person to ask.

use async_trait::async_trait;
use cortex_core::{ApprovalRequest, Approver, destructive};

/// Approves what a job does in its own worktree, and refuses what could
/// harm more than that: the commands [`destructive`] names, and git
/// commands that move the job off its branch or publish its work. The
/// runner commits and merges for the job. Deleting a file is allowed:
/// in a worktree it is undone by not merging the branch.
pub struct JobApprover;

/// Git subcommands a job leaves to the runner, or to the person.
const REFUSED_GIT: [&str; 11] = [
    "push",
    "checkout",
    "switch",
    "merge",
    "rebase",
    "reset",
    "worktree",
    "branch",
    "tag",
    "stash",
    "commit",
];

impl JobApprover {
    pub fn refusal(request: &ApprovalRequest) -> Option<String> {
        if request.tool == "shell"
            && let Some(reason) = destructive(request)
        {
            return Some(format!("a background job may not run a command that {reason}"));
        }

        if request.tool == "shell" && refused_git(&request.summary) {
            return Some(
                "a background job leaves committing, merging, pushing and changing \
                 branches to the job runner"
                    .to_string(),
            );
        }

        None
    }
}

/// Whether `command` runs one of [`REFUSED_GIT`], anywhere in a chain.
fn refused_git(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|c: char| {
            c.is_whitespace() || matches!(c, ';' | '&' | '|' | '`' | '(' | ')' | '\'' | '"')
        })
        .filter(|word| !word.is_empty())
        .collect();

    words.iter().enumerate().any(|(index, word)| {
        if !(*word == "git" || word.ends_with("/git")) {
            return false;
        }

        // The subcommand is the first word after any options, such as
        // `git -C dir push`.
        let mut rest = words[index + 1..].iter();

        while let Some(next) = rest.next() {
            if *next == "-C" || *next == "-c" {
                rest.next();

                continue;
            }

            if next.starts_with('-') {
                continue;
            }

            return REFUSED_GIT.contains(next);
        }

        false
    })
}

#[async_trait]
impl Approver for JobApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        match Self::refusal(request) {
            None => true,

            Some(reason) => {
                eprintln!("refused in a job: {} ({}): {reason}", request.tool, request.summary);

                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> ApprovalRequest {
        ApprovalRequest {
            tool: "shell".to_string(),
            summary: command.to_string(),
        }
    }

    #[test]
    fn work_in_the_worktree_is_approved_and_branch_changes_are_not() {
        assert!(JobApprover::refusal(&shell("cargo test")).is_none());
        assert!(JobApprover::refusal(&shell("git status && git diff")).is_none());
        assert!(JobApprover::refusal(&shell("git log --oneline -5")).is_none());

        assert!(JobApprover::refusal(&shell("git push origin main")).is_some());
        assert!(JobApprover::refusal(&shell("cargo fmt; git -C . checkout main")).is_some());
        assert!(JobApprover::refusal(&shell("/usr/bin/git reset --hard")).is_some());
        assert!(JobApprover::refusal(&shell("rm -rf src")).is_some());

        let delete = ApprovalRequest {
            tool: "delete_file".to_string(),
            summary: "a.rs".to_string(),
        };

        assert!(JobApprover::refusal(&delete).is_none());
    }
}
