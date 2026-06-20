# Bow Image Studio — Phase 1: Backend Host Swap — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert Bow from a Tauri desktop app + browser extension into a standalone Rust server that serves a web UI and the existing WebSocket protocol on a single localhost port, runs from a system tray, launches via a root `.bat`, and removes the extension — folding in the MCP startup-blocking regression fix.

**Architecture:** Replace the Tauri host with a plain binary that runs a tokio runtime hosting an `axum` server (static SPA + REST + WebSocket upgrade, all on port 9357) and a `tao` event loop driving a `tray-icon`. The existing agent/WS protocol in `server.rs` is preserved but moved onto axum's WebSocket extractor. MCP servers load in a background task so the server accepts connections immediately.

**Tech Stack:** Rust, tokio, axum 0.7 (+ ws), tower-http (fs), tray-icon, tao, existing rmcp/reqwest/image stack. Frontend stays React+Vite (a placeholder SPA in this phase; full UI is Phase 2).

## Global Constraints

- Platform: Windows 11 (primary). All paths and launchers target Windows.
- Bind address: `127.0.0.1` only. Never bind `0.0.0.0`.
- Single port for everything: `config.ws_port` (default `9357`) serves static SPA, REST, and WS.
- Local LLM only — no Anthropic/cloud API calls (project rule).
- Fix all compiler warnings, not just errors (project rule).
- Do not auto-commit beyond the per-task commits in this plan; no pushing.
- Keep the existing WS JSON protocol message shapes unchanged (`auth`, `auth_ok`, `auth_error`, `user_message`, `page_context`, `interrupt`, `ping`, agent events) so Phase 2's UI can speak the same protocol.

---

### Task 1: MCP load no longer blocks the accept loop

**Files:**
- Modify: `desktop/src-tauri/src/server.rs:31-69` (the `start` function)
- Modify: `desktop/src-tauri/src/tools/mcp.rs` (make per-server connect concurrent inside `load`)
- Test: `desktop/src-tauri/src/tools/mcp.rs` (inline `#[cfg(test)]` module)

**Interfaces:**
- Consumes: existing `McpManager::load(workspace_root: &str) -> McpManager`, `McpManager::empty()`.
- Produces: `McpManager` becomes constructible as a shared, lazily-populated handle:
  - `McpManager::empty() -> McpManager` (unchanged signature; now also the initial value while loading).
  - `McpManager::load_in_background(workspace_root: String) -> McpManager` — returns an empty-but-shared handle immediately and fills it in via a spawned task; `schemas()`, `is_mcp_tool()`, `dispatch()` operate on whatever is loaded so far.

- [ ] **Step 1: Write the failing test**

Add to the bottom of `desktop/src-tauri/src/tools/mcp.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bow-desktop load_in_background_returns_immediately --lib`
Expected: FAIL — `no function or associated item named 'load_in_background'`.

- [ ] **Step 3: Make the manager's inner state shared/swappable and add the background loader**

In `mcp.rs`, change `McpManager` to hold a swappable inner so a background task can publish results. Replace the `struct McpManager { inner: Arc<Inner> }` definition and `empty()`/`load()` with:

```rust
use tokio::sync::RwLock;

/// Cheaply-cloneable handle to the set of connected MCP servers.
/// Inner is behind an RwLock so a background loader can publish servers
/// after the HTTP/WS server has already started accepting connections.
#[derive(Clone)]
pub struct McpManager {
    inner: Arc<RwLock<Inner>>,
}

impl McpManager {
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
}
```

- [ ] **Step 4: Convert the old `load` body into `connect_all` returning `Inner`**

Rename the existing `load`'s body to a private `async fn connect_all(workspace_root: &str) -> Inner` that builds and returns the `Inner { servers, routes, schemas }` value (the same logic currently in `load`, but returning `Inner` instead of `McpManager`, and returning `Inner { servers: HashMap::new(), routes: HashMap::new(), schemas: Vec::new() }` on every early-return/skip path instead of `Self::empty()`). Keep the per-server connect concurrent by collecting futures and awaiting them with `futures_util::future::join_all`:

```rust
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
```

- [ ] **Step 5: Update the accessor methods to read through the RwLock**

