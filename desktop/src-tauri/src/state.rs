use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crate::tools::shell_session::ShellSessionManager;
use crate::tools::controlled_browser::ControlledBrowser;
use crate::llm_engine::LlmEngine;

#[derive(Debug, Clone)]
pub struct Config {
    pub tavily_api_key: String,
    pub bow_secret: String,
    pub ws_port: u16,
    pub workspace_root: PathBuf,
    pub lm_studio_url: String,
    pub lm_studio_model: String,
    /// Optional manual override for the image-QA vision model. When empty (the
    /// default), the app auto-detects the loaded `vlm` from LM Studio at scrape time.
    pub lm_studio_vision_model: String,
    pub searxng_url: String,
    /// "low" | "medium" | "high" — passed as reasoning_effort in chat completions.
    /// Leave unset to omit the field (model default).
    pub reasoning_effort: Option<String>,
    /// Token budget for reasoning. Passed as reasoning_tokens in chat completions.
    /// Leave unset to omit the field (model default).
    pub reasoning_tokens: Option<u32>,
    /// Directory scanned for local GGUF models (Bow-managed llama-server engine).
    pub models_dir: PathBuf,
    /// Context size (tokens) used when loading a model into the local engine.
    pub ctx_size: u32,
}

impl Config {
    #[cfg(test)]
    pub fn test_default(workspace_root: std::path::PathBuf) -> Self {
        Config {
            tavily_api_key: String::new(),
            bow_secret: "test-secret".to_string(),
            ws_port: 9357,
            workspace_root,
            lm_studio_url: "http://localhost:1234".to_string(),
            lm_studio_model: "test-model".to_string(),
            lm_studio_vision_model: String::new(),
            searxng_url: "http://localhost:8888".to_string(),
            reasoning_effort: None,
            reasoning_tokens: None,
            models_dir: PathBuf::from(r"C:\AI\models"),
            ctx_size: 8192,
        }
    }

    pub fn from_env() -> Result<Self> {
        let candidates = env_candidates();
        for path in &candidates {
            if path.exists() {
                let _ = dotenvy::from_path(path);
            }
        }
        dotenvy::dotenv().ok();

        let tavily_api_key = std::env::var("TAVILY_API_KEY")
            .unwrap_or_default();
        let bow_secret = std::env::var("BOW_SECRET")
            .context("BOW_SECRET not set. Add it to desktop\\.env")?;
        let ws_port = std::env::var("BOW_WS_PORT")
            .unwrap_or_else(|_| "9357".to_string())
            .parse::<u16>()
            .context("BOW_WS_PORT must be a valid port number")?;
        let workspace_root = std::env::var("BOW_WORKSPACE")
            .unwrap_or_else(|_| r"C:\AI\workspace".to_string())
            .into();
        let lm_studio_url = std::env::var("LM_STUDIO_URL")
            .unwrap_or_else(|_| "http://localhost:1234".to_string());
        let lm_studio_model = std::env::var("LM_STUDIO_MODEL")
            .unwrap_or_else(|_| "qwen3.5-9b".to_string());
        // Empty by default → auto-detect the loaded vision model from LM Studio.
        let lm_studio_vision_model = std::env::var("LM_STUDIO_VISION_MODEL")
            .unwrap_or_default();
        let searxng_url = std::env::var("SEARXNG_URL")
            .unwrap_or_else(|_| "http://localhost:8888".to_string());

        let reasoning_effort = std::env::var("LM_STUDIO_REASONING_EFFORT").ok().and_then(|v| {
            match v.to_lowercase().as_str() {
                "low" | "medium" | "high" => Some(v.to_lowercase()),
                other => {
                    eprintln!("LM_STUDIO_REASONING_EFFORT: invalid value '{}' (use low/medium/high) — ignored", other);
                    None
                }
            }
        });

        let reasoning_tokens = std::env::var("LM_STUDIO_REASONING_TOKENS").ok().and_then(|v| {
            v.parse::<u32>().map_err(|_| {
                eprintln!("LM_STUDIO_REASONING_TOKENS: '{}' is not a valid u32 — ignored", v);
            }).ok()
        });

        let models_dir = std::env::var("BOW_MODELS_DIR")
            .unwrap_or_else(|_| r"C:\AI\models".to_string())
            .into();
        let ctx_size = std::env::var("BOW_CTX_SIZE")
            .unwrap_or_else(|_| "8192".to_string())
            .parse::<u32>()
            .context("BOW_CTX_SIZE must be a valid u32")?;

        Ok(Config {
            tavily_api_key,
            bow_secret,
            ws_port,
            workspace_root,
            lm_studio_url,
            lm_studio_model,
            lm_studio_vision_model,
            searxng_url,
            reasoning_effort,
            reasoning_tokens,
            models_dir,
            ctx_size,
        })
    }
}

fn env_candidates() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();

    // 1. Next to the running executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join(".env"));
        }
    }

    // 2. Hardcoded dev project path
    paths.push(PathBuf::from(r"C:\AI\agent Bow\desktop\.env"));

    // 3. Current working directory
    paths.push(PathBuf::from(".env"));

    paths
}

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub shell_session: ShellSessionManager,
    pub controlled_browser: ControlledBrowser,
    pub llm_engine: LlmEngine,
    /// Consumed by the model-management REST endpoints (Task 6).
    #[allow(dead_code)]
    pub models_dir: Arc<Mutex<PathBuf>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let browser_profile = config.workspace_root.join(".bow_browser_profile");
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("llama")))
            .unwrap_or_else(|| PathBuf::from("llama"));
        let models_dir = config.models_dir.clone();
        Self {
            shell_session: ShellSessionManager::new(),
            controlled_browser: ControlledBrowser::new(browser_profile),
            llm_engine: LlmEngine::new(bin_dir),
            models_dir: Arc::new(Mutex::new(models_dir)),
            config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_models_dir() {
        std::env::remove_var("BOW_MODELS_DIR");
        let c = Config::test_default(std::path::PathBuf::from(r"C:\tmp"));
        assert_eq!(c.models_dir, std::path::PathBuf::from(r"C:\AI\models"));
        assert_eq!(c.ctx_size, 8192);
    }
}
