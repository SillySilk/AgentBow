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
    Downloaded { done: usize, target: usize, path: String },
    Failed { url: String, reason: String },
    Done { downloaded: Vec<String>, log_note: String },
}

impl ScrapeEvent {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ScrapeEvent::Phase { label } => json!({ "kind": "phase", "label": label }),
            ScrapeEvent::Source { source, count, error } =>
                json!({ "kind": "source", "source": source, "count": count, "error": error }),
            ScrapeEvent::Candidates { total, filtered } =>
                json!({ "kind": "candidates", "total": total, "filtered": filtered }),
            ScrapeEvent::Downloaded { done, target, path } =>
                json!({ "kind": "downloaded", "done": done, "target": target, "path": path }),
            ScrapeEvent::Failed { url, reason } =>
                json!({ "kind": "failed", "url": url, "reason": reason }),
            ScrapeEvent::Done { downloaded, log_note } =>
                json!({ "kind": "done", "downloaded": downloaded, "log_note": log_note }),
        }
    }
}

// ── Session log ───────────────────────────────────────────────────────────────

pub struct SessionLog {
    path: String,
    lines: Vec<String>,
}

impl SessionLog {
    fn new(log_dir: &str, query: &str) -> Self {
        // Ensure logs directory exists; if it fails we'll surface the error in flush()
        let _ = std::fs::create_dir_all(log_dir);
        let path = format!("{}\\bow_downloads.log",
            log_dir.trim_end_matches(['\\', '/']));
        let mut log = Self { path, lines: Vec::new() };
        log.push(format!("=== bow image_download [ts:{}] ===", unix_ts()));
        log.push(format!("query: {:?}", query));
        log
    }
    fn push(&mut self, line: String) {
        info!("{}", line);
        self.lines.push(line);
    }
    /// Write log to disk. Returns a warning string if the write fails so the
    /// caller can surface it — no more silent failures.
    fn flush(&self) -> String {
        match std::fs::OpenOptions::new()
            .create(true).append(true).open(&self.path)
        {
            Err(e) => format!("(log write failed: {} — path: {})", e, self.path),
            Ok(mut f) => {
                let mut ok = true;
                for l in &self.lines {
                    if writeln!(f, "{}", l).is_err() { ok = false; break; }
                }
                let _ = writeln!(f, "");
                if ok {
                    format!("Log: {}", self.path)
                } else {
                    format!("(log partially written — path: {})", self.path)
                }
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

    const MAX_VERIFY_BYTES: usize = 4 * 1024 * 1024;
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
        .post(&format!("{}/v1/chat/completions", lm_studio_url))
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

// ── image_download ────────────────────────────────────────────────────────────

/// Download images matching `query` into `dest_dir`, up to `count` files.
/// Writes a session log to `{log_dir}\\bow_downloads.log`.
/// `sources` is `None`/empty → run all scrapers; otherwise only the named ones.
/// Canonical keys: `bing`, `ddg`, `yandex`, `brave`, `qwant`, `searxng`.
pub async fn image_download(
    query: &str,
    count: usize,
    dest_dir: &str,
    log_dir: &str,
    sources: Option<Vec<String>>,
    progress: Option<UnboundedSender<ScrapeEvent>>,
) -> Result<String> {
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

    let want = count * 4;
    let mut candidates: Vec<String> = Vec::new();

    log.push("-- Scraping sources --".to_string());
    emit(ScrapeEvent::Phase { label: "Scraping sources".into() });

    let mut results: Vec<ScrapeResult> = Vec::new();
    if source_enabled(&sources, "bing")    { results.push(scrape_bing_images(&client, query, want).await); }
    if source_enabled(&sources, "ddg")     { results.push(scrape_duckduckgo_images(&client, query, want).await); }
    if source_enabled(&sources, "yandex")  { results.push(scrape_yandex_images(&client, query, want).await); }
    if source_enabled(&sources, "brave")   { results.push(scrape_brave_images(&client, query, want).await); }
    if source_enabled(&sources, "qwant")   { results.push(scrape_qwant_images(&client, query, want).await); }
    if source_enabled(&sources, "searxng") { results.push(scrape_searxng_images(&client, query, want).await); }

    for r in &results {
        log.push(r.log_line());
        emit(ScrapeEvent::Source { source: r.source.to_string(), count: r.urls.len(), error: r.error.clone() });
        for u in &r.urls {
            if !candidates.contains(u) { candidates.push(u.clone()); }
        }
    }
    log.push(format!("Total candidates: {}", candidates.len()));

    // Filter out known paid/auth-gated CDNs that always return 400/403
    let before = candidates.len();
    candidates.retain(|u| !is_paywalled_url(u));
    let filtered = before - candidates.len();
    if filtered > 0 {
        log.push(format!("Filtered {} paid CDN URLs (Getty, iStock, Shutterstock, Alamy)", filtered));
    }
    emit(ScrapeEvent::Candidates { total: candidates.len(), filtered });

    if candidates.is_empty() {
        log.push("FATAL: no candidates — all scrapers returned 0 URLs".to_string());
        let log_note = log.flush();
        return Err(anyhow::anyhow!(
            "No images found for {:?}. {}", query, log_note
        ));
    }

    // ── Download phase ────────────────────────────────────────────────────────
    let downloaded = download_urls_to_dir(candidates, count, dest_dir, query, &mut log, &progress).await?;

    let log_note = log.flush();

    if downloaded.is_empty() {
        return Err(anyhow::anyhow!(
            "All downloads failed for {:?}. {}", query, log_note
        ));
    }

    emit(ScrapeEvent::Done { downloaded: downloaded.clone(), log_note: log_note.clone() });

    Ok(format!(
        "Downloaded {}/{} images to {}\n{}\nFiles:\n{}",
        downloaded.len(), count, dest_dir, log_note,
        downloaded.join("\n")
    ))
}

// ── Scrapers ──────────────────────────────────────────────────────────────────

async fn scrape_bing_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let url = format!(
        "https://www.bing.com/images/search?q={}&count=50&first=1&safeSearch=Off&adlt=off&mkt=en-US",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.bing.com/")
        .header("Cookie", "SRCHHPGUSR=SRCHLANG=en&ADLT=OFF&NNT=10&NRSLT=50; adlt=off; SUID=M; MSCC=NR; _EDGE_S=ui=en-us")
        .send().await;

    match result {
        Err(e) => ScrapeResult::err("Bing", e.to_string()),
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                return ScrapeResult::err("Bing", format!("HTTP {}", status));
            }
            match resp.text().await {
                Err(e) => ScrapeResult::err("Bing", format!("read: {}", e)),
                Ok(html) => {
                    let mut urls = Vec::new();
                    // Primary: HTML-entity encoded data-m attributes
                    extract_between(&html, "&quot;murl&quot;:&quot;", "&quot;", max, &mut urls);
                    // Fallback 1: plain JSON in script blocks
                    if urls.is_empty() {
                        extract_between(&html, "\"murl\":\"", "\"", max, &mut urls);
                    }
                    // Fallback 2: data-imgurl attributes (older Bing layout)
                    if urls.is_empty() {
                        extract_between(&html, "data-imgurl=\"", "\"", max, &mut urls);
                    }
                    ScrapeResult::ok("Bing", urls, &html)
                }
            }
        }
    }
}

async fn scrape_duckduckgo_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);

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
        "https://duckduckgo.com/i.js?q={}&vqd={}&o=json&l=us-en&s=0&f=,,,,,&p=-2",
        encoded, vqd
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

async fn scrape_yandex_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let url = format!(
        "https://yandex.com/images/search?text={}&nomisspell=1&numdoc=50&filter=0&itype=photo",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "text/html,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Referer", "https://yandex.com/")
        .header("Cookie", "safesearch=0; yp=1999999999.sp.ssp%3D0")
        .send().await;

