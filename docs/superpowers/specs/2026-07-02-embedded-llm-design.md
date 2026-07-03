# Embedded LLM Engine — Design Spec

**Date:** 2026-07-02
**Status:** Approved 2026-07-02 — user confirmed all recommended decisions
("go with your recommendations").

## Goal

Remove the LM Studio dependency entirely. Bow manages its own local inference:
the user points Settings at a folder of GGUF models (default `C:\AI\models`),
picks a model (~4B-class recommended), and Bow loads it itself. No external app
to launch, no connection to configure.

## Non-goals

- Cloud/Anthropic API calls (forbidden by project policy).
- Training, fine-tuning, or model downloads from within the agent loop.
- Multi-model concurrent serving (one chat/vision model loaded at a time).

## Decisions (all approved)

1. **Engine: bundled `llama-server` child process** (approach A). Bow ships a
   pinned llama.cpp server build (CUDA 12 + CPU fallback), spawns it silently on
   a private localhost port, and owns its lifecycle. All existing
   OpenAI-compatible call sites keep working. Rejected: in-process `llama-cpp-2`
   (immature vision/tool-call bindings, CUDA build pain) and `mistral.rs`
   (narrower model coverage).
2. **Models come from the user's folder** (`BOW_MODELS_DIR`, default
   `C:\AI\models`), scanned recursively for `*.gguf`. No bundled default model
   in v1 (the user already curates models there).
3. **Vision QA stays.** A model with a sibling `mmproj-*.gguf` is loaded with
   `--mmproj` and marked vision-capable; the scrape Verify toggle is disabled
   when the loaded model lacks vision.
4. **Memory embeddings drop to FTS5-only.** The embeddings re-rank path is
   already optional; remove the `/v1/embeddings` call. (Possible future: second
   tiny embed-model instance.)

## Hardware context

RTX 4060 Ti 16GB VRAM, 32GB RAM, i7-12700F. A Q4_K_M 4B model + mmproj +
8k context fits comfortably on GPU (`--n-gpu-layers 999`). The user's
`C:\AI\models` already holds `Gemma-4-E4B-…-Q4_K_M.gguf` + f16 mmproj (vision,
~4B-class) and a Qwen3.6-35B-A3B MoE as a larger option.

## Architecture

New module `desktop/src-tauri/src/llm_engine.rs` — the only owner of the child
process:

- **`LlmEngine` (Clone, Arc-backed)** — spawn/stop/restart `llama-server`;
  holds current `EngineStatus { state: Stopped|Starting|Ready|Failed(reason),
  model: Option<LoadedModel>, base_url }`.
- **Spawn:** `llama-server -m <gguf> [--mmproj <file>] --port <ephemeral>
  --host 127.0.0.1 --jinja --n-gpu-layers 999 --ctx-size <cfg>` with
  `CREATE_NO_WINDOW` (shell-silent agreement). Readiness = poll `/health`.
- **Model switch:** stop + respawn with the new file. In-flight agent runs get
  a clean "engine restarting" error.
- **Shutdown:** kill child on tray Quit and on panic (Windows Job Object so the
  child dies with the parent).
- **Binary acquisition:** `llama-server` binaries are downloaded on first run
  (or by `bow.bat`) from a pinned llama.cpp release into
  `desktop/src-tauri/bin/llama/`, checksum-verified; CUDA build preferred,
  CPU-only fallback if no NVIDIA driver. Not vendored into git.

### Model catalog + quantization rule

`scan_models(dir)` returns `ModelEntry { path, name, size_bytes, quant,
vision: bool }`.

- `quant` parsed from GGUF metadata (`general.file_type`) with filename-tag
  fallback (`Q4_K_M`, `Q5_K_S`, `IQ4_XS`, …).
- **Quantized is enforced:** entries whose primary weights are F16/BF16/F32 are
  listed but marked *"unquantized — not loadable"* and refused at load time.
  mmproj projector files are exempt (they're auxiliary) and are hidden from the
  main list, attached instead to their sibling model.
- `vision` = a matching `mmproj-*.gguf` exists in the same directory.

### Config & persistence

- Remove `LM_STUDIO_URL`, `LM_STUDIO_MODEL`, `LM_STUDIO_REASONING_*` from
  `.env`/`Config`; add `BOW_MODELS_DIR` (default `C:\AI\models`) and
  `BOW_CTX_SIZE` (default 8192).
- Selected model persists in a small `engine.json` beside the exe (survives
  restarts; Settings writes it, startup auto-loads the last model if present).
- Internal `Config.lm_studio_url`/`lm_studio_model` fields are replaced by the
  engine's `base_url()`/`model_id()`; `ScrapeTuning`'s vision fields collapse to
  "engine has vision or not" (auto-detect endpoint `/api/v0/models` is deleted).

### REST + Settings UI

New endpoints in `web_api.rs` (auth like the others):

- `GET /api/engine` → status + loaded model
- `GET /api/models` → scanned catalog
- `POST /api/engine/load { path }` → switch model (validates quantization)
- `POST /api/engine/stop`
- `POST /api/engine/models-dir { dir }` → change + rescan

Webapp gets a **Settings panel** (gear entry in the Agent 008 cockpit chrome):
models-dir field with rescan, model list (name, quant tag, size, vision badge),
Load button per row, engine status line (state, VRAM-friendly model info), and
context-size input. Status updates poll `GET /api/engine` (no new WS traffic).

### Touchpoint migration (74 refs, 7 files)

| Site | Change |
| --- | --- |
| `local_llm.rs` chat loop | URL/model from `LlmEngine`; drop `/api/v1/models` reasoning probe (llama-server doesn't have it) |
| `image_search.rs` vision QA | same completions call; model = loaded model; `pick_loaded_vision_model` + `/api/v0/models` deleted; verify requires `engine.has_vision()` |
| `web_search.rs` eval calls | URL/model from engine |
| `memory.rs` embeddings | delete `try_embed`; FTS5-only |
| `server.rs`, `state.rs`, `tools/mod.rs` | plumb `LlmEngine` handle instead of URL strings (extends the new `ToolCtx`/`ChatRuntime` structs) |

## Error handling

- Engine not started / no model selected → agent + scrape verify return a clear
  "no model loaded — pick one in Settings" error surfaced in the UI.
- Spawn fail (missing binary, port clash, VRAM OOM) → `Failed(reason)` status
  shown verbatim in Settings; retry button. Port chosen ephemerally to avoid
  clashes.
- llama-server crash mid-run → health poll flips status to Failed; next request
  errors cleanly; Settings offers reload.

## Testing

- Unit: quant parser (filenames + GGUF metadata), mmproj pairing, catalog scan,
  engine.json round-trip, refusal of unquantized weights.
- Integration (ignored-by-default, like the live browser test): spawn real
  llama-server with a small GGUF, health check, one completion, one tool call.
- Manual: model switch mid-session, tray Quit kills child, vision verify with
  Gemma-4-E4B mmproj, agent loop tool-calling parity vs. LM Studio behavior.

## Resolved questions

1. Decisions 1–4 confirmed by the user 2026-07-02.
2. Auto-load the last-used model at startup: **yes**.
3. Tool-calling quality on Gemma-4-E4B via llama-server `--jinja` will be
   validated live during implementation; the user's larger GGUFs remain
   selectable fallbacks in the model list.
