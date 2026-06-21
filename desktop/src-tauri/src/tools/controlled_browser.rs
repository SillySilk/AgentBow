// This module is a forward-looking seam: the public API (chrome_executable,
// ControlledBrowser) is wired into the browser_* tools by later Phase 3 tasks,
// so the items are intentionally unused until then.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn chrome_executable() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    const CANDIDATES: &[&str] = &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists())
}

use anyhow::{anyhow, Result};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;

struct BrowserState {
    browser: Browser,
    page: Page,
    _handler: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub struct ControlledBrowser {
    inner: Arc<Mutex<Option<BrowserState>>>,
    profile_dir: PathBuf,
}

impl ControlledBrowser {
    pub fn new(profile_dir: PathBuf) -> Self {
        ControlledBrowser {
            inner: Arc::new(Mutex::new(None)),
            profile_dir,
        }
    }

    pub async fn is_running(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    /// Launch Chrome with the persistent profile if not already running.
    pub async fn ensure_launched(&self, headless: bool) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let exe = chrome_executable().ok_or_else(|| {
            anyhow!("No Chrome/Edge found. Set CHROME_PATH in .env to the chrome.exe path.")
        })?;
        std::fs::create_dir_all(&self.profile_dir).ok();

        let mut builder = BrowserConfig::builder()
            .chrome_executable(exe)
            .user_data_dir(self.profile_dir.clone());
        if !headless {
            builder = builder.with_head();
        }
        let cfg = builder.build().map_err(|e| anyhow!("BrowserConfig: {}", e))?;

        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .map_err(|e| anyhow!("Chrome launch failed: {}", e))?;
        // The handler stream MUST be polled for the browser to function.
        let handler_task = tokio::spawn(async move { while (handler.next().await).is_some() {} });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| anyhow!("new_page: {}", e))?;

        *guard = Some(BrowserState {
            browser,
            page,
            _handler: handler_task,
        });
        Ok(())
    }

    /// Internal: run a closure with the current page, erroring if not launched.
    async fn with_page<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(Page) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let guard = self.inner.lock().await;
        let st = guard
            .as_ref()
            .ok_or_else(|| anyhow!("Browser not launched — call browser_open first"))?;
        let page = st.page.clone();
        drop(guard);
        f(page).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chrome_executable_honors_env_override() {
        // Point CHROME_PATH at a file we know exists (this test binary itself).
        let me = std::env::current_exe().unwrap();
        std::env::set_var("CHROME_PATH", &me);
        assert_eq!(chrome_executable(), Some(me));
        std::env::remove_var("CHROME_PATH");
    }

    #[tokio::test]
    #[ignore = "requires a real Chrome install; run manually with --ignored"]
    async fn launches_and_navigates_live() {
        let dir = std::env::temp_dir().join("bow_cb_live");
        let cb = ControlledBrowser::new(dir);
        cb.ensure_launched(true).await.expect("launch");
        assert!(cb.is_running().await);
    }
}
