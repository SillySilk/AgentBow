use anyhow::Result;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;
use std::io::Write as IoWrite;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info};

// ── LM Studio types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LmStudioResponse {
    choices: Vec<LmStudioChoice>,
}
#[derive(Deserialize)]
struct LmStudioChoice {
    message: LmStudioMessage,
}
#[derive(Deserialize)]
struct LmStudioMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
}

// ── Scraper result type ───────────────────────────────────────────────────────

struct ScrapeResult {
    source: &'static str,
    urls: Vec<String>,
    /// Set when the HTTP call succeeded but 0 URLs were extracted — includes
    /// a snippet of the raw response so the log shows what the site returned.
    debug_hint: Option<String>,
    /// Set when the HTTP call itself failed.
    error: Option<String>,
}

impl ScrapeResult {
    fn ok(source: &'static str, urls: Vec<String>, raw: &str) -> Self {
        let hint = if urls.is_empty() {
            // Skip HTML <head> (first ~800 chars) and grab body content for useful debugging
            let snippet: String = raw.chars().skip(800).take(1200).collect();
            Some(snippet.replace('\n', " "))
        } else {
            None
        };
        Self { source, urls, debug_hint: hint, error: None }
    }
    fn err(source: &'static str, e: String) -> Self {
        Self { source, urls: vec![], debug_hint: None, error: Some(e) }
    }
    fn log_line(&self) -> String {
        if let Some(e) = &self.error {
            format!("  {:8} ERROR: {}", self.source, e)
        } else if self.urls.is_empty() {
            let hint = self.debug_hint.as_deref().unwrap_or("(no response)");
            format!("  {:8} 0 URLs — hint: {:.500}", self.source, hint)
        } else {
            format!("  {:8} {} URLs", self.source, self.urls.len())
        }
    }
}

// ── ScrapeEvent ───────────────────────────────────────────────────────────────

use tokio::sync::mpsc::UnboundedSender;

/// Progress events emitted during a streamed `image_download`.
#[derive(Debug, Clone)]
pub enum ScrapeEvent {
    Phase { label: String },
    Source { source: String, count: usize, error: Option<String> },
    Candidates { total: usize, filtered: usize },
    Verifying { url: String, done: usize, target: usize },
    Downloaded { done: usize, target: usize, path: String },
    Failed { url: String, reason: String },
    Done { downloaded: Vec<String>, log_note: String, dest_dir: String },
}

impl ScrapeEvent {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ScrapeEvent::Phase { label } => json!({ "kind": "phase", "label": label }),
            ScrapeEvent::Source { source, count, error } =>
                json!({ "kind": "source", "source": source, "count": count, "error": error }),
            ScrapeEvent::Candidates { total, filtered } =>
                json!({ "kind": "candidates", "total": total, "filtered": filtered }),
            ScrapeEvent::Verifying { url, done, target } =>
                json!({ "kind": "verifying", "url": url, "done": done, "target": target }),
            ScrapeEvent::Downloaded { done, target, path } =>
                json!({ "kind": "downloaded", "done": done, "target": target, "path": path }),
            ScrapeEvent::Failed { url, reason } =>
                json!({ "kind": "failed", "url": url, "reason": reason }),
            ScrapeEvent::Done { downloaded, log_note, dest_dir } =>
                json!({ "kind": "done", "downloaded": downloaded, "log_note": log_note, "dest_dir": dest_dir }),
        }
    }
}

// ── Session log ───────────────────────────────────────────────────────────────

pub struct SessionLog {
    path: String,
    file: Option<std::fs::File>,
}

impl SessionLog {
    pub fn new(log_dir: &str, query: &str) -> Self {
        // Ensure logs directory exists; if it fails we'll surface it in flush()
        let _ = std::fs::create_dir_all(log_dir);
        let path = format!("{}\\bow_downloads.log",
            log_dir.trim_end_matches(['\\', '/']));
        let file = std::fs::OpenOptions::new()
            .create(true).append(true).open(&path).ok();
        let mut log = Self { path, file };
        log.push(format!("=== bow image_download [ts:{}] ===", unix_ts()));
        log.push(format!("query: {:?}", query));
        log
    }
    /// Append a line to the log immediately (write-through). A run that hangs or is
    /// interrupted (e.g. the window is closed) still leaves a diagnostic trail —
    /// std::fs::File is unbuffered, so each line hits the OS without an explicit flush.
    fn push(&mut self, line: String) {
        info!("{}", line);
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{}", line);
        }
    }
    /// Lines are already on disk (write-through); this just returns a path note for
    /// the UI/result, plus a trailing separator between sessions.
    pub fn flush(&self) -> String {
        match &self.file {
            None => format!("(log unavailable — could not open {})", self.path),
            Some(f) => {
                // &File implements Write; rebind as a mutable place for writeln!.
                let mut fh = f;
                let _ = writeln!(fh);
                format!("Log: {}", self.path)
            }
        }
    }
}

fn unix_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── image_verify ──────────────────────────────────────────────────────────────

/// Vision models reject very large payloads; skip verification past this size.
const MAX_VERIFY_BYTES: usize = 4 * 1024 * 1024;

pub async fn image_verify(
    image_path: &str,
    prompt: &str,
    lm_studio_url: &str,
    model: &str,
) -> Result<String> {
    let path = Path::new(image_path);
    if !path.exists() {
        return Err(anyhow::anyhow!("Image file not found: {}", image_path));
    }

    let mut image_bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Failed to read image '{}': {}", image_path, e))?;

    let mut ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    // Most local vision models reject WebP. Transcode it to PNG in-memory so the
    // image can still be verified instead of being skipped.
    if ext == "webp" {
        if let Ok(img) = image::load_from_memory(&image_bytes) {
            let mut buf = Vec::new();
            if img
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .is_ok()
            {
                image_bytes = buf;
                ext = "png".to_string();
            }
        }
    }

    if image_bytes.len() > MAX_VERIFY_BYTES {
        return Ok(format!(
            "[image_verify skipped: {} is {:.1} MB — may exceed vision model context window. \
             File exists and appears valid.]",
            image_path,
            image_bytes.len() as f64 / 1024.0 / 1024.0
        ));
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
    let mime = match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        _ => "image/jpeg",
    };
    let data_uri = format!("data:{};base64,{}", mime, b64);

    call_vision_model(&data_uri, prompt, lm_studio_url, model, 300).await
}

/// POST a vision request (text prompt + one image as a data URI) to LM Studio's
/// OpenAI-compatible endpoint and return the model's text. Falls back to the
/// reasoning channel if a reasoning model returned no plain content.
async fn call_vision_model(
    data_uri: &str,
    prompt: &str,
    lm_studio_url: &str,
    model: &str,
    max_tokens: u32,
) -> Result<String> {
    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": [
            { "type": "text", "text": prompt },
            { "type": "image_url", "image_url": { "url": data_uri } }
        ]}],
        "max_tokens": max_tokens
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/completions", lm_studio_url))
        .json(&body).send().await
        .map_err(|e| anyhow::anyhow!("LM Studio request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("LM Studio error {}: {}", status, body));
    }

    let data: LmStudioResponse = resp.json().await
        .map_err(|e| anyhow::anyhow!("Failed to parse LM Studio response: {}", e))?;
    let choice = data.choices.first()
        .ok_or_else(|| anyhow::anyhow!("LM Studio returned no choices"))?;
    let result = choice.message.content.as_deref().unwrap_or("");
    let reasoning = choice.message.reasoning_content.as_deref().unwrap_or("");
    if result.is_empty() && !reasoning.is_empty() { Ok(reasoning.to_string()) }
    else if !result.is_empty() { Ok(result.to_string()) }
    else { Ok("(no response from vision model)".to_string()) }
}

// ── image_autotag ──────────────────────────────────────────────────────────────

