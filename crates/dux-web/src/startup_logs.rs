//! REST reads for an agent's startup-command logs — the web counterpart to the
//! TUI's `read-startup-command-logs` palette command. Each run of a project's
//! startup command (see [`crate::session_actions`]'s `rerun_startup_command` and
//! the agent-create launch path) writes a timestamped `.log` file under
//! `{dux_root}/startup-command-logs/{project_id}/{session_id}/`; these GETs list
//! those files for an agent and return a chosen file's contents.
//!
//! - `GET /api/v1/sessions/:id/startup-logs` — the agent's log files, newest
//!   first, plus the newest file's contents pre-loaded (`selected`) so the viewer
//!   renders without a second round-trip. 404 for an unknown session id.
//! - `GET /api/v1/sessions/:id/startup-logs/content?name=` — one log file's
//!   contents. `name` must be one of the listed files (membership-checked, so a
//!   `..`/path-traversal value can never escape the agent's log directory); an
//!   empty/absent `name` returns the newest. 404 for an unknown session or an
//!   unknown log name.
//!
//! The directory listing and file reads run OFF the async reactor
//! (`spawn_blocking`), following the read precedent in [`crate::project_reads`].
//! The session → `(paths, project_id)` resolution is an instant clone off the
//! engine thread (`EngineHandle::session_startup_log_context`). Merged into the
//! authenticated sub-router in `server.rs`, so an unauthenticated request 401s
//! before reaching here.

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use dux_core::config::DuxPaths;

use crate::rest_common::id_within_bound;
use crate::server::AppState;

/// The gated startup-command-log read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/startup-logs", get(list_startup_logs))
        .route(
            "/api/v1/sessions/{id}/startup-logs/content",
            get(read_startup_log),
        )
}

/// One log file in the listing: its file name and last-modified time (RFC 3339).
#[derive(Serialize)]
struct StartupLogEntryView {
    name: String,
    modified_at: Option<String>,
}

/// A log file's name + full contents (the pre-loaded newest, or a requested one).
#[derive(Serialize)]
struct StartupLogContentView {
    name: String,
    content: String,
}

/// The list response: every log file (newest first) plus the newest file's
/// contents pre-loaded so the viewer can render immediately. `selected` is `None`
/// only when the agent has no startup-command logs yet.
#[derive(Serialize)]
struct StartupLogsReply {
    entries: Vec<StartupLogEntryView>,
    selected: Option<StartupLogContentView>,
}

async fn list_startup_logs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    match state.engine.session_startup_log_context(id.clone()).await {
        None => unknown_session(),
        Some((paths, project_id)) => {
            let session_id = id;
            match tokio::task::spawn_blocking(move || {
                collect_logs(&paths, &project_id, &session_id)
            })
            .await
            {
                Ok(Ok(reply)) => Json(reply).into_response(),
                Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("startup log listing failed: {e}"),
                )
                    .into_response(),
            }
        }
    }
}

#[derive(Deserialize)]
struct ContentQuery {
    /// The log file name to read; empty/absent returns the newest log.
    #[serde(default)]
    name: String,
}

async fn read_startup_log(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<ContentQuery>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    match state.engine.session_startup_log_context(id.clone()).await {
        None => unknown_session(),
        Some((paths, project_id)) => {
            let session_id = id;
            let name = query.name;
            match tokio::task::spawn_blocking(move || {
                read_named_log(&paths, &project_id, &session_id, &name)
            })
            .await
            {
                Ok(Ok(Some(reply))) => Json(reply).into_response(),
                Ok(Ok(None)) => {
                    (StatusCode::NOT_FOUND, "unknown startup command log").into_response()
                }
                Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("startup log read failed: {e}"),
                )
                    .into_response(),
            }
        }
    }
}

