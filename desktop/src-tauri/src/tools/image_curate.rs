//! Image-set curation for training-data prep.
//!
//! Bow's main job for the user is gathering image sets (public figures, cartoon
//! characters) to train on. After `image_download` pulls candidates, these tools
//! clean the set up:
//!
//! - `image_dedupe`  — perceptual-hash (pHash) near-duplicate removal
//! - `image_stats`   — a read-only report on a folder (counts, formats, resolutions)
//! - `image_resize`  — non-destructive resize/convert into a separate output dir
//!
//! All heavy decode/hash work runs on a blocking thread so it never stalls the
//! async runtime.

use anyhow::{anyhow, Result};
use image::GenericImageView;
use image_hasher::{HashAlg, HasherConfig, ImageHash};
use std::path::{Path, PathBuf};

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"];

fn is_image_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Collect image file paths under `dir`. Recurses into subdirectories when
/// `recursive` is set (skipping our own `_bow_dupes` quarantine folder).
pub(crate) fn collect_images(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive && path.file_name().and_then(|n| n.to_str()) != Some("_bow_dupes") {
                collect_images(&path, recursive, out);
            }
        } else if is_image_path(&path) {
            out.push(path);
        }
    }
}

/// Max Hamming distance between two 64-bit pHashes for the images to count as the
/// same. Shared by `image_dedupe` and the download-time dedup in `image_search`.
pub(crate) const DEDUPE_DIST: u32 = 10;

/// Perceptual hash (Mean + DCT = classic pHash) of encoded image bytes. Returns
/// `None` when the bytes don't decode. Lets callers dedup downloaded images in-memory
/// without writing them to disk first.
pub(crate) fn phash_bytes(bytes: &[u8]) -> Option<ImageHash<Box<[u8]>>> {
    let img = image::load_from_memory(bytes).ok()?;
    let hasher = HasherConfig::new().hash_alg(HashAlg::Mean).preproc_dct().to_hasher();
    Some(hasher.hash_image(&img))
}

// ── Dedupe ──────────────────────────────────────────────────────────────────────

struct Hashed {
    path: PathBuf,
    hash: ImageHash<Box<[u8]>>,
    pixels: u64,
    bytes: u64,
}

/// Find perceptual near-duplicates in `dir` and (optionally) quarantine the
/// redundant copies, keeping the highest-resolution image of each group.
///
/// `threshold` is the max Hamming distance (0 = identical) between 64-bit pHashes
/// for two images to count as duplicates; 10 is a sensible default. When `apply`
/// is false (default) it only reports; when true it MOVES the extras into a
/// `_bow_dupes` subfolder (non-destructive — nothing is deleted).
pub async fn image_dedupe(
    dir: &str,
    threshold: u32,
    recursive: bool,
    apply: bool,
) -> Result<String> {
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || dedupe_blocking(&dir, threshold, recursive, apply))
        .await
        .map_err(|e| anyhow!("dedupe task panicked: {}", e))?
}

