use std::path::{Path, PathBuf};

pub fn within_workspace(workspace_root: &Path, candidate: &str) -> Option<PathBuf> {
    let root = workspace_root.canonicalize().ok()?;
    let cand = Path::new(candidate).canonicalize().ok()?;
    if cand.starts_with(&root) { Some(cand) } else { None }
}

use crate::http::HttpState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct DirQuery {
    pub dir: String,
}

pub async fn list_images(State(s): State<HttpState>, Query(q): Query<DirQuery>) -> Response {
    let Some(dir) = within_workspace(&s.app.config.workspace_root, &q.dir) else {
        return (StatusCode::BAD_REQUEST, "dir outside workspace").into_response();
    };
    let mut paths = Vec::new();
    crate::tools::image_curate::collect_images(&dir, false, &mut paths);
    let images: Vec<_> = paths
        .iter()
        .map(|p| {
            let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            json!({
                "name": p.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": p.to_string_lossy(),
                "bytes": bytes
            })
        })
        .collect();
    Json(json!({ "dir": dir.to_string_lossy(), "images": images })).into_response()
}

#[derive(Deserialize)]
pub struct ThumbQuery {
    pub path: String,
    pub w: Option<u32>,
}

pub async fn thumb(State(s): State<HttpState>, Query(q): Query<ThumbQuery>) -> Response {
    let Some(path) = within_workspace(&s.app.config.workspace_root, &q.path) else {
        return (StatusCode::BAD_REQUEST, "path outside workspace").into_response();
    };
    let w = q.w.unwrap_or(256).clamp(32, 1024);
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let img = image::open(&path)?;
        let img = img.resize(w, w, image::imageops::FilterType::Triangle);
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img.to_rgb8())
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)?;
        Ok(buf)
    })
    .await;
    match bytes {
        Ok(Ok(b)) => ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], b).into_response(),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, "could not render thumbnail").into_response(),
    }
}

#[derive(Deserialize)]
pub struct DeleteBody {
    pub paths: Vec<String>,
}

pub async fn delete_images(State(s): State<HttpState>, Json(b): Json<DeleteBody>) -> Response {
    let (mut deleted, mut errors) = (0usize, 0usize);
    for p in &b.paths {
        match within_workspace(&s.app.config.workspace_root, p) {
            Some(path) => {
                if std::fs::remove_file(&path).is_ok() {
                    deleted += 1;
                } else {
                    errors += 1;
                }
            }
            None => errors += 1,
        }
    }
    Json(json!({ "deleted": deleted, "errors": errors })).into_response()
}

#[derive(Deserialize)]
pub struct DedupeBody {
    pub dir: String,
    pub threshold: Option<u32>,
    pub apply: Option<bool>,
}

pub async fn dedupe(State(s): State<HttpState>, Json(b): Json<DedupeBody>) -> Response {
    let Some(dir) = within_workspace(&s.app.config.workspace_root, &b.dir) else {
        return (StatusCode::BAD_REQUEST, "dir outside workspace").into_response();
    };
    match crate::tools::image_curate::image_dedupe(
        &dir.to_string_lossy(),
        b.threshold.unwrap_or(10),
        false,
        b.apply.unwrap_or(false),
    )
    .await
    {
        Ok(report) => Json(json!({ "report": report })).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct OpenBody {
    pub path: String,
}

pub async fn open_folder(State(s): State<HttpState>, Json(b): Json<OpenBody>) -> Response {
    let Some(path) = within_workspace(&s.app.config.workspace_root, &b.path) else {
        return (StatusCode::BAD_REQUEST, "path outside workspace").into_response();
    };
    let _ = std::process::Command::new("explorer.exe").arg(&path).spawn();
    Json(json!({ "ok": true })).into_response()
}

pub fn routes() -> Router<HttpState> {
    Router::new()
        .route("/api/images", get(list_images))
        .route("/api/thumb", get(thumb))
        .route("/api/images/delete", post(delete_images))
        .route("/api/curate/dedupe", post(dedupe))
        .route("/api/open-folder", post(open_folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_outside_workspace() {
        let ws =
            std::env::temp_dir().join(format!("bow_ws_{}", uuid::Uuid::new_v4().simple()));
        let inside = ws.join("a");
        std::fs::create_dir_all(&inside).unwrap();
        let f = inside.join("x.txt");
        std::fs::write(&f, b"hi").unwrap();
        // inside ok
        assert!(within_workspace(&ws, f.to_str().unwrap()).is_some());
        // outside rejected
        let outside = std::env::temp_dir().join("definitely_not_in_ws.txt");
        std::fs::write(&outside, b"hi").ok();
        assert!(within_workspace(&ws, outside.to_str().unwrap()).is_none());
        std::fs::remove_dir_all(&ws).ok();
    }

    #[tokio::test]
    async fn list_images_returns_files_in_dir() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use image::{Rgb, RgbImage};
        use tower::ServiceExt;

        let ws = std::env::temp_dir()
            .join(format!("bow_ws_li_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).unwrap();
        RgbImage::from_pixel(20, 20, Rgb([1, 2, 3]))
            .save(ws.join("a.png"))
            .unwrap();

        let state = crate::http::HttpState::test_state(ws.clone());
        let app = crate::web_api::routes().with_state(state);
        let uri = format!(
            "/api/images?dir={}",
            urlencoding::encode(ws.to_str().unwrap())
        );
        let res = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["images"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(&ws).ok();
    }
}
