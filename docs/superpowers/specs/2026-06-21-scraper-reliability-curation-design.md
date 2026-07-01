# Scraper Reliability + Simplified Curation — Design

Date: 2026-06-21
Branch: `bow-image-studio`
Status: Approved (brainstorming) — pending spec review

## Problem

A live `image_download` run surfaced several issues:

- Search sources are degraded: Qwant `HTTP 403`, Yandex `0 URLs`, SearXNG `0 URLs`
  (dead public instance). Only Bing/DDG/Brave produce URLs.
- Despite 189 candidates, **every download failed with `HTTP 403 Forbidden`** — a
  total download failure, not just degraded sources.
- Safe-search ("safe filters") off is unreliable across engines.
- The curation grid's thumbnail click only toggles a selection used by
  Delete/dedupe — features the user does not want.
- No image-size filtering exists: small icons and thumbnails download freely.

## Goals

1. Repair/expand search sources and fix the all-403 download failure.
2. Make safe-search-off more reliable.
3. Persist the user's search UI selections across sessions.
4. Add a minimum image-size filter (exclude icons / tiny images).
5. Simplify the curation grid: remove select/delete/dedupe; make a thumbnail
   click copy that image to the clipboard.

## Non-Goals

- No lightbox/preview overlay (click = copy, not preview).
- No re-architecture of the WS/streaming pipeline beyond threading new params.
- No new AI-assist features (separate future phase).

---

## A. Search sources — Hybrid (fast HTTP + browser fallback)

### A1. Cheap HTTP wins (always on)

- **Brave referer fix (root cause of the all-403 downloads).** Brave returns
  proxied URLs on `imgs.search.brave.com`, which 403 when fetched with the
  default `google.com` referer. Add per-host referer rules in
  `download_image_bytes`:
  - host contains `imgs.search.brave.com` → `https://search.brave.com/`
  - (existing reddit/bing rules retained)
- **SearXNG configurable instance.** The hardcoded `search.hbubli.cc` is dead.
  Read `SEARXNG_URL` from `.env`; if unset, try a short built-in list of
  JSON-enabled public instances in order, stopping at the first that returns
  usable JSON. A whole-source failure is non-fatal (logged, 0 URLs).
- **Add Mojeek images** as an extra always-on HTTP source (low anti-bot). New
  `scrape_mojeek_images` following the existing `ScrapeResult` pattern; added to
  the `ALL_SOURCES` list and the dispatch in `image_download`.

### A2. Browser fallback (opt-in, default off) — PHASE 2

- New UI toggle "Use browser fallback" (off by default), threaded through the WS
  scrape request into `image_download` as a `use_browser_fallback: bool`.
- When on, engines that HTTP-fail (return an error or 0 URLs) — primarily Yandex
  and Qwant — are retried by driving the existing `ControlledBrowser` to that
  engine's image-results page and extracting image URLs from the live DOM.
- Downloads that still 403 after the referer fix get **one** retry fetched
  through the browser context (its cookie jar + correct origin).
- Heaviest piece; shipped **after** everything else (see Phasing).

---

## B. Safe-search ("safe filters") off — reliability

- Keep the existing per-engine HTTP param/cookie combinations (`adlt=off`,
  `kp=-2`, `safesearch=0/off`, etc.).
- The durable win rides on the browser fallback (A2): set each engine's
  safe-search-OFF preference cookie **once** in the persistent Chrome profile
  (`<BOW_WORKSPACE>\.bow_browser_profile`) so it persists across runs instead of
  being re-negotiated on every stateless HTTP request.
- No standalone safe-search work outside A1/A2 — it is a property of how each
  source is queried, not a separate subsystem.

---

## C. Persist UI selections

- Persist the SearchPanel settings to `localStorage`:
  source checkboxes, size preset, count, destination folder, browser-fallback
  toggle.
- Restored on load; falls back to current defaults when absent or malformed.
- Implementation: a small persisted slice (Zustand `persist` middleware or a
  `useEffect` read/write). Query text is **not** persisted (per-search intent).

---

## D. Image-size filter — local enforcement, preset

- UI dropdown **Min size**: `Any / Medium (≥640px) / Large (≥1024px) /
  Huge (≥2048px)`, measured on the **shorter** edge (excludes both icons and
  skinny banners).
