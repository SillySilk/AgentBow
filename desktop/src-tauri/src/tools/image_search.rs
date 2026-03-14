use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

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

/// Send a local image file to LM Studio's vision model for analysis.
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
    let b64 = base64_encode(&image_bytes);

    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };

    let data_uri = format!("data:{};base64,{}", mime, b64);

    let body = json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": data_uri } }
                ]
            }
        ],
        "max_tokens": 300
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&format!("{}/v1/chat/completions", lm_studio_url))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("LM Studio request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("LM Studio error {}: {}", status, err_body));
    }

    let data: LmStudioResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse LM Studio response: {}", e))?;

    let choice = data.choices.first()
        .ok_or_else(|| anyhow::anyhow!("LM Studio returned no choices"))?;

    let result = choice.message.content.as_deref().unwrap_or("");
    let reasoning = choice.message.reasoning_content.as_deref().unwrap_or("");

    if result.is_empty() && !reasoning.is_empty() {
        Ok(reasoning.to_string())
    } else if !result.is_empty() {
        Ok(result.to_string())
    } else {
        Ok("(no response from vision model)".to_string())
    }
}

/// Download images matching `query` into `dest_dir`, up to `count` files.
/// Scrapes Bing, DuckDuckGo, Yandex, and Qwant with safe search disabled.
pub async fn image_download(query: &str, count: usize, dest_dir: &str) -> Result<String> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create dest_dir '{}': {}", dest_dir, e))?;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

    // Gather candidates from all sources sequentially, stopping when we have enough
    let want = count * 4; // collect a large pool so failed downloads have fallbacks
    let mut candidates: Vec<String> = Vec::new();
    let mut source_report: Vec<String> = Vec::new();

    let sources: &[(&str, &dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>>> + Send>>)] = &[];
    let _ = sources; // unused — we call directly below for lifetime simplicity

    // Bing
    match scrape_bing_images(&client, query, want).await {
        Ok(urls) => {
            info!("Bing: {} URLs", urls.len());
            source_report.push(format!("Bing:{}", urls.len()));
            for u in urls { if !candidates.contains(&u) { candidates.push(u); } }
        }
        Err(e) => {
            warn!("Bing scrape failed: {}", e);
            source_report.push("Bing:0".to_string());
        }
    }

    if candidates.len() < want {
        match scrape_duckduckgo_images(&client, query, want).await {
            Ok(urls) => {
                info!("DDG: {} URLs", urls.len());
                source_report.push(format!("DDG:{}", urls.len()));
                for u in urls { if !candidates.contains(&u) { candidates.push(u); } }
            }
            Err(e) => {
                warn!("DDG scrape failed: {}", e);
                source_report.push("DDG:0".to_string());
            }
        }
    }

    if candidates.len() < want {
        match scrape_yandex_images(&client, query, want).await {
            Ok(urls) => {
                info!("Yandex: {} URLs", urls.len());
                source_report.push(format!("Yandex:{}", urls.len()));
                for u in urls { if !candidates.contains(&u) { candidates.push(u); } }
            }
            Err(e) => {
                warn!("Yandex scrape failed: {}", e);
                source_report.push("Yandex:0".to_string());
            }
        }
    }

    if candidates.len() < want {
        match scrape_qwant_images(&client, query, want).await {
            Ok(urls) => {
                info!("Qwant: {} URLs", urls.len());
                source_report.push(format!("Qwant:{}", urls.len()));
                for u in urls { if !candidates.contains(&u) { candidates.push(u); } }
            }
            Err(e) => {
                warn!("Qwant scrape failed: {}", e);
                source_report.push("Qwant:0".to_string());
            }
        }
    }

    info!("Total candidates: {} ({})", candidates.len(), source_report.join(", "));

    if candidates.is_empty() {
        return Err(anyhow::anyhow!(
            "No images found for '{}'. Sources: {}",
            query,
            source_report.join(", ")
        ));
    }

    let sanitized = sanitize_filename(query);
    let dest_base = dest_dir.trim_end_matches('\\').to_string();

    // Download concurrently (8 at a time), trying all candidates until we reach count
    let sem = Arc::new(Semaphore::new(8));
    let client = Arc::new(client);
    let mut tasks = tokio::task::JoinSet::new();

    for (i, url) in candidates.iter().enumerate() {
        let url = url.clone();
        let client = client.clone();
        let sem = sem.clone();
        let sanitized = sanitized.clone();
        let dest_base = dest_base.clone();
        let idx = i;
        tasks.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let (bytes, ext) = download_image_bytes(&client, &url).await.ok()?;
            let filename = format!("{}_{:03}.{}", sanitized, idx + 1, ext);
            let path = format!("{}\\{}", dest_base, filename);
            std::fs::write(&path, &bytes).ok()?;
            debug!("Saved: {}", path);
            Some(path)
        });
    }

    let mut downloaded: Vec<String> = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(Some(path)) = result {
            downloaded.push(path);
            if downloaded.len() >= count {
                tasks.abort_all();
                break;
            }
        }
    }
    downloaded.sort();

    if downloaded.is_empty() {
        return Err(anyhow::anyhow!(
            "All downloads failed for '{}'. Had {} candidate URLs. Sources: {}",
            query,
            candidates.len(),
            source_report.join(", ")
        ));
    }

    Ok(format!(
        "Downloaded {}/{} images to {}\nSources: {}\nFiles:\n{}",
        downloaded.len(),
        count,
        dest_dir,
        source_report.join(", "),
        downloaded.join("\n")
    ))
}

