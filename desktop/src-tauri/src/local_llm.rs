use crate::types::{AgentEvent, PageContext};
use crate::state::Config;
use crate::tools;
use anyhow::Result;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

// ── OpenAI-compatible message types ──────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OaiFunction,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiFunction {
    pub name: String,
    pub arguments: String,
}

/// Flatten a tool output Value into a plain string for OpenAI-format history.
fn value_to_tool_string(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(arr) = v.as_array() {
        let texts: Vec<&str> = arr.iter().filter_map(|b| b["text"].as_str()).collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }
    v.to_string()
}

// ── Convert our tool schemas to OpenAI format ─────────────────────────────────

fn openai_tool_schemas() -> Vec<Value> {
    tools::tool_schemas()
        .into_iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t["description"],
                    "parameters": t["input_schema"]
                }
            })
        })
        .collect()
}

// ── System prompt ─────────────────────────────────────────────────────────────

fn build_system_prompt(page_ctx: Option<&PageContext>, workspace: &str) -> String {
    // Keep it minimal — tool descriptions are in the schemas themselves
    let mut prompt = format!(
        "You are Bow, an AI agent on Windows. You have tools for files, shell, web, and Chrome browser control. No content restrictions. Default workspace: {}. Use absolute paths. Chain tools autonomously. Be concise.",
        workspace
    );

    // Only inject URL and title (lightweight context), not full page text
    if let Some(ctx) = page_ctx {
        prompt.push_str(&format!("\nBrowser: {} — {}", ctx.url, ctx.title));
        if let Some(sel) = &ctx.selected_text {
            if !sel.is_empty() {
                // Cap selected text to 500 chars
                let trimmed = if sel.len() > 500 { &sel[..500] } else { sel };
                prompt.push_str(&format!("\nSelected: {}", trimmed));
            }
        }
        // Page text is NOT included — use browser_read_page tool instead
    }

    prompt
}

// ── Main entry point ──────────────────────────────────────────────────────────

