# Embedded LLM Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the LM Studio HTTP dependency with a Bow-managed `llama-server` child process; models are picked from the user's GGUF folder in a new Settings panel, quantized-only enforced, vision kept via mmproj.

**Architecture:** New `llm_engine.rs` module owns a silently-spawned `llama-server` child (OpenAI-compatible on a private localhost port). All existing `/v1/chat/completions` call sites re-point at the engine's base URL. A model catalog scans `BOW_MODELS_DIR` for `*.gguf`, pairs `mmproj-*` projectors with their sibling models, and refuses unquantized weights. REST endpoints + a webapp Settings panel drive load/stop/switch.

**Tech Stack:** Rust (tokio, axum, reqwest — all already present), llama.cpp release `b9860` Windows binaries, React/TypeScript/Zustand webapp.

## Global Constraints

- Local inference only — no cloud/Anthropic calls (project policy).
- Child processes must be **silent**: `CREATE_NO_WINDOW` (0x08000000) on every spawn.
- Fix all compiler/linter warnings (`cargo clippy --all-targets` and `npx eslint .` must stay at zero).
- Launcher scripts stay in the repo root.
- Quantization rule: refuse loading any GGUF whose filename carries no quant tag or a full-precision tag (`F16`, `BF16`, `F32`). `mmproj-*.gguf` files are auxiliary and exempt (never listed as loadable models).
- Spec: `docs/superpowers/specs/2026-07-02-embedded-llm-design.md`. Pinned llama.cpp release: **b9860** (assets `llama-b9860-bin-win-cuda-12.4-x64.zip`, `cudart-llama-bin-win-cuda-12.4-x64.zip`, CPU fallback `llama-b9860-bin-win-cpu-x64.zip`).
- Commit after each green task. Do not push until the user asks.

---

### Task 1: Model catalog — scan, quant parse, mmproj pairing

**Files:**
- Create: `desktop/src-tauri/src/llm_engine.rs` (catalog half)
- Modify: `desktop/src-tauri/src/lib.rs` (or `main.rs` module list — wherever `pub mod tools;` lives, add `pub mod llm_engine;`)

**Interfaces:**
- Produces (used by Tasks 3, 6, 7):
  - `pub struct ModelEntry { pub path: PathBuf, pub name: String, pub size_bytes: u64, pub quant: Option<String>, pub mmproj: Option<PathBuf> }` (derives `Clone, Debug, serde::Serialize`)
  - `pub fn quant_from_filename(name: &str) -> Option<String>`
  - `pub fn is_loadable_quant(quant: &Option<String>) -> bool`
  - `pub fn scan_models(dir: &Path) -> Vec<ModelEntry>`

