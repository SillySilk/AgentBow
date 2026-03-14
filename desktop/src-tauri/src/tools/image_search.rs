use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::io::Write as IoWrite;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

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
            // Grab first 600 chars of raw response as a scraping-failure hint
            Some(raw.chars().take(600).collect::<String>().replace('\n', " "))
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
            format!("  {:8} 0 URLs — hint: {:.120}", self.source, hint)
        } else {
            format!("  {:8} {} URLs", self.source, self.urls.len())
        }
    }
}

// ── Session log ───────────────────────────────────────────────────────────────

struct SessionLog {
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

    let image_bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Failed to read image '{}': {}", image_path, e))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if ext == "webp" {
        return Ok(format!(
            "[image_verify skipped: {} is WebP — not supported by most local vision models. \
             Re-download as JPEG/PNG if you need to verify this image.]",
            image_path
        ));
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

    let b64 = base64_encode(&image_bytes);
    let mime = match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        _ => "image/jpeg",
    };
    let data_uri = format!("data:{};base64,{}", mime, b64);

    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": [
            { "type": "text", "text": prompt },
            { "type": "image_url", "image_url": { "url": data_uri } }
        ]}],
        "max_tokens": 300
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

// ── image_download ────────────────────────────────────────────────────────────

/// Download images matching `query` into `dest_dir`, up to `count` files.
/// Writes a session log to `{log_dir}\\bow_downloads.log`.
pub async fn image_download(query: &str, count: usize, dest_dir: &str, log_dir: &str) -> Result<String> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create dest_dir '{}': {}", dest_dir, e))?;

    let mut log = SessionLog::new(log_dir, query);
    log.push(format!("dest_dir: {}", dest_dir));

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

    let want = count * 4;
    let mut candidates: Vec<String> = Vec::new();

    log.push("-- Scraping sources --".to_string());

    // Run all scrapers; always run all of them so the log captures every source
    let results: Vec<ScrapeResult> = vec![
        scrape_bing_images(&client, query, want).await,
        scrape_duckduckgo_images(&client, query, want).await,
        scrape_yandex_images(&client, query, want).await,
        scrape_qwant_images(&client, query, want).await,
        scrape_searxng_images(&client, query, want).await,
    ];

    for r in &results {
        log.push(r.log_line());
        for u in &r.urls {
            if !candidates.contains(u) { candidates.push(u.clone()); }
        }
    }
    log.push(format!("Total candidates: {}", candidates.len()));

    if candidates.is_empty() {
        log.push("FATAL: no candidates — all scrapers returned 0 URLs".to_string());
        let log_note = log.flush();
        return Err(anyhow::anyhow!(
            "No images found for {:?}. {}", query, log_note
        ));
    }

    // ── Download phase ────────────────────────────────────────────────────────
    log.push(format!("-- Downloading (target: {}, pool: {}) --", count, candidates.len()));

    let sanitized = sanitize_filename(query);
    let dest_base = dest_dir.trim_end_matches(['\\', '/']).to_string();

    // 3 concurrent downloads — low enough to not spike RAM
    let sem = Arc::new(Semaphore::new(3));
    let client = Arc::new(client);
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
                if downloaded.len() >= count {
                    tasks.abort_all();
                    break;
                }
            } else {
                debug!("FAIL {} — {}", url, reason);
                failures.push((url, reason));
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

    let log_note = log.flush();

    if downloaded.is_empty() {
        return Err(anyhow::anyhow!(
            "All downloads failed for {:?}. {}", query, log_note
        ));
    }

    Ok(format!(
        "Downloaded {}/{} images to {}\n{}\nFiles:\n{}",
        downloaded.len(), count, dest_dir, log_note,
        downloaded.join("\n")
    ))
}

// ── Scrapers ──────────────────────────────────────────────────────────────────

async fn scrape_bing_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoded(query);
    let url = format!(
        "https://www.bing.com/images/search?q={}&count=50&first=1&safeSearch=Off&adlt=off",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.bing.com/")
        .header("Cookie", "SRCHHPGUSR=ADLT=OFF; adlt=off; SUID=M; MSCC=NR; _EDGE_S=ui=en-us")
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
    let encoded = urlencoded(query);
    let page_url = format!("https://duckduckgo.com/?q={}&iax=images&ia=images&kp=-2", encoded);

    let html = match client.get(&page_url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Cookie", "kp=-2; ay=b")
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
    let resp = match client.get(&api_url)
        .header("Referer", "https://duckduckgo.com/")
        .header("Accept", "application/json, */*; q=0.01")
        .header("Cookie", "kp=-2; ay=b")
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
    let encoded = urlencoded(query);
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
                extract_between(&html, "\"img_href\":\"", "\"", max, &mut urls);
                if urls.is_empty() {
                    extract_between(&html, "\"url\":\"https://", "\"", max, &mut urls);
                    for u in urls.iter_mut() {
                        if !u.starts_with("http") { *u = format!("https://{}", u); }
                    }
                }
                ScrapeResult::ok("Yandex", urls, &html)
            }
        }
    }
}

async fn scrape_qwant_images(client: &reqwest::Client, query: &str, max: usize) -> ScrapeResult {
    let encoded = urlencoded(query);
    let url = format!(
        "https://api.qwant.com/v3/search/images?q={}&count=50&offset=0&safesearch=0&locale=en_US&tgp=2",
        encoded
    );
    let result = client.get(&url)
        .header("Accept", "application/json")
        .header("Referer", "https://www.qwant.com/")
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
    let encoded = urlencoded(query);
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
        Ok(r) => match r.json::<serde_json::Value>().await {
            Err(e) => ScrapeResult::err("SearXNG", format!("json: {}", e)),
            Ok(data) => {
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
                let raw = data.to_string();
                ScrapeResult::ok("SearXNG", urls, &raw)
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

    let resp = client.get(url)
        .header("Referer", "https://www.google.com/")
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

fn urlencoded(s: &str) -> String {
    s.chars().flat_map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            vec![c]
        } else if c == ' ' {
            vec!['%', '2', '0']
        } else {
            format!("%{:02X}", c as u32).chars().collect()
        }
    }).collect()
}

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

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut r = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let t = (b0 << 16) | (b1 << 8) | b2;
        r.push(CHARS[((t >> 18) & 0x3F) as usize] as char);
        r.push(CHARS[((t >> 12) & 0x3F) as usize] as char);
        r.push(if chunk.len() > 1 { CHARS[((t >> 6) & 0x3F) as usize] as char } else { '=' });
        r.push(if chunk.len() > 2 { CHARS[(t & 0x3F) as usize] as char } else { '=' });
    }
    r
}
