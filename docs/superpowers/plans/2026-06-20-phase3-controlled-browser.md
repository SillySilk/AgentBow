# Bow Image Studio — Phase 3: Controlled Browser & Page Scraping — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the backend its own controlled Chrome (via chromiumoxide) with a persistent login profile, repoint the dead extension-relay browser tools onto it, and add a "scrape this page" flow that extracts images from a page/gallery (including auth-walled, JS-heavy sites) into the existing download + curation pipeline.

**Architecture:** A new `ControlledBrowser` subsystem owns a single long-lived chromiumoxide `Browser` (launched with a persistent `--user-data-dir` under the workspace; headful so the user can log in once) plus its driver `Handler` task. The legacy `browser_*` agent tools are repointed from `BrowserBridge` (WS→extension, now dead) to `ControlledBrowser`. The download phase of `image_download` is extracted into a shared `download_urls_to_dir` so both search-scrape (Phase 2) and page-scrape reuse it. A new WS `page_scrape_request` navigates/extracts image URLs from the controlled browser's current page and streams the same `scrape_event`s as Phase 2; a PageScrapePanel drives it and feeds the existing CurationGrid.

**Tech Stack:** Rust (chromiumoxide for CDP, tokio, the existing reqwest/image stack), React + TypeScript (reusing Phase 2's store/grid).

## ⚠️ Verification reality (read before executing)
chromiumoxide drives a REAL Chrome process. The live behavior (launch, login, navigation, scroll-to-load, image extraction) cannot be verified headless by an automated agent the way Phases 1–2 were. Tasks below TDD the pure seams (URL normalization/filtering, the shared download refactor, schema/dispatch wiring, frontend reducer/api) and use `#[ignore]`-marked integration tests for anything that needs a live Chrome. The full page-scrape flow MUST be verified by a human with Chrome installed. Do not claim live-browser behavior "passes" from an automated run.

## Global Constraints
- Platform: Windows 11. Chrome/Chromium must be installed; discover its path (see Task 1).
- Bind 127.0.0.1 only; single port 9357; local-LLM-only (page-scrape is user-driven, no LLM).
- Fix all compiler warnings; pristine test output.
- Persistent browser profile lives under `config.workspace_root\.bow_browser_profile` (created on demand).
- Page-scrape destinations are workspace-guarded exactly like Phase 2 (reuse `web_api::resolve_within_workspace`).
- Preserve existing WS protocol shapes; only ADD new messages (`browser_open`, `page_scrape_request`) and reuse the existing `scrape_event` stream.
- Reuse existing engine code (the Phase-2 download/curation pipeline); do not duplicate the download loop.
- The `browser_get_bookmarks` tool is REMOVED (no CDP equivalent; it was a Chrome-extension API).

## File Structure
- Create `desktop/src-tauri/src/tools/controlled_browser.rs` — owns the chromiumoxide Browser + handler task + current-page handle; methods mirroring the repointed tools plus `extract_image_urls`.
- Modify `desktop/src-tauri/src/tools/image_search.rs` — extract `download_urls_to_dir(...)`; `image_download` calls it.
- Modify `desktop/src-tauri/src/tools/mod.rs` — repoint the `browser_*` dispatch to `ControlledBrowser`; drop `browser_get_bookmarks`.
- Modify `desktop/src-tauri/src/server.rs` — hold a shared `ControlledBrowser`; handle `browser_open` + `page_scrape_request`; pass `ControlledBrowser` to the tool dispatcher instead of `BrowserBridge`.
- Delete `desktop/src-tauri/src/tools/browser.rs` (`BrowserBridge`) once nothing references it (keep its `distill_html`/`truncate_text` helpers by moving them to `controlled_browser.rs` or `util.rs`).
- Modify `desktop/src-tauri/Cargo.toml` — add `chromiumoxide`.
- Create `desktop/webapp/src/components/PageScrapePanel.tsx`; modify `store.ts`, `App.tsx`.

---

### Task 1: chromiumoxide dependency + ControlledBrowser launch/lifecycle

**Files:**
- Modify: `desktop/src-tauri/Cargo.toml`
- Create: `desktop/src-tauri/src/tools/controlled_browser.rs`
- Modify: `desktop/src-tauri/src/tools/mod.rs` (add `pub mod controlled_browser;`)
- Test: inline `#[cfg(test)]` in `controlled_browser.rs`

**Interfaces:**
- Produces:
  - `pub fn chrome_executable() -> Option<std::path::PathBuf>` — finds a Chrome/Edge binary (checks `CHROME_PATH` env, then common Windows install paths). Pure-ish (filesystem existence checks) and unit-testable for the env-override branch.
  - `#[derive(Clone)] pub struct ControlledBrowser { inner: Arc<tokio::sync::Mutex<Option<BrowserState>>>, profile_dir: PathBuf, exe: Option<PathBuf> }`
  - `pub fn new(profile_dir: PathBuf) -> ControlledBrowser` — does NOT launch; lazy.
  - `pub async fn ensure_launched(&self, headless: bool) -> anyhow::Result<()>` — launches Chrome with `--user-data-dir=<profile_dir>` if not already running; spawns the handler task; stores the `Browser` + a first `Page`.
  - `pub async fn is_running(&self) -> bool`

- [ ] **Step 1: Add the dependency**

Run from `desktop/src-tauri/`:
```bash
cargo add chromiumoxide --features tokio-runtime
```
If `cargo add` hits the SChannel cert issue, set `CARGO_HTTP_CHECK_REVOKE=false` for that one command only (a prior task hit this). Confirm `chromiumoxide` is in `Cargo.toml`. NOTE: chromiumoxide's exact API/version varies — the code below uses the common 0.5/0.7 shape (`Browser::launch(BrowserConfig)`, a `Handler` stream you must poll in a spawned task, `browser.new_page(url)`, `page.goto`, `page.content()`, `page.evaluate`, `page.find_element`). Adapt method names/signatures to the resolved version and note adaptations in your report.

- [ ] **Step 2: Write the failing test (env-override branch of chrome_executable)**

Create `controlled_browser.rs` with the function stub + test:

```rust
use std::path::PathBuf;

pub fn chrome_executable() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() { return Some(pb); }
    }
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chrome_executable_honors_env_override() {
        // Point CHROME_PATH at a file we know exists (this test binary itself).
        let me = std::env::current_exe().unwrap();
        std::env::set_var("CHROME_PATH", &me);
        assert_eq!(chrome_executable(), Some(me));
        std::env::remove_var("CHROME_PATH");
    }
}
```

- [ ] **Step 3: Run test to verify it fails, then add module decl**

Add `pub mod controlled_browser;` to `tools/mod.rs`.
Run: `cargo test -p bow-desktop chrome_executable_honors_env_override --lib`
Expected: compile-fails until module is declared, then PASS.

- [ ] **Step 4: Implement the launch/lifecycle**

Append to `controlled_browser.rs`:

```rust
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;

struct BrowserState {
    browser: Browser,
    page: Page,
    _handler: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub struct ControlledBrowser {
    inner: Arc<Mutex<Option<BrowserState>>>,
    profile_dir: PathBuf,
}

impl ControlledBrowser {
    pub fn new(profile_dir: PathBuf) -> Self {
        ControlledBrowser { inner: Arc::new(Mutex::new(None)), profile_dir }
    }

    pub async fn is_running(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    /// Launch Chrome with the persistent profile if not already running.
    pub async fn ensure_launched(&self, headless: bool) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if guard.is_some() { return Ok(()); }

        let exe = chrome_executable()
            .ok_or_else(|| anyhow!("No Chrome/Edge found. Set CHROME_PATH in .env to the chrome.exe path."))?;
        std::fs::create_dir_all(&self.profile_dir).ok();

        let mut cfg = BrowserConfig::builder()
            .chrome_executable(exe)
            .user_data_dir(self.profile_dir.clone());
        if !headless { cfg = cfg.with_head(); }
        let cfg = cfg.build().map_err(|e| anyhow!("BrowserConfig: {}", e))?;

        let (browser, mut handler) = Browser::launch(cfg).await
            .map_err(|e| anyhow!("Chrome launch failed: {}", e))?;
        // The handler stream MUST be polled for the browser to function.
        let handler_task = tokio::spawn(async move {
            while let Some(_event) = handler.next().await {}
        });
        let page = browser.new_page("about:blank").await
            .map_err(|e| anyhow!("new_page: {}", e))?;

        *guard = Some(BrowserState { browser, page, _handler: handler_task });
        Ok(())
    }

    /// Internal: run a closure with the current page, erroring if not launched.
    async fn with_page<F, Fut, T>(&self, f: F) -> Result<T>
    where F: FnOnce(Page) -> Fut, Fut: std::future::Future<Output = Result<T>> {
        let guard = self.inner.lock().await;
        let st = guard.as_ref().ok_or_else(|| anyhow!("Browser not launched — call browser_open first"))?;
        let page = st.page.clone();
        drop(guard);
        f(page).await
    }
}
```

- [ ] **Step 5: Build (warning-free) + run the env test**

Run: `cargo build -p bow-desktop` — Expected: compiles (adapt chromiumoxide API if needed), zero warnings.
Run: `cargo test -p bow-desktop chrome_executable_honors_env_override --lib` — Expected: PASS.

- [ ] **Step 6: Add an `#[ignore]` live launch test**

Add (it requires real Chrome, so it is ignored by default):

```rust
#[tokio::test]
#[ignore = "requires a real Chrome install; run manually with --ignored"]
async fn launches_and_navigates_live() {
    let dir = std::env::temp_dir().join("bow_cb_live");
    let cb = ControlledBrowser::new(dir);
    cb.ensure_launched(true).await.expect("launch");
    assert!(cb.is_running().await);
}
```

- [ ] **Step 7: Commit**

```bash
git add desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock desktop/src-tauri/src/tools/controlled_browser.rs desktop/src-tauri/src/tools/mod.rs
git commit -m "feat: ControlledBrowser launch/lifecycle via chromiumoxide with persistent profile"
```

---

### Task 2: Navigation, page reading, scroll, and image-URL extraction

**Files:**
- Modify: `desktop/src-tauri/src/tools/controlled_browser.rs`
- Move into it (from `browser.rs`): `distill_html`, `simple_strip_html`, `remove_tag_block`, `truncate_text` helpers (or move them to `util.rs` and import). Keep their existing tests.
- Test: inline `#[cfg(test)]` (pure URL-normalization tests + the moved distill tests)

**Interfaces:**
- Produces (on `ControlledBrowser`):
  - `pub async fn navigate(&self, url: &str) -> Result<Value>` → `"Navigated to <final url>"`
  - `pub async fn get_url(&self) -> Result<Value>` → `{ url, title }`
  - `pub async fn read_page(&self, mode: &str) -> Result<Value>` → `{ url, title, content }` (mode text/html/links; html distilled, text truncated — same shape as the old BrowserBridge)
  - `pub async fn scroll(&self, target: &str, pixels: i64) -> Result<Value>`
  - `pub async fn extract_image_urls(&self) -> Result<Vec<String>>` — collects absolute image URLs from the current page (`img[src]`, `img[srcset]` largest, `a[href]` ending in an image extension, CSS `background-image`).
  - Pure helper `pub fn normalize_image_urls(raw: Vec<String>, base: &str) -> Vec<String>` — resolves relative URLs against `base`, dedupes, keeps only `http(s)` image-looking URLs, drops `data:` URIs. **This is the unit-tested seam.**

- [ ] **Step 1: Write the failing test for `normalize_image_urls`**

```rust
#[test]
fn normalize_resolves_dedupes_and_filters() {
    let raw = vec![
        "https://e.com/a.jpg".to_string(),
        "https://e.com/a.jpg".to_string(),     // dup
        "/img/b.png".to_string(),               // relative
        "data:image/png;base64,xxxx".to_string(), // data URI dropped
        "https://e.com/script.js".to_string(),  // non-image dropped
    ];
    let out = normalize_image_urls(raw, "https://e.com/gallery/");
    assert_eq!(out, vec![
        "https://e.com/a.jpg".to_string(),
        "https://e.com/img/b.png".to_string(),
    ]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bow-desktop normalize_resolves_dedupes_and_filters --lib`
Expected: FAIL — `normalize_image_urls` not found.

- [ ] **Step 3: Implement `normalize_image_urls`**

```rust
use url::Url;

const IMG_EXTS: &[&str] = &["jpg","jpeg","png","gif","webp","bmp","tif","tiff","avif"];

pub fn normalize_image_urls(raw: Vec<String>, base: &str) -> Vec<String> {
    let base_url = Url::parse(base).ok();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let r = r.trim();
        if r.is_empty() || r.starts_with("data:") { continue; }
        let abs = if r.starts_with("http") {
            r.to_string()
        } else if let Some(b) = &base_url {
            match b.join(r) { Ok(u) => u.to_string(), Err(_) => continue }
        } else { continue };
        let lower = abs.split('?').next().unwrap_or(&abs).to_lowercase();
        let looks_img = IMG_EXTS.iter().any(|e| lower.ends_with(&format!(".{}", e)));
        if !looks_img { continue; }
        if seen.insert(abs.clone()) { out.push(abs); }
    }
    out
}
```

(`url` crate is already a transitive dep via reqwest; if it's not directly usable, `cargo add url`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bow-desktop normalize_resolves_dedupes_and_filters --lib`
Expected: PASS.

- [ ] **Step 5: Implement the live page methods**

Add these methods (using `with_page`). Adapt chromiumoxide calls to the resolved API:

```rust
use serde_json::{json, Value};

impl ControlledBrowser {
    pub async fn navigate(&self, url: &str) -> Result<Value> {
        self.ensure_launched(false).await?;
        let u = url.to_string();
        self.with_page(|page| async move {
            page.goto(&u).await.map_err(|e| anyhow!("goto: {}", e))?;
            page.wait_for_navigation().await.ok();
            let final_url = page.url().await.ok().flatten().unwrap_or(u);
            Ok(json!(format!("Navigated to {}", final_url)))
        }).await
    }

    pub async fn get_url(&self) -> Result<Value> {
        self.with_page(|page| async move {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url, "title": title }))
        }).await
    }

    pub async fn read_page(&self, mode: &str) -> Result<Value> {
        let mode = mode.to_string();
        self.with_page(|page| async move {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            let html = page.content().await.map_err(|e| anyhow!("content: {}", e))?;
            let content = match mode.as_str() {
                "html" => distill_html(&html),
                "links" => {
                    // anchors via JS evaluate
                    let v: Value = page.evaluate(
                        "JSON.stringify(Array.from(document.querySelectorAll('a[href]')).map(a=>({text:a.innerText.trim().slice(0,100),href:a.href})).filter(l=>l.text&&l.href))"
                    ).await.ok().and_then(|r| r.into_value().ok()).unwrap_or(Value::Null);
                    v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())
                }
                _ => truncate_text(&distill_html(&html), 8000),
            };
            Ok(json!({ "url": url, "title": title, "content": content }))
        }).await
    }

    pub async fn scroll(&self, target: &str, pixels: i64) -> Result<Value> {
        let js = match target {
            "top" => "window.scrollTo(0,0)".to_string(),
            "bottom" => "window.scrollTo(0,document.body.scrollHeight)".to_string(),
            "up" => format!("window.scrollBy(0,-{})", pixels),
            "down" => format!("window.scrollBy(0,{})", pixels),
            sel => format!("document.querySelector({:?})?.scrollIntoView({{behavior:'smooth',block:'center'}})", sel),
        };
        self.with_page(|page| async move {
            page.evaluate(js).await.map_err(|e| anyhow!("scroll: {}", e))?;
            Ok(json!(format!("Scrolled: {}", target)))
        }).await
    }

    pub async fn extract_image_urls(&self) -> Result<Vec<String>> {
        self.ensure_launched(false).await?;
        self.with_page(|page| async move {
            let base = page.url().await.ok().flatten().unwrap_or_default();
            let raw: Value = page.evaluate(r#"
                JSON.stringify((() => {
                  const out = [];
                  document.querySelectorAll('img').forEach(im => {
                    if (im.currentSrc) out.push(im.currentSrc);
                    else if (im.src) out.push(im.src);
                    if (im.srcset) im.srcset.split(',').forEach(s => out.push(s.trim().split(' ')[0]));
                  });
                  document.querySelectorAll('a[href]').forEach(a => out.push(a.href));
                  return out;
                })())
            "#).await.ok().and_then(|r| r.into_value().ok()).unwrap_or(Value::Null);
            let list: Vec<String> = raw.as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .or_else(|| serde_json::from_value(raw.clone()).ok())
                .unwrap_or_default();
            Ok(normalize_image_urls(list, &base))
        }).await
    }
}
```

Move `distill_html`/`simple_strip_html`/`remove_tag_block`/`truncate_text` (and their tests) from `browser.rs` into this module (or `util.rs`); fix imports. They are still needed by `read_page`.

- [ ] **Step 6: Build + tests**

Run: `cargo build -p bow-desktop` — zero warnings.
Run: `cargo test -p bow-desktop --lib` — all pass (the normalize test + the moved distill tests).

- [ ] **Step 7: Commit**

```bash
git add desktop/src-tauri/src/tools/controlled_browser.rs desktop/src-tauri/src/tools/browser.rs desktop/src-tauri/src/util.rs
git commit -m "feat: controlled-browser navigation, page reading, scroll, image-url extraction"
```

---

### Task 3: Interaction methods (click, fill, screenshot, exec_js, cookies, history) + drop bookmarks

**Files:**
- Modify: `desktop/src-tauri/src/tools/controlled_browser.rs`
- Test: inline (only where a pure seam exists; live methods are exercised by the `#[ignore]` integration test)

**Interfaces:**
- Produces on `ControlledBrowser` (same Value shapes the old `BrowserBridge` returned so the dispatcher needs no shape changes):
  - `click(&self, selector) -> Result<Value>`, `fill(&self, selector, value, submit) -> Result<Value>`,
  - `screenshot(&self) -> Result<Value>` (returns the same image+text content array shape as before),
  - `exec_js(&self, js) -> Result<Value>`, `get_cookies(&self, url)`, `set_cookie(&self, params)`, `delete_cookies(&self, url, name)`,
  - `back(&self)`, `forward(&self)`, `reload(&self, bypass)`.
  - `analyze_page(&self)` — screenshot + read_page("text") combined (same shape as before).

- [ ] **Step 1: Implement the methods**

Add to `controlled_browser.rs` (adapt chromiumoxide API as needed):

```rust
impl ControlledBrowser {
    pub async fn click(&self, selector: &str) -> Result<Value> {
        let sel = selector.to_string();
        self.with_page(|page| async move {
            let el = page.find_element(&sel).await.map_err(|_| anyhow!("Element not found: {}", sel))?;
            el.click().await.map_err(|e| anyhow!("click: {}", e))?;
            Ok(json!(format!("Clicked: {}", sel)))
        }).await
    }

    pub async fn fill(&self, selector: &str, value: &str, submit: bool) -> Result<Value> {
        let (sel, val) = (selector.to_string(), value.to_string());
        self.with_page(|page| async move {
            let el = page.find_element(&sel).await.map_err(|_| anyhow!("Element not found: {}", sel))?;
            el.click().await.ok();
            el.type_str(&val).await.map_err(|e| anyhow!("type: {}", e))?;
            if submit { el.press_key("Enter").await.ok(); }
            Ok(json!(format!("Filled: {}", sel)))
        }).await
    }

    pub async fn exec_js(&self, js: &str) -> Result<Value> {
        let js = js.to_string();
        self.with_page(|page| async move {
            let r = page.evaluate(js).await.map_err(|e| anyhow!("eval: {}", e))?;
            Ok(json!(r.into_value::<Value>().unwrap_or(Value::Null)))
        }).await
    }

    pub async fn screenshot(&self) -> Result<Value> {
        self.with_page(|page| async move {
            let bytes = page.screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
                .await.map_err(|e| anyhow!("screenshot: {}", e))?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(json!([
                { "type":"image","source":{"type":"base64","media_type":"image/png","data": b64} },
                { "type":"text","text":"Screenshot of current page." }
            ]))
        }).await
    }

    pub async fn reload(&self, _bypass: bool) -> Result<Value> {
        self.with_page(|page| async move {
            page.reload().await.map_err(|e| anyhow!("reload: {}", e))?;
            let url = page.url().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url }))
        }).await
    }

    pub async fn analyze_page(&self) -> Result<Value> {
        let (shot, page) = tokio::join!(self.screenshot(), self.read_page("text"));
        let b64 = shot.ok().and_then(|v| v.as_array()?.first()?["source"]["data"].as_str().map(|s| s.to_string()));
        let (url, title, text) = match page {
            Ok(v) => (v["url"].as_str().unwrap_or("").into(), v["title"].as_str().unwrap_or("").into(), v["content"].as_str().unwrap_or("").into()),
            Err(_) => (String::new(), String::new(), String::new()),
        };
        let mut out = json!({ "url": url, "title": title, "text_content": text });
        if let Some(b) = b64 { out["screenshot_base64"] = json!(b); }
        Ok(out)
    }
}
```

For `back`, `forward`, `get_cookies`, `set_cookie`, `delete_cookies`: implement via `page.evaluate("history.back()")` / `history.forward()` for history, and CDP `Network`/`Storage` cookie commands for cookies. If a clean chromiumoxide cookie API isn't available in the resolved version, return `Ok(json!("not supported in controlled browser"))` for the cookie methods rather than failing the build, and note it as a limitation in your report (cookies are a low priority for image scraping; the persistent profile already carries login state).

- [ ] **Step 2: Build (warning-free)**

Run: `cargo build -p bow-desktop`
Expected: compiles, zero warnings. (No new unit test here — these are live methods; the `#[ignore]` integration test in Task 8 exercises them. If you add any pure helper, test it.)

- [ ] **Step 3: Commit**

```bash
git add desktop/src-tauri/src/tools/controlled_browser.rs
git commit -m "feat: controlled-browser interaction methods (click/fill/screenshot/exec_js/history/cookies)"
```

---

### Task 4: Extract a shared `download_urls_to_dir` from `image_download`

**Files:**
- Modify: `desktop/src-tauri/src/tools/image_search.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub async fn download_urls_to_dir(urls: Vec<String>, count: usize, dest_dir: &str, name_hint: &str, log: &mut SessionLog, progress: &Option<UnboundedSender<ScrapeEvent>>) -> Result<Vec<String>>` — the download phase (filter paid CDNs, concurrent download pool, magic-byte validation, emit `Downloaded`/`Failed` events) extracted verbatim from the back half of `image_download`. Returns the list of downloaded file paths.
  - `image_download` is refactored to call `download_urls_to_dir` for its download phase (behavior unchanged).
- Consumes: existing `is_paywalled_url`, `download_image_bytes`, `sanitize_filename`, `ScrapeEvent`, `SessionLog`.

- [ ] **Step 1: Write a test for the candidate-filtering seam**

The download itself needs network, but the paid-CDN filter + sanitize are pure. Add a small pure helper used by `download_urls_to_dir` and test it:

```rust
pub fn filter_candidates(urls: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .map(|u| u.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""))
        .filter(|u| !is_paywalled_url(u))
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

#[test]
fn filter_candidates_drops_paywalled_and_dedupes() {
    let out = filter_candidates(vec![
        "https://e.com/a.jpg".into(),
        "https://e.com/a.jpg".into(),
        "https://media.gettyimages.com/x.jpg".into(),
    ]);
    assert_eq!(out, vec!["https://e.com/a.jpg".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails, implement `filter_candidates`, verify it passes**

Run: `cargo test -p bow-desktop filter_candidates_drops_paywalled_and_dedupes --lib` → FAIL then (after adding the fn) PASS.

- [ ] **Step 3: Extract `download_urls_to_dir`**

Move the download phase of `image_download` (from the `-- Downloading --` log line through building `downloaded`/`failures` and the per-file events) into `download_urls_to_dir`, using `filter_candidates` for the dedup/paywall/entity step. `image_download` keeps its scraping phase, then calls `download_urls_to_dir(candidates, count, dest_dir, query, &mut log, &progress)`. Keep the emitted events and the final summary string identical.

- [ ] **Step 4: Build + full test**

Run: `cargo build -p bow-desktop` — zero warnings.
Run: `cargo test -p bow-desktop --lib` — all pass (existing scrape-event/source tests still green, new filter test green).

- [ ] **Step 5: Commit**

```bash
git add desktop/src-tauri/src/tools/image_search.rs
git commit -m "refactor: extract download_urls_to_dir shared by search- and page-scrape"
```

---

### Task 5: Repoint the `browser_*` tool dispatch to ControlledBrowser; remove BrowserBridge

**Files:**
- Modify: `desktop/src-tauri/src/tools/mod.rs` (dispatcher signature + arms; drop `browser_get_bookmarks` schema + arm)
- Modify: `desktop/src-tauri/src/server.rs` (construct a shared `ControlledBrowser`; pass it to the dispatcher; remove `BrowserBridge` construction and the `browser_result` pending-resolution block that fed it)
- Delete: `desktop/src-tauri/src/tools/browser.rs` (after helpers moved in Task 2)
- Modify: `desktop/src-tauri/src/local_llm.rs` (the dispatch call passes `ControlledBrowser`)
- Test: build + the existing suite

**Interfaces:**
- Consumes: `ControlledBrowser` methods from Tasks 2–3 (same Value shapes as the old `BrowserBridge`).
- Produces: the tool dispatcher's `browser` parameter is now `&crate::tools::controlled_browser::ControlledBrowser`. Tool names unchanged EXCEPT `browser_get_bookmarks` is removed; tab tools (`browser_tab_*`) are removed too (a single controlled page has no multi-tab model) — remove their schemas and arms.

- [ ] **Step 1: Update the dispatcher**

In `mod.rs`: change the dispatcher's `browser: &browser::BrowserBridge` param to `browser: &crate::tools::controlled_browser::ControlledBrowser`. Keep the arms for screenshot/exec_js/navigate/back/forward/reload/get_cookies/set_cookie/delete_cookies/read_page/click/fill/scroll/get_url/analyze_page (method names match). REMOVE the arms and schema entries for `browser_tab_list`, `browser_tab_new`, `browser_tab_close`, `browser_tab_switch`, and `browser_get_bookmarks`. Update the top `pub mod browser;` → remove once the file is deleted.

- [ ] **Step 2: Update server.rs**

In `run_ws`: replace `let browser = crate::tools::browser::BrowserBridge::new(out_tx.clone());` with a shared `ControlledBrowser` obtained from `AppState` (see Step 3). REMOVE the `browser_result` early-skip/resolution block in the message loop (no extension sends `browser_result` anymore) and remove `browser_result` from `classify`'s Skip set. Pass the `ControlledBrowser` into `local_llm`'s dispatch where `browser` was passed.

- [ ] **Step 3: Hold ControlledBrowser in AppState**

In `state.rs`, add a `pub controlled_browser: crate::tools::controlled_browser::ControlledBrowser` field to `AppState`, constructed in `AppState::new` as `ControlledBrowser::new(config.workspace_root.join(".bow_browser_profile"))`. Update `Config::test_default`/`AppState` test constructors accordingly (the browser is lazy — `new` does not launch, so tests are unaffected).

- [ ] **Step 4: Delete browser.rs and fix references**

Delete `desktop/src-tauri/src/tools/browser.rs`. Remove `pub mod browser;` from `mod.rs`. Ensure the distill/truncate helpers were moved (Task 2) so nothing dangles. `grep` the crate for `BrowserBridge` / `tools::browser` and fix every reference.

- [ ] **Step 5: Build + tests**

Run: `cargo build -p bow-desktop` — zero warnings (fix all).
Run: `cargo test -p bow-desktop --lib` — all pass.

- [ ] **Step 6: Commit**

```bash
git add -A desktop/src-tauri/src
git commit -m "refactor: repoint browser_* tools to ControlledBrowser; remove extension BrowserBridge"
```

---

### Task 6: WS `browser_open` + `page_scrape_request` (streamed page scrape)

**Files:**
- Modify: `desktop/src-tauri/src/server.rs`
- Test: inline parse tests

**Interfaces:**
- Consumes: `ControlledBrowser` (navigate, scroll, extract_image_urls), `image_search::download_urls_to_dir`, `web_api::resolve_within_workspace`, `ScrapeEvent`.
- Produces WS additions:
  - Inbound `{ "type":"browser_open", "url": String }` — launches the controlled browser headful and navigates to `url` (so the user can log in / reach a gallery). Replies `{ "type":"browser_opened", "url": <final> }` or an error event.
  - Inbound `{ "type":"page_scrape_request", "count": u32, "dest_dir": String, "scrolls": u32 }` — scrolls the current page `scrolls` times (to trigger lazy-load), extracts image URLs, then downloads up to `count` into `dest_dir` (workspace-guarded), streaming the SAME `scrape_event` frames as Phase 2 (so the existing ProgressLog + CurationGrid just work).

- [ ] **Step 1: Write the failing parse tests**

```rust
#[test]
fn browser_open_and_page_scrape_parse() {
    let a: InboundMsg = serde_json::from_value(serde_json::json!({"type":"browser_open","url":"https://x"})).unwrap();
    matches!(a, InboundMsg::BrowserOpen { .. });
    let b: InboundMsg = serde_json::from_value(serde_json::json!({"type":"page_scrape_request","count":20,"dest_dir":"C:\\x","scrolls":3})).unwrap();
    match b { InboundMsg::PageScrapeRequest { count, scrolls, .. } => { assert_eq!(count,20); assert_eq!(scrolls,3); }, _ => panic!() }
}
```

- [ ] **Step 2: Run → fail; add variants → pass**

Add to `InboundMsg`:
```rust
    BrowserOpen { url: String },
    PageScrapeRequest { count: u32, dest_dir: String, #[serde(default)] scrolls: u32 },
```
Run: `cargo test -p bow-desktop browser_open_and_page_scrape_parse --lib` → PASS.

- [ ] **Step 3: Handle `BrowserOpen` (after auth gate)**

```rust
                    InboundMsg::BrowserOpen { url } => {
                        let cb = state_browser.clone(); // ControlledBrowser handle in run_ws
                        let out_tx = out_tx.clone();
                        tokio::spawn(async move {
                            let msg = match cb.navigate(&url).await {
                                Ok(_) => serde_json::json!({"type":"browser_opened","url": url}),
                                Err(e) => serde_json::json!({"type":"scrape_event","kind":"error","message": format!("browser_open: {}", e)}),
                            };
                            let _ = out_tx.send(msg.to_string()).await;
                        });
                    }
```
(`navigate` calls `ensure_launched(false)` → headful.)

- [ ] **Step 4: Handle `PageScrapeRequest` (after auth gate)**

```rust
                    InboundMsg::PageScrapeRequest { count, dest_dir, scrolls } => {
                        let cb = state_browser.clone();
                        let out_tx = out_tx.clone();
                        let workspace = config.workspace_root.clone();
                        let log_dir = format!("{}\\logs", workspace.to_string_lossy().trim_end_matches(['\\','/']));
                        let count = (count as usize).clamp(1, 500);
                        tokio::spawn(async move {
                            let dest = match crate::web_api::resolve_within_workspace(&workspace, &dest_dir) {
                                Some(p) => p.to_string_lossy().to_string(),
                                None => { let _ = out_tx.send(serde_json::json!({"type":"scrape_event","kind":"error","message":"dest_dir outside workspace"}).to_string()).await; return; }
                            };
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            let fwd = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    let mut v = ev.to_json(); v["type"] = serde_json::Value::String("scrape_event".into());
                                    let _ = fwd.send(v.to_string()).await;
                                }
                            });
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Scrolling page".into() });
                            for _ in 0..scrolls { let _ = cb.scroll("down", 1200).await; tokio::time::sleep(std::time::Duration::from_millis(700)).await; }
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Extracting images".into() });
                            let urls = cb.extract_image_urls().await.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Candidates { total: urls.len(), filtered: 0 });
                            let mut log = crate::tools::image_search::SessionLog::new(&log_dir, "page_scrape");
                            let result = crate::tools::image_search::download_urls_to_dir(urls, count, &dest, "page", &mut log, &Some(tx.clone())).await;
                            let log_note = log.flush();
                            let downloaded = result.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Done { downloaded, log_note });
                            drop(tx);
                            let _ = forwarder.await;
                        });
                    }
