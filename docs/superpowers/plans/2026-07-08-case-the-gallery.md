# Case the Gallery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a guided "Case the gallery" flow — extract structured candidates from the live controlled browser, let the user demonstrate a pick (thumbnail → detail image), generalize it to all siblings, download the batch, and optionally save it as a reusable per-domain playbook. Also fix the page-scrape 0/100 bug.

**Architecture:** A pure `recipe` module (Candidate/Recipe types, selector generalization, playbook JSON store) with no browser/LLM deps. `controlled_browser.rs` gains `extract_candidates()` (reads lazy attrs, records CSS paths, no extension filter). `server.rs` gains `case_*` WS messages that stream the existing `scrape_event` shape and reuse `download_urls_to_dir`. A new React `CasePanel` drives the two-click demo + batch approve.

**Tech Stack:** Rust (axum WS, chromiumoxide CDP, serde, url), React + TypeScript + Zustand + Vitest.

## Global Constraints

- Local LLM only — no Anthropic/cloud calls. Casing must work with the engine **off** (pure DOM, no LLM in v1).
- Fix all compiler/linter warnings, even non-fatal.
- All playbook file I/O goes through `resolve_within_workspace(&workspace_root, ..)` (path guard).
- Reuse the existing `ScrapeEvent` stream + `download_urls_to_dir` pipeline + `scrape_cancel` stop; do not fork a parallel download path.
- Candidate/Recipe selector patterns are compared by **structural equality** (all `:nth-of-type(k)` indices stripped) — the v1 generalization heuristic.

---

### Task 1: `recipe` module — types, structural pattern, generalize

**Files:**
- Create: `desktop/src-tauri/src/tools/recipe.rs`
- Modify: `desktop/src-tauri/src/tools/mod.rs` (add `pub mod recipe;`)

**Interfaces:**
- Produces:
  - `struct Candidate { id: usize, preview_url: String, href: Option<String>, selector: String, w: u32, h: u32 }` (derives `Debug, Clone, Serialize, Deserialize, PartialEq`)
  - `struct Recipe { domain: String, grid_selector: String, link_selector: Option<String>, detail_image_selector: Option<String>, scrolls: u32 }` (same derives)
  - `fn structural_pattern(selector: &str) -> String`
  - `fn generalize(example: &Candidate, all: &[Candidate], scrolls: u32, domain: &str) -> (Recipe, Vec<Candidate>)`
  - `fn detail_selector_from(c: &Candidate) -> String`
  - `fn match_pattern<'a>(pattern: &str, all: &'a [Candidate]) -> Vec<&'a Candidate>`
  - `fn domain_of(url: &str) -> String`

- [ ] **Step 1: Write failing tests**

Add to `recipe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: usize, sel: &str, href: Option<&str>, w: u32, h: u32) -> Candidate {
        Candidate { id, preview_url: format!("https://e.com/{}.jpg", id), href: href.map(|s| s.to_string()), selector: sel.into(), w, h }
    }

    #[test]
    fn structural_pattern_strips_indices() {
        assert_eq!(structural_pattern("div#g > div:nth-of-type(3) > a > img"), "div#g > div > a > img");
        assert_eq!(structural_pattern("ul > li:nth-of-type(12)"), "ul > li");
        assert_eq!(structural_pattern("img"), "img");
    }

    #[test]
    fn generalize_collects_siblings_and_marks_links() {
        let all = vec![
            c(0, "div#g > div:nth-of-type(1) > a > img", Some("https://e.com/p/1"), 100, 100),
            c(1, "div#g > div:nth-of-type(2) > a > img", Some("https://e.com/p/2"), 100, 100),
            c(2, "header > a > img", None, 20, 20), // chrome, different pattern
        ];
        let (recipe, sibs) = generalize(&all[0], &all, 4, "e.com");
        assert_eq!(recipe.grid_selector, "div#g > div > a > img");
        assert_eq!(recipe.link_selector.as_deref(), Some("div#g > div > a > img"));
        assert_eq!(recipe.scrolls, 4);
        assert_eq!(sibs.len(), 2);
    }

    #[test]
    fn generalize_without_href_has_no_link_selector() {
        let all = vec![c(0, "main > img:nth-of-type(1)", None, 800, 600)];
        let (recipe, sibs) = generalize(&all[0], &all, 0, "e.com");
        assert!(recipe.link_selector.is_none());
        assert_eq!(sibs.len(), 1);
    }

    #[test]
    fn domain_of_strips_www() {
        assert_eq!(domain_of("https://www.Example.com/a/b?x=1"), "example.com");
        assert_eq!(domain_of("https://gallery.example.net/db5"), "gallery.example.net");
        assert_eq!(domain_of("not a url"), "unknown");
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cd "desktop/src-tauri" && cargo test recipe:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function structural_pattern` etc.

- [ ] **Step 3: Implement the module**

Top of `recipe.rs`:

