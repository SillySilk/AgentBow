# Bow — Project Instructions

## ⚡ FIRST THING EVERY SESSION: scan for agentic-app advancements

This is an **agentic AI app** in a fast-moving category — the state of the art
shifts almost daily. **Before doing substantive work in this repo, run a web
search for the latest advancements, techniques, and tooling** relevant to Bow,
then briefly summarize anything new and actionable for the user before starting.

Do this at the start of each working session (not on every trivial reply). If
the most recent results were already gathered earlier in the *same* session,
don't repeat the search.

**What to search for** (favor results from the last ~30–60 days; include the
current month/year in queries):
- Agent architecture & loops — planning, ReAct, Reflexion, self-verification,
  multi-agent orchestration, long-horizon / memory strategies
- Tool calling — schemas, parallel dispatch, structured output, reliability
- Local LLM serving — **llama.cpp / `llama-server` above all** (that's what Bow
  embeds: watch for releases, flag changes, GGUF/quant and vision/`--mmproj`
  developments); also LM Studio, vLLM, Ollama for comparison. New local models
  good at tool use / function calling and their quirks
- **MCP (Model Context Protocol)** ecosystem — spec changes, new servers,
  transport/auth updates
- Browser automation & web scraping for agents; image search/scrape + vision
  (captioning, dedupe, tagging) techniques
- Prompting techniques and evals for agentic systems

**How to report:** lead with 2–5 bullets of *new* things that matter for Bow
(what changed, why it's relevant, whether to adopt). Cite sources. Then proceed
with the requested work.

> If WebSearch/WebFetch is unavailable, say so explicitly rather than skipping
> silently, then continue with the task.

---

## What Bow is

A standalone, local, privacy-respecting AI agent for Windows. One Rust binary
(`bow-desktop`) serves a built-in React web UI and runs all agent logic on
`http://127.0.0.1:9357` (axum server + WebSocket + system tray). Bow **runs its
own inference**: it spawns a bundled llama.cpp **`llama-server`** child process
against a GGUF you pick from `BOW_MODELS_DIR` and talks to it over its
OpenAI-compatible API — **no external app to launch, no cloud/Anthropic API
calls**. Launched via `bow.bat`; stopped via `kill-bow.bat`.

Product focus is a dedicated **image scraper/downloader + curation** tool; the
general AI agent is secondary.

## Layout

- `desktop/src-tauri/` — the Rust brain: streaming agent loop (planning,
  self-verification, Reflexion, observation masking, parallel tool dispatch),
  SQLite/FTS5 episodic memory, axum REST + WebSocket, tray icon. (Tauri itself
  is removed; the directory name is legacy.)
- `desktop/src-tauri/src/llm_engine.rs` — the **only** owner of the
  `llama-server` child process: GGUF catalog scan, quant enforcement, mmproj
  vision pairing, spawn/health/stop.
- `desktop/webapp/` — React/TypeScript UI, built to `desktop/webapp/dist` and
  served by the backend. `SettingsPanel.tsx` drives the engine (`/api/engine`,
  `/api/models`).
- `desktop/.env` — secrets/config (`BOW_SECRET`, `BOW_MODELS_DIR`,
  `BOW_CTX_SIZE`, `BOW_WORKSPACE`, etc.).
- `get-llama.ps1` — downloads the pinned llama.cpp build into
  `desktop/src-tauri/bin/llama/` (CUDA if `nvidia-smi` present, else CPU).
  Idempotent; called by `bow.bat`. The binaries are **not** vendored into git.
- `bow.bat` / `kill-bow.bat` — launch / stop (kept in repo root by convention).

## Commands

Run from `desktop/` (verified in `desktop/package.json`):
- `npm run dev` — Vite dev server for the webapp
- `npm run build` — `tsc && vite build`, compiles + bundles to `desktop/webapp/dist`
- `npm run preview` — preview the production build

No lint/test script is defined in `package.json`. `package.json` also lists
`@tauri-apps/api`/`@tauri-apps/cli` as devDependencies and a `"tauri"` script —
these are vestigial leftovers from before Tauri was removed and are unused; the
Rust side has no `tauri` crate dependency and no `tauri::` calls (verified in
`desktop/src-tauri/Cargo.toml` and `src/`). Don't mistake their presence for
Tauri still being active.

## Working agreements

- Local LLM only — never reintroduce Anthropic/cloud model calls, and never
  reintroduce the LM Studio dependency (removed 2026-07-02; Bow owns its own
  `llama-server`).
- Only **quantized** GGUFs are loadable; F16/BF16/F32 weights are listed but
  refused. `mmproj-*.gguf` siblings are what make a model vision-capable.
- Fix compiler/linter warnings even when non-fatal.
- Shell execution by the agent must stay silent (no popup window).
- Commit only when explicitly asked — **except**: at the end of any redesign
  task (a visual/UI rebrand or restyling pass), commit the finished work
  locally and push to `origin master` without asking first. This exception is
  scoped to redesign work; all other commit/push activity still requires
  explicit sign-off.
- Keep `.bat`/launcher files in the repo root, never buried in subdirectories.