- Thresholds (shorter-edge px): Any=0, Medium=640, Large=1024, Huge=2048.
- Enforced in the download path after bytes are fetched and magic-validated:
  read image **header** dimensions only (e.g.
  `image::io::Reader::new(Cursor).with_guessed_format()?.into_dimensions()`),
  no full decode. If `min(width, height) < threshold`, discard and emit a
  `Failed { reason: "too small (WxH)" }` event so it shows in the progress log.
- Threshold flows UI → WS scrape request (`min_size: u32`) → `image_download` →
  `download_urls_to_dir` → per-image check.
- `Any` (0) short-circuits the check.

---

## E. Curation grid — simplify + copy-to-clipboard

### E1. Removal

- Remove from `CurationGrid.tsx`: the `selected` set, "Delete selected",
  "Remove duplicates", and the per-tile selection highlight logic. Keep
  **Open folder** and **Refresh**.
- Remove the now-dead backend + client code (per the repo's "remove dead code"
  rule):
  - `web_api.rs`: `delete_images`, `dedupe` handlers and their routes
    (`/api/images/delete`, `/api/curate/dedupe`) and the `DeleteBody`/`DedupeBody`
    structs.
  - `api.ts`: `deleteImages`, `dedupe` functions.
  - `image_curate::image_dedupe` is retained only if still referenced by a tool;
    otherwise left in place (it is a tool, not UI-only) — verify before removing.

### E2. Click-to-copy

- New endpoint `GET /api/image?path=` — serves the full-resolution original file
  with its real content-type, guarded by `within_workspace`.
- Clicking a tile:
  1. fetch the full image as a Blob from `/api/image`,
  2. draw it onto an offscreen `<canvas>`,
  3. `canvas.toBlob(..., "image/png")`,
  4. `navigator.clipboard.write([new ClipboardItem({ "image/png": blob })])`.
- Canvas conversion normalizes jpg/webp/gif → PNG (Clipboard API reliably
  supports PNG). `http://127.0.0.1` is a secure context, so the Clipboard API is
  available.
- Visual ack: a brief "Copied!" badge overlaid on the clicked tile (~1s),
  tracked by path in component state. On failure, show "Copy failed".

---

## Data-flow changes (summary)

```
SearchPanel (sources, count, destDir, minSize preset, browserFallback)
  → localStorage (persist)                                  [C]
  → WS scrape_request { ..., min_size, use_browser_fallback }
    → image_download(query, count, dest, log, sources,
                     min_size, use_browser_fallback, progress)
      → scrapers (HTTP A1; + browser fallback A2)
      → download_urls_to_dir(..., min_size)
        → download_image_bytes (per-host referer A1)
        → header-dims check (D)  → drop if too small
CurationGrid: click tile → GET /api/image → canvas → clipboard PNG  [E]
```

## Testing

- **A1 Brave referer:** unit test mapping a `imgs.search.brave.com/...` URL to the
  Brave referer (pure function extracted from `download_image_bytes`).
- **A1 SearXNG instance selection:** unit test that instance list falls through
  on empty/invalid JSON to the next entry.
- **D size filter:** unit test `min(w,h) < threshold` drop logic on generated
  images of known dimensions (reuse `image::RgbImage::from_pixel`).
- **C persistence:** verify settings round-trip through localStorage
  (lightweight; component/store test).
- **E endpoint:** `/api/image` returns bytes for an in-workspace path and 400 for
  an out-of-workspace path (mirrors existing `thumb` tests).
- **A2 browser fallback:** behind the existing `#[ignore]` live-launch test
  convention (needs real Chrome); not in the default CI path.
- Manual live verification (per project norm): a real scrape with size filter +
  copy-to-clipboard + persistence reload.

## Phasing

1. **Phase 1 (cheap, high-value):** A1 (Brave referer fix, SearXNG config, Mojeek),
   C (persistence), D (size filter), E (grid simplify + copy). Ships standalone.
2. **Phase 2 (heavier):** A2 browser fallback + B's persistent-profile safe-search
   cookie.

## Open considerations

- Public SearXNG JSON instances are rate-limited and come and go; the `.env`
  override is the durable path, the built-in list is best-effort.
- Qwant's API 403 may persist even with better headers; it is expected to rely on
  the Phase-2 browser fallback.
- Clipboard image write requires user-gesture context (a click) — satisfied by
  the tile click handler.
```
