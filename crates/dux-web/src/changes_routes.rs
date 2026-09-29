//! `GET /api/v1/sessions/:id/changes`: a session's changed files, backed by
//! [`crate::changes::ChangesService`]; and
//! `GET /api/v1/sessions/:id/changes/children?dir=…&side=staged|unstaged`: one
//! level of a folded folder's contents (see [`get_children`]).
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
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

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

/// The changed-files read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/changes", get(get_changes))
        .route("/api/v1/sessions/{id}/changes/children", get(get_children))
}

/// Which folder to list, and on which side of the listing.
#[derive(Deserialize)]
struct ChildrenQuery {
    dir: String,
    side: String,
}

/// One level of a folded folder, in the listing's own row shape.
#[derive(Serialize)]
struct ChildrenResponseBody {
    dir: String,
    side: &'static str,
    children: Vec<ChangedFileView>,
}

/// Why a folder may not be listed, in words for the browser.
enum ChildrenError {
    Refused(String),
    Git(String),
}

/// The children of `dir` on `side`, refusing anything that is not a folder
/// the live listing shows.
///
/// The folder must be a folded folder row of that side, or a folder inside
/// one that git itself lists something under (which is how a sub-folder of
/// an expanded folder is reached). The path must be in its plain spelling, so
/// nothing like `node_modules/..` can name the worktree or climb out. On the
/// unstaged side it must be a real directory rather than a symlink, so nothing
/// outside the worktree is ever read; on the staged side, which reads the
/// index, it may be gone from disk but may not be a symlink. The listing is read fresh, the same way the git
/// routes validate a path.
fn list_children(
    worktree: &std::path::Path,
    dir: &str,
    side: dux_core::git::ChangesSide,
) -> Result<Vec<dux_core::model::ChangedFile>, ChildrenError> {
    use dux_core::git::{ChangesSide, changed_dir_children, changed_files, rows_answering};
    if dir.is_empty() || !dux_core::model::is_lexically_normal_path(dir) {
        return Err(ChildrenError::Refused(format!(
            "\"{dir}\" is not a plain path inside the worktree"
        )));
    }
    // What is at the path, not followed through a link. A refusal names a
    // directory with its trailing slash and anything else as it is.
    let on_disk = std::fs::symlink_metadata(worktree.join(dir)).ok();
    let is_dir = on_disk
        .as_ref()
        .is_some_and(|meta| meta.file_type().is_dir());
    let is_symlink = on_disk
        .as_ref()
        .is_some_and(|meta| meta.file_type().is_symlink());
    let shown = if is_dir {
        format!("{dir}/")
    } else {
        dir.to_string()
    };
    let refused = |why: &str| ChildrenError::Refused(format!("\"{shown}\" {why}"));
    let (staged, unstaged) =
        changed_files(worktree).map_err(|e| ChildrenError::Git(format!("{e:#}")))?;
    let files = match side {
        ChangesSide::Staged => &staged,
        ChangesSide::Unstaged => &unstaged,
    };
    let listed = match dux_core::model::listing_row_for(files, dir) {
        Some(row) if row.path == dir => row.is_expandable(),
        Some(_) => rows_answering(worktree, files, side, &[dir.to_string()])
            .map_err(|e| ChildrenError::Git(format!("{e:#}")))?
            .contains(dir),
        None => false,
    };
    if !listed {
        return Err(refused(
            "is not a folder the changes list shows; refresh the changes and expand it from its row",
        ));
    }
    // The unstaged side lists the working tree, so it must be a real directory
    // there. The staged side lists the index, which still holds a folder staged
    // whole and then deleted from disk; it is refused only when something at
    // the path is a symlink, which nothing may be read through.
    let listable = match side {
        ChangesSide::Unstaged => is_dir,
        ChangesSide::Staged => !is_symlink,
    };
    if !listable {
        return Err(refused("is not a folder in the worktree"));
    }
    changed_dir_children(worktree, dir, side).map_err(|e| ChildrenError::Git(format!("{e:#}")))
}

/// `GET /api/v1/sessions/:id/changes/children`: one level of a folded folder.
///
/// There is no server-side deadline, as for `/changes`: the read is a blocking
/// git call that cannot be cancelled once running. The browser holds the
/// request to `[server] changes_request_timeout_seconds` and gives up on it
/// with a sentence, and a superseded request is aborted by the browser too.
async fn get_children(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ChildrenQuery>,
) -> Response {
    if id.chars().count() > MAX_ID_LEN {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    let side = match query.side.as_str() {
        "staged" => dux_core::git::ChangesSide::Staged,
        "unstaged" => dux_core::git::ChangesSide::Unstaged,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "side must be \"staged\" or \"unstaged\"",
            )
                .into_response();
        }
    };
    let worktree = match resolve_worktree(&state, id).await {
        Ok(worktree) => worktree,
        Err(resp) => return resp.into_response(),
    };
    let dir = query.dir.clone();
    let wt = worktree.clone();
    let answer = tokio::task::spawn_blocking(move || list_children(&wt, &dir, side)).await;
    match answer {
        Ok(Ok(children)) => Json(ChildrenResponseBody {
            dir: query.dir,
            side: match side {
                dux_core::git::ChangesSide::Staged => "staged",
                dux_core::git::ChangesSide::Unstaged => "unstaged",
            },
            children: crate::changes::sorted_views(&children),
        })
        .into_response(),
        Ok(Err(ChildrenError::Refused(why))) => (
            StatusCode::BAD_REQUEST,
            dux_core::git::redact_worktree_path(&why, &worktree),
        )
            .into_response(),
        Ok(Err(ChildrenError::Git(detail))) => {
            dux_core::logger::warn(&format!("[web] could not list a folder: {detail}"));
            (
                StatusCode::CONFLICT,
                [(header::RETRY_AFTER, RETRY_AFTER_SECS.to_string())],
                format!(
                    "Could not list the folder. {}",
                    dux_core::git::redact_worktree_path(&detail, &worktree)
                ),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("git task failed: {e}"),
        )
            .into_response(),
    }
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