- [ ] **Step 1: Write failing tests** (bottom of new `llm_engine.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant_parses_common_tags() {
        assert_eq!(quant_from_filename("Gemma-4-E4B-Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_from_filename("model-iq4_xs.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(quant_from_filename("model-Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(quant_from_filename("model-BF16.gguf").as_deref(), Some("BF16"));
        assert_eq!(quant_from_filename("model-f16.gguf").as_deref(), Some("F16"));
        assert_eq!(quant_from_filename("mystery-model.gguf"), None);
    }

    #[test]
    fn loadable_rejects_full_precision_and_unknown() {
        assert!(is_loadable_quant(&Some("Q4_K_M".into())));
        assert!(is_loadable_quant(&Some("IQ2_M".into())));
        assert!(!is_loadable_quant(&Some("F16".into())));
        assert!(!is_loadable_quant(&Some("BF16".into())));
        assert!(!is_loadable_quant(&Some("F32".into())));
        assert!(!is_loadable_quant(&None));
    }

    #[test]
    fn scan_pairs_mmproj_and_skips_it_as_model() {
        let dir = std::env::temp_dir().join("bow_scan_test");
        let sub = dir.join("fam");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Foo-4B-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(sub.join("mmproj-Foo-4B-f16.gguf"), b"x").unwrap();
        std::fs::write(sub.join("Bare-F16.gguf"), b"x").unwrap();
        let models = scan_models(&dir);
        assert_eq!(models.len(), 2); // mmproj not listed as a model
        let foo = models.iter().find(|m| m.name.contains("Foo")).unwrap();
        assert!(foo.mmproj.is_some());
        let bare = models.iter().find(|m| m.name.contains("Bare")).unwrap();
        assert!(bare.mmproj.is_none());
        assert!(!is_loadable_quant(&bare.quant));
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test llm_engine` → FAIL (module/functions missing)

- [ ] **Step 3: Implement**

```rust
//! Bow-managed local LLM engine: model catalog + llama-server child process.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelEntry {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    /// Quant tag parsed from the filename (`Q4_K_M`, `IQ4_XS`, `F16`, …); `None` = no tag.
    pub quant: Option<String>,
    /// Sibling `mmproj-*.gguf` vision projector, when present in the same directory.
    pub mmproj: Option<PathBuf>,
}

/// Parse a quantization tag out of a GGUF filename. Uppercased on return.
pub fn quant_from_filename(name: &str) -> Option<String> {
    let stem = name.trim_end_matches(".gguf");
    let upper = stem.to_ascii_uppercase();
    // Split on separators and take the last token that looks like a quant tag.
    for tok in upper.rsplit(['-', '_', '.']) {
        if matches!(tok, "F16" | "BF16" | "F32") {
            return Some(tok.to_string());
        }
    }
    // Q/IQ tags may themselves contain '_' (Q4_K_M) — regex-free scan on the whole stem.
    let bytes = upper.as_bytes();
    for i in 0..bytes.len() {
        let rest = &upper[i..];
        let q = rest.strip_prefix("IQ").or_else(|| rest.strip_prefix('Q').map(|r| r));
        if let Some(r) = q {
            let mut end = 0;
            let rb = r.as_bytes();
            if !rb.is_empty() && rb[0].is_ascii_digit() {
                end = 1;
                while end < rb.len() && (rb[end].is_ascii_alphanumeric() || rb[end] == b'_') {
                    end += 1;
                }
                let tag = &upper[i..i + (rest.len() - r.len()) + end];
                return Some(tag.trim_end_matches('_').to_string());
            }
        }
    }
    None
}

/// Quantized-only rule: full-precision or untagged weights are refused.
pub fn is_loadable_quant(quant: &Option<String>) -> bool {
    match quant.as_deref() {
        Some("F16") | Some("BF16") | Some("F32") | None => false,
        Some(_) => true,
    }
}

fn is_mmproj(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().starts_with("mmproj")
}

/// Recursively scan `dir` for GGUF models; pair each with an mmproj in its directory.
pub fn scan_models(dir: &Path) -> Vec<ModelEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let entries: Vec<_> = rd.flatten().collect();
        let mmproj: Option<PathBuf> = entries
            .iter()
            .find(|e| {
                e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".gguf")
                    && is_mmproj(&e.file_name().to_string_lossy())
            })
            .map(|e| e.path());
        for e in &entries {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let fname = e.file_name().to_string_lossy().to_string();
            if !fname.to_ascii_lowercase().ends_with(".gguf") || is_mmproj(&fname) {
                continue;
            }
            let size_bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(ModelEntry {
                name: fname.trim_end_matches(".gguf").to_string(),
                quant: quant_from_filename(&fname),
                mmproj: mmproj.clone(),
                path: p,
                size_bytes,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
```

- [ ] **Step 4: Run tests** — `cargo test llm_engine` → all 3 PASS. Also `cargo clippy --all-targets` → zero warnings (adjust until so).
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat: GGUF model catalog with quant enforcement and mmproj pairing"`

---

### Task 2: Engine state persistence (`engine.json`)

**Files:**
- Modify: `desktop/src-tauri/src/llm_engine.rs`

**Interfaces:**
- Produces (used by Tasks 3, 5):
  - `pub struct EnginePersist { pub model_path: Option<PathBuf>, pub ctx_size: u32 }` (derives `Clone, Debug, serde::Serialize, serde::Deserialize`; `Default` = `{ None, 8192 }`)
  - `pub fn persist_path() -> PathBuf` — `engine.json` next to the running exe
  - `pub fn load_persist(path: &Path) -> EnginePersist` (missing/corrupt file → default)
  - `pub fn save_persist(path: &Path, p: &EnginePersist)` (best-effort, logs on failure)

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn persist_round_trips_and_defaults() {
        let f = std::env::temp_dir().join("bow_engine_test.json");
        std::fs::remove_file(&f).ok();
        assert_eq!(load_persist(&f).ctx_size, 8192); // missing → default
        let p = EnginePersist { model_path: Some(PathBuf::from(r"C:\m\a.gguf")), ctx_size: 4096 };
        save_persist(&f, &p);
        let back = load_persist(&f);
        assert_eq!(back.model_path, p.model_path);
        assert_eq!(back.ctx_size, 4096);
        std::fs::write(&f, "not json").unwrap();
        assert_eq!(load_persist(&f).ctx_size, 8192); // corrupt → default
        std::fs::remove_file(&f).ok();
    }
```

- [ ] **Step 2: Verify fail** — `cargo test persist_round` → FAIL
- [ ] **Step 3: Implement**

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EnginePersist {
    pub model_path: Option<PathBuf>,
    pub ctx_size: u32,
}
impl Default for EnginePersist {
    fn default() -> Self { EnginePersist { model_path: None, ctx_size: 8192 } }
}

pub fn persist_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("engine.json")))
        .unwrap_or_else(|| PathBuf::from("engine.json"))
}

pub fn load_persist(path: &Path) -> EnginePersist {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_persist(path: &Path, p: &EnginePersist) {
    match serde_json::to_string_pretty(p) {
        Ok(s) => {
            if let Err(e) = std::fs::write(path, s) {
                tracing::warn!("engine.json write failed: {}", e);
            }
        }
        Err(e) => tracing::warn!("engine.json serialize failed: {}", e),
    }
}
```

- [ ] **Step 4: Run** — `cargo test persist_round` → PASS; clippy clean.
- [ ] **Step 5: Commit** — `git commit -am "feat: engine.json persistence for selected model + ctx size"`

---

### Task 3: `LlmEngine` process manager

**Files:**
- Modify: `desktop/src-tauri/src/llm_engine.rs`

**Interfaces:**
- Consumes: `ModelEntry`, `is_loadable_quant`, `EnginePersist`, `save_persist`, `persist_path` (Tasks 1–2)
- Produces (used by Tasks 5–8):
  - `#[derive(Clone)] pub struct LlmEngine` — `LlmEngine::new(bin_dir: PathBuf) -> Self`
  - `pub async fn load(&self, entry: ModelEntry, ctx_size: u32) -> anyhow::Result<()>`
  - `pub async fn stop(&self)`
  - `pub async fn status(&self) -> EngineStatus`
  - `pub struct EngineStatus { pub state: String /* "stopped"|"starting"|"ready"|"failed" */, pub error: Option<String>, pub model: Option<ModelEntry>, pub base_url: Option<String>, pub vision: bool }` (derives `Clone, Debug, serde::Serialize`)
  - Convention: chat-completions `model` field = `ModelEntry.name` (llama-server serves one model and ignores mismatches).

- [ ] **Step 1: Failing test** (no real binary needed — validates refusal + stopped status)

```rust
    #[tokio::test]
    async fn engine_refuses_unquantized_and_reports_stopped() {
        let eng = LlmEngine::new(std::env::temp_dir().join("no_bin_dir"));
        let st = eng.status().await;
        assert_eq!(st.state, "stopped");
        assert!(st.base_url.is_none());
        let bad = ModelEntry {
            path: PathBuf::from(r"C:\m\big-F16.gguf"), name: "big-F16".into(),
            size_bytes: 1, quant: Some("F16".into()), mmproj: None,
        };
        let err = eng.load(bad, 8192).await.unwrap_err().to_string();
        assert!(err.contains("unquantized"), "err was: {}", err);
    }
```

- [ ] **Step 2: Verify fail** — `cargo test engine_refuses` → FAIL
- [ ] **Step 3: Implement**

```rust
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct EngineInner {
    child: Option<tokio::process::Child>,
    state: String,
    error: Option<String>,
    model: Option<ModelEntry>,
    port: Option<u16>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct EngineStatus {
    pub state: String,
    pub error: Option<String>,
    pub model: Option<ModelEntry>,
    pub base_url: Option<String>,
    pub vision: bool,
}

#[derive(Clone)]
pub struct LlmEngine {
    inner: Arc<Mutex<EngineInner>>,
    bin_dir: PathBuf,
}

impl LlmEngine {
    pub fn new(bin_dir: PathBuf) -> Self {
        LlmEngine {
            inner: Arc::new(Mutex::new(EngineInner {
                child: None, state: "stopped".into(), error: None, model: None, port: None,
            })),
            bin_dir,
        }
    }

    fn server_exe(&self) -> PathBuf { self.bin_dir.join("llama-server.exe") }

    async fn free_port() -> Result<u16> {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        Ok(l.local_addr()?.port())
    }

    pub async fn status(&self) -> EngineStatus {
        let g = self.inner.lock().await;
        EngineStatus {
            state: g.state.clone(),
            error: g.error.clone(),
            model: g.model.clone(),
            base_url: g.port.map(|p| format!("http://127.0.0.1:{}", p)),
            vision: g.model.as_ref().map(|m| m.mmproj.is_some()).unwrap_or(false),
        }
    }

    pub async fn stop(&self) {
        let mut g = self.inner.lock().await;
        if let Some(mut c) = g.child.take() {
            let _ = c.kill().await;
        }
        g.state = "stopped".into();
        g.port = None;
        g.model = None;
        g.error = None;
    }

    /// Stop any running server, spawn llama-server on `entry`, wait for /health.
    pub async fn load(&self, entry: ModelEntry, ctx_size: u32) -> Result<()> {
        if !is_loadable_quant(&entry.quant) {
            return Err(anyhow!(
                "'{}' is unquantized (tag {:?}) — Bow only loads quantized GGUFs (Q*/IQ*)",
                entry.name, entry.quant
            ));
        }
        let exe = self.server_exe();
        if !exe.exists() {
            return Err(anyhow!(
                "llama-server.exe not found at {} — run get-llama.ps1 (repo root) first",
                exe.display()
            ));
        }
        self.stop().await;
        let port = Self::free_port().await?;
        {
            let mut g = self.inner.lock().await;
            g.state = "starting".into();
            g.port = Some(port);
            g.model = Some(entry.clone());
        }
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("-m").arg(&entry.path)
            .arg("--host").arg("127.0.0.1")
            .arg("--port").arg(port.to_string())
            .arg("--jinja")
            .arg("-ngl").arg("999")
            .arg("-c").arg(ctx_size.to_string())
            .arg("--no-webui")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Some(mm) = &entry.mmproj {
            cmd.arg("--mmproj").arg(mm);
        }
        let child = cmd.spawn().map_err(|e| anyhow!("spawn llama-server: {}", e))?;
        {
            let mut g = self.inner.lock().await;
            g.child = Some(child);
        }
        // Poll /health until ready (model load can take a while on first touch).
        let url = format!("http://127.0.0.1:{}/health", port);
        let client = reqwest::Client::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Ok(r) = client.get(&url).send().await {
                if r.status().is_success() {
                    let mut g = self.inner.lock().await;
                    g.state = "ready".into();
                    save_persist(&persist_path(), &EnginePersist {
                        model_path: Some(entry.path.clone()), ctx_size,
                    });
                    return Ok(());
                }
            }
            // Child died?
            {
                let mut g = self.inner.lock().await;
                if let Some(c) = g.child.as_mut() {
                    if let Ok(Some(status)) = c.try_wait() {
                        g.state = "failed".into();
                        g.error = Some(format!("llama-server exited: {}", status));
                        g.child = None;
                        g.port = None;
                        return Err(anyhow!("llama-server exited during startup ({})", status));
                    }
                }
            }
            if std::time::Instant::now() > deadline {
                self.stop().await;
                let mut g = self.inner.lock().await;
                g.state = "failed".into();
                g.error = Some("startup timed out after 180s".into());
                return Err(anyhow!("llama-server startup timed out"));
            }
        }
    }
}
```

- [ ] **Step 4: Run** — `cargo test engine_refuses` → PASS; clippy clean.
- [ ] **Step 5: Add ignored live integration test + commit**

```rust
    #[tokio::test]
    #[ignore = "needs llama-server.exe + a real GGUF; run manually with --ignored"]
    async fn engine_loads_real_model_live() {
        let eng = LlmEngine::new(PathBuf::from(r"C:\AI\agent Bow\desktop\src-tauri\bin\llama"));
        let models = scan_models(Path::new(r"C:\AI\models"));
        let m = models.into_iter().find(|m| is_loadable_quant(&m.quant)).expect("a model");
        eng.load(m, 4096).await.expect("load");
        assert_eq!(eng.status().await.state, "ready");
        eng.stop().await;
    }
```

`git commit -am "feat: LlmEngine llama-server child-process manager"`

---

### Task 4: Binary acquisition — `get-llama.ps1` + `bow.bat` wiring

**Files:**
- Create: `get-llama.ps1` (repo root)
- Modify: `bow.bat` (repo root)

**Interfaces:**
- Produces: `desktop/src-tauri/bin/llama/llama-server.exe` (+ DLLs); `bow.bat` copies `bin/llama` → `target/debug/llama` so `LlmEngine::new(<exe_dir>/llama)` resolves at runtime.

- [ ] **Step 1: Write `get-llama.ps1`**

```powershell
# Downloads the pinned llama.cpp server build into desktop\src-tauri\bin\llama.
# CUDA build when an NVIDIA driver is present, CPU build otherwise. Idempotent.
$ErrorActionPreference = "Stop"
$tag = "b9860"
$dest = Join-Path $PSScriptRoot "desktop\src-tauri\bin\llama"
if (Test-Path (Join-Path $dest "llama-server.exe")) { Write-Host "llama-server present — skipping"; exit 0 }
New-Item -ItemType Directory -Force $dest | Out-Null
$hasNvidia = $null -ne (Get-Command nvidia-smi -ErrorAction SilentlyContinue)
$assets = if ($hasNvidia) {
    @("llama-$tag-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip")
} else {
    @("llama-$tag-bin-win-cpu-x64.zip")
}
foreach ($a in $assets) {
    $url = "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$a"
    $zip = Join-Path $env:TEMP $a
    Write-Host "Downloading $a ..."
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $dest -Force
    Remove-Item $zip
}
Write-Host "llama-server ready in $dest"
```

- [ ] **Step 2: Wire into `bow.bat`** — after the existing webapp xcopy block, add:

```bat
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0get-llama.ps1" || goto :err
if not exist "target\debug\llama" mkdir "target\debug\llama"
xcopy /E /I /Y "bin\llama\*" "target\debug\llama\" >nul || goto :err
```

(Note: the `pushd desktop\src-tauri` block is active at that point, so relative paths are `bin\llama`.) Add `desktop/src-tauri/bin/` to `.gitignore`.

- [ ] **Step 3: Verify** — run `powershell -File get-llama.ps1`; expect download + `llama-server.exe` in `desktop\src-tauri\bin\llama`. Run again → "skipping".
- [ ] **Step 4: Live engine test** — `cargo test engine_loads_real_model_live -- --ignored --nocapture` → PASS (loads Gemma-4-E4B from `C:\AI\models`).
- [ ] **Step 5: Commit** — `git add get-llama.ps1 bow.bat .gitignore && git commit -m "build: fetch pinned llama.cpp b9860 server binaries"`

---

### Task 5: Config migration + AppState + startup auto-load

**Files:**
- Modify: `desktop/src-tauri/src/state.rs` (Config + AppState)
- Modify: `desktop/src-tauri/src/host.rs` (auto-load spawn, engine kill on Quit)
- Modify: `desktop/.env` (new keys; keep LM_STUDIO_* until Task 7 removes their readers)

**Interfaces:**
- Consumes: `LlmEngine`, `load_persist`, `persist_path`, `scan_models` (Tasks 1–3)
- Produces: `Config.models_dir: PathBuf` (env `BOW_MODELS_DIR`, default `C:\AI\models`), `Config.ctx_size: u32` (env `BOW_CTX_SIZE`, default 8192), `AppState.llm_engine: LlmEngine`

- [ ] **Step 1: Failing test** (in `state.rs` tests)

```rust
    #[test]
    fn config_default_models_dir() {
        std::env::remove_var("BOW_MODELS_DIR");
        let c = Config::test_default(std::path::PathBuf::from(r"C:\tmp"));
        assert_eq!(c.models_dir, std::path::PathBuf::from(r"C:\AI\models"));
        assert_eq!(c.ctx_size, 8192);
    }
```

- [ ] **Step 2: Verify fail** — `cargo test config_default_models_dir` → FAIL (no such fields)
- [ ] **Step 3: Implement**
  - Add to `Config`: `pub models_dir: PathBuf, pub ctx_size: u32`. In `from_env()`: `BOW_MODELS_DIR` default `C:\AI\models`; `BOW_CTX_SIZE` parsed u32 default 8192. Mirror in `test_default`.
  - `AppState`: add `pub llm_engine: LlmEngine`; in `new()`: `llm_engine: LlmEngine::new(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("llama"))).unwrap_or_else(|| PathBuf::from("llama")))`.
  - `host.rs`: right after the axum serve spawn, add auto-load:

```rust
    // Auto-load the last-used model (engine.json), if it still exists.
    {
        let engine = state.llm_engine.clone();
        let ctx = config.ctx_size;
        let models_dir = config.models_dir.clone();
        rt.spawn(async move {
            let p = crate::llm_engine::load_persist(&crate::llm_engine::persist_path());
            if let Some(path) = p.model_path {
                if path.exists() {
                    if let Some(entry) = crate::llm_engine::scan_models(&models_dir)
                        .into_iter().find(|m| m.path == path)
                    {
                        if let Err(e) = engine.load(entry, ctx).await {
                            tracing::warn!("auto-load failed: {}", e);
                        }
                    }
                }
            }
        });
    }
```

  - In the tray Quit handler (where the app exits), call a blocking stop first: `rt.block_on(state.llm_engine.stop());` (match how host.rs already accesses state/config there — thread the `AppState` clone in).
  - `.env`: add `BOW_MODELS_DIR=C:\AI\models` and `BOW_CTX_SIZE=8192`.
- [ ] **Step 4: Run** — `cargo test` → all pass; `cargo clippy --all-targets` → zero warnings.
- [ ] **Step 5: Commit** — `git commit -am "feat: engine in AppState, BOW_MODELS_DIR/BOW_CTX_SIZE, startup auto-load"`

---

### Task 6: REST endpoints for engine + catalog

**Files:**
- Modify: `desktop/src-tauri/src/web_api.rs` (new handlers, added to its `routes()` Router)
- Test: same file, following the existing web_api test pattern (axum `oneshot`)

**Interfaces:**
- Consumes: `AppState.llm_engine`, `Config.models_dir`, `Config.ctx_size`, `scan_models`, `is_loadable_quant` (Tasks 1, 3, 5); `HttpState` (existing, `http.rs:12`)
- Produces (consumed by Task 9 webapp):
  - `GET /api/engine` → `EngineStatus` JSON
  - `GET /api/models` → `{ "dir": string, "models": [{ name, path, size_bytes, quant, vision, loadable }] }`
  - `POST /api/engine/load` body `{ "path": string }` → 200 `{ "ok": true }` or 400 `{ "error": string }`
  - `POST /api/engine/stop` → `{ "ok": true }`
  - `POST /api/engine/models-dir` body `{ "dir": string }` → same shape as `GET /api/models` (persists to nothing — env stays authoritative next boot; runtime override lives in a `Mutex<PathBuf>` added to `HttpState`… **no** — keep YAGNI: runtime dir change only affects the current session via `AppState`-level `Arc<Mutex<PathBuf>>` field `models_dir_override` on `LlmEngine`; simpler: add `pub models_dir: Arc<std::sync::Mutex<PathBuf>>` to `AppState`, initialized from config, read by both models handlers)

- [ ] **Step 1: Failing test**

```rust
    #[tokio::test]
    async fn engine_status_endpoint_returns_stopped() {
        let state = crate::http::HttpState::test_state(std::env::temp_dir());
        let app = routes().with_state(state);
        let res = app.oneshot(Request::builder().uri("/api/engine").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("\"state\":\"stopped\""));
    }

    #[tokio::test]
    async fn engine_load_rejects_bad_path() {
        let state = crate::http::HttpState::test_state(std::env::temp_dir());
        let app = routes().with_state(state);
        let res = app.oneshot(Request::builder().method("POST").uri("/api/engine/load")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"path":"C:\\nope\\missing.gguf"}"#)).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
```

- [ ] **Step 2: Verify fail** — `cargo test engine_status_endpoint` → FAIL (route missing)
- [ ] **Step 3: Implement handlers** (in `web_api.rs`, mirroring its existing handler style)

```rust
async fn engine_status(State(s): State<crate::http::HttpState>) -> Json<serde_json::Value> {
    let st = s.app.llm_engine.status().await;
    Json(serde_json::to_value(st).unwrap_or_else(|_| serde_json::json!({"state":"stopped"})))
}

fn models_payload(dir: &std::path::Path) -> serde_json::Value {
    let models: Vec<serde_json::Value> = crate::llm_engine::scan_models(dir)
        .into_iter()
        .map(|m| serde_json::json!({
            "name": m.name, "path": m.path, "size_bytes": m.size_bytes,
            "quant": m.quant, "vision": m.mmproj.is_some(),
            "loadable": crate::llm_engine::is_loadable_quant(&m.quant),
        }))
        .collect();
    serde_json::json!({ "dir": dir.to_string_lossy(), "models": models })
}

async fn engine_models(State(s): State<crate::http::HttpState>) -> Json<serde_json::Value> {
    let dir = s.app.models_dir.lock().unwrap().clone();
    Json(models_payload(&dir))
}

#[derive(serde::Deserialize)]
struct LoadReq { path: String }

async fn engine_load(
    State(s): State<crate::http::HttpState>,
    Json(req): Json<LoadReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let dir = s.app.models_dir.lock().unwrap().clone();
    let entry = crate::llm_engine::scan_models(&dir)
        .into_iter()
        .find(|m| m.path == std::path::PathBuf::from(&req.path))
        .ok_or_else(|| (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "model not found in models dir"}))))?;
    s.app.llm_engine.load(entry, s.app.config.ctx_size).await
        .map(|_| Json(serde_json::json!({"ok": true})))
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))))
}

async fn engine_stop(State(s): State<crate::http::HttpState>) -> Json<serde_json::Value> {
    s.app.llm_engine.stop().await;
    Json(serde_json::json!({"ok": true}))
}

#[derive(serde::Deserialize)]
struct DirReq { dir: String }

async fn engine_models_dir(
    State(s): State<crate::http::HttpState>,
    Json(req): Json<DirReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let p = std::path::PathBuf::from(&req.dir);
    if !p.is_dir() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "not a directory"}))));
    }
    *s.app.models_dir.lock().unwrap() = p.clone();
    Ok(Json(models_payload(&p)))
}
```

Add to `routes()`:

```rust
        .route("/api/engine", get(engine_status))
        .route("/api/models", get(engine_models))
        .route("/api/engine/load", post(engine_load))
        .route("/api/engine/stop", post(engine_stop))
        .route("/api/engine/models-dir", post(engine_models_dir))
```

And add to `AppState` (state.rs): `pub models_dir: Arc<std::sync::Mutex<PathBuf>>`, initialized `Arc::new(std::sync::Mutex::new(config.models_dir.clone()))` in `new()`.

- [ ] **Step 4: Run** — `cargo test` → PASS incl. the two new; clippy clean.
- [ ] **Step 5: Commit** — `git commit -am "feat: REST endpoints for engine status, model catalog, load/stop"`

---

### Task 7: Migrate all LM Studio call sites to the engine

**Files:**
- Modify: `desktop/src-tauri/src/local_llm.rs` (chat URL/model from engine; delete `query_model_reasoning` + reasoning payload fields)
- Modify: `desktop/src-tauri/src/tools/mod.rs` (`ToolCtx` fields `lm_studio_url`/`lm_studio_model` → `llm_base_url`/`llm_model`)
- Modify: `desktop/src-tauri/src/tools/image_search.rs` (`ScrapeTuning` → `{ llm_base_url: String, llm_model: String, vision: bool, … }`; delete `pick_loaded_vision_model` + `/api/v0/models` fetch + their tests; verify gate skips with a `Phase` event when `!vision`)
- Modify: `desktop/src-tauri/src/tools/web_search.rs` (params rename only)
- Modify: `desktop/src-tauri/src/tools/memory.rs` (delete `try_embed` and the embeddings re-rank block; `memory_store`/`memory_retrieve` lose the `lm_studio_url` param; embedding column simply stays NULL)
- Modify: `desktop/src-tauri/src/server.rs` (`ChatRuntime` gains `engine: LlmEngine`; `ScrapeRequest` builds tuning from `engine.status()`; error event `"No model loaded — open Settings"` when engine not ready)
- Modify: `desktop/src-tauri/src/state.rs` (delete `lm_studio_url`, `lm_studio_model`, `lm_studio_vision_model`, `reasoning_effort`, `reasoning_tokens` from `Config` + `from_env` + `test_default`)
- Modify: `desktop/.env` (delete `LM_STUDIO_*` lines)

**Interfaces:**
- Consumes: `LlmEngine::status()` → `EngineStatus { base_url, model.name, vision, state }` (Task 3)
- Produces: `ChatRuntime { config, engine, shell_session, browser, mcp }`; `ToolCtx { llm_base_url, llm_model, … }`; `ScrapeTuning { delay_ms, verify, vision_prompt, llm_base_url, llm_model, vision, dedupe, sources }`
- Gate pattern used everywhere a completion is made:

```rust
let st = engine.status().await;
let (Some(base_url), Some(model)) = (st.base_url.clone(), st.model.as_ref().map(|m| m.name.clone())) else {
    return Err(anyhow!("No model loaded — open Settings and load a model"));
};
```

- [ ] **Step 1: Grep the full surface** — `rg -n "lm_studio|LM_STUDIO" desktop/src-tauri` and convert every hit per the mapping above. Chat-completion request bodies keep their existing shape minus `reasoning_effort`/`reasoning_tokens` fields; URLs become `format!("{}/v1/chat/completions", base_url)`.
- [ ] **Step 2: Vision gate** — in `image_download`, where the verify config is built, replace model auto-detect with:

```rust
let verify_cfg = if tuning.verify && tuning.vision {
    Some(VerifyConfig { base_url: tuning.llm_base_url.clone(), model: tuning.llm_model.clone(),
                        prompt: tuning.vision_prompt.clone() })
} else {
    if tuning.verify && !tuning.vision {
        emit(ScrapeEvent::Phase { label: "Loaded model has no vision — verify skipped".into() });
    }
    None
};
```

(`VerifyConfig` fields rename from their LM Studio names to `base_url`/`model`; adjust its construction + use in `download_urls_to_dir`'s QA call.)
- [ ] **Step 3: Compile-run loop** — `cargo clippy --all-targets` until zero errors/warnings; update every existing test that constructed `Config`/`ScrapeTuning`/`ToolCtx` shapes.
- [ ] **Step 4: Run tests** — `cargo test` → all pass. Verify `rg -n "lm_studio" desktop/src-tauri` → **zero hits**.
- [ ] **Step 5: Commit** — `git commit -am "feat!: all model calls route to the embedded engine; LM Studio removed"`

---

### Task 8: Webapp — Settings panel ("The Workshop")

**Files:**
- Modify: `desktop/webapp/src/api.ts` (engine API client)
- Create: `desktop/webapp/src/components/SettingsPanel.tsx`
- Modify: `desktop/webapp/src/App.tsx` (nav item + view switch)
- Modify: `desktop/webapp/src/store.ts` (engine status slice for the verify toggle)
- Modify: `desktop/webapp/src/components/SearchPanel.tsx` (disable Verify toggle when `!engine.vision`)
- Test: `desktop/webapp/src/api.test.ts` or existing vitest file pattern

**Interfaces:**
- Consumes: Task 6 endpoints.
- Produces (api.ts):

```ts
export interface EngineStatus { state: "stopped" | "starting" | "ready" | "failed"; error: string | null; base_url: string | null; vision: boolean; model: { name: string } | null }
export interface EngineModel { name: string; path: string; size_bytes: number; quant: string | null; vision: boolean; loadable: boolean }
export async function engineStatus(): Promise<EngineStatus>
export async function listModels(): Promise<{ dir: string; models: EngineModel[] }>
export async function loadModel(path: string): Promise<{ ok?: boolean; error?: string }>
export async function stopEngine(): Promise<void>
export async function setModelsDir(dir: string): Promise<{ dir: string; models: EngineModel[] }>
```

- [ ] **Step 1: api.ts additions** — same `fetch` style as the existing functions (console.error + safe fallback on !ok).
- [ ] **Step 2: SettingsPanel.tsx** — Agent 008 styling via existing CSS vars + `ui/Button`. Layout: models-dir text input + "Rescan" Button (calls `setModelsDir`); status line (`state`, model name, error verbatim when failed, "Stop" Button when ready); model table — name, quant `Tag`, `GB` size, vision badge, per-row "Load" Button (disabled when `!loadable`, title "unquantized — not loadable"). Poll `engineStatus()` every 3s via `useEffect` interval **setting state only inside the promise callback** (same lint rule as CurationGrid). While `state === "starting"` show "Loading model — first load can take a minute…".
- [ ] **Step 3: App.tsx** — add `Settings` lucide icon to `HOUSE_ITEMS` as "The Workshop"; add `const [view, setView] = useState<"job" | "settings">("job")`; clicking Workshop sets `view`, main column renders `<SettingsPanel />` when `view === "settings"`. Store: add `engine: EngineStatus | null` + `setEngine`; App polls `engineStatus()` every 5s into the store; SearchPanel greys the Verify switch (`disabled`, hint "Loaded model has no vision") when `engine && !engine.vision`.
- [ ] **Step 4: Tests + lint** — vitest for the api helpers with `fetch` mocked (status parse, error path returns fallback); `npm run test && npx eslint . && npm run build` → all green.
- [ ] **Step 5: Commit** — `git commit -am "feat: Workshop settings panel — model catalog, load/stop, engine status"`

---

### Task 9: Cleanup, docs, end-to-end verification

**Files:**
- Modify: `CLAUDE.md` (What-Bow-is paragraph: LM Studio → embedded llama-server engine)
- Modify: `desktop/.env` (confirm only BOW_* + TAVILY/SEARXNG keys remain)

- [ ] **Step 1: Doc sweep** — update CLAUDE.md ("All model calls go to a **Bow-managed llama-server child process** — no external server, no cloud"); check README if it mentions LM Studio.
- [ ] **Step 2: Full gates** — `cargo clippy --all-targets` (0 warnings), `cargo test` (all pass), `cd desktop/webapp && npx eslint . && npm test && npm run build` (all green), `rg -in "lm.?studio" --glob !docs` → zero code hits.
- [ ] **Step 3: Live end-to-end** (manual, with the user or via `bow.bat`):
  1. `bow.bat` → app builds, llama binaries fetched/copied, Edge opens the UI.
  2. Workshop panel lists `C:\AI\models` GGUFs; Gemma-4-E4B shows quant `Q4_K_M` + vision badge; the BF16 mmproj is absent from the list; `Qwen3.6-35B-A3B-Q4_K_M` listed.
  3. Load Gemma-4-E4B → status `starting` → `ready` (llama-server silent, no window).
  4. Console chat: ask a question that forces a tool call (e.g. "list files in the workspace") → tool call round-trips.
  5. Scrape with Verify ON → vision QA judges images; scrape events flow.
  6. Switch model to the Qwen 35B → old process dies, new loads, chat works.
  7. Tray Quit → `llama-server.exe` gone from Task Manager.
  8. Relaunch → last model auto-loads.
- [ ] **Step 4: Commit** — `git commit -am "docs: Bow runs its own embedded llama-server engine"`

---

## Self-review notes

- Spec coverage: engine child process (T3), binary acquisition (T4), catalog + quant rule (T1), persistence + auto-load (T2/T5), REST (T6), call-site migration incl. vision gate + embeddings removal + reasoning-probe removal (T7), Settings UI + verify-toggle disable (T8), cleanup/e2e (T9). Ephemeral port, silent spawn, kill-on-quit in T3/T5.
- Deviation from spec recorded: quant detection is **filename-tag only** (no GGUF metadata parse) — conservative direction: untagged files are refused, satisfying "insist on quantized" with far less code. Spec updated alongside this plan.
- Type consistency: `EngineStatus.state` is a string enum (`"stopped"|"starting"|"ready"|"failed"`) in both Rust serialization and TS; `ModelEntry.name` doubles as the chat `model` field everywhere.
