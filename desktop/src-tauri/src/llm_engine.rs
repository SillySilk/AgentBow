//! Bow-managed local LLM engine: model catalog + llama-server child process.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        let q = rest.strip_prefix("IQ").or_else(|| rest.strip_prefix('Q').map(|_| &rest[1..]));
        if let Some(r) = q {
            let rb = r.as_bytes();
            if !rb.is_empty() && rb[0].is_ascii_digit() {
                let mut end = 1;
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
#[allow(dead_code)]
pub fn is_loadable_quant(quant: &Option<String>) -> bool {
    match quant.as_deref() {
        Some("F16") | Some("BF16") | Some("F32") | None => false,
        Some(_) => true,
    }
}

#[allow(dead_code)]
fn is_mmproj(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().starts_with("mmproj")
}

/// Extract base model name without quantization tag
#[allow(dead_code)]
fn model_base_name(stem: &str) -> String {
    // Use the same logic as quant_from_filename to detect and remove the quant tag
    let upper = stem.to_ascii_uppercase();

    // Try full-precision tags first (they don't contain underscores in the quant itself)
    for tok in upper.rsplit(['-', '_', '.']) {
        if matches!(tok, "F16" | "BF16" | "F32") {
            // Found a full-precision tag, remove everything from this separator onward
            let tag_pos = upper.rfind(tok).unwrap();
            // Find the last separator before this token
            if let Some(sep_pos) = upper[..tag_pos].rfind(['-', '_', '.']) {
                return stem[..sep_pos].to_ascii_lowercase();
            } else {
                // Tag is at the start, which is unusual
                return stem.to_ascii_lowercase();
            }
        }
    }

    // For Q/IQ tags, scan the stem from right to left for the pattern
    let bytes = upper.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'-' || bytes[i] == b'_' {
            // Check if what follows looks like a quant tag (Q4_K_M, IQ4_XS, ...)
            let after = &upper[i + 1..];
            let prefix_len = if after.starts_with("IQ") {
                2
            } else if after.starts_with('Q') {
                1
            } else {
                0
            };
            if prefix_len > 0
                && after[prefix_len..].chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                // This looks like a quant tag, trim it
                return stem[..i].to_ascii_lowercase();
            }
        }
    }

    // No quant tag found, return as-is (lowercase)
    stem.to_ascii_lowercase()
}

/// Recursively scan `dir` for GGUF models; pair each with an mmproj in its directory.
#[allow(dead_code)]
pub fn scan_models(dir: &Path) -> Vec<ModelEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let entries: Vec<_> = rd.flatten().collect();
        // Collect all mmproj files in this directory
        let mmprojs: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".gguf")
                    && is_mmproj(&e.file_name().to_string_lossy())
            })
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .collect();
        // Count model (non-mmproj) GGUF files here — enables the single-model fallback below.
        let model_count = entries
            .iter()
            .filter(|e| {
                let f = e.file_name().to_string_lossy().to_string();
                e.path().is_file()
                    && f.to_ascii_lowercase().ends_with(".gguf")
                    && !is_mmproj(&f)
            })
            .count();

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

            // Find matching mmproj by comparing base model names (without quant)
            let model_base = fname.trim_end_matches(".gguf");
            let model_name = model_base_name(model_base);
            let mmproj = mmprojs
                .iter()
                .find(|(mmproj_name, _)| {
                    // Drop the leading "mmproj" marker (any case) plus separators.
                    let stem = mmproj_name.trim_end_matches(".gguf");
                    let rest = stem
                        .get("mmproj".len()..)
                        .unwrap_or("")
                        .trim_start_matches(['-', '_', '.']);
                    model_base_name(rest) == model_name
                })
                .map(|(_, path)| path.clone())
                // HF-style layout: one model per directory — pair the sole model with the
                // sole mmproj even when their names don't match (e.g. "mmproj-model-f16").
                .or_else(|| {
                    if model_count == 1 && mmprojs.len() == 1 {
                        Some(mmprojs[0].1.clone())
                    } else {
                        None
                    }
                });

            let size_bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(ModelEntry {
                name: model_base.to_string(),
                quant: quant_from_filename(&fname),
                mmproj,
                path: p,
                size_bytes,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct EnginePersist {
    pub model_path: Option<PathBuf>,
    pub ctx_size: u32,
}

impl Default for EnginePersist {
    fn default() -> Self {
        EnginePersist {
            model_path: None,
            ctx_size: 8192,
        }
    }
}

#[allow(dead_code)]
pub fn persist_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("engine.json")))
        .unwrap_or_else(|| PathBuf::from("engine.json"))
}

