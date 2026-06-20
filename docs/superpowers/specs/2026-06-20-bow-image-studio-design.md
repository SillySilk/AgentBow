# Bow Image Studio — Design Spec

**Date:** 2026-06-20
**Status:** Draft for review

## Summary

Refocus Bow from a browser-extension AI agent into a **standalone, locally-run web app
specialized in image scraping and downloading**, with an optional AI assist. The user
launches a single Rust backend (via a `.bat` in the project root), which serves a web UI;
the user opens `http://localhost:9357` in any browser. No browser extension, no Tauri
window.

This dissolves the current blocking problems rather than patching them: the "broken in
Chrome" service-worker regression, the MV3 keepalive hacks, and the Edge-port question all
disappear because the extension is removed entirely.

## Goals

- Standalone web app: launch backend, open a URL, use it. No side-loaded extension.
- Two first-class scrape modes, equally supported:
  1. **Search-and-bulk-download** — query → scrape search engines → filter → download N.
  2. **Page/site scrape** — scrape a specific page/gallery, including auth-walled and
     JS-heavy sites, via a backend-controlled browser with a persistent login profile.
- A dedicated scraper UI (direct controls) as the primary interface; AI as optional assist.
- Curation: dedupe, verify, filter (e.g. paid-CDN), preview, select, download.
- Keep the useful agent capabilities (file ops, web search, MCP, memory, LLM loop) but
  subordinate them to the scraping focus.

## Non-Goals

- No Chrome/Edge extension. The `extension/` tree is removed (archived in git history).
- No Tauri desktop webview window. (System tray is retained via a Rust tray library.)
- No remote/multi-user hosting. Backend binds to `127.0.0.1` only; single local user.
- No new image sources beyond repairing existing ones (deferred enhancement).

## Decisions (locked during brainstorming)

| Decision | Choice |
|----------|--------|
| Product shape | Dedicated scraper app + optional AI assist |
| Delivery | Standalone web app opened at `http://localhost:9357` |
| Scrape modes | Both: search-engine HTTP **and** controlled-browser page scrape |
| Backend host | Plain Rust server + system tray, launched by a root `.bat` |
| Tauri window | Removed |
| Browser extension | Removed |
| Controlled browser | `chromiumoxide` (Rust CDP client) driving local Chrome |

## Architecture

```
Browser (any) → http://localhost:9357
  └─ React SPA (served as static files by the backend)
        • Search-scrape panel  • Page-scrape panel  • Curation grid
        • AI assist (collapsible)
        ▲ REST + WebSocket (same origin, same port)
Rust backend (single binary; .bat launch; tray icon)
  • axum HTTP server: serves SPA + REST + WS upgrade at /ws
  • Scraper engine (search-engine HTTP)            [from image_search.rs]
  • Controlled-browser subsystem (chromiumoxide)   [new]
  • Curation: dedupe / verify / filter             [image_curate.rs + image_hasher]
  • File ops, web search, MCP client, memory       [kept]
  • LLM agent loop (optional assist)               [local_llm.rs]
  • System tray + single-instance + .env handling  [new, replaces Tauri]
```

Single port (9357) serves the SPA, REST endpoints, and the WS upgrade — one origin, no
CORS, no separate ports.

## Components

### 1. Backend host (replaces Tauri)
- **axum** HTTP server (tokio/reqwest already in tree). Serves the built SPA as static
  assets, REST endpoints, and a WS endpoint at `/ws`.
- **System tray** via the `tray-icon` crate with a `tao`/`winit` event loop on the main
  thread; the tokio runtime runs the server. Tray menu: Open (launches browser to
  localhost), Open Workspace, Edit Settings (.env), Quit.
- **Launcher:** `bow.bat` in project root — builds/starts the backend and auto-opens the
  browser to `http://localhost:9357`.
- **Config:** keep `Config::from_env` / `.env`. `.env` parse failure shows a native
  message box (as today) instead of the Tauri dialog.
- **Single-instance** guard so a second launch focuses the existing one (reuses the
  `SO_REUSEADDR`/port-bind check or a named mutex).

### 2. Scraper engine (existing, retained)
- `image_search.rs` keeps multi-source search-engine scraping (Bing, DDG, Brave, Yandex,
  Qwant, SearXNG), paid-CDN filtering, magic-byte validation, download pool.
- Exposed via REST/WS so the UI can drive it directly (not only through the LLM).
- Source repair is a later phase (Yandex/SearXNG anti-bot challenges, Qwant 403).

### 3. Controlled-browser subsystem (new)
- `chromiumoxide` driving a local Chrome via CDP.
- **Persistent profile** dir under the workspace so the user logs into sites once
  (headful login flow), then scrapes (headless or headful) reusing the session.
