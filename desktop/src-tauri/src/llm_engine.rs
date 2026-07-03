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
