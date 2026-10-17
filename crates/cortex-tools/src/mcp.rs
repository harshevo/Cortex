//! Tools from MCP servers.
//!
//! The Model Context Protocol lets a separate program offer tools. The
//! servers to start are listed in `~/.cortex/mcp.json`, in the format
//! other clients use:
//!
//! ```json
//! { "mcpServers": { "docs": { "command": "npx", "args": ["-y", "some-mcp-server"], "env": {} } } }
//! ```
//!
//! Only the person's own file is read. A file in a project would start
//! programs as soon as the project was opened, before anyone asked for
//! anything, so projects cannot add servers.
//!
//! Each server is started when the agent is built and spoken to over its
//! standard input and output. Its tools are named `mcp__<server>__<tool>`.
//! A tool the server marks as read-only runs without asking; every other
//! one asks for approval, like the shell.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{
    Access, Image, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

const PROTOCOL_VERSION: &str = "2025-06-18";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    command: Option<String>,

    #[serde(default)]
    args: Vec<String>,

    #[serde(default)]
    env: BTreeMap<String, String>,

    #[serde(default)]
    disabled: bool,
}

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A running server.
struct Server {
    name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,

    /// Held so the program is stopped when the last tool lets go.
    _child: Child,
}

impl Server {
    async fn start(name: &str, config: &ServerConfig, root: &Path) -> Result<Arc<Self>, String> {
        let program = config
            .command
            .as_deref()
            .ok_or("only servers started with a `command` are supported")?;

        let mut child = Command::new(program)
            .args(&config.args)
            .envs(&config.env)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start `{program}`: {error}"))?;

        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no input")?));
        let stdout = child.stdout.take().ok_or("no output")?;

        let pending: Pending = Arc::default();

        tokio::spawn(read_messages(stdout, stdin.clone(), pending.clone()));

        let server = Arc::new(Self {
            name: name.to_string(),
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            _child: child,
        });

        server
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "cortex", "version": env!("CARGO_PKG_VERSION") },
                }),
                START_TIMEOUT,
            )
            .await?;

        server
            .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await?;

        Ok(server)
    }

    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');

        let mut stdin = self.stdin.lock().await;

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("the server stopped: {error}"))?;

        stdin.flush().await.map_err(|error| error.to_string())
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();

        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, reply);

        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;

        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("the server stopped".to_string()),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);

                Err(format!(
                    "no answer to `{method}` within {} seconds",
                    timeout.as_secs()
                ))
            }
        }
    }

    async fn tools(&self) -> Result<Vec<Value>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        // A server lists its tools a page at a time.
        for _ in 0..20 {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };

            let page = self.request("tools/list", params, START_TIMEOUT).await?;

            if let Some(listed) = page.get("tools").and_then(Value::as_array) {
                tools.extend(listed.iter().cloned());
            }

            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);

            if cursor.is_none() {
                break;
            }
        }

        Ok(tools)
    }
}

/// Hands each answer to whoever is waiting for it, and answers the few
/// requests a server may make of its client.
async fn read_messages(
    stdout: tokio::process::ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
) {
    let mut lines = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);

        match (id, method) {
            // An answer to one of ours.
            (Some(id), None) => {
                let Some(id) = id.as_u64() else { continue };

                let waiting = pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);

                if let Some(waiting) = waiting {
                    let result = match message.get("error") {
                        Some(error) => Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("the server reported an error")
                            .to_string()),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };

                    let _ = waiting.send(result);
                }
            }

            // A request of the server's own.
            (Some(id), Some(method)) => {
                let reply = match method {
                    "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                    _ => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "not supported by this client" },
                    }),
                };

                let mut line = reply.to_string();
                line.push('\n');

                let mut stdin = stdin.lock().await;
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.flush().await;
            }

            // A notification; nothing here needs one.
            _ => {}
        }
    }

    // The server has gone: nobody will be answered.
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

struct McpTool {
    server: Arc<Server>,
    tool: String,
    spec: ToolSpec,
    read_only: bool,
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn access(&self) -> Access {
        match self.read_only {
            true => Access::ReadOnly,
            false => Access::Mutating,
        }
    }

    fn describe(&self, input: &Value) -> String {
        let input = input.to_string();
        let shown: String = input.chars().take(300).collect();

        format!(
            "{} on MCP server `{}`: {shown}",
            self.tool, self.server.name
        )
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        self.run_with_images(input, context)
            .await
            .map(|output| output.text)
    }

    async fn run_with_images(
        &self,
        input: Value,
        _context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let result = self
            .server
            .request(
                "tools/call",
                json!({ "name": self.tool, "arguments": input }),
                CALL_TIMEOUT,
            )
            .await
            .map_err(ToolError::Failed)?;

        let output = content(&result);

        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(ToolError::Failed(output.text));
        }

        Ok(output)
    }
}