`schemas()` currently returns `&[Value]`; with an `RwLock` it must return owned data. Change the three accessors:

```rust
/// OpenAI-shaped tool schemas for all currently-connected MCP tools.
pub fn schemas(&self) -> Vec<Value> {
    self.inner.try_read().map(|g| g.schemas.clone()).unwrap_or_default()
}

pub fn is_mcp_tool(&self, name: &str) -> bool {
    self.inner.try_read().map(|g| g.routes.contains_key(name)).unwrap_or(false)
}
```

For `dispatch`, take a read guard at call time and resolve the route/client before awaiting the call (clone the `server_key`/real name out of the guard, drop the guard, then dispatch). Update its body to read `let guard = self.inner.read().await;` and look up in `guard.routes` / `guard.servers`.

- [ ] **Step 6: Update call sites for the new signatures**

In `desktop/src-tauri/src/server.rs:52`, replace:

```rust
    let mcp = crate::tools::mcp::McpManager::load(
        &config.workspace_root.to_string_lossy(),
    ).await;
```

with:

```rust
    let mcp = crate::tools::mcp::McpManager::load_in_background(
        config.workspace_root.to_string_lossy().to_string(),
    );
```

In `desktop/src-tauri/src/local_llm.rs:264`, `openai_tool_schemas(mcp.schemas())` now receives a `Vec<Value>`; change `openai_tool_schemas(mcp_schemas: &[Value])` call to pass `&mcp.schemas()` (bind to a local first to satisfy the borrow): 

```rust
    let mcp_schemas = mcp.schemas();
    let tools = openai_tool_schemas(&mcp_schemas);
```

- [ ] **Step 7: Run the test and the build**

Run: `cargo test -p bow-desktop load_in_background_returns_immediately --lib`
Expected: PASS.
Run: `cargo build -p bow-desktop`
Expected: builds with no warnings.

- [ ] **Step 8: Commit**

```bash
git add desktop/src-tauri/src/tools/mcp.rs desktop/src-tauri/src/server.rs desktop/src-tauri/src/local_llm.rs
git commit -m "fix: load MCP servers in background so WS server accepts immediately"
```

---

### Task 2: Add axum HTTP server module (static SPA + health + config), reusing existing WS handler