/// List the agent's startup-command logs (newest first) and pre-load the newest
/// file's contents. Returns a user-facing error string when the directory listing
/// or the newest file's read fails.
fn collect_logs(
    paths: &DuxPaths,
    project_id: &str,
    session_id: &str,
) -> Result<StartupLogsReply, String> {
    let entries = dux_core::startup::list_agent_logs(paths, project_id, session_id)
        .map_err(|e| format!("{e:#}"))?;
    let selected = match entries.first() {
        Some(entry) => Some(StartupLogContentView {
            name: entry.display_name.clone(),
            content: dux_core::startup::read_log(&entry.path).map_err(|e| format!("{e:#}"))?,
        }),
        None => None,
    };
    let entries = entries
        .into_iter()
        .map(|entry| StartupLogEntryView {
            name: entry.display_name,
            modified_at: entry.modified_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Ok(StartupLogsReply { entries, selected })
}

/// Read one of the agent's startup-command logs by file `name` (empty → newest).
/// `Ok(None)` when the agent has no logs or `name` does not match a listed file;
/// matching `name` against the listed files is the traversal guard (a value can
/// only ever name a real `.log` file in the agent's own directory). `Err` on a
/// directory-listing or read failure.
fn read_named_log(
    paths: &DuxPaths,
    project_id: &str,
    session_id: &str,
    name: &str,
) -> Result<Option<StartupLogContentView>, String> {
    let entries = dux_core::startup::list_agent_logs(paths, project_id, session_id)
        .map_err(|e| format!("{e:#}"))?;
    let entry = if name.is_empty() {
        entries.first()
    } else {
        entries.iter().find(|entry| entry.display_name == name)
    };
    match entry {
        None => Ok(None),
        Some(entry) => Ok(Some(StartupLogContentView {
            name: entry.display_name.clone(),
            content: dux_core::startup::read_log(&entry.path).map_err(|e| format!("{e:#}"))?,
        })),
    }
}

fn unknown_session() -> Response {
    (StatusCode::NOT_FOUND, "unknown session").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::path::Path;
    use tower::ServiceExt;

    use crate::test_support::{router_no_auth, router_with_auth};

    fn paths_for(root: &Path) -> DuxPaths {
        DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        }
    }

    /// Write two log files (an older and a newer) for project `p1` / session `s1`.
    /// Returns the newest file's name so assertions can target it.
    fn seed_two_logs(paths: &DuxPaths) -> String {
        let dir = dux_core::startup::agent_log_dir(paths, "p1", "s1");
        std::fs::create_dir_all(&dir).unwrap();
        // Lexicographically ordered, timestamp-style names; the listing sorts by
        // mtime then path, so the higher-stamped file is "newest".
        std::fs::write(dir.join("20260101T000000Z-feat.log"), "old run").unwrap();
        let newest = "20260102T000000Z-feat.log";
        std::fs::write(dir.join(newest), "newest run output").unwrap();
        newest.to_string()
    }

    #[test]
    fn collect_logs_lists_newest_first_and_preloads_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let newest = seed_two_logs(&paths);

        let reply = collect_logs(&paths, "p1", "s1").expect("collect");
        assert_eq!(reply.entries.len(), 2);
        assert_eq!(reply.entries[0].name, newest, "newest must sort first");
        let selected = reply.selected.expect("selected newest");
        assert_eq!(selected.name, newest);
        assert_eq!(selected.content, "newest run output");
    }

    #[test]
    fn collect_logs_empty_returns_no_entries_and_no_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let reply = collect_logs(&paths, "p1", "s1").expect("collect");
        assert!(reply.entries.is_empty());
        assert!(reply.selected.is_none());
    }

    #[test]
    fn read_named_log_returns_requested_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_two_logs(&paths);

        let reply = read_named_log(&paths, "p1", "s1", "20260101T000000Z-feat.log")
            .expect("read")
            .expect("found");
        assert_eq!(reply.name, "20260101T000000Z-feat.log");
        assert_eq!(reply.content, "old run");
    }

    #[test]
    fn read_named_log_empty_name_returns_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let newest = seed_two_logs(&paths);
        let reply = read_named_log(&paths, "p1", "s1", "")
            .expect("read")
            .expect("found");
        assert_eq!(reply.name, newest);
        assert_eq!(reply.content, "newest run output");
    }

    #[test]
    fn read_named_log_rejects_unknown_or_traversal_name() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_two_logs(&paths);
        // A name not among the listed files (including a traversal attempt) yields
        // None — the membership check keeps reads inside the agent's log dir.
        assert!(
            read_named_log(&paths, "p1", "s1", "../../etc/passwd")
                .expect("read")
                .is_none()
        );
        assert!(
            read_named_log(&paths, "p1", "s1", "nope.log")
                .expect("read")
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sessions/ghost/startup-logs")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn content_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sessions/ghost/startup-logs/content?name=x.log")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        // Drain the body so the response is fully consumed.
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }

    #[tokio::test]
    async fn startup_log_reads_are_gated() {
        for uri in [
            "/api/v1/sessions/s1/startup-logs",
            "/api/v1/sessions/s1/startup-logs/content?name=x.log",
        ] {
            let (_tmp, app) = router_with_auth();
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be gated"
            );
        }
    }
}
