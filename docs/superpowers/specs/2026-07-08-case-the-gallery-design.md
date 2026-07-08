# Case the gallery — guided grab + saveable playbook — design

**Date:** 2026-07-08
**Status:** Approved (autonomous build authorized), implementing
**Branch:** feat/case-gallery

## Problem

Bow scrapes search engines well, but **page-scrape of an arbitrary gallery site
returns 0/100**. Two causes, both in the controlled-browser extractor:

1. **Extension-only filter** — `normalize_image_urls` (`controlled_browser.rs:546`)
   keeps a URL only if its path (before `?`) ends in `.jpg/.png/...`. Modern galleries
   serve images from extensionless CDN paths (`/image/12345`, `?id=…`), so every
   candidate is dropped.
2. **No lazy-load / no follow-through** — the extractor reads `img.src`/`currentSrc`/
   `srcset` but not `data-src`/`data-original`, and it has no notion that the
   full-resolution image usually sits **one click behind** a thumbnail
   (`<a href="/photo/123">`). Listing-page thumbnails alone are low-value.

Beyond the bug, there is no way to **teach** Bow how a specific site is laid out, or
to **save** that knowledge for reuse.

## Insight

The `ControlledBrowser` already drives the user's real, logged-in Chrome over CDP.
If Bow extracts *structured candidates* from the live page (not just URL strings), a
user can **demonstrate** the pattern with one or two clicks, Bow **generalizes** it to
all siblings, and the result can be **saved as a per-site playbook** and replayed.
This is the mainstream 2026 record-and-replay ("robot training") pattern, done locally.

## Decisions (from brainstorming)

- **Guided grab + optional save** (not silent auto-scrape, not save-only). You case a
  gallery interactively; if it works you may save it as a playbook.
- **Click-to-mark in a Bow panel** — candidates are extracted from the live page and
  rendered as a grid *inside Bow's own UI*; you click there, not in the raw site.
- **Mark one → harvest all like it, with follow-through** — one example generalizes to
  the sibling set; if the example is a thumbnail linking to a detail page, Bow follows
  each sibling's link to grab the full-size image.
- **Two-click demonstration for detail pages** — after marking a thumbnail, Bow opens
  *that one* detail page into the same mark-grid; you click the full image; Bow derives
  the detail-image selector. No largest-image guessing.
- **No LLM in the v1 loop** — generalization is pure DOM pattern-matching, so Casing
  works even when the embedded engine is off (consistent with scrape only needing the
  engine for Verify). LLM-based self-healing selectors → v2.
- **v1 scope = one page + infinite-scroll** (existing scroll passes). Multi-page
  "next page →" pagination → v2.

## Architecture

### Candidate extractor (`controlled_browser.rs`)

New `pub async fn extract_candidates(&self) -> Result<Vec<Candidate>>`. Runs one JS
pass over the live page collecting, for each `<img>` and each `<a>` wrapping an `<img>`:

```rust
pub struct Candidate {
    pub id: usize,             // stable index within this extraction
    pub preview_url: String,   // best thumbnail URL for display (src/currentSrc/data-src/srcset)
    pub href: Option<String>,  // absolute link if the img is wrapped in <a href>
    pub selector: String,      // a stable-ish CSS path to the element (nth-of-type chain)
    pub w: u32, pub h: u32,     // natural/rendered size, for ranking + filtering chrome
}
```

- Reads lazy attributes: `data-src`, `data-original`, `data-lazy`, `srcset` (first URL),
  falling back to `currentSrc`/`src`.
- **Drops the extension-only filter** for preview/candidate purposes; keeps only
  `http(s)` and non-`data:` URLs. (The pure `normalize_image_urls` helper stays for the
  legacy page-scrape path but is no longer the gate for Casing.)
- Records each element's CSS selector path (tag + `:nth-of-type` chain, capped depth) so
  the generalizer can find siblings.

The legacy `extract_image_urls()` path (`server.rs` PageScrapeRequest) is **kept** and
also upgraded to read lazy attributes + drop the extension gate, so plain "Work the
gallery" stops returning 0 even without Casing.

### Selector generalizer + playbook store (new `desktop/src-tauri/src/tools/recipe.rs`)

Pure, unit-tested DOM logic (no browser, no LLM):

- `fn generalize(example: &str, all: &[Candidate]) -> Recipe` — the **repeating unit** is
  the linked ancestor when the example has an `href` (the `<a>`/card the user clicked),
  else the `<img>` itself. Derive a **grid selector** by stripping the final
  `:nth-of-type(k)` index from that unit's selector path, then collect every candidate
  whose selector matches the generalized pattern = the sibling set.
- `struct Recipe { domain, grid_selector, link_selector: Option<String>,
  detail_image_selector: Option<String>, scrolls: u32 }`. A recipe is **URL-independent**
  (reusable across pages of the same domain); the concrete grid URL is a per-run param.
