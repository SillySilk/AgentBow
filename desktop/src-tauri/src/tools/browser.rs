use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

/// Sends browser commands to the Chrome extension over the existing WebSocket
/// connection and awaits their results via a pending-request map.
///
/// # Example
/// ```ignore
/// let bridge = BrowserBridge::new(out_tx.clone());
/// let screenshot = bridge.screenshot().await?;
/// ```
pub struct BrowserBridge {
    /// Channel to write outbound JSON text frames to the WS sink task.
    pub ws_out: tokio::sync::mpsc::Sender<String>,
    /// Pending one-shot senders keyed by request_id. Shared across clones.
    pub pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
}

impl Clone for BrowserBridge {
    fn clone(&self) -> Self {
        BrowserBridge {
            ws_out: self.ws_out.clone(),
            pending: Arc::clone(&self.pending),
        }
    }
}

impl BrowserBridge {
    /// Create a new bridge tied to `ws_out`.
    pub fn new(ws_out: tokio::sync::mpsc::Sender<String>) -> Self {
        BrowserBridge {
            ws_out,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return a clone that shares the same pending map (cheap; just clones the
    /// `Arc` and the mpsc `Sender`).
    #[allow(dead_code)]
    pub fn clone_handle(&self) -> Self {
        self.clone()
    }

    /// Send `cmd` (after injecting a unique `request_id`) over the WebSocket
    /// and block until the extension replies or a 30-second timeout fires.
    pub async fn send_and_wait(&self, mut cmd: Value) -> Result<Value> {
        let request_id = Uuid::new_v4().to_string();
        cmd["request_id"] = json!(request_id);

        let (tx, rx) = oneshot::channel::<Value>();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        let text = serde_json::to_string(&cmd)
            .map_err(|e| anyhow!("Failed to serialise browser command: {}", e))?;

        self.ws_out
            .send(text)
            .await
            .map_err(|_| anyhow!("WebSocket send channel closed"))?;

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                // Clean up the dangling sender on timeout.
                let pending = self.pending.clone();
                let rid = request_id.clone();
                tokio::spawn(async move {
                    pending.lock().await.remove(&rid);
                });
                anyhow!("Browser command timed out after 30 s")
            })?
            .map_err(|_| anyhow!("Browser result channel closed unexpectedly"))?;

        // Propagate errors reported by the extension.
        if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
            return Err(anyhow!("Browser error: {}", err));
        }

        Ok(result)
    }

