//! MCP (Model Context Protocol) client.
//!
//! Bow can spawn external MCP servers as stdio child processes, discover their
//! tools at startup, and expose them to the LLM alongside Bow's native tools.
//! This lets Bow plug into the entire MCP ecosystem (filesystem, GitHub, git,
//! Playwright, SQLite, …) without hand-writing each integration.
//!
//! Configuration lives in `mcp.json` (Claude-Desktop-compatible schema) next to
//! `.env` or in the workspace root:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "filesystem": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:\\AI\\workspace"],
//!       "env": {},
//!       "disabled": false
//!     }
//!   }
//! }
//! ```
//!
//! Exposed tool names are namespaced `mcp__<server>__<tool>` so they never
//! collide with Bow's native tools or with each other.

use anyhow::Result;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait for a single MCP server to start up and list its tools
/// before giving up on it.
const SERVER_INIT_TIMEOUT: Duration = Duration::from_secs(60);

// ── Config schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct McpConfigFile {
    #[serde(rename = "mcpServers", default)]
    servers: HashMap<String, ServerConfig>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    disabled: bool,
}

// ── Manager ────────────────────────────────────────────────────────────────────

type Client = RunningService<RoleClient, ()>;

struct Inner {
    /// server_key → running client (owns the child process; keep alive for the
    /// lifetime of the app).
    servers: HashMap<String, Client>,
    /// exposed tool name → (server_key, real tool name).
    routes: HashMap<String, (String, String)>,
    /// OpenAI-shaped tool schemas with the exposed (namespaced) names.
    schemas: Vec<Value>,
}

/// Cheaply-cloneable handle to the set of connected MCP servers.
#[derive(Clone)]
pub struct McpManager {
    inner: Arc<Inner>,
}

impl McpManager {
    /// An empty manager — no servers, no tools. Used when there is no config or
    /// it fails to load.
    pub fn empty() -> Self {
        McpManager {
            inner: Arc::new(Inner {
                servers: HashMap::new(),
                routes: HashMap::new(),
                schemas: Vec::new(),
            }),
        }
    }

    /// Load `mcp.json` (searching next to the executable, the dev project dir,
    /// and the workspace root) and connect to every enabled server. Best-effort:
    /// a server that fails to start is logged and skipped; the rest still load.
    pub async fn load(workspace_root: &str) -> Self {
        let Some(path) = find_config(workspace_root) else {
            info!("MCP: no mcp.json found — skipping (Bow native tools still available)");
            return Self::empty();
        };

        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                warn!("MCP: could not read {:?}: {} — skipping", path, e);
                return Self::empty();
            }
        };
        let cfg: McpConfigFile = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                warn!("MCP: {:?} is not valid JSON: {} — skipping", path, e);
                return Self::empty();
            }
        };

        info!("MCP: loading {} server(s) from {:?}", cfg.servers.len(), path);

        let mut servers: HashMap<String, Client> = HashMap::new();
        let mut routes: HashMap<String, (String, String)> = HashMap::new();
        let mut schemas: Vec<Value> = Vec::new();

        for (name, sc) in cfg.servers {
            if sc.disabled {
                info!("MCP: '{}' is disabled — skipping", name);
                continue;
            }
            match tokio::time::timeout(SERVER_INIT_TIMEOUT, connect_server(&sc)).await {
                Ok(Ok((client, tools))) => {
                    let server_key = sanitize(&name);
                    let mut count = 0;
                    for tool in tools {
                        let real = tool.name.to_string();
                        let exposed = unique_exposed_name(&server_key, &real, &routes);
                        let description = tool
                            .description
                            .as_deref()
                            .unwrap_or("")
                            .to_string();
                        // input_schema is Arc<Map<String,Value>>
                        let input_schema = Value::Object((*tool.input_schema).clone());
                        schemas.push(json!({
                            "name": exposed,
                            "description": format!("[{}] {}", name, description),
                            "input_schema": input_schema,
                        }));
                        routes.insert(exposed, (server_key.clone(), real));
                        count += 1;
                    }
                    info!("MCP: '{}' connected — {} tool(s)", name, count);
                    servers.insert(server_key, client);
                }
                Ok(Err(e)) => warn!("MCP: '{}' failed to start: {} — skipping", name, e),
                Err(_) => warn!("MCP: '{}' timed out after {:?} — skipping", name, SERVER_INIT_TIMEOUT),
            }
        }

        info!("MCP: ready — {} server(s), {} tool(s)", servers.len(), routes.len());
        McpManager {
            inner: Arc::new(Inner { servers, routes, schemas }),
        }
    }

    /// OpenAI-shaped tool schemas for all connected MCP tools.
    pub fn schemas(&self) -> &[Value] {
        &self.inner.schemas
    }

    /// Whether `name` is a tool provided by an MCP server.
    pub fn is_mcp_tool(&self, name: &str) -> bool {
        self.inner.routes.contains_key(name)
    }

    /// Call an MCP tool by its exposed (namespaced) name. Returns the tool's
    /// text content as a JSON string value, matching Bow's native tool outputs.
    pub async fn dispatch(&self, exposed_name: &str, input: &Value) -> Result<Value> {
        let (server_key, real) = self
            .inner
            .routes
            .get(exposed_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP tool: {}", exposed_name))?;

        let client = self
            .inner
            .servers
            .get(server_key)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' is not connected", server_key))?;

        let arguments: Map<String, Value> = match input {
            Value::Object(m) => m.clone(),
            Value::Null => Map::new(),
            other => {
                let mut m = Map::new();
                m.insert("value".to_string(), other.clone());
                m
            }
        };

        let params = CallToolRequestParams::new(real.clone()).with_arguments(arguments);
        let result = client
            .call_tool(params)
            .await
            .map_err(|e| anyhow::anyhow!("MCP '{}' call failed: {}", exposed_name, e))?;

        // Flatten content blocks to text; prefer structured_content if present.
        let mut text = String::new();
        for block in &result.content {
            if let Some(t) = block.as_text() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t.text);
            }
        }
        if text.is_empty() {
            if let Some(sc) = &result.structured_content {
                text = sc.to_string();
            }
        }
        if text.is_empty() {
            text = "(no content)".to_string();
        }

        if result.is_error.unwrap_or(false) {
            return Err(anyhow::anyhow!("{}", text));
        }
        Ok(json!(text))
    }
}

