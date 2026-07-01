# Bow Image Studio — Phase 2: Search-Scrape Web UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the standalone web app a dedicated search-scrape UI — query + source toggles + count + destination, a live progress stream, and a curation grid of the downloaded images with select/delete/dedupe/open-folder actions — driven directly by the user (no LLM in the loop).

**Architecture:** The existing `image_download` scraper engine is refactored to emit structured progress events over an mpsc channel. A new WebSocket message (`scrape_request`) runs a scrape and streams `scrape_event` messages back. New REST endpoints under `/api` list a folder's images, serve on-the-fly thumbnails, delete selected files, and run perceptual dedupe — all guardrailed to the workspace. The React SPA gains a Zustand store, a SearchPanel, a live ProgressLog, and a CurationGrid.

**Tech Stack:** Rust (axum, tokio mpsc, the `image` + `image_hasher` crates already in tree), React + TypeScript + Vite + Zustand, vitest for store-logic tests.

## Design decisions (settled by the approved spec; called out for reviewer)
- **Curate AFTER download.** Per the spec's search-scrape data flow (scrape → filter → download → curation grid renders thumbnails). Pre-download remote thumbnails are not used because candidate image URLs mostly hotlink-block in a browser (they need the backend's Referer headers). The grid shows local downloaded files served by the backend.
- **Scrape over WS (long-running, streamed); curation over REST (request/response).**
- **REST file endpoints are loopback-only and path-guardrailed to `config.workspace_root`** (consistent with Phase 1's loopback `/api/config`); no per-request token in this phase. Final auth model remains a deferred spec item.

## Global Constraints
- Platform: Windows 11. Paths use the existing `\\`-style join conventions already in `image_search.rs`.
- Bind 127.0.0.1 only (already enforced in `host.rs`); single port `config.ws_port` (9357).
- Local LLM only — no cloud/Anthropic calls. The scrape UI does NOT invoke the LLM.
- Fix all compiler warnings; pristine test output.
- Preserve all existing WS protocol message shapes; only ADD new ones (`scrape_request` inbound, `scrape_event` outbound).
- Every filesystem-mutating or filesystem-reading REST endpoint MUST reject paths outside `config.workspace_root` (canonicalized) with HTTP 400.
- Reuse existing engine functions; do not reimplement scraping, dedupe, or image listing.

---

### Task 1: Stream scrape progress from `image_download`

**Files:**
- Modify: `desktop/src-tauri/src/tools/image_search.rs` (add `ScrapeEvent`, thread an optional progress sender through `image_download`)
- Modify: the single call site of `image_download` in `desktop/src-tauri/src/tools/mod.rs` (pass `None`)
- Test: inline `#[cfg(test)]` in `image_search.rs`

**Interfaces:**
- Produces:
  - `pub enum ScrapeEvent` with variants:
    - `Phase { label: String }`
    - `Source { source: String, count: usize, error: Option<String> }`
    - `Candidates { total: usize, filtered: usize }`
    - `Downloaded { done: usize, target: usize, path: String }`
    - `Failed { url: String, reason: String }`
    - `Done { downloaded: Vec<String>, log_note: String }`
  - `impl ScrapeEvent { pub fn to_json(&self) -> serde_json::Value }` — shape `{ "kind": "<variant snake_case>", ...fields }`, with a top-level `"type":"scrape_event"` added by the caller (Task 2), NOT here.
  - `pub async fn image_download(query: &str, count: usize, dest_dir: &str, log_dir: &str, progress: Option<tokio::sync::mpsc::UnboundedSender<ScrapeEvent>>) -> Result<String>` — same behavior as today; when `progress` is `Some`, it also emits events at each phase. The final `Ok(String)` summary is unchanged.

- [ ] **Step 1: Write the failing test for `ScrapeEvent::to_json`**

Add to the `#[cfg(test)] mod tests` in `image_search.rs`:

```rust
#[test]
fn scrape_event_to_json_shapes() {
    let e = ScrapeEvent::Source { source: "Bing".into(), count: 35, error: None };
    let v = e.to_json();
    assert_eq!(v["kind"], "source");
    assert_eq!(v["source"], "Bing");
    assert_eq!(v["count"], 35);

    let d = ScrapeEvent::Downloaded { done: 3, target: 15, path: "C:\\x\\a.jpg".into() };
    let dv = d.to_json();
    assert_eq!(dv["kind"], "downloaded");
    assert_eq!(dv["done"], 3);
    assert_eq!(dv["target"], 15);
    assert_eq!(dv["path"], "C:\\x\\a.jpg");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bow-desktop scrape_event_to_json_shapes --lib`
Expected: FAIL — `ScrapeEvent` not found.

- [ ] **Step 3: Define `ScrapeEvent` and `to_json`**

Add near the top of `image_search.rs` (after the `ScrapeResult` block):

```rust
use tokio::sync::mpsc::UnboundedSender;

/// Progress events emitted during a streamed `image_download`.
#[derive(Debug, Clone)]
pub enum ScrapeEvent {
    Phase { label: String },
    Source { source: String, count: usize, error: Option<String> },
    Candidates { total: usize, filtered: usize },
    Downloaded { done: usize, target: usize, path: String },
    Failed { url: String, reason: String },
    Done { downloaded: Vec<String>, log_note: String },
}

impl ScrapeEvent {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ScrapeEvent::Phase { label } => json!({ "kind": "phase", "label": label }),
            ScrapeEvent::Source { source, count, error } =>
                json!({ "kind": "source", "source": source, "count": count, "error": error }),
            ScrapeEvent::Candidates { total, filtered } =>
                json!({ "kind": "candidates", "total": total, "filtered": filtered }),
            ScrapeEvent::Downloaded { done, target, path } =>
                json!({ "kind": "downloaded", "done": done, "target": target, "path": path }),
            ScrapeEvent::Failed { url, reason } =>
                json!({ "kind": "failed", "url": url, "reason": reason }),
            ScrapeEvent::Done { downloaded, log_note } =>
                json!({ "kind": "done", "downloaded": downloaded, "log_note": log_note }),
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p bow-desktop scrape_event_to_json_shapes --lib`
Expected: PASS.

- [ ] **Step 5: Thread the optional sender through `image_download`**

Change the signature to add the `progress` parameter, and emit events. Add a tiny local helper at the top of the function body:

```rust
pub async fn image_download(
    query: &str,
    count: usize,
    dest_dir: &str,
    log_dir: &str,
    progress: Option<UnboundedSender<ScrapeEvent>>,
) -> Result<String> {
    let emit = |e: ScrapeEvent| { if let Some(tx) = &progress { let _ = tx.send(e); } };
```

Then add `emit(...)` calls at these existing points (leave all current logic and the `log.push(...)` lines intact):
- After `log.push("-- Scraping sources --"...)`: `emit(ScrapeEvent::Phase { label: "Scraping sources".into() });`
- In the `for r in &results` loop, after `log.push(r.log_line())`: `emit(ScrapeEvent::Source { source: r.source.to_string(), count: r.urls.len(), error: r.error.clone() });`
- After the paid-CDN `retain` + the `filtered` calculation: `emit(ScrapeEvent::Candidates { total: candidates.len(), filtered });`
- After `log.push("-- Downloading ...")`: `emit(ScrapeEvent::Phase { label: "Downloading".into() });`
- Inside the `while let Some(task_result)` loop, in the `if ok` branch right after `downloaded.push(path)`: `emit(ScrapeEvent::Downloaded { done: downloaded.len(), target: count, path: downloaded.last().cloned().unwrap_or_default() });`
- In the `else` branch (failure), after `failures.push((url, reason))`: clone before push so you can emit: change to `failures.push((url.clone(), reason.clone())); emit(ScrapeEvent::Failed { url, reason });`
- Just before the final `Ok(format!(...))`: `emit(ScrapeEvent::Done { downloaded: downloaded.clone(), log_note: log_note.clone() });`

Note: `r.source` is `&'static str` and `r.error` is `Option<String>` (already `Clone`). `download` failure branch currently moves `url`/`reason` into the tuple; reorder to clone for the event as shown.

- [ ] **Step 6: Update the call site in `tools/mod.rs`**

Find the `image_download(` call in `desktop/src-tauri/src/tools/mod.rs` and add `None` as the final argument:

```rust
crate::tools::image_search::image_download(&query, count, &dest_dir, &log_dir, None).await
```

(Keep the existing `query`/`count`/`dest_dir`/`log_dir` derivation exactly as-is.)

- [ ] **Step 7: Build and test**

Run: `cargo build -p bow-desktop` — Expected: zero warnings.
Run: `cargo test -p bow-desktop --lib` — Expected: all pass (existing + the new event test).

- [ ] **Step 8: Commit**

```bash
git add desktop/src-tauri/src/tools/image_search.rs desktop/src-tauri/src/tools/mod.rs
git commit -m "feat: emit structured progress events from image_download"
```

---

### Task 2: WebSocket `scrape_request` → streamed `scrape_event`

**Files:**
- Modify: `desktop/src-tauri/src/server.rs` (add `ScrapeRequest` inbound variant + handler)
- Test: inline `#[cfg(test)]` in `server.rs`

**Interfaces:**
- Consumes: `ScrapeEvent`, `image_download(.., Some(tx))`, the existing `out_tx: mpsc::Sender<String>` and `config: Arc<Config>` in `run_ws`.
- Produces: WS protocol additions:
  - Inbound: `{ "type": "scrape_request", "query": String, "count": u32, "dest_dir": String }`
  - Outbound: `{ "type": "scrape_event", ...<ScrapeEvent::to_json fields> }`

- [ ] **Step 1: Write the failing test for inbound parsing**

In `server.rs` test module add:

```rust
#[test]
fn scrape_request_parses() {
    let v = serde_json::json!({"type":"scrape_request","query":"cats","count":15,"dest_dir":"C:\\x"});
    let parsed: InboundMsg = serde_json::from_value(v).unwrap();
    match parsed {
        InboundMsg::ScrapeRequest { query, count, dest_dir } => {
            assert_eq!(query, "cats");
            assert_eq!(count, 15);
            assert_eq!(dest_dir, "C:\\x");
        }
        _ => panic!("wrong variant"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bow-desktop scrape_request_parses --lib`
Expected: FAIL — no `ScrapeRequest` variant.

- [ ] **Step 3: Add the inbound variant**

In the `InboundMsg` enum in `server.rs`, add:

```rust
    ScrapeRequest { query: String, count: u32, dest_dir: String },
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bow-desktop scrape_request_parses --lib`
Expected: PASS.

- [ ] **Step 5: Handle `ScrapeRequest` in the message loop**

In `run_ws`, in the `match inbound` block (must be authenticated — place it after the auth gate like the other handlers), add an arm. It spawns the scrape and forwards events to the client over `out_tx`:

```rust
                    InboundMsg::ScrapeRequest { query, count, dest_dir } => {
                        let out_tx = out_tx.clone();
                        let log_dir = format!("{}\\logs", config.workspace_root.to_string_lossy().trim_end_matches(['\\','/']));
                        tokio::spawn(async move {
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            // Forward events to the client as they arrive.
                            let fwd_tx = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    let mut v = ev.to_json();
                                    v["type"] = serde_json::Value::String("scrape_event".into());
                                    let _ = fwd_tx.send(v.to_string()).await;
                                }
                            });
                            let result = crate::tools::image_search::image_download(
                                &query, count as usize, &dest_dir, &log_dir, Some(tx),
                            ).await;
                            // tx dropped here → forwarder drains and exits.
                            let _ = forwarder.await;
                            if let Err(e) = result {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message": e.to_string()});
                                let _ = out_tx.send(err.to_string()).await;
                            }
                        });
                    }
```

Note: `out_tx` is the `mpsc::Sender<String>` already used in `run_ws` to push frames to the WS sink. Confirm its name matches the existing code; if the existing sender is named differently, use that name.

- [ ] **Step 6: Build and test**

Run: `cargo build -p bow-desktop` — Expected: zero warnings.
Run: `cargo test -p bow-desktop --lib` — Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add desktop/src-tauri/src/server.rs
git commit -m "feat: WS scrape_request runs a streamed scrape and emits scrape_event"
```

---

### Task 3: REST curation endpoints (list / thumbnail / delete / dedupe / open-folder)

**Files:**
- Create: `desktop/src-tauri/src/web_api.rs` (handlers + path guardrail)
- Modify: `desktop/src-tauri/src/http.rs` (mount the routes)
- Modify: `desktop/src-tauri/src/lib.rs` (add `mod web_api;`)
- Test: inline `#[cfg(test)]` in `web_api.rs`

**Interfaces:**
- Consumes: `HttpState` (has `app.config.workspace_root`), `image_curate::{collect_images, image_dedupe}`, the `image` crate.
- Produces:
  - `pub fn within_workspace(workspace_root: &std::path::Path, candidate: &str) -> Option<std::path::PathBuf>` — returns the canonicalized path if it is inside `workspace_root`, else `None`.
  - `pub fn routes() -> axum::Router<HttpState>` mounting:
    - `GET  /api/images?dir=<path>` → `{ "dir": String, "images": [ { "name": String, "path": String, "bytes": u64 } ] }`
    - `GET  /api/thumb?path=<path>&w=<u32>` → image bytes (`image/jpeg`), longest side ≤ w (default 256)
    - `POST /api/images/delete` body `{ "paths": [String] }` → `{ "deleted": usize, "errors": usize }`
    - `POST /api/curate/dedupe` body `{ "dir": String, "threshold": u32, "apply": bool }` → `{ "report": String }`
    - `POST /api/open-folder` body `{ "path": String }` → `{ "ok": true }` (spawns `explorer.exe`)

- [ ] **Step 1: Write the failing test for the guardrail**

Create `desktop/src-tauri/src/web_api.rs` with just:

```rust
use std::path::{Path, PathBuf};

pub fn within_workspace(workspace_root: &Path, candidate: &str) -> Option<PathBuf> {
    let root = workspace_root.canonicalize().ok()?;
    let cand = Path::new(candidate).canonicalize().ok()?;
    if cand.starts_with(&root) { Some(cand) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_path_outside_workspace() {
        let ws = std::env::temp_dir().join(format!("bow_ws_{}", uuid::Uuid::new_v4().simple()));
        let inside = ws.join("a"); std::fs::create_dir_all(&inside).unwrap();
        let f = inside.join("x.txt"); std::fs::write(&f, b"hi").unwrap();
        // inside ok
        assert!(within_workspace(&ws, f.to_str().unwrap()).is_some());
        // outside rejected
        let outside = std::env::temp_dir().join("definitely_not_in_ws.txt");
        std::fs::write(&outside, b"hi").ok();
        assert!(within_workspace(&ws, outside.to_str().unwrap()).is_none());
        std::fs::remove_dir_all(&ws).ok();
    }
}
```

- [ ] **Step 2: Run test to verify it fails, then add module decl**

Add `mod web_api;` to `lib.rs` (after `mod server;`).
Run: `cargo test -p bow-desktop rejects_path_outside_workspace --lib`
Expected: first FAIL if module not declared / then PASS once `mod web_api;` is added and the function compiles. (Write the test, confirm RED by temporarily expecting a missing symbol if needed, then GREEN.)

- [ ] **Step 3: Implement the handlers**

Append to `web_api.rs`:

```rust
use crate::http::HttpState;
use axum::{extract::{Query, State}, http::StatusCode, response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct DirQuery { pub dir: String }

pub async fn list_images(State(s): State<HttpState>, Query(q): Query<DirQuery>) -> Response {
    let Some(dir) = within_workspace(&s.app.config.workspace_root, &q.dir) else {
        return (StatusCode::BAD_REQUEST, "dir outside workspace").into_response();
    };
    let mut paths = Vec::new();
    crate::tools::image_curate::collect_images(&dir, false, &mut paths);
    let images: Vec<_> = paths.iter().map(|p| {
        let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        json!({ "name": p.file_name().and_then(|n| n.to_str()).unwrap_or(""), "path": p.to_string_lossy(), "bytes": bytes })
    }).collect();
    Json(json!({ "dir": dir.to_string_lossy(), "images": images })).into_response()
}

#[derive(Deserialize)]
pub struct ThumbQuery { pub path: String, pub w: Option<u32> }

pub async fn thumb(State(s): State<HttpState>, Query(q): Query<ThumbQuery>) -> Response {
    let Some(path) = within_workspace(&s.app.config.workspace_root, &q.path) else {
        return (StatusCode::BAD_REQUEST, "path outside workspace").into_response();
    };
    let w = q.w.unwrap_or(256).clamp(32, 1024);
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let img = image::open(&path)?;
        let img = img.resize(w, w, image::imageops::FilterType::Triangle);
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img.to_rgb8())
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
        Ok(buf)
    }).await;
    match bytes {
        Ok(Ok(b)) => ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], b).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, "could not render thumbnail").into_response(),
    }
}

#[derive(Deserialize)]
pub struct DeleteBody { pub paths: Vec<String> }

pub async fn delete_images(State(s): State<HttpState>, Json(b): Json<DeleteBody>) -> Response {
    let (mut deleted, mut errors) = (0usize, 0usize);
    for p in &b.paths {
        match within_workspace(&s.app.config.workspace_root, p) {
            Some(path) => if std::fs::remove_file(&path).is_ok() { deleted += 1 } else { errors += 1 },
            None => errors += 1,
        }
    }
    Json(json!({ "deleted": deleted, "errors": errors })).into_response()
}

#[derive(Deserialize)]
pub struct DedupeBody { pub dir: String, pub threshold: Option<u32>, pub apply: Option<bool> }

pub async fn dedupe(State(s): State<HttpState>, Json(b): Json<DedupeBody>) -> Response {
    let Some(dir) = within_workspace(&s.app.config.workspace_root, &b.dir) else {
        return (StatusCode::BAD_REQUEST, "dir outside workspace").into_response();
    };
    match crate::tools::image_curate::image_dedupe(&dir.to_string_lossy(), b.threshold.unwrap_or(10), false, b.apply.unwrap_or(false)).await {
        Ok(report) => Json(json!({ "report": report })).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct OpenBody { pub path: String }

pub async fn open_folder(State(s): State<HttpState>, Json(b): Json<OpenBody>) -> Response {
    let Some(path) = within_workspace(&s.app.config.workspace_root, &b.path) else {
        return (StatusCode::BAD_REQUEST, "path outside workspace").into_response();
    };
    let _ = std::process::Command::new("explorer.exe").arg(&path).spawn();
    Json(json!({ "ok": true })).into_response()
}

pub fn routes() -> Router<HttpState> {
    Router::new()
        .route("/api/images", get(list_images))
        .route("/api/thumb", get(thumb))
        .route("/api/images/delete", post(delete_images))
        .route("/api/curate/dedupe", post(dedupe))
        .route("/api/open-folder", post(open_folder))
}
```

- [ ] **Step 4: Mount the routes in `http.rs`**

In `build_router`, merge the web_api routes before `.fallback_service(...)`:

```rust
        .merge(crate::web_api::routes())
```

(Place it after the existing `.route("/ws", ...)` line and before `.fallback_service`.)

- [ ] **Step 5: Write a test for `/api/images` listing**

Add to `web_api.rs` tests:

```rust
#[tokio::test]
async fn list_images_returns_files_in_dir() {
    use axum::body::Body; use axum::http::{Request, StatusCode}; use tower::ServiceExt;
    use image::{Rgb, RgbImage};
    let ws = std::env::temp_dir().join(format!("bow_ws_li_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&ws).unwrap();
    RgbImage::from_pixel(20, 20, Rgb([1,2,3])).save(ws.join("a.png")).unwrap();

    let state = crate::http::HttpState::test_state(ws.clone()); // see Step 6
    let app = crate::web_api::routes().with_state(state);
    let uri = format!("/api/images?dir={}", urlencoding::encode(ws.to_str().unwrap()));
    let res = app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1<<20).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["images"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(&ws).ok();
}
```

- [ ] **Step 6: Add a test constructor for `HttpState`**

`HttpState` needs a way to build a minimal instance for tests without a full `Config::from_env`. In `http.rs`, add (gated to tests) a constructor that builds an `AppState` with a given workspace_root and defaults for the rest. Implement `HttpState::test_state` so the test compiles:

```rust
#[cfg(test)]
impl HttpState {
    pub fn test_state(workspace_root: std::path::PathBuf) -> Self {
        let config = crate::state::Config::test_default(workspace_root);
        HttpState { app: crate::state::AppState::new(config), mcp: crate::tools::mcp::McpManager::empty() }
    }
}
```

And in `state.rs`, add a matching `#[cfg(test)] pub fn test_default(workspace_root: PathBuf) -> Config` that fills `Config` with harmless defaults (ws_port 9357, empty/placeholder strings for secrets/urls, `reasoning_*: None`). Match the actual `Config` field set at implementation time.

- [ ] **Step 7: Run tests + build**

Run: `cargo test -p bow-desktop --lib` — Expected: guardrail + listing tests pass, all others pass.
Run: `cargo build -p bow-desktop` — Expected: zero warnings.

- [ ] **Step 8: Commit**

```bash
git add desktop/src-tauri/src/web_api.rs desktop/src-tauri/src/http.rs desktop/src-tauri/src/lib.rs desktop/src-tauri/src/state.rs
git commit -m "feat: REST endpoints for image listing, thumbnails, delete, dedupe, open-folder"
```

---

### Task 4: Frontend — Zustand store + SearchPanel + live progress

**Files:**
- Modify: `desktop/webapp/package.json` (add `zustand`, dev: `vitest`)
- Create: `desktop/webapp/src/store.ts` (connection + scrape state, WS wiring)
- Create: `desktop/webapp/src/components/SearchPanel.tsx`
- Create: `desktop/webapp/src/components/ProgressLog.tsx`
- Modify: `desktop/webapp/src/App.tsx` (compose the panels)
- Test: `desktop/webapp/src/store.test.ts` (vitest)

**Interfaces:**
- Consumes: WS protocol `scrape_request` / `scrape_event` (Task 2), `/api/config` (token).
- Produces: a Zustand store exposing `status`, `events: ScrapeEventMsg[]`, `downloaded: string[]`, `lastDestDir: string`, and actions `connect()`, `startScrape({query, count, destDir})`. A pure reducer `applyEvent(state, msg)` for testability.

- [ ] **Step 1: Add deps**

Run: `cd desktop/webapp && npm install zustand && npm install -D vitest`
Add to `package.json` scripts: `"test": "vitest run"`.

- [ ] **Step 2: Write the failing reducer test**

Create `desktop/webapp/src/store.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { applyEvent, initialScrapeState } from "./store";

describe("applyEvent", () => {
  it("accumulates downloaded files and tracks done count", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "phase", label: "Downloading" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 1, target: 3, path: "C:\\x\\a.jpg" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 2, target: 3, path: "C:\\x\\b.jpg" });
    expect(s.downloaded).toEqual(["C:\\x\\a.jpg", "C:\\x\\b.jpg"]);
    expect(s.done).toBe(2);
    expect(s.target).toBe(3);
  });

  it("marks finished on done", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "done", downloaded: ["a"], log_note: "Log: x" });
    expect(s.finished).toBe(true);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd desktop/webapp && npx vitest run store.test.ts`
Expected: FAIL — `./store` has no `applyEvent`/`initialScrapeState`.

- [ ] **Step 4: Implement the store with a pure reducer**

Create `desktop/webapp/src/store.ts`:

```ts
import { create } from "zustand";

export type ScrapeEventMsg =
  | { type: "scrape_event"; kind: "phase"; label: string }
  | { type: "scrape_event"; kind: "source"; source: string; count: number; error: string | null }
  | { type: "scrape_event"; kind: "candidates"; total: number; filtered: number }
  | { type: "scrape_event"; kind: "downloaded"; done: number; target: number; path: string }
  | { type: "scrape_event"; kind: "failed"; url: string; reason: string }
  | { type: "scrape_event"; kind: "done"; downloaded: string[]; log_note: string }
  | { type: "scrape_event"; kind: "error"; message: string };

export interface ScrapeState {
  running: boolean;
  finished: boolean;
  phase: string;
  done: number;
  target: number;
  downloaded: string[];
  sources: { source: string; count: number; error: string | null }[];
  log: string[];
  error: string | null;
}

export function initialScrapeState(): ScrapeState {
  return { running: false, finished: false, phase: "", done: 0, target: 0, downloaded: [], sources: [], log: [], error: null };
}

export function applyEvent(s: ScrapeState, m: ScrapeEventMsg): ScrapeState {
  switch (m.kind) {
    case "phase": return { ...s, phase: m.label, log: [...s.log, m.label] };
    case "source": return { ...s, sources: [...s.sources, { source: m.source, count: m.count, error: m.error }],
                            log: [...s.log, `${m.source}: ${m.error ? "ERROR " + m.error : m.count + " URLs"}`] };
    case "candidates": return { ...s, log: [...s.log, `candidates: ${m.total} (filtered ${m.filtered})`] };
    case "downloaded": return { ...s, done: m.done, target: m.target, downloaded: [...s.downloaded, m.path] };
    case "failed": return { ...s, log: [...s.log, `failed: ${m.reason}`] };
    case "done": return { ...s, running: false, finished: true, log: [...s.log, m.log_note] };
    case "error": return { ...s, running: false, finished: true, error: m.message, log: [...s.log, "ERROR: " + m.message] };
    default: return s;
  }
}

interface Store {
  status: string;
  scrape: ScrapeState;
  lastDestDir: string;
  connect: () => void;
  startScrape: (a: { query: string; count: number; destDir: string }) => void;
  _ws?: WebSocket;
  _token?: string;
}

export const useStore = create<Store>((set, get) => ({
  status: "connecting…",
  scrape: initialScrapeState(),
  lastDestDir: "",
  connect: () => {
    fetch("/api/config").then(r => r.json()).then(cfg => {
      const token: string = cfg.token ?? "";
      const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
      const ws = new WebSocket(wsUrl);
      ws.onopen = () => ws.send(JSON.stringify({ type: "auth", token, session_id: crypto.randomUUID() }));
      ws.onmessage = (e) => {
        const m = JSON.parse(e.data);
        if (m.type === "auth_ok") set({ status: "connected" });
        else if (m.type === "auth_error") set({ status: "auth error: " + (m.message ?? "") });
        else if (m.type === "scrape_event") set((st) => ({ scrape: applyEvent(st.scrape, m) }));
      };
      ws.onclose = () => set({ status: "disconnected" });
      ws.onerror = () => set({ status: "error" });
      set({ _ws: ws, _token: token });
    }).catch(() => set({ status: "config unavailable" }));
  },
  startScrape: ({ query, count, destDir }) => {
    const ws = get()._ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: count }, lastDestDir: destDir });
    ws.send(JSON.stringify({ type: "scrape_request", query, count, dest_dir: destDir }));
  },
}));
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd desktop/webapp && npx vitest run store.test.ts`
Expected: PASS.

- [ ] **Step 6: Build SearchPanel + ProgressLog and compose in App**

Create `desktop/webapp/src/components/SearchPanel.tsx`:

```tsx
import { useState } from "react";
import { useStore } from "../store";

export default function SearchPanel() {
  const startScrape = useStore((s) => s.startScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [query, setQuery] = useState("");
  const [count, setCount] = useState(15);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");

  const disabled = running || status !== "connected" || !query.trim() || !destDir.trim();
  return (
    <div style={{ display: "grid", gap: 8, maxWidth: 560 }}>
      <input placeholder="Search query (e.g. golden retriever puppies)" value={query}
        onChange={(e) => setQuery(e.target.value)} style={inp} />
      <div style={{ display: "flex", gap: 8 }}>
        <input type="number" min={1} max={200} value={count}
          onChange={(e) => setCount(Math.max(1, Math.min(200, Number(e.target.value) || 1)))}
          style={{ ...inp, width: 90 }} />
        <input placeholder="Destination folder" value={destDir}
          onChange={(e) => setDestDir(e.target.value)} style={{ ...inp, flex: 1 }} />
      </div>
      <button disabled={disabled} onClick={() => startScrape({ query, count, destDir })}
        style={{ ...btn, opacity: disabled ? 0.5 : 1 }}>
        {running ? "Scraping…" : "Download images"}
      </button>
    </div>
  );
}
const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
```

Create `desktop/webapp/src/components/ProgressLog.tsx`:

```tsx
import { useStore } from "../store";

export default function ProgressLog() {
  const scrape = useStore((s) => s.scrape);
  if (!scrape.running && !scrape.finished) return null;
  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ color: "#a8b2d8", marginBottom: 6 }}>
        {scrape.phase} — {scrape.done}/{scrape.target} downloaded
        {scrape.error ? ` · ${scrape.error}` : ""}
      </div>
      <pre style={{ background: "#16213e", color: "#8893b8", padding: 10, borderRadius: 8, maxHeight: 200, overflow: "auto", fontSize: 12 }}>
        {scrape.log.join("\n")}
      </pre>
    </div>
  );
}
```

Update `desktop/webapp/src/App.tsx` to connect on mount and render the panels:

```tsx
import { useEffect } from "react";
import { useStore } from "./store";
import SearchPanel from "./components/SearchPanel";
import ProgressLog from "./components/ProgressLog";

export default function App() {
  const connect = useStore((s) => s.connect);
  const status = useStore((s) => s.status);
  useEffect(() => { connect(); }, [connect]);
  return (
    <div style={{ fontFamily: "system-ui", padding: 24, background: "#1a1a2e", color: "#a8b2d8", minHeight: "100vh" }}>
      <h1 style={{ color: "#e94560", marginTop: 0 }}>Bow Image Studio</h1>
      <p style={{ marginTop: -8, fontSize: 13 }}>Backend: {status}</p>
      <SearchPanel />
      <ProgressLog />
    </div>
  );
}
```

- [ ] **Step 7: Build the web app**

Run: `cd desktop/webapp && npm run build`
Expected: succeeds; `dist/index.html` produced; no TypeScript errors.

- [ ] **Step 8: Commit**

```bash
git add desktop/webapp/package.json desktop/webapp/package-lock.json desktop/webapp/src/store.ts desktop/webapp/src/store.test.ts desktop/webapp/src/App.tsx desktop/webapp/src/components/
git commit -m "feat: search panel + zustand store + live scrape progress"
```

---

### Task 5: Frontend — CurationGrid (thumbnails, select, delete, dedupe, open folder)

**Files:**
- Create: `desktop/webapp/src/components/CurationGrid.tsx`
- Create: `desktop/webapp/src/api.ts` (typed REST helpers)
- Modify: `desktop/webapp/src/App.tsx` (render the grid after a scrape finishes)
- Test: `desktop/webapp/src/api.test.ts` (vitest — URL construction)

**Interfaces:**
- Consumes: `/api/images`, `/api/thumb`, `/api/images/delete`, `/api/curate/dedupe`, `/api/open-folder` (Task 3); the store's `lastDestDir` and `scrape.finished`.
- Produces: `api.ts` with `listImages(dir)`, `thumbUrl(path, w)`, `deleteImages(paths)`, `dedupe(dir, apply)`, `openFolder(dir)`.

- [ ] **Step 1: Write the failing test for `thumbUrl`**

Create `desktop/webapp/src/api.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { thumbUrl } from "./api";

describe("thumbUrl", () => {
  it("encodes the path and width", () => {
    const u = thumbUrl("C:\\x\\a b.jpg", 256);
    expect(u).toBe(`/api/thumb?path=${encodeURIComponent("C:\\x\\a b.jpg")}&w=256`);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd desktop/webapp && npx vitest run api.test.ts`
Expected: FAIL — no `./api`.

- [ ] **Step 3: Implement `api.ts`**

```ts
export interface ImageItem { name: string; path: string; bytes: number }

export function thumbUrl(path: string, w = 256): string {
  return `/api/thumb?path=${encodeURIComponent(path)}&w=${w}`;
}
export async function listImages(dir: string): Promise<ImageItem[]> {
  const r = await fetch(`/api/images?dir=${encodeURIComponent(dir)}`);
  if (!r.ok) return [];
  return (await r.json()).images as ImageItem[];
}
export async function deleteImages(paths: string[]): Promise<{ deleted: number; errors: number }> {
  const r = await fetch("/api/images/delete", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ paths }) });
  return r.json();
}
export async function dedupe(dir: string, apply: boolean): Promise<string> {
  const r = await fetch("/api/curate/dedupe", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ dir, apply }) });
  return (await r.json()).report ?? "";
}
export async function openFolder(path: string): Promise<void> {
  await fetch("/api/open-folder", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ path }) });
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd desktop/webapp && npx vitest run api.test.ts`
Expected: PASS.

- [ ] **Step 5: Implement CurationGrid**

Create `desktop/webapp/src/components/CurationGrid.tsx`:

```tsx
import { useEffect, useState, useCallback } from "react";
import { useStore } from "../store";
import { ImageItem, listImages, thumbUrl, deleteImages, dedupe, openFolder } from "../api";

export default function CurationGrid() {
  const dir = useStore((s) => s.lastDestDir);
  const finished = useStore((s) => s.scrape.finished);
  const [items, setItems] = useState<ImageItem[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    if (!dir) return;
    setItems(await listImages(dir));
    setSelected(new Set());
  }, [dir]);

  useEffect(() => { if (finished) refresh(); }, [finished, refresh]);

  if (!dir || items.length === 0) return null;

  const toggle = (p: string) => setSelected((s) => { const n = new Set(s); n.has(p) ? n.delete(p) : n.add(p); return n; });

  const onDelete = async () => {
    if (selected.size === 0) return;
    const res = await deleteImages([...selected]);
    setNote(`Deleted ${res.deleted}${res.errors ? `, ${res.errors} errors` : ""}`);
    refresh();
  };
  const onDedupe = async () => { setNote(await dedupe(dir, true)); refresh(); };

  return (
    <div style={{ marginTop: 20 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
        <strong style={{ color: "#a8b2d8" }}>{items.length} images</strong>
        <button onClick={onDelete} disabled={selected.size === 0} style={tool}>Delete selected ({selected.size})</button>
        <button onClick={onDedupe} style={tool}>Remove duplicates</button>
        <button onClick={() => openFolder(dir)} style={tool}>Open folder</button>
        <button onClick={refresh} style={tool}>Refresh</button>
        {note && <span style={{ color: "#8893b8", fontSize: 12 }}>{note}</span>}
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))", gap: 8 }}>
        {items.map((it) => {
          const sel = selected.has(it.path);
          return (
            <div key={it.path} onClick={() => toggle(it.path)}
              style={{ border: `2px solid ${sel ? "#e94560" : "#2a2a4a"}`, borderRadius: 8, overflow: "hidden", cursor: "pointer", background: "#16213e" }}>
              <img src={thumbUrl(it.path)} alt={it.name} loading="lazy"
                style={{ width: "100%", height: 120, objectFit: "cover", display: "block", opacity: sel ? 0.7 : 1 }} />
              <div style={{ fontSize: 10, color: "#8893b8", padding: "2px 4px", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{it.name}</div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
const tool: React.CSSProperties = { background: "#0f3460", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 6, padding: "6px 10px", cursor: "pointer", fontSize: 12 };
```

Add `<CurationGrid />` to `App.tsx` below `<ProgressLog />` (import it at the top).

- [ ] **Step 6: Build the web app**

Run: `cd desktop/webapp && npm run build`
Expected: succeeds; `dist/index.html` produced; no TS errors.

- [ ] **Step 7: Commit**

```bash
git add desktop/webapp/src/components/CurationGrid.tsx desktop/webapp/src/api.ts desktop/webapp/src/api.test.ts desktop/webapp/src/App.tsx
git commit -m "feat: curation grid with thumbnails, multi-select, delete, dedupe, open-folder"
```

---

### Task 6: End-to-end wiring, launcher asset copy, and docs

**Files:**
- Modify: `bow.bat` (already copies `dist`→`web`; verify it still works with the larger build)
- Modify: `README.md` (document the search-scrape + curation workflow)
- Test: full manual run

**Interfaces:** none new — this task integrates and documents.

- [ ] **Step 1: Full rebuild via launcher path**

Run: `cd desktop/webapp && npm run build` then `cd ../src-tauri && cargo build` — both succeed, warning-free.

- [ ] **Step 2: Confirm the web assets land where the server serves them**

Run: `bash -c 'mkdir -p desktop/src-tauri/target/debug/web && cp -r desktop/webapp/dist/* desktop/src-tauri/target/debug/web/ && ls desktop/src-tauri/target/debug/web/index.html'`
Expected: prints the index.html path. (This mirrors what `bow.bat` does.)

- [ ] **Step 3: Manual end-to-end (verification by running)**

Set `BOW_SECRET` to any value in `desktop/.env`. Ensure LM Studio is NOT required (scrape path doesn't use it). Launch `target/debug/bow-desktop.exe`. In the browser at `http://127.0.0.1:9357`:
- Status shows "connected".
- Enter a query (e.g. "red panda"), count 10, a destination under `C:\AI\workspace\`, click Download.
- Live progress shows per-source counts then download progress to 10/10.
- The curation grid appears with ~10 thumbnails. Select a few → Delete selected → count drops and files are gone from disk. Remove duplicates → report note shows. Open folder → Explorer opens the dest.
Document the result (pass/fail per step) in the report. Kill the exe when done.

- [ ] **Step 4: Update README**

Add a "Using the scraper" subsection under the Run section describing: enter query/count/destination → watch progress → curate the grid (select+delete, remove duplicates, open folder). Note destinations must be inside the workspace root (the REST endpoints reject paths outside it).

- [ ] **Step 5: Commit**

```bash
git add README.md bow.bat
git commit -m "docs: document the search-scrape + curation workflow"
```

---

### Task 7: Per-source toggles (filter which scrapers run, end-to-end)

**Files:**
- Modify: `desktop/src-tauri/src/tools/image_search.rs` (add `sources` filter to `image_download` + `source_enabled` helper)
- Modify: `desktop/src-tauri/src/tools/mod.rs` (call site)
- Modify: `desktop/src-tauri/src/server.rs` (`scrape_request` gains `sources`)
- Modify: `desktop/webapp/src/store.ts` (`startScrape` sends `sources`)
- Modify: `desktop/webapp/src/components/SearchPanel.tsx` (checkbox row)
- Test: inline `#[cfg(test)]` in `image_search.rs`

**Interfaces:**
- Produces:
  - `pub async fn image_download(query: &str, count: usize, dest_dir: &str, log_dir: &str, sources: Option<Vec<String>>, progress: Option<UnboundedSender<ScrapeEvent>>) -> Result<String>` — `sources` is `None`/empty → run all scrapers; otherwise run only the named ones. Canonical keys: `bing`, `ddg`, `yandex`, `brave`, `qwant`, `searxng`.
  - `fn source_enabled(sources: &Option<Vec<String>>, key: &str) -> bool`
  - WS inbound `scrape_request` gains optional `sources: [String]`.

- [ ] **Step 1: Write the failing test for `source_enabled`**

Add to the `image_search.rs` test module:

```rust
#[test]
fn source_enabled_filters() {
    assert!(source_enabled(&None, "bing"));
    assert!(source_enabled(&Some(vec![]), "bing"));
    assert!(source_enabled(&Some(vec!["bing".into(), "ddg".into()]), "BING"));
    assert!(!source_enabled(&Some(vec!["ddg".into()]), "bing"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bow-desktop source_enabled_filters --lib`
Expected: FAIL — `source_enabled` not found.

- [ ] **Step 3: Add the helper and thread `sources` through `image_download`**

Add the helper near the other helpers in `image_search.rs`:

```rust
fn source_enabled(sources: &Option<Vec<String>>, key: &str) -> bool {
    match sources {
        None => true,
        Some(list) if list.is_empty() => true,
        Some(list) => list.iter().any(|s| s.eq_ignore_ascii_case(key)),
    }
}
```

Change the `image_download` signature to add `sources: Option<Vec<String>>` immediately before `progress`. Replace the current unconditional `let results: Vec<ScrapeResult> = vec![ ... ];` block with conditional execution:

```rust
    let mut results: Vec<ScrapeResult> = Vec::new();
    if source_enabled(&sources, "bing")    { results.push(scrape_bing_images(&client, query, want).await); }
    if source_enabled(&sources, "ddg")     { results.push(scrape_duckduckgo_images(&client, query, want).await); }
    if source_enabled(&sources, "yandex")  { results.push(scrape_yandex_images(&client, query, want).await); }
    if source_enabled(&sources, "brave")   { results.push(scrape_brave_images(&client, query, want).await); }
    if source_enabled(&sources, "qwant")   { results.push(scrape_qwant_images(&client, query, want).await); }
    if source_enabled(&sources, "searxng") { results.push(scrape_searxng_images(&client, query, want).await); }
```

(Leave the rest of `image_download` — the `for r in &results` loop, filtering, download phase, events — unchanged.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p bow-desktop source_enabled_filters --lib`
Expected: PASS.

- [ ] **Step 5: Update both backend call sites**

- `tools/mod.rs`: the agent tool path passes no filter → `image_download(&query, count, &dest_dir, &log_dir, None, None).await`.
- `server.rs` `ScrapeRequest` arm: add `sources` to the variant and pass it. Change the variant to `ScrapeRequest { query: String, count: u32, dest_dir: String, #[serde(default)] sources: Option<Vec<String>> }`, and in the spawn call: `image_download(&query, count as usize, &dest_dir, &log_dir, sources, Some(tx)).await`. Update the existing `scrape_request_parses` test (Task 2) is unaffected because `sources` defaults; optionally extend it to assert `sources` parses when present.

- [ ] **Step 6: Build + test (backend)**

Run: `cargo build -p bow-desktop` — zero warnings.
Run: `cargo test -p bow-desktop --lib` — all pass.

- [ ] **Step 7: Frontend — send selected sources**

In `store.ts`, change `startScrape` to accept `sources: string[]` and include it:

```ts
  startScrape: (a: { query: string; count: number; destDir: string; sources: string[] }) => {
    const ws = get()._ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: a.count }, lastDestDir: a.destDir });
    ws.send(JSON.stringify({ type: "scrape_request", query: a.query, count: a.count, dest_dir: a.destDir, sources: a.sources }));
  },
```

(Update the `Store` interface's `startScrape` signature to match.)

In `SearchPanel.tsx`, add the source list + checkboxes and pass enabled keys:

```tsx
const ALL_SOURCES = [
  { key: "bing", label: "Bing" }, { key: "ddg", label: "DuckDuckGo" },
  { key: "yandex", label: "Yandex" }, { key: "brave", label: "Brave" },
  { key: "qwant", label: "Qwant" }, { key: "searxng", label: "SearXNG" },
];
```

Inside the component, add state `const [enabled, setEnabled] = useState<Set<string>>(new Set(ALL_SOURCES.map(s => s.key)));`, render a checkbox row, and change the button handler to `startScrape({ query, count, destDir, sources: [...enabled] })`:

```tsx
      <div style={{ display: "flex", gap: 10, flexWrap: "wrap", fontSize: 12, color: "#a8b2d8" }}>
        {ALL_SOURCES.map((s) => (
          <label key={s.key} style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <input type="checkbox" checked={enabled.has(s.key)}
              onChange={(e) => setEnabled((prev) => { const n = new Set(prev); e.target.checked ? n.add(s.key) : n.delete(s.key); return n; })} />
            {s.label}
          </label>
        ))}
      </div>
```

Also include `enabled.size === 0` in the button's `disabled` condition (can't scrape with no sources).

- [ ] **Step 8: Build (frontend)**

Run: `cd desktop/webapp && npm run build` — succeeds, no TS errors.

- [ ] **Step 9: Commit**

```bash
git add desktop/src-tauri/src/tools/image_search.rs desktop/src-tauri/src/tools/mod.rs desktop/src-tauri/src/server.rs desktop/webapp/src/store.ts desktop/webapp/src/components/SearchPanel.tsx
git commit -m "feat: per-source scraper toggles end-to-end"
```

---

## Self-Review

**Spec coverage (Phase 2 scope):**
- Search panel (query, count, destination, per-source toggles) — Tasks 4, 7 ✓
- Live progress over WS — Tasks 1,2,4 ✓
- Curation grid (thumbnails, select, delete, dedupe, open folder) — Tasks 3,5 ✓
- Wired to existing scraper engine (not LLM) — Tasks 1,2 ✓
- Per-source on/off toggles — Task 7 ✓
- Loopback + workspace path guardrails — Task 3 ✓
- Deferred to later phases (correctly absent): controlled-browser/page scrape (Phase 3), AI assist (Phase 4), source repair (Phase 4).

**Source-toggle note:** Task 7 adds the source filter end-to-end (a `sources: Option<Vec<String>>` on `image_download` + WS field + SearchPanel checkboxes). It is sequenced last because Tasks 1–6 establish the all-sources pipeline first; Task 7 inserts the `sources` param before `progress`, so it updates both `image_download` call sites (mod.rs `None`, server.rs the request's `sources`).

**Placeholder scan:** No TBD/TODO. All code blocks are complete and copy-pasteable.

**Type consistency:** `ScrapeEvent` variants ↔ `to_json` `kind` strings ↔ `ScrapeEventMsg` TS union ↔ `applyEvent` switch all use the same kind names (phase/source/candidates/downloaded/failed/done/error). `image_download(query,count,dest_dir,log_dir,progress)` signature is consistent across Tasks 1, 2. `within_workspace(root, candidate) -> Option<PathBuf>` and `HttpState`/`Config::test_default` used consistently in Task 3. REST routes in Task 3 match `api.ts` calls in Task 5.

**Risk callouts (carry into execution):**
- Task 2: confirm the exact name of the outbound frame sender in `run_ws` (the plan assumes `out_tx: mpsc::Sender<String>`); if it differs, use the actual name. Also confirm the scrape arm sits AFTER the auth gate.
- Task 3: `HttpState`/`AppState`/`Config` test constructors must match the real field set at implementation time (Phase 1 may have changed `Config` — e.g. it now returns `token` in `/api/config`). Build `Config::test_default` against the current struct.
- Task 3: `within_workspace` relies on `canonicalize`, which requires the path to exist — fine for listing/thumb/delete/dedupe (paths exist) and for dest dirs created by the scrape; if a not-yet-created dir is ever passed, canonicalize the parent instead.
- `axum::body::to_bytes` signature (Task 3 test) is `to_bytes(body, limit)` in axum 0.7 — adjust if the resolved version differs.