**Files:**
- Modify: `desktop/src-tauri/Cargo.toml` (add deps)
- Create: `desktop/src-tauri/src/http.rs`
- Modify: `desktop/src-tauri/src/server.rs` (expose `handle_socket` usable from axum; see Task 3)
- Modify: `desktop/src-tauri/src/lib.rs` (add `mod http;`)
- Test: `desktop/src-tauri/src/http.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `http::build_router(state: AppState, mcp: McpManager, web_dir: PathBuf) -> axum::Router`
  - REST: `GET /api/health` → `200 "ok"`; `GET /api/config` → `{"ws_port": u16}` (loopback-only metadata for the SPA).
  - Static: any non-`/api`, non-`/ws` path served from `web_dir` with SPA fallback to `index.html`.
  - WS route `GET /ws` (wired in Task 3).

- [ ] **Step 1: Add dependencies**

Run from `desktop/src-tauri/`:

```bash
cargo add axum@0.7 --features ws
cargo add tower-http@0.6 --features fs,trace
cargo add tower@0.5
```

Verify `Cargo.toml` now lists `axum`, `tower-http`, `tower`.

- [ ] **Step 2: Write the failing test**

Create `desktop/src-tauri/src/http.rs` with only:

```rust
use axum::Router;
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    fn test_router() -> Router {
        // health/config routes do not depend on AppState, so build a minimal router.
        Router::new()
            .route("/api/health", axum::routing::get(|| async { "ok" }))
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_router();
        let res = app
            .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    fn _unused(_d: PathBuf) {}
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p bow-desktop health_returns_ok --lib`
Expected: FAIL — `http` module not declared / unresolved import, or compile error.

- [ ] **Step 4: Declare the module and implement the router**

Add `mod http;` to `desktop/src-tauri/src/lib.rs` (after `mod server;`). Replace the body of `http.rs` (keep the test module) with:

```rust
use crate::state::AppState;
use crate::tools::mcp::McpManager;
use axum::{routing::get, Json, Router};
use serde_json::json;
use std::path::PathBuf;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct HttpState {
    pub app: AppState,
    pub mcp: McpManager,
}

pub fn build_router(state: AppState, mcp: McpManager, web_dir: PathBuf) -> Router {
    let ws_port = state.config.ws_port;
    let http_state = HttpState { app: state, mcp };

    let index = web_dir.join("index.html");
    let static_service = ServeDir::new(&web_dir)
        .not_found_service(tower_http::services::ServeFile::new(index));

    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/config", get(move || async move { Json(json!({ "ws_port": ws_port })) }))
        // /ws is added in Task 3.
        .fallback_service(static_service)
        .with_state(http_state)
}
```

Add `cargo add tower-http@0.6 --features fs` already covers `ServeFile`/`ServeDir`. Remove the `_unused` helper and the `PathBuf`-only test import; update the test's `test_router` to call the real `build_router` is deferred to Task 3 (needs AppState); for now keep the minimal `/api/health` test router.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p bow-desktop health_returns_ok --lib`
Expected: PASS.
Run: `cargo build -p bow-desktop`
Expected: builds (warnings about unused `HttpState` fields are acceptable until Task 3 wires the WS route; if any remain after Task 3, fix them).

- [ ] **Step 6: Commit**

```bash
git add desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock desktop/src-tauri/src/http.rs desktop/src-tauri/src/lib.rs
git commit -m "feat: add axum http router serving static SPA, health, and config"
```

---

### Task 3: Move the WebSocket protocol onto axum's WS extractor

**Files:**
- Modify: `desktop/src-tauri/src/server.rs` (refactor `handle_connection` to accept a generic sink/source; add `serve` over axum)
- Modify: `desktop/src-tauri/src/http.rs` (add `/ws` route)
- Test: `desktop/src-tauri/src/server.rs` (inline test for the JSON dispatch helper, see below)

**Interfaces:**
- Consumes: `http::HttpState`, existing `InboundMsg`, `local_llm`, `BrowserBridge`.
- Produces:
  - `server::run_ws(socket: axum::extract::ws::WebSocket, config: Arc<Config>, shell_session: ShellSessionManager, mcp: McpManager)` — drives one client connection over an axum WebSocket, preserving all existing message handling.
  - The old raw `tokio-tungstenite` `start`/`accept_async` loop is deleted (axum owns the listener now).

- [ ] **Step 1: Add the `/ws` route to the router**

In `http.rs` `build_router`, add before `.fallback_service`:

```rust
        .route("/ws", get(ws_upgrade))
```

and add the handler:

```rust
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::Response;
use std::sync::Arc;

async fn ws_upgrade(State(s): State<HttpState>, ws: WebSocketUpgrade) -> Response {
    let config = Arc::new(s.app.config.clone());
    let shell_session = s.app.shell_session.clone();
    let mcp = s.mcp.clone();
    ws.on_upgrade(move |socket| crate::server::run_ws(socket, config, shell_session, mcp))
}
```

- [ ] **Step 2: Refactor `server.rs` message loop to read/write axum `Message`**

In `server.rs`, replace `pub async fn start(...)` and `async fn handle_connection(stream: TcpStream, ...)` with `pub async fn run_ws(socket: axum::extract::ws::WebSocket, ...)`. The body is the existing `handle_connection` logic with three substitutions:
- `use axum::extract::ws::{Message as WsMessage, WebSocket};` (replace the `tokio_tungstenite::tungstenite::Message` import).
- `let (mut ws_sink, mut ws_source) = socket.split();` (axum `WebSocket` implements `Stream`/`Sink`; `futures_util::StreamExt::split` still applies).
- Sending text: `ws_sink.send(WsMessage::Text(text)).await` — axum's `Message::Text` takes a `String` (same as today). Receiving: match `WsMessage::Text(t) => t`, `WsMessage::Close(_) => break`, `_ => continue`. axum yields `Result<Message, axum::Error>`; keep `Some(Ok(m)) => m, _ => break`.

Delete the `TcpListener`/`socket2`/`accept_async` code entirely. Remove now-unused imports (`socket2`, `TcpListener`, `TcpStream`, `tokio_tungstenite`). Run `cargo build` and fix every resulting unused-import warning.

- [ ] **Step 3: Extract the inbound-JSON classifier as a testable pure function**

To get a unit test on this task, factor the early message classification into a pure helper at the top of `server.rs`:

```rust
/// Classify a raw inbound WS text frame before full deserialization.
/// Returns None for control frames the loop should skip (ping/browser_result).
#[derive(Debug, PartialEq)]
pub enum Inbound { Skip, Process }

pub fn classify(raw: &serde_json::Value) -> Inbound {
    match raw["type"].as_str() {
        Some("ping") | Some("browser_result") => Inbound::Skip,
        _ => Inbound::Process,
    }
}
```

Use it in the loop where the `ping`/`browser_result` checks were (keep the `browser_result` pending-resolution block, but gate the "skip" decision through `classify`). Add the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ping_is_skipped() {
        assert_eq!(classify(&json!({"type":"ping"})), Inbound::Skip);
    }
    #[test]
    fn user_message_is_processed() {
        assert_eq!(classify(&json!({"type":"user_message","content":"hi"})), Inbound::Process);
    }
}
```

- [ ] **Step 4: Run the test to verify it fails, then passes**

Run: `cargo test -p bow-desktop classify --lib` (after writing the test but before adding `classify`): Expected FAIL (`classify` not found). After adding `classify`: Expected PASS.

- [ ] **Step 5: Build the whole crate**

Run: `cargo build -p bow-desktop`
Expected: compiles, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add desktop/src-tauri/src/server.rs desktop/src-tauri/src/http.rs
git commit -m "refactor: serve the agent WebSocket over axum instead of raw tungstenite"
```

