use crate::local_llm::{self, OaiMessage};
use crate::types::{AgentEvent, PageContext};
use crate::auth;
use anyhow::Result;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum InboundMsg {
    Auth { token: String, session_id: String },
    PageContext {
        url: String,
        title: String,
        selected_text: Option<String>,
        page_text: Option<String>,
    },
    UserMessage { content: String, message_id: String },
    Interrupt { session_id: String },
    ScrapeRequest { query: String, count: u32, dest_dir: String, #[serde(default)] sources: Option<Vec<String>> },
}

/// Classify a raw inbound WS text frame before full deserialization.
/// Returns None for control frames the loop should skip (e.g. ping).
#[derive(Debug, PartialEq)]
pub enum Inbound { Skip, Process }

pub fn classify(raw: &serde_json::Value) -> Inbound {
    match raw["type"].as_str() {
        Some("ping") => Inbound::Skip,
        _ => Inbound::Process,
    }
}

pub async fn run_ws(
    socket: WebSocket,
    config: Arc<crate::state::Config>,
    shell_session: crate::tools::shell_session::ShellSessionManager,
    controlled_browser: crate::tools::controlled_browser::ControlledBrowser,
    mcp: crate::tools::mcp::McpManager,
) -> Result<()> {
    let (mut ws_sink, mut ws_source) = socket.split();

    let (out_tx, mut out_rx) = mpsc::channel::<String>(128);

    let sink_handle = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if ws_sink.send(WsMessage::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let mut authenticated = false;
    let mut history: Vec<OaiMessage> = Vec::new();
    let mut page_ctx: Option<PageContext> = None;
    let interrupt_flag = Arc::new(AtomicBool::new(false));
    // Guards against two agent runs racing on the same `history`. Each run
    // snapshots history and writes it back on completion; concurrent runs would
    // silently clobber each other (last writer wins).
    let busy = Arc::new(AtomicBool::new(false));

    let (hist_tx, mut hist_rx) = mpsc::channel::<Vec<OaiMessage>>(4);

    loop {
        tokio::select! {
            msg = ws_source.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ => break,
                };

                let text = match msg {
                    WsMessage::Text(t) => t,
                    WsMessage::Close(_) => break,
                    _ => continue,
                };

                let raw: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Invalid JSON: {}", e);
                        continue;
                    }
                };

                if classify(&raw) == Inbound::Skip {
                    continue;
                }

                let inbound: InboundMsg = match serde_json::from_value(raw) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Invalid message: {}", e);
                        continue;
                    }
                };

                match inbound {
                    InboundMsg::Auth { token, session_id: _ } => {
                        if auth::validate_token(&token, &config.bow_secret) {
                            authenticated = true;
                            send_json(&out_tx, json!({"type": "auth_ok"})).await;
                            info!("Auth OK");
                        } else {
                            send_json(&out_tx, json!({"type": "auth_error", "message": "Invalid token"})).await;
                            warn!("Auth failed");
                            break;
                        }
                    }

                    InboundMsg::PageContext { url, title, selected_text, page_text } => {
                        page_ctx = Some(PageContext { url, title, selected_text, page_text });
                    }

                    InboundMsg::UserMessage { content, message_id } => {
                        if !authenticated {
                            send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await;
                            continue;
                        }

                        // Reject a second message while one is still running —
                        // concurrent runs would corrupt the shared history.
                        if busy.swap(true, Ordering::SeqCst) {
                            send_json(&out_tx, json!({"type":"error","code":"busy","message":"Still working on the previous message — interrupt it first or wait for it to finish."})).await;
                            continue;
                        }

                        interrupt_flag.store(false, Ordering::Relaxed);
                        info!("Processing: {}...", crate::util::char_prefix(&content, 60));

                        let config = config.clone();
                        let ctx_snapshot = page_ctx.clone();
                        let interrupt = interrupt_flag.clone();
                        let out = out_tx.clone();
                        let hist_snapshot = history.clone();
                        let htx = hist_tx.clone();
                        let shell_session_clone = shell_session.clone();
                        let browser_clone = controlled_browser.clone();
                        let mcp_clone = mcp.clone();
                        let busy_clone = busy.clone();

                        tokio::spawn(async move {
                            let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(128);

                            let out_fwd = out.clone();
                            let fwd = tokio::spawn(async move {
                                while let Some(evt) = event_rx.recv().await {
                                    send_json(&out_fwd, agent_event_to_json(evt)).await;
                                }
                            });

                            let mut hist = hist_snapshot;
                            if let Err(e) = local_llm::run_local_chat(
                                config, &mut hist, content, message_id,
                                ctx_snapshot, interrupt, event_tx, shell_session_clone,
                                browser_clone, mcp_clone,
                            ).await {
                                error!("local_llm: {}", e);
                            }

                            fwd.await.ok();
                            let _ = htx.send(hist).await;
                            // Release the guard so the next message can run.
                            busy_clone.store(false, Ordering::SeqCst);
                        });
                    }

                    InboundMsg::Interrupt { session_id: _ } => {
                        interrupt_flag.store(true, Ordering::Relaxed);
                    }

                    InboundMsg::ScrapeRequest { query, count, dest_dir, sources } => {
                        if !authenticated {
                            send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await;
                            continue;
                        }
                        // Guard dest_dir to the workspace (Fix 1).
                        let dest_dir = match crate::web_api::resolve_within_workspace(&config.workspace_root, &dest_dir) {
                            Some(p) => p.to_string_lossy().to_string(),
                            None => {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message":"dest_dir is outside the workspace"});
                                let _ = out_tx.send(err.to_string()).await;
                                continue;
                            }
                        };
                        // Clamp count to a sane bound (Fix 4).
                        let count = (count as usize).clamp(1, 500);
                        let out_tx = out_tx.clone();
                        let log_dir = format!("{}\\logs", config.workspace_root.to_string_lossy().trim_end_matches(['\\', '/']));
                        tokio::spawn(async move {
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            // Forward events to the client as they arrive.
                            let fwd_tx = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    let mut v = ev.to_json();
                                    v["type"] = serde_json::Value::String("scrape_event".into());
                                    let _ = fwd_tx.send(v.to_string()).await;
                                }
                            });
                            let result = crate::tools::image_search::image_download(
                                &query, count, &dest_dir, &log_dir, sources, Some(tx),
                            ).await;
                            // tx dropped here → forwarder drains and exits.
                            let _ = forwarder.await;
                            if let Err(e) = result {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message": e.to_string()});
                                let _ = out_tx.send(err.to_string()).await;
                            }
                        });
                    }
                }
            }

            Some(h) = hist_rx.recv() => {
                history = h;
            }
        }
    }

    sink_handle.abort();
    Ok(())
}