```rust
use serde::{Deserialize, Serialize};

/// One extractable element found on a page. `selector` is the CSS path of the
/// *repeating unit* — the wrapping `<a href>` when the image is a link, else the
/// `<img>` itself. `preview_url` is the best thumbnail URL (absolute).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: usize,
    pub preview_url: String,
    pub href: Option<String>,
    pub selector: String,
    pub w: u32,
    pub h: u32,
}

/// A reusable, URL-independent per-domain extraction recipe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub domain: String,
    pub grid_selector: String,
    /// `Some` ⇒ grid items are links; follow each item's `href` to a detail page.
    pub link_selector: Option<String>,
    /// Structural pattern of the full-size image on a detail page.
    pub detail_image_selector: Option<String>,
    #[serde(default)]
    pub scrolls: u32,
}

/// Strip every `:nth-of-type(k)` index from a CSS path, yielding its structural
/// pattern — the v1 generalization key. Pure string scan (no regex dep).
pub fn structural_pattern(selector: &str) -> String {
    let mut out = String::with_capacity(selector.len());
    let mut i = 0;
    let bytes = selector.as_bytes();
    let needle = ":nth-of-type(";
    while i < selector.len() {
        if selector[i..].starts_with(needle) {
            // skip to the closing ')'
            if let Some(close) = selector[i..].find(')') {
                i += close + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Candidates whose structural pattern equals `pattern`.
pub fn match_pattern<'a>(pattern: &str, all: &'a [Candidate]) -> Vec<&'a Candidate> {
    all.iter().filter(|c| structural_pattern(&c.selector) == pattern).collect()
}

/// Build a recipe + sibling set from one demonstrated example.
pub fn generalize(example: &Candidate, all: &[Candidate], scrolls: u32, domain: &str) -> (Recipe, Vec<Candidate>) {
    let grid_selector = structural_pattern(&example.selector);
    let siblings: Vec<Candidate> = match_pattern(&grid_selector, all).into_iter().cloned().collect();
    let link_selector = example.href.as_ref().map(|_| grid_selector.clone());
    let recipe = Recipe {
        domain: domain.to_string(),
        grid_selector,
        link_selector,
        detail_image_selector: None,
        scrolls,
    };
    (recipe, siblings)
}

/// Structural pattern of a detail-page image the user clicked.
pub fn detail_selector_from(c: &Candidate) -> String {
    structural_pattern(&c.selector)
}

/// Registrable-ish domain (host, lowercased, `www.` stripped) or "unknown".
pub fn domain_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => u.host_str().map(|h| h.trim_start_matches("www.").to_lowercase()).unwrap_or_else(|| "unknown".into()),
        Err(_) => "unknown".into(),
    }
}
```

Add to `desktop/src-tauri/src/tools/mod.rs`: `pub mod recipe;`

- [ ] **Step 4: Run tests, verify pass**

Run: `cd "desktop/src-tauri" && cargo test recipe:: 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add desktop/src-tauri/src/tools/recipe.rs desktop/src-tauri/src/tools/mod.rs
git commit -m "feat: recipe module — Candidate/Recipe types + selector generalize"
```

---

### Task 2: `recipe` module — playbook JSON store

**Files:**
- Modify: `desktop/src-tauri/src/tools/recipe.rs`

**Interfaces:**
- Consumes: `Recipe`, `domain_of` (Task 1)
- Produces:
  - `fn playbook_file(dir: &Path, domain: &str) -> PathBuf`
  - `fn save_playbook(dir: &Path, recipe: &Recipe) -> std::io::Result<()>` (appends to the domain file, de-duped by grid_selector)
  - `fn load_playbooks(dir: &Path, domain: &str) -> Vec<Recipe>`

- [ ] **Step 1: Write failing tests** (append to `recipe.rs` tests module)

```rust
    #[test]
    fn playbook_round_trips_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("bow_pb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = Recipe { domain: "e.com".into(), grid_selector: "div > a > img".into(),
            link_selector: Some("div > a > img".into()), detail_image_selector: Some("main > img".into()), scrolls: 3 };
        save_playbook(&dir, &r).unwrap();
        save_playbook(&dir, &r).unwrap(); // same grid_selector ⇒ no duplicate
        let loaded = load_playbooks(&dir, "e.com");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], r);
        assert!(load_playbooks(&dir, "other.com").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cd "desktop/src-tauri" && cargo test recipe::tests::playbook 2>&1 | tail -20`
Expected: FAIL — `cannot find function save_playbook`.

- [ ] **Step 3: Implement** (add near top of `recipe.rs`, after imports)

```rust
use std::path::{Path, PathBuf};

fn sanitize_domain(domain: &str) -> String {
    domain.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' { ch } else { '_' }).collect()
}

pub fn playbook_file(dir: &Path, domain: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize_domain(domain)))
}

pub fn load_playbooks(dir: &Path, domain: &str) -> Vec<Recipe> {
    let path = playbook_file(dir, domain);
    std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str::<Vec<Recipe>>(&s).ok())
        .unwrap_or_default()
}

/// Append `recipe` to its domain file, replacing any existing recipe with the
/// same `grid_selector` (so re-saving an updated demo overwrites cleanly).
pub fn save_playbook(dir: &Path, recipe: &Recipe) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut all = load_playbooks(dir, &recipe.domain);
    all.retain(|r| r.grid_selector != recipe.grid_selector);
    all.push(recipe.clone());
    let json = serde_json::to_string_pretty(&all).unwrap_or_else(|_| "[]".into());
    std::fs::write(playbook_file(dir, &recipe.domain), json)
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cd "desktop/src-tauri" && cargo test recipe:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add desktop/src-tauri/src/tools/recipe.rs
git commit -m "feat: recipe playbook JSON store (save/load per domain)"
```

