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
use tokio::sync::RwLock;
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
/// Inner is behind an RwLock so a background loader can publish servers
/// after the HTTP/WS server has already started accepting connections.
#[derive(Clone)]
pub struct McpManager {
    inner: Arc<RwLock<Inner>>,
}

impl McpManager {
    /// An empty manager — no servers, no tools. Used when there is no config or
    /// it fails to load.
    pub fn empty() -> Self {
        McpManager {
            inner: Arc::new(RwLock::new(Inner {
                servers: HashMap::new(),
                routes: HashMap::new(),
                schemas: Vec::new(),
            })),
        }
    }

    /// Return an immediately-usable (empty) manager and connect to configured
    /// servers in the background. Never blocks the caller.
    pub fn load_in_background(workspace_root: String) -> Self {
        let mgr = Self::empty();
        let target = mgr.clone();
        tokio::spawn(async move {
            let loaded = Self::connect_all(&workspace_root).await;
            let mut guard = target.inner.write().await;
            *guard = loaded;
            info!(
                "MCP: ready — {} server(s), {} tool(s)",
                guard.servers.len(),
                guard.routes.len()
            );
        });
        mgr
    }

    /// Connect to all enabled MCP servers (concurrently) and return a populated
    /// `Inner`. This is the body that was previously in `load`.
    async fn connect_all(workspace_root: &str) -> Inner {
        let empty = || Inner { servers: HashMap::new(), routes: HashMap::new(), schemas: Vec::new() };

        let Some(path) = find_config(workspace_root) else {
            info!("MCP: no mcp.json found — skipping (Bow native tools still available)");
            return empty();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => { warn!("MCP: could not read {:?}: {} — skipping", path, e); return empty(); }
        };
        let cfg: McpConfigFile = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => { warn!("MCP: {:?} is not valid JSON: {} — skipping", path, e); return empty(); }
        };
        info!("MCP: loading {} server(s) from {:?}", cfg.servers.len(), path);

        // Connect to all enabled servers concurrently.
        let connects = cfg.servers.into_iter().filter(|(_, sc)| !sc.disabled).map(|(name, sc)| async move {
            match tokio::time::timeout(SERVER_INIT_TIMEOUT, connect_server(&sc)).await {
                Ok(Ok((client, tools))) => Some((name, client, tools)),
                Ok(Err(e)) => { warn!("MCP: '{}' failed to start: {} — skipping", name, e); None }
                Err(_) => { warn!("MCP: '{}' timed out after {:?} — skipping", name, SERVER_INIT_TIMEOUT); None }
            }
        });
        let results = futures_util::future::join_all(connects).await;

        let mut servers: HashMap<String, Client> = HashMap::new();
        let mut routes: HashMap<String, (String, String)> = HashMap::new();
        let mut schemas: Vec<Value> = Vec::new();
        for (name, client, tools) in results.into_iter().flatten() {
            let server_key = sanitize(&name);
            let mut count = 0;
            for tool in tools {
                let real = tool.name.to_string();
                let exposed = unique_exposed_name(&server_key, &real, &routes);
                let description = tool.description.as_deref().unwrap_or("").to_string();
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
        Inner { servers, routes, schemas }
    }

    /// OpenAI-shaped tool schemas for all currently-connected MCP tools.
    pub fn schemas(&self) -> Vec<Value> {
        self.inner.try_read().map(|g| g.schemas.clone()).unwrap_or_default()
    }

    /// Whether `name` is a tool provided by an MCP server.
    pub fn is_mcp_tool(&self, name: &str) -> bool {
        self.inner.try_read().map(|g| g.routes.contains_key(name)).unwrap_or(false)
    }

    /// Call an MCP tool by its exposed (namespaced) name. Returns the tool's
    /// text content as a JSON string value, matching Bow's native tool outputs.
    pub async fn dispatch(&self, exposed_name: &str, input: &Value) -> Result<Value> {
        // Take a read guard, clone out the route/client info, then drop the guard
        // before awaiting the MCP call (avoid holding guard across .await).
        let (server_key, real) = {
            let guard = self.inner.read().await;
            guard
                .routes
                .get(exposed_name)
                .map(|(sk, rn)| (sk.clone(), rn.clone()))
                .ok_or_else(|| anyhow::anyhow!("Unknown MCP tool: {}", exposed_name))?
        };

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

        // Re-acquire to get the client reference — we clone only what we need.
        // Because `Client` (RunningService) is not Clone, we must hold the guard
        // across the call_tool await. This is safe: read guards don't deadlock
        // each other and no writer runs during normal request handling.
        let guard = self.inner.read().await;
        let client = guard
            .servers
            .get(&server_key)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' is not connected", server_key))?;

        let result = client
            .call_tool(params)
            .await
            .map_err(|e| anyhow::anyhow!("MCP '{}' call failed: {}", exposed_name, e))?;
        drop(guard);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_in_background_returns_immediately_for_missing_config() {
        // A directory with no mcp.json must yield an immediately-usable manager.
        let dir = std::env::temp_dir().join("bow_mcp_test_none");
        std::fs::create_dir_all(&dir).unwrap();
        let start = std::time::Instant::now();
        let mgr = McpManager::load_in_background(dir.to_string_lossy().to_string());
        // Must not block on any network/process work.
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
        // Empty until/unless servers load; safe to query.
        assert!(mgr.schemas().is_empty());
        assert!(!mgr.is_mcp_tool("anything"));
    }
}
