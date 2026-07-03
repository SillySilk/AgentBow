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
    pub searxng_url: String,
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
            searxng_url: "http://localhost:8888".to_string(),
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
        let searxng_url = std::env::var("SEARXNG_URL")
            .unwrap_or_else(|_| "http://localhost:8888".to_string());

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
            searxng_url,
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