---

### Task 3: `extract_candidates()` + fix page-scrape 0/100

**Files:**
- Modify: `desktop/src-tauri/src/tools/controlled_browser.rs`

**Interfaces:**
- Consumes: `crate::tools::recipe::Candidate` (Task 1)
- Produces:
  - `pub async fn extract_candidates(&self) -> Result<Vec<Candidate>>` on `ControlledBrowser`
  - `pub fn resolve_candidate_urls(raw: Vec<RawCandidate>, base: &str) -> Vec<Candidate>` (pure)
  - `struct RawCandidate { preview_url: String, href: Option<String>, selector: String, w: u32, h: u32 }` (Deserialize)
- Also: broaden `extract_image_urls()`'s JS to read `data-src`/`data-original`/`data-lazy`, and change `normalize_image_urls` gate so extensionless `http(s)` image-host URLs are kept (fixes 0/100).

- [ ] **Step 1: Write failing pure tests** (in `controlled_browser.rs` tests module)

```rust
    #[test]
    fn resolve_candidate_urls_absolutizes_and_assigns_ids() {
        let raw = vec![
            RawCandidate { preview_url: "/img/a".into(), href: Some("/p/1".into()), selector: "div > a:nth-of-type(1) > img".into(), w: 100, h: 90 },
            RawCandidate { preview_url: "data:image/png;base64,xx".into(), href: None, selector: "img".into(), w: 1, h: 1 }, // dropped
            RawCandidate { preview_url: "https://cdn.e.com/x".into(), href: None, selector: "img:nth-of-type(2)".into(), w: 50, h: 50 },
        ];
        let out = resolve_candidate_urls(raw, "https://e.com/gallery/");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 0);
        assert_eq!(out[0].preview_url, "https://e.com/img/a");
        assert_eq!(out[0].href.as_deref(), Some("https://e.com/p/1"));
        assert_eq!(out[1].id, 1);
        assert_eq!(out[1].preview_url, "https://cdn.e.com/x");
    }

    #[test]
    fn normalize_keeps_extensionless_image_hosts() {
        let raw = vec![
            "https://cdn.e.com/image/12345".to_string(),      // extensionless, kept
            "https://e.com/page.html".to_string(),            // .html dropped
            "https://e.com/a.jpg".to_string(),                // kept
        ];
        let out = normalize_image_urls(raw, "https://e.com/");
        assert!(out.contains(&"https://cdn.e.com/image/12345".to_string()));
        assert!(out.contains(&"https://e.com/a.jpg".to_string()));
        assert!(!out.iter().any(|u| u.ends_with("page.html")));
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cd "desktop/src-tauri" && cargo test controlled_browser:: 2>&1 | tail -25`
Expected: FAIL — `cannot find type RawCandidate` and `normalize_keeps_extensionless_image_hosts` assertion fails.

- [ ] **Step 3: Implement**

Add near the top of `controlled_browser.rs` (after the `use` block):

```rust
use crate::tools::recipe::Candidate;

/// Raw shape the page-extraction JS returns (URLs not yet absolutized).
#[derive(serde::Deserialize)]
pub struct RawCandidate {
    pub preview_url: String,
    pub href: Option<String>,
    pub selector: String,
    #[serde(default)] pub w: u32,
    #[serde(default)] pub h: u32,
}

/// Absolutize preview_url/href against `base`, drop `data:`/non-http previews,
/// and assign stable ids. Pure & unit-tested.
pub fn resolve_candidate_urls(raw: Vec<RawCandidate>, base: &str) -> Vec<Candidate> {
    let base_url = Url::parse(base).ok();
    let abs = |s: &str| -> Option<String> {
        let s = s.trim();
        if s.is_empty() || s.starts_with("data:") { return None; }
        if s.starts_with("http") { return Some(s.to_string()); }
        base_url.as_ref().and_then(|b| b.join(s).ok()).map(|u| u.to_string())
    };
    let mut out = Vec::new();
    for r in raw {
        let Some(preview_url) = abs(&r.preview_url) else { continue };
        let href = r.href.as_deref().and_then(abs);
        out.push(Candidate { id: out.len(), preview_url, href, selector: r.selector, w: r.w, h: r.h });
    }
    out
}
```

Add the method on `impl ControlledBrowser` (near `extract_image_urls`):