/// Caption every image in a folder for LoRA/SD training, writing a `<name>.txt`
/// sidecar next to each one (the kohya caption convention).
///
/// `style` is "tags" (comma-separated booru-style tags) or "caption" (one
/// sentence). `trigger`, if non-empty, is prepended to every line — the standard
/// way to bind a concept to a token. Images are downscaled before tagging (full
/// resolution isn't needed and it speeds up inference). Existing `.txt` files are
/// skipped unless `overwrite` is set.
pub async fn image_autotag(
    dir: &str,
    style: &str,
    trigger: &str,
    recursive: bool,
    overwrite: bool,
    lm_studio_url: &str,
    model: &str,
) -> Result<String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(anyhow::anyhow!("image_autotag: '{}' is not a directory", dir));
    }

    let tags_mode = !style.eq_ignore_ascii_case("caption");
    let prompt = if tags_mode {
        "List 15-25 short descriptive tags for this image as a single comma-separated line. \
         Cover the subject, appearance, hair, clothing, expression, pose, background, and art style. \
         Output ONLY the comma-separated tags — no preamble, no numbering, no sentences."
    } else {
        "Write one concise descriptive sentence captioning this image (subject, appearance, setting). \
         Output only the caption, no preamble."
    };

    let mut paths = Vec::new();
    crate::tools::image_curate::collect_images(root, recursive, &mut paths);
    if paths.is_empty() {
        return Ok(format!("No images found in {}", dir));
    }

    let mut tagged = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut sample: Option<String> = None;

    for path in &paths {
        let sidecar = path.with_extension("txt");
        if sidecar.exists() && !overwrite {
            skipped += 1;
            continue;
        }

        let data_uri = match load_resize_data_uri(path, 1024) {
            Ok(u) => u,
            Err(_) => { failed += 1; continue; }
        };

        // Per-image timeout so one slow/hung inference can't stall the batch.
        let call = call_vision_model(&data_uri, prompt, lm_studio_url, model, 200);
        let raw = match tokio::time::timeout(std::time::Duration::from_secs(120), call).await {
            Ok(Ok(text)) => text,
            _ => { failed += 1; continue; }
        };

        let line = clean_caption(&raw, trigger, tags_mode);
        if line.is_empty() || raw.starts_with("(no response") {
            failed += 1;
            continue;
        }

        if std::fs::write(&sidecar, &line).is_ok() {
            tagged += 1;
            if sample.is_none() {
                sample = Some(format!("{} → {}", file_stem(path), line));
            }
        } else {
            failed += 1;
        }
    }

    let mut report = format!(
        "Auto-tagged {} image(s) in {}{} ({} style).\n  written: {}  skipped (existing): {}  failed: {}",
        paths.len(), dir, if recursive { " (recursive)" } else { "" },
        if tags_mode { "tags" } else { "caption" },
        tagged, skipped, failed
    );
    if let Some(s) = sample {
        report.push_str(&format!("\n  e.g. {}", s));
    }
    Ok(report)
}

/// Load an image, downscale so its longest side is at most `max_dim`, and return
/// a PNG data URI suitable for an OpenAI-style image_url field.
fn load_resize_data_uri(path: &Path, max_dim: u32) -> Result<String> {
    let img = image::open(path).map_err(|e| anyhow::anyhow!("decode failed: {}", e))?;
    let (w, h) = {
        use image::GenericImageView;
        img.dimensions()
    };
    let img = if w.max(h) > max_dim {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("encode failed: {}", e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/png;base64,{}", b64))
}

/// Normalise raw model output into a single caption line and prepend the trigger.
fn clean_caption(raw: &str, trigger: &str, tags_mode: bool) -> String {
    let mut text = raw.trim().to_string();

    // Drop a common leading label like "Tags:" / "Caption:".
    for prefix in ["tags:", "caption:", "answer:"] {
        if text.to_lowercase().starts_with(prefix) {
            text = text[prefix.len()..].trim().to_string();
        }
    }
    // Strip surrounding quotes/backticks.
    text = text.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();

    let body = if tags_mode {
        // Flatten any newlines/bullets into comma-separated tags, then tidy.
        let joined = text
            .lines()
            .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        joined
            .split(',')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        // Caption: collapse to a single line.
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    let trigger = trigger.trim();
    if trigger.is_empty() {
        body
    } else if body.is_empty() {
        trigger.to_string()
    } else {
        format!("{}, {}", trigger, body)
    }
}

fn file_stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string()
}

// ── source_enabled ────────────────────────────────────────────────────────────

/// Returns true when the given source key should run.
/// `None` or an empty list means "run all". Matching is case-insensitive.
fn source_enabled(sources: &Option<Vec<String>>, key: &str) -> bool {
    match sources {
        None => true,
        Some(list) if list.is_empty() => true,
        Some(list) => list.iter().any(|s| s.eq_ignore_ascii_case(key)),
    }
}

// ── Pacing + vision-QA ──────────────────────────────────────────────────────────

/// Per-run knobs threaded from the WS request: download pacing + the vision gate.
#[derive(Clone, Default)]
pub struct ScrapeTuning {
    /// Delay between downloads, in milliseconds. 0 + no verify ⇒ fast concurrent path.
    pub delay_ms: u64,
    /// Run the vision-QA inline keep/discard gate.
    pub verify: bool,
    /// Override the default judging prompt (empty/None ⇒ default).
    pub vision_prompt: Option<String>,
    pub lm_studio_url: String,
    /// Manual vision-model override. Empty ⇒ auto-detect the loaded VLM from LM Studio.
    pub vision_model_override: String,
    /// Chat model id — used as a last-resort fallback if auto-detect fails.
    pub chat_model: String,
    /// Skip images that perceptually match an image already in the bin or kept earlier
    /// this run. Forces the sequential download path so hashing stays ordered.
    pub dedupe: bool,
    /// Which scrapers to run. `None`/empty ⇒ all. Canonical keys: `bing`, `ddg`,
    /// `yandex`, `brave`.
    pub sources: Option<Vec<String>>,
}

/// Choose the vision model id from LM Studio's `/api/v0/models` JSON. Prefers a
/// **loaded** model of `type == "vlm"`; if the loaded model isn't a VLM it's used
/// anyway with a warning; if nothing is loaded it falls back to `fallback`.
fn pick_loaded_vision_model(models_json: &serde_json::Value, fallback: &str) -> (String, Option<String>) {
    let models = match models_json["data"].as_array() {
        Some(m) => m,
        None => return (fallback.to_string(), Some("LM Studio returned no model list — using fallback".to_string())),
    };
    let loaded: Vec<&serde_json::Value> =
        models.iter().filter(|m| m["state"].as_str() == Some("loaded")).collect();
    if let Some(m) = loaded.iter().find(|m| m["type"].as_str() == Some("vlm")) {
        return (m["id"].as_str().unwrap_or(fallback).to_string(), None);
    }
    if let Some(m) = loaded.first() {
        let id = m["id"].as_str().unwrap_or(fallback).to_string();
        let ty = m["type"].as_str().unwrap_or("unknown");
        return (id.clone(), Some(format!(
            "loaded model '{}' is type '{}', not a vision model — image checks may be unreliable. Load a vision (VLM) model in LM Studio.",
            id, ty
        )));
    }
    (fallback.to_string(), Some("no model loaded in LM Studio — using fallback; load a vision model".to_string()))
}

/// Resolve which model to use for the vision gate. A non-empty `override_id` wins;
/// otherwise auto-detect the loaded VLM from LM Studio's native API. Returns the
/// chosen id and an optional warning to surface to the user.
async fn resolve_vision_model(lm_studio_url: &str, override_id: &str, fallback: &str) -> (String, Option<String>) {
    if !override_id.trim().is_empty() {
        return (override_id.to_string(), None);
    }
    let url = format!("{}/api/v0/models", lm_studio_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    match client.get(&url).timeout(std::time::Duration::from_secs(8)).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(data) => pick_loaded_vision_model(&data, fallback),
            Err(e) => (fallback.to_string(), Some(format!("could not read LM Studio models ({}) — using fallback", e))),
        },
        Ok(resp) => (fallback.to_string(), Some(format!("LM Studio /api/v0/models HTTP {} — using fallback", resp.status()))),
        Err(e) => (fallback.to_string(), Some(format!("LM Studio not reachable for model auto-detect ({}) — using fallback", e))),
    }
}

/// Resolved vision-gate settings (prompt already query-interpolated).
#[derive(Clone)]
pub struct VerifyConfig {
    pub prompt: String,
    pub lm_studio_url: String,
    pub vision_model: String,
}

/// Default judging prompt for the vision gate, with the query interpolated in.
fn default_verify_prompt(query: &str) -> String {
    format!(
        "You are curating image-search results for the query: \"{query}\".\n\
         Judge this single image on three things:\n\
         1. Relevance — does it clearly depict \"{query}\"? Reject off-topic images, the wrong \
         subject, or text/memes about the topic rather than the thing itself.\n\
         2. Technical quality — reject blurry, low-resolution, heavily compressed images and \
         upscaled thumbnails.\n\
         3. Cleanliness — reject watermarks, logos, collages or grids of multiple images, \
         screenshots with UI chrome, and heavy text overlays.\n\
         Respond with ONLY a one-line JSON object: {{\"keep\": true or false, \"reason\": \"short reason\"}}."
    )
}

/// Parse a vision reply into (keep, reason). Lenient: grabs the first `{…}` object.
/// On any parse failure, defaults to keep=true with a flag so parser issues never
/// silently discard valid images.
fn parse_verdict(reply: &str) -> (bool, String) {
    if let (Some(s), Some(e)) = (reply.find('{'), reply.rfind('}')) {
        if e > s {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply[s..=e]) {
                let keep = v["keep"].as_bool().unwrap_or(true);
                let reason = v["reason"].as_str().unwrap_or("").to_string();
                return (keep, reason);
            }
        }
    }
    (true, format!("unparsed verdict (kept): {:.80}", reply.replace('\n', " ")))
}

/// Judge already-downloaded image bytes with the vision model. Mirrors
/// `image_verify`'s WebP→PNG transcode + size cap. Never panics; on model/transcode
/// failure it keeps the image (flagged) rather than losing it.
async fn vision_judge(bytes: &[u8], ext: &str, cfg: &VerifyConfig) -> (bool, String) {
    let mut data = bytes.to_vec();
    let mut ext = ext.to_string();
    if ext == "webp" {
        if let Ok(img) = image::load_from_memory(&data) {
            let mut buf = Vec::new();
            if img
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .is_ok()
            {
                data = buf;
                ext = "png".to_string();
            }
        }
    }
    if data.len() > MAX_VERIFY_BYTES {
        return (true, "too large to verify — kept".to_string());
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    let mime = match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        _ => "image/jpeg",
    };
    let data_uri = format!("data:{};base64,{}", mime, b64);
    match call_vision_model(&data_uri, &cfg.prompt, &cfg.lm_studio_url, &cfg.vision_model, 200).await {
        Ok(reply) => {
            debug!("vision verdict: {:.120}", reply.replace('\n', " "));
            parse_verdict(&reply)
        }
        Err(e) => (true, format!("vision error — kept: {}", e)),
    }
}

