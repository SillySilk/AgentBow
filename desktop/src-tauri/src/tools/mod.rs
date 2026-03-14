pub mod browser;
pub mod file_ops;
pub mod image_search;
pub mod shell_exec;
pub mod shell_session;
pub mod web_search;

use anyhow::Result;
use serde_json::{json, Value};

pub fn tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "name": "file_read",
            "description": "Read a file at an absolute Windows path.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "file_write",
            "description": "Write content to a file. Creates parent dirs if needed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "file_list",
            "description": "List files and directories at a path.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string" }
                },
                "required": ["dir"]
            }
        }),
        json!({
            "name": "shell_exec",
            "description": "Run a PowerShell command. Returns stdout/stderr. 120s timeout.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "web_search",
            "description": "Search the web via Tavily. Returns summary and top results.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "image_verify",
            "description": "Analyze a local image with vision AI. Identify people, describe contents.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "image_path": { "type": "string" },
                    "prompt": { "type": "string" }
                },
                "required": ["image_path", "prompt"]
            }
        }),
        json!({
            "name": "browser_screenshot",
            "description": "Capture screenshot of current browser tab.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "browser_exec_js",
            "description": "Execute JavaScript in the active browser tab.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "js": { "type": "string" }
                },
                "required": ["js"]
            }
        }),
        json!({
            "name": "browser_navigate",
            "description": "Navigate active tab to a URL. Waits for load.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "browser_tab_list",
            "description": "List all open browser tabs with ID, title, URL.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "browser_tab_new",
            "description": "Open a new browser tab, optionally at a URL.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "active": { "type": "boolean" }
                }
            }
        }),
        json!({
            "name": "browser_tab_close",
            "description": "Close browser tabs by ID.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tab_ids": { "type": "array", "items": { "type": "integer" } }
                },
                "required": ["tab_ids"]
            }
        }),
        json!({
            "name": "browser_tab_switch",
            "description": "Switch to a browser tab by ID.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tab_id": { "type": "integer" },
                    "window_id": { "type": "integer" }
                },
                "required": ["tab_id"]
            }
        }),
        json!({
            "name": "browser_back",
            "description": "Go back in browser history.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "browser_forward",
            "description": "Go forward in browser history.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "browser_reload",
            "description": "Reload the active browser tab.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "bypass_cache": { "type": "boolean" }
                }
            }
        }),
        json!({
            "name": "browser_get_cookies",
            "description": "Get cookies for a URL.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "browser_set_cookie",
            "description": "Set a browser cookie.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "name": { "type": "string" },
                    "value": { "type": "string" },
                    "domain": { "type": "string" },
                    "path": { "type": "string" },
                    "secure": { "type": "boolean" },
                    "httpOnly": { "type": "boolean" },
                    "sameSite": { "type": "string" },
                    "expirationDate": { "type": "number" }
                },
                "required": ["url", "name", "value"]
            }
        }),
        json!({
            "name": "browser_delete_cookies",
            "description": "Delete cookies for a URL, optionally by name.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "browser_read_page",
            "description": "Read page content. Mode: 'text', 'html', or 'links'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "browser_click",
            "description": "Click an element by CSS selector.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string" }
                },
                "required": ["selector"]
            }
        }),
        json!({
            "name": "browser_fill",
            "description": "Fill a form field by CSS selector. Fires input/change events.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "value": { "type": "string" },
                    "submit": { "type": "boolean" }
                },
                "required": ["selector", "value"]
            }
        }),
        json!({
            "name": "browser_scroll",
            "description": "Scroll the page. Direction: 'up', 'down', 'top', 'bottom', or a CSS selector to scroll to.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string" },
                    "pixels": { "type": "integer" }
                },
                "required": ["target"]
            }
        }),
        json!({
            "name": "browser_get_url",
            "description": "Get current tab URL, title, and ID.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "image_download",
            "description": "Download images matching a search query to disk.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "count": { "type": "integer" },
                    "dest_dir": { "type": "string" }
                },
                "required": ["query", "dest_dir"]
            }
        }),
    ]
}

