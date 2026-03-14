use anyhow::{Context, Result};
use std::path::PathBuf;
use crate::tools::shell_session::ShellSessionManager;

#[derive(Debug, Clone)]
pub struct Config {
    pub anthropic_api_key: String,
    pub tavily_api_key: String,
    pub bow_secret: String,
    pub model: String,
    pub ws_port: u16,
    pub workspace_root: PathBuf,
    pub lm_studio_url: String,
    pub lm_studio_model: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Try all .env candidates — load all that exist (later ones won't override)
        let candidates = env_candidates();
        for path in &candidates {
            if path.exists() {
                let _ = dotenvy::from_path(path);
            }
        }
        dotenvy::dotenv().ok();

        let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY not set. Add it to desktop\\.env")?;
        let tavily_api_key = std::env::var("TAVILY_API_KEY")
            .context("TAVILY_API_KEY not set. Add it to desktop\\.env")?;
        let bow_secret = std::env::var("BOW_SECRET")
            .context("BOW_SECRET not set. Add it to desktop\\.env")?;
        let model = std::env::var("BOW_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
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
            .unwrap_or_else(|_| "qwen3.5-9b-uncensored-hauhaucs-aggressive".to_string());

        Ok(Config {
            anthropic_api_key,
            tavily_api_key,
            bow_secret,
            model,
            ws_port,
            workspace_root,
            lm_studio_url,
            lm_studio_model,
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
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            shell_session: ShellSessionManager::new(),
        }
    }
}
