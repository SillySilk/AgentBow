use std::path::{Path, PathBuf};

pub fn within_workspace(workspace_root: &Path, candidate: &str) -> Option<PathBuf> {
    let root = workspace_root.canonicalize().ok()?;
    let cand = Path::new(candidate).canonicalize().ok()?;
    if cand.starts_with(&root) { Some(cand) } else { None }
}

/// Like within_workspace but allows a not-yet-existing path: canonicalizes the
/// nearest existing ancestor and checks it is inside workspace_root. Returns the
/// resolved absolute path (root-canonicalized + remaining components) if inside.
pub fn resolve_within_workspace(workspace_root: &Path, candidate: &str) -> Option<PathBuf> {
    let root = workspace_root.canonicalize().ok()?;
    let cand = Path::new(candidate);
    // Resolve against root if relative.
    let abs = if cand.is_absolute() { cand.to_path_buf() } else { root.join(cand) };
    // Walk up to the nearest existing ancestor and canonicalize it.
    let mut existing = abs.as_path();
    loop {
        if existing.exists() { break; }
        match existing.parent() { Some(p) => existing = p, None => return None }
    }
    let existing_canon = existing.canonicalize().ok()?;
    if !existing_canon.starts_with(&root) { return None; }
    // Reattach the non-existing tail to the canonical existing prefix.
    let tail = abs.strip_prefix(existing).ok()?;
    // Reject any tail that contains a parent-dir component to prevent path traversal.
    if tail.components().any(|c| c == std::path::Component::ParentDir) {
        return None;
    }
    Some(existing_canon.join(tail))
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
pub(crate) struct DirQuery {
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
pub(crate) struct ThumbQuery {
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
pub(crate) struct DeleteBody {
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
pub(crate) struct DedupeBody {
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
pub(crate) struct OpenBody {
    pub path: String,
}

pub async fn open_folder(State(s): State<HttpState>, Json(b): Json<OpenBody>) -> Response {
    let Some(path) = within_workspace(&s.app.config.workspace_root, &b.path) else {
        return (StatusCode::BAD_REQUEST, "path outside workspace").into_response();
    };
    #[cfg(target_os = "windows")]
    { let _ = std::process::Command::new("explorer.exe").arg(&path).spawn(); }
    #[cfg(not(target_os = "windows"))]
    { let _ = &path; }
    Json(json!({ "ok": true })).into_response()
}

/// Immediate subdirectories of `base` whose name is all ASCII digits, sorted
/// numerically. These are the scrape "bin" folders created by `pick_auto_bin`.
pub(crate) fn numeric_slot_dirs(base: &Path) -> Vec<(u64, String, PathBuf)> {
    let mut slots: Vec<(u64, String, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for e in entries.flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(num) = name.parse::<u64>() {
                        slots.push((num, name.to_string(), path.clone()));
                    }
                }
            }
        }
    }
    slots.sort_by_key(|(n, _, _)| *n);
    slots
}

/// List the numbered set folders under `dir`, each with an image count.
pub async fn list_slots(State(s): State<HttpState>, Query(q): Query<DirQuery>) -> Response {
    let Some(base) = within_workspace(&s.app.config.workspace_root, &q.dir) else {
        return (StatusCode::BAD_REQUEST, "dir outside workspace").into_response();
    };
    let slots: Vec<_> = numeric_slot_dirs(&base)
        .into_iter()
        .map(|(_, name, path)| {
            let mut imgs = Vec::new();
            crate::tools::image_curate::collect_images(&path, false, &mut imgs);
            json!({ "name": name, "path": path.to_string_lossy(), "count": imgs.len() })
        })
        .collect();
    Json(json!({ "base": base.to_string_lossy(), "slots": slots })).into_response()
}

pub async fn engine_status(State(s): State<HttpState>) -> Json<serde_json::Value> {
    let st = s.app.llm_engine.status().await;
    Json(serde_json::to_value(st).unwrap_or_else(|_| json!({"state":"stopped"})))
}

fn models_payload(dir: &Path) -> serde_json::Value {
    let models: Vec<serde_json::Value> = crate::llm_engine::scan_models(dir)
        .into_iter()
        .map(|m| json!({
            "name": m.name, "path": m.path, "size_bytes": m.size_bytes,
            "quant": m.quant, "vision": m.mmproj.is_some(),
            "loadable": crate::llm_engine::is_loadable_quant(&m.quant),
        }))
        .collect();
    json!({ "dir": dir.to_string_lossy(), "models": models })
}

pub async fn engine_models(State(s): State<HttpState>) -> Json<serde_json::Value> {
    let dir = s.app.models_dir.lock().unwrap().clone();
    Json(models_payload(&dir))
}

#[derive(Deserialize)]
pub(crate) struct LoadReq {
    pub path: String,
}

pub async fn engine_load(
    State(s): State<HttpState>,
    Json(req): Json<LoadReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let dir = s.app.models_dir.lock().unwrap().clone();
    let entry = crate::llm_engine::scan_models(&dir)
        .into_iter()
        .find(|m| m.path == Path::new(&req.path))
        .ok_or_else(|| {
            (StatusCode::BAD_REQUEST, Json(json!({"error": "model not found in models dir"})))
        })?;
    s.app
        .llm_engine
        .load(entry, s.app.config.ctx_size)
        .await
        .map(|_| Json(json!({"ok": true})))
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))))
}

