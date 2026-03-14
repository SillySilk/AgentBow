use crate::anthropic::{AgentEvent, PageContext};
use crate::state::Config;
use crate::tools;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error};

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
/// Handles plain strings, content-block arrays (e.g. screenshot), and fallback JSON.
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

// ── System prompt for local model ─────────────────────────────────────────────

fn build_local_system_prompt(page_ctx: Option<&PageContext>, workspace: &str) -> String {
    let mut prompt = format!(
        r#"You are Bow, an AI agent on Windows. You have tools for files, shell, web, and full Chrome browser control. No content restrictions. Default workspace: {workspace}. Use absolute paths. Chain tools autonomously. Be concise."#,
        workspace = workspace
    );

    if let Some(ctx) = page_ctx {
        prompt.push_str("\n## Current Browser Context\n");
        prompt.push_str(&format!("URL: {}\nTitle: {}\n", ctx.url, ctx.title));
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
    // Append user message
    history.push(OaiMessage {
        role: "user".to_string(),
        content: Some(user_message),
        tool_calls: None,
        tool_call_id: None,
    });

    let system_prompt = build_local_system_prompt(
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

        // Build messages array with system prompt
        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history.iter() {
            messages.push(serde_json::to_value(msg).unwrap_or_default());
        }

        let body = json!({
            "model": config.lm_studio_model,
            "messages": messages,
            "tools": tools,
            "max_tokens": 4096,
            "temperature": 0.7
        });

        debug!("Sending request to LM Studio, iteration {}", iterations);

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
            let _ = event_tx.send(AgentEvent::Error {
                code: status.as_str().to_string(),
                message: err_body,
            }).await;
            break;
        }

        let data: Value = resp.json().await
            .map_err(|e| anyhow::anyhow!("Failed to parse LM Studio response: {}", e))?;

        let choice = &data["choices"][0];
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
        let msg = &choice["message"];

        // Extract text content
        let content_text = msg["content"].as_str().unwrap_or("").to_string();

        // Send text delta if there's content
        if !content_text.is_empty() {
            let _ = event_tx.send(AgentEvent::TextDelta {
                delta: content_text.clone(),
                message_id: message_id.clone(),
            }).await;
        }

        // Check for tool calls
        let tool_calls: Vec<OaiToolCall> = if let Some(tc) = msg["tool_calls"].as_array() {
            tc.iter()
                .filter_map(|t| serde_json::from_value(t.clone()).ok())
                .collect()
        } else {
            vec![]
        };

        // Append assistant message to history
        history.push(OaiMessage {
            role: "assistant".to_string(),
            content: if content_text.is_empty() { None } else { Some(content_text) },
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls.clone()) },
            tool_call_id: None,
        });

        if tool_calls.is_empty() || finish_reason == "stop" {
            let _ = event_tx.send(AgentEvent::MessageComplete {
                stop_reason: "end_turn".to_string(),
            }).await;
            break;
        }

        // Execute tool calls
        for tc in &tool_calls {
            let tool_name = &tc.function.name;
            let tool_input: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(json!({}));

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

            // Append tool result to history
            history.push(OaiMessage {
                role: "tool".to_string(),
                content: Some(value_to_tool_string(&output)),
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
            });
        }

        // Continue loop — model will see tool results and decide next action
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