```
NOTE: `SessionLog` and `download_urls_to_dir` must be `pub` (Task 4). `state_browser` is the `ControlledBrowser` you wired into `run_ws` in Task 5 — use its actual binding name.

- [ ] **Step 5: Build + tests**

Run: `cargo build -p bow-desktop` — zero warnings.
Run: `cargo test -p bow-desktop --lib` — all pass.

- [ ] **Step 6: Commit**

```bash
git add desktop/src-tauri/src/server.rs desktop/src-tauri/src/tools/image_search.rs
git commit -m "feat: WS browser_open + streamed page_scrape_request"
```

---

### Task 7: Frontend — PageScrapePanel

**Files:**
- Modify: `desktop/webapp/src/store.ts` (add `openBrowser(url)` + `pageScrape({count,destDir,scrolls})` actions; handle `browser_opened`)
- Create: `desktop/webapp/src/components/PageScrapePanel.tsx`
- Modify: `desktop/webapp/src/App.tsx`
- Test: `desktop/webapp/src/store.test.ts` (extend)

**Interfaces:**
- Consumes: WS `browser_open` / `browser_opened` / `page_scrape_request` (+ the existing `scrape_event` stream the store already reduces).
- Produces: store actions `openBrowser(url: string)` and `pageScrape(a: {count:number; destDir:string; scrolls:number})`; a `browserUrl` status field set on `browser_opened`.

- [ ] **Step 1: Extend the reducer test**

The page-scrape reuses `applyEvent` (same `scrape_event` kinds), so the reducer needs no change — assert that a `browser_opened` message is handled by the store's message router (add a store-level field). Add a focused test for a tiny pure helper `isBrowserOpened(msg)`:

```ts
import { isBrowserOpened } from "./store";
it("detects browser_opened", () => {
  expect(isBrowserOpened({ type: "browser_opened", url: "https://x" })).toBe(true);
  expect(isBrowserOpened({ type: "scrape_event", kind: "done", downloaded: [], log_note: "" })).toBe(false);
});
```

- [ ] **Step 2: Run → fail; implement → pass**

In `store.ts` add `export function isBrowserOpened(m: any): boolean { return m?.type === "browser_opened"; }`, route it in `onmessage` to `set({ browserUrl: m.url, lastDestDir: get().lastDestDir })`, add `browserUrl?: string` to the store, and add the two actions:

```ts
  openBrowser: (url: string) => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "browser_open", url })); },
  pageScrape: ({ count, destDir, scrolls }) => {
    const ws = get()._ws; if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: count }, lastDestDir: destDir });
    ws.send(JSON.stringify({ type: "page_scrape_request", count, dest_dir: destDir, scrolls }));
  },