    match result {
        Err(e) => ScrapeResult::err("Yandex", e.to_string()),
        Ok(r) if !r.status().is_success() =>
            ScrapeResult::err("Yandex", format!("HTTP {}", r.status())),
        Ok(r) => match r.text().await {
            Err(e) => ScrapeResult::err("Yandex", format!("read: {}", e)),
            Ok(html) => {
                let mut urls = Vec::new();

                // JS layout: img_href in data-bem JSON
                extract_between(&html, "\"img_href\":\"", "\"", max, &mut urls);

                // No-JS layout: extract img_url= from result link hrefs
                // These contain URL-encoded original image URLs like:
                //   href="/images/search?...&img_url=https%3A%2F%2Fexample.com%2Fimage.jpg&..."
                if urls.is_empty() {
                    let mut encoded_urls = Vec::new();
                    extract_between(&html, "img_url=http", "&", max, &mut encoded_urls);
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
                    extract_between(&html, "src=\"//avatars.mds.yandex.net/", "\"", max, &mut thumb_urls);
                    for u in &thumb_urls {
                        urls.push(format!("https://avatars.mds.yandex.net/{}", u));
                    }
                }
                // im0-tub style thumbs
                if urls.is_empty() {
                    let mut thumb_urls = Vec::new();
                    extract_between(&html, "src=\"//im", "\"", max, &mut thumb_urls);
                    for u in &thumb_urls {
                        if u.contains(".yandex.net/") || u.contains(".yandex.ru/") {
                            urls.push(format!("https://im{}", u));
                        }
                    }
                }

                ScrapeResult::ok("Yandex", urls, &html)
            }
        }
    }
}