fn agent_event_to_json(evt: AgentEvent) -> Value {
    match evt {
        AgentEvent::TextDelta { delta, message_id } =>
            json!({"type":"text_delta","delta":delta,"message_id":message_id}),
        AgentEvent::ToolStart { tool_name, tool_use_id, input } =>
            json!({"type":"tool_start","tool_name":tool_name,"tool_use_id":tool_use_id,"input":input}),
        AgentEvent::ToolResult { tool_use_id, output, is_error } =>
            json!({"type":"tool_result","tool_use_id":tool_use_id,"output":output,"is_error":is_error}),
        AgentEvent::MessageComplete { stop_reason } =>
            json!({"type":"message_complete","stop_reason":stop_reason}),
        AgentEvent::Error { code, message } =>
            json!({"type":"error","code":code,"message":message}),
    }
}

async fn send_json(tx: &mpsc::Sender<String>, value: Value) {
    if let Ok(s) = serde_json::to_string(&value) {
        let _ = tx.send(s).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ping_is_skipped() {
        assert_eq!(classify(&json!({"type":"ping"})), Inbound::Skip);
    }
    #[test]
    fn user_message_is_processed() {
        assert_eq!(classify(&json!({"type":"user_message","content":"hi"})), Inbound::Process);
    }
    #[test]
    fn scrape_request_parses() {
        let v = serde_json::json!({"type":"scrape_request","query":"cats","count":15,"dest_dir":"C:\\x"});
        let parsed: InboundMsg = serde_json::from_value(v).unwrap();
        match parsed {
            InboundMsg::ScrapeRequest { query, count, dest_dir, sources } => {
                assert_eq!(query, "cats");
                assert_eq!(count, 15);
                assert_eq!(dest_dir, "C:\\x");
                assert!(sources.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }
}