```

Run: `cd desktop/webapp && npx vitest run store.test.ts` → PASS.

- [ ] **Step 3: Build PageScrapePanel**

Create `desktop/webapp/src/components/PageScrapePanel.tsx`:

```tsx
import { useState } from "react";
import { useStore } from "../store";

export default function PageScrapePanel() {
  const openBrowser = useStore((s) => s.openBrowser);
  const pageScrape = useStore((s) => s.pageScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [url, setUrl] = useState("");
  const [count, setCount] = useState(30);
  const [scrolls, setScrolls] = useState(5);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");
  const ready = status === "connected";
  return (
    <div style={{ display: "grid", gap: 8, maxWidth: 560, marginTop: 24, borderTop: "1px solid #2a2a4a", paddingTop: 16 }}>
      <strong style={{ color: "#a8b2d8" }}>Scrape a page / gallery</strong>
      <div style={{ display: "flex", gap: 8 }}>
        <input placeholder="Page URL (log in / navigate first)" value={url} onChange={(e) => setUrl(e.target.value)} style={inp} />
        <button disabled={!ready || !url.trim()} onClick={() => openBrowser(url)} style={btn2}>Open browser</button>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <input type="number" min={1} max={500} value={count} onChange={(e) => setCount(Math.max(1, Math.min(500, Number(e.target.value) || 1)))} style={{ ...inp, width: 80 }} title="max images" />
        <input type="number" min={0} max={50} value={scrolls} onChange={(e) => setScrolls(Math.max(0, Math.min(50, Number(e.target.value) || 0)))} style={{ ...inp, width: 80 }} title="scroll passes" />
        <input placeholder="Destination folder" value={destDir} onChange={(e) => setDestDir(e.target.value)} style={{ ...inp, flex: 1 }} />
      </div>
      <button disabled={!ready || running || !destDir.trim()} onClick={() => pageScrape({ count, destDir, scrolls })} style={btn}>
        {running ? "Scraping…" : "Scrape images from current page"}
      </button>
    </div>
  );
}
const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
const btn2: React.CSSProperties = { background: "#0f3460", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 12px", cursor: "pointer" };
```

Add `<PageScrapePanel />` to `App.tsx` below `<SearchPanel />` (import it). The existing `<ProgressLog />` and `<CurationGrid />` already react to the shared scrape state, so page-scrape progress and results render automatically.

- [ ] **Step 4: Build**

Run: `cd desktop/webapp && npm run build` — succeeds, no TS errors.

- [ ] **Step 5: Commit**

```bash
git add desktop/webapp/src/store.ts desktop/webapp/src/store.test.ts desktop/webapp/src/components/PageScrapePanel.tsx desktop/webapp/src/App.tsx
git commit -m "feat: page-scrape panel (open browser, scrape current page) reusing curation grid"
```

---

### Task 8: Live integration verification + docs

**Files:**
- Modify: `README.md`
- Test: the `#[ignore]` live tests + a documented manual checklist

**Interfaces:** none new — integration + docs.

- [ ] **Step 1: Build the whole app**

Run: `cd desktop/webapp && npm run build` then `cd ../src-tauri && cargo build` — both succeed, warning-free.

- [ ] **Step 2: Run the ignored live test (requires Chrome)**

Run: `cargo test -p bow-desktop --lib -- --ignored launches_and_navigates_live`
Expected (on a machine with Chrome / `CHROME_PATH` set): PASS. If no Chrome is available in the execution environment, record that it could not be run and leave it for the human.

- [ ] **Step 3: Manual checklist (human, with Chrome)**

Document in the report (cannot be automated): launch `bow.bat`; in the UI, paste a gallery URL → Open browser (a Chrome window opens with the persistent profile; log in if needed) → set count/scrolls/destination → Scrape images from current page → progress streams → curation grid fills → delete/dedupe/open-folder work. Verify the profile persists a login across a second run.

- [ ] **Step 4: README**

Add a "Scrape a page or gallery" subsection: open browser to a URL (log into auth-walled sites once — the profile under `.bow_browser_profile` persists), navigate to the gallery, set scroll passes for lazy-loaded grids, then scrape into a workspace folder; results appear in the same curation grid. Note `CHROME_PATH` in `.env` if Chrome isn't auto-detected. Note the controlled browser is separate from your everyday browser.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document controlled-browser page/gallery scraping"
```

---

## Self-Review

**Spec coverage (Phase 3 scope):**
- Controlled browser via chromiumoxide + persistent profile — Task 1 ✓
- Navigate / read / scroll / extract images — Task 2 ✓
- Interaction (click/fill/screenshot/etc.) — Task 3 ✓
- Repoint legacy browser tools to the controlled browser — Task 5 ✓
- Page-scrape feeds the same download + curation pipeline — Tasks 4, 6, 7 ✓
- Page-scrape UI — Task 7 ✓
- Workspace guard on page-scrape dest — Task 6 ✓
- Deferred to Phase 4 (correctly absent): AI assist panel; scraper-source repair.

**Placeholder scan:** No TBD/TODO. chromiumoxide method calls are flagged as version-adaptable (like Phase 1's tray-icon) — the integration shape is fixed, exact signatures adapt to the resolved crate. Live-browser behavior is verified via `#[ignore]` tests + a human checklist, not false automated "passes."

**Type consistency:** `ControlledBrowser` method names/Value shapes match what `mod.rs`'s dispatcher calls (screenshot/exec_js/navigate/back/forward/reload/get_cookies/set_cookie/delete_cookies/read_page/click/fill/scroll/get_url/analyze_page). `download_urls_to_dir`/`SessionLog`/`ScrapeEvent` signatures consistent across Tasks 4 and 6. `normalize_image_urls`/`filter_candidates` are the unit-tested pure seams. WS `scrape_event` reused so the Phase-2 store/grid need no reducer change.

**Risks (carry into execution):**
- chromiumoxide API drift is the biggest unknown — version-pin and adapt the page/element/screenshot calls; budget time for it.
- Removing `browser_tab_*` and `browser_get_bookmarks` changes the tool surface — confirm nothing else references them.
- Cookie methods may be stubbed if the resolved chromiumoxide lacks a clean cookie API (documented fallback in Task 3).
- The whole live path needs a human + Chrome to verify; do not merge claiming the page-scrape flow is verified without that.