// ── image_download ────────────────────────────────────────────────────────────

/// Download images matching `query` into `dest_dir`, up to `count` files.
/// Writes a session log to `{log_dir}\\bow_downloads.log`.
/// Source selection lives in `tuning.sources` (`None`/empty → run all scrapers).
pub async fn image_download(
    query: &str,
    count: usize,
    dest_dir: &str,
    log_dir: &str,
    tuning: ScrapeTuning,
    browser: &crate::tools::controlled_browser::ControlledBrowser,
    progress: Option<UnboundedSender<ScrapeEvent>>,
) -> Result<String> {
    let sources = tuning.sources.clone();
    let emit = |e: ScrapeEvent| { if let Some(tx) = &progress { let _ = tx.send(e); } };

    std::fs::create_dir_all(dest_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create dest_dir '{}': {}", dest_dir, e))?;

    let mut log = SessionLog::new(log_dir, query);
    log.push(format!("dest_dir: {}", dest_dir));

    let client = reqwest::Client::builder()
        .cookie_store(true)  // DDG requires session cookies from page request to carry into i.js API
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

    // A single results page yields ~50 images, so over-fetch per page and paginate
    // until we have `count` *successful* downloads (or the enabled sources run out).
    const MAX_SCRAPE_PAGES: usize = 12;
    let per_page_max = (count * 4).max(50);
    let encoded = urlencoding::encode(query);

    // (key, display, page-url builder, parser, pre-nav cookies). Yandex first: its
    // safe-search-off is confirmed working, so leading with it puts uncensored
    // candidates at the front of the download queue. DDG (HTTP) runs last.
    type EngineRow = (
        &'static str,
        &'static str,
        fn(&str, usize) -> String,
        fn(&str, usize) -> Vec<String>,
        &'static [(&'static str, &'static str, &'static str)],
    );
    let browser_engines: &[EngineRow] = &[
        ("yandex", "Yandex", yandex_page_url, parse_yandex,
         &[("safesearch", "0", ".yandex.com"), ("yp", "1999999999.sp.ssp%3D0", ".yandex.com")]),
        ("bing", "Bing", bing_page_url, parse_bing,
         &[("SRCHHPGUSR", "SRCHLANG=en&ADLT=OFF&NNT=10&NRSLT=50", ".bing.com"), ("adlt", "off", ".bing.com")]),
        ("brave", "Brave", brave_page_url, parse_brave,
         &[("safesearch", "off", ".search.brave.com")]),
    ];

    // Resolve the vision gate once (independent of paging).
    let verify_cfg = if tuning.verify {
        let (model, warn) = resolve_vision_model(
            &tuning.lm_studio_url, &tuning.vision_model_override, &tuning.chat_model,
        ).await;
        if let Some(w) = &warn {
            log.push(format!("Vision-QA WARNING: {}", w));
            emit(ScrapeEvent::Phase { label: format!("Vision-QA: {}", w) });
        }
        let prompt = tuning.vision_prompt.clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| default_verify_prompt(query));
        log.push(format!("Vision-QA gate ON (model: {})", model));
        Some(VerifyConfig {
            prompt,
            lm_studio_url: tuning.lm_studio_url.clone(),
            vision_model: model,
        })
    } else {
        None
    };

    // Dedicated download client (separate from the scraping `client` above).
    let dl_client = Arc::new(
        reqwest::Client::builder()
            .cookie_store(true)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .redirect(safe_redirect_policy())
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?
    );

    let sanitized = sanitize_filename(query);
    let dest_base = dest_dir.trim_end_matches(['\\', '/']).to_string();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut downloaded: Vec<String> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    // Continue numbering after any files already in the bin (resume/append) so an
    // existing set is never overwritten.
    let mut seq = highest_existing_index(&dest_base, &sanitized);
    // Seed the content-dedup set with the bin's existing images so a resumed scrape
    // skips ones it already holds (even under different filenames).
    let mut seen_hashes: Vec<Phash> = if tuning.dedupe {
        let h = hash_dir_images(&dest_base).await;
        log.push(format!("Dedupe ON — hashed {} existing image(s) in bin", h.len()));
        h
    } else {
        Vec::new()
    };

    log.push("-- Scraping + downloading (paginated until target met) --".to_string());
    emit(ScrapeEvent::Phase { label: "Scraping sources".into() });

    for page in 0..MAX_SCRAPE_PAGES {
        if downloaded.len() >= count {
            break;
        }

        // Scrape this page from every enabled source.
        let mut results: Vec<ScrapeResult> = Vec::new();
        let fetch = BrowserFetch { browser, log_dir, progress: &progress };
        for (key, name, page_url, parse, cookies) in browser_engines {
            if source_enabled(&sources, key) {
                let url = page_url(&encoded, page);
                results.push(
                    scrape_via_browser(&fetch, name, &url, per_page_max, *parse, cookies).await,
                );
            }
        }
        if source_enabled(&sources, "ddg") {
            results.push(scrape_duckduckgo_images(&client, query, per_page_max, page).await);
        }

        // Gather raw URLs, decode/paywall-filter/dedupe within the page, then keep only
        // candidates not already seen on an earlier page.
        let mut raw: Vec<String> = Vec::new();
        for r in &results {
            log.push(format!("p{} {}", page, r.log_line()));
            emit(ScrapeEvent::Source { source: r.source.to_string(), count: r.urls.len(), error: r.error.clone() });
            raw.extend(r.urls.iter().cloned());
        }
        let raw_n = raw.len();
        let decoded = filter_candidates(raw);
        let filtered = raw_n.saturating_sub(decoded.len());
        let new_candidates: Vec<String> =
            decoded.into_iter().filter(|u| seen.insert(u.clone())).collect();

        emit(ScrapeEvent::Candidates { total: seen.len(), filtered });
        log.push(format!(
            "p{}: +{} new candidates (pool {}), downloaded {}/{}",
            page, new_candidates.len(), seen.len(), downloaded.len(), count
        ));

        // No new candidates this round → the enabled sources are exhausted.
        if new_candidates.is_empty() {
            break;
        }

        download_batch(
            &dl_client, new_candidates, count, &dest_base, &sanitized,
            tuning.delay_ms, &verify_cfg, tuning.dedupe, &mut seen_hashes,
            &mut downloaded, &mut failures, &mut seq, &progress,
        ).await;
    }

    downloaded.sort();
    log_download_summary(&mut log, &downloaded, &failures, count, verify_cfg.is_some());

    let log_note = log.flush();

    if seen.is_empty() {
        return Err(anyhow::anyhow!("No images found for {:?}. {}", query, log_note));
    }

    // Nothing new saved. Distinguish "everything was already in the bin" (a normal
    // resume outcome) from a genuine total failure so the UI doesn't show a scary error.
    if downloaded.is_empty() {
        let dup_skips = failures.iter().filter(|(_, r)| r.starts_with("duplicate")).count();
        let other = failures.len() - dup_skips;
        if dup_skips == 0 {
            return Err(anyhow::anyhow!("All downloads failed for {:?}. {}", query, log_note));
        }
        emit(ScrapeEvent::Done { downloaded: vec![], log_note: log_note.clone(), dest_dir: dest_dir.to_string() });
        return Ok(format!(
            "0 new images for {:?} — {} candidate(s) already in the bin{}.\n{}",
            query, dup_skips,
            if other > 0 { format!(", {} failed", other) } else { String::new() },
            log_note
        ));
    }

    emit(ScrapeEvent::Done { downloaded: downloaded.clone(), log_note: log_note.clone(), dest_dir: dest_dir.to_string() });

    Ok(format!(
        "Downloaded {}/{} images to {}\n{}\nFiles:\n{}",
        downloaded.len(), count, dest_dir, log_note,
        downloaded.join("\n")
    ))
}

// ── Scrapers ──────────────────────────────────────────────────────────────────

/// Shared context for fetching engine pages through the controlled browser:
/// the browser itself, where to dump debug HTML, and the progress channel.
struct BrowserFetch<'a> {
    browser: &'a crate::tools::controlled_browser::ControlledBrowser,
    log_dir: &'a str,
    progress: &'a Option<UnboundedSender<ScrapeEvent>>,
}

