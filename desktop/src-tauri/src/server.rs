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
    ScrapeRequest {
        query: String,
        count: u32,
        dest_dir: String,
        #[serde(default)] sources: Option<Vec<String>>,
        /// Delay between downloads (ms). 0 + verify=false ⇒ fast concurrent path.
        #[serde(default)] delay_ms: u64,
        /// Run the vision-QA inline keep/discard gate.
        #[serde(default)] verify: bool,
        /// Optional override for the vision judging prompt.
        #[serde(default)] vision_prompt: Option<String>,
        /// Target a specific bin (1–10). None ⇒ auto-pick the lowest empty bin.
        #[serde(default)] bin: Option<u32>,
        /// Skip images that perceptually match ones already in the bin or this run.
        #[serde(default = "default_true")] dedupe: bool,
        /// Animus Sorter category (`Character`/`Object`/`Style`). `None` ⇒ legacy naming.
        #[serde(default)] category: Option<String>,
    },
    BrowserOpen { url: String },
    PageScrapeRequest { count: u32, dest_dir: String, #[serde(default)] scrolls: u32 },
    /// Cooperatively stop any in-flight scrape (query or page). Downloads already
    /// completed are kept; the run finishes with a normal `done` event.
    StopScrape,
    // ── Case the gallery (guided grab) ──
    /// Extract structured candidates from the current controlled-browser page.
    CaseExtract,
    /// Follow a thumbnail's link to its detail page and extract that page's candidates.
    CaseOpenDetail { href: String },
    /// Build a recipe from a demonstrated grid example (+ optional detail image).
    CaseGeneralize {
        example_id: usize,
        #[serde(default)] detail_image_id: Option<usize>,
        #[serde(default)] scrolls: u32,
    },
    /// Replay a recipe over `grid_url`, download the batch.
    CaseRun {
        recipe: crate::tools::recipe::Recipe,
        grid_url: String,
        count: u32,
        dest_dir: String,
    },
    /// Persist a recipe to the per-domain playbook store.
    PlaybookSave { recipe: crate::tools::recipe::Recipe },
    /// List saved recipes for a domain.
    PlaybookList { domain: String },
}

