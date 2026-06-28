# Bin reuse + skip-existing + manual bin + content dedup — design

**Date:** 2026-06-28
**Status:** Approved, implementing
**Branch:** bow-image-studio

## Problem

Three related issues when scraping into the numbered "bins" (`<base>\N`):

1. **Empty bins are never reused.** `next_numbered_subdir` picks the lowest `N`
   whose *folder does not exist*. A bin folder that exists but is **empty** (e.g.
   its images were curated/deleted) is skipped, so a brand-new bin is created even
   though an empty one is sitting right there.
2. **No resume/append workflow.** There is no way to target a specific existing bin
   to add to it, and writing into a bin with files would overwrite `_001`, `_002`…
3. **No duplicate protection.** Re-scraping the same subject re-downloads images
   already present — both exact filename collisions and the same image under a
   different filename.

## Goals / decisions (from brainstorming)

- **Auto-bin reuses empty bins, hard-capped at 10.** Pick the lowest `1..=10` bin
  that is missing or empty; if all 10 contain images, fail with a clear message.
- **Manual bin selection.** A 1–10 dropdown, disabled by default behind a checkbox,
  lets the user target a specific bin (even a non-empty one) to resume/append.
- **Never overwrite existing files.** New files append after the highest existing
  index; any name collision is bumped to the next free number.
- **Content dedup (pHash), checkbox default ON.** Skip images that visually match
  something already in the target bin *or* an image kept earlier in the same run.
- **Skips don't count toward the requested N.** Keep paging until N genuinely new,
  unique images are saved (or sources are exhausted) — consistent with the
  success-only download count.

## Components

### 1. Bin selection (`tools/image_search.rs`)

Replace `next_numbered_subdir` with:

- **`pick_auto_bin(parent) -> Result<String>`** — for `n` in `1..=10`, let
  `dir = <parent>\n`. If `dir` is missing, create and return it. If it exists and
  contains **zero image files** (non-recursive; `_bow_dupes`/non-images ignored),
  return it (reuse empty). Otherwise continue. If all 10 contain images, return
  `Err("All 10 bins contain images — clear one or pick a bin manually.")`.
- **`resolve_manual_bin(parent, n: u32) -> Result<String>`** — error unless
  `1 <= n <= 10`; create `<parent>\n` if missing; return it (even if non-empty).

`server.rs` (`ScrapeRequest` and `PageScrapeRequest`) calls `resolve_manual_bin`
when a `bin` is supplied, else `pick_auto_bin`. `next_numbered_subdir` and its test
are removed (no remaining callers). Image-emptiness uses the existing
`image_curate::collect_images` (non-recursive).

### 2. Protocol + UI

- `InboundMsg::ScrapeRequest` gains `#[serde(default)] bin: Option<u32>` and
  `#[serde(default = "default_true")] dedupe: bool`.
- `ScrapeTuning` gains `dedupe: bool`. (`bin` is resolved to a path in `server.rs`
  before `image_download`, so it is not threaded into `ScrapeTuning`.)
- **SearchPanel.tsx**:
  - ☐ *"Add to a specific bin"* (default off) + a **1–10 `<select>`** disabled until
    the box is checked. When checked, include `bin` in the request.
  - ☑ *"Skip duplicates already in the bin"* (**default on**) → `dedupe`.
- **store.ts**: `startScrape` accepts `bin?: number` and `dedupe: boolean` and
  includes them in the `scrape_request` payload.

### 3. Skip existing filenames (no overwrite)

In `image_download`, before downloading, scan the resolved bin for existing
`<sanitized>_NNN.*` files and seed `seq` to the highest `N` found (0 if none). In
`download_batch` naming, if the computed path already exists, advance `seq` until
the path is free. Result: resuming into a bin appends cleanly; nothing is clobbered.

### 4. Content dedup (pHash)

- Extract a shared helper **`phash(bytes: &[u8]) -> Option<ImageHash>`** (Mean +
  `preproc_dct`, the existing `image_curate` config). Reuse a single
  `HasherConfig`. Threshold constant `DEDUPE_DIST = 10` (matches `image_curate`).
- When `tuning.dedupe` is true, at the start of `image_download` hash all existing
  images in the target bin on a blocking thread → `existing: Vec<ImageHash>`.
- **Dedup forces the sequential download path** (joins the `verify || delay_ms > 0`
  condition). After a successful download (and any vision-keep), compute the new
  image's pHash on a blocking thread and compare (Hamming distance) against
  `existing` plus the run's kept hashes. `<= DEDUPE_DIST` → **skip**
  (`ScrapeEvent::Failed { reason: "duplicate of existing image" }`, not counted);
  otherwise push the hash to the run set, write the file, count it.
- **Fail-open:** if hashing the new image fails, keep it (don't silently drop).
- The fast 3-concurrent path is unchanged and used only when `verify`, `delay_ms`,
  **and** `dedupe` are all off.

### 5. Net-new counting

Skips append to the failure/skip path and never increment `downloaded`, so the
existing pagination loop keeps pulling pages until `downloaded.len() >= count` or a
page yields no new candidates. N net-new falls out of the success-only design.

## Error handling / edges

- All 10 bins full (auto) or bin out of `1..=10` (manual) → error surfaced before
  any download.
- Existing bin images that are unreadable during the initial hash → skipped.
- A new image that fails to hash → kept (fail-open).
- Empty bin (`dedupe` on) → `existing` is empty; dedup only guards within-run.

## Testing

- `pick_auto_bin`: reuses an existing-empty bin, skips non-empty ones, errors when
  all 10 contain images (temp tree).
- `resolve_manual_bin`: out-of-range error; creates/returns the requested bin.
- `seq` seeding: with `query_001.jpg`/`query_002.jpg` present, the next write is
  `query_003.*`; collisions bump to the next free index.
- pHash dedup: a downloaded image matching an existing bin image is skipped while a
  distinct image is kept (reuse the `image_curate` gradient/checker patterns).
- Frontend: `startScrape` includes `bin`/`dedupe`; the dropdown is disabled until
  the checkbox is on.
- `cargo test` + `cargo check` clean (zero warnings); `npm run build` clean.

## Out of scope

Bin rename/reorder, cross-bin dedup, and a UI threshold slider (uses default 10).