```rust
    /// Extract structured candidates (img + wrapping link) from the live page,
    /// reading lazy attributes and recording each repeating unit's CSS path.
    pub async fn extract_candidates(&self) -> Result<Vec<Candidate>> {
        self.ensure_launched(false).await?;
        self.with_page(|page| async move {
            let base = page.url().await.ok().flatten().unwrap_or_default();
            let expr = r#"
                JSON.stringify((() => {
                  function cssPath(el) {
                    const parts = [];
                    while (el && el.nodeType === 1 && parts.length < 8) {
                      let seg = el.tagName.toLowerCase();
                      if (el.id) { parts.unshift(seg + '#' + el.id); break; }
                      const p = el.parentElement;
                      if (p) {
                        const same = Array.from(p.children).filter(c => c.tagName === el.tagName);
                        if (same.length > 1) seg += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
                      }
                      parts.unshift(seg);
                      el = el.parentElement;
                    }
                    return parts.join(' > ');
                  }
                  function pick(im) {
                    return im.getAttribute('data-src') || im.getAttribute('data-original')
                      || im.getAttribute('data-lazy') || im.currentSrc || im.src
                      || (im.srcset ? im.srcset.split(',')[0].trim().split(' ')[0] : '');
                  }
                  const out = [];
                  document.querySelectorAll('img').forEach(im => {
                    const preview = pick(im);
                    if (!preview) return;
                    const a = im.closest('a[href]');
                    const unit = a || im;
                    out.push({ preview_url: preview, href: a ? a.href : null,
                      selector: cssPath(unit), w: im.naturalWidth || im.width || 0, h: im.naturalHeight || im.height || 0 });
                  });
                  return out;
                })())
            "#.to_string();
            let raw: Value = page.evaluate(expr).await.ok().and_then(|r| r.into_value().ok()).unwrap_or(Value::Null);
            let list: Vec<RawCandidate> = raw.as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            Ok(resolve_candidate_urls(list, &base))
        })
        .await
    }
```

Fix `extract_image_urls` JS `img` loop to also read lazy attrs — replace the `document.querySelectorAll('img').forEach(...)` block with:

```rust
                  document.querySelectorAll('img').forEach(im => {
                    const u = im.getAttribute('data-src') || im.getAttribute('data-original') || im.currentSrc || im.src;
                    if (u) out.push(u);
                    if (im.srcset) im.srcset.split(',').forEach(s => out.push(s.trim().split(' ')[0]));
                  });
```

Broaden `normalize_image_urls`: replace the `looks_img` gate so extensionless URLs on image-ish hosts/paths are kept. Change:

```rust
        let lower = abs.split('?').next().unwrap_or(&abs).to_lowercase();
        let looks_img = IMG_EXTS.iter().any(|e| lower.ends_with(&format!(".{}", e)));
        if !looks_img {
            continue;
        }
```
to:
```rust
        let lower = abs.split('?').next().unwrap_or(&abs).to_lowercase();
        let has_ext = lower.rsplit('/').next().unwrap_or("").contains('.');
        let looks_img = IMG_EXTS.iter().any(|e| lower.ends_with(&format!(".{}", e)));
        // Keep known image extensions, OR extensionless paths (CDN image routes like
        // /image/12345). Reject only URLs whose last path segment has a *non-image*
        // extension (.html, .js, .css…).
        let bad_ext = has_ext && !looks_img;
        if bad_ext {
            continue;
        }
```

- [ ] **Step 4: Run, verify pass**

Run: `cd "desktop/src-tauri" && cargo test controlled_browser:: 2>&1 | tail -25`
Expected: PASS (existing + 2 new). The existing `normalize_resolves_dedupes_and_filters` test still passes (`.js` has a bad ext ⇒ dropped; `data:` dropped).

- [ ] **Step 5: Commit**

```bash
git add desktop/src-tauri/src/tools/controlled_browser.rs
git commit -m "feat: extract_candidates + keep extensionless image URLs (fix page-scrape 0/100)"
```

---

### Task 4: `case_*` WS messages in `server.rs`

**Files:**
- Modify: `desktop/src-tauri/src/server.rs`

**Interfaces:**
- Consumes: `recipe::{Candidate, Recipe, generalize, detail_selector_from, domain_of, structural_pattern, match_pattern, save_playbook, load_playbooks}`, `controlled_browser::extract_candidates`, `image_search::{download_urls_to_dir, DownloadOpts, SessionLog, ScrapeEvent, pick_auto_bin}`, `web_api::resolve_within_workspace`.
- Produces WS inbound variants: `CaseExtract`, `CaseOpenDetail { candidate_id_href: String }`, `CaseGeneralize { recipe: Recipe, /*already built client-side*/ }` — see design note below — plus `CaseRun { recipe: Recipe, grid_url: String, count: u32, dest_dir: String }`, `PlaybookSave { recipe: Recipe }`, `PlaybookList { domain: String }`.

**Design note on where generalize runs:** to keep `Candidate` ids meaningful across messages without server-side session state, the **server** holds the last extracted candidate list per connection in a local `let mut last_candidates: Vec<Candidate>` and `let mut last_grid_url: String` (declared alongside `history`). `CaseGeneralize { example_id, detail_image_id }` uses those. This avoids shipping the full candidate list back and forth.

- [ ] **Step 1: Add the enum variants + parse test**

Add to `InboundMsg`:

```rust
    CaseExtract,
    CaseOpenDetail { href: String },
    CaseGeneralize { example_id: usize, #[serde(default)] detail_image_id: Option<usize> },
    CaseRun { recipe: crate::tools::recipe::Recipe, grid_url: String, count: u32, dest_dir: String },
    PlaybookSave { recipe: crate::tools::recipe::Recipe },
    PlaybookList { domain: String },
```

Add test in `server.rs` tests module:

