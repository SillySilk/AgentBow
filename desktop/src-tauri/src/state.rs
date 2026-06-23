use anyhow::{Context, Result};
use std::path::PathBuf;
use crate::tools::shell_session::ShellSessionManager;
use crate::tools::controlled_browser::ControlledBrowser;

#[derive(Debug, Clone)]
pub struct Config {
    pub tavily_api_key: String,
    pub bow_secret: String,
    pub ws_port: u16,
    pub workspace_root: PathBuf,
    pub lm_studio_url: String,
    pub lm_studio_model: String,
    /// Vision-capable model used for the image-QA gate. Defaults to `lm_studio_model`
    /// when `LM_STUDIO_VISION_MODEL` is unset.
    pub lm_studio_vision_model: String,
    pub searxng_url: String,
    /// "low" | "medium" | "high" — passed as reasoning_effort in chat completions.
    /// Leave unset to omit the field (model default).
    pub reasoning_effort: Option<String>,
    /// Token budget for reasoning. Passed as reasoning_tokens in chat completions.
    /// Leave unset to omit the field (model default).
    pub reasoning_tokens: Option<u32>,
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
            lm_studio_vision_model: "test-model".to_string(),
            searxng_url: "http://localhost:8888".to_string(),
            reasoning_effort: None,
            reasoning_tokens: None,
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
        let lm_studio_vision_model = std::env::var("LM_STUDIO_VISION_MODEL")
            .unwrap_or_else(|_| lm_studio_model.clone());
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
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let browser_profile = config.workspace_root.join(".bow_browser_profile");
        Self {
            shell_session: ShellSessionManager::new(),
            controlled_browser: ControlledBrowser::new(browser_profile),
            config,
        }
    }
}