// ── Scrapers ──────────────────────────────────────────────────────────────────

async fn scrape_bing_images(client: &reqwest::Client, query: &str, max: usize) -> Result<Vec<String>> {
    let encoded = urlencoded(query);
    // adlt=off in BOTH the URL param and cookie to maximally disable safe search
    let url = format!(
        "https://www.bing.com/images/search?q={}&count=50&first=1&safeSearch=Off&adlt=off",
        encoded
    );

    let resp = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.bing.com/")
        // SRCHHPGUSR cookie with ADLT=OFF disables adult content filtering
        // SUID+MSCC suppress consent/region redirects
        .header("Cookie", "SRCHHPGUSR=ADLT=OFF; adlt=off; SUID=M; MSCC=NR; _EDGE_S=ui=en-us; _EDGE_V=1")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Bing request failed: {}", e))?;

    let html = resp.text().await
        .map_err(|e| anyhow::anyhow!("Failed to read Bing response: {}", e))?;

    let mut urls: Vec<String> = Vec::new();
    // Bing encodes image URLs as HTML entities in data-m attributes
    extract_between(&html, "&quot;murl&quot;:&quot;", "&quot;", max, &mut urls);
    // Fallback: plain JSON inside script blocks
    if urls.is_empty() {
        extract_between(&html, "\"murl\":\"", "\"", max, &mut urls);
    }

    Ok(urls)
}

async fn scrape_duckduckgo_images(client: &reqwest::Client, query: &str, max: usize) -> Result<Vec<String>> {
    let encoded = urlencoded(query);
    // kp=-2 disables safe search on DDG
    let page_url = format!("https://duckduckgo.com/?q={}&iax=images&ia=images&kp=-2", encoded);

    let html = client
        .get(&page_url)
        .header("Accept-Language", "en-US,en;q=0.9")
        // kp=-2 cookie is the correct cookie name for DDG safe search preference
        .header("Cookie", "kp=-2; ay=b")
        .send().await
        .map_err(|e| anyhow::anyhow!("DDG page request failed: {}", e))?
        .text().await
        .map_err(|e| anyhow::anyhow!("DDG page read failed: {}", e))?;

    // Extract VQD token — try both single and double quoted forms
    let vqd = extract_vqd(&html)
        .ok_or_else(|| anyhow::anyhow!("DDG: vqd token not found in page"))?;

    debug!("DDG vqd: {}", vqd);

    // i.js API — p=-2 is the API param for safe search off
    let api_url = format!(
        "https://duckduckgo.com/i.js?q={}&vqd={}&o=json&l=us-en&s=0&f=,,,,,&p=-2",
        encoded, vqd
    );

    let resp = client
        .get(&api_url)
        .header("Referer", "https://duckduckgo.com/")
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("Cookie", "kp=-2; ay=b")
        .send().await
        .map_err(|e| anyhow::anyhow!("DDG i.js request failed: {}", e))?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("DDG JSON parse failed: {}", e))?;

    let mut urls = Vec::new();
    if let Some(results) = data["results"].as_array() {
        for r in results {
            if urls.len() >= max { break; }
            if let Some(url) = r["image"].as_str() {
                if url.starts_with("http") {
                    urls.push(url.to_string());
                }
            }
        }
    }
    Ok(urls)
}

async fn scrape_yandex_images(client: &reqwest::Client, query: &str, max: usize) -> Result<Vec<String>> {
    let encoded = urlencoded(query);
    // filter=0 disables safe search; numdoc=50 requests more results
    let url = format!(
        "https://yandex.com/images/search?text={}&nomisspell=1&numdoc=50&filter=0&itype=photo",
        encoded
    );

    let html = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Referer", "https://yandex.com/")
        // safesearch=0 disables filtering; yp cookie value encodes safe search preference
        .header("Cookie", "safesearch=0; yp=1999999999.sp.ssp%3D0; _yasc=off")
        .send().await
        .map_err(|e| anyhow::anyhow!("Yandex request failed: {}", e))?
        .text().await
        .map_err(|e| anyhow::anyhow!("Failed to read Yandex response: {}", e))?;

    let mut urls = Vec::new();
    // Primary: Yandex embeds original image URLs as "img_href":"https://..."
    extract_between(&html, "\"img_href\":\"", "\"", max, &mut urls);
    // Secondary: "url":"https://..." pattern in JSON blobs
    if urls.is_empty() {
        extract_between(&html, "\"url\":\"https://", "\"", max, &mut urls);
        for u in urls.iter_mut() {
            if !u.starts_with("http") { *u = format!("https://{}", u); }
        }
    }
    Ok(urls)
}

