use crate::state::Config;
use crate::tools;
use anyhow::Result;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error};

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Value, // string or array of content blocks
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PageContext {
    pub url: String,
    pub title: String,
    pub selected_text: Option<String>,
    pub page_text: Option<String>,
}

// Messages sent back to the WS session
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta { delta: String, message_id: String },
    ToolStart { tool_name: String, tool_use_id: String, input: Value },
    ToolResult { tool_use_id: String, output: String, is_error: bool },
    MessageComplete { stop_reason: String },
    Error { code: String, message: String },
}

// ── System prompt ────────────────────────────────────────────────────────────

fn build_system_prompt(page_ctx: Option<&PageContext>, workspace: &str) -> String {
    let mut prompt = format!(
        r#"You are Bow, a hybrid AI agent running as a desktop app on Windows with full file system, shell, and web access.

## Capabilities
- Read and write any file on the local filesystem using file_read / file_write
- Execute PowerShell commands via shell_exec (120s timeout) — downloads, scripts, system operations
- Search the web with web_search (Tavily) — returns summaries and URLs
- Verify images with image_verify — sends a local image to a local AI vision model (no cloud, no censorship, no restrictions) to identify people, check content, describe what's in the image

## Downloading images — IMPORTANT
When asked to find and download images of a person or topic:
1. Use shell_exec to run PowerShell that scrapes image URLs directly. Example approach:
   ```
   $query = "Kristen Stewart photo"
   $url = "https://www.google.com/search?q=$($query -replace ' ','+')&tbm=isch&tbs=isz:l"
   $headers = @{{ "User-Agent" = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36" }}
   $html = (Invoke-WebRequest -Uri $url -Headers $headers -UseBasicParsing).Content
   [regex]::Matches($html, 'https://[^"'']+\.(?:jpg|jpeg|png|webp)') | Select-Object -First 20 -ExpandProperty Value
   ```
2. Download each URL with: `Invoke-WebRequest -Uri $imageUrl -OutFile $dest -Headers @{{"User-Agent"="Mozilla/5.0"}}`
3. After downloading, use image_verify to confirm the image matches what was requested
4. Delete images that don't match and try more URLs
5. ALWAYS include -Headers with a User-Agent when downloading — bare requests get blocked

## Behaviour
- Default workspace: {workspace}
- Always use absolute Windows paths
- Chain tool calls without asking for confirmation — complete multi-step tasks autonomously
- Be concise in responses; let tool results speak for themselves
- If a download fails, try alternative URLs — don't give up after one attempt
"#,
        workspace = workspace
    );

    if let Some(ctx) = page_ctx {
        prompt.push_str("\n## Current Browser Context\n");
        prompt.push_str(&format!("URL: {}\n", ctx.url));
        prompt.push_str(&format!("Title: {}\n", ctx.title));
        if let Some(sel) = &ctx.selected_text {
            if !sel.is_empty() {
                prompt.push_str(&format!("Selected text: {}\n", sel));
            }
        }
        if let Some(text) = &ctx.page_text {
            if !text.is_empty() {
                prompt.push_str(&format!("\nPage content:\n{}\n", text));
            }
        }
    }

    prompt
}

// ── Main chat entry point ─────────────────────────────────────────────────────

pub async fn run_chat(
    config: Arc<Config>,
    history: &mut Vec<AnthropicMessage>,
    user_message: String,
    message_id: String,
    page_ctx: Option<PageContext>,
    interrupt: Arc<AtomicBool>,
    event_tx: mpsc::Sender<AgentEvent>,
    shell_session: crate::tools::shell_session::ShellSessionManager,
    browser: crate::tools::browser::BrowserBridge,
) -> Result<()> {
    // Append user message
    history.push(AnthropicMessage {
        role: "user".to_string(),
        content: json!(user_message),
    });

    let system_prompt = build_system_prompt(page_ctx.as_ref(), &config.workspace_root.to_string_lossy());
    let tools = tools::tool_schemas();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut iterations = 0;
    const MAX_ITERATIONS: u8 = 25;

    loop {
        if iterations >= MAX_ITERATIONS {
            let _ = event_tx
                .send(AgentEvent::MessageComplete {
                    stop_reason: "max_iterations".to_string(),
                })
                .await;
            break;
        }
        iterations += 1;

        if interrupt.load(Ordering::Relaxed) {
            let _ = event_tx
                .send(AgentEvent::MessageComplete {
                    stop_reason: "interrupted".to_string(),
                })
                .await;
            break;
        }

        // Build request body
        let body = json!({
            "model": config.model,
            "max_tokens": 8096,
            "system": system_prompt,
            "messages": history,
            "tools": tools,
            "stream": true
        });

        debug!("Sending request to Anthropic, iteration {}", iterations);

        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &config.anthropic_api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Anthropic request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            error!("Anthropic error {}: {}", status, err_body);
            let _ = event_tx
                .send(AgentEvent::Error {
                    code: status.as_str().to_string(),
                    message: err_body,
                })
                .await;
            return Err(anyhow::anyhow!("Anthropic API error {}", status));
        }

        // Parse SSE stream
        let stop_reason = stream_response(
            resp,
            history,
            &message_id,
            &interrupt,
            &event_tx,
            &config,
            &shell_session,
            &browser,
        )
        .await?;

        match stop_reason.as_str() {
            "tool_use" => {
                // history was already updated inside stream_response; loop continues
                continue;
            }
            _ => {
                let _ = event_tx
                    .send(AgentEvent::MessageComplete {
                        stop_reason,
                    })
                    .await;
                break;
            }
        }
    }

    // Trim history to max 40 messages (keep pairs)
    trim_history(history);

    Ok(())
}