#[allow(dead_code)]
pub fn load_persist(path: &Path) -> EnginePersist {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[allow(dead_code)]
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
    /// Bumped by every `load()` takeover and every `stop()`. In-flight `load()` calls
    /// remember the generation they started under and refuse to touch shared state
    /// once it has moved on, so overlapping loads/stops cannot stomp each other.
    generation: u64,
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
#[allow(dead_code)]
pub struct LlmEngine {
    inner: Arc<Mutex<EngineInner>>,
    bin_dir: PathBuf,
}

#[allow(dead_code)]
impl LlmEngine {
    pub fn new(bin_dir: PathBuf) -> Self {
        LlmEngine {
            inner: Arc::new(Mutex::new(EngineInner {
                child: None, state: "stopped".into(), error: None, model: None, port: None,
                generation: 0,
            })),
            bin_dir,
        }
    }

    fn server_exe(&self) -> PathBuf { self.bin_dir.join("llama-server.exe") }

    /// Bind-then-drop has a TOCTOU window (another process could grab the port before
    /// llama-server binds it) — accepted tradeoff for a local, single-user tool.
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
        g.generation += 1; // invalidate any in-flight load()
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
        let my_gen = {
            let mut g = self.inner.lock().await;
            g.generation += 1; // take over: any older in-flight load() is now superseded
            g.state = "starting".into();
            g.port = Some(port);
            g.model = Some(entry.clone());
            g.generation
        };
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
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Some(mm) = &entry.mmproj {
            cmd.arg("--mmproj").arg(mm);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("spawn llama-server: {}", e);
                let mut g = self.inner.lock().await;
                if g.generation == my_gen {
                    g.state = "failed".into();
                    g.error = Some(msg.clone());
                    g.port = None;
                }
                return Err(anyhow!(msg));
            }
        };
        {
            let mut g = self.inner.lock().await;
            if g.generation != my_gen {
                // A newer load()/stop() took over while we were spawning: this child
                // belongs to no one — kill it and bail without touching shared state.
                drop(g);
                let _ = child.kill().await;
                return Err(anyhow!("superseded by a newer load"));
            }
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
                    {
                        let mut g = self.inner.lock().await;
                        if g.generation != my_gen {
                            return Err(anyhow!("superseded by a newer load"));
                        }
                        g.state = "ready".into();
                    }
                    // Persist outside the lock — file I/O must not block status()/stop().
                    save_persist(&persist_path(), &EnginePersist {
                        model_path: Some(entry.path.clone()), ctx_size,
                    });
                    return Ok(());
                }
            }
            // Child died?
            {
                let mut g = self.inner.lock().await;
                if g.generation != my_gen {
                    return Err(anyhow!("superseded by a newer load"));
                }
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
                let mut g = self.inner.lock().await;
                if g.generation != my_gen {
                    return Err(anyhow!("superseded by a newer load"));
                }
                if let Some(mut c) = g.child.take() {
                    let _ = c.kill().await;
                }
                g.state = "failed".into();
                g.error = Some("startup timed out after 180s".into());
                g.port = None;
                return Err(anyhow!("llama-server startup timed out"));
            }
        }
    }
}

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
    fn scan_pairs_iq_quant_model_with_mmproj() {
        // Regression: IQ-tagged quant suffixes must strip so name-based pairing works.
        let dir = std::env::temp_dir().join("bow_scan_test_iq");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Gemma-4-E4B-Aggressive-IQ4_XS.gguf"), b"x").unwrap();
        std::fs::write(dir.join("mmproj-Gemma-4-E4B-Aggressive-f16.gguf"), b"x").unwrap();
        std::fs::write(dir.join("Other-2B-Q8_0.gguf"), b"x").unwrap(); // defeats single-model fallback
        let models = scan_models(&dir);
        assert_eq!(models.len(), 2);
        let gemma = models.iter().find(|m| m.name.contains("Gemma")).unwrap();
        assert!(gemma.mmproj.is_some(), "IQ4_XS model must pair with its mmproj");
        let other = models.iter().find(|m| m.name.contains("Other")).unwrap();
        assert!(other.mmproj.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_pairs_two_families_to_their_own_mmproj() {
        let dir = std::env::temp_dir().join("bow_scan_test_families");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Alpha-7B-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(dir.join("mmproj-Alpha-7B-f16.gguf"), b"x").unwrap();
        std::fs::write(dir.join("Beta-2B-IQ2_M.gguf"), b"x").unwrap();
        std::fs::write(dir.join("mmproj-Beta-2B-f16.gguf"), b"x").unwrap();
        let models = scan_models(&dir);
        assert_eq!(models.len(), 2);
        let alpha = models.iter().find(|m| m.name.contains("Alpha")).unwrap();
        let beta = models.iter().find(|m| m.name.contains("Beta")).unwrap();
        let alpha_proj = alpha.mmproj.as_ref().expect("Alpha must have an mmproj");
        let beta_proj = beta.mmproj.as_ref().expect("Beta must have an mmproj");
        assert!(alpha_proj.to_string_lossy().contains("Alpha"), "Alpha paired with wrong mmproj");
        assert!(beta_proj.to_string_lossy().contains("Beta"), "Beta paired with wrong mmproj");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_pairs_hf_style_single_model_directory() {
        // HF-style layout: one model per directory, mmproj name need not match the model name.
        let dir = std::env::temp_dir().join("bow_scan_test_hf");
        std::fs::remove_dir_all(&dir).ok();
        let sub = dir.join("Qwen2.5-VL-7B-Instruct-GGUF");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(sub.join("mmproj-model-f16.gguf"), b"x").unwrap();
        let models = scan_models(&dir);
        assert_eq!(models.len(), 1);
        assert!(models[0].mmproj.is_some(), "sole model in dir must pair with sole mmproj");
        std::fs::remove_dir_all(&dir).ok();
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

    #[tokio::test]
    async fn failed_load_then_stop_reports_clean_stopped() {
        let eng = LlmEngine::new(std::env::temp_dir().join("no_bin_dir_seq"));
        let good = ModelEntry {
            path: PathBuf::from(r"C:\m\ok-Q4_K_M.gguf"), name: "ok-Q4_K_M".into(),
            size_bytes: 1, quant: Some("Q4_K_M".into()), mmproj: None,
        };
        // Two sequential loads, both failing on the missing llama-server.exe.
        let e1 = eng.load(good.clone(), 4096).await.unwrap_err().to_string();
        assert!(e1.contains("not found"), "e1 was: {}", e1);
        let e2 = eng.load(good, 4096).await.unwrap_err().to_string();
        assert!(e2.contains("not found"), "e2 was: {}", e2);
        // After failed loads, stop() must leave a clean "stopped" status.
        eng.stop().await;
        let st = eng.status().await;
        assert_eq!(st.state, "stopped");
        assert!(st.error.is_none(), "stale error: {:?}", st.error);
        assert!(st.model.is_none(), "stale model");
        assert!(st.base_url.is_none());
        assert!(!st.vision);
    }

    #[tokio::test]
    async fn spawn_failure_resets_state_to_failed() {
        // llama-server.exe exists but is not a valid PE — spawn() itself errors on
        // Windows, exercising the previously-unhandled early-return path.
        let dir = std::env::temp_dir().join("bow_engine_test_bad_exe");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("llama-server.exe"), b"not a real executable").unwrap();

        let eng = LlmEngine::new(dir.clone());
        let entry = ModelEntry {
            path: PathBuf::from(r"C:\m\ok-Q4_K_M.gguf"), name: "ok-Q4_K_M".into(),
            size_bytes: 1, quant: Some("Q4_K_M".into()), mmproj: None,
        };
        let err = eng.load(entry, 4096).await.unwrap_err().to_string();
        assert!(err.contains("spawn"), "err was: {}", err);

        let st = eng.status().await;
        assert_eq!(st.state, "failed");
        assert!(st.error.is_some(), "expected error to be set on failed spawn");
        assert!(st.base_url.is_none(), "port must be cleared on failed spawn");

        std::fs::remove_dir_all(&dir).ok();
    }

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
}
