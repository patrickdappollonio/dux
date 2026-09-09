//! `GET /api/v1/sessions/:id/changes`: a session's changed files, backed by
//! [`crate::changes::ChangesService`].
//!
//! Status codes:
//! - 200 with [`ChangesResponseBody`]. Deliberately not
//!   `dux_core::viewmodel::ChangedFilesView`, which carries a global watched
//!   session id and has no `rev`.
//! - 404 when the session is unknown (no worktree).
//! - 409 with `Retry-After` on a git lock or rebase error; 409 rather than 503
//!   because proxies may reroute a 503.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use dux_core::viewmodel::ChangedFileView;

use crate::changes::GitError;
use crate::git_routes::resolve_worktree;
use crate::server::AppState;

/// Upper bound on the `:id` path segment before any lookup (matches the
/// length-bounding convention for path params elsewhere).
const MAX_ID_LEN: usize = 128;

/// `Retry-After` seconds returned alongside a 409 so a client backs off before
/// refetching during a transient git lock/rebase.
const RETRY_AFTER_SECS: u64 = 2;

/// The dedicated 200 body. Distinct from `ChangedFilesView` (no global
/// `watched_session_id`; carries `rev`). The per-file [`ChangedFileView`] is reused.
#[derive(Serialize)]
struct ChangesResponseBody {
    rev: u64,
    staged: Vec<ChangedFileView>,
    unstaged: Vec<ChangedFileView>,
}

/// The changed-files read route.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/sessions/{id}/changes", get(get_changes))
}

async fn get_changes(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Length-bound the id before any lookup. Count characters, not bytes, so a
    // multi-byte id is not rejected early by its UTF-8 length.
    if id.chars().count() > MAX_ID_LEN {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    // 404 if the session is unknown (reuse the shared worktree resolver).
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    match state.changes.get(&id).await {
        Ok(c) => Json(ChangesResponseBody {
            rev: c.rev,
            staged: c.staged,
            unstaged: c.unstaged,
        })
        .into_response(),
        // The session vanished between the resolve and the read.
        Err(GitError::SessionNotFound) => {
            (StatusCode::NOT_FOUND, "unknown session").into_response()
        }
        // A git lock/rebase (or other git failure): the service already logged it.
        Err(GitError::Git(_)) => (
            StatusCode::CONFLICT,
            [(header::RETRY_AFTER, RETRY_AFTER_SECS.to_string())],
            "changed files are temporarily unavailable (the repository is busy); retry shortly",
        )
            .into_response(),
    }
}