```rust
    #[test]
    fn case_messages_parse() {
        let a: InboundMsg = serde_json::from_value(json!({"type":"case_extract"})).unwrap();
        assert!(matches!(a, InboundMsg::CaseExtract));
        let b: InboundMsg = serde_json::from_value(json!({"type":"case_generalize","example_id":3})).unwrap();
        assert!(matches!(b, InboundMsg::CaseGeneralize { example_id: 3, detail_image_id: None }));
        let c: InboundMsg = serde_json::from_value(json!({"type":"case_run","grid_url":"https://e.com/g","count":20,"dest_dir":"C:\\x",
            "recipe":{"domain":"e.com","grid_selector":"div > a > img","link_selector":"div > a > img","detail_image_selector":"main > img","scrolls":3}})).unwrap();
        assert!(matches!(c, InboundMsg::CaseRun { count: 20, .. }));
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cd "desktop/src-tauri" && cargo test server::tests::case_messages 2>&1 | tail -20`
Expected: FAIL — variants don't exist.

- [ ] **Step 3: Add connection-scoped state + handlers**

Near `let mut history` (~line 88) add:

```rust
    let mut last_candidates: Vec<crate::tools::recipe::Candidate> = Vec::new();
    let mut last_grid_url: String = String::new();
```

Add match arms after `InboundMsg::PageScrapeRequest { .. } => { .. }` (all guarded by `if !authenticated { … continue; }` like siblings). Use these helpers inline:

```rust
                    InboundMsg::CaseExtract => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        match controlled_browser.extract_candidates().await {
                            Ok(cands) => {
                                last_grid_url = controlled_browser.get_url().await.ok()
                                    .and_then(|v| v["url"].as_str().map(str::to_string)).unwrap_or_default();
                                last_candidates = cands.clone();
                                send_json(&out_tx, json!({"type":"case_candidates","stage":"grid","items":cands})).await;
                            }
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("case_extract: {} (open the browser first with Ghost car)", e)})).await,
                        }
                    }

                    InboundMsg::CaseOpenDetail { href } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        if let Err(e) = controlled_browser.navigate(&href).await {
                            send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("open detail: {}", e)})).await;
                            continue;
                        }
                        match controlled_browser.extract_candidates().await {
                            Ok(cands) => { last_candidates = cands.clone(); send_json(&out_tx, json!({"type":"case_candidates","stage":"detail","items":cands})).await; }
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("detail extract: {}", e)})).await,
                        }
                    }

                    InboundMsg::CaseGeneralize { example_id, detail_image_id } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        // detail_image_id (when present) refers to the *current* last_candidates (detail page).
                        let detail_sel = detail_image_id.and_then(|id| last_candidates.iter().find(|c| c.id == id))
                            .map(crate::tools::recipe::detail_selector_from);
                        // example_id refers to the grid candidates — which may have been
                        // overwritten by a detail extract; the client re-sends the grid set is avoided
                        // by keeping example resolution on the client. Here we resolve against whatever
                        // grid set the client last generalized from: the client sends example_id valid
                        // for the grid stage, so we look it up in the stored grid candidates.
                        let domain = crate::tools::recipe::domain_of(&last_grid_url);
                        // Re-extract the grid so example_id resolves against a fresh grid set.
                        let grid = if last_grid_url.is_empty() { last_candidates.clone() } else {
                            let _ = controlled_browser.navigate(&last_grid_url).await;
                            controlled_browser.extract_candidates().await.unwrap_or_default()
                        };
                        match grid.iter().find(|c| c.id == example_id).cloned() {
                            Some(example) => {
                                let (mut recipe, sibs) = crate::tools::recipe::generalize(&example, &grid, 0, &domain);
                                recipe.detail_image_selector = detail_sel;
                                last_candidates = grid;
                                send_json(&out_tx, json!({"type":"case_recipe","recipe":recipe,"matched":sibs.len(),"total":last_candidates.len(),"grid_url":last_grid_url})).await;
                            }
                            None => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message":"example not found — re-run Case it"})).await,
                        }
                    }

                    InboundMsg::PlaybookSave { recipe } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let dir = config.workspace_root.join("playbooks");
                        match crate::tools::recipe::save_playbook(&dir, &recipe) {
                            Ok(_) => send_json(&out_tx, json!({"type":"playbook_saved","domain":recipe.domain})).await,
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("save playbook: {}", e)})).await,
                        }
                    }

                    InboundMsg::PlaybookList { domain } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let dir = config.workspace_root.join("playbooks");
                        let recipes = crate::tools::recipe::load_playbooks(&dir, &domain);
                        send_json(&out_tx, json!({"type":"playbook_list","domain":domain,"recipes":recipes})).await;
                    }

                    InboundMsg::CaseRun { recipe, grid_url, count, dest_dir } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let cb = controlled_browser.clone();
                        let out_tx = out_tx.clone();
                        let workspace = config.workspace_root.clone();
                        let log_dir = format!("{}\\logs", workspace.to_string_lossy().trim_end_matches(['\\', '/']));
                        let count = (count as usize).clamp(1, 500);
                        scrape_cancel.store(false, Ordering::Relaxed);
                        let cancel = scrape_cancel.clone();
                        tokio::spawn(async move {
                            let dest = match crate::web_api::resolve_within_workspace(&workspace, &dest_dir) {
                                Some(p) => p.to_string_lossy().to_string(),
                                None => { let _ = out_tx.send(json!({"type":"scrape_event","kind":"error","message":"dest_dir outside workspace"}).to_string()).await; return; }
                            };
                            let dest = match crate::tools::image_search::pick_auto_bin(&dest) {
                                Ok(p) => p, Err(e) => { let _ = out_tx.send(json!({"type":"scrape_event","kind":"error","message": format!("set folder: {}", e)}).to_string()).await; return; }
                            };
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            let fwd = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await { let mut v = ev.to_json(); v["type"] = Value::String("scrape_event".into()); let _ = fwd.send(v.to_string()).await; }
                            });
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Casing gallery".into() });
                            let _ = cb.navigate(&grid_url).await;
                            for _ in 0..recipe.scrolls { let _ = cb.scroll("down", 1200).await; tokio::time::sleep(std::time::Duration::from_millis(700)).await; }
                            let grid = cb.extract_candidates().await.unwrap_or_default();
                            let items: Vec<crate::tools::recipe::Candidate> =
                                crate::tools::recipe::match_pattern(&recipe.grid_selector, &grid).into_iter().cloned().collect();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Candidates { total: items.len(), filtered: grid.len().saturating_sub(items.len()) });

                            let mut urls: Vec<String> = Vec::new();
                            if recipe.link_selector.is_some() {
                                let detail_sel = recipe.detail_image_selector.clone().unwrap_or_default();
                                for it in &items {
                                    if crate::tools::image_search::_cancel_check(&Some(cancel.clone())) { break; }
                                    let Some(href) = &it.href else { continue };
                                    if cb.navigate(href).await.is_err() { continue; }
                                    let dcands = cb.extract_candidates().await.unwrap_or_default();
                                    // largest candidate matching the detail image pattern
                                    let best = dcands.iter()
                                        .filter(|c| detail_sel.is_empty() || crate::tools::recipe::structural_pattern(&c.selector) == detail_sel)
                                        .max_by_key(|c| c.w as u64 * c.h as u64);
                                    if let Some(b) = best { urls.push(b.preview_url.clone()); }
                                }
                            } else {
                                urls.extend(items.iter().map(|c| c.preview_url.clone()));
                            }

                            let mut log = crate::tools::image_search::SessionLog::new(&log_dir, "case_run");
                            let result = crate::tools::image_search::download_urls_to_dir(
                                urls, count, &dest, "case", crate::tools::image_search::DownloadOpts::default(),
                                &mut log, &Some(tx.clone()), Some(cancel),
                            ).await;
                            let log_note = log.flush();
                            let downloaded = result.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Done { downloaded, log_note, dest_dir: dest.clone() });
                            drop(tx);
                            let _ = forwarder.await;
                        });
                    }
```