/// Fetch an engine's results page through the real headed browser and parse it with
/// `parse`. If the page is a captcha challenge, prompt the user (via a Phase event)
/// and wait for them to solve it before extracting.
async fn scrape_via_browser(
    fetch: &BrowserFetch<'_>,
    source: &'static str,
    url: &str,
    max: usize,
    parse: fn(&str, usize) -> Vec<String>,
    cookies: &[(&str, &str, &str)],
) -> ScrapeResult {
    let BrowserFetch { browser, log_dir, progress } = *fetch;
    let emit = |e: ScrapeEvent| { if let Some(tx) = progress { let _ = tx.send(e); } };

    let html = match browser.scrape_search_page(url, 3, cookies).await {
        Ok(h) => h,
        Err(e) => return ScrapeResult::err(source, format!("browser: {}", e)),
    };

    // Parse FIRST. If results are present, any captcha marker is a false positive
    // (e.g. Brave is Cloudflare-fronted, so its normal page mentions cloudflare
    // challenge scripts) — never wait, which avoids multi-minute hangs.
    let urls = parse(&html, max);
    if !urls.is_empty() {
        return ScrapeResult::ok(source, urls, &html);
    }

    // 0 results: dump the rendered HTML for diagnosis…
    dump_debug_html(log_dir, source, &html);

    // …and only treat it as a real captcha if a challenge marker is also present.
    if is_captcha_page(&html) {
        emit(ScrapeEvent::Phase {
            label: format!("Solve the captcha for {} in the browser window — then it continues…", source),
        });
        match wait_for_captcha_clear(browser, std::time::Duration::from_secs(120)).await {
            Some(h) => {
                let urls = parse(&h, max);
                if urls.is_empty() { dump_debug_html(log_dir, source, &h); }
                ScrapeResult::ok(source, urls, &h)
            }
            None => ScrapeResult::err(source, "captcha — not solved in time".to_string()),
        }
    } else {
        // Genuinely empty (no results, no challenge).
        ScrapeResult::ok(source, urls, &html)
    }
}

/// Write a browser engine's rendered HTML to `logs\<engine>_debug.html` so a parser
/// that returns 0 URLs can be fixed against the real DOM.
fn dump_debug_html(log_dir: &str, source: &str, html: &str) {
    let _ = std::fs::create_dir_all(log_dir);
    let path = format!("{}\\{}_debug.html",
        log_dir.trim_end_matches(['\\', '/']), source.to_lowercase());
    let _ = std::fs::write(&path, html);
}

/// Poll the current page until it's no longer a captcha challenge, or `timeout`
/// elapses. Returns the cleared HTML on success.
async fn wait_for_captcha_clear(
    browser: &crate::tools::controlled_browser::ControlledBrowser,
    timeout: std::time::Duration,
) -> Option<String> {
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let html = browser.raw_html().await.ok()?;
        if !is_captcha_page(&html) {
            return Some(html);
        }
        if start.elapsed() > timeout {
            return None;
        }
    }
}

// ── Per-engine page URLs ────────────────────────────────────────────────────────
// `page` is 0-indexed. Each builder maps it to that engine's pagination param so the
// scraper can keep pulling fresh results until the success target is met.

fn yandex_page_url(encoded_query: &str, page: usize) -> String {
    format!(
        "https://yandex.com/images/search?text={}&nomisspell=1&numdoc=50&filter=0&itype=photo&p={}",
        encoded_query, page
    )
}

fn bing_page_url(encoded_query: &str, page: usize) -> String {
    // Bing's `first` is a 1-indexed result offset; 50 results per page.
    format!(
        "https://www.bing.com/images/search?q={}&count=50&first={}&safeSearch=Off&adlt=off&mkt=en-US",
        encoded_query, page * 50 + 1
    )
}

fn brave_page_url(encoded_query: &str, page: usize) -> String {
    format!(
        "https://search.brave.com/images?q={}&safesearch=off&source=web&offset={}",
        encoded_query, page
    )
}

/// Parse original image URLs from a Bing images results page.
fn parse_bing(html: &str, max: usize) -> Vec<String> {
    let mut urls = Vec::new();
    // Primary: HTML-entity encoded data-m attributes
    extract_between(html, "&quot;murl&quot;:&quot;", "&quot;", max, &mut urls);
    // Fallback 1: plain JSON in script blocks / decoded DOM attributes
    if urls.is_empty() {
        extract_between(html, "\"murl\":\"", "\"", max, &mut urls);
    }
    // Fallback 2: data-imgurl attributes (older Bing layout)
    if urls.is_empty() {
        extract_between(html, "data-imgurl=\"", "\"", max, &mut urls);
    }
    urls
}

async fn scrape_duckduckgo_images(client: &reqwest::Client, query: &str, max: usize, page: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let offset = page * 100; // DDG's i.js `s` param pages ~100 results at a time

    // Step 1: Pre-seed safe search OFF in cookie store by visiting DDG with kp=-2
    // This causes DDG to set the safe search preference cookie in our cookie jar
    let _ = client.get("https://duckduckgo.com/?kp=-2")
        .header("Accept", "text/html,*/*;q=0.8")
        .send().await;

    // Step 2: Now do the actual image search — cookies carry the safe search pref
    let page_url = format!("https://duckduckgo.com/?q={}&iax=images&ia=images&kp=-2", encoded);

    let html = match client.get(&page_url)
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none")
        .send().await
    {
        Err(e) => return ScrapeResult::err("DDG", format!("page request: {}", e)),
        Ok(r) if !r.status().is_success() =>
            return ScrapeResult::err("DDG", format!("page HTTP {}", r.status())),
        Ok(r) => match r.text().await {
            Err(e) => return ScrapeResult::err("DDG", format!("page read: {}", e)),
            Ok(h) => h,
        }
    };

    let vqd = match extract_vqd(&html) {
        Some(v) => v,
        None => {
            // VQD missing — log the HTML snippet to help diagnose layout changes
            return ScrapeResult { source: "DDG", urls: vec![], error: None,
                debug_hint: Some(format!("vqd not found. HTML: {:.400}",
                    html.chars().take(400).collect::<String>().replace('\n', " "))) };
        }
    };

    let api_url = format!(
        "https://duckduckgo.com/i.js?q={}&vqd={}&o=json&l=us-en&s={}&f=,,,,,&p=-2",
        encoded, vqd, offset
    );
    info!("DDG vqd={}", vqd);
    let resp = match client.get(&api_url)
        .header("Referer", &page_url)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-origin")
        .send().await
    {
        Err(e) => return ScrapeResult::err("DDG", format!("api request: {}", e)),
        Ok(r) => r,
    };

    if !resp.status().is_success() {
        return ScrapeResult::err("DDG", format!("api HTTP {}", resp.status()));
    }

    match resp.json::<serde_json::Value>().await {
        Err(e) => ScrapeResult::err("DDG", format!("json: {}", e)),
        Ok(data) => {
            let mut urls = Vec::new();
            if let Some(results) = data["results"].as_array() {
                for r in results {
                    if urls.len() >= max { break; }
                    if let Some(u) = r["image"].as_str() {
                        if u.starts_with("http") { urls.push(u.to_string()); }
                    }
                }
            }
            let raw = data.to_string();
            ScrapeResult::ok("DDG", urls, &raw)
        }
    }
}

/// True when a results page is actually a bot/captcha challenge rather than results.
/// Covers Yandex SmartCaptcha, Google `/sorry`, and generic reCAPTCHA/hCaptcha/Cloudflare.
fn is_captcha_page(html: &str) -> bool {
    const MARKERS: &[&str] = &[
        "SmartCaptcha", "showcaptcha", "/checkcaptcha", "captcha-required",
        "/sorry/index", "g-recaptcha", "h-captcha", "hcaptcha.com",
        "challenges.cloudflare.com", "Checking your browser",
    ];
    MARKERS.iter().any(|m| html.contains(m))
}

/// Parse original image URLs from a Yandex images results page.
fn parse_yandex(html: &str, max: usize) -> Vec<String> {
    let mut urls = Vec::new();

    // img_href lives in the `data-bem` JSON. The real browser serializes that
    // attribute with its quotes HTML-encoded (`&quot;`), while a raw HTTP response
    // uses literal quotes — try both. Yandex also escapes slashes as `\/`.
    let mut hrefs = Vec::new();
    extract_between(html, "&quot;img_href&quot;:&quot;", "&quot;", max, &mut hrefs);
    extract_between(html, "\"img_href\":\"", "\"", max, &mut hrefs);
    for u in &hrefs {
        let unescaped = u.replace("\\/", "/");
        if unescaped.starts_with("http") && unescaped.len() > 12 && !urls.contains(&unescaped) {
            urls.push(unescaped);
        }
    }

    // No-JS layout: extract img_url= from result link hrefs, e.g.
    //   href="/images/search?...&img_url=https%3A%2F%2Fexample.com%2Fimage.jpg&..."
    if urls.is_empty() {
        let mut encoded_urls = Vec::new();
        extract_between(html, "img_url=http", "&", max, &mut encoded_urls);
        for u in &encoded_urls {
            let full = format!("http{}", u);
            let decoded = urlencoding::decode(&full)
                .map(|c| c.into_owned())
                .unwrap_or(full);
            if decoded.len() > 12 { urls.push(decoded); }
        }
    }

    // Thumbnails on avatars.mds.yandex.net
    if urls.is_empty() {
        let mut thumb_urls = Vec::new();
        extract_between(html, "src=\"//avatars.mds.yandex.net/", "\"", max, &mut thumb_urls);
        for u in &thumb_urls {
            urls.push(format!("https://avatars.mds.yandex.net/{}", u));
        }
    }
    // im0-tub style thumbs
    if urls.is_empty() {
        let mut thumb_urls = Vec::new();
        extract_between(html, "src=\"//im", "\"", max, &mut thumb_urls);
        for u in &thumb_urls {
            if u.contains(".yandex.net/") || u.contains(".yandex.ru/") {
                urls.push(format!("https://im{}", u));
            }
        }
    }

    urls
}