pub async fn dispatch(
    tool_name: &str,
    input: &Value,
    tavily_api_key: &str,
    lm_studio_url: &str,
    lm_studio_model: &str,
    workspace_root: &str,
    shell_session: &shell_session::ShellSessionManager,
    browser: &browser::BrowserBridge,
) -> Result<Value> {
    match tool_name {
        "file_read" => {
            let path = input["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_read: missing 'path'"))?;
            let s = file_ops::file_read(path)?;
            Ok(json!(s))
        }
        "file_list" => {
            let dir = input["dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_list: missing 'dir'"))?;
            let s = file_ops::file_list(dir)?;
            Ok(json!(s))
        }
        "file_write" => {
            let path = input["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_write: missing 'path'"))?;
            let content = input["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_write: missing 'content'"))?;
            let s = file_ops::file_write(path, content)?;
            Ok(json!(s))
        }
        "shell_exec" => {
            let command = input["command"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("shell_exec: missing 'command'"))?;
            let s = shell_session.execute(command).await?;
            Ok(json!(s))
        }
        "web_search" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("web_search: missing 'query'"))?;
            let s = web_search::web_search(query, tavily_api_key).await?;
            Ok(json!(s))
        }
        "image_verify" => {
            let image_path = input["image_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_verify: missing 'image_path'"))?;
            let prompt = input["prompt"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_verify: missing 'prompt'"))?;
            let s = image_search::image_verify(image_path, prompt, lm_studio_url, lm_studio_model).await?;
            Ok(json!(s))
        }
        "browser_screenshot" => browser.screenshot().await,
        "browser_exec_js" => {
            let js = input["js"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_exec_js: missing 'js'"))?;
            browser.exec_js(js).await
        }
        "browser_navigate" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_navigate: missing 'url'"))?;
            browser.navigate(url).await
        }
        "browser_tab_list" => browser.tab_list().await,
        "browser_tab_new" => {
            let url = input["url"].as_str();
            let active = input["active"].as_bool().unwrap_or(true);
            browser.tab_new(url, active).await
        }
        "browser_tab_close" => {
            let tab_ids: Vec<i64> = input["tab_ids"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("browser_tab_close: missing 'tab_ids'"))?
                .iter()
                .filter_map(|v| v.as_i64())
                .collect();
            browser.tab_close(tab_ids).await
        }
        "browser_tab_switch" => {
            let tab_id = input["tab_id"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("browser_tab_switch: missing 'tab_id'"))?;
            let window_id = input["window_id"].as_i64();
            browser.tab_switch(tab_id, window_id).await
        }
        "browser_back" => browser.back().await,
        "browser_forward" => browser.forward().await,
        "browser_reload" => {
            let bypass = input["bypass_cache"].as_bool().unwrap_or(false);
            browser.reload(bypass).await
        }
        "browser_get_cookies" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_get_cookies: missing 'url'"))?;
            browser.get_cookies(url).await
        }
        "browser_set_cookie" => {
            browser.set_cookie(input).await
        }
        "browser_delete_cookies" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_delete_cookies: missing 'url'"))?;
            let name = input["name"].as_str();
            browser.delete_cookies(url, name).await
        }
        "browser_read_page" => {
            let mode = input["mode"].as_str().unwrap_or("text");
            browser.read_page(mode).await
        }
        "browser_click" => {
            let selector = input["selector"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_click: missing 'selector'"))?;
            browser.click(selector).await
        }
        "browser_fill" => {
            let selector = input["selector"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_fill: missing 'selector'"))?;
            let value = input["value"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_fill: missing 'value'"))?;
            let submit = input["submit"].as_bool().unwrap_or(false);
            browser.fill(selector, value, submit).await
        }
        "browser_scroll" => {
            let target = input["target"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("browser_scroll: missing 'target'"))?;
            let pixels = input["pixels"].as_i64().unwrap_or(500);
            browser.scroll(target, pixels).await
        }
        "browser_get_url" => browser.get_url().await,
        "image_download" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_download: missing 'query'"))?;
            let count = input["count"].as_u64().unwrap_or(10) as usize;
            let dest_dir = input["dest_dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_download: missing 'dest_dir'"))?;
            let log_dir = format!("{}\\logs", workspace_root.trim_end_matches(['\\', '/']));
            let s = image_search::image_download(query, count, dest_dir, &log_dir).await?;
            Ok(json!(s))
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
    }
}