    /// Capture a screenshot of the current browser tab.
    ///
    /// Returns an Anthropic-style content array with an `image` block followed
    /// by a descriptive `text` block so it can be embedded directly in a
    /// `tool_result` content field.
    pub async fn screenshot(&self) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "screenshot" });
        let result = self.send_and_wait(cmd).await?;

        let data_url = result["data"]
            .as_str()
            .ok_or_else(|| anyhow!("screenshot: missing 'data' field in response"))?;

        // Strip the data URI prefix ("data:image/png;base64,").
        let base64_data = data_url
            .split_once(',')
            .map(|(_, b)| b)
            .unwrap_or(data_url);

        Ok(json!([
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": base64_data
                }
            },
            {
                "type": "text",
                "text": "Screenshot of current browser tab."
            }
        ]))
    }

    /// Execute JavaScript in the active browser tab and return the result as a
    /// JSON string value.
    pub async fn exec_js(&self, js: &str) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "exec_js", "js": js });
        let result = self.send_and_wait(cmd).await?;

        let output = result["result"]
            .as_str()
            .unwrap_or("(no result)")
            .to_string();

        Ok(json!(output))
    }

    /// Navigate the active browser tab to `url` and wait for the load to
    /// complete (max 10 s on the extension side).
    pub async fn navigate(&self, url: &str) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "navigate", "url": url });
        let result = self.send_and_wait(cmd).await?;

        let final_url = result["url"].as_str().unwrap_or(url).to_string();
        Ok(json!(format!("Navigated to {}", final_url)))
    }

    /// List all open browser tabs across all windows.
    pub async fn tab_list(&self) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "tab_list" });
        let result = self.send_and_wait(cmd).await?;
        Ok(result["tabs"].clone())
    }

    /// Create a new browser tab, optionally navigating to a URL.
    pub async fn tab_new(&self, url: Option<&str>, active: bool) -> Result<Value> {
        let cmd = json!({
            "type": "browser_cmd", "cmd": "tab_new",
            "url": url.unwrap_or("about:blank"),
            "active": active,
        });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({
            "id": result["id"],
            "url": result["url"],
            "title": result["title"]
        }))
    }

    /// Close one or more browser tabs by their IDs.
    pub async fn tab_close(&self, tab_ids: Vec<i64>) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "tab_close", "tab_ids": tab_ids });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!(format!("Closed tabs: {:?}", result["closed"])))
    }

    /// Switch to a tab by its ID.
    pub async fn tab_switch(&self, tab_id: i64, window_id: Option<i64>) -> Result<Value> {
        let mut cmd = json!({ "type": "browser_cmd", "cmd": "tab_switch", "tab_id": tab_id });
        if let Some(wid) = window_id {
            cmd["window_id"] = json!(wid);
        }
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({
            "id": result["id"],
            "url": result["url"],
            "title": result["title"]
        }))
    }

    /// Navigate back in the active tab's history.
    pub async fn back(&self) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "back" });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({ "url": result["url"], "title": result["title"] }))
    }

    /// Navigate forward in the active tab's history.
    pub async fn forward(&self) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "forward" });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({ "url": result["url"], "title": result["title"] }))
    }

    /// Reload the active tab, optionally bypassing cache.
    pub async fn reload(&self, bypass_cache: bool) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "reload", "bypass_cache": bypass_cache });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({ "url": result["url"], "title": result["title"] }))
    }

    /// Get cookies for a URL.
    pub async fn get_cookies(&self, url: &str) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "get_cookies", "url": url });
        let result = self.send_and_wait(cmd).await?;
        Ok(result["cookies"].clone())
    }

    /// Set a cookie.
    pub async fn set_cookie(&self, params: &Value) -> Result<Value> {
        let mut cmd = params.clone();
        cmd["type"] = json!("browser_cmd");
        cmd["cmd"] = json!("set_cookie");
        self.send_and_wait(cmd).await?;
        Ok(json!("Cookie set"))
    }

    /// Delete cookies for a URL, optionally filtering by name.
    pub async fn delete_cookies(&self, url: &str, name: Option<&str>) -> Result<Value> {
        let mut cmd = json!({ "type": "browser_cmd", "cmd": "delete_cookies", "url": url });
        if let Some(n) = name {
            cmd["name"] = json!(n);
        }
        let result = self.send_and_wait(cmd).await?;
        let count = result["deleted"].as_u64().unwrap_or(0);
        Ok(json!(format!("Deleted {} cookie(s)", count)))
    }

    /// Read the current page content in different modes: "text", "html", or "links".
    pub async fn read_page(&self, mode: &str) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "read_page", "mode": mode });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({
            "url": result["url"],
            "title": result["title"],
            "content": result["content"]
        }))
    }

    /// Click an element by CSS selector.
    pub async fn click(&self, selector: &str) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "click", "selector": selector });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!(result["result"]))
    }

    /// Fill a form field by CSS selector.
    pub async fn fill(&self, selector: &str, value: &str, submit: bool) -> Result<Value> {
        let cmd = json!({
            "type": "browser_cmd", "cmd": "fill",
            "selector": selector, "value": value, "submit": submit
        });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!(result["result"]))
    }

    /// Get the current tab's URL and title.
    pub async fn get_url(&self) -> Result<Value> {
        let cmd = json!({ "type": "browser_cmd", "cmd": "get_url" });
        let result = self.send_and_wait(cmd).await?;
        Ok(json!({
            "url": result["url"],
            "title": result["title"],
            "id": result["id"]
        }))
    }
}