fn dedupe_blocking(dir: &str, threshold: u32, recursive: bool, apply: bool) -> Result<String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(anyhow!("image_dedupe: '{}' is not a directory", dir));
    }

    let mut paths = Vec::new();
    collect_images(root, recursive, &mut paths);
    if paths.is_empty() {
        return Ok(format!("No images found in {}", dir));
    }

    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::Mean)
        .preproc_dct() // Mean + DCT = the classic pHash
        .to_hasher();

    let mut items: Vec<Hashed> = Vec::with_capacity(paths.len());
    let mut unreadable = 0usize;
    for path in &paths {
        match image::open(path) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                items.push(Hashed {
                    path: path.clone(),
                    hash: hasher.hash_image(&img),
                    pixels: w as u64 * h as u64,
                    bytes,
                });
            }
            Err(_) => unreadable += 1,
        }
    }

    // Union-find clustering over all pairs within the distance threshold.
    let n = items.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if items[i].hash.dist(&items[j].hash) <= threshold {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Bucket indices by cluster root.
    let mut clusters: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        clusters.entry(r).or_default().push(i);
    }

    let mut dup_groups: Vec<Vec<usize>> = clusters.into_values().filter(|g| g.len() > 1).collect();
    // Stable, readable ordering: largest groups first.
    dup_groups.sort_by_key(|g| std::cmp::Reverse(g.len()));

    let total_dupes: usize = dup_groups.iter().map(|g| g.len() - 1).sum();

    let mut report = String::new();
    report.push_str(&format!(
        "Scanned {} image(s) in {}{}.\n",
        items.len(),
        dir,
        if recursive { " (recursive)" } else { "" }
    ));
    if unreadable > 0 {
        report.push_str(&format!("Skipped {} unreadable/corrupt file(s).\n", unreadable));
    }
    report.push_str(&format!(
        "Found {} near-duplicate group(s) covering {} redundant file(s) (threshold {}).\n",
        dup_groups.len(),
        total_dupes,
        threshold
    ));

    if dup_groups.is_empty() {
        return Ok(report);
    }

    // Pick keeper per group (highest resolution, then largest file), gather removals.
    let mut removals: Vec<usize> = Vec::new();
    let mut group_lines: Vec<String> = Vec::new();
    for group in &dup_groups {
        let keeper = *group
            .iter()
            .max_by(|&&a, &&b| {
                items[a]
                    .pixels
                    .cmp(&items[b].pixels)
                    .then(items[a].bytes.cmp(&items[b].bytes))
            })
            .unwrap();
        let mut line = format!("• keep {}", file_name(&items[keeper].path));
        for &idx in group {
            if idx != keeper {
                removals.push(idx);
                line.push_str(&format!("\n    dup  {}", file_name(&items[idx].path)));
            }
        }
        group_lines.push(line);
    }

    // Cap the per-group detail so the report stays readable.
    let shown = group_lines.len().min(25);
    report.push('\n');
    report.push_str(&group_lines[..shown].join("\n"));
    if group_lines.len() > shown {
        report.push_str(&format!("\n… and {} more group(s).", group_lines.len() - shown));
    }

    if apply {
        let quarantine = root.join("_bow_dupes");
        std::fs::create_dir_all(&quarantine)
            .map_err(|e| anyhow!("could not create quarantine dir: {}", e))?;
        let mut moved = 0usize;
        let mut errors = 0usize;
        for &idx in &removals {
            let src = &items[idx].path;
            let dest = unique_dest(&quarantine, src);
            match std::fs::rename(src, &dest) {
                Ok(_) => moved += 1,
                Err(_) => {
                    // rename can fail across volumes — fall back to copy + delete.
                    match std::fs::copy(src, &dest).and_then(|_| std::fs::remove_file(src)) {
                        Ok(_) => moved += 1,
                        Err(_) => errors += 1,
                    }
                }
            }
        }
        report.push_str(&format!(
            "\n\nApplied: moved {} duplicate(s) to {}{}.",
            moved,
            quarantine.display(),
            if errors > 0 { format!(" ({} failed)", errors) } else { String::new() }
        ));
    } else {
        report.push_str("\n\nDry run — nothing moved. Call again with apply=true to quarantine the duplicates into a _bow_dupes subfolder.");
    }

    Ok(report)
}

// ── Stats ─────────────────────────────────────────────────────────────────────

/// Read-only report on an image folder: count, format histogram, resolution
/// range, corrupt files, total size. Useful before/after building a training set.
pub async fn image_stats(dir: &str, recursive: bool) -> Result<String> {
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || stats_blocking(&dir, recursive))
        .await
        .map_err(|e| anyhow!("stats task panicked: {}", e))?
}