/// Parse image URLs from a Brave images results page. Brave proxies every image
/// through `imgs.search.brave.com`, so we keep only those.
fn parse_brave(html: &str, max: usize) -> Vec<String> {
    let mut urls = Vec::new();
    let mut all_hrefs = Vec::new();
    extract_between(html, "href=\"", "\"", max * 3, &mut all_hrefs);
    extract_between(html, "src=\"", "\"", max * 3, &mut all_hrefs);
    for u in &all_hrefs {
        if u.contains("imgs.search.brave.com/") && !urls.contains(u) {
            urls.push(u.clone());
        }
        if urls.len() >= max { break; }
    }
    urls
}

// ── Content dedup (pHash) ───────────────────────────────────────────────────────

type Phash = image_hasher::ImageHash<Box<[u8]>>;

/// Perceptual-hash encoded image bytes off the async runtime (decode is CPU-heavy).
/// Returns `None` on decode failure.
async fn phash_async(bytes: &[u8]) -> Option<Phash> {
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || crate::tools::image_curate::phash_bytes(&bytes))
        .await
        .ok()
        .flatten()
}

/// Decide whether a freshly downloaded image is a content duplicate. When `dedupe`
/// is on, hashes `bytes` and compares it against `seen_hashes` (existing bin images
/// plus images kept earlier this run). Returns `Some(reason)` to skip a near-duplicate;
/// otherwise records the hash and returns `None`. Fail-open: an undecodable image is
/// kept. With `dedupe` off it always returns `None`.
async fn dedupe_skip(dedupe: bool, bytes: &[u8], seen_hashes: &mut Vec<Phash>) -> Option<String> {
    if !dedupe {
        return None;
    }
    let h = phash_async(bytes).await?;
    if seen_hashes.iter().any(|e| e.dist(&h) <= crate::tools::image_curate::DEDUPE_DIST) {
        Some("duplicate of existing image".to_string())
    } else {
        seen_hashes.push(h);
        None
    }
}

/// Perceptual-hash every image already in `dir` (non-recursive), off the runtime, so a
/// resumed scrape can skip re-downloading images the bin already holds.
async fn hash_dir_images(dir: &str) -> Vec<Phash> {
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || {
        let mut paths = Vec::new();
        crate::tools::image_curate::collect_images(Path::new(&dir), false, &mut paths);
        paths
            .iter()
            .filter_map(|p| std::fs::read(p).ok())
            .filter_map(|b| crate::tools::image_curate::phash_bytes(&b))
            .collect()
    })
    .await
    .unwrap_or_default()
}

// ── SSRF guard ──────────────────────────────────────────────────────────────────

/// True for addresses we must never fetch from search results: loopback, private,
/// link-local (incl. the `169.254.169.254` cloud-metadata endpoint), CGNAT,
/// broadcast, documentation, and unspecified ranges. Defense-in-depth against SSRF
/// via a malicious image URL or redirect.
fn is_blocked_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_blocked_v4(v4),
        std::net::IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(&mapped);
            }
            let head = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (head & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (head & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

fn is_blocked_v4(v4: &std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
}

/// Reject a URL whose host is — or resolves to — a non-public address before we ever
/// connect. Public IP literals pass without a DNS lookup; hostnames are resolved and
/// every returned address must be public.
async fn ensure_public_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid url: {}", e))?;
    let host = parsed.host_str().ok_or_else(|| anyhow::anyhow!("url has no host"))?;

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return if is_blocked_ip(&ip) {
            Err(anyhow::anyhow!("blocked non-public address {}", ip))
        } else {
            Ok(())
        };
    }

    let port = parsed.port_or_known_default().unwrap_or(80);
    let mut any = false;
    for addr in tokio::net::lookup_host((host, port)).await
        .map_err(|e| anyhow::anyhow!("dns resolve failed for {}: {}", host, e))?
    {
        any = true;
        if is_blocked_ip(&addr.ip()) {
            return Err(anyhow::anyhow!("{} resolves to blocked address {}", host, addr.ip()));
        }
    }
    if !any {
        return Err(anyhow::anyhow!("dns returned no addresses for {}", host));
    }
    Ok(())
}

/// Redirect policy that follows up to 5 hops but refuses any hop to an IP-literal
/// non-public address — the classic SSRF "public URL → http://127.0.0.1" trick.
fn safe_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        if let Some(host) = attempt.url().host_str() {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                if is_blocked_ip(&ip) {
                    return attempt.error("redirect to blocked non-public address");
                }
            }
        }
        attempt.follow()
    })
}

// ── Download ──────────────────────────────────────────────────────────────────

const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024; // 6 MB

/// Download an image URL, returning (bytes, extension).
/// Streams in chunks, validates magic bytes, enforces size cap.
async fn download_image_bytes(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, &'static str)> {
    use futures_util::StreamExt;

    // Refuse non-public targets before connecting (SSRF guard).
    ensure_public_url(url).await?;

    // Use a domain-appropriate Referer — Reddit images 403 without reddit.com as referer
    let referer = if url.contains("redd.it") || url.contains("reddit.com") {
        "https://www.reddit.com/"
    } else if url.contains("bing.") || url.contains("bing.net") {
        "https://www.bing.com/"
    } else {
        "https://www.google.com/"
    };

    let resp = client.get(url)
        .header("Referer", referer)
        .header("Accept", "image/jpeg,image/png,image/gif,image/*;q=0.8")
        .send().await
        .map_err(|e| anyhow::anyhow!("request: {}", e))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("HTTP {}", resp.status()));
    }

    if let Some(len) = resp.content_length() {
        if len as usize > MAX_IMAGE_BYTES {
            return Err(anyhow::anyhow!("Content-Length {} > {}MB cap", len, MAX_IMAGE_BYTES / 1024 / 1024));
        }
    }

    let ct_ext = resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(content_type_to_ext);

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("stream: {}", e))?;
        if buf.len() + chunk.len() > MAX_IMAGE_BYTES {
            return Err(anyhow::anyhow!("exceeded {}MB cap mid-stream", MAX_IMAGE_BYTES / 1024 / 1024));
        }
        buf.extend_from_slice(&chunk);
    }

    let ext = validate_image_bytes(&buf, ct_ext, url)?;
    Ok((buf, ext))
}

