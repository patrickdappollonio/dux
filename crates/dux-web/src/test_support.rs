//! Test-only helpers shared by the REST route modules' `#[cfg(test)]` suites:
//! a minimal headless engine handle plus a plain router builder. dux is
//! trusted-local with no login gate, so every route is served plainly.
//! Mirrors the private `test_engine_handle` in `server.rs`, lifted here so every
//! route module can boot the same engine without duplicating the recipe.

use std::net::SocketAddr;
use std::path::Path;

use axum::Router;
use tempfile::TempDir;

use crate::engine_actor::EngineHandle;
use crate::server;

/// Boot a minimal headless engine handle rooted at `tmp`. The handle just needs
/// to exist; routing-only tests never drive a real agent through it.
pub(crate) fn test_engine_handle(tmp: &Path) -> EngineHandle {
    let paths = dux_core::config::DuxPaths {
        root: tmp.to_path_buf(),
        config_path: tmp.join("config.toml"),
        sessions_db_path: tmp.join("sessions.sqlite3"),
        worktrees_root: tmp.join("worktrees"),
        lock_path: tmp.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
    let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
    handle
}

/// A fresh temp dir + an engine-backed router. Returns the `TempDir` so the
/// caller keeps it alive for the test's duration.
pub(crate) fn router_no_auth() -> (TempDir, Router) {
    let tmp = tempfile::tempdir().unwrap();
    let router = server::router(test_engine_handle(tmp.path()));
    (tmp, router)
}

/// Bind a real loopback server on an ephemeral port and serve the plain router on
/// a background task. Returns the bound `SocketAddr` so an integration test can
/// issue real HTTP/WebSocket requests against it. The `TempDir` is kept alive by
/// the returned guard; drop it to clean up the engine's on-disk state.
#[allow(dead_code)]
pub(crate) async fn boot_plain_test_server() -> (TempDir, SocketAddr) {
    let tmp = tempfile::tempdir().unwrap();
    let app = server::router(test_engine_handle(tmp.path()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    (tmp, addr)
}