---

### Task 4: Replace the Tauri host with a plain binary (tokio runtime + axum + tray)

**Files:**
- Modify: `desktop/src-tauri/Cargo.toml` (remove tauri deps, simplify crate-type/bin, add tray deps)
- Delete: `desktop/src-tauri/build.rs` references to tauri (and `tauri.conf.json`), `desktop/src-tauri/src/lib.rs` Tauri code
- Rewrite: `desktop/src-tauri/src/main.rs`
- Create: `desktop/src-tauri/src/host.rs` (tray + runtime wiring)
- Reference: web assets dir resolution (`web_dir`)

**Interfaces:**
- Consumes: `http::build_router`, `Config::from_env`, `AppState::new`, `McpManager::load_in_background`.
- Produces: a runnable `bow-desktop.exe` that binds `127.0.0.1:<ws_port>`, serves the router, shows a tray icon, and opens the browser on first start.

- [ ] **Step 1: Add tray/runtime deps, drop Tauri**

Edit `desktop/src-tauri/Cargo.toml`:
- Remove `tauri`, `tauri-plugin-shell`, and the `[build-dependencies] tauri-build` block.
- Change `[lib] crate-type` to `["rlib"]` and keep `[[bin]] name = "bow-desktop" path = "src/main.rs"`.
- Add:

```bash
cargo add tray-icon@0.19
cargo add tao@0.30
cargo add image@0.25 --features png   # already present; ensure png feature for the tray icon
```

Delete `desktop/src-tauri/build.rs` if it only contains `tauri_build::build()`. Delete `desktop/src-tauri/tauri.conf.json`.

- [ ] **Step 2: Strip Tauri from `lib.rs`, keep config/error helpers**

Replace `desktop/src-tauri/src/lib.rs` with module declarations and the `.env`-failure message box only (no Tauri):

```rust
mod auth;
mod host;
mod http;
mod local_llm;
mod server;
mod state;
mod tools;
mod types;
mod util;

pub use host::run;
```

Move the `.env` failure message-box logic into `host::run` (Step 3).

- [ ] **Step 3: Write `host.rs` — runtime + axum + tray**

Create `desktop/src-tauri/src/host.rs`:

