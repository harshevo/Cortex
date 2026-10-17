//! Commands the person sets to run before and after tool calls.
//!
//! Hooks let a project or a person add their own rules without changing
//! the harness: format a file after every edit, refuse writes to a
//! generated folder, log every command. They are read from
//! `.cortex/hooks.json` in the project and `~/.cortex/hooks.json`, in the
//! format Claude Code uses, so existing hooks can be copied over:
//!
//! ```json
//! { "hooks": {
//!     "PreToolUse":  [{ "matcher": "shell", "hooks": [{ "type": "command", "command": "./check.sh" }] }],
//!     "PostToolUse": [{ "matcher": "edit_file|write_file", "hooks": [{ "type": "command", "command": "cargo fmt" }] }]
//! } }
//! ```
//!
//! A matcher is tool names separated by `|`; empty or `*` matches every
//! tool. Each command gets the call as JSON on its standard input
//! (`hook_event_name`, `tool_name`, `tool_input`, and for `PostToolUse`
//! `tool_response`). A `PreToolUse` command that exits with code 2 stops
//! the call, and what it wrote to standard error is the model's answer. A
//! `PostToolUse` command that exits with 2 has what it wrote shown to the
//! model after the tool's output. Any other exit is ignored.
//!
//! Hooks run in the project, inside the same sandbox as the shell tool.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::message::{ToolCall, ToolResult};
use crate::process;
use crate::workspace::Workspace;

const DEFAULT_TIMEOUT: u64 = 60;

/// Runs around tool calls. See the module documentation.
#[async_trait]
pub trait ToolHooks: Send + Sync {
    /// `Some(reason)` stops the call; the reason is what the model is told.
    async fn before(&self, call: &ToolCall, workspace: &Workspace) -> Option<String>;

    /// Text to add for the model after the tool's output, if any.
    async fn after(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        workspace: &Workspace,
    ) -> Option<String>;
}

#[derive(Debug, Clone, Default, Deserialize)]
struct File {
    #[serde(default)]
    hooks: Events,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Events {
    #[serde(default, rename = "PreToolUse")]
    pre_tool_use: Vec<Matcher>,

    #[serde(default, rename = "PostToolUse")]
    post_tool_use: Vec<Matcher>,
}

#[derive(Debug, Clone, Deserialize)]
struct Matcher {
    #[serde(default)]
    matcher: String,

    #[serde(default)]
    hooks: Vec<Hook>,
}

#[derive(Debug, Clone, Deserialize)]
struct Hook {
    command: String,

    /// Seconds.
    #[serde(default)]
    timeout: Option<u64>,
}

impl Matcher {
    fn matches(&self, tool: &str) -> bool {
        let pattern = self.matcher.trim();

        pattern.is_empty() || pattern == "*" || pattern.split('|').any(|name| name.trim() == tool)
    }
}

/// The hooks of the files found, project first.
#[derive(Debug, Clone, Default)]
pub struct CommandHooks {
    events: Events,

    /// The files they came from, to tell the person.
    pub sources: Vec<String>,
}

impl CommandHooks {
    /// Reads `.cortex/hooks.json` in `project` and in `home`, either of
    /// which may be missing. A file that cannot be read is reported and
    /// passed over.
    pub fn load(project: &Path, home: Option<&Path>) -> Self {
        let mut hooks = Self::default();

        let files = [Some(project), home]
            .into_iter()
            .flatten()
            .map(|folder| folder.join(".cortex").join("hooks.json"));

        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };

            match serde_json::from_str::<File>(&text) {
                Ok(file) => {
                    hooks.events.pre_tool_use.extend(file.hooks.pre_tool_use);
                    hooks.events.post_tool_use.extend(file.hooks.post_tool_use);
                    hooks.sources.push(path.display().to_string());
                }

                Err(error) => eprintln!("warning: {} was skipped: {error}", path.display()),
            }
        }

