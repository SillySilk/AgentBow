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
            // Check if what follows looks like a quant tag
            let after = &upper[i + 1..];
            if (after.starts_with('Q') || after.starts_with("IQ"))
                && after.len() > 1
                && after[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
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
            let mmproj = mmprojs.iter().find(|(mmproj_name, _)| {
                let mmproj_stem = mmproj_name.trim_end_matches(".gguf").strip_prefix("mmproj-").unwrap_or("");
                let mmproj_name_normalized = model_base_name(mmproj_stem);
                model_name == mmproj_name_normalized
            }).map(|(_, path)| path.clone());

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
