//! REST reads for startup-command logs, the web counterpart to the TUI's
//! `read-startup-command-logs` palette command. Each run of a project's startup
//! command (see [`crate::session_actions`]'s `rerun_startup_command` and the
//! agent-create launch path) writes a timestamped `.log` file under
//! `{dux_root}/startup-command-logs/{project_id}/{session_id}/`; these GETs list
//! those files and return a chosen file's contents.
//!
//! Both SCOPES of `dux_core::startup::StartupCommandLogScope` are served, so the
//! web matches the TUI (which picks Agent scope when an agent is selected and
//! Project scope otherwise):
//!
//! - `GET /api/v1/sessions/:id/startup-logs` — the agent's log files, newest
//!   first, plus the newest file's contents pre-loaded (`selected`) so the viewer
//!   renders without a second round-trip. 404 for an unknown session id.
//! - `GET /api/v1/sessions/:id/startup-logs/content?name=` — one log file's
//!   contents. `name` must be one of the listed files (membership-checked, so a
//!   `..`/path-traversal value can never escape the agent's log directory); an
//!   empty/absent `name` returns the newest. 404 for an unknown session or an
//!   unknown log name.
//! - `GET /api/v1/projects/:id/startup-logs` and
//!   `GET /api/v1/projects/:id/startup-logs/content?name=`, the same two reads
//!   in PROJECT scope: every run across every session of the project. 404
//!   (`unknown project`) for an unknown project id.
//!
//! The scope is never re-derived here: both handlers hand a
//! `StartupCommandLogScope` to `dux_core::startup::list_logs_for_scope`, which is
//! the one place "agent means this directory, project means every session
//! directory under it" is written down.
//!
//! The directory listing and file reads run OFF the async reactor
//! (`spawn_blocking`), following the read precedent in [`crate::project_reads`].
//! The session → `(paths, project_id)` and project → `paths` resolutions are
//! instant clones off the engine thread
//! (`EngineHandle::session_startup_log_context` /
//! `EngineHandle::project_startup_log_context`). Merged into the authenticated
//! sub-router in `server.rs`, so an unauthenticated request 401s before reaching
//! here.

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use dux_core::config::DuxPaths;
use dux_core::startup::StartupCommandLogScope;

use crate::rest_common::{id_within_bound, unknown_session};
use crate::server::AppState;

/// The gated startup-command-log read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/startup-logs", get(list_startup_logs))
        .route(
            "/api/v1/sessions/{id}/startup-logs/content",
            get(read_startup_log),
        )
        .route(
            "/api/v1/projects/{id}/startup-logs",
            get(list_project_startup_logs),
        )
        .route(
            "/api/v1/projects/{id}/startup-logs/content",
            get(read_project_startup_log),
        )
}

