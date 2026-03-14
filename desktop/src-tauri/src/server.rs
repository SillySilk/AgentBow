use crate::anthropic::{self, AgentEvent, AnthropicMessage, PageContext};
use crate::auth;
use crate::local_llm::{self, OaiMessage};
use crate::router;
use crate::state::AppState;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
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
}

pub async fn start(state: AppState) -> Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], state.config.ws_port));

    // Use SO_REUSEADDR so we can bind even if stale connections linger from a killed instance
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    let listener = TcpListener::from_std(std_listener)?;
    info!("WebSocket server listening on ws://{}", addr);

    let config = Arc::new(state.config);
    let shell_session = state.shell_session;

    while let Ok((stream, peer)) = listener.accept().await {
        info!("New connection from {}", peer);
        let config = config.clone();
        let shell_session = shell_session.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, config, shell_session).await {
                error!("Connection error from {}: {}", peer, e);
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    config: Arc<crate::state::Config>,
    shell_session: crate::tools::shell_session::ShellSessionManager,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    let (out_tx, mut out_rx) = mpsc::channel::<String>(128);

    let sink_handle = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if ws_sink.send(WsMessage::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let browser = crate::tools::browser::BrowserBridge::new(out_tx.clone());

    let mut authenticated = false;
    let mut claude_history: Vec<AnthropicMessage> = Vec::new();
    let mut local_history: Vec<OaiMessage> = Vec::new();
    let mut page_ctx: Option<PageContext> = None;
    let interrupt_flag = Arc::new(AtomicBool::new(false));

    // Channels for history returned from background tasks
    let (claude_hist_tx, mut claude_hist_rx) = mpsc::channel::<Vec<AnthropicMessage>>(4);
    let (local_hist_tx, mut local_hist_rx) = mpsc::channel::<Vec<OaiMessage>>(4);

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

                // Parse to Value first so we can handle browser_result (which
                // has spread fields rather than a nested object) before routing
                // to the typed InboundMsg enum.
                let raw: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Invalid JSON: {}", e);
                        continue;
                    }
                };

                // Silently ignore keepalive pings from the extension
                if raw["type"].as_str() == Some("ping") {
                    continue;
                }

                if raw["type"].as_str() == Some("browser_result") {
                    if let Some(request_id) = raw["request_id"].as_str() {
                        let mut pending = browser.pending.lock().await;
                        if let Some(tx) = pending.remove(request_id) {
                            let _ = tx.send(raw.clone());
                        }
                    }
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

                        interrupt_flag.store(false, Ordering::Relaxed);

                        let use_local = router::should_use_local(&content);
                        info!("Processing: {}...", &content[..content.len().min(60)]);

                        let config = config.clone();
                        let ctx_snapshot = page_ctx.clone();
                        let interrupt = interrupt_flag.clone();
                        let out = out_tx.clone();

                        if use_local {
                            let hist_snapshot = local_history.clone();
                            let htx = local_hist_tx.clone();
                            let shell_session_local = shell_session.clone();
                            let browser_local = browser.clone();

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
                                    ctx_snapshot, interrupt, event_tx, shell_session_local,
                                    browser_local,
                                ).await {
                                    error!("local_llm: {}", e);
                                }

                                fwd.await.ok();
                                let _ = htx.send(hist).await;
                            });
                        } else {
                            let hist_snapshot = claude_history.clone();
                            let htx = claude_hist_tx.clone();
                            let shell_session_clone = shell_session.clone();
                            let browser_claude = browser.clone();

                            tokio::spawn(async move {
                                let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(128);

                                let out_fwd = out.clone();
                                let fwd = tokio::spawn(async move {
                                    while let Some(evt) = event_rx.recv().await {
                                        send_json(&out_fwd, agent_event_to_json(evt)).await;
                                    }
                                });

                                let mut hist = hist_snapshot;
                                if let Err(e) = anthropic::run_chat(
                                    config, &mut hist, content, message_id,
                                    ctx_snapshot, interrupt, event_tx, shell_session_clone,
                                    browser_claude,
                                ).await {
                                    error!("anthropic: {}", e);
                                }

                                fwd.await.ok();
                                let _ = htx.send(hist).await;
                            });
                        }
                    }

                    InboundMsg::Interrupt { session_id: _ } => {
                        interrupt_flag.store(true, Ordering::Relaxed);
                    }
                }
            }

            Some(h) = claude_hist_rx.recv() => {
                claude_history = h;
            }

            Some(h) = local_hist_rx.recv() => {
                local_history = h;
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
