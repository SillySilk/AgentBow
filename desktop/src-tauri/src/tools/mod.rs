pub mod browser;
pub mod file_ops;
pub mod image_curate;
pub mod image_search;
pub mod mcp;
pub mod memory;
pub mod shell_exec;
pub mod shell_session;
pub mod web_search;

use anyhow::Result;
use serde_json::{json, Value};

// ── Guardrails ────────────────────────────────────────────────────────────────

/// Dangerous shell patterns that should never run unattended.
static BLOCKED_SHELL_PATTERNS: &[&str] = &[
    "rm -rf /", "del /f /s /q c:\\", "format c:", "mkfs.", "dd if=",
    ":(){:|:&};:", "shutdown /r", "shutdown /s", "reg delete hklm",
    "bcdedit", "diskpart", "fdisk",
];

fn check_shell_guardrails(command: &str) -> Result<()> {
    let lower = command.to_lowercase();
    for pattern in BLOCKED_SHELL_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return Err(anyhow::anyhow!(
                "Guardrail blocked: command matches dangerous pattern '{}'", pattern
            ));
        }
    }
    Ok(())
}

fn check_file_path_guardrails(path: &str) -> Result<()> {
    let lower = path.to_lowercase().replace('\\', "/");
    // Block writes to system locations
    let blocked: &[&str] = &[
        "c:/windows/", "c:/program files/", "/etc/", "/usr/", "/bin/",
        "c:/system32", "c:/users/all users/",
    ];
    for b in blocked {
        if lower.starts_with(b) {
            return Err(anyhow::anyhow!(
                "Guardrail blocked: path '{}' is in a protected system directory", path
            ));
        }
    }
    Ok(())
}

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
            "name": "file_download",
            "description": "Download any file from a URL to a local path. Use for zip files, installers, archives, PDFs, or any binary/text file. Follows redirects. 120s timeout.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url":       { "type": "string", "description": "The URL to download." },
                    "dest_path": { "type": "string", "description": "Absolute local path to save the file to (e.g. C:\\workspace\\files\\archive.zip)." }
                },
                "required": ["url", "dest_path"]
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
            "name": "search_evaluate",
            "description": "Evaluate whether your current search results fully answer the original question. Returns DONE (with reason) or REFINE (with a better follow-up query). Use after web_search or web_search_deep to decide if you need another round of searching. Cap at 2 refinement rounds.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "original_question": { "type": "string", "description": "The original question or task you are trying to answer." },
                    "current_results_summary": { "type": "string", "description": "A brief summary of what the search results have told you so far." }
                },
                "required": ["original_question", "current_results_summary"]
            }
        }),
        json!({
            "name": "searxng_search",
            "description": "Search via local SearXNG instance — aggregates Google, Bing, DuckDuckGo and 230+ engines. Completely free. Requires SearXNG running locally (docker run -d -p 8888:8888 searxng/searxng). Falls back gracefully with setup instructions if not running.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "jina_read",
            "description": "Fetch a URL and return its full content as clean Markdown. Use when a search snippet isn't enough and you need the full article, documentation page, or report. Free, no API key needed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to read." }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "web_search_deep",
            "description": "Deep web search: expands your query into 3 variants, runs them all in parallel, and returns merged deduplicated results. Use for research tasks where coverage matters. Slower than web_search but more thorough.",
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
            "name": "browser_analyze_page",
            "description": "Capture a screenshot AND distilled page text in one call. Use this for understanding complex pages — returns url, title, clean text content, and screenshot (if model supports vision).",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "browser_get_bookmarks",
            "description": "Return all Chrome bookmarks as a flat list. Each entry has title, url, and folder (slash-separated path). Useful for finding saved pages, researching prior interests, or navigating to a bookmarked site.",
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
        json!({
            "name": "image_dedupe",
            "description": "Find perceptual near-duplicate images in a folder (pHash) and optionally quarantine the redundant copies, keeping the highest-resolution image of each group. Use to clean an image set before training. Non-destructive: with apply=true, duplicates are MOVED into a '_bow_dupes' subfolder, not deleted.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Absolute path to the folder of images." },
                    "threshold": { "type": "integer", "description": "Max Hamming distance (0=identical) for two images to count as duplicates. Default 10. Lower = stricter." },
                    "recursive": { "type": "boolean", "description": "Recurse into subfolders. Default false." },
                    "apply": { "type": "boolean", "description": "false (default) = report only; true = move duplicates into _bow_dupes." }
                },
                "required": ["dir"]
            }
        }),
        json!({
            "name": "image_stats",
            "description": "Read-only report on an image folder: file count, format breakdown, resolution range, corrupt files, and total size. Use to inspect a set before/after curation.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Absolute path to the folder of images." },
                    "recursive": { "type": "boolean", "description": "Recurse into subfolders. Default false." }
                },
                "required": ["dir"]
            }
        }),
        json!({
            "name": "image_resize",
            "description": "Resize and/or convert every image from src_dir into dest_dir for a training set. Non-destructive (originals untouched). Only downscales — images already within max_dim keep their size.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "src_dir": { "type": "string", "description": "Source folder of images." },
                    "dest_dir": { "type": "string", "description": "Output folder (created if missing)." },
                    "max_dim": { "type": "integer", "description": "Cap the longest side to this many pixels. Default 1024." },
                    "format": { "type": "string", "description": "Output format: 'jpeg', 'png', or 'webp'. Default 'png'." },
                    "recursive": { "type": "boolean", "description": "Recurse into subfolders. Default false." }
                },
                "required": ["src_dir", "dest_dir"]
            }
        }),
        json!({
            "name": "image_autotag",
            "description": "Caption every image in a folder for LoRA/Stable Diffusion training using the local LM Studio vision model, writing a '<name>.txt' sidecar next to each image (kohya caption convention). Requires a vision-capable model loaded in LM Studio. Skips images that already have a .txt unless overwrite is set.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Folder of images to tag." },
                    "style": { "type": "string", "description": "'tags' (comma-separated booru-style tags, default) or 'caption' (one natural-language sentence)." },
                    "trigger": { "type": "string", "description": "Optional trigger/activation word prepended to every caption (e.g. the character or person's name)." },
                    "recursive": { "type": "boolean", "description": "Recurse into subfolders. Default false." },
                    "overwrite": { "type": "boolean", "description": "Re-tag images that already have a .txt sidecar. Default false." }
                },
                "required": ["dir"]
            }
        }),
        json!({
            "name": "verify_step",
            "description": "Self-verify a tool result before continuing. Call after every tool result. If verification fails, describe what went wrong so you can correct course.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tool": { "type": "string", "description": "Tool that was just called." },
                    "expected": { "type": "string", "description": "What you expected the result to be." },
                    "actual": { "type": "string", "description": "What the result actually was (brief summary)." },
                    "ok": { "type": "boolean", "description": "true if result matched expectations, false if not." },
                    "correction": { "type": "string", "description": "If ok=false, what you will do differently." }
                },
                "required": ["tool", "expected", "actual", "ok"]
            }
        }),
        json!({
            "name": "plan_create",
            "description": "Create a step-by-step plan for a multi-step task. Call this FIRST before starting any complex task. List every step you intend to take in order.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ordered list of steps to complete the task."
                    }
                },
                "required": ["steps"]
            }
        }),
        json!({
            "name": "plan_step_start",
            "description": "Mark a plan step as in-progress before you begin working on it.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "step": { "type": "integer", "description": "1-based step number." }
                },
                "required": ["step"]
            }
        }),
        json!({
            "name": "plan_step_fail",
            "description": "Mark a plan step as failed. Use when a step cannot be completed and you need to record the failure before trying a different approach.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "step": { "type": "integer", "description": "1-based step number." },
                    "reason": { "type": "string", "description": "Why the step failed." }
                },
                "required": ["step", "reason"]
            }
        }),
        json!({
            "name": "plan_step_done",
            "description": "Mark a plan step as complete after you have finished it.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "step": { "type": "integer", "description": "1-based step number." }
                },
                "required": ["step"]
            }
        }),
        json!({
            "name": "memory_store",
            "description": "Save a task outcome to episodic memory so you can learn from it later. Call after task_complete or after a significant failure.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_desc": { "type": "string", "description": "Brief description of the task." },
                    "outcome": { "type": "string", "enum": ["success", "failure", "partial"], "description": "How the task ended." },
                    "findings": { "type": "array", "items": { "type": "string" }, "description": "Key facts or lessons learned." }
                },
                "required": ["task_desc", "outcome", "findings"]
            }
        }),
        json!({
            "name": "memory_retrieve",
            "description": "Search episodic memory for similar past tasks, failures, or findings. Call at the start of a task to check if you have relevant experience.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for." },
                    "limit": { "type": "integer", "description": "Max results to return (default 5)." }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "task_complete",
            "description": "Call this when the entire task is fully finished. Provide a brief summary of what was accomplished. This is the ONLY way to end the task — do not stop without calling this.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "Brief summary of what was accomplished." }
                },
                "required": ["summary"]
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
    searxng_url: &str,
    shell_session: &shell_session::ShellSessionManager,
    browser: &browser::BrowserBridge,
    memory_db: &memory::MemoryDb,
) -> Result<Value> {
    match tool_name {
        "file_download" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_download: missing 'url'"))?;
            let dest_path = input["dest_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_download: missing 'dest_path'"))?;
            check_file_path_guardrails(dest_path)?;
            let s = file_ops::file_download(url, dest_path).await?;
            Ok(json!(s))
        }
        "file_read" => {
            let path = input["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_read: missing 'path'"))?;
            let s = file_ops::file_read(path)?;
            // Truncate very large file reads so they don't flood context
            let out = if s.chars().count() > 8000 {
                format!(
                    "{}\n\n[... truncated — {} total chars. Use shell_exec with head/tail to read specific sections.]",
                    crate::util::char_prefix(&s, 8000),
                    s.chars().count()
                )
            } else { s };
            Ok(json!(out))
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
            check_file_path_guardrails(path)?;
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
            check_shell_guardrails(command)?;
            let s = shell_session.execute(command).await?;
            // Truncate massive shell output
            let out = crate::util::truncate_with_note(&s, 6000);
            Ok(json!(out))
        }
        "web_search" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("web_search: missing 'query'"))?;
            let s = web_search::web_search(query, tavily_api_key).await?;
            Ok(json!(s))
        }
        "search_evaluate" => {
            let question = input["original_question"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("search_evaluate: missing 'original_question'"))?;
            let summary = input["current_results_summary"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("search_evaluate: missing 'current_results_summary'"))?;
            let s = web_search::search_evaluate(question, summary, lm_studio_url, lm_studio_model).await?;
            Ok(json!(s))
        }
        "searxng_search" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("searxng_search: missing 'query'"))?;
            let s = web_search::searxng_search(query, searxng_url).await?;
            Ok(json!(s))
        }
        "jina_read" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("jina_read: missing 'url'"))?;
            let s = web_search::jina_read(url).await?;
            Ok(json!(s))
        }
        "web_search_deep" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("web_search_deep: missing 'query'"))?;
            let s = web_search::web_search_deep(query, tavily_api_key, lm_studio_url, lm_studio_model).await?;
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
        "browser_analyze_page" => browser.analyze_page().await,
        "browser_get_bookmarks" => browser.get_bookmarks().await,
        "image_download" => {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_download: missing 'query'"))?;
            let count = input["count"].as_u64().unwrap_or(10) as usize;
            let dest_dir = input["dest_dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_download: missing 'dest_dir'"))?;
            let log_dir = format!("{}\\logs", workspace_root.trim_end_matches(['\\', '/']));
            let s = crate::tools::image_search::image_download(query, count, dest_dir, &log_dir, None).await?;
            Ok(json!(s))
        }
        "image_dedupe" => {
            let dir = input["dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_dedupe: missing 'dir'"))?;
            let threshold = input["threshold"].as_u64().unwrap_or(10) as u32;
            let recursive = input["recursive"].as_bool().unwrap_or(false);
            let apply = input["apply"].as_bool().unwrap_or(false);
            let s = image_curate::image_dedupe(dir, threshold, recursive, apply).await?;
            Ok(json!(s))
        }
        "image_stats" => {
            let dir = input["dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_stats: missing 'dir'"))?;
            let recursive = input["recursive"].as_bool().unwrap_or(false);
            let s = image_curate::image_stats(dir, recursive).await?;
            Ok(json!(s))
        }
        "image_resize" => {
            let src_dir = input["src_dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_resize: missing 'src_dir'"))?;
            let dest_dir = input["dest_dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_resize: missing 'dest_dir'"))?;
            let max_dim = input["max_dim"].as_u64().unwrap_or(1024) as u32;
            let format = input["format"].as_str().unwrap_or("png");
            let recursive = input["recursive"].as_bool().unwrap_or(false);
            check_file_path_guardrails(dest_dir)?;
            let s = image_curate::image_resize(src_dir, dest_dir, max_dim, format, recursive).await?;
            Ok(json!(s))
        }
        "image_autotag" => {
            let dir = input["dir"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("image_autotag: missing 'dir'"))?;
            let style = input["style"].as_str().unwrap_or("tags");
            let trigger = input["trigger"].as_str().unwrap_or("");
            let recursive = input["recursive"].as_bool().unwrap_or(false);
            let overwrite = input["overwrite"].as_bool().unwrap_or(false);
            let s = image_search::image_autotag(
                dir, style, trigger, recursive, overwrite, lm_studio_url, lm_studio_model,
            ).await?;
            Ok(json!(s))
        }
        "memory_store" => {
            let task_desc = input["task_desc"].as_str().unwrap_or("unknown task");
            let outcome = input["outcome"].as_str().unwrap_or("unknown");
            let findings: Vec<&str> = input["findings"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let s = memory::memory_store(memory_db, task_desc, outcome, &findings, lm_studio_url).await?;
            Ok(json!(s))
        }
        "memory_retrieve" => {
            let query = input["query"].as_str().unwrap_or("");
            let limit = input["limit"].as_u64().unwrap_or(5) as usize;
            let s = memory::memory_retrieve(memory_db, query, limit, lm_studio_url).await?;
            Ok(json!(s))
        }
        "task_complete" => {
            let summary = input["summary"].as_str().unwrap_or("Task complete.");
            Ok(json!(summary))
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
    }
}