fn validate_image_bytes(bytes: &[u8], _ct_ext: Option<&'static str>, url: &str) -> Result<&'static str> {
    if bytes.len() < 512 {
        return Err(anyhow::anyhow!("too small ({} bytes)", bytes.len()));
    }
    if bytes.starts_with(b"\xFF\xD8\xFF")                              { Ok("jpg") }
    else if bytes.starts_with(b"\x89PNG\r\n\x1a\n")                   { Ok("png") }
    else if bytes.starts_with(b"GIF8")                                 { Ok("gif") }
    else if bytes.len() >= 12 && &bytes[..4] == b"RIFF"
                              && &bytes[8..12] == b"WEBP"              { Ok("webp") }
    else {
        Err(anyhow::anyhow!(
            "not an image (magic {:02X?}): {}", &bytes[..bytes.len().min(4)], url
        ))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_between(haystack: &str, needle: &str, end_marker: &str, max: usize, out: &mut Vec<String>) {
    let mut pos = 0;
    while out.len() < max {
        match haystack[pos..].find(needle) {
            None => break,
            Some(rel) => {
                let start = pos + rel + needle.len();
                match haystack[start..].find(end_marker) {
                    None => break,
                    Some(end_rel) => {
                        let candidate = &haystack[start..start + end_rel];
                        if candidate.starts_with("http") && !candidate.contains(' ') && candidate.len() > 12 {
                            out.push(candidate.to_string());
                        }
                        pos = start + end_rel + end_marker.len();
                    }
                }
            }
        }
    }
}

fn extract_vqd(html: &str) -> Option<String> {
    for needle in &["vqd='", "vqd=\"", "vqd="] {
        if let Some(pos) = html.find(needle) {
            let rest = &html[pos + needle.len()..];
            let end = rest.find(['\'', '"', '&', ' ', '\n'])
                .unwrap_or_else(|| rest.len().min(80));
            let token = rest[..end].trim_matches(|c| c == '\'' || c == '"').to_string();
            if token.len() > 3 { return Some(token); }
        }
    }
    None
}

/// Returns true for stock-photo CDNs that require auth and always 400/403.
fn is_paywalled_url(url: &str) -> bool {
    const BLOCKED: &[&str] = &[
        "media.gettyimages.com",
        "media.istockphoto.com",
        "shutterstock.com/image",
        "alamy.com/",
        "stock.adobe.com",
        "dreamstime.com/",
        "depositphotos.com/",
        "123rf.com/",
    ];
    BLOCKED.iter().any(|b| url.contains(b))
}

/// HTML-entity decode, remove paywalled URLs, and order-preserving dedup.
pub fn filter_candidates(urls: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .map(|u| u.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""))
        .filter(|u| !is_paywalled_url(u))
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Pacing + verification knobs for a bulk download run.
#[derive(Default)]
pub struct DownloadOpts {
    /// Delay between downloads, in milliseconds. 0 + no verify ⇒ fast concurrent path.
    pub delay_ms: u64,
    /// Vision-QA keep/discard gate; `None` ⇒ keep everything.
    pub verify: Option<VerifyConfig>,
}

/// Download a list of URLs into `dest_dir`, stopping once `count` succeed.
/// Files are named `<name_hint>_NNN.ext` (name_hint is sanitized).
/// Emits `Downloaded`/`Failed` events; logs results into `log`.
/// Returns the sorted list of successfully downloaded file paths.
pub async fn download_urls_to_dir(
    urls: Vec<String>,
    count: usize,
    dest_dir: &str,
    name_hint: &str,
    opts: DownloadOpts,
    log: &mut SessionLog,
    progress: &Option<UnboundedSender<ScrapeEvent>>,
) -> Result<Vec<String>> {
    let DownloadOpts { delay_ms, verify } = opts;
    let emit = |e: ScrapeEvent| { if let Some(tx) = progress { let _ = tx.send(e); } };

    emit(ScrapeEvent::Phase { label: "Downloading".into() });

    let candidates = filter_candidates(urls);
    log.push(format!("-- Downloading (target: {}, pool: {}) --", count, candidates.len()));

    let sanitized = sanitize_filename(name_hint);
    let dest_base = dest_dir.trim_end_matches(['\\', '/']).to_string();

    let client = Arc::new(
        reqwest::Client::builder()
            .cookie_store(true)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .redirect(safe_redirect_policy())
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?
    );

    let mut downloaded: Vec<String> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new(); // (url, reason)
    let mut seq = highest_existing_index(&dest_base, &sanitized);

    download_batch(
        &client, candidates, count, &dest_base, &sanitized,
        delay_ms, &verify, false, &mut Vec::new(),
        &mut downloaded, &mut failures, &mut seq, progress,
    ).await;

    downloaded.sort();
    log_download_summary(log, &downloaded, &failures, count, verify.is_some());
    Ok(downloaded)
}

/// Download from `candidates`, appending successful paths to `downloaded` and
/// `(url, reason)` pairs to `failures`, until `downloaded` reaches `target_total`
/// successes or the list is exhausted. Only *successful* downloads count toward the
/// target — failures are recorded but never advance it. `seq` is a running file-number
/// counter so naming (`<name>_NNN.ext`) stays unique and continuous across the
/// multiple paginated batches that make up one request.
#[allow(clippy::too_many_arguments)]
async fn download_batch(
    client: &Arc<reqwest::Client>,
    candidates: Vec<String>,
    target_total: usize,
    dest_base: &str,
    sanitized: &str,
    delay_ms: u64,
    verify: &Option<VerifyConfig>,
    dedupe: bool,
    seen_hashes: &mut Vec<Phash>,
    downloaded: &mut Vec<String>,
    failures: &mut Vec<(String, String)>,
    seq: &mut usize,
    progress: &Option<UnboundedSender<ScrapeEvent>>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let emit = |e: ScrapeEvent| { if let Some(tx) = progress { let _ = tx.send(e); } };

    // The vision gate, content dedup, and any non-zero pacing delay all force the
    // sequential path: download one candidate, optionally judge/hash it, keep or
    // discard, then pace.
    if verify.is_some() || dedupe || delay_ms > 0 {
        for url in &candidates {
            if downloaded.len() >= target_total { break; }
            match download_image_bytes(client, url).await {
                Ok((bytes, ext)) => {
                    let (keep, reason) = match verify {
                        Some(cfg) => {
                            emit(ScrapeEvent::Verifying { url: url.clone(), done: downloaded.len(), target: target_total });
                            vision_judge(&bytes, ext, cfg).await
                        }
                        None => (true, String::new()),
                    };
                    if !keep {
                        let reason = format!("rejected: {}", reason);
                        debug!("SKIP {} — {}", url, reason);
                        failures.push((url.clone(), reason.clone()));
                        emit(ScrapeEvent::Failed { url: url.clone(), reason });
                    } else if let Some(dup) = dedupe_skip(dedupe, &bytes, seen_hashes).await {
                        debug!("DUP  {} — {}", url, dup);
                        failures.push((url.clone(), dup.clone()));
                        emit(ScrapeEvent::Failed { url: url.clone(), reason: dup });
                    } else {
                        let path = next_free_path(dest_base, sanitized, ext, seq);
                        match std::fs::write(&path, &bytes) {
                            Ok(_) => {
                                debug!("OK  {}", path);
                                downloaded.push(path.clone());
                                emit(ScrapeEvent::Downloaded { done: downloaded.len(), target: target_total, path });
                            }
                            Err(e) => {
                                let reason = format!("write: {}", e);
                                failures.push((url.clone(), reason.clone()));
                                emit(ScrapeEvent::Failed { url: url.clone(), reason });
                            }
                        }
                    }
                }
                Err(e) => {
                    failures.push((url.clone(), e.to_string()));
                    emit(ScrapeEvent::Failed { url: url.clone(), reason: e.to_string() });
                }
            }
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }
        return;
    }

    // Fast path: 3 concurrent downloads, no verification or pacing. A shared atomic
    // hands out file numbers so concurrent successes never collide, continuing from
    // `seq` (the count carried over from previous pages).
    let sem = Arc::new(Semaphore::new(3));
    let seq_atomic = Arc::new(AtomicUsize::new(*seq));
    let mut tasks = tokio::task::JoinSet::new();

    for url in candidates {
        let client = client.clone();
        let sem = sem.clone();
        let sanitized = sanitized.to_string();
        let dest_base = dest_base.to_string();
        let seq_atomic = seq_atomic.clone();
        tasks.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            match download_image_bytes(&client, &url).await {
                Ok((bytes, ext)) => {
                    // Unique number per success; skip any number whose file already
                    // exists (resume into a non-empty bin never overwrites).
                    let mut n = seq_atomic.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut path = format!("{}\\{}_{:03}.{}", dest_base, sanitized, n, ext);
                    while Path::new(&path).exists() {
                        n = seq_atomic.fetch_add(1, Ordering::SeqCst) + 1;
                        path = format!("{}\\{}_{:03}.{}", dest_base, sanitized, n, ext);
                    }
                    match std::fs::write(&path, &bytes) {
                        Ok(_) => Some((true, url, path, String::new())),
                        Err(e) => Some((false, url, String::new(), format!("write: {}", e))),
                    }
                }
                Err(e) => Some((false, url, String::new(), e.to_string())),
            }
        });
    }

    while let Some(task_result) = tasks.join_next().await {
        if let Ok(Some((ok, url, path, reason))) = task_result {
            if ok {
                debug!("OK  {}", path);
                downloaded.push(path.clone());
                emit(ScrapeEvent::Downloaded { done: downloaded.len(), target: target_total, path });
                if downloaded.len() >= target_total {
                    tasks.abort_all();
                    break;
                }
            } else {
                debug!("FAIL {} — {}", url, reason);
                failures.push((url.clone(), reason.clone()));
                emit(ScrapeEvent::Failed { url, reason });
            }
        }
    }
    *seq = seq_atomic.load(Ordering::SeqCst);
}

/// Append the post-download summary to the session log: success count, grouped
/// failure reasons, a sample of failed URLs, and the final file list.
fn log_download_summary(
    log: &mut SessionLog,
    downloaded: &[String],
    failures: &[(String, String)],
    count: usize,
    verify_on: bool,
) {
    log.push(format!("Downloaded: {}/{}", downloaded.len(), count));
    if downloaded.len() < count {
        log.push(format!(
            "Short of target: approved {}/{} — sources exhausted{}",
            downloaded.len(), count,
            if verify_on { " (raise count or loosen the vision prompt)" } else { " (raise count or enable more sources)" }
        ));
    }
    if !failures.is_empty() {
        log.push(format!("Failed: {} URLs", failures.len()));
        // Group failures by reason for readability.
        let mut reason_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (_, reason) in failures {
            let key = reason.chars().take(60).collect::<String>();
            *reason_counts.entry(key).or_insert(0) += 1;
        }
        for (reason, c) in &reason_counts {
            log.push(format!("  x{} — {}", c, reason));
        }
        // Log up to 10 specific failed URLs for targeted debugging.
        log.push("  Sample failed URLs:".to_string());
        for (url, reason) in failures.iter().take(10) {
            log.push(format!("    [{}] {}", reason.chars().take(40).collect::<String>(), url));
        }
    }
    if !downloaded.is_empty() {
        log.push("Files:".to_string());
        for f in downloaded {
            log.push(format!("  {}", f));
        }
    }
}

fn content_type_to_ext(ct: &str) -> Option<&'static str> {
    if ct.contains("jpeg") || ct.contains("jpg") { Some("jpg") }
    else if ct.contains("png")  { Some("png") }
    else if ct.contains("gif")  { Some("gif") }
    else if ct.contains("webp") { Some("webp") }
    else { None }
}