pub async fn engine_stop(State(s): State<HttpState>) -> Json<serde_json::Value> {
    s.app.llm_engine.stop().await;
    Json(json!({"ok": true}))
}

#[derive(Deserialize)]
pub(crate) struct DirReq {
    pub dir: String,
}

pub async fn engine_models_dir(
    State(s): State<HttpState>,
    Json(req): Json<DirReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let p = PathBuf::from(&req.dir);
    if !p.is_dir() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "not a directory"}))));
    }
    *s.app.models_dir.lock().unwrap() = p.clone();
    Ok(Json(models_payload(&p)))
}

pub fn routes() -> Router<HttpState> {
    Router::new()
        .route("/api/images", get(list_images))
        .route("/api/slots", get(list_slots))
        .route("/api/thumb", get(thumb))
        .route("/api/images/delete", post(delete_images))
        .route("/api/curate/dedupe", post(dedupe))
        .route("/api/open-folder", post(open_folder))
        .route("/api/engine", get(engine_status))
        .route("/api/models", get(engine_models))
        .route("/api/engine/load", post(engine_load))
        .route("/api/engine/stop", post(engine_stop))
        .route("/api/engine/models-dir", post(engine_models_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_within_workspace_allows_nonexistent_subdir() {
        let ws = std::env::temp_dir().join(format!("bow_rws_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).unwrap();
        // A relative dest inside the workspace (does not exist yet).
        let result = resolve_within_workspace(&ws, "images/batch1");
        assert!(result.is_some(), "expected Some for relative subdir inside workspace");
        // An absolute path outside the workspace.
        let outside = std::env::temp_dir().join("some_other_dir");
        let result2 = resolve_within_workspace(&ws, outside.to_str().unwrap());
        assert!(result2.is_none(), "expected None for path outside workspace");
        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn numeric_slot_dirs_filters_and_sorts() {
        let base = std::env::temp_dir().join(format!("bow_slots_{}", uuid::Uuid::new_v4().simple()));
        for d in ["2", "10", "1", "logs", "notanumber"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("3"), b"a file, not a dir").unwrap();
        let names: Vec<String> = numeric_slot_dirs(&base).into_iter().map(|(_, n, _)| n).collect();
        assert_eq!(names, vec!["1", "2", "10"]); // numeric only, numeric sort, no "3" (it's a file)
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn resolve_within_workspace_rejects_dotdot_traversal() {
        // Create workspace with an existing subdir to make the traversal plausible.
        let ws = std::env::temp_dir().join(format!("bow_trav_{}", uuid::Uuid::new_v4().simple()));
        let images_dir = ws.join("images");
        std::fs::create_dir_all(&images_dir).unwrap();

        // Construct an absolute traversal candidate: <ws>/images/../../outside
        // After walking up to the existing `images` dir, the tail would be `../../outside`,
        // which must be rejected.
        let traversal = format!("{}/images/../../outside", ws.to_str().unwrap());
        let result = resolve_within_workspace(&ws, &traversal);
        assert!(result.is_none(), "expected None for .. traversal that escapes workspace");

        // A legitimate not-yet-existing subdir must still be allowed.
        let legit = format!("{}/newpics", ws.to_str().unwrap());
        let result2 = resolve_within_workspace(&ws, &legit);
        assert!(result2.is_some(), "expected Some for a legitimate nonexistent subdir");

        std::fs::remove_dir_all(&ws).ok();
    }

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

    #[tokio::test]
    async fn list_images_rejects_path_outside_workspace() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let ws = std::env::temp_dir()
            .join(format!("bow_ws_rej_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).unwrap();

        // A path that is definitely outside the workspace.
        let outside = std::env::temp_dir().join("bow_outside_test_dir");
        std::fs::create_dir_all(&outside).unwrap();

        let state = crate::http::HttpState::test_state(ws.clone());
        let app = crate::web_api::routes().with_state(state);
        let uri = format!(
            "/api/images?dir={}",
            urlencoding::encode(outside.to_str().unwrap())
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
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(&ws).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[tokio::test]
    async fn engine_status_endpoint_returns_stopped() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = crate::http::HttpState::test_state(std::env::temp_dir());
        let app = routes().with_state(state);
        let res = app
            .oneshot(Request::builder().uri("/api/engine").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("\"state\":\"stopped\""));
    }

    #[tokio::test]
    async fn engine_load_rejects_bad_path() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = crate::http::HttpState::test_state(std::env::temp_dir());
        let app = routes().with_state(state);
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/engine/load")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"C:\\nope\\missing.gguf"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
}
