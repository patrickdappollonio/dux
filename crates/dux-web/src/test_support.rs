//! Test-only helpers shared by the REST route modules' `#[cfg(test)]` suites:
//! a minimal headless engine handle plus router builders (gate off / gate on).
//! Mirrors the private `test_engine_handle` in `server.rs`, lifted here so every
//! route module can boot the same engine without duplicating the recipe.

use std::path::Path;

use axum::Router;
use tempfile::TempDir;

use crate::auth;
use crate::engine_actor::EngineHandle;
use crate::server::{self, RouterParams, build_app};

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

/// A fresh temp dir + an engine-backed router with the login gate OFF (the
/// common case for happy-path/404/400 route tests). Returns the `TempDir` so the
/// caller keeps it alive for the test's duration.
pub(crate) fn router_no_auth() -> (TempDir, Router) {
    let tmp = tempfile::tempdir().unwrap();
    let router = server::router(test_engine_handle(tmp.path()));
    (tmp, router)
}

/// A fresh temp dir + an engine-backed router with the login gate ON (a single
/// bcrypt-hashed user, no live session). Every `/api/v1/*` request without a
/// session cookie must 401, which is what the gated-401 tests assert.
pub(crate) fn router_with_auth() -> (TempDir, Router) {
    let tmp = tempfile::tempdir().unwrap();
    let hash = dux_core::auth::hash_password("secret-pw").unwrap();
    // `disable_auth = false` + a valid user → the gate is ENABLED, so a request
    // without a session cookie is rejected with 401.
    let auth = auth::shared_auth(&[format!("alice:{hash}")], false);
    let (router, _store) = build_app(
        test_engine_handle(tmp.path()),
        auth,
        Router::new(),
        RouterParams::plain_http(),
    );
    (tmp, router)
}