/// 404 for an unknown or over-length project id, matching the body text the
/// other project-scoped reads use ([`crate::project_reads`]).
fn unknown_project() -> Response {
    (StatusCode::NOT_FOUND, "unknown project").into_response()
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
            let scope = StartupCommandLogScope::Agent {
                project_id,
                session_id: id,
            };
            match tokio::task::spawn_blocking(move || collect_logs(&paths, scope)).await {
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
            let scope = StartupCommandLogScope::Agent {
                project_id,
                session_id: id,
            };
            let name = query.name;
            match tokio::task::spawn_blocking(move || read_named_log(&paths, scope, &name)).await {
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

async fn list_project_startup_logs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    match state.engine.project_startup_log_context(id.clone()).await {
        None => unknown_project(),
        Some(paths) => {
            let scope = StartupCommandLogScope::Project { project_id: id };
            match tokio::task::spawn_blocking(move || collect_logs(&paths, scope)).await {
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

async fn read_project_startup_log(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<ContentQuery>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    match state.engine.project_startup_log_context(id.clone()).await {
        None => unknown_project(),
        Some(paths) => {
            let scope = StartupCommandLogScope::Project { project_id: id };
            let name = query.name;
            match tokio::task::spawn_blocking(move || read_named_log(&paths, scope, &name)).await {
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

/// List `scope`'s startup-command logs (newest first) and pre-load the newest
/// file's contents. Returns a user-facing error string when the directory listing
/// or the newest file's read fails.
fn collect_logs(
    paths: &DuxPaths,
    scope: StartupCommandLogScope,
) -> Result<StartupLogsReply, String> {
    let entries =
        dux_core::startup::list_logs_for_scope(paths, scope).map_err(|e| format!("{e:#}"))?;
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

/// Read one of `scope`'s startup-command logs by file `name` (empty → newest).
/// `Ok(None)` when the scope has no logs or `name` does not match a listed file;
/// matching `name` against the listed files is the traversal guard (a value can
/// only ever name a real `.log` file inside the scope's own directories). `Err`
/// on a directory-listing or read failure.
///
/// In PROJECT scope the listing spans every session directory, so two runs can
/// in principle carry the same file name (same second, same branch name, two
/// sessions). The search takes the first hit in a newest-first listing, so a
/// duplicate name resolves to the newest of them, deterministically.
fn read_named_log(
    paths: &DuxPaths,
    scope: StartupCommandLogScope,
    name: &str,
) -> Result<Option<StartupLogContentView>, String> {
    let entries =
        dux_core::startup::list_logs_for_scope(paths, scope).map_err(|e| format!("{e:#}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::path::Path;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

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

        let reply = collect_logs(&paths, agent_scope("s1")).expect("collect");
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
        let reply = collect_logs(&paths, agent_scope("s1")).expect("collect");
        assert!(reply.entries.is_empty());
        assert!(reply.selected.is_none());
    }

    #[test]
    fn read_named_log_returns_requested_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_two_logs(&paths);

        let reply = read_named_log(&paths, agent_scope("s1"), "20260101T000000Z-feat.log")
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
        let reply = read_named_log(&paths, agent_scope("s1"), "")
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
            read_named_log(&paths, agent_scope("s1"), "../../etc/passwd")
                .expect("read")
                .is_none()
        );
        assert!(
            read_named_log(&paths, agent_scope("s1"), "nope.log")
                .expect("read")
                .is_none()
        );
    }

    /// Seed one run under project `p1` / session `sid`, named `name`, with an
    /// explicit mtime so the project-scope ordering (which spans directories) is
    /// deterministic rather than dependent on filesystem timestamp resolution.
    fn seed_run(paths: &DuxPaths, session_id: &str, name: &str, body: &str, mtime_secs: u64) {
        let dir = dux_core::startup::agent_log_dir(paths, "p1", session_id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs),
        )
        .unwrap();
    }

    fn project_scope() -> dux_core::startup::StartupCommandLogScope {
        dux_core::startup::StartupCommandLogScope::Project {
            project_id: "p1".to_string(),
        }
    }

    fn agent_scope(session_id: &str) -> dux_core::startup::StartupCommandLogScope {
        dux_core::startup::StartupCommandLogScope::Agent {
            project_id: "p1".to_string(),
            session_id: session_id.to_string(),
        }
    }

    #[test]
    fn collect_logs_project_scope_spans_every_session_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_run(
            &paths,
            "s1",
            "20260101T000000Z-one.log",
            "s1 run",
            1_767_225_600,
        );
        seed_run(
            &paths,
            "s2",
            "20260102T000000Z-two.log",
            "s2 run",
            1_767_312_000,
        );

        let reply = collect_logs(&paths, project_scope()).expect("collect");
        assert_eq!(
            reply
                .entries
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>(),
            vec!["20260102T000000Z-two.log", "20260101T000000Z-one.log"],
            "every session's runs are in project scope, newest first"
        );
        let selected = reply.selected.expect("selected newest");
        assert_eq!(selected.name, "20260102T000000Z-two.log");
        assert_eq!(selected.content, "s2 run");
    }

    #[test]
    fn collect_logs_project_scope_empty_returns_no_entries_and_no_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let reply = collect_logs(&paths, project_scope()).expect("collect");
        assert!(reply.entries.is_empty());
        assert!(reply.selected.is_none());
    }

    #[test]
    fn read_named_log_project_scope_finds_a_run_in_any_session() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_run(
            &paths,
            "s1",
            "20260101T000000Z-one.log",
            "s1 run",
            1_767_225_600,
        );
        seed_run(
            &paths,
            "s2",
            "20260102T000000Z-two.log",
            "s2 run",
            1_767_312_000,
        );

        // A run belonging to a session OTHER than the newest one is reachable in
        // project scope; the agent scope for s2 must not see it.
        let reply = read_named_log(&paths, project_scope(), "20260101T000000Z-one.log")
            .expect("read")
            .expect("found");
        assert_eq!(reply.content, "s1 run");
        assert!(
            read_named_log(&paths, agent_scope("s2"), "20260101T000000Z-one.log")
                .expect("read")
                .is_none(),
            "the agent scope must stay confined to its own session directory"
        );
    }

    #[test]
    fn read_named_log_project_scope_rejects_unknown_or_traversal_name() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_run(
            &paths,
            "s1",
            "20260101T000000Z-one.log",
            "s1 run",
            1_767_225_600,
        );
        for name in [
            "../../etc/passwd",
            "s1/20260101T000000Z-one.log",
            "nope.log",
        ] {
            assert!(
                read_named_log(&paths, project_scope(), name)
                    .expect("read")
                    .is_none(),
                "{name} must not resolve: the membership check is the traversal guard"
            );
        }
    }

    /// KNOWN TRADEOFF, pinned deliberately: a project-scope name is only a file
    /// name, so two sessions that ran in the same second on the same branch name
    /// produce the same wire name. The lookup then resolves to the NEWEST match,
    /// because the listing is newest-first and the search takes the first hit.
    #[test]
    fn read_named_log_project_scope_duplicate_name_resolves_to_the_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        seed_run(
            &paths,
            "s1",
            "20260101T000000Z-dup.log",
            "older dup",
            1_767_225_600,
        );
        seed_run(
            &paths,
            "s2",
            "20260101T000000Z-dup.log",
            "newer dup",
            1_767_312_000,
        );

        let reply = read_named_log(&paths, project_scope(), "20260101T000000Z-dup.log")
            .expect("read")
            .expect("found");
        assert_eq!(reply.content, "newer dup");
    }

    #[tokio::test]
    async fn project_list_404_for_unknown_project() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/ghost/startup-logs")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        // Assert the BODY too: an unregistered path also 404s through the SPA
        // fallback, so the status alone would pass without the route existing.
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"unknown project");
    }

    #[tokio::test]
    async fn project_content_404_for_unknown_project() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/ghost/startup-logs/content?name=x.log")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"unknown project");
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
}
