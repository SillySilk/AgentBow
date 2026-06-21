use std::path::PathBuf;

pub fn chrome_executable() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists())
}

use anyhow::{anyhow, Result};
use base64::Engine as _;
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, DeleteCookiesParams};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

struct BrowserState {
    // Held alive to keep the browser process running; methods are only needed
    // during launch (ensure_launched) where it is used directly before storage.
    #[allow(dead_code)]
    browser: Browser,
    page: Page,
    _handler: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub struct ControlledBrowser {
    inner: Arc<Mutex<Option<BrowserState>>>,
    profile_dir: PathBuf,
}

impl ControlledBrowser {
    pub fn new(profile_dir: PathBuf) -> Self {
        ControlledBrowser {
            inner: Arc::new(Mutex::new(None)),
            profile_dir,
        }
    }

    #[cfg(test)]
    pub async fn is_running(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    /// Launch Chrome with the persistent profile if not already running.
    pub async fn ensure_launched(&self, headless: bool) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let exe = chrome_executable().ok_or_else(|| {
            anyhow!("No Chrome/Edge found. Set CHROME_PATH in .env to the chrome.exe path.")
        })?;
        std::fs::create_dir_all(&self.profile_dir).ok();

        let mut builder = BrowserConfig::builder()
            .chrome_executable(exe)
            .user_data_dir(self.profile_dir.clone());
        if !headless {
            builder = builder.with_head();
        }
        let cfg = builder.build().map_err(|e| anyhow!("BrowserConfig: {}", e))?;

        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .map_err(|e| anyhow!("Chrome launch failed: {}", e))?;
        // The handler stream MUST be polled for the browser to function.
        let handler_task = tokio::spawn(async move { while (handler.next().await).is_some() {} });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| anyhow!("new_page: {}", e))?;

        *guard = Some(BrowserState {
            browser,
            page,
            _handler: handler_task,
        });
        Ok(())
    }

