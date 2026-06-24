# Working-slot preview + slot switcher — design

**Date:** 2026-06-23
**Status:** Approved, implementing
**Branch:** bow-image-studio

## Problem

Since scrapes started landing in numbered set folders (`<base>\N`), the curation grid
(the "preview at the bottom") is broken: it lists `lastDestDir`, which is the **parent**
(`C:\AI\workspace`), and `collect_images` is non-recursive — so the just-scraped images
(in `\N`) never appear.

## Goal

After a scrape into slot `N`, the preview shows **only that slot's** images, `N` becomes
the "working slot", and the UI reflects it automatically. Plus a switcher to load any
existing slot into the preview (the user keeps sets in slots 1–10 and swaps between them).

## Decisions (from brainstorming)

- **Base stays the parent.** The destination field remains `<base>`; each new scrape
  creates the next numbered slot via `next_numbered_subdir`. The "working slot" is a
  *preview* concept only — it never changes where the next scrape writes (no nesting).
- **Slot switcher now.** A row of existing slots is clickable; the latest scrape is
  auto-selected.

## Backend

1. **`ScrapeEvent::Done` carries the slot dir.** Add `dest_dir: String` to the `Done`
   variant + `to_json` (`"dest_dir"`). `image_download` already has the resolved slot
   path — pass it in. Lets the UI point the preview at the exact slot.
2. **New `GET /api/slots?dir=<base>`.** Lists immediate subdirectories of `<base>` whose
   name is all digits, each with an image count, sorted numerically:
   `{ "base": "...", "slots": [ { "name": "1", "path": "...", "count": 12 }, ... ] }`.
   Workspace-guarded with `within_workspace`. Image count via the existing
   non-recursive `collect_images`.

## Frontend

3. **Store:** add `workingSlotDir: string`. On the `done` event set
   `workingSlotDir = m.dest_dir`. Keep `lastDestDir` as the base (parent) for the
   switcher's `dir` query. Extend `ScrapeEventMsg` `done` with `dest_dir`.
4. **CurationGrid:** preview `workingSlotDir` instead of `lastDestDir` (delete/dedupe/
   open-folder/refresh all target the working slot). Show a "Working slot: <name>"
   heading.
5. **Slot switcher** (small component above the grid): fetch `/api/slots?dir=<lastDestDir>`,
   render clickable chips per slot (name + count), highlight the active one, set
   `workingSlotDir` on click. Refresh the slot list when a scrape finishes; the latest
   slot auto-selects because the `done` event already set `workingSlotDir`.

## Error handling / edges

- No slots yet / base empty → switcher renders nothing; grid stays hidden (current
  null-render behavior preserved).
- Paths may carry the Windows `\\?\` verbatim prefix from canonicalization; existing
  `within_workspace` canonicalizes both sides, so listing/thumbs still match.

## Testing

- Backend: unit-test the slot-listing filter (numeric dirs only, numeric sort) against a
  temp tree. `cargo check` clean, zero warnings.
- Manual: scrape → preview shows the new slot's images and it's highlighted; click an
  older slot → its images load; a fresh scrape auto-selects the newest slot.

## Out of scope

The broader UI redesign (planned separately). Renaming/reordering slots.
