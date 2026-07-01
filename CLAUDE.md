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
- Local LLM serving — LM Studio, llama.cpp, vLLM, Ollama; new local models good
  at tool use / function calling and their quirks
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
`http://127.0.0.1:9357` (axum server + WebSocket + system tray). All model calls
go to a **local LM Studio** server (OpenAI-compatible) — **no cloud/Anthropic
API calls**. Launched via `bow.bat`; stopped via `kill-bow.bat`.

Product focus is a dedicated **image scraper/downloader + curation** tool; the
general AI agent is secondary.

## Layout

- `desktop/src-tauri/` — the Rust brain: streaming agent loop (planning,
  self-verification, Reflexion, observation masking, parallel tool dispatch),
  SQLite/FTS5 episodic memory, axum REST + WebSocket, tray icon. (Tauri itself
  is removed; the directory name is legacy.)
- `desktop/webapp/` — React/TypeScript UI, built to `desktop/webapp/dist` and
  served by the backend.
- `desktop/.env` — secrets/config (`BOW_SECRET`, `LM_STUDIO_URL`,
  `BOW_WORKSPACE`, etc.).
- `bow.bat` / `kill-bow.bat` — launch / stop (kept in repo root by convention).

## Working agreements

- Local LLM only — never reintroduce Anthropic/cloud model calls.
- Fix compiler/linter warnings even when non-fatal.
- Shell execution by the agent must stay silent (no popup window).
- Commit only when explicitly asked — **except**: at the end of any redesign
  task (a visual/UI rebrand or restyling pass), commit the finished work
  locally and push to `origin master` without asking first. This exception is
  scoped to redesign work; all other commit/push activity still requires
  explicit sign-off.
- Keep `.bat`/launcher files in the repo root, never buried in subdirectories.