async fn scrape_qwant_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let url = format!(
        "https://api.qwant.com/v3/search/images?q={}&count=50&offset=0&safesearch=0&locale=en_US&tgp=2",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.qwant.com/")
        .header("Origin", "https://www.qwant.com")
        .header("DNT", "1")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-origin")
        .header("X-Requested-With", "XMLHttpRequest")
        .send().await;

    match result {
        Err(e) => ScrapeResult::err("Qwant", e.to_string()),
        Ok(r) if !r.status().is_success() =>
            ScrapeResult::err("Qwant", format!("HTTP {}", r.status())),
        Ok(r) => match r.json::<serde_json::Value>().await {
            Err(e) => ScrapeResult::err("Qwant", format!("json: {}", e)),
            Ok(data) => {
                let mut urls = Vec::new();
                if let Some(items) = data["data"]["result"]["items"].as_array() {
                    for item in items {
                        if urls.len() >= max { break; }
                        if let Some(u) = item["media"].as_str() {
                            if u.starts_with("http") { urls.push(u.to_string()); }
                        }
                    }
                }
                let raw = data.to_string();
                ScrapeResult::ok("Qwant", urls, &raw)
            }
        }
    }
}

async fn scrape_searxng_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let url = format!(
        "https://search.hbubli.cc/search?q={}&category=images&format=json&safesearch=0&pageno=1",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "application/json, */*; q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://search.hbubli.cc/")
        .send().await;

    match result {
        Err(e) => ScrapeResult::err("SearXNG", e.to_string()),
        Ok(r) if r.status() == 429 =>
            ScrapeResult::err("SearXNG", "rate-limited (429)".to_string()),
        Ok(r) if !r.status().is_success() =>
            ScrapeResult::err("SearXNG", format!("HTTP {}", r.status())),
        Ok(r) => {
            let text = match r.text().await {
                Err(e) => return ScrapeResult::err("SearXNG", format!("read: {}", e)),
                Ok(t) => t,
            };
            // Try JSON first (format=json supported)
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                let mut urls = Vec::new();
                if let Some(results) = data["results"].as_array() {
                    for r in results {
                        if urls.len() >= max { break; }
                        let img = r["img_src"].as_str().or_else(|| r["url"].as_str());
                        if let Some(u) = img {
                            if u.starts_with("http") { urls.push(u.to_string()); }
                        }
                    }
                }
                ScrapeResult::ok("SearXNG", urls, &text)
            } else {
                // Instance returned HTML — scrape img src attributes as fallback
                let mut urls = Vec::new();
                extract_between(&text, "data-src=\"http", "\"", max, &mut urls);
                for u in urls.iter_mut() { if !u.starts_with("http") { *u = format!("http{}", u); } }
                ScrapeResult::ok("SearXNG", urls, &text)
            }
        }
    }
}

async fn scrape_brave_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoding::encode(query);
    let url = format!(
        "https://search.brave.com/images?q={}&safesearch=off&source=web",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Cookie", "safesearch=off")
        .send().await;

    match result {
        Err(e) => ScrapeResult::err("Brave", e.to_string()),
        Ok(r) if !r.status().is_success() =>
            ScrapeResult::err("Brave", format!("HTTP {}", r.status())),
        Ok(r) => match r.text().await {
            Err(e) => ScrapeResult::err("Brave", format!("read: {}", e)),
            Ok(html) => {
                let mut urls = Vec::new();
                // Brave proxies all images via imgs.search.brave.com
                // Extract from href="..." and src="..." — use href=" as needle
                // so the candidate starts with "https://" (passes extract_between filter)
                let mut all_hrefs = Vec::new();
                extract_between(&html, "href=\"", "\"", max * 3, &mut all_hrefs);
                extract_between(&html, "src=\"", "\"", max * 3, &mut all_hrefs);
                for u in &all_hrefs {
                    if u.contains("imgs.search.brave.com/") && !urls.contains(u) {
                        urls.push(u.clone());
                    }
                    if urls.len() >= max { break; }
                }
                ScrapeResult::ok("Brave", urls, &html)
            }
        }
    }
}