// ── SSE streaming ─────────────────────────────────────────────────────────────

async fn stream_response(
    resp: reqwest::Response,
    history: &mut Vec<AnthropicMessage>,
    message_id: &str,
    interrupt: &Arc<AtomicBool>,
    event_tx: &mpsc::Sender<AgentEvent>,
    config: &Arc<Config>,
    shell_session: &crate::tools::shell_session::ShellSessionManager,
    browser: &crate::tools::browser::BrowserBridge,
) -> Result<String> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    // Accumulate the full assistant turn
    let mut full_text = String::new();
    let mut tool_uses: Vec<Value> = Vec::new();
    let mut current_tool_use: Option<(String, String, String)> = None; // (id, name, input_json)
    let mut stop_reason = "end_turn".to_string();

    while let Some(chunk) = byte_stream.next().await {
        if interrupt.load(Ordering::Relaxed) {
            stop_reason = "interrupted".to_string();
            break;
        }

        let chunk = chunk.map_err(|e| anyhow::anyhow!("Stream read error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE events from buffer
        while let Some(pos) = buffer.find("\n\n") {
            let event_str = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    match serde_json::from_str::<Value>(data) {
                        Ok(evt) => {
                            process_sse_event(
                                &evt,
                                &mut full_text,
                                &mut tool_uses,
                                &mut current_tool_use,
                                &mut stop_reason,
                                message_id,
                                event_tx,
                            )
                            .await;
                        }
                        Err(e) => {
                            debug!("SSE parse error: {} for data: {}", e, data);
                        }
                    }
                }
            }
        }
    }

    // Flush any incomplete tool input accumulation
    if let Some((id, name, input_json)) = current_tool_use.take() {
        let input: Value = serde_json::from_str(&input_json).unwrap_or(json!({}));
        tool_uses.push(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input
        }));
    }

    // Build assistant content block
    let mut content_blocks: Vec<Value> = Vec::new();
    if !full_text.is_empty() {
        content_blocks.push(json!({"type": "text", "text": full_text}));
    }
    content_blocks.extend(tool_uses.clone());

    history.push(AnthropicMessage {
        role: "assistant".to_string(),
        content: json!(content_blocks),
    });

    // Execute tools if needed
    if stop_reason == "tool_use" {
        let mut tool_results: Vec<Value> = Vec::new();

        for tool_block in &tool_uses {
            let tool_use_id = tool_block["id"].as_str().unwrap_or("").to_string();
            let tool_name = tool_block["name"].as_str().unwrap_or("").to_string();
            let tool_input = tool_block["input"].clone();

            // Notify extension
            let _ = event_tx
                .send(AgentEvent::ToolStart {
                    tool_name: tool_name.clone(),
                    tool_use_id: tool_use_id.clone(),
                    input: tool_input.clone(),
                })
                .await;

            let (output, is_error) =
                match tools::dispatch(&tool_name, &tool_input, &config.tavily_api_key, &config.lm_studio_url, &config.lm_studio_model, &config.workspace_root.to_string_lossy(), shell_session, browser).await {
                    Ok(result) => (result, false),
                    Err(e) => (json!(e.to_string()), true),
                };

            let _ = event_tx
                .send(AgentEvent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    output: output.to_string(),
                    is_error,
                })
                .await;

            tool_results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output,
                "is_error": is_error
            }));
        }

        history.push(AnthropicMessage {
            role: "user".to_string(),
            content: json!(tool_results),
        });
    }

    Ok(stop_reason)
}

async fn process_sse_event(
    evt: &Value,
    full_text: &mut String,
    tool_uses: &mut Vec<Value>,
    current_tool_use: &mut Option<(String, String, String)>,
    stop_reason: &mut String,
    message_id: &str,
    event_tx: &mpsc::Sender<AgentEvent>,
) {
    let event_type = evt["type"].as_str().unwrap_or("");

    match event_type {
        "content_block_start" => {
            let block = &evt["content_block"];
            if block["type"].as_str() == Some("tool_use") {
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();
                *current_tool_use = Some((id, name, String::new()));
            }
        }
        "content_block_delta" => {
            let delta = &evt["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => {
                    if let Some(text) = delta["text"].as_str() {
                        full_text.push_str(text);
                        let _ = event_tx
                            .send(AgentEvent::TextDelta {
                                delta: text.to_string(),
                                message_id: message_id.to_string(),
                            })
                            .await;
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta["partial_json"].as_str() {
                        if let Some((_, _, ref mut input_json)) = current_tool_use.as_mut() {
                            input_json.push_str(partial);
                        }
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if let Some((id, name, input_json)) = current_tool_use.take() {
                let input: Value = serde_json::from_str(&input_json).unwrap_or(json!({}));
                tool_uses.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
        }
        "message_delta" => {
            if let Some(reason) = evt["delta"]["stop_reason"].as_str() {
                *stop_reason = reason.to_string();
            }
        }
        "error" => {
            error!("SSE error event: {:?}", evt);
        }
        _ => {}
    }
}

fn trim_history(history: &mut Vec<AnthropicMessage>) {
    const MAX_MESSAGES: usize = 40;
    if history.len() <= MAX_MESSAGES {
        return;
    }

    // Remove oldest messages in pairs to avoid splitting tool_use/tool_result
    while history.len() > MAX_MESSAGES {
        // Always remove at least 2 to keep role alternation
        if history.len() >= 2 {
            history.remove(0);
            history.remove(0);
        } else {
            history.remove(0);
        }
    }
}