// ── Connection ──────────────────────────────────────────────────────────────────

async fn connect_server(sc: &ServerConfig) -> Result<(Client, Vec<rmcp::model::Tool>)> {
    let mut command = build_command(&sc.command, &sc.args);
    for (k, v) in &sc.env {
        command.env(k, v);
    }

    let transport = TokioChildProcess::new(command)
        .map_err(|e| anyhow::anyhow!("spawn '{}' failed: {}", sc.command, e))?;

    let client = ()
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP handshake failed: {}", e))?;

    let tools = client
        .list_all_tools()
        .await
        .map_err(|e| anyhow::anyhow!("list_tools failed: {}", e))?;

    Ok((client, tools))
}

/// Build the child-process command.
///
/// On Windows, `CreateProcess` (used by std/tokio) does not consult `PATHEXT`,
/// so a bare `npx` (really `npx.cmd`) won't resolve. We route such commands
/// through `cmd /C` unless the caller gave an explicit `.exe`/`.bat`/`.cmd` or a
/// path, which is the standard fix for npm/uvx-based MCP servers on Windows.
#[cfg(windows)]
fn build_command(command: &str, args: &[String]) -> tokio::process::Command {
    let lower = command.to_lowercase();
    let is_direct = lower.ends_with(".exe")
        || lower.ends_with(".cmd")
        || lower.ends_with(".bat")
        || command.contains('\\')
        || command.contains('/');
    if is_direct {
        let mut c = tokio::process::Command::new(command);
        c.args(args);
        c
    } else {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command).args(args);
        c
    }
}

#[cfg(not(windows))]
fn build_command(command: &str, args: &[String]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(command);
    c.args(args);
    c
}

// ── Naming helpers ──────────────────────────────────────────────────────────────

/// Keep only characters valid in an OpenAI tool name (`[a-zA-Z0-9_-]`).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

/// Build `mcp__<server>__<tool>`, sanitized and capped at 64 chars, ensuring it
/// does not already exist in `routes` (suffixing a counter if needed).
fn unique_exposed_name(
    server_key: &str,
    real_tool: &str,
    routes: &HashMap<String, (String, String)>,
) -> String {
    let base = format!("mcp__{}__{}", server_key, sanitize(real_tool));
    let base = if base.len() > 64 {
        base.chars().take(64).collect::<String>()
    } else {
        base
    };
    if !routes.contains_key(&base) {
        return base;
    }
    for i in 1.. {
        let candidate = format!("{}_{}", base.chars().take(60).collect::<String>(), i);
        if !routes.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

// ── Config discovery ─────────────────────────────────────────────────────────────

fn find_config(workspace_root: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("mcp.json"));
        }
    }
    candidates.push(PathBuf::from(r"C:\AI\agent Bow\desktop\mcp.json"));
    candidates.push(PathBuf::from(workspace_root).join("mcp.json"));
    candidates.push(PathBuf::from("mcp.json"));

    candidates.into_iter().find(|p| p.exists())
}