fn stats_blocking(dir: &str, recursive: bool) -> Result<String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(anyhow!("image_stats: '{}' is not a directory", dir));
    }

    let mut paths = Vec::new();
    collect_images(root, recursive, &mut paths);
    if paths.is_empty() {
        return Ok(format!("No images found in {}", dir));
    }

    let mut formats: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut widths: Vec<u32> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut corrupt = 0usize;
    let mut min_dim = (u32::MAX, u32::MAX);
    let mut max_dim = (0u32, 0u32);

    for path in &paths {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("?")
            .to_lowercase();
        *formats.entry(ext).or_insert(0) += 1;
        total_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        match image::open(path) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                widths.push(w.min(h)); // track smallest side for "min usable resolution"
                if (w as u64 * h as u64) < (min_dim.0 as u64 * min_dim.1 as u64) {
                    min_dim = (w, h);
                }
                if (w as u64 * h as u64) > (max_dim.0 as u64 * max_dim.1 as u64) {
                    max_dim = (w, h);
                }
            }
            Err(_) => corrupt += 1,
        }
    }

    let mut report = String::new();
    report.push_str(&format!(
        "Image stats for {}{}:\n",
        dir,
        if recursive { " (recursive)" } else { "" }
    ));
    report.push_str(&format!("  Files: {}\n", paths.len()));
    report.push_str(&format!("  Total size: {:.1} MB\n", total_bytes as f64 / 1_048_576.0));

    let mut fmt_pairs: Vec<(&String, &usize)> = formats.iter().collect();
    fmt_pairs.sort_by(|a, b| b.1.cmp(a.1));
    let fmt_str: Vec<String> = fmt_pairs.iter().map(|(k, v)| format!("{}×{}", v, k)).collect();
    report.push_str(&format!("  Formats: {}\n", fmt_str.join(", ")));

    if corrupt > 0 {
        report.push_str(&format!("  Corrupt/unreadable: {}\n", corrupt));
    }
    if !widths.is_empty() {
        widths.sort_unstable();
        let median_min_side = widths[widths.len() / 2];
        report.push_str(&format!(
            "  Resolution: smallest {}×{}, largest {}×{}, median shortest-side {}px\n",
            min_dim.0, min_dim.1, max_dim.0, max_dim.1, median_min_side
        ));
    }

    Ok(report)
}

// ── Resize / convert ─────────────────────────────────────────────────────────────

/// Resize and/or convert every image in `src_dir` into `dest_dir`
/// (non-destructive — originals are never touched). Only downscales: images
/// already within `max_dim` keep their size. `format` is "jpeg", "png", or "webp".
pub async fn image_resize(
    src_dir: &str,
    dest_dir: &str,
    max_dim: u32,
    format: &str,
    recursive: bool,
) -> Result<String> {
    let (src, dest, fmt) = (src_dir.to_string(), dest_dir.to_string(), format.to_lowercase());
    tokio::task::spawn_blocking(move || resize_blocking(&src, &dest, max_dim, &fmt, recursive))
        .await
        .map_err(|e| anyhow!("resize task panicked: {}", e))?
}

fn resize_blocking(
    src_dir: &str,
    dest_dir: &str,
    max_dim: u32,
    format: &str,
    recursive: bool,
) -> Result<String> {
    let src_root = Path::new(src_dir);
    if !src_root.is_dir() {
        return Err(anyhow!("image_resize: src '{}' is not a directory", src_dir));
    }
    let ext = match format {
        "jpg" | "jpeg" => "jpg",
        "png" => "png",
        "webp" => "webp",
        other => return Err(anyhow!("image_resize: unsupported format '{}' (use jpeg/png/webp)", other)),
    };
    if max_dim < 16 {
        return Err(anyhow!("image_resize: max_dim {} is too small", max_dim));
    }

    std::fs::create_dir_all(dest_dir)
        .map_err(|e| anyhow!("could not create dest dir: {}", e))?;

    let mut paths = Vec::new();
    collect_images(src_root, recursive, &mut paths);
    if paths.is_empty() {
        return Ok(format!("No images found in {}", src_dir));
    }

    let mut written = 0usize;
    let mut skipped = 0usize;
    for path in &paths {
        let Ok(img) = image::open(path) else { skipped += 1; continue };
        let (w, h) = img.dimensions();
        let resized = if w.max(h) > max_dim {
            img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };

        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let dest = unique_named(Path::new(dest_dir), stem, ext);

        // JPEG/WebP have no alpha channel — flatten to RGB to avoid encode errors.
        let save_result = if ext == "jpg" {
            image::DynamicImage::ImageRgb8(resized.to_rgb8()).save(&dest)
        } else {
            resized.save(&dest)
        };
        match save_result {
            Ok(_) => written += 1,
            Err(_) => skipped += 1,
        }
    }

    Ok(format!(
        "Resized {} image(s) → {} (longest side ≤ {}px, {} format){}.",
        written,
        dest_dir,
        max_dim,
        ext,
        if skipped > 0 { format!(", skipped {}", skipped) } else { String::new() }
    ))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string()
}