/// The text and pictures of a tool's answer.
fn content(result: &Value) -> ToolOutput {
    let mut output = ToolOutput::default();
    let mut parts = Vec::new();

    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                parts.push(
                    item.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }

            Some("image") => {
                if let (Some(data), Some(media_type)) = (
                    item.get("data").and_then(Value::as_str),
                    item.get("mimeType").and_then(Value::as_str),
                ) {
                    output.images.push(Image {
                        media_type: media_type.to_string(),
                        data: data.to_string(),
                    });
                }
            }

            Some("resource") => {
                if let Some(text) = item.pointer("/resource/text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }

            _ => {}
        }
    }

    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        parts.push(structured.to_string());
    }

    output.text = parts.join("\n");

    if output.text.is_empty() && output.images.is_empty() {
        output.text = "(no output)".to_string();
    }

    output
}

/// A tool name every provider accepts: letters, digits, `_` and `-`, at
/// most 64 characters.
fn tool_name(server: &str, tool: &str) -> String {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };

    let mut name = format!("mcp__{}__{}", clean(server), clean(tool));

    name.truncate(64);

    name
}

/// Starts the servers in `~/.cortex/mcp.json` and adds their tools.
/// Returns a line per server saying how it went.
pub async fn add_mcp_tools(registry: &mut ToolRegistry, home: &Path, root: &Path) -> Vec<String> {
    let path = home.join(".cortex").join("mcp.json");

    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };

    let config: ConfigFile = match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(error) => return vec![format!("{} was skipped: {error}", path.display())],
    };

    let starting = config
        .servers
        .into_iter()
        .filter(|(_, server)| !server.disabled)
        .map(|(name, server)| {
            let root = root.to_path_buf();

            async move {
                let started = Server::start(&name, &server, &root).await;

                let tools = match &started {
                    Ok(server) => server.tools().await,
                    Err(error) => Err(error.clone()),
                };

                (name, started, tools)
            }
        });

    let mut report = Vec::new();

    for (name, started, tools) in join_all(starting).await {
        match (started, tools) {
            (Ok(server), Ok(tools)) => {
                let count = tools.len();

                for tool in tools {
                    let Some(tool_id) = tool.get("name").and_then(Value::as_str) else {
                        continue;
                    };

                    let read_only = tool
                        .pointer("/annotations/readOnlyHint")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);

                    let spec = ToolSpec {
                        name: tool_name(&name, tool_id),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("A tool from an MCP server.")
                            .to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    };

                    registry.register(McpTool {
                        server: server.clone(),
                        tool: tool_id.to_string(),
                        spec,
                        read_only,
                    });
                }

                report.push(format!("MCP server `{name}`: {count} tools"));
            }

            (Err(error), _) | (_, Err(error)) => {
                report.push(format!("MCP server `{name}` was not started: {error}"));
            }
        }
    }

    report
}

/// Waits for every future, in the order given.
async fn join_all<F>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futures.into_iter().map(tokio::spawn).collect();

    let mut results = Vec::with_capacity(handles.len());

    for handle in handles {
        if let Ok(result) = handle.await {
            results.push(result);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_for_every_provider() {
        assert_eq!(tool_name("my docs", "search.v2"), "mcp__my_docs__search_v2");
        assert!(tool_name("s", &"x".repeat(100)).len() <= 64);
    }

    #[test]
    fn text_and_pictures_are_read_from_an_answer() {
        let output = content(&json!({
            "content": [
                { "type": "text", "text": "one" },
                { "type": "image", "data": "AAAA", "mimeType": "image/png" },
                { "type": "text", "text": "two" },
            ],
        }));

        assert_eq!(output.text, "one\ntwo");
        assert_eq!(output.images.len(), 1);
    }

    /// A server written in a few lines of shell: it answers `initialize`,
    /// lists one tool, and echoes the call back.
    #[tokio::test]
    async fn a_server_is_started_and_its_tool_called() {
        if cfg!(windows)
            || std::process::Command::new("python3")
                .arg("-V")
                .output()
                .is_err()
        {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();

        let script = home.path().join("server.py");

        std::fs::write(
            &script,
            r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": True}}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "said " + m["params"]["arguments"]["text"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#,
        )
        .unwrap();

        std::fs::create_dir_all(home.path().join(".cortex")).unwrap();
        std::fs::write(
            home.path().join(".cortex/mcp.json"),
            json!({ "mcpServers": { "test": { "command": "python3", "args": [script] } } })
                .to_string(),
        )
        .unwrap();

        let mut registry = ToolRegistry::new();

        let report = add_mcp_tools(&mut registry, home.path(), project.path()).await;

        assert_eq!(report, vec!["MCP server `test`: 1 tools".to_string()]);
        assert!(!registry.is_mutating("mcp__test__echo"));

        let context = ToolContext {
            workspace: cortex_core::Workspace::new(project.path()).unwrap(),
            approver: Arc::new(cortex_core::AutoApprove),
        };

        let result = registry
            .dispatch(
                &cortex_core::ToolCall {
                    id: "1".to_string(),
                    name: "mcp__test__echo".to_string(),
                    input: json!({ "text": "hi" }),
                },
                &context,
            )
            .await;

        assert_eq!(result.output, "said hi");
        assert!(!result.is_error);
    }
}