/// Count image files directly inside `dir` (non-recursive). A bin is "empty" when
/// this is 0 — `_bow_dupes` and non-image files don't count.
fn bin_image_count(dir: &Path) -> usize {
    let mut v = Vec::new();
    crate::tools::image_curate::collect_images(dir, false, &mut v);
    v.len()
}

/// Auto-select a bin under `parent`: the lowest `N` in `1..=10` that is either
/// missing or exists but holds no images (reusing an empty bin). Creates the chosen
/// bin if needed. Errors when all ten bins already contain images.
pub fn pick_auto_bin(parent: &str) -> Result<String> {
    let base = parent.trim_end_matches(['\\', '/']);
    std::fs::create_dir_all(base)
        .map_err(|e| anyhow::anyhow!("Failed to create '{}': {}", base, e))?;
    for n in 1..=10u32 {
        let candidate = format!("{}\\{}", base, n);
        let path = Path::new(&candidate);
        if !path.exists() {
            std::fs::create_dir(&candidate)
                .map_err(|e| anyhow::anyhow!("Failed to create '{}': {}", candidate, e))?;
            return Ok(candidate);
        }
        if path.is_dir() && bin_image_count(path) == 0 {
            return Ok(candidate);
        }
    }
    Err(anyhow::anyhow!(
        "All 10 bins under '{}' contain images — clear one or pick a bin manually.", base
    ))
}

/// Resolve a user-chosen bin number (`1..=10`) under `parent` to its path, creating
/// it if needed. Returns the bin even when it already contains images (resume/append).
pub fn resolve_manual_bin(parent: &str, n: u32) -> Result<String> {
    if !(1..=10).contains(&n) {
        return Err(anyhow::anyhow!("Bin must be between 1 and 10 (got {})", n));
    }
    let base = parent.trim_end_matches(['\\', '/']);
    let candidate = format!("{}\\{}", base, n);
    std::fs::create_dir_all(&candidate)
        .map_err(|e| anyhow::anyhow!("Failed to create '{}': {}", candidate, e))?;
    Ok(candidate)
}

/// Highest `N` among files named `<prefix>_NNN.<ext>` directly in `dir` (0 if none).
/// Lets a resumed scrape continue numbering instead of overwriting existing files.
fn highest_existing_index(dir: &str, prefix: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let needle = format!("{}_", prefix);
    let mut max = 0usize;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_image_name(name) { continue; }
        let Some(stem) = Path::new(name).file_stem().and_then(|s| s.to_str()) else { continue };
        if let Some(rest) = stem.strip_prefix(&needle) {
            if let Ok(n) = rest.parse::<usize>() {
                max = max.max(n);
            }
        }
    }
    max
}

/// True when `name` has a known image extension (case-insensitive).
fn is_image_name(name: &str) -> bool {
    matches!(
        Path::new(name).extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff")
    )
}