    /// Internal: run a closure with the current page, erroring if not launched.
    async fn with_page<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(Page) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let guard = self.inner.lock().await;
        let st = guard
            .as_ref()
            .ok_or_else(|| anyhow!("Browser not launched — call browser_open first"))?;
        let page = st.page.clone();
        drop(guard);
        f(page).await
    }

    /// Navigate to `url`, waiting for load. Returns `"Navigated to <final url>"`.
    pub async fn navigate(&self, url: &str) -> Result<Value> {
        self.ensure_launched(false).await?;
        let u = url.to_string();
        self.with_page(|page| async move {
            page.goto(&u).await.map_err(|e| anyhow!("goto: {}", e))?;
            page.wait_for_navigation().await.ok();
            let final_url = page.url().await.ok().flatten().unwrap_or(u);
            Ok(json!(format!("Navigated to {}", final_url)))
        })
        .await
    }

    /// Return the current page's `{ url, title }`.
    pub async fn get_url(&self) -> Result<Value> {
        self.with_page(|page| async move {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url, "title": title }))
        })
        .await
    }

    /// Read the current page in `mode` ("text" | "html" | "links").
    /// HTML is distilled; text is distilled + truncated; links is a JSON array.
    pub async fn read_page(&self, mode: &str) -> Result<Value> {
        let mode = mode.to_string();
        self.with_page(|page| async move {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            let html = page.content().await.map_err(|e| anyhow!("content: {}", e))?;
            let content = match mode.as_str() {
                "html" => crate::util::distill_html(&html),
                "links" => {
                    let expr = "JSON.stringify(Array.from(document.querySelectorAll('a[href]')).map(a=>({text:a.innerText.trim().slice(0,100),href:a.href})).filter(l=>l.text&&l.href))".to_string();
                    let v: Value = page
                        .evaluate(expr)
                        .await
                        .ok()
                        .and_then(|r| r.into_value().ok())
                        .unwrap_or(Value::Null);
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                }
                _ => crate::util::truncate_text(&crate::util::distill_html(&html), 8000),
            };
            Ok(json!({ "url": url, "title": title, "content": content }))
        })
        .await
    }

    /// Scroll the page: "top", "bottom", "up", "down", or a CSS selector.
    pub async fn scroll(&self, target: &str, pixels: i64) -> Result<Value> {
        let target_label = target.to_string();
        let js = match target {
            "top" => "window.scrollTo(0,0)".to_string(),
            "bottom" => "window.scrollTo(0,document.body.scrollHeight)".to_string(),
            "up" => format!("window.scrollBy(0,-{})", pixels),
            "down" => format!("window.scrollBy(0,{})", pixels),
            sel => format!(
                "document.querySelector({:?})?.scrollIntoView({{behavior:'smooth',block:'center'}})",
                sel
            ),
        };
        self.with_page(|page| async move {
            page.evaluate(js).await.map_err(|e| anyhow!("scroll: {}", e))?;
            Ok(json!(format!("Scrolled: {}", target_label)))
        })
        .await
    }

    /// Collect absolute, image-looking URLs from the current page
    /// (`img[src]`/`currentSrc`, `img[srcset]`, and `a[href]`).
    pub async fn extract_image_urls(&self) -> Result<Vec<String>> {
        self.ensure_launched(false).await?;
        self.with_page(|page| async move {
            let base = page.url().await.ok().flatten().unwrap_or_default();
            let expr = r#"
                JSON.stringify((() => {
                  const out = [];
                  document.querySelectorAll('img').forEach(im => {
                    if (im.currentSrc) out.push(im.currentSrc);
                    else if (im.src) out.push(im.src);
                    if (im.srcset) im.srcset.split(',').forEach(s => out.push(s.trim().split(' ')[0]));
                  });
                  document.querySelectorAll('a[href]').forEach(a => out.push(a.href));
                  return out;
                })())
            "#
            .to_string();
            let raw: Value = page
                .evaluate(expr)
                .await
                .ok()
                .and_then(|r| r.into_value().ok())
                .unwrap_or(Value::Null);
            let list: Vec<String> = raw
                .as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .or_else(|| serde_json::from_value(raw.clone()).ok())
                .unwrap_or_default();
            Ok(normalize_image_urls(list, &base))
        })
        .await
    }

    /// Click the first element matching `selector`.
    pub async fn click(&self, selector: &str) -> Result<Value> {
        let sel = selector.to_string();
        self.with_page(|page| async move {
            let el = page
                .find_element(&sel)
                .await
                .map_err(|_| anyhow!("Element not found: {}", sel))?;
            el.click().await.map_err(|e| anyhow!("click: {}", e))?;
            Ok(json!(format!("Clicked: {}", sel)))
        })
        .await
    }

    /// Focus the first element matching `selector`, type `value`, and optionally
    /// press Enter to submit.
    pub async fn fill(&self, selector: &str, value: &str, submit: bool) -> Result<Value> {
        let (sel, val) = (selector.to_string(), value.to_string());
        self.with_page(|page| async move {
            let el = page
                .find_element(&sel)
                .await
                .map_err(|_| anyhow!("Element not found: {}", sel))?;
            el.click().await.ok();
            el.type_str(&val)
                .await
                .map_err(|e| anyhow!("type: {}", e))?;
            if submit {
                el.press_key("Enter").await.ok();
            }
            Ok(json!(format!("Filled: {}", sel)))
        })
        .await
    }

    /// Evaluate arbitrary JavaScript in the page and return its JSON result.
    pub async fn exec_js(&self, js: &str) -> Result<Value> {
        let js = js.to_string();
        self.with_page(|page| async move {
            let r = page.evaluate(js).await.map_err(|e| anyhow!("eval: {}", e))?;
            Ok(r.into_value::<Value>().unwrap_or(Value::Null))
        })
        .await
    }

    /// Capture a PNG screenshot of the current page. Returns the same
    /// image+text content-array shape the old BrowserBridge produced so it can
    /// be embedded directly into a `tool_result` content field.
    pub async fn screenshot(&self) -> Result<Value> {
        self.with_page(|page| async move {
            let bytes = page
                .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
                .await
                .map_err(|e| anyhow!("screenshot: {}", e))?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(json!([
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64
                    }
                },
                {
                    "type": "text",
                    "text": "Screenshot of current browser tab."
                }
            ]))
        })
        .await
    }

    /// Navigate back in history. chromiumoxide 0.9.1 has no native history API,
    /// so this is driven via `history.back()` (see report for the adaptation).
    pub async fn back(&self) -> Result<Value> {
        self.with_page(|page| async move {
            page.evaluate("history.back()")
                .await
                .map_err(|e| anyhow!("back: {}", e))?;
            page.wait_for_navigation().await.ok();
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url, "title": title }))
        })
        .await
    }

    /// Navigate forward in history (driven via `history.forward()`).
    pub async fn forward(&self) -> Result<Value> {
        self.with_page(|page| async move {
            page.evaluate("history.forward()")
                .await
                .map_err(|e| anyhow!("forward: {}", e))?;
            page.wait_for_navigation().await.ok();
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url, "title": title }))
        })
        .await
    }

    /// Reload the current page. chromiumoxide 0.9.1's `reload()` takes no
    /// cache-bypass flag, so `_bypass` is accepted for signature parity only.
    pub async fn reload(&self, _bypass: bool) -> Result<Value> {
        self.with_page(|page| async move {
            page.reload().await.map_err(|e| anyhow!("reload: {}", e))?;
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            Ok(json!({ "url": url, "title": title }))
        })
        .await
    }

    /// Return cookies for the current page as a JSON array. chromiumoxide's
    /// `get_cookies` is scoped to the tab's current URL, so `_url` is accepted
    /// for signature parity only.
    pub async fn get_cookies(&self, _url: &str) -> Result<Value> {
        self.with_page(|page| async move {
            let cookies = page
                .get_cookies()
                .await
                .map_err(|e| anyhow!("get_cookies: {}", e))?;
            Ok(serde_json::to_value(cookies).unwrap_or(Value::Null))
        })
        .await
    }

    /// Set a single cookie from a JSON object matching CDP's `CookieParam`
    /// (`{ name, value, url?, domain?, path?, ... }`).
    pub async fn set_cookie(&self, params: &Value) -> Result<Value> {
        let cookie: CookieParam = serde_json::from_value(params.clone())
            .map_err(|e| anyhow!("set_cookie: invalid cookie params: {}", e))?;
        self.with_page(|page| async move {
            page.set_cookie(cookie)
                .await
                .map_err(|e| anyhow!("set_cookie: {}", e))?;
            Ok(json!("Cookie set"))
        })
        .await
    }

    /// Delete cookies for `url`. If `name` is given, only that cookie is
    /// removed; otherwise every cookie scoped to the current page is removed.
    pub async fn delete_cookies(&self, url: &str, name: Option<&str>) -> Result<Value> {
        let url = url.to_string();
        let name = name.map(|s| s.to_string());
        self.with_page(|page| async move {
            let targets: Vec<DeleteCookiesParams> = match name {
                Some(n) => vec![DeleteCookiesParams::builder()
                    .name(n)
                    .url(url)
                    .build()
                    .map_err(|e| anyhow!("delete_cookies: {}", e))?],
                None => {
                    let existing = page
                        .get_cookies()
                        .await
                        .map_err(|e| anyhow!("delete_cookies: {}", e))?;
                    existing
                        .into_iter()
                        .map(|c| {
                            DeleteCookiesParams::builder()
                                .name(c.name)
                                .url(url.clone())
                                .build()
                                .map_err(|e| anyhow!("delete_cookies: {}", e))
                        })
                        .collect::<Result<Vec<_>>>()?
                }
            };
            let count = targets.len();
            if !targets.is_empty() {
                page.delete_cookies(targets)
                    .await
                    .map_err(|e| anyhow!("delete_cookies: {}", e))?;
            }
            Ok(json!(format!("Deleted {} cookie(s)", count)))
        })
        .await
    }

    /// Capture a screenshot + distilled page text in one call (same shape the
    /// old BrowserBridge returned).
    pub async fn analyze_page(&self) -> Result<Value> {
        let (shot, page) = tokio::join!(self.screenshot(), self.read_page("text"));
        let b64 = shot.ok().and_then(|v| {
            v.as_array()?
                .first()?
                .get("source")?
                .get("data")?
                .as_str()
                .map(|s| s.to_string())
        });
        let (url, title, text): (String, String, String) = match page {
            Ok(v) => (
                v["url"].as_str().unwrap_or("").to_string(),
                v["title"].as_str().unwrap_or("").to_string(),
                v["content"].as_str().unwrap_or("").to_string(),
            ),
            Err(_) => (String::new(), String::new(), String::new()),
        };
        let mut out = json!({ "url": url, "title": title, "text_content": text });
        if let Some(b) = b64 {
            out["screenshot_base64"] = json!(b);
            out["screenshot_note"] =
                json!("PNG screenshot available if your model supports vision.");
        }
        Ok(out)
    }
}

