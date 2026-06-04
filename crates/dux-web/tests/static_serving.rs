use axum::body::Body;
use axum::http::{Request, StatusCode};
use dux_core::config::DuxPaths;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use tower::ServiceExt;

fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    (tmp, paths)
}

#[tokio::test]
async fn serves_embedded_index_at_root() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    assert!(
        html.contains("<!doctype html") || html.contains("id=\"root\""),
        "not the SPA index: {html}"
    );
}

#[tokio::test]
async fn unknown_path_falls_back_to_index() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/some/client/route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Build a router backed by a throwaway engine for static-asset assertions.
fn test_router() -> (tempfile::TempDir, axum::Router) {
    let (tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    (tmp, router(handle))
}

async fn get(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn manifest_served_with_manifest_mime() {
    let (_tmp, app) = test_router();
    let resp = get(app, "/manifest.webmanifest").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "content-type").as_deref(),
        Some("application/manifest+json"),
        "the web manifest must advertise the PWA manifest MIME type"
    );
}

#[tokio::test]
async fn service_worker_served_no_cache_and_js_mime() {
    let (_tmp, app) = test_router();
    let resp = get(app, "/sw.js").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "cache-control").as_deref(),
        Some("no-cache"),
        "the service worker must not be cached so SW updates are picked up promptly"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("javascript"),
        "sw.js must be served with a JavaScript MIME type, got {ctype}"
    );
}

#[tokio::test]
async fn missing_hashed_asset_returns_404_not_spa_fallback() {
    // A request for a hashed bundle chunk that does not exist must 404, NOT fall
    // back to index.html. Serving HTML for a `*.js` import() makes the browser
    // reject it as a module, which unmounts the React tree (white screen). This
    // happens after a rebuild+restart while a stale tab is still open.
    let (_tmp, app) = test_router();
    let resp = get(app, "/assets/nonexistent-deadbeef.js").await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a missing hashed asset must 404 so the client can recover, not get the SPA shell"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        !ctype.contains("html"),
        "a missing asset must not be served as text/html, got {ctype}"
    );
}

#[tokio::test]
async fn unknown_non_asset_path_still_serves_spa_shell() {
    // Client-side routes (anything outside `assets/`) must keep falling back to
    // the SPA index so deep links and the router keep working.
    let (_tmp, app) = test_router();
    let resp = get(app, "/some/client/route").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("html"),
        "an unknown non-asset path must serve the SPA shell as text/html, got {ctype}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    assert!(
        html.contains("<!doctype html") || html.contains("id=\"root\""),
        "the SPA shell must be served for client routes"
    );
}

#[tokio::test]
async fn offline_page_reachable_and_not_shadowed_by_spa_fallback() {
    let (_tmp, app) = test_router();
    let resp = get(app, "/offline.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    // The real offline page, not the SPA index served by the fallback.
    assert!(
        html.contains("dux is unreachable"),
        "the offline page itself must be served, not the SPA index fallback"
    );
    assert!(
        !html.contains("id=\"root\""),
        "offline.html should not be the SPA shell"
    );
}
