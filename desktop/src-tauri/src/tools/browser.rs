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
}
