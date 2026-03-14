use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Browser page context sent by the Chrome extension on tab change/load.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PageContext {
    pub url: String,
    pub title: String,
    pub selected_text: Option<String>,
    pub page_text: Option<String>,
}

/// Events emitted by the LLM loop, forwarded over WebSocket to the extension.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta { delta: String, message_id: String },
    ToolStart { tool_name: String, tool_use_id: String, input: Value },
    ToolResult { tool_use_id: String, output: String, is_error: bool },
    MessageComplete { stop_reason: String },
    Error { code: String, message: String },
}