- Capabilities: navigate, read DOM/HTML, extract image URLs from galleries, scroll to
  trigger lazy-load/infinite scroll, click, fill, screenshot.
- The legacy 18 "browser tools" that relayed to the extension are **repointed** to this
  subsystem so the agent keeps `navigate/read_page/click/scroll/...` driving *its* browser.
- Page-scrape flow: user gives a URL (or uses the headful browser to reach a page), the
  subsystem extracts candidate image URLs, which then flow into the same
  curation/download pipeline as search results.

### 4. Web UI (new SPA)
- React + Vite + Zustand + Tailwind, reusing the existing design palette and chat
  components from `extension/src/sidepanel`. New project dir (e.g. `webapp/`); the old
  extension code is the component source to port from before `extension/` is removed.
- **Search-scrape panel:** query, per-source toggles, target count, destination folder,
  "go". Live progress (per-source URL counts, download progress) over WS.
- **Curation grid:** thumbnails of candidates/downloads, dedupe indicator, select/deselect,
  delete, re-run, open folder.
- **Page-scrape panel:** URL input + "open controlled browser" (headful) for login/
  navigation, then "scrape images from this page".
- **AI assist:** collapsible chat that talks to the existing agent loop for complex cases
  ("paginate this gallery and grab everything", "skip stock sites", curation by description).

### 5. Curation
- `image_curate.rs` + `image_hasher` (already a dependency) for perceptual-hash dedupe,
  dimension/quality filtering, and the existing paid-CDN filter.

### 6. Retained agent capabilities
- `local_llm.rs` agent loop kept as the AI assist backend; tools available to it:
  controlled-browser tools, scraper, curation, file ops, web search, MCP, memory.
- **MCP regression fix folded in:** `McpManager::load` must not block the WS/HTTP accept
  path. Load MCP servers concurrently in a background task; the server starts accepting
  immediately and MCP tools become available once ready. (Root cause of the current
  "broken in Chrome": sequential 60s-per-server MCP load ran before `listener.accept()`.)

## Data flow (search scrape)

1. UI sends `{query, sources, count, dest}` over WS.
2. Backend scrapes sources concurrently → candidate URL pool.
3. Filter (paid-CDN, dedupe) → download pool → download to dest with magic-byte validation.
4. Progress + results streamed to UI; curation grid renders thumbnails.

## Data flow (page scrape)

1. User opens controlled browser (headful) and logs in / navigates if needed.
2. UI requests "scrape this page"; subsystem scrolls/extracts image URLs.
3. URLs enter the same filter → download → curation pipeline.

## Error handling

- Per-source scrape failures are logged and skipped; partial results still returned (as
  today). UI surfaces per-source status.
- Controlled-browser failures (no Chrome found, profile locked, CDP disconnect) surface a
  clear UI error with remediation; never crash the server.
- MCP server start failures are best-effort and never block startup.
- `.env`/config errors: native message box, exit.

## Security

- Bind `127.0.0.1` only. Same-origin SPA (served by the backend) → no CORS.
- Auth: since the SPA and API share an origin and the bind is loopback-only, replace the
  manual token with a same-origin check (and/or a backend-set session cookie). Keep
  `BOW_SECRET` as an optional gate for the WS handshake. (Open item — confirm during plan.)
- Existing shell/file-write guardrails retained for agent tools.

## Phasing

1. **Backend host swap** — axum serving SPA + REST + WS on 9357; Rust tray; `bow.bat`;
   remove Tauri window deps; remove `extension/`; fold in the MCP non-blocking fix.
2. **Search-scrape web UI** — search panel + curation grid + live progress, wired to the
   existing scraper engine.
3. **Controlled browser** — chromiumoxide subsystem, persistent profile, repoint legacy
   browser tools, page-scrape UI.
4. **AI assist + source repair** — assist chat panel; repair degraded scraper sources.

## Open items (resolve during planning)

- Auth model for the localhost web app (drop token vs. cookie vs. keep `BOW_SECRET`).
- Rust tray library specifics and main-thread event-loop integration with tokio.
- chromiumoxide Chrome-binary discovery and profile location.
- Whether `webapp/` is a fresh Vite project or the renamed/repurposed `extension/` build.

## Risks

- **Rust tray without Tauri**: tray needs a main-thread OS event loop; must coexist with
  the tokio server. Known pattern but the main integration risk of Phase 1.
- **chromiumoxide** depends on a present Chrome binary and is more manual than Playwright
  for tricky sites; mitigated by the persistent profile and headful fallback.
- **Scraper source decay**: anti-bot challenges (Anubis on SearXNG, Yandex, Qwant 403)
  are an ongoing maintenance cost regardless of this redesign.
