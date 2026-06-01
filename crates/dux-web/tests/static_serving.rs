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
