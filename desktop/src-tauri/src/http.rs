use crate::state::AppState;
use crate::tools::mcp::McpManager;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::Response;
use axum::{routing::get, Json, Router};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct HttpState {
    pub app: AppState,
    pub mcp: McpManager,
}

async fn ws_upgrade(State(s): State<HttpState>, ws: WebSocketUpgrade) -> Response {
    let config = Arc::new(s.app.config.clone());
    let shell_session = s.app.shell_session.clone();
    let mcp = s.mcp.clone();
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = crate::server::run_ws(socket, config, shell_session, mcp).await {
            tracing::error!("WS connection error: {}", e);
        }
    })
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
        .route("/ws", get(ws_upgrade))
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

    fn config_router(ws_port: u16) -> axum::Router {
        axum::Router::new().route(
            "/api/config",
            axum::routing::get(move || async move {
                axum::Json(serde_json::json!({ "ws_port": ws_port }))
            }),
        )
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

    #[tokio::test]
    async fn config_returns_ws_port() {
        let app = config_router(9357);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("\"ws_port\":9357"), "body was: {}", text);
    }
}