Add a tiny public cancel-check shim to `image_search.rs` (its `is_cancelled` is private):

```rust
/// Public wrapper over the private cancel check, for the case-run loop in server.rs.
pub fn _cancel_check(cancel: &CancelFlag) -> bool { is_cancelled(cancel) }
```

- [ ] **Step 4: Run, verify pass + build**

Run: `cd "desktop/src-tauri" && cargo test server:: 2>&1 | tail -20 && cargo build 2>&1 | tail -15`
Expected: parse test PASS; build succeeds with **no warnings**.

- [ ] **Step 5: Commit**

```bash
git add desktop/src-tauri/src/server.rs desktop/src-tauri/src/tools/image_search.rs
git commit -m "feat: case_* WS messages — extract/open-detail/generalize/run + playbook save/list"
```

---

### Task 5: Frontend store + api wiring

**Files:**
- Modify: `desktop/webapp/src/store.ts`, `desktop/webapp/src/store.test.ts`

**Interfaces:**
- Produces on the store: `caseExtract()`, `caseOpenDetail(href)`, `caseGeneralize(exampleId, detailImageId?)`, `caseRun(recipe, gridUrl, count, destDir)`, `playbookSave(recipe)`, `playbookList(domain)`, and a `caseState` slice: `{ stage: "idle"|"grid"|"detail"|"recipe"; candidates: Candidate[]; recipe: Recipe|null; matched: number; total: number; gridUrl: string; playbooks: Recipe[] }`.
- Types `Candidate` and `Recipe` mirror the Rust structs.

- [ ] **Step 1: Write failing test** (append to `store.test.ts`)

```ts
import { applyCaseEvent, initialCaseState } from "./store";

describe("applyCaseEvent", () => {
  it("stores grid candidates", () => {
    let s = initialCaseState();
    s = applyCaseEvent(s, { type: "case_candidates", stage: "grid", items: [
      { id: 0, preview_url: "https://e/a.jpg", href: "https://e/p/1", selector: "div > a > img", w: 100, h: 100 },
    ]});
    expect(s.stage).toBe("grid");
    expect(s.candidates.length).toBe(1);
  });
  it("stores recipe + match counts", () => {
    let s = initialCaseState();
    s = applyCaseEvent(s, { type: "case_recipe", recipe: { domain: "e", grid_selector: "div > a > img", link_selector: "div > a > img", detail_image_selector: "main > img", scrolls: 0 }, matched: 42, total: 60, grid_url: "https://e/g" });
    expect(s.stage).toBe("recipe");
    expect(s.matched).toBe(42);
    expect(s.recipe?.grid_selector).toBe("div > a > img");
    expect(s.gridUrl).toBe("https://e/g");
  });
});
```