```rust
use crate::state::{AppState, Config};
use std::path::PathBuf;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem}, TrayIconBuilder};
use tracing::info;

/// Resolve the directory holding the built web UI (index.html, assets).
/// Dev: `desktop/webapp/dist` next to the project. Release: `web/` next to the exe.
fn web_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let next = exe.parent().map(|p| p.join("web"));
        if let Some(d) = next { if d.join("index.html").exists() { return d; } }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../webapp/dist")
}

fn fatal_config_box(msg: &str) {
    eprintln!("{}", msg);
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command",
            &format!("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}', 'Bow Error')", msg.replace('\'', "`'"))])
        .spawn();
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("bow_desktop=debug".parse().unwrap()))
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            fatal_config_box(&format!("Bow failed to start:\n\n{}\n\nEdit C:\\AI\\agent Bow\\desktop\\.env and ensure all keys are set.", e));
            std::process::exit(1);
        }
    };
    let ws_port = config.ws_port;
    let workspace = config.workspace_root.to_string_lossy().to_string();
    info!("Bow starting — http://127.0.0.1:{}", ws_port);

    let app_state = AppState::new(config.clone());

    // tokio runtime on a background thread; tao event loop owns the main thread.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server_state = app_state.clone();
    let dir = web_dir();
    rt.spawn(async move {
        let mcp = crate::tools::mcp::McpManager::load_in_background(workspace.clone());
        let router = crate::http::build_router(server_state, mcp, dir);
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], ws_port));
        let listener = tokio::net::TcpListener::bind(addr).await.expect("bind 127.0.0.1");
        info!("HTTP+WS listening on http://{}", addr);
        axum::serve(listener, router).await.expect("axum serve");
    });

    // Open the browser once the server is up.
    let url = format!("http://127.0.0.1:{}", ws_port);
    std::thread::spawn({
        let url = url.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
        }
    });

    // Tray + event loop (main thread).
    let icon = load_tray_icon();
    let menu = Menu::new();
    let open_i = MenuItem::new("Open Bow", true, None);
    let ws_i = MenuItem::new("Open Workspace", true, None);
    let env_i = MenuItem::new("Edit Settings (.env)", true, None);
    let quit_i = MenuItem::new("Quit Bow", true, None);
    menu.append_items(&[&open_i, &PredefinedMenuItem::separator(), &ws_i, &env_i, &PredefinedMenuItem::separator(), &quit_i]).unwrap();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("Bow Image Studio — port {}", ws_port))
        .with_icon(icon)
        .build()
        .expect("tray icon");

    let event_loop = EventLoopBuilder::new().build();
    let menu_channel = MenuEvent::receiver();
    let workspace_path = config.workspace_root.clone();
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Ok(ev) = menu_channel.try_recv() {
            if ev.id == open_i.id() {
                let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
            } else if ev.id == ws_i.id() {
                let _ = std::process::Command::new("explorer.exe").arg(&workspace_path).spawn();
            } else if ev.id == env_i.id() {
                let _ = std::process::Command::new("notepad.exe").arg(r"C:\AI\agent Bow\desktop\.env").spawn();
            } else if ev.id == quit_i.id() {
                std::process::exit(0);
            }
        }
    });
}

fn load_tray_icon() -> tray_icon::Icon {
    // 16x16 solid accent square fallback if no icon file is found.
    let bytes = include_bytes!("../icons/icon32.png");
    let img = image::load_from_memory(bytes).expect("decode tray icon").to_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).expect("tray icon from rgba")
}
```

- [ ] **Step 4: Ensure a tray icon asset exists**

Confirm `desktop/src-tauri/icons/icon32.png` exists (Tauri shipped icons there). If not, copy any existing PNG (e.g. `icons/128x128.png`) to `icons/icon32.png`.

Run: `ls desktop/src-tauri/icons/`
Expected: shows an `icon32.png` (create it if missing).

- [ ] **Step 5: Point `main.rs` at the new host**

Replace `desktop/src-tauri/src/main.rs` with:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    bow_desktop_lib::run();
}
```

(`bow_desktop_lib` is the lib name from `[lib] name`. Confirm `[lib] name = "bow_desktop_lib"` still set in Cargo.toml.)

- [ ] **Step 6: Build**

Run: `cargo build -p bow-desktop`
Expected: compiles with zero warnings, produces `target/debug/bow-desktop.exe`.

