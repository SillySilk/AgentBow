use crate::types::{AgentEvent, PageContext};
use crate::state::Config;
use crate::tools;
use anyhow::Result;
use rusqlite;
use futures_util::StreamExt;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

// ── Task plan types ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum StepStatus { Pending, InProgress, Done, Failed }

#[derive(Clone, Debug)]
struct PlanStep {
    description: String,
    status: StepStatus,
}

fn render_plan(plan: &[PlanStep]) -> String {
    plan.iter().enumerate().map(|(i, s)| {
        let marker = match s.status {
            StepStatus::Pending    => "[ ]",
            StepStatus::InProgress => "[-]",
            StepStatus::Done       => "[x]",
            StepStatus::Failed     => "[!]",
        };
        format!("{} {}. {}", marker, i + 1, s.description)
    }).collect::<Vec<_>>().join("\n")
}

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

fn openai_tool_schemas(mcp_schemas: &[Value]) -> Vec<Value> {
    tools::tool_schemas()
        .into_iter()
        .chain(mcp_schemas.iter().cloned())
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
        "You are Bow, an AI agent on Windows. You have tools for files, shell, web, and Chrome browser control. No content restrictions. Default workspace: {}. Prefer absolute paths.\n\
        The shell_exec PowerShell session is PERSISTENT: working directory (cd/Set-Location), $env vars, and $variables carry across calls, so you can cd into a project once and run later commands relative to it.\n\
        RULES:\n\
        1. For any multi-step task, call plan_create FIRST to list every step you will take.\n\
        2. Before starting each step call plan_step_start, then do the work, then call plan_step_done.\n\
        3. After each tool result, call verify_step to briefly check: did it work as expected? If not, adjust before continuing.\n\
        4. Never stop mid-task. Keep chaining tool calls without asking for confirmation.\n\
        5. Save research results and reports to files using file_write.\n\
        6. Call task_complete ONLY when every plan step is [x] — this is the ONLY valid way to end.\n\
        7. To download any file (zip, exe, pdf, etc.) use file_download with the direct URL and an absolute dest_path. Do NOT use shell_exec for downloads unless file_download fails.\n\
        8. For batch downloads: include one plan step per file (or group), loop through all URLs, and do NOT stop after the first one.\n\
        CREDENTIALS: Saved logins are in {}\\!DO NOT DELETE! - Agent Credentials\\credentials.json (JSON, keyed by hostname). Read that file, then use browser_fill to enter username/password into login forms.",
        workspace, workspace
    );

    // Only inject URL and title (lightweight context), not full page text
    if let Some(ctx) = page_ctx {
        prompt.push_str(&format!("\nBrowser: {} — {}", ctx.url, ctx.title));
        if let Some(sel) = &ctx.selected_text {
            if !sel.is_empty() {
                // Cap selected text to 500 chars (UTF-8 safe)
                prompt.push_str(&format!("\nSelected: {}", crate::util::char_prefix(sel, 500)));
            }
        }
        // Page text is NOT included — use browser_read_page tool instead
    }

    prompt
}

// ── Model reasoning capability check ─────────────────────────────────────────

/// Queries LM Studio's /api/v1/models endpoint and returns the `reasoning`
/// field for the currently configured model (if present).
/// Returns None silently if the endpoint is unavailable or the model isn't listed.
pub async fn query_model_reasoning(lm_studio_url: &str, model: &str) -> Option<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let resp = client
        .get(format!("{}/api/v1/models", lm_studio_url))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    v["data"]
        .as_array()?
        .iter()
        .find(|m| m["id"].as_str() == Some(model))
        .and_then(|m| m.get("reasoning").cloned())
}

// ── Reasoning fields helper ───────────────────────────────────────────────────

/// Injects reasoning_effort / reasoning_tokens into a mutable JSON body
/// when the config has them set.
fn apply_reasoning_fields(body: &mut serde_json::Value, config: &Config) {
    if let Some(ref effort) = config.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    if let Some(tokens) = config.reasoning_tokens {
        body["reasoning_tokens"] = json!(tokens);
    }
}

// ── Reflexion helper ──────────────────────────────────────────────────────────