        hooks
    }

    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let file: File = serde_json::from_str(text)?;

        Ok(Self {
            events: file.hooks,
            sources: Vec::new(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// How many commands there are.
    pub fn count(&self) -> usize {
        self.events
            .pre_tool_use
            .iter()
            .chain(&self.events.post_tool_use)
            .map(|matcher| matcher.hooks.len())
            .sum()
    }

    /// Runs the commands of `matchers` for `tool`, in order, until one
    /// exits with 2. Returns what that one said.
    async fn run(
        matchers: &[Matcher],
        tool: &str,
        input: &Value,
        workspace: &Workspace,
    ) -> Option<String> {
        let commands = matchers
            .iter()
            .filter(|matcher| matcher.matches(tool))
            .flat_map(|matcher| &matcher.hooks);

        let payload = serde_json::to_vec(input).unwrap_or_default();

        for hook in commands {
            let timeout = Duration::from_secs(hook.timeout.unwrap_or(DEFAULT_TIMEOUT).max(1));

            let output = process::run_shell_in_with_input(
                workspace,
                &hook.command,
                timeout,
                payload.clone(),
            )
            .await;

            let Ok(output) = output else {
                continue;
            };

            if output.exit_code == Some(2) {
                let said = match output.stderr.trim() {
                    "" => output.stdout.trim(),
                    errors => errors,
                };

                let said = match said {
                    "" => format!("a hook (`{}`) stopped this", hook.command),
                    said => said.to_string(),
                };

                return Some(said);
            }
        }

        None
    }
}

#[async_trait]
impl ToolHooks for CommandHooks {
    async fn before(&self, call: &ToolCall, workspace: &Workspace) -> Option<String> {
        if self.events.pre_tool_use.is_empty() {
            return None;
        }

        let input = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": call.name,
            "tool_input": call.input,
            "cwd": workspace.root(),
        });

        Self::run(&self.events.pre_tool_use, &call.name, &input, workspace).await
    }

    async fn after(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        workspace: &Workspace,
    ) -> Option<String> {
        if self.events.post_tool_use.is_empty() {
            return None;
        }

        let input = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": call.name,
            "tool_input": call.input,
            "tool_response": { "output": result.output, "is_error": result.is_error },
            "cwd": workspace.root(),
        });

        Self::run(&self.events.post_tool_use, &call.name, &input, workspace).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Sandbox;

    fn call(tool: &str) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            name: tool.to_string(),
            input: json!({ "command": "rm -rf build" }),
        }
    }

    fn workspace(root: &Path) -> Workspace {
        Workspace::new(root).unwrap().with_sandbox(Sandbox::Off)
    }

    #[tokio::test]
    async fn a_pre_hook_that_exits_2_stops_the_call_with_its_reason() {
        let project = tempfile::tempdir().unwrap();

        let hooks = CommandHooks::from_json(
            r#"{ "hooks": { "PreToolUse": [
                { "matcher": "shell", "hooks": [{ "type": "command",
                  "command": "grep -q 'rm -rf' && echo 'no recursive deletes' >&2 && exit 2; exit 0" }] }
            ] } }"#,
        )
        .unwrap();

        let workspace = workspace(project.path());

        let stopped = hooks.before(&call("shell"), &workspace).await;
        assert_eq!(stopped.as_deref(), Some("no recursive deletes"));

        // Another tool does not match.
        assert!(hooks.before(&call("read_file"), &workspace).await.is_none());
    }

    #[tokio::test]
    async fn a_post_hook_sees_the_result_and_can_speak() {
        let project = tempfile::tempdir().unwrap();

        let hooks = CommandHooks::from_json(
            r#"{ "hooks": { "PostToolUse": [
                { "matcher": "*", "hooks": [{ "type": "command",
                  "command": "grep -q '\"is_error\":true' && echo 'remember the lint' >&2 && exit 2; exit 0" }] }
            ] } }"#,
        )
        .unwrap();

        let workspace = workspace(project.path());

        let failed = ToolResult {
            call_id: "1".to_string(),
            output: "boom".to_string(),
            is_error: true,
            images: Vec::new(),
        };

        let said = hooks.after(&call("shell"), &failed, &workspace).await;
        assert_eq!(said.as_deref(), Some("remember the lint"));

        let passed = ToolResult {
            is_error: false,
            ..failed
        };

        assert!(
            hooks
                .after(&call("shell"), &passed, &workspace)
                .await
                .is_none()
        );
    }

    #[test]
    fn files_are_read_from_the_project_and_home() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        for folder in [project.path(), home.path()] {
            std::fs::create_dir_all(folder.join(".cortex")).unwrap();
            std::fs::write(
                folder.join(".cortex/hooks.json"),
                r#"{ "hooks": { "PostToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": "true" }] }] } }"#,
            )
            .unwrap();
        }

        let hooks = CommandHooks::load(project.path(), Some(home.path()));

        assert_eq!(hooks.count(), 2);
        assert_eq!(hooks.sources.len(), 2);
    }
}
