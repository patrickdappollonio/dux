//! HTTP endpoints for the web code editor: list the worktree's files, read and
//! write a file's working copy, and open a file in a locally-installed GUI editor
//! (a process spawned ON the server). Request/response so the editor gets real
//! content + errors and can drive per-file loading/saving state.
//!
//! Security model (per the worktree-containment directive): the editor may touch
//! ANY path inside the worktree tree — tracked or not — and may create new files,
//! but NOTHING outside it. Containment is enforced by `dux_core`:
//! `resolve_worktree_path` rejects absolute/`..`/`.git` paths and symlinks that
//! escape, `worktree_file::{read,write}_file` additionally refuse symlinks and
//! (on create) validate the parent stays inside the tree. There is deliberately
//! NO git-tracked/changed-file gate here — that is the changes pane's concern;
//! the editor works against the worktree itself. The `list` endpoint returns
//! git's file set (tracked, untracked-not-ignored, AND loose gitignored files —
//! fully-ignored directories like node_modules are collapsed out) purely so the
//! tree is a clean, finite browse surface — it does not bound what is editable.
//! `open-in-editor` only spawns an editor (no extra capability beyond read/write
//! given the single-tenant trusted-access model); it is gated to local-access
//! clients in the UI and is a harmless no-op when spawned on a headless server.
//!
//! All routes are auth-gated and run the file I/O OFF the async reactor
//! (`spawn_blocking`). After a write, the engine recomputes changed files so the
//! new state reaches every connected client over the WebSocket ViewModel.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};

use crate::git_routes::resolve_worktree;
use crate::server::AppState;

#[derive(Deserialize)]
struct SessionOp {
    session_id: String,
}

#[derive(Deserialize)]
struct ReadOp {
    session_id: String,
    path: String,
}

#[derive(Deserialize)]
struct WriteOp {
    session_id: String,
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenInEditorOp {
    session_id: String,
    path: String,
    /// Which editor to open, as a dux-core editor config key/alias (e.g.
    /// "vscode", "zed"). When absent, the configured/preferred editor is used —
    /// the original auto-pick behavior.
    #[serde(default)]
    editor: Option<String>,
}

#[derive(Serialize)]
struct FileList {
    files: Vec<String>,
}

#[derive(Serialize)]
struct OpenedEditor {
    /// Human-readable editor label (e.g. "VS Code") for the success toast.
    editor: String,
}

/// The gated editor file routes, merged into the authenticated sub-router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/file/list", post(list_files))
        .route("/api/file/read", post(read_file))
        .route("/api/file/diff", post(diff_contents))
        .route("/api/file/write", post(write_file))
        .route("/api/file/open-in-editor", post(open_in_editor))
}

async fn list_files(State(state): State<AppState>, Json(op): Json<SessionOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    match tokio::task::spawn_blocking(move || dux_core::git::worktree_files(&worktree)).await {
        Ok(Ok(files)) => Json(FileList { files }).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list task failed: {e}"),
        )
            .into_response(),
    }
}

async fn read_file(State(state): State<AppState>, Json(op): Json<ReadOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::read_file(&worktree, &path))
        .await
    {
        Ok(Ok(file)) => Json(file).into_response(),
        // read_file's errors are path/containment/binary/size — client conditions.
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read task failed: {e}"),
        )
            .into_response(),
    }
}

/// Return the two raw sides (HEAD vs working copy) of a changed file so the web
/// editor can render a Monaco diff. Same worktree-relative path security as
/// `read`; binary content is reported via the `binary` flag with empty sides.
async fn diff_contents(State(state): State<AppState>, Json(op): Json<ReadOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::diff::file_diff_contents(&worktree, &path))
        .await
    {
        Ok(Ok(contents)) => Json(contents).into_response(),
        // file_diff_contents errors are mostly client conditions (path/containment,
        // too-large, symlink); a git/IO failure also lands here as 400, matching
        // read_file (both wrap dux_core errors without classifying them).
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("diff task failed: {e}"),
        )
            .into_response(),
    }
}

async fn write_file(State(state): State<AppState>, Json(op): Json<WriteOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let wt = worktree.clone();
    let path = op.path;
    let content = op.content;
    // write_file's errors are path/containment validation — client conditions, so
    // map them to 400.
    match tokio::task::spawn_blocking(move || {
        dux_core::worktree_file::write_file(&wt, &path, &content)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write task failed: {e}"),
            )
                .into_response();
        }
    }
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    StatusCode::OK.into_response()
}

/// Open a worktree file in a locally-installed GUI editor, reusing the TUI's
/// detection + launch path. `op.editor` (a dux-core editor config key like
/// "vscode") picks a specific editor — the web picker always sends one — and we
/// report "<editor> isn't installed" when it isn't on PATH. With no pick we fall
/// back to the configured/preferred editor (`config.editor.default`). The editor
/// is spawned on the SERVER machine, so this is only useful when the browser is on
/// that same machine — the web UI gates the picker to local-access URLs and
/// disables it for remote clients. On a headless/remote server the spawn simply
/// fails and we return the error. Containment is enforced by
/// `resolve_worktree_path` exactly like read/write, so no path outside the
/// worktree can be targeted.
async fn open_in_editor(State(state): State<AppState>, Json(op): Json<OpenInEditorOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let configured = state.engine.editor_default().await;
    let path = op.path;
    let requested = op.editor;
    // Detecting editors scans PATH and launching spawns a process — both blocking,
    // so run them off the async reactor.
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let abs = dux_core::git::resolve_worktree_path(&worktree, &path)?;
        if !abs.exists() {
            anyhow::bail!("file does not exist in the worktree");
        }
        let editors = dux_core::editor::detect_installed_editors();
        let choice = match requested {
            // An explicit pick from the web editor menu: launch THAT editor, or
            // report it isn't installed (naming it even when absent from PATH).
            Some(name) => {
                // The key comes from the fixed editor menu. Bound the length by
                // CHARS (never byte-slice user-facing input) and don't echo the raw
                // value back in the error — it could carry control characters.
                if name.chars().count() > 64 {
                    anyhow::bail!("unrecognized editor key");
                }
                let label = dux_core::editor::editor_label(&name)
                    .ok_or_else(|| anyhow::anyhow!("unrecognized editor key"))?;
                editors
                    .into_iter()
                    .find(|editor| dux_core::editor::matches_configured_editor(editor, &name))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "{label} isn't installed on this machine (no matching command on PATH)"
                        )
                    })?
            }
            // No pick: fall back to the configured/preferred editor.
            None => dux_core::editor::preferred_editor(&editors, &configured).ok_or_else(|| {
                anyhow::anyhow!(
                    "No supported editor found on PATH (install cursor, code, zed, vscodium, or sublime)"
                )
            })?,
        };
        dux_core::editor::launch_editor(&choice, &abs)?;
        Ok(choice.label.to_string())
    })
    .await;
    match result {
        Ok(Ok(editor)) => Json(OpenedEditor { editor }).into_response(),
        // Path/containment/no-editor/spawn failures are all client-actionable.
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("open-in-editor task failed: {e}"),
        )
            .into_response(),
    }
}
