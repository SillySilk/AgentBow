use crate::state::AppState;
use crate::tools::mcp::McpManager;
use axum::{routing::get, Json, Router};
use serde_json::json;
use std::path::PathBuf;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct HttpState {
    pub app: AppState,
    pub mcp: McpManager,
}

pub fn build_router(state: AppState, mcp: McpManager, web_dir: PathBuf) -> Router {
    let ws_port = state.config.ws_port;
    let http_state = HttpState { app: state, mcp };

    let index = web_dir.join("index.html");
    let static_service = ServeDir::new(&web_dir)
        .not_found_service(tower_http::services::ServeFile::new(index));

    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route(
            "/api/config",
            get(move || async move { Json(json!({ "ws_port": ws_port })) }),
        )
        // /ws is added in Task 3.
        .fallback_service(static_service)
        .with_state(http_state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    fn test_router() -> axum::Router {
        // health/config routes do not depend on AppState; build a minimal router.
        axum::Router::new()
            .route("/api/health", axum::routing::get(|| async { "ok" }))
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_router();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