// ── Download ──────────────────────────────────────────────────────────────────

const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024; // 6 MB

/// Download an image URL, returning (bytes, extension).
/// Streams in chunks, validates magic bytes, enforces size cap.
async fn download_image_bytes(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, &'static str)> {
    use futures_util::StreamExt;

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
            let end = rest.find(|c: char| c == '\'' || c == '"' || c == '&' || c == ' ' || c == '\n')
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

/// Download a list of URLs into `dest_dir`, stopping once `count` succeed.
/// Files are named `<name_hint>_NNN.ext` (name_hint is sanitized).
/// Emits `Downloaded`/`Failed` events; logs results into `log`.
/// Returns the sorted list of successfully downloaded file paths.
pub async fn download_urls_to_dir(
    urls: Vec<String>,
    count: usize,
    dest_dir: &str,
    name_hint: &str,
    log: &mut SessionLog,
    progress: &Option<UnboundedSender<ScrapeEvent>>,
) -> Result<Vec<String>> {
    let emit = |e: ScrapeEvent| { if let Some(tx) = progress { let _ = tx.send(e); } };

    log.push(format!("-- Downloading (target: {}, pool: {}) --", count, urls.len()));
    emit(ScrapeEvent::Phase { label: "Downloading".into() });

    let candidates = filter_candidates(urls);

    let sanitized = sanitize_filename(name_hint);
    let dest_base = dest_dir.trim_end_matches(['\\', '/']).to_string();

    let client = Arc::new(
        reqwest::Client::builder()
            .cookie_store(true)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?
    );

    // 3 concurrent downloads — low enough to not spike RAM
    let sem = Arc::new(Semaphore::new(3));
    let mut tasks = tokio::task::JoinSet::new();

    for (i, url) in candidates.iter().enumerate() {
        let url = url.clone();
        let client = client.clone();
        let sem = sem.clone();
        let sanitized = sanitized.clone();
        let dest_base = dest_base.clone();
        tasks.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            match download_image_bytes(&client, &url).await {
                Ok((bytes, ext)) => {
                    let path = format!("{}\\{}_{:03}.{}", dest_base, sanitized, i + 1, ext);
                    match std::fs::write(&path, &bytes) {
                        Ok(_) => Some((true, url, path, String::new())),
                        Err(e) => Some((false, url, String::new(), format!("write: {}", e))),
                    }
                }
                Err(e) => Some((false, url, String::new(), e.to_string())),
            }
        });
    }

    let mut downloaded: Vec<String> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new(); // (url, reason)

    while let Some(task_result) = tasks.join_next().await {
        if let Ok(Some((ok, url, path, reason))) = task_result {
            if ok {
                debug!("OK  {}", path);
                downloaded.push(path);
                emit(ScrapeEvent::Downloaded { done: downloaded.len(), target: count, path: downloaded.last().cloned().unwrap_or_default() });
                if downloaded.len() >= count {
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
    downloaded.sort();

    // Log download results
    log.push(format!("Downloaded: {}/{}", downloaded.len(), count));
    if !failures.is_empty() {
        log.push(format!("Failed: {} URLs", failures.len()));
        // Group failures by reason for readability
        let mut reason_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (_, reason) in &failures {
            // Normalise reason to a short key (first 60 chars)
            let key = reason.chars().take(60).collect::<String>();
            *reason_counts.entry(key).or_insert(0) += 1;
        }
        for (reason, count) in &reason_counts {
            log.push(format!("  x{} — {}", count, reason));
        }
        // Log up to 10 specific failed URLs for targeted debugging
        log.push("  Sample failed URLs:".to_string());
        for (url, reason) in failures.iter().take(10) {
            log.push(format!("    [{}] {}", reason.chars().take(40).collect::<String>(), url));
        }
    }
    if !downloaded.is_empty() {
        log.push("Files:".to_string());
        for f in &downloaded {
            log.push(format!("  {}", f));
        }
    }

    Ok(downloaded)
}

fn content_type_to_ext(ct: &str) -> Option<&'static str> {
    if ct.contains("jpeg") || ct.contains("jpg") { Some("jpg") }
    else if ct.contains("png")  { Some("png") }
    else if ct.contains("gif")  { Some("gif") }
    else if ct.contains("webp") { Some("webp") }
    else { None }
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

