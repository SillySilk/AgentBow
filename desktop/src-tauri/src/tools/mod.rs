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
            "description": "Read the contents of a file at an absolute path on the Windows filesystem.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute Windows path to read, e.g. C:\\AI\\workspace\\notes.md"
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "file_write",
            "description": "Write content to a file at an absolute path. Creates parent directories if needed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute Windows path to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "file_list",
            "description": "List files and directories at a given path. Use before downloading to see what already exists.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Absolute Windows path to list, e.g. C:\\AI\\workspace" }
                },
                "required": ["dir"]
            }
        }),
        json!({
            "name": "shell_exec",
            "description": "Execute a PowerShell command on the local Windows machine. Returns stdout and stderr. 30 second timeout. Use for file downloads (Invoke-WebRequest), system info, running scripts, etc.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "PowerShell command to execute"
                    }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "web_search",
            "description": "Search the web using Tavily. Returns a summary answer and top results with URLs. Use for text-based research, news, facts, articles. Also useful for finding image URLs on pages.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "image_verify",
            "description": "Analyze a local image file using local vision AI (no cloud, no censorship). Send a downloaded image for verification — identify people, describe contents, check quality. Use after downloading images to confirm they match what was expected. The image must already be saved to disk.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "image_path": {
                        "type": "string",
                        "description": "Absolute path to the image file on disk"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "What to ask about the image, e.g. 'Is this a photo of Kristen Stewart? Describe what you see.'"
                    }
                },
                "required": ["image_path", "prompt"]
            }
        }),
        json!({
            "name": "browser_screenshot",
            "description": "Capture a screenshot of the current browser tab. Returns a visual image of what the browser is showing right now. Use this to see the page before interacting with it.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "browser_exec_js",
            "description": "Execute JavaScript in the active browser tab and return the result. Use for clicking elements (document.querySelector('.btn').click()), reading DOM values, filling forms, scrolling, etc.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "js": {
                        "type": "string",
                        "description": "JavaScript code to execute in the page context"
                    }
                },
                "required": ["js"]
            }
        }),
        json!({
            "name": "browser_navigate",
            "description": "Navigate the active browser tab to a URL. Waits for the page to finish loading before returning.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to navigate to"
                    }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "image_download",
            "description": "Download images from the web matching a search query. Saves files to disk and returns the list of downloaded paths. Use this instead of shell_exec for downloading images.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query, e.g. 'Eva Green actress'"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Number of images to download (default 10, max 50)"
                    },
                    "dest_dir": {
                        "type": "string",
                        "description": "Absolute Windows path to save images, e.g. C:\\AI\\workspace\\eva_green"
                    }
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