- [ ] **Step 7: Manual smoke test (verification by running)**

Create a throwaway web dir so static serving has something:
Run: `mkdir -p desktop/webapp/dist && echo "<h1>Bow Image Studio</h1>" > desktop/webapp/dist/index.html`
Run: `target/debug/bow-desktop.exe` (from `desktop/src-tauri/`)
Expected: a tray icon appears; the default browser opens `http://127.0.0.1:9357` showing "Bow Image Studio"; `curl http://127.0.0.1:9357/api/health` returns `ok`. Quit via the tray menu.

- [ ] **Step 8: Commit**

```bash
git add desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock desktop/src-tauri/src/main.rs desktop/src-tauri/src/lib.rs desktop/src-tauri/src/host.rs desktop/src-tauri/icons/
git rm desktop/src-tauri/build.rs desktop/src-tauri/tauri.conf.json
git commit -m "feat: replace Tauri host with plain server + system tray binary"
```

---

### Task 5: Placeholder web app project (Vite) that connects over WS

**Files:**
- Create: `desktop/webapp/` (Vite + React project: `package.json`, `vite.config.ts`, `index.html`, `src/main.tsx`, `src/App.tsx`)
- Reference for porting later: `extension/src/sidepanel/*` (do NOT delete yet; removed in Task 6)

**Interfaces:**
- Produces: `desktop/webapp/dist/index.html` (+ assets) — the `web_dir` served by `host.rs`. A minimal page that calls `GET /api/config`, opens `ws://<host>/ws`, sends `{type:"auth", token, session_id}`, and shows connection status.

- [ ] **Step 1: Scaffold the Vite project**

Run:
```bash
cd desktop && npm create vite@latest webapp -- --template react-ts
cd webapp && npm install
```

- [ ] **Step 2: Configure Vite base + build to `dist` and dev proxy**

Replace `desktop/webapp/vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "./",
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      "/api": "http://127.0.0.1:9357",
      "/ws": { target: "ws://127.0.0.1:9357", ws: true },
    },
  },
});
```

- [ ] **Step 3: Minimal connecting App**

Replace `desktop/webapp/src/App.tsx`:

```tsx
import { useEffect, useState } from "react";

export default function App() {
  const [status, setStatus] = useState("connecting…");
  useEffect(() => {
    const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
    const ws = new WebSocket(wsUrl);
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth", token: "dev", session_id: crypto.randomUUID() }));
    };
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.type === "auth_ok") setStatus("connected");
      else if (m.type === "auth_error") setStatus("auth error: " + (m.message ?? ""));
    };
    ws.onclose = () => setStatus("disconnected");
    ws.onerror = () => setStatus("error");
    return () => ws.close();
  }, []);
  return (
    <div style={{ fontFamily: "system-ui", padding: 24, background: "#1a1a2e", color: "#a8b2d8", minHeight: "100vh" }}>
      <h1 style={{ color: "#e94560" }}>Bow Image Studio</h1>
      <p>Backend: {status}</p>
    </div>
  );
}
```

- [ ] **Step 4: Build the web app**

Run: `cd desktop/webapp && npm run build`
Expected: `desktop/webapp/dist/index.html` exists.

- [ ] **Step 5: End-to-end run (verification)**

Note: the auth token here is `"dev"`. For this phase, allow it: temporarily, `auth::validate_token` must accept the configured `BOW_SECRET`. Set `BOW_SECRET=dev` in `desktop/.env` for local testing (real auth model is a Phase-later decision per the spec).

Run: `target/debug/bow-desktop.exe`
Expected: browser opens to the React page; status shows "connected" (WS upgrade works, auth_ok received).

- [ ] **Step 6: Commit**

```bash
git add desktop/webapp/ -- ':!desktop/webapp/node_modules'
git commit -m "feat: minimal web app that connects to the backend over WS"
```

(Ensure `desktop/webapp/node_modules` and `desktop/webapp/dist` are gitignored — add to `.gitignore` if needed and include that change in this commit.)

---

### Task 6: Remove the browser extension and update launchers/docs

