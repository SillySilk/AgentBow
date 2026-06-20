# Bow

A local, privacy-respecting AI agent for Windows. Bow runs your own model in
**LM Studio** and gives it real tools: files, shell, web search, full browser
control, image scraping + vision, episodic memory, and — via **MCP** — the
entire Model Context Protocol ecosystem.

There are no content restrictions and no cloud model calls: everything runs
against your local LM Studio server.

---

## Architecture

Bow is a standalone Rust binary that serves a built-in web UI and handles all
agent logic on one local port:

```
┌─────────────────────────┐         http://127.0.0.1:9357
│  Browser (any)          │ ─────────────────────────────┐
│  • Bow web UI (SPA)     │   REST + WebSocket           │
│  • chat interface       │ ◀────────────────────────────┤
└─────────────────────────┘                              │
                                             ┌───────────┴──────────┐
                                             │  bow-desktop (Rust)  │
                                             │  • axum web server   │
                                             │  • agent loop        │
                                             │  • talks to LM Studio│
                                             │  • runs all tools    │
                                             │  • system tray icon  │
                                             └───────────┬──────────┘
                                                         │ OpenAI-compatible
                                                         ▼ /v1/chat/completions
                                               ┌──────────────────────┐
                                               │  LM Studio (local)   │
                                               └──────────────────────┘
```

- **`desktop/src-tauri/`** — the Rust brain. Streaming agent loop with planning,
  self-verification, Reflexion on failure, observation masking, parallel tool
  dispatch, and SQLite (FTS5) episodic memory. Serves the web UI as static files
  and exposes REST + WebSocket on `127.0.0.1:9357`.
- **`desktop/webapp/`** — React/TypeScript chat UI, built to `desktop/webapp/dist`
  and served directly by the backend.

### The agent loop (high level)

1. User message arrives over the WebSocket.
2. Bow builds the tool list (native tools **+** any MCP server tools) and streams
   a completion from LM Studio.
3. Tool calls are executed — plan/verify tools run serially; independent tool
   calls run in parallel.
4. Results feed back in; older tool results get masked to control context growth.
5. The model calls `task_complete` to finish, or hits the iteration cap (which
   triggers a stored reflection for next time).

---

## Setup

### Prerequisites