- `fn save_playbook(dir, &Recipe)` / `fn load_playbooks(dir, domain) -> Vec<Recipe>` —
  JSON files under `workspace/playbooks/<domain>.json` (path-guarded via the existing
  `resolve_within_workspace`). Keyed by registrable domain of the cased URL.

### WS protocol (`server.rs` + `store.ts` + `api.ts`)

New inbound messages (all require auth, like existing ones):

- `case_extract` → run `extract_candidates()`, stream back `{type:"case_candidates", items:[…]}`.
- `case_open_detail { candidate_id }` → navigate the controlled browser to that
  candidate's `href`, extract *its* candidates, stream back `case_candidates` tagged
  `stage:"detail"`.
- `case_generalize { example_id, detail_image_id? }` → build a `Recipe`, stream back
  `{type:"case_recipe", recipe, matched, total}` (the batch preview).
- `case_run { recipe, grid_url, count, dest_dir }` → **re-navigate** to `grid_url`,
  scroll `recipe.scrolls`, re-extract candidates, filter to the sibling set via
  `recipe.grid_selector`. Then for each grid item: if `link_selector` set, open its
  `href` → find `detail_image_selector` → collect that image URL; else collect the
  item's own image URL. Feed the collected URLs into the **existing**
  `download_urls_to_dir` pipeline. This single re-navigate+re-extract path serves both
  the immediate grab (grid_url = where Ghost car is) and saved-playbook replay (grid_url
  = a fresh URL on the same domain). Misses are **skipped and counted**, never fatal.
  Emits the same `scrape_event` stream (Phase/Candidates/Progress/Done) the UI already
  renders, and honours the existing `scrape_cancel` cooperative stop.
- `playbook_save { recipe }` / `playbook_list { domain }` → persist / return recipes.

### Teach UI (new `desktop/webapp/src/components/CasePanel.tsx`)

Replaces the confusing single-purpose flow around *Ghost car*:

- **Ghost car** button stays = "open the controlled browser + navigate" (log in there).
- **Case it** button (new, next to it) = `case_extract` → renders the returned
  candidates as a **click-to-mark grid** (each tile is the `preview_url` image).
- **Two-click demo:** click a tile → if it has `href`, panel switches to the detail
  page's candidates (`case_open_detail`) and prompts "click the full image"; click that →
  `case_generalize`.
- **Batch preview:** shows "matched N of M", grid of results with per-tile deselect,
  a count input, destination folder, and **Grab** (`case_run`).
- **Save as playbook** + a **Load playbook** dropdown (populated from `playbook_list`
  for the current domain) that pre-fills the recipe and skips straight to Grab.

Store/state (`store.ts`): a `case` slice mirroring the existing `scrape` slice
(`stage`, `candidates`, `recipe`, `matched`, `running`), driven by the new WS events.

## Data flow

```
Ghost car → controlled Chrome (logged in, on the grid)
   │
 Case it → extract_candidates() ─► CasePanel grid (click to mark)
   │                                  │ click thumbnail
   │                                  ▼ (has href) case_open_detail
   │                          detail-page candidates → click big image
   │                                  ▼ case_generalize
   │                          Recipe + sibling batch (matched N of M)
   │                                  ▼ case_run (re-navigate grid_url, re-extract)
   │                   for each sibling → open href → grab detail image
   ▼                                  ▼
 approve/deselect ─► download_urls_to_dir ─► dest folder
                          │
                    playbook_save (JSON, by domain)  ◄── Load playbook pre-fills recipe
```

## Error handling & edge cases

- **Non-uniform siblings** → items whose selector/link/detail-image can't be resolved
  are skipped and counted; the run reports "matched N of M" and downloads what it got.
- **Detail image not found** on a page → skip + count, don't abort the batch.
- **Login / consent walls** → already solved: user is in the real logged-in browser
  before Casing starts.
- **Browser not launched** → `case_extract` returns a friendly error ("open the browser
  first with Ghost car"), same shape as existing gating messages.
- **Stop** → reuses `scrape_cancel`; partial downloads are kept, run ends with a normal
  `done` event.
- **Playbook path safety** → all playbook file I/O goes through `resolve_within_workspace`.

## Testing

- `recipe.rs` unit tests: `generalize` strips the repeating index and matches siblings;
  save/load round-trips; domain keying; non-matching example yields a single-item recipe.
- `controlled_browser.rs`: extend `normalize_image_urls` tests; add a pure test for the
  selector-path → grid-selector reduction (the JS extraction itself stays behind the
  `#[ignore]` live-Chrome test).
- `server.rs`: parse tests for the new inbound messages (mirroring
  `browser_open_and_page_scrape_parse`).
- Frontend: `store.test.ts` cases for the new `case_*` events; a `CasePanel` smoke test.
- Manual: drive Ghost car → Case it → two-click demo → Grab on a real gallery; confirm
  non-zero download and a saved playbook that reloads.

## Out of scope (v2)

- LLM self-healing selectors when a saved playbook's selectors drift.
- Multi-page "next page →" pagination and cross-URL playbook replay.
- Vision-based semantic ("grab the product photos") selectorless extraction.