**Files:**
- Delete: `extension/` (entire tree)
- Delete/replace: `Launch Bow.bat`, `Rebuild Extension.bat`, `Rebuild All.bat`, `Rebuild Bow.bat`
- Create: `bow.bat` (root launcher)
- Modify: `README.md` (remove extension instructions, document standalone app)

**Interfaces:**
- Produces: `bow.bat` in project root that builds the web app + backend and launches the exe.

- [ ] **Step 1: Create the root launcher `bow.bat`**

Create `bow.bat` in the project root:

```bat
@echo off
REM Bow Image Studio launcher — builds web UI + backend, then runs.
pushd "%~dp0desktop\webapp"
call npm run build || goto :err
popd
pushd "%~dp0desktop\src-tauri"
cargo build || goto :err
REM Copy built web assets next to the exe so release runs find them.
if not exist "target\debug\web" mkdir "target\debug\web"
xcopy /E /I /Y "..\webapp\dist\*" "target\debug\web\" >nul
start "" "target\debug\bow-desktop.exe"
popd
exit /b 0
:err
echo Build failed.
popd
exit /b 1
```

- [ ] **Step 2: Remove the extension tree**

Run:
```bash
git rm -r extension
```

- [ ] **Step 3: Remove obsolete .bat files**

Run:
```bash
git rm "Launch Bow.bat" "Rebuild Extension.bat" "Rebuild All.bat" "Rebuild Bow.bat"
```

- [ ] **Step 4: Update README**

In `README.md`, remove the "load the extension in Chrome/Edge" section and any extension build steps; add a "Run" section:

```markdown
## Run

1. Ensure `desktop/.env` is configured (LM Studio URL/model, BOW_SECRET, etc.).
2. Double-click `bow.bat` in the project root.
3. Your browser opens to `http://127.0.0.1:9357` (Bow Image Studio).

There is no browser extension — Bow runs as a standalone local web app.
```

- [ ] **Step 5: Verify the full launch path (verification by running)**

Run: `./bow.bat` (or double-click)
Expected: web app builds, backend builds, exe launches, tray appears, browser opens to the connected page. `extension/` no longer exists.

- [ ] **Step 6: Commit**

```bash
git add bow.bat README.md .gitignore
git commit -m "chore: remove browser extension; add standalone bow.bat launcher"
```

---

## Self-Review

**Spec coverage (Phase 1 scope):**
- Standalone web app at localhost — Tasks 2–6 ✓
- Backend host swap: axum + tray + .bat, drop Tauri window — Tasks 2,3,4,6 ✓
- Single port 9357 for SPA + REST + WS — Tasks 2,3 ✓
- Remove extension — Task 6 ✓
- MCP non-blocking regression fix — Task 1 ✓
- 127.0.0.1-only bind — Task 4 ✓
- Deferred to later phases (correctly absent here): search-scrape UI (Phase 2), controlled browser (Phase 3), AI assist panel + source repair (Phase 4), final auth model (open item).

**Placeholder scan:** No TBD/TODO; each code step shows full code; commands have expected output. Auth uses an explicit interim token (`BOW_SECRET=dev`) with a note that the final auth model is a later decision — this is a deliberate, documented interim, not a placeholder.

**Type consistency:** `McpManager::load_in_background(String)`, `schemas() -> Vec<Value>`, `is_mcp_tool(&str) -> bool` used consistently across Tasks 1/3/4. `http::build_router(AppState, McpManager, PathBuf) -> Router` used identically in Tasks 2/3/4. `server::run_ws(WebSocket, Arc<Config>, ShellSessionManager, McpManager)` defined in Task 3 and called in Task 3's `ws_upgrade`.

**Risk callouts (carry into execution):**
- `tray-icon`/`tao` exact versions may resolve differently; if the `0.19`/`0.30` APIs differ (e.g. `Icon::from_rgba`, `MenuEvent::receiver`), adjust per the resolved crate docs — the integration shape (tray on main thread, tokio runtime on background) holds regardless.
- axum `Message::Text` signature across 0.7.x: confirm it wraps `String` (older) vs `Utf8Bytes` (newer); adapt the `WsMessage::Text(text)` send/recv accordingly.
- `AppState` must be `Clone` and expose `.config` and `.shell_session` (it already does per current `lib.rs`/`server.rs` usage).