async fn scrape_qwant_images(client: &reqwest::Client, query: &str, max: usize) -> Result<Vec<String>> {
    let encoded = urlencoded(query);
    // safesearch=0 disables filtering; tgp=2 = general (not kids)
    let url = format!(
        "https://api.qwant.com/v3/search/images?q={}&count=50&offset=0&safesearch=0&locale=en_US&tgp=2",
        encoded
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Referer", "https://www.qwant.com/")
        .send().await
        .map_err(|e| anyhow::anyhow!("Qwant request failed: {}", e))?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("Qwant JSON parse failed: {}", e))?;

    let mut urls = Vec::new();
    if let Some(items) = data["data"]["result"]["items"].as_array() {
        for item in items {
            if urls.len() >= max { break; }
            if let Some(url) = item["media"].as_str() {
                if url.starts_with("http") {
                    urls.push(url.to_string());
                }
            }
        }
    }
    Ok(urls)
}

// ── Download ──────────────────────────────────────────────────────────────────

/// Download an image URL, returning (bytes, extension).
/// Uses Content-Type header for extension; validates magic bytes.
async fn download_image_bytes(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, &'static str)> {
    let resp = client
        .get(url)
        .header("Referer", "https://www.google.com/")
        .header("Accept", "image/webp,image/apng,image/*,*/*;q=0.8")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("HTTP {}", resp.status()));
    }

    // Determine extension from Content-Type before consuming body
    let ct_ext = resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| content_type_to_ext(ct));

    let bytes = resp.bytes().await
        .map_err(|e| anyhow::anyhow!("Failed to read bytes: {}", e))?
        .to_vec();

    // Validate magic bytes to confirm it's a real image
    let ext = validate_image_bytes(&bytes, ct_ext, url)?;

    Ok((bytes, ext))
}

/// Check magic bytes and return the correct extension, or error if not a real image.
fn validate_image_bytes(bytes: &[u8], ct_ext: Option<&'static str>, url: &str) -> Result<&'static str> {
    if bytes.len() < 512 {
        return Err(anyhow::anyhow!("Too small ({} bytes): {}", bytes.len(), url));
    }

    // Check magic bytes
    let ext = if bytes.starts_with(b"\xFF\xD8\xFF") {
        "jpg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        // If magic bytes don't match a known format, reject (catches HTML error pages, etc.)
        return Err(anyhow::anyhow!(
            "Not a recognised image (magic: {:02X?}): {}",
            &bytes[..bytes.len().min(4)],
            url
        ));
    };

    // Prefer Content-Type ext if it agrees; otherwise trust magic bytes
    let _ = ct_ext;
    Ok(ext)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Percent-encode a query string for use in URLs (spaces → %20, etc.)
fn urlencoded(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c]
            } else if c == ' ' {
                vec!['%', '2', '0']
            } else {
                let b = c as u32;
                format!("%{:02X}", b).chars().collect()
            }
        })
        .collect()
}

/// Extract substrings between `needle` and `end_marker`.
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

/// Extract DuckDuckGo VQD token from page HTML.
fn extract_vqd(html: &str) -> Option<String> {
    // Try both quoted and unquoted forms: vqd='...' vqd="..." vqd=...&
    for needle in &["vqd='", "vqd=\"", "vqd="] {
        if let Some(pos) = html.find(needle) {
            let rest = &html[pos + needle.len()..];
            let end = rest.find(|c: char| c == '\'' || c == '"' || c == '&' || c == ' ' || c == '\n')
                .unwrap_or_else(|| rest.len().min(80));
            let token = rest[..end].trim_matches(|c| c == '\'' || c == '"').to_string();
            if !token.is_empty() && token.len() > 3 {
                return Some(token);
            }
        }
    }
    None
}

fn content_type_to_ext(ct: &str) -> Option<&'static str> {
    if ct.contains("jpeg") || ct.contains("jpg") { Some("jpg") }
    else if ct.contains("png") { Some("png") }
    else if ct.contains("gif") { Some("gif") }
    else if ct.contains("webp") { Some("webp") }
    else { None }
}

fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    sanitized.trim_matches('_').to_string()
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