- **Windows 10/11**
- **[LM Studio](https://lmstudio.ai/)** running a tool-capable model, server
  started on `http://localhost:1234`. A vision-capable model is needed for
  `image_verify` screenshots.
- **Rust** (stable) and **Node.js** for building.
- *(Optional)* **Node/npx** and/or **uv/uvx** if you want to run MCP servers.

### Configure

Edit `desktop/.env` (see the file for the full annotated list). The keys that
are actually read:

| Key | Purpose |
|---|---|
| `BOW_SECRET` | Auth token the web UI uses. **Required.** |
| `LM_STUDIO_URL` | LM Studio server URL (default `http://localhost:1234`). |
| `LM_STUDIO_MODEL` | Model id as shown in LM Studio. |
| `LM_STUDIO_REASONING_EFFORT` | `low`/`medium`/`high`, or blank. |
| `LM_STUDIO_REASONING_TOKENS` | Reasoning token budget, or blank. |
| `BOW_WS_PORT` | WebSocket port (default `9357`). |
| `BOW_WORKSPACE` | Where Bow reads/writes files, stores `memory.db`, finds `mcp.json`. |
| `TAVILY_API_KEY` | For `web_search` / `web_search_deep`. |
| `SEARXNG_URL` | Local SearXNG instance for `searxng_search` (optional). |

## Run

1. Ensure `desktop/.env` is configured (LM Studio URL/model, BOW_SECRET, etc.).
2. Double-click `bow.bat` in the project root.
3. Your browser opens to `http://127.0.0.1:9357` (Bow Image Studio).

There is no browser extension — Bow runs as a standalone local web app.

`bow.bat` does the following automatically:
- Builds the web UI (`npm run build` in `desktop/webapp`)
- Builds the backend (`cargo build` in `desktop/src-tauri`)
- Copies built web assets next to the exe (`target/debug/web/`)
- Launches `bow-desktop.exe` (tray icon appears)

---

## Native tools

| Area | Tools |
|---|---|
| Files | `file_read`, `file_write`, `file_list`, `file_download` |
| Shell | `shell_exec` — **persistent** PowerShell session (cwd, `$env:`, and `$vars` carry across calls; per-command timeout auto-respawns a hung shell) |
| Web | `web_search`, `web_search_deep`, `searxng_search`, `jina_read`, `search_evaluate` |
| Images | `image_download` (Bing/DDG/Yandex/Brave/Qwant/SearXNG), `image_verify` (vision; transcodes WebP→PNG), `image_dedupe` (pHash near-dup quarantine), `image_stats` (folder report), `image_resize` (non-destructive resize/convert for training sets), `image_autotag` (writes kohya `.txt` captions via the local vision model) |
| Browser | `browser_navigate`, `browser_click`, `browser_fill`, `browser_read_page`, `browser_screenshot`, `browser_analyze_page`, tabs, cookies, bookmarks, history, `browser_exec_js` |
| Planning | `plan_create`, `plan_step_start/done/fail`, `verify_step`, `task_complete` |
| Memory | `memory_store`, `memory_retrieve` (SQLite FTS5 + optional embeddings) |

### Image-training workflow

The image tools chain into a training-set prep pipeline. A typical agent run:

1. `image_download` — gather candidates for a subject into a folder.
2. `image_stats` — inspect the set (counts, resolutions, corrupt files).
3. `image_dedupe` — remove perceptual near-duplicates (keeps the highest-res of
   each group; `apply=true` moves the rest into a `_bow_dupes` subfolder, nothing
   is deleted).
4. `image_resize` — write normalized copies (capped longest side, consistent
   format) into a clean output folder, leaving originals untouched.
5. `image_autotag` — caption each image with the local LM Studio vision model,
   writing a `<name>.txt` sidecar (kohya convention). Use `style:"tags"` for
   booru-style tags or `"caption"` for a sentence, and `trigger` to prepend an
   activation word (the character/person's name).

---

## MCP (Model Context Protocol)

Bow is an **MCP client**: on startup it reads `mcp.json`, spawns each enabled
server as a stdio child process, discovers its tools, and exposes them to the
model as `mcp__<server>__<tool>`. This is how you extend Bow without writing Rust.

The config format is the same one Claude Desktop uses, so you can copy server
definitions from anywhere. It lives at `desktop/mcp.json` (also searched: next to
the executable, and in `BOW_WORKSPACE`).

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:\\AI\\workspace"],
      "env": {},
      "disabled": false
    }
  }
}
```

- `disabled: true` skips a server. A server that fails to start (or takes > 60s)
  is logged and skipped — the rest still load, and Bow's native tools always work.
- **First launch downloads the npm/uvx packages**, so the first start after
  enabling a server can be slow; it's cached afterward.
- On Windows, bare `npx`/`uvx` commands are automatically routed through
  `cmd /C` so `PATHEXT` resolution works (a common MCP-on-Windows gotcha).

### Recommended servers

These are vetted, well-rated, and chosen to **add capabilities Bow doesn't
already have** (so no redundant filesystem-vs-filesystem overlap beyond the
sandboxed extras). Enabled-by-default ones need no API keys.

| Server | Command | Adds | Default |
|---|---|---|---|
| **filesystem** (official) | `npx -y @modelcontextprotocol/server-filesystem <dir>` | Sandboxed dir tree, multi-file read, search, move | ✅ on |
| **sequential-thinking** (official) | `npx -y @modelcontextprotocol/server-sequential-thinking` | Structured step-by-step reasoning scratchpad | ✅ on |
| **git** (official) | `uvx mcp-server-git --repository <dir>` | Local repo: status, diff, log, commit, branches | off (needs uv) |
| **github** (official) | `npx -y @modelcontextprotocol/server-github` | Remote repos, issues, PRs, code search | off (needs `GITHUB_PERSONAL_ACCESS_TOKEN`) |
| **playwright** (Microsoft) | `npx -y @playwright/mcp@latest` | Headless browser automation | off (downloads browsers) |

Other solid options worth adding for specific needs: **sqlite** (query local
DBs), **time** (`uvx mcp-server-time`), **fetch** (`uvx mcp-server-fetch`).

To enable one, set its `"disabled": false` (and fill any required `env`), then
restart Bow.

---

## Security notes

Bow is an intentionally unrestricted local agent. Be aware:

- The agent can read/write files, run PowerShell, and drive your browser.
  Guardrails block a small set of catastrophic shell/path patterns
  (`tools/mod.rs`), but it is otherwise unsandboxed.
- Saved logins are read from `credentials.json` in plaintext and typed into
  forms. Consider moving these to Windows Credential Manager / DPAPI.
- `browser_exec_js` runs arbitrary JS in the active tab via the browser bridge.

Run Bow only with models and tasks you trust.