async fn generate_reflection(
    config: &Config,
    history: &[OaiMessage],
    system_prompt: &str,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let mut messages = vec![json!({"role": "system", "content": system_prompt})];
    for msg in history {
        messages.push(serde_json::to_value(msg).unwrap_or_default());
    }
    messages.push(json!({
        "role": "user",
        "content": "The task ran out of iterations and did not complete. In 2-3 sentences: what went wrong, what could be done differently, and what should be remembered for next time?"
    }));

    let mut body = json!({
        "model": config.lm_studio_model,
        "messages": messages,
        "max_tokens": 256,
        "temperature": 0.4,
        "stream": false
    });
    apply_reasoning_fields(&mut body, config);

    let resp = client
        .post(format!("{}/v1/chat/completions", config.lm_studio_url))
        .json(&body)
        .send()
        .await?;

    let v: serde_json::Value = resp.json().await?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("No reflection generated.")
        .to_string();
    Ok(text)
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
    mcp: crate::tools::mcp::McpManager,
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

    // Log model reasoning capabilities (best-effort, non-blocking)
    if let Some(reasoning_caps) = query_model_reasoning(&config.lm_studio_url, &config.lm_studio_model).await {
        debug!("Model '{}' reasoning capabilities: {}", config.lm_studio_model, reasoning_caps);
    }

    // Open (or create) the episodic memory DB in the workspace root
    let memory_db = match crate::tools::memory::open_db(&config.workspace_root.to_string_lossy()) {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to open memory DB: {} — continuing without memory", e);
            // Create an in-memory fallback DB so dispatch still has something to pass
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB failed");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS memories (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                    outcome TEXT NOT NULL, task_desc TEXT NOT NULL,
                    findings TEXT NOT NULL, embedding BLOB
                );"
            ).ok();
            std::sync::Arc::new(std::sync::Mutex::new(conn))
        }
    };
    let tools = openai_tool_schemas(mcp.schemas());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut iterations = 0;
    const MAX_ITERATIONS: u8 = 50;
    let mut plan: Vec<PlanStep> = Vec::new();

    loop {
        // Warn model when approaching the iteration limit so it can wrap up gracefully
        if iterations == MAX_ITERATIONS - 5 {
            history.push(OaiMessage {
                role: "user".to_string(),
                content: Some(format!(
                    "WARNING: Only 5 iterations remain (iteration {}/{}). Finish any remaining steps as efficiently as possible and call task_complete soon.",
                    iterations, MAX_ITERATIONS
                )),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        if iterations >= MAX_ITERATIONS || interrupt.load(Ordering::Relaxed) {
            // On max_iterations (not interrupt), generate a reflection and store it
            if !interrupt.load(Ordering::Relaxed) && !history.is_empty() {
                let reflection = generate_reflection(
                    &config,
                    history,
                    &system_prompt,
                ).await;
                if let Ok(ref text) = reflection {
                    // Store in memory for future tasks
                    let findings = vec![text.as_str()];
                    let task_desc = history.first()
                        .and_then(|m| m.content.as_deref())
                        .unwrap_or("unknown task");
                    let _ = crate::tools::memory::memory_store(
                        &memory_db, task_desc, "failure", &findings, &config.lm_studio_url
                    ).await;
                    let _ = event_tx.send(AgentEvent::TextDelta {
                        delta: format!("\n\n**Reflection:** {}", text),
                        message_id: message_id.clone(),
                    }).await;
                }
            }
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
        // Inject live plan state so the model always sees current progress
        if !plan.is_empty() {
            let done = plan.iter().filter(|s| s.status == StepStatus::Done).count();
            messages.push(json!({
                "role": "system",
                "content": format!("Current task plan ({}/{} done):\n{}", done, plan.len(), render_plan(&plan))
            }));
        }
        // Observation masking: keep last MASK_WINDOW tool results verbatim,
        // replace older ones with a placeholder to control context growth.
        const MASK_WINDOW: usize = 6;
        let tool_indices: Vec<usize> = history.iter().enumerate()
            .filter(|(_, m)| m.role == "tool")
            .map(|(i, _)| i)
            .collect();
        let mask_before = tool_indices.len().saturating_sub(MASK_WINDOW);
        let old_tool_indices: std::collections::HashSet<usize> =
            tool_indices.iter().take(mask_before).cloned().collect();

        for (idx, msg) in history.iter().enumerate() {
            if old_tool_indices.contains(&idx) {
                let mut masked = msg.clone();
                masked.content = Some("[masked — older result removed to save context]".to_string());
                messages.push(serde_json::to_value(&masked).unwrap_or_default());
            } else {
                messages.push(serde_json::to_value(msg).unwrap_or_default());
            }
        }

        let mut body = json!({
            "model": config.lm_studio_model,
            "messages": messages,
            "tools": tools,
            "max_tokens": 4096,
            "temperature": 0.7,
            "stream": true
        });
        apply_reasoning_fields(&mut body, &config);

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

        if tool_calls.is_empty() {
            // Model produced text with no tool calls — inject a plan-aware nudge and continue
            let nudge = if plan.is_empty() {
                "You produced text but no tool calls. You MUST keep using tools. Call plan_create now if you haven't, then start executing. Do NOT stop until task_complete is called.".to_string()
            } else {
                let next_step = plan.iter().enumerate().find(|(_, s)| s.status == StepStatus::Pending || s.status == StepStatus::InProgress);
                match next_step {
                    Some((i, s)) => format!(
                        "You produced text but no tool calls. Resume immediately. Next step is step {} ({}). Call plan_step_start({}) then execute it. Do NOT stop until task_complete is called.\nCurrent plan:\n{}",
                        i + 1, s.description, i + 1, render_plan(&plan)
                    ),
                    None => {
                        let failed = plan.iter().any(|s| s.status == StepStatus::Failed);
                        if failed {
                            format!("All steps are done or failed. Review failures and either retry them or call task_complete with a summary.\nCurrent plan:\n{}", render_plan(&plan))
                        } else {
                            format!("All plan steps are [x]. Call task_complete now.\nCurrent plan:\n{}", render_plan(&plan))
                        }
                    }
                }
            };
            history.push(OaiMessage {
                role: "user".to_string(),
                content: Some(nudge),
                tool_calls: None,
                tool_call_id: None,
            });
            continue;
        }

        // ── Check for task_complete before executing ─────────────────────────
        let mut task_done = false;
        let mut done_summary = String::new();
        for tc in &tool_calls {
            if tc.function.name == "task_complete" {
                task_done = true;
                if let Ok(v) = serde_json::from_str::<Value>(&tc.function.arguments) {
                    done_summary = v["summary"].as_str().unwrap_or("").to_string();
                }
                break;
            }
        }

        // ── Execute tool calls ───────────────────────────────────────────────
        // In-loop tools mutate plan state and must run serially.
        // If the batch is pure dispatch tools, run them in parallel.
        const IN_LOOP_TOOLS: &[&str] = &[
            "plan_create", "plan_step_start", "plan_step_done", "plan_step_fail", "verify_step", "task_complete",
        ];
        let has_in_loop = tool_calls.iter().any(|tc| {
            IN_LOOP_TOOLS.contains(&tc.function.name.as_str())
        });

        // ── Parse all inputs first (report errors immediately) ───────────────
        struct ParsedCall {
            id: String,
            name: String,
            input: Result<Value, String>,
        }
        let parsed: Vec<ParsedCall> = tool_calls.iter().map(|tc| {
            let input = serde_json::from_str::<Value>(&tc.function.arguments)
                .map_err(|e| format!(
                    "Tool call error: could not parse arguments for '{}'. Error: {}. Your arguments were: {}. Please try again with valid JSON.",
                    tc.function.name, e, tc.function.arguments
                ));
            ParsedCall { id: tc.id.clone(), name: tc.function.name.clone(), input }
        }).collect();

        // Emit ToolStart for all calls
        for p in &parsed {
            let input_val = p.input.as_ref().map(|v| v.clone()).unwrap_or(Value::Null);
            let _ = event_tx.send(AgentEvent::ToolStart {
                tool_name: p.name.clone(),
                tool_use_id: p.id.clone(),
                input: input_val,
            }).await;
        }

        if has_in_loop {
            // ── Serial path (plan/verify tools) ─────────────────────────────
            for p in &parsed {
                let tool_input = match &p.input {
                    Ok(v) => v.clone(),
                    Err(msg) => {
                        warn!("Malformed args for {}", p.name);
                        let _ = event_tx.send(AgentEvent::ToolResult {
                            tool_use_id: p.id.clone(),
                            output: msg.clone(),
                            is_error: true,
                        }).await;
                        history.push(OaiMessage {
                            role: "tool".to_string(),
                            content: Some(msg.clone()),
                            tool_calls: None,
                            tool_call_id: Some(p.id.clone()),
                        });
                        continue;
                    }
                };

                let (output, is_error) = match p.name.as_str() {
                    "plan_create" => {
                        if let Some(steps) = tool_input["steps"].as_array() {
                            plan = steps.iter()
                                .filter_map(|v| v.as_str())
                                .map(|s| PlanStep { description: s.to_string(), status: StepStatus::Pending })
                                .collect();
                            (json!(format!("Plan created:\n{}", render_plan(&plan))), false)
                        } else {
                            (json!("plan_create: 'steps' must be an array of strings"), true)
                        }
                    }
                    "plan_step_start" => {
                        if let Some(idx) = tool_input["step"].as_u64().map(|n| n as usize).filter(|&n| n >= 1 && n <= plan.len()) {
                            plan[idx - 1].status = StepStatus::InProgress;
                            (json!(format!("Step {} marked in-progress.\n{}", idx, render_plan(&plan))), false)
                        } else {
                            (json!(format!("plan_step_start: step out of range (plan has {} steps)", plan.len())), true)
                        }
                    }
                    "plan_step_done" => {
                        if let Some(idx) = tool_input["step"].as_u64().map(|n| n as usize).filter(|&n| n >= 1 && n <= plan.len()) {
                            plan[idx - 1].status = StepStatus::Done;
                            let done = plan.iter().filter(|s| s.status == StepStatus::Done).count();
                            (json!(format!("Step {} done ({}/{}).\n{}", idx, done, plan.len(), render_plan(&plan))), false)
                        } else {
                            (json!(format!("plan_step_done: step out of range (plan has {} steps)", plan.len())), true)
                        }
                    }
                    "plan_step_fail" => {
                        if let Some(idx) = tool_input["step"].as_u64().map(|n| n as usize).filter(|&n| n >= 1 && n <= plan.len()) {
                            plan[idx - 1].status = StepStatus::Failed;
                            let reason = tool_input["reason"].as_str().unwrap_or("unspecified");
                            (json!(format!("Step {} marked failed: {}\n{}", idx, reason, render_plan(&plan))), false)
                        } else {
                            (json!(format!("plan_step_fail: step out of range (plan has {} steps)", plan.len())), true)
                        }
                    }
                    "verify_step" => {
                        let ok = tool_input["ok"].as_bool().unwrap_or(true);
                        let msg = if ok {
                            format!("✓ Verified: {}", tool_input["actual"].as_str().unwrap_or("ok"))
                        } else {
                            format!(
                                "✗ Mismatch — expected: {} | got: {} | correction: {}",
                                tool_input["expected"].as_str().unwrap_or("?"),
                                tool_input["actual"].as_str().unwrap_or("?"),
                                tool_input["correction"].as_str().unwrap_or("retry")
                            )
                        };
                        (json!(msg), false)
                    }
                    _ if mcp.is_mcp_tool(&p.name) => match mcp.dispatch(&p.name, &tool_input).await {
                        Ok(r) => (r, false),
                        Err(e) => (json!(e.to_string()), true),
                    },
                    _ => match tools::dispatch(
                        &p.name,
                        &tool_input,
                        &config.tavily_api_key,
                        &config.lm_studio_url,
                        &config.lm_studio_model,
                        &config.workspace_root.to_string_lossy(),
                        &config.searxng_url,
                        &shell_session,
                        &browser,
                        &memory_db,
                    ).await {
                        Ok(r) => (r, false),
                        Err(e) => (json!(e.to_string()), true),
                    },
                };

                let _ = event_tx.send(AgentEvent::ToolResult {
                    tool_use_id: p.id.clone(),
                    output: output.to_string(),
                    is_error,
                }).await;
                history.push(OaiMessage {
                    role: "tool".to_string(),
                    content: Some(value_to_tool_string(&output)),
                    tool_calls: None,
                    tool_call_id: Some(p.id.clone()),
                });
            }
        } else {
            // ── Parallel dispatch path ───────────────────────────────────────
            let futures: Vec<_> = parsed.iter().map(|p| {
                let name = p.name.clone();
                let id = p.id.clone();
                let input_res = p.input.clone();
                let tavily = config.tavily_api_key.clone();
                let lm_url = config.lm_studio_url.clone();
                let lm_model = config.lm_studio_model.clone();
                let ws_root = config.workspace_root.to_string_lossy().to_string();
                let searxng = config.searxng_url.clone();
                let sess = shell_session.clone();
                let brow = browser.clone();
                let mem = memory_db.clone();
                let mcp = mcp.clone();
                async move {
                    let tool_input = match input_res {
                        Ok(v) => v,
                        Err(msg) => return (id, json!(msg), true),
                    };
                    let result = if mcp.is_mcp_tool(&name) {
                        mcp.dispatch(&name, &tool_input).await
                    } else {
                        tools::dispatch(
                            &name, &tool_input, &tavily, &lm_url, &lm_model, &ws_root,
                            &searxng, &sess, &brow, &mem,
                        ).await
                    };
                    match result {
                        Ok(v) => (id, v, false),
                        Err(e) => (id, json!(e.to_string()), true),
                    }
                }
            }).collect();

            let results = join_all(futures).await;

            for (id, output, is_error) in results {
                let _ = event_tx.send(AgentEvent::ToolResult {
                    tool_use_id: id.clone(),
                    output: output.to_string(),
                    is_error,
                }).await;
                history.push(OaiMessage {
                    role: "tool".to_string(),
                    content: Some(value_to_tool_string(&output)),
                    tool_calls: None,
                    tool_call_id: Some(id),
                });
            }
        }

        if task_done {
            let summary = if done_summary.is_empty() {
                "Task complete.".to_string()
            } else {
                done_summary
            };
            let _ = event_tx.send(AgentEvent::TextDelta {
                delta: format!("\n\n{}", summary),
                message_id: message_id.clone(),
            }).await;
            let _ = event_tx.send(AgentEvent::MessageComplete {
                stop_reason: "task_complete".to_string(),
            }).await;
            break;
        }
    }

    // Trim history, keeping the most recent messages without corrupting the
    // assistant→tool grouping the OpenAI API requires.
    trim_history(history, 40);

    Ok(())
}

/// Trim `history` from the front to at most `max` messages.
///
/// The OpenAI-compatible API rejects a conversation where a `tool` message is
/// not preceded by the `assistant` message carrying its matching `tool_calls`.
/// A naive front-trim can remove that assistant message while leaving its tool
/// responses behind, producing leading orphan `tool` messages and a 400 on the
/// next turn. We advance the cut point past any such orphans.
fn trim_history(history: &mut Vec<OaiMessage>, max: usize) {
    if history.len() <= max {
        return;
    }
    let mut drop = history.len() - max;
    while drop < history.len() && history[drop].role == "tool" {
        drop += 1;
    }
    history.drain(0..drop);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str) -> OaiMessage {
        OaiMessage {
            role: role.to_string(),
            content: Some(String::new()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn trim_never_leaves_leading_tool_message() {
        // assistant(tool_calls) → tool → tool → user → assistant ...
        let roles = [
            "user", "assistant", "tool", "tool", "user", "assistant",
            "tool", "user", "assistant", "tool", "tool", "tool",
        ];
        let mut history: Vec<OaiMessage> = roles.iter().map(|r| msg(r)).collect();
        trim_history(&mut history, 5);
        assert!(history.len() <= 7); // may keep a few extra to avoid orphan tools
        assert_ne!(history.first().unwrap().role, "tool",
            "history must not start with an orphan tool message");
    }

    #[test]
    fn trim_noop_when_short() {
        let mut history: Vec<OaiMessage> = vec![msg("user"), msg("assistant")];
        trim_history(&mut history, 40);
        assert_eq!(history.len(), 2);
    }
}
