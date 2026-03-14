use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

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
/// Returns the model's description of the image.
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

    // Read and base64-encode the image
    let image_bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Failed to read image '{}': {}", image_path, e))?;
    let b64 = base64_encode(&image_bytes);

    // Detect mime type from extension
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

    // Use content if present, fall back to reasoning_content
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
/// Scrapes Bing Images for URLs, then downloads each one.
pub async fn image_download(query: &str, count: usize, dest_dir: &str) -> Result<String> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create dest_dir '{}': {}", dest_dir, e))?;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

    // Try sources in order until we have enough candidates
    let want = count * 3;
    let mut candidates = scrape_bing_images(&client, query, want).await.unwrap_or_default();

    if candidates.len() < count {
        if let Ok(ddg) = scrape_duckduckgo_images(&client, query, want).await {
            for u in ddg {
                if !candidates.contains(&u) { candidates.push(u); }
            }
        }
    }

    if candidates.is_empty() {
        return Err(anyhow::anyhow!("No images found for '{}' from any source", query));
    }

    let sanitized = sanitize_filename(query);
    let dest_base = dest_dir.trim_end_matches('\\').to_string();

    // Download up to `count` images concurrently (8 at a time)
    let sem = Arc::new(Semaphore::new(8));
    let client = Arc::new(client);
    let mut tasks = tokio::task::JoinSet::new();

    for (i, url) in candidates.iter().enumerate().take(count * 2) {
        let ext = url_to_ext(url);
        let filename = format!("{}_{:03}.{}", sanitized, i + 1, ext);
        let path = format!("{}\\{}", dest_base, filename);
        let url = url.clone();
        let client = client.clone();
        let sem = sem.clone();
        tasks.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            download_image_url(&client, &url, &path).await.ok()?;
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
        return Err(anyhow::anyhow!("All downloads failed for '{}'", query));
    }

    let files_list = downloaded.join("\n");
    Ok(format!(
        "Downloaded {}/{} images to {}\nFiles:\n{}",
        downloaded.len(),
        count,
        dest_dir,
        files_list
    ))
}

async fn scrape_bing_images(
    client: &reqwest::Client,
    query: &str,
    max: usize,
) -> Result<Vec<String>> {
    let encoded = query.replace(' ', "+");
    let url = format!(
        "https://www.bing.com/images/search?q={}&count=50&first=1",
        encoded
    );

    let resp = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Bing Images request failed: {}", e))?;

    let html = resp
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read Bing response: {}", e))?;

    let mut urls: Vec<String> = Vec::new();

    // Bing encodes image metadata in data-m attributes with HTML-entity quotes:
    // data-m="{&quot;murl&quot;:&quot;https://...&quot;,...}"
    extract_between(&html, "&quot;murl&quot;:&quot;", "&quot;", max, &mut urls);

    // Fallback: plain JSON inside <script> blocks
    if urls.is_empty() {
        extract_between(&html, "\"murl\":\"", "\"", max, &mut urls);
    }

    Ok(urls)
}

/// Extract substrings between `needle` and `end_marker` from `haystack`.
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
                        if candidate.starts_with("http") && !candidate.contains(' ') {
                            out.push(candidate.to_string());
                        }
                        pos = start + end_rel + end_marker.len();
                    }
                }
            }
        }
    }
}

/// Scrape DuckDuckGo Images (unofficial i.js API — no API key needed).
async fn scrape_duckduckgo_images(
    client: &reqwest::Client,
    query: &str,
    max: usize,
) -> Result<Vec<String>> {
    // Step 1: get VQD token from the search page
    let encoded = query.replace(' ', "+");
    let page_url = format!("https://duckduckgo.com/?q={}&iax=images&ia=images", encoded);
    let html = client
        .get(&page_url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send().await?.text().await?;

    // VQD looks like: vqd=4-XXXXXXXXXX or vqd=4-XXXX-XXXX
    let vqd = html
        .find("vqd=")
        .and_then(|pos| {
            let rest = &html[pos + 4..];
            let end = rest.find(|c: char| c == '&' || c == '"' || c == '\'').unwrap_or(rest.len().min(64));
            Some(rest[..end].trim_matches('\'').trim_matches('"').to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("DDG: vqd token not found"))?;

    // Step 2: fetch image JSON
    let api_url = format!(
        "https://duckduckgo.com/i.js?q={}&vqd={}&o=json&l=us-en&s=0&f=,,,,,&p=1",
        encoded, vqd
    );
    let resp = client
        .get(&api_url)
        .header("Referer", "https://duckduckgo.com/")
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .send().await?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let data: serde_json::Value = resp.json().await?;
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

async fn download_image_url(
    client: &reqwest::Client,
    url: &str,
    path: &str,
) -> Result<()> {
    let resp = client
        .get(url)
        .header("Referer", "https://www.bing.com/")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Image download request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("HTTP {} for {}", resp.status(), url));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read image bytes: {}", e))?;

    if bytes.len() < 1000 {
        return Err(anyhow::anyhow!(
            "Response too small ({} bytes), likely not a real image",
            bytes.len()
        ));
    }

    std::fs::write(path, &bytes)
        .map_err(|e| anyhow::anyhow!("Failed to write image to '{}': {}", path, e))?;

    Ok(())
}

fn url_to_ext(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    if lower.contains(".png") {
        "png"
    } else if lower.contains(".webp") {
        "webp"
    } else if lower.contains(".gif") {
        "gif"
    } else {
        "jpg"
    }
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