const IMG_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "avif",
];

/// Resolve relative URLs against `base`, drop `data:`/non-image URLs, and
/// dedupe — keeping only `http(s)` image-looking URLs. Pure & unit-tested.
pub fn normalize_image_urls(raw: Vec<String>, base: &str) -> Vec<String> {
    let base_url = Url::parse(base).ok();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let r = r.trim();
        if r.is_empty() || r.starts_with("data:") {
            continue;
        }
        let abs = if r.starts_with("http") {
            r.to_string()
        } else if let Some(b) = &base_url {
            match b.join(r) {
                Ok(u) => u.to_string(),
                Err(_) => continue,
            }
        } else {
            continue;
        };
        let lower = abs.split('?').next().unwrap_or(&abs).to_lowercase();
        let looks_img = IMG_EXTS.iter().any(|e| lower.ends_with(&format!(".{}", e)));
        if !looks_img {
            continue;
        }
        if seen.insert(abs.clone()) {
            out.push(abs);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chrome_executable_honors_env_override() {
        // Point CHROME_PATH at a file we know exists (this test binary itself).
        let me = std::env::current_exe().unwrap();
        std::env::set_var("CHROME_PATH", &me);
        assert_eq!(chrome_executable(), Some(me));
        std::env::remove_var("CHROME_PATH");
    }

    #[test]
    fn normalize_resolves_dedupes_and_filters() {
        let raw = vec![
            "https://e.com/a.jpg".to_string(),
            "https://e.com/a.jpg".to_string(),        // dup
            "/img/b.png".to_string(),                 // relative
            "data:image/png;base64,xxxx".to_string(), // data URI dropped
            "https://e.com/script.js".to_string(),    // non-image dropped
        ];
        let out = normalize_image_urls(raw, "https://e.com/gallery/");
        assert_eq!(
            out,
            vec![
                "https://e.com/a.jpg".to_string(),
                "https://e.com/img/b.png".to_string(),
            ]
        );
    }

    #[tokio::test]
    #[ignore = "requires a real Chrome install; run manually with --ignored"]
    async fn launches_and_navigates_live() {
        let dir = std::env::temp_dir().join("bow_cb_live");
        let cb = ControlledBrowser::new(dir);
        cb.ensure_launched(true).await.expect("launch");
        assert!(cb.is_running().await);
    }
}