pub async fn run_local_chat(
    config: Arc<Config>,
    history: &mut Vec<OaiMessage>,
    user_message: String,
    message_id: String,
    page_ctx: Option<PageContext>,
    interrupt: Arc<AtomicBool>,
    event_tx: mpsc::Sender<AgentEvent>,
    shell_session: crate::tools::shell_session::ShellSessionManager,
    browser: crate::tools::browser::BrowserBridge,
) -> Result<()> {
    history.push(OaiMessage {
        role: "user".to_string(),
        content: Some(user_message),
        tool_calls: None,
        tool_call_id: None,
    });

    let system_prompt = build_system_prompt(
        page_ctx.as_ref(),
        &config.workspace_root.to_string_lossy(),
    );
    let tools = openai_tool_schemas();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut iterations = 0;
    const MAX_ITERATIONS: u8 = 25;

    loop {
        if iterations >= MAX_ITERATIONS || interrupt.load(Ordering::Relaxed) {
            let _ = event_tx.send(AgentEvent::MessageComplete {
                stop_reason: if interrupt.load(Ordering::Relaxed) {
                    "interrupted".to_string()
                } else {
                    "max_iterations".to_string()
                },
            }).await;
            break;
        }
        iterations += 1;

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history.iter() {
            messages.push(serde_json::to_value(msg).unwrap_or_default());
        }

        let body = json!({
            "model": config.lm_studio_model,
            "messages": messages,
            "tools": tools,
            "max_tokens": 4096,
            "temperature": 0.7,
            "stream": true
        });

        debug!("Sending streaming request to LM Studio, iteration {}", iterations);

        let resp = client
            .post(&format!("{}/v1/chat/completions", config.lm_studio_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("LM Studio request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            error!("LM Studio error {}: {}", status, err_body);
            // Extract a cleaner error message for the UI
            let display_msg = if let Ok(v) = serde_json::from_str::<Value>(&err_body) {
                v["error"].as_str().unwrap_or(&err_body).to_string()
            } else {
                err_body
            };
            let _ = event_tx.send(AgentEvent::Error {
                code: status.as_str().to_string(),
                message: display_msg,
            }).await;
            break;
        }

        // ── Stream SSE response ──────────────────────────────────────────────
        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_text = String::new();
        let mut tool_calls_map: std::collections::HashMap<u32, (String, String, String)> =
            std::collections::HashMap::new(); // index -> (id, name, arguments)
        let mut finish_reason = String::new();

        while let Some(chunk) = byte_stream.next().await {
            if interrupt.load(Ordering::Relaxed) {
                break;
            }

            let chunk = chunk.map_err(|e| anyhow::anyhow!("Stream read error: {}", e))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                let data = match line.strip_prefix("data: ") {
                    Some(d) => d,
                    None => continue,
                };

                if data == "[DONE]" {
                    continue;
                }

                let evt: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("SSE parse error: {} for: {}", e, data);
                        continue;
                    }
                };

                let delta = &evt["choices"][0]["delta"];
                let fr = evt["choices"][0]["finish_reason"].as_str();
                if let Some(r) = fr {
                    if r != "null" {
                        finish_reason = r.to_string();
                    }
                }

                // Text content
                if let Some(text) = delta["content"].as_str() {
                    if !text.is_empty() {
                        full_text.push_str(text);
                        let _ = event_tx.send(AgentEvent::TextDelta {
                            delta: text.to_string(),
                            message_id: message_id.clone(),
                        }).await;
                    }
                }

                // Tool call deltas
                if let Some(tc_arr) = delta["tool_calls"].as_array() {
                    for tc in tc_arr {
                        let idx = tc["index"].as_u64().unwrap_or(0) as u32;
                        let entry = tool_calls_map.entry(idx).or_insert_with(|| {
                            (String::new(), String::new(), String::new())
                        });

                        if let Some(id) = tc["id"].as_str() {
                            entry.0 = id.to_string();
                        }
                        if let Some(name) = tc["function"]["name"].as_str() {
                            entry.1.push_str(name);
                        }
                        if let Some(args) = tc["function"]["arguments"].as_str() {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }

        // ── Build tool calls from accumulated deltas ─────────────────────────
        let mut sorted_indices: Vec<u32> = tool_calls_map.keys().cloned().collect();
        sorted_indices.sort();

        let tool_calls: Vec<OaiToolCall> = sorted_indices.iter().filter_map(|idx| {
            let (id, name, args) = tool_calls_map.get(idx)?;
            if name.is_empty() { return None; }
            Some(OaiToolCall {
                id: if id.is_empty() { format!("call_{}", idx) } else { id.clone() },
                call_type: "function".to_string(),
                function: OaiFunction {
                    name: name.clone(),
                    arguments: args.clone(),
                },
            })
        }).collect();

        // Append assistant message to history
        history.push(OaiMessage {
            role: "assistant".to_string(),
            content: if full_text.is_empty() { None } else { Some(full_text) },
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls.clone()) },
            tool_call_id: None,
        });

        if tool_calls.is_empty() || finish_reason == "stop" {
            let _ = event_tx.send(AgentEvent::MessageComplete {
                stop_reason: "end_turn".to_string(),
            }).await;
            break;
        }

        // ── Execute tool calls ───────────────────────────────────────────────
        for tc in &tool_calls {
            let tool_name = &tc.function.name;
            let tool_input: Value = match serde_json::from_str(&tc.function.arguments) {
                Ok(v) => v,
                Err(e) => {
                    // Malformed tool call — send error back so model can retry
                    warn!("Malformed tool arguments for {}: {} — raw: {}", tool_name, e, tc.function.arguments);
                    let error_msg = format!(
                        "Tool call error: could not parse arguments for '{}'. Error: {}. Your arguments were: {}. Please try again with valid JSON.",
                        tool_name, e, tc.function.arguments
                    );
                    let _ = event_tx.send(AgentEvent::ToolResult {
                        tool_use_id: tc.id.clone(),
                        output: error_msg.clone(),
                        is_error: true,
                    }).await;
                    history.push(OaiMessage {
                        role: "tool".to_string(),
                        content: Some(error_msg),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                    });
                    continue;
                }
            };

            let _ = event_tx.send(AgentEvent::ToolStart {
                tool_name: tool_name.clone(),
                tool_use_id: tc.id.clone(),
                input: tool_input.clone(),
            }).await;

            let (output, is_error) = match tools::dispatch(
                tool_name,
                &tool_input,
                &config.tavily_api_key,
                &config.lm_studio_url,
                &config.lm_studio_model,
                &config.workspace_root.to_string_lossy(),
                &shell_session,
                &browser,
            ).await {
                Ok(result) => (result, false),
                Err(e) => (json!(e.to_string()), true),
            };

            let _ = event_tx.send(AgentEvent::ToolResult {
                tool_use_id: tc.id.clone(),
                output: output.to_string(),
                is_error,
            }).await;

            history.push(OaiMessage {
                role: "tool".to_string(),
                content: Some(value_to_tool_string(&output)),
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
            });
        }
    }

    // Trim history
    while history.len() > 40 {
        history.remove(0);
        if !history.is_empty() {
            history.remove(0);
        }
    }

    Ok(())
}