/// A destination path inside `dir` for `src`'s file name, suffixing a counter if
/// a file with that name already exists.
fn unique_dest(dir: &Path, src: &Path) -> PathBuf {
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("");
    unique_named(dir, stem, ext)
}

fn unique_named(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    for i in 0.. {
        let name = if i == 0 {
            format!("{}.{}", stem, ext)
        } else {
            format!("{}_{}.{}", stem, i, ext)
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_solid(path: &Path, w: u32, h: u32, color: [u8; 3]) {
        let img = RgbImage::from_pixel(w, h, Rgb(color));
        img.save(path).unwrap();
    }

    /// Write an image whose pixels are a function of (x, y), so it has real
    /// luminance *structure* — pHash compares structure, not flat color.
    fn write_pattern(path: &Path, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) {
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, Rgb(f(x, y)));
            }
        }
        img.save(path).unwrap();
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bow_curate_test_{}_{}", tag, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn dedupe_groups_and_quarantines() {
        let dir = tmp_dir("dedupe");
        // A structured base image (diagonal gradient) and a TRUE downscale of it
        // (perceptually identical → duplicates). Bigger one is the deterministic keeper.
        let diag = |x: u32, y: u32| { let v = ((x + y) % 256) as u8; [v, v, v] };
        write_pattern(&dir.join("grad_big.png"), 256, 256, diag);
        image::open(dir.join("grad_big.png"))
            .unwrap()
            .resize(128, 128, image::imageops::FilterType::Lanczos3)
            .save(dir.join("grad_small.png"))
            .unwrap();
        // Coarse checkerboard (64px cells) — strong mid-frequency structure that
        // survives the 8x8 hash downscale, so its pHash is far from a smooth gradient.
        write_pattern(&dir.join("checker.png"), 256, 256, |x, y| {
            if (x / 64 + y / 64) % 2 == 0 { [0, 0, 0] } else { [255, 255, 255] }
        });

        let report = image_dedupe(dir.to_str().unwrap(), 10, false, true).await.unwrap();
        assert!(report.contains("1 near-duplicate group"), "report: {report}");
        assert!(dir.join("grad_big.png").exists(), "keeper should remain");
        assert!(!dir.join("grad_small.png").exists(), "dup should be moved");
        assert!(dir.join("_bow_dupes").join("grad_small.png").exists(), "dup should be in quarantine");
        assert!(dir.join("checker.png").exists(), "distinct image should remain");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stats_reports_counts_and_formats() {
        let dir = tmp_dir("stats");
        write_solid(&dir.join("a.png"), 64, 48, [1, 2, 3]);
        write_solid(&dir.join("b.png"), 128, 256, [4, 5, 6]);

        let report = image_stats(dir.to_str().unwrap(), false).await.unwrap();
        assert!(report.contains("Files: 2"), "report: {report}");
        assert!(report.contains("png"), "report: {report}");
        assert!(report.contains("largest 128×256"), "report: {report}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn resize_only_downscales_into_dest() {
        let src = tmp_dir("resize_src");
        let dest = tmp_dir("resize_dest");
        write_solid(&src.join("big.png"), 1000, 500, [9, 9, 9]);
        write_solid(&src.join("small.png"), 50, 50, [9, 9, 9]);

        let report = image_resize(src.to_str().unwrap(), dest.to_str().unwrap(), 256, "png", false)
            .await
            .unwrap();
        assert!(report.contains("Resized 2"), "report: {report}");

        let big = image::open(dest.join("big.png")).unwrap();
        assert_eq!(big.dimensions(), (256, 128), "longest side should be capped");
        let small = image::open(dest.join("small.png")).unwrap();
        assert_eq!(small.dimensions(), (50, 50), "small image should be untouched");
        // Originals untouched.
        assert_eq!(image::open(src.join("big.png")).unwrap().dimensions(), (1000, 500));

        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dest).ok();
    }
}
