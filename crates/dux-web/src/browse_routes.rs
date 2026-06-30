//! Two stateless "utility" reads the add-project / new-agent dialogs need
//! (Phase 6 of the REST-first migration). These used to ride the retired `/ws`
//! request/reply pairs (`browse_dir` → `dir_entries`, `generate_agent_name` →
//! `agent_name`); they are now plain authenticated GETs.
//!
//! - `GET /api/v1/browse?path=` — directory listing for the add-project picker.
//!   An absent (or empty) `path` resolves the configured `defaults.start_directory`
//!   (shared fallback chain) from the live engine config, so the picker honors the
//!   setting and reflects an explicit reload; if the engine is gone it falls back
//!   to `$HOME`. The reply echoes the resolved `path` plus the child `entries`.
//! - `GET /api/v1/agent-name` — a freshly generated two-word pet name for the
//!   new-agent dialog's randomized-name preview (reuses `git::docker_style_name`).
//!
//! The filesystem read runs OFF the async reactor (`spawn_blocking`), following
//! the old handler's precedent. Merged into the authenticated (gated) sub-router
//! in `server.rs`, so an unauthenticated request 401s before reaching here.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::server::AppState;

/// Upper bound on the `?path=` query value before any filesystem touch. Generous
/// (well above `PATH_MAX` on supported platforms) so it rejects only an abusive
/// string, never a legitimate directory path.
const MAX_PATH_LEN: usize = 4096;

/// The gated utility read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/browse", get(browse))
        .route("/api/v1/agent-name", get(agent_name))
}

#[derive(Deserialize)]
struct BrowseQuery {
    #[serde(default)]
    path: Option<String>,
}

/// A single directory entry in the project picker, mirroring the frontend's
/// `DirEntryView` (`browseApi.ts` / `types.ts`).
#[derive(Serialize)]
struct DirEntryView {
    path: String,
    label: String,
    is_git_repo: bool,
}

/// The browse reply: the resolved directory plus its child entries.
#[derive(Serialize)]
struct BrowseReply {
    path: String,
    entries: Vec<DirEntryView>,
}

async fn browse(State(state): State<AppState>, Query(query): Query<BrowseQuery>) -> Response {
    // An explicit `path` always wins. An absent OR empty path means "open at the
    // configured default": resolve `defaults.start_directory` (with the shared
    // fallback chain) from the LIVE engine config, so the picker honors the
    // setting and reflects an explicit reload. If the engine is gone, fall back to
    // `$HOME` (then `/`), exactly as the old `BrowseDir` handler did.
    let dir = match query.path.filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => match state.engine.browse_start_dir().await {
            Some(dir) => dir,
            None => std::env::var("HOME").unwrap_or_else(|_| "/".to_string()),
        },
    };

    if dir.chars().count() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "path is too long").into_response();
    }

    // Filesystem read off the reactor (the `browse_dir` precedent).
    let result = tokio::task::spawn_blocking(move || {
        let p = std::path::Path::new(&dir);
        let entries = dux_core::project_browser::browser_entries(p)
            .into_iter()
            .map(|e| DirEntryView {
                path: e.path.to_string_lossy().to_string(),
                label: e.label,
                is_git_repo: e.is_git_repo,
            })
            .collect::<Vec<_>>();
        (dir, entries)
    })
    .await;

    match result {
        Ok((path, entries)) => Json(BrowseReply { path, entries }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("browse failed: {e}"),
        )
            .into_response(),
    }
}

/// The agent-name reply: a freshly generated pet name.
#[derive(Serialize)]
struct AgentNameReply {
    name: String,
}

async fn agent_name(State(_state): State<AppState>) -> Response {
    // Pure, fast, and self-contained: answer directly without round-tripping
    // through the engine thread (the old `GenerateAgentName` precedent).
    let name = dux_core::git::docker_style_name();
    Json(AgentNameReply { name }).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

    /// Percent-encode the bytes a directory path could carry in a query value so a
    /// space or other reserved char does not corrupt the request line. Small,
    /// dependency-free (the crate has no urlencoding dep).
    fn encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    #[tokio::test]
    async fn browse_lists_a_directory_and_echoes_the_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        std::fs::create_dir(dir.path().join("beta")).unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/browse?path={}", encode(&path)))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["path"], path);
        let labels: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["label"].as_str().unwrap())
            .collect();
        assert!(labels.contains(&"alpha/"));
        assert!(labels.contains(&"beta/"));
    }

    /// With `path` omitted, the picker must open at the configured
    /// `defaults.start_directory` (resolved through the live engine), not `$HOME`.
    /// This is the web side of the start-directory wiring (the TUI already honored
    /// it). Boots a real engine from a config.toml that points start_directory at a
    /// temp dir and asserts the no-path browse echoes that dir.
    #[tokio::test]
    async fn browse_without_a_path_opens_the_configured_start_directory() {
        let cfg_root = tempfile::tempdir().unwrap();
        let start = tempfile::tempdir().unwrap();
        std::fs::create_dir(start.path().join("alpha")).unwrap();
        let start_path = start.path().to_string_lossy().to_string();

        // Minimal config: only set the one key under test; everything else defaults.
        std::fs::write(
            cfg_root.path().join("config.toml"),
            format!("[defaults]\nstart_directory = \"{start_path}\"\n"),
        )
        .unwrap();

        let paths = dux_core::config::DuxPaths {
            root: cfg_root.path().to_path_buf(),
            config_path: cfg_root.path().join("config.toml"),
            sessions_db_path: cfg_root.path().join("sessions.sqlite3"),
            worktrees_root: cfg_root.path().join("worktrees"),
            lock_path: cfg_root.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::router(handle);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/browse")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["path"], start_path);
        let labels: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["label"].as_str().unwrap())
            .collect();
        assert!(labels.contains(&"alpha/"));
    }

    #[tokio::test]
    async fn browse_rejects_an_overlong_path() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/browse?path={}",
                        "x".repeat(MAX_PATH_LEN + 1)
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn agent_name_returns_a_hyphenated_pet_name() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/agent-name")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value["name"].as_str().unwrap().contains('-'));
    }
}