/// Advance `*seq` to the next index whose `<prefix>_NNN.<ext>` path doesn't already
/// exist in `dest_base`, and return that path. Guarantees existing files are never
/// overwritten, even across paginated batches and resumed scrapes.
fn next_free_path(dest_base: &str, prefix: &str, ext: &str, seq: &mut usize) -> String {
    loop {
        *seq += 1;
        let path = format!("{}\\{}_{:03}.{}", dest_base, prefix, *seq, ext);
        if !Path::new(&path).exists() {
            return path;
        }
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn source_enabled_filters() {
        assert!(source_enabled(&None, "bing"));
        assert!(source_enabled(&Some(vec![]), "bing"));
        assert!(source_enabled(&Some(vec!["bing".into(), "ddg".into()]), "BING"));
        assert!(!source_enabled(&Some(vec!["ddg".into()]), "bing"));
    }

    #[test]
    fn parse_verdict_keep_discard_and_malformed() {
        let (k, _) = parse_verdict("{\"keep\": true, \"reason\": \"sharp cat\"}");
        assert!(k);
        let (k, r) = parse_verdict("sure: {\"keep\": false, \"reason\": \"blurry\"} done");
        assert!(!k);
        assert_eq!(r, "blurry");
        // Malformed → keep (never silently drop a valid image)
        let (k, r) = parse_verdict("the model rambled with no json");
        assert!(k);
        assert!(r.contains("unparsed"));
    }

    fn write_test_png(path: &Path) {
        use image::{Rgb, RgbImage};
        RgbImage::from_pixel(8, 8, Rgb([1, 2, 3])).save(path).unwrap();
    }

    fn fresh_base(tag: &str) -> (std::path::PathBuf, String) {
        let base = std::env::temp_dir().join(format!("bow_bin_{}_{}", tag, uuid::Uuid::new_v4().simple()));
        let s = base.to_string_lossy().to_string();
        (base, s)
    }

    #[test]
    fn auto_bin_reuses_existing_empty_bin() {
        let (base, base_s) = fresh_base("reuse");
        // Bin 1 has an image; bin 2 exists but is empty → reuse 2.
        std::fs::create_dir_all(base.join("1")).unwrap();
        write_test_png(&base.join("1").join("a.png"));
        std::fs::create_dir_all(base.join("2")).unwrap();

        let chosen = pick_auto_bin(&base_s).unwrap();
        assert!(chosen.ends_with("\\2"), "should reuse empty bin 2, got {}", chosen);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn auto_bin_creates_lowest_missing_when_no_empty_bin() {
        let (base, base_s) = fresh_base("missing");
        // Bin 1 has an image; bins 2..=10 don't exist → create 2.
        std::fs::create_dir_all(base.join("1")).unwrap();
        write_test_png(&base.join("1").join("a.png"));

        let chosen = pick_auto_bin(&base_s).unwrap();
        assert!(chosen.ends_with("\\2"), "should create bin 2, got {}", chosen);
        assert!(Path::new(&chosen).is_dir());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn auto_bin_errors_when_all_ten_contain_images() {
        let (base, base_s) = fresh_base("full");
        for n in 1..=10 {
            let d = base.join(n.to_string());
            std::fs::create_dir_all(&d).unwrap();
            write_test_png(&d.join("a.png"));
        }
        assert!(pick_auto_bin(&base_s).is_err(), "all 10 full → error");

        std::fs::remove_dir_all(&base).ok();
    }

    fn encode_png(img: image::RgbImage) -> Vec<u8> {
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }
    fn gradient_png(w: u32, h: u32) -> Vec<u8> {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(w, h);
        for y in 0..h { for x in 0..w { let v = ((x + y) % 256) as u8; img.put_pixel(x, y, Rgb([v, v, v])); } }
        encode_png(img)
    }
    fn checker_png(w: u32, h: u32) -> Vec<u8> {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(w, h);
        for y in 0..h { for x in 0..w {
            let c = if (x / 64 + y / 64) % 2 == 0 { [0, 0, 0] } else { [255, 255, 255] };
            img.put_pixel(x, y, Rgb(c));
        } }
        encode_png(img)
    }

    #[test]
    fn blocks_private_and_reserved_ips() {
        use std::net::IpAddr;
        for s in ["127.0.0.1", "10.0.0.1", "192.168.1.5", "172.16.0.1",
                  "169.254.169.254", "100.64.0.1", "0.0.0.0",
                  "::1", "fe80::1", "fc00::1", "::ffff:127.0.0.1"] {
            assert!(is_blocked_ip(&s.parse::<IpAddr>().unwrap()), "{} should be blocked", s);
        }
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34",
                  "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(!is_blocked_ip(&s.parse::<IpAddr>().unwrap()), "{} should be allowed", s);
        }
    }

    #[tokio::test]
    async fn ensure_public_url_blocks_local_and_metadata_targets() {
        assert!(ensure_public_url("http://127.0.0.1/x").await.is_err());
        assert!(ensure_public_url("http://169.254.169.254/latest/meta-data").await.is_err());
        assert!(ensure_public_url("http://[::1]:8080/").await.is_err());
        assert!(ensure_public_url("http://10.1.2.3/img.jpg").await.is_err());
        assert!(ensure_public_url("http://localhost/").await.is_err()); // resolves to loopback
        assert!(ensure_public_url("not a url").await.is_err());
        // Public IP literal: allowed without any DNS lookup.
        assert!(ensure_public_url("https://8.8.8.8/img.jpg").await.is_ok());
    }

    #[tokio::test]
    async fn dedupe_skip_detects_duplicate_and_keeps_distinct() {
        let big = gradient_png(256, 256);
        // A true downscale of the gradient — perceptually identical → duplicate.
        let small = {
            let img = image::load_from_memory(&big).unwrap()
                .resize(128, 128, image::imageops::FilterType::Lanczos3)
                .to_rgb8();
            encode_png(img)
        };
        let distinct = checker_png(256, 256);

        let mut hashes: Vec<Phash> = Vec::new();
        // First image is novel → kept and remembered.
        assert!(dedupe_skip(true, &big, &mut hashes).await.is_none());
        assert_eq!(hashes.len(), 1);
        // Its downscale is a near-duplicate → skipped, not remembered.
        assert!(dedupe_skip(true, &small, &mut hashes).await.is_some());
        assert_eq!(hashes.len(), 1);
        // A structurally different image → kept.
        assert!(dedupe_skip(true, &distinct, &mut hashes).await.is_none());
        assert_eq!(hashes.len(), 2);
        // dedupe disabled → never skips, never records.
        assert!(dedupe_skip(false, &big, &mut hashes).await.is_none());
        assert_eq!(hashes.len(), 2);
    }

    #[test]
    fn highest_existing_index_finds_max_for_prefix() {
        let (base, base_s) = fresh_base("seq");
        std::fs::create_dir_all(&base).unwrap();
        write_test_png(&base.join("query_001.png"));
        write_test_png(&base.join("query_004.png"));
        write_test_png(&base.join("dog_009.png")); // different prefix — ignored
        std::fs::write(base.join("query_007.txt"), b"x").unwrap(); // non-image — ignored
        assert_eq!(highest_existing_index(&base_s, "query"), 4);
        assert_eq!(highest_existing_index(&base_s, "cat"), 0); // none → 0
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn next_free_path_skips_existing_files() {
        let (base, base_s) = fresh_base("free");
        std::fs::create_dir_all(&base).unwrap();
        write_test_png(&base.join("query_001.png"));
        let mut seq = 0usize;
        let p = next_free_path(&base_s, "query", "png", &mut seq);
        assert!(p.ends_with("query_002.png"), "got {}", p);
        assert_eq!(seq, 2);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn manual_bin_rejects_out_of_range() {
        let (base, base_s) = fresh_base("range");
        std::fs::create_dir_all(&base).unwrap();
        assert!(resolve_manual_bin(&base_s, 0).is_err());
        assert!(resolve_manual_bin(&base_s, 11).is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn manual_bin_creates_and_returns_requested() {
        let (base, base_s) = fresh_base("manual");
        let p = resolve_manual_bin(&base_s, 5).unwrap();
        assert!(p.ends_with("\\5"), "got {}", p);
        assert!(Path::new(&p).is_dir());
        // Returns the same bin even when it already has images.
        write_test_png(&Path::new(&p).join("a.png"));
        let p2 = resolve_manual_bin(&base_s, 5).unwrap();
        assert_eq!(p, p2);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pick_vision_model_prefers_loaded_vlm() {
        let json = serde_json::json!({"data": [
            {"id": "qwen-text", "type": "llm", "state": "loaded"},
            {"id": "gemma-4-e4b", "type": "vlm", "state": "loaded"},
            {"id": "other-vlm", "type": "vlm", "state": "not-loaded"},
        ]});
        let (model, warn) = pick_loaded_vision_model(&json, "fallback");
        assert_eq!(model, "gemma-4-e4b");
        assert!(warn.is_none());
    }

    #[test]
    fn pick_vision_model_warns_when_loaded_is_text() {
        let json = serde_json::json!({"data": [
            {"id": "qwen-text", "type": "llm", "state": "loaded"},
        ]});
        let (model, warn) = pick_loaded_vision_model(&json, "fallback");
        assert_eq!(model, "qwen-text");
        assert!(warn.unwrap().contains("not a vision model"));
    }

    #[test]
    fn pick_vision_model_falls_back_when_none_loaded() {
        let json = serde_json::json!({"data": [
            {"id": "x", "type": "vlm", "state": "not-loaded"},
        ]});
        let (model, warn) = pick_loaded_vision_model(&json, "fallback");
        assert_eq!(model, "fallback");
        assert!(warn.is_some());
    }

    #[test]
    fn captcha_page_detected() {
        assert!(is_captcha_page("<html>... SmartCaptcha ...</html>"));
        assert!(is_captcha_page("redirect to /checkcaptcha?key=1"));
        assert!(is_captcha_page("<div class=\"g-recaptcha\"></div>"));
        assert!(is_captcha_page("challenges.cloudflare.com/turnstile"));
        assert!(!is_captcha_page("<html><div class=\"serp-item\">img</div></html>"));
    }

    #[test]
    fn page_urls_advance_per_engine_offsets() {
        // Yandex: 0-indexed page param.
        assert!(yandex_page_url("cats", 0).contains("&p=0"));
        assert!(yandex_page_url("cats", 3).contains("&p=3"));
        // Bing: 1-indexed result offset, 50 per page.
        assert!(bing_page_url("cats", 0).contains("first=1"));
        assert!(bing_page_url("cats", 1).contains("first=51"));
        assert!(bing_page_url("cats", 2).contains("first=101"));
        // Brave: offset param echoes the page index.
        assert!(brave_page_url("cats", 0).contains("offset=0"));
        assert!(brave_page_url("cats", 4).contains("offset=4"));
    }

    #[test]
    fn parse_bing_extracts_murl() {
        let html = "x &quot;murl&quot;:&quot;https://a.com/1.jpg&quot; y \
                    &quot;murl&quot;:&quot;https://b.com/2.png&quot; z";
        let urls = parse_bing(html, 10);
        assert_eq!(urls, vec!["https://a.com/1.jpg", "https://b.com/2.png"]);
    }

    #[test]
    fn parse_brave_keeps_only_proxy_urls() {
        let html = "<a href=\"https://imgs.search.brave.com/abc\">x</a>\
                    <img src=\"https://other.com/skip.jpg\">\
                    <img src=\"https://imgs.search.brave.com/def\">";
        let urls = parse_brave(html, 10);
        assert_eq!(urls, vec![
            "https://imgs.search.brave.com/abc",
            "https://imgs.search.brave.com/def",
        ]);
    }

    #[test]
    fn parse_yandex_unescapes_img_href() {
        let html = r#"...{"img_href":"https:\/\/ex.com\/cat.jpg"}...{"img_href":"https:\/\/ex.com\/dog.png"}..."#;
        let urls = parse_yandex(html, 10);
        assert_eq!(urls, vec!["https://ex.com/cat.jpg", "https://ex.com/dog.png"]);
    }

    #[test]
    fn parse_yandex_handles_browser_entity_encoded() {
        // page.content() serializes data-bem quotes as &quot; — the real-browser case.
        let html = "data-bem=\"{&quot;serp-item&quot;:{&quot;img_href&quot;:&quot;https:\\/\\/ex.com\\/a.jpg&quot;}}\"";
        let urls = parse_yandex(html, 10);
        assert_eq!(urls, vec!["https://ex.com/a.jpg"]);
    }

    #[test]
    fn scrape_event_to_json_shapes() {
        let e = ScrapeEvent::Source { source: "Bing".into(), count: 35, error: None };
        let v = e.to_json();
        assert_eq!(v["kind"], "source");
        assert_eq!(v["source"], "Bing");
        assert_eq!(v["count"], 35);

        let d = ScrapeEvent::Downloaded { done: 3, target: 15, path: "C:\\x\\a.jpg".into() };
        let dv = d.to_json();
        assert_eq!(dv["kind"], "downloaded");
        assert_eq!(dv["done"], 3);
        assert_eq!(dv["target"], 15);
        assert_eq!(dv["path"], "C:\\x\\a.jpg");
    }

    #[test]
    fn clean_tags_flattens_and_prepends_trigger() {
        let raw = "Tags: red hair, blue eyes\n- smiling\n- school uniform";
        let out = clean_caption(raw, "alice", true);
        assert_eq!(out, "alice, red hair, blue eyes, smiling, school uniform");
    }

    #[test]
    fn clean_tags_dedupes_whitespace_and_commas() {
        let out = clean_caption("a,,  b , ,c", "", true);
        assert_eq!(out, "a, b, c");
    }

    #[test]
    fn clean_caption_collapses_to_one_line() {
        let raw = "\"A woman with\n   red hair.\"";
        let out = clean_caption(raw, "", false);
        assert_eq!(out, "A woman with red hair.");
    }

    #[test]
    fn clean_caption_prepends_trigger_for_caption() {
        let out = clean_caption("standing in a field", "bob", false);
        assert_eq!(out, "bob, standing in a field");
    }

    #[test]
    fn filter_candidates_drops_paywalled_and_dedupes() {
        let out = filter_candidates(vec![
            "https://e.com/a.jpg".into(),
            "https://e.com/a.jpg".into(),
            "https://media.gettyimages.com/x.jpg".into(),
        ]);
        assert_eq!(out, vec!["https://e.com/a.jpg".to_string()]);
    }

    #[test]
    fn filter_candidates_decodes_html_entities() {
        let out = filter_candidates(vec![
            "https://e.com/a?foo=1&amp;bar=2".into(),
        ]);
        assert_eq!(out, vec!["https://e.com/a?foo=1&bar=2".to_string()]);
    }

    #[test]
    fn load_resize_produces_png_data_uri_and_downscales() {
        use image::{Rgb, RgbImage};
        let dir = std::env::temp_dir().join(format!("bow_autotag_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.png");
        RgbImage::from_pixel(2000, 1000, Rgb([10, 20, 30])).save(&p).unwrap();

        let uri = load_resize_data_uri(Path::new(&p), 1024).unwrap();
        assert!(uri.starts_with("data:image/png;base64,"), "uri: {}", &uri[..40.min(uri.len())]);

        // Decode the embedded PNG and confirm the longest side was capped.
        let b64 = uri.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        use image::GenericImageView;
        assert_eq!(decoded.dimensions(), (1024, 512));

        std::fs::remove_dir_all(&dir).ok();
    }
}