/// serde default for the dedup flag (on unless the client explicitly disables it).
fn default_true() -> bool { true }

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
    llm_engine: crate::llm_engine::LlmEngine,
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
    // Case-the-gallery scratch state (per connection): the last grid extraction, the
    // last detail-page extraction, and the URL the grid was cased from. Kept separate
    // so a detail extraction never clobbers the grid the example id refers to.
    let mut grid_candidates: Vec<crate::tools::recipe::Candidate> = Vec::new();
    let mut detail_candidates: Vec<crate::tools::recipe::Candidate> = Vec::new();
    let mut grid_url: String = String::new();
    let interrupt_flag = Arc::new(AtomicBool::new(false));
    // Cooperative stop signal for scrapes (query + page). Set by `stop_scrape`,
    // reset when a fresh scrape starts, checked between downloads by the scraper.
    let scrape_cancel = Arc::new(AtomicBool::new(false));
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
                        let engine_clone = llm_engine.clone();
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
                                local_llm::ChatRuntime {
                                    config,
                                    engine: engine_clone,
                                    shell_session: shell_session_clone,
                                    browser: browser_clone,
                                    mcp: mcp_clone,
                                },
                                &mut hist, content, message_id,
                                ctx_snapshot, interrupt, event_tx,
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

                    InboundMsg::StopScrape => {
                        scrape_cancel.store(true, Ordering::Relaxed);
                    }

                    InboundMsg::ScrapeRequest { query, count, dest_dir, sources, delay_ms, verify, vision_prompt, bin, dedupe, category } => {
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
                        // Resolve the target bin: a manual 1–10 choice (resume/append, even if
                        // non-empty) or auto-pick the lowest empty bin (error if all ten are full).
                        let bin_result = match bin {
                            Some(n) => crate::tools::image_search::resolve_manual_bin(&dest_dir, n),
                            None => crate::tools::image_search::pick_auto_bin(&dest_dir),
                        };
                        let dest_dir = match bin_result {
                            Ok(p) => p,
                            Err(e) => {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message": format!("bin: {}", e)});
                                let _ = out_tx.send(err.to_string()).await;
                                continue;
                            }
                        };
                        let _ = out_tx.send(serde_json::json!({"type":"scrape_event","kind":"phase","label": format!("Set folder: {}", dest_dir)}).to_string()).await;
                        // Clamp count to a sane bound (Fix 4).
                        let count = (count as usize).clamp(1, 500);
                        // Clamp pacing to a sane ceiling (0–30s between downloads).
                        let delay_ms = delay_ms.min(30_000);
                        // Resolve the embedded engine's endpoint once per scrape. Only the
                        // vision-QA gate needs a model — a plain scrape proceeds without one.
                        let st = llm_engine.status().await;
                        let (llm_base_url, llm_model, vision) = match (st.base_url.clone(), st.model.as_ref().map(|m| m.name.clone())) {
                            (Some(b), Some(m)) if st.state == "ready" => (b, m, st.vision),
                            _ if verify => {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message": st.not_ready_message()});
                                let _ = out_tx.send(err.to_string()).await;
                                continue;
                            }
                            // Engine not ready but verify is off: the LLM fields are unused
                            // on this path, so scrape anyway.
                            _ => (String::new(), String::new(), false),
                        };
                        let tuning = crate::tools::image_search::ScrapeTuning {
                            delay_ms,
                            verify,
                            vision_prompt,
                            llm_base_url,
                            llm_model,
                            vision,
                            dedupe,
                            sources,
                            category,
                        };
                        let out_tx = out_tx.clone();
                        let cb = controlled_browser.clone();
                        let log_dir = format!("{}\\logs", config.workspace_root.to_string_lossy().trim_end_matches(['\\', '/']));
                        // Fresh scrape: clear any prior stop request, then hand a clone to the task.
                        scrape_cancel.store(false, Ordering::Relaxed);
                        let cancel = scrape_cancel.clone();
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
                                &query, count, &dest_dir, &log_dir, tuning, &cb, Some(tx), Some(cancel),
                            ).await;
                            // tx dropped here → forwarder drains and exits.
                            let _ = forwarder.await;
                            if let Err(e) = result {
                                let err = serde_json::json!({"type":"scrape_event","kind":"error","message": e.to_string()});
                                let _ = out_tx.send(err.to_string()).await;
                            }
                        });
                    }

                    InboundMsg::BrowserOpen { url } => {
                        if !authenticated {
                            send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await;
                            continue;
                        }
                        let cb = controlled_browser.clone();
                        let out_tx = out_tx.clone();
                        tokio::spawn(async move {
                            let msg = match cb.navigate(&url).await {
                                Ok(_) => serde_json::json!({"type":"browser_opened","url": url}),
                                Err(e) => serde_json::json!({"type":"scrape_event","kind":"error","message": format!("browser_open: {}", e)}),
                            };
                            let _ = out_tx.send(msg.to_string()).await;
                        });
                    }

                    InboundMsg::PageScrapeRequest { count, dest_dir, scrolls } => {
                        if !authenticated {
                            send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await;
                            continue;
                        }
                        let cb = controlled_browser.clone();
                        let out_tx = out_tx.clone();
                        let workspace = config.workspace_root.clone();
                        let log_dir = format!("{}\\logs", workspace.to_string_lossy().trim_end_matches(['\\', '/']));
                        let count = (count as usize).clamp(1, 500);
                        // Fresh scrape: clear any prior stop request, then hand a clone to the task.
                        scrape_cancel.store(false, Ordering::Relaxed);
                        let cancel = scrape_cancel.clone();
                        tokio::spawn(async move {
                            let dest = match crate::web_api::resolve_within_workspace(&workspace, &dest_dir) {
                                Some(p) => p.to_string_lossy().to_string(),
                                None => {
                                    let _ = out_tx.send(serde_json::json!({"type":"scrape_event","kind":"error","message":"dest_dir outside workspace"}).to_string()).await;
                                    return;
                                }
                            };
                            // Auto-select a bin per page-scrape too (see ScrapeRequest).
                            let dest = match crate::tools::image_search::pick_auto_bin(&dest) {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = out_tx.send(serde_json::json!({"type":"scrape_event","kind":"error","message": format!("set folder: {}", e)}).to_string()).await;
                                    return;
                                }
                            };
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            let fwd = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    let mut v = ev.to_json();
                                    v["type"] = serde_json::Value::String("scrape_event".into());
                                    let _ = fwd.send(v.to_string()).await;
                                }
                            });
                            if scrolls > 0 {
                                let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Scrolling page".into() });
                            }
                            for _ in 0..scrolls {
                                let _ = cb.scroll("down", 1200).await;
                                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                            }
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Extracting images".into() });
                            let urls = cb.extract_image_urls().await.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Candidates { total: urls.len(), filtered: 0 });
                            let mut log = crate::tools::image_search::SessionLog::new(&log_dir, "page_scrape");
                            let result = crate::tools::image_search::download_urls_to_dir(
                                urls, count, &dest, "page",
                                crate::tools::image_search::DownloadOpts::default(),
                                &mut log, &Some(tx.clone()), Some(cancel),
                            ).await;
                            let log_note = log.flush();
                            let downloaded = result.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Done { downloaded, log_note, dest_dir: dest.clone() });
                            drop(tx);
                            let _ = forwarder.await;
                        });
                    }

                    InboundMsg::CaseExtract => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        match controlled_browser.extract_candidates().await {
                            Ok(cands) => {
                                grid_url = controlled_browser.get_url().await.ok()
                                    .and_then(|v| v["url"].as_str().map(str::to_string)).unwrap_or_default();
                                grid_candidates = cands.clone();
                                send_json(&out_tx, json!({"type":"case_candidates","stage":"grid","items":cands})).await;
                            }
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("case_extract: {} — open the browser first with Ghost car", e)})).await,
                        }
                    }

                    InboundMsg::CaseOpenDetail { href } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        if let Err(e) = controlled_browser.navigate(&href).await {
                            send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("open detail: {}", e)})).await;
                            continue;
                        }
                        match controlled_browser.extract_candidates().await {
                            Ok(cands) => { detail_candidates = cands.clone(); send_json(&out_tx, json!({"type":"case_candidates","stage":"detail","items":cands})).await; }
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("detail extract: {}", e)})).await,
                        }
                    }

                    InboundMsg::CaseGeneralize { example_id, detail_image_id, scrolls } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let detail_sel = detail_image_id
                            .and_then(|id| detail_candidates.iter().find(|c| c.id == id))
                            .map(crate::tools::recipe::detail_selector_from);
                        let domain = crate::tools::recipe::domain_of(&grid_url);
                        match grid_candidates.iter().find(|c| c.id == example_id).cloned() {
                            Some(example) => {
                                let (mut recipe, sibs) = crate::tools::recipe::generalize(&example, &grid_candidates, scrolls, &domain);
                                recipe.detail_image_selector = detail_sel;
                                send_json(&out_tx, json!({"type":"case_recipe","recipe":recipe,"matched":sibs.len(),"total":grid_candidates.len(),"grid_url":grid_url})).await;
                            }
                            None => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message":"example not found — re-run Case it"})).await,
                        }
                    }

                    InboundMsg::PlaybookSave { recipe } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let dir = config.workspace_root.join("playbooks");
                        match crate::tools::recipe::save_playbook(&dir, &recipe) {
                            Ok(_) => send_json(&out_tx, json!({"type":"playbook_saved","domain":recipe.domain})).await,
                            Err(e) => send_json(&out_tx, json!({"type":"scrape_event","kind":"error","message": format!("save playbook: {}", e)})).await,
                        }
                    }

                    InboundMsg::PlaybookList { domain } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let dir = config.workspace_root.join("playbooks");
                        let recipes = crate::tools::recipe::load_playbooks(&dir, &domain);
                        send_json(&out_tx, json!({"type":"playbook_list","domain":domain,"recipes":recipes})).await;
                    }

                    InboundMsg::CaseRun { recipe, grid_url: run_url, count, dest_dir } => {
                        if !authenticated { send_json(&out_tx, json!({"type":"error","code":"unauthenticated","message":"Must authenticate first"})).await; continue; }
                        let cb = controlled_browser.clone();
                        let out_tx = out_tx.clone();
                        let workspace = config.workspace_root.clone();
                        let log_dir = format!("{}\\logs", workspace.to_string_lossy().trim_end_matches(['\\', '/']));
                        let count = (count as usize).clamp(1, 500);
                        scrape_cancel.store(false, Ordering::Relaxed);
                        let cancel = scrape_cancel.clone();
                        tokio::spawn(async move {
                            let dest = match crate::web_api::resolve_within_workspace(&workspace, &dest_dir) {
                                Some(p) => p.to_string_lossy().to_string(),
                                None => { let _ = out_tx.send(json!({"type":"scrape_event","kind":"error","message":"dest_dir outside workspace"}).to_string()).await; return; }
                            };
                            let dest = match crate::tools::image_search::pick_auto_bin(&dest) {
                                Ok(p) => p,
                                Err(e) => { let _ = out_tx.send(json!({"type":"scrape_event","kind":"error","message": format!("set folder: {}", e)}).to_string()).await; return; }
                            };
                            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::tools::image_search::ScrapeEvent>();
                            let fwd = out_tx.clone();
                            let forwarder = tokio::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    let mut v = ev.to_json();
                                    v["type"] = Value::String("scrape_event".into());
                                    let _ = fwd.send(v.to_string()).await;
                                }
                            });
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Phase { label: "Casing gallery".into() });
                            let _ = cb.navigate(&run_url).await;
                            for _ in 0..recipe.scrolls {
                                let _ = cb.scroll("down", 1200).await;
                                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                            }
                            let grid = cb.extract_candidates().await.unwrap_or_default();
                            let items: Vec<crate::tools::recipe::Candidate> =
                                crate::tools::recipe::match_pattern(&recipe.grid_selector, &grid).into_iter().cloned().collect();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Candidates { total: items.len(), filtered: grid.len().saturating_sub(items.len()) });

                            let mut urls: Vec<String> = Vec::new();
                            if recipe.link_selector.is_some() {
                                let detail_sel = recipe.detail_image_selector.clone().unwrap_or_default();
                                for it in &items {
                                    if crate::tools::image_search::cancel_check(&Some(cancel.clone())) { break; }
                                    let Some(href) = &it.href else { continue };
                                    if cb.navigate(href).await.is_err() { continue; }
                                    let dcands = cb.extract_candidates().await.unwrap_or_default();
                                    let best = dcands.iter()
                                        .filter(|c| detail_sel.is_empty() || crate::tools::recipe::structural_pattern(&c.selector) == detail_sel)
                                        .max_by_key(|c| c.w as u64 * c.h as u64);
                                    if let Some(b) = best { urls.push(b.preview_url.clone()); }
                                }
                            } else {
                                urls.extend(items.iter().map(|c| c.preview_url.clone()));
                            }

                            let mut log = crate::tools::image_search::SessionLog::new(&log_dir, "case_run");
                            let result = crate::tools::image_search::download_urls_to_dir(
                                urls, count, &dest, "case", crate::tools::image_search::DownloadOpts::default(),
                                &mut log, &Some(tx.clone()), Some(cancel),
                            ).await;
                            let log_note = log.flush();
                            let downloaded = result.unwrap_or_default();
                            let _ = tx.send(crate::tools::image_search::ScrapeEvent::Done { downloaded, log_note, dest_dir: dest.clone() });
                            drop(tx);
                            let _ = forwarder.await;
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
            InboundMsg::ScrapeRequest { query, count, dest_dir, sources, delay_ms, verify, bin, dedupe, .. } => {
                assert_eq!(query, "cats");
                assert_eq!(count, 15);
                assert_eq!(dest_dir, "C:\\x");
                assert!(sources.is_none());
                assert_eq!(delay_ms, 0);
                assert!(!verify);
                assert!(bin.is_none(), "bin defaults to None (auto)");
                assert!(dedupe, "dedupe defaults to ON");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn case_messages_parse() {
        let a: InboundMsg = serde_json::from_value(json!({"type":"case_extract"})).unwrap();
        assert!(matches!(a, InboundMsg::CaseExtract));
        let b: InboundMsg = serde_json::from_value(json!({"type":"case_generalize","example_id":3})).unwrap();
        assert!(matches!(b, InboundMsg::CaseGeneralize { example_id: 3, detail_image_id: None, scrolls: 0 }));
        let c: InboundMsg = serde_json::from_value(json!({"type":"case_run","grid_url":"https://e.com/g","count":20,"dest_dir":"C:\\x",
            "recipe":{"domain":"e.com","grid_selector":"div > a > img","link_selector":"div > a > img","detail_image_selector":"main > img","scrolls":3}})).unwrap();
        assert!(matches!(c, InboundMsg::CaseRun { count: 20, .. }));
    }

    #[test]
    fn browser_open_and_page_scrape_parse() {
        let a: InboundMsg = serde_json::from_value(serde_json::json!({"type":"browser_open","url":"https://x"})).unwrap();
        assert!(matches!(a, InboundMsg::BrowserOpen { .. }));
        let b: InboundMsg = serde_json::from_value(serde_json::json!({"type":"page_scrape_request","count":20,"dest_dir":"C:\\x","scrolls":3})).unwrap();
        match b {
            InboundMsg::PageScrapeRequest { count, scrolls, .. } => {
                assert_eq!(count, 20);
                assert_eq!(scrolls, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn stop_scrape_parses() {
        let v: InboundMsg = serde_json::from_value(serde_json::json!({"type":"stop_scrape"})).unwrap();
        assert!(matches!(v, InboundMsg::StopScrape));
    }
}