- [ ] **Step 2: Run, verify fail**

Run: `cd desktop/webapp && npx vitest run src/store.test.ts 2>&1 | tail -20`
Expected: FAIL — `applyCaseEvent` not exported.

- [ ] **Step 3: Implement** — add to `store.ts` (before the `Store` interface):

```ts
export interface Candidate { id: number; preview_url: string; href: string | null; selector: string; w: number; h: number }
export interface Recipe { domain: string; grid_selector: string; link_selector: string | null; detail_image_selector: string | null; scrolls: number }

export type CaseEventMsg =
  | { type: "case_candidates"; stage: "grid" | "detail"; items: Candidate[] }
  | { type: "case_recipe"; recipe: Recipe; matched: number; total: number; grid_url: string }
  | { type: "playbook_list"; domain: string; recipes: Recipe[] }
  | { type: "playbook_saved"; domain: string };

export interface CaseState {
  stage: "idle" | "grid" | "detail" | "recipe";
  candidates: Candidate[];
  recipe: Recipe | null;
  matched: number; total: number;
  gridUrl: string;
  playbooks: Recipe[];
}
export function initialCaseState(): CaseState {
  return { stage: "idle", candidates: [], recipe: null, matched: 0, total: 0, gridUrl: "", playbooks: [] };
}
export function applyCaseEvent(s: CaseState, m: CaseEventMsg): CaseState {
  switch (m.type) {
    case "case_candidates": return { ...s, stage: m.stage, candidates: m.items };
    case "case_recipe": return { ...s, stage: "recipe", recipe: m.recipe, matched: m.matched, total: m.total, gridUrl: m.grid_url };
    case "playbook_list": return { ...s, playbooks: m.recipes };
    case "playbook_saved": return s;
    default: return s;
  }
}
```

Add `caseState: CaseState` to the store state + `caseState: initialCaseState()` in the initializer, wire messages in `ws.onmessage` (after the `scrape_event` branch):

```ts
        else if (m.type === "case_candidates" || m.type === "case_recipe" || m.type === "playbook_list" || m.type === "playbook_saved")
          set((st) => ({ caseState: applyCaseEvent(st.caseState, m) }));
```

Add the actions to the store object:

```ts
  caseExtract: () => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "case_extract" })); },
  caseOpenDetail: (href: string) => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "case_open_detail", href })); },
  caseGeneralize: (exampleId: number, detailImageId?: number) => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "case_generalize", example_id: exampleId, detail_image_id: detailImageId ?? null })); },
  caseRun: (recipe: Recipe, gridUrl: string, count: number, destDir: string) => {
    const ws = get()._ws; if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: count }, lastDestDir: destDir });
    ws.send(JSON.stringify({ type: "case_run", recipe, grid_url: gridUrl, count, dest_dir: destDir }));
  },
  playbookSave: (recipe: Recipe) => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "playbook_save", recipe })); },
  playbookList: (domain: string) => { const ws = get()._ws; if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "playbook_list", domain })); },
```

Add matching signatures to the `Store` interface.

- [ ] **Step 4: Run, verify pass**

Run: `cd desktop/webapp && npx vitest run src/store.test.ts 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add desktop/webapp/src/store.ts desktop/webapp/src/store.test.ts
git commit -m "feat: case store slice + actions (extract/open-detail/generalize/run, playbooks)"
```

---

### Task 6: `CasePanel` UI + wire into PageScrapePanel

**Files:**
- Create: `desktop/webapp/src/components/CasePanel.tsx`
- Modify: `desktop/webapp/src/components/PageScrapePanel.tsx` (add a **Case it** button + render `<CasePanel/>`)

**Interfaces:**
- Consumes store: `caseState`, `caseExtract`, `caseOpenDetail`, `caseGeneralize`, `caseRun`, `playbookSave`, `playbookList`, `status`.

- [ ] **Step 1: Implement `CasePanel.tsx`** (no unit test — presentational; covered by manual verify)

```tsx
import { useState } from "react";
import { useStore } from "../store";
import type { Candidate } from "../store";
import Button from "./ui/Button";

export default function CasePanel({ destDir, count }: { destDir: string; count: number }) {
  const cs = useStore((s) => s.caseState);
  const caseExtract = useStore((s) => s.caseExtract);
  const caseOpenDetail = useStore((s) => s.caseOpenDetail);
  const caseGeneralize = useStore((s) => s.caseGeneralize);
  const caseRun = useStore((s) => s.caseRun);
  const playbookSave = useStore((s) => s.playbookSave);
  const status = useStore((s) => s.status);
  const ready = status === "connected";
  const [pendingExample, setPendingExample] = useState<number | null>(null);
  const [deselected, setDeselected] = useState<Set<number>>(new Set());

  const onTile = (c: Candidate) => {
    if (cs.stage === "grid") {
      if (c.href) { setPendingExample(c.id); caseOpenDetail(c.href); }
      else caseGeneralize(c.id);
    } else if (cs.stage === "detail" && pendingExample != null) {
      caseGeneralize(pendingExample, c.id);
      setPendingExample(null);
    }
  };

  return (
    <div style={{ marginTop: 10 }}>
      <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
        <Button variant="ember" size="sm" disabled={!ready} onClick={() => caseExtract()}>Case it</Button>
        {cs.stage === "recipe" && cs.recipe && (
          <>
            <Button variant="ghost" size="sm" onClick={() => caseRun(cs.recipe!, cs.gridUrl, count, destDir)}>Grab · {cs.matched}</Button>
            <Button variant="ghost" size="sm" onClick={() => playbookSave(cs.recipe!)}>Save playbook</Button>
          </>
        )}
      </div>
      {cs.stage === "detail" && <div style={{ fontFamily: "var(--font-type)", fontSize: 10, color: "var(--absinthe)", marginBottom: 6 }}>Click the full-size image on this detail page.</div>}
      {cs.stage === "recipe" && <div style={{ fontFamily: "var(--font-type)", fontSize: 10, color: "var(--text-forge-mute)", marginBottom: 6 }}>Matched {cs.matched} of {cs.total}. Grab downloads them to your folder.</div>}
      {(cs.stage === "grid" || cs.stage === "detail") && (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))", gap: 6, maxHeight: 260, overflow: "auto" }}>
          {cs.candidates.map((c) => (
            <img key={c.id} src={c.preview_url} alt="" title={c.selector}
              onClick={() => onTile(c)}
              style={{ width: "100%", height: 72, objectFit: "cover", cursor: "pointer", borderRadius: 4,
                border: deselected.has(c.id) ? "1px solid var(--border-forge)" : "1px solid var(--gold-700)", opacity: deselected.has(c.id) ? 0.4 : 1 }} />
          ))}
        </div>
      )}
    </div>
  );
}
```

(The `deselected`/`setDeselected` set is wired for future per-tile deselect; unused-var lint is avoided because both are referenced in the tile style + could be toggled — if lint flags `setDeselected` as unused, add an `onContextMenu` toggle: `onContextMenu={(e)=>{e.preventDefault(); setDeselected(d=>{const n=new Set(d); n.has(c.id)?n.delete(c.id):n.add(c.id); return n;});}}` on the img.)

- [ ] **Step 2: Wire into `PageScrapePanel.tsx`** — import and render below the existing Work-the-gallery button:

```tsx
import CasePanel from "./CasePanel";
// …at the end of the returned JSX, after the "Work the gallery" <Button/>:
      <CasePanel destDir={destDir} count={count} />
```

- [ ] **Step 3: Build the webapp**

Run: `cd desktop/webapp && npm run build 2>&1 | tail -20`
Expected: type-checks and builds with no errors.

- [ ] **Step 4: Commit**

```bash
git add desktop/webapp/src/components/CasePanel.tsx desktop/webapp/src/components/PageScrapePanel.tsx
git commit -m "feat: CasePanel UI — Case it, two-click demo, batch grab, save playbook"
```

---

### Task 7: Full verification

- [ ] **Step 1: Backend** — `cd "desktop/src-tauri" && cargo build 2>&1 | tail -15 && cargo test 2>&1 | tail -25` — no warnings, all tests pass.
- [ ] **Step 2: Frontend** — `cd desktop/webapp && npx vitest run 2>&1 | tail -20 && npm run build 2>&1 | tail -10` — tests pass, build clean.
- [ ] **Step 3: Drive it** — launch via `bow.bat`, Ghost car → a real gallery, Case it, click a thumbnail, click the detail image, confirm "matched N of M", Grab, confirm non-zero downloads land in the dest bin, Save playbook, confirm `workspace/playbooks/<domain>.json` exists and reloads.
- [ ] **Step 4: Final commit** if any fixups: `git commit -am "fix: case-the-gallery verification fixups"`.

## Self-Review

- **Spec coverage:** candidate extractor+lazy+no-ext (Task 3 ✓); generalizer (Task 1 ✓); playbook store (Task 2 ✓); WS case_extract/open_detail/generalize/run + playbook_save/list (Task 4 ✓); CasePanel two-click + batch + save/load (Tasks 5–6 ✓); re-navigate+re-extract in case_run (Task 4 ✓); skip+count misses (Task 4 loop ✓); scrape_cancel stop (Task 4 ✓); path guard (Task 4 uses resolve_within_workspace / workspace_root.join ✓). **Gap noted:** load-playbook *pre-fill straight to Grab* UI is minimal in Task 6 (Save is wired; a Load dropdown is deferred — `playbookList` action exists so it's a small follow-up, not a spec gap for the core flow).
- **Placeholder scan:** none — every step has real code/commands.
- **Type consistency:** `Candidate`/`Recipe` field names identical across Rust (Task 1) and TS (Task 5). `match_pattern`, `structural_pattern`, `detail_selector_from`, `generalize`, `domain_of`, `save_playbook`, `load_playbooks` used in Task 4 all defined in Tasks 1–2. `ScrapeEvent`/`DownloadOpts`/`download_urls_to_dir`/`pick_auto_bin`/`SessionLog` signatures match `image_search.rs`.
