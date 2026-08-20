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
//! the editor works against the worktree itself. The `tree` endpoint lazily
//! lists exactly ONE directory per request (no recursion, no cap) and backs the
//! editor's file tree; the `list` endpoint is a flat filesystem walk capped by
//! `[server] search_index_max_files` that backs ONLY the "Search files…" box.
//! Neither bounds what is editable.
//! `open-in-editor` only spawns an editor (no extra capability beyond read/write
//! given the single-tenant trusted-access model); it is gated to local-access
//! clients in the UI and is a harmless no-op when spawned on a headless server.
//!
//! A save may carry the freshness token the read handed out, as the pair
//! `expected_modified`/`expected_size`. With it, the write is refused with a 409 when the file
//! moved underneath the editor's buffer, which is the whole answer to an agent
//! and a browser editing the same file; without it the write is unconditional,
//! exactly as it always was. See `WriteOp` and
//! `dux_core::worktree_file::write_file_checked`.
//!
//! All routes are served like every other API route, with the host-allowlist
//! and same-origin guard applied app-wide, and run the file I/O OFF the async reactor
//! (`spawn_blocking`). After a write, the changed-files cache is invalidated so a
//! `session.changes` event reaches subscribed clients on `/ws/events`.

use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path as ApiPath, Query, State, rejection::JsonRejection,
    },
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use dux_core::model::TerminalRoute;

use crate::git_routes::resolve_worktree;
use crate::rest_common::{id_within_bound, unknown_session};
use crate::server::AppState;

/// Largest raw asset the markdown-preview proxy will serve. Bigger than the
/// editable-file cap (images/screenshots run larger than source files) but still
/// bounded so a single request can't buffer an unbounded blob into memory.
const MAX_RAW_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Deserialize)]
struct ReadOp {
    path: String,
}

#[derive(Deserialize)]
struct WriteOp {
    path: String,
    content: String,
    /// The freshness token the editor was handed by `read` (or by this route's
    /// own success body), echoed back so the server can refuse to overwrite a
    /// file that moved underneath the buffer.
    ///
    /// Optional, and the guard is opt-in BY PRESENCE of BOTH halves: a client
    /// that sends neither (an older page, any other writer) gets exactly the
    /// unconditional write this route has always done. Half a token is treated
    /// as no token: enforcing a size against an mtime nobody supplied would be
    /// comparing against a value the server invented. Both halves are needed
    /// because mtime granularity is coarse enough that two writes inside one
    /// tick share a timestamp, which is the very race the guard is for.
    #[serde(default)]
    expected_modified: Option<String>,
    #[serde(default)]
    expected_size: Option<u64>,
}

/// A save's success body: the file's stamp AFTER the write. The client
/// re-baselines on it so its own save is not mistaken for somebody else's edit
/// the moment the changed-files broadcast moves.
#[derive(Serialize)]
struct WriteResult {
    modified: Option<String>,
    size: u64,
}

/// A 409's body. `deleted` is its own field rather than being inferred from a
/// null `modified`, because a null mtime is a legitimate answer on a filesystem
/// that reports none, and "changed" and "deleted" are different rungs with
/// different offers in the UI (reload versus close-or-keep).
#[derive(Serialize)]
struct WriteConflictBody {
    modified: Option<String>,
    size: Option<u64>,
    deleted: bool,
}

#[derive(Deserialize)]
struct PathOp {
    path: String,
}

#[derive(Deserialize)]
struct RenameOp {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct OpenInEditorOp {
    path: String,
    /// Which editor to open, as a dux-core editor config key/alias (e.g.
    /// "vscode", "zed"). When absent, the configured/preferred editor is used —
    /// the original auto-pick behavior.
    #[serde(default)]
    editor: Option<String>,
}

/// Query for the raw-asset proxy. A GET so it can back an `<img src>`; the
/// session resolves the worktree (from the `:id` path segment), `path` is
/// worktree-relative.
#[derive(Deserialize)]
struct RawQuery {
    path: String,
}

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Serialize)]
struct FileList {
    files: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    truncated: bool,
}

/// Request for the lazy tree listing: one worktree-relative directory
/// (`""` = the worktree root).
#[derive(Deserialize)]
struct TreeOp {
    #[serde(default)]
    dir: String,
}

/// One directory's children for the lazy file tree, pre-sorted dirs-first.
#[derive(Serialize)]
struct TreeList {
    dir: String,
    entries: Vec<dux_core::git::DirEntryInfo>,
}

#[derive(Serialize)]
struct OpenedEditor {
    /// Human-readable editor label (e.g. "VS Code") for the success toast.
    editor: String,
}

/// Largest request body the editor's save route accepts.
///
/// It exists because the read and the write disagreed. The reader opens
/// anything up to `MAX_EDITABLE_BYTES` (5 MB), while the save route inherited
/// the framework's 2 MB default, so a 3 MB file opened in the editor and then
/// could not be saved, failing with the framework's terse length message that
/// names no cause. That is a functional bug, not a memory one.
///
/// It is not simply the read cap, because the content travels as a JSON STRING
/// and escaping grows it. serde_json escapes a quote, a backslash and the
/// newline/carriage-return/tab controls to two bytes each and passes every other
/// non-ASCII byte through untouched, so twice the read cap covers any text file,
/// even one made entirely of quotes. The extra 64 KB is room for the envelope
/// and the path.
///
/// The remaining case that does not fit is a file made largely of OTHER control
/// characters, which each escape to a six-character u-escape. Such a file is valid
/// UTF-8, so the reader will open it, but it is not a thing anybody edits, and
/// the refusal now says what the limit is instead of failing opaquely.
const MAX_EDIT_WRITE_BYTES: usize =
    2 * dux_core::worktree_file::MAX_EDITABLE_BYTES as usize + 64 * 1024;

/// What an editor request's paths are resolved against, decided by the ADDRESS
/// the request arrived at and never by the id alone.
///
/// Three addresses reach exactly the same handlers. `/api/v1/sessions/{id}/files/*`
/// roots at an agent's worktree; `/api/v1/terminals/{tid}/files/*` roots at a
/// STANDALONE terminal's spawn directory; and
/// `/api/v1/projects/{pid}/terminals/{tid}/files/*` roots at a project
/// terminal's. Each address is an extractor that answers the root or refuses
/// with a 404, so the guard for a namespace is written once and every handler
/// is generic over which one ran. There is deliberately no session-nested
/// terminal address: a session-owned terminal shares its agent's worktree, so
/// its editor IS the agent's editor and gets the agent's routes, diff mode and
/// changes broadcast along with it.
///
/// The terminal roots are pinned at spawn. That is the one place this parts
/// company with the file-drop tenet, which follows the shell's live directory
/// so a dropped file lands where the user is typing. An editor root backs a
/// tree, a set of buffers, their drafts and a bookmarkable URL, and a root that
/// moved when somebody typed `cd` would invalidate all four at once.
trait EditorRoot: Send + 'static {
    /// The absolute directory every path in the request resolves against.
    fn path(&self) -> &StdPath;

    /// The agent whose changed-files broadcast a mutation here should refresh.
    ///
    /// A terminal root answers `None`, and the absence is the design: it has no
    /// agent, no changes pane and no diff mode, so there is nothing for a
    /// broadcast to update. Freshness in a terminal-rooted editor rides the
    /// remaining two triggers, window focus and tab activation.
    fn changes_session(&self) -> Option<String>;

    /// The flat walk backing the "Search files…" box. A worktree walks its own
    /// dot directories (the editor opens `.git/config` through it); a terminal
    /// root prunes them, for the reasons on [`dux_core::git::rooted_files`].
    fn search_walk(
        root: &StdPath,
        max_files: usize,
    ) -> anyhow::Result<dux_core::git::WorktreeFileList>;
}

/// An agent's worktree, from `/api/v1/sessions/{id}/files/*`.
struct SessionRoot {
    worktree: PathBuf,
    session_id: String,
}

impl EditorRoot for SessionRoot {
    fn path(&self) -> &StdPath {
        &self.worktree
    }
    fn changes_session(&self) -> Option<String> {
        Some(self.session_id.clone())
    }
    fn search_walk(
        root: &StdPath,
        max_files: usize,
    ) -> anyhow::Result<dux_core::git::WorktreeFileList> {
        dux_core::git::worktree_files(root, max_files)
    }
}

impl FromRequestParts<AppState> for SessionRoot {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let ApiPath(id) = ApiPath::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| unknown_session())?;
        if !id_within_bound(&id) {
            return Err(unknown_session());
        }
        let worktree = resolve_worktree(state, id.clone()).await?;
        Ok(Self {
            worktree,
            session_id: id,
        })
    }
}

/// A standalone terminal's pinned spawn directory, from
/// `/api/v1/terminals/{tid}/files/*`.
struct StandaloneTerminalRoot(PathBuf);

/// A project terminal's pinned spawn directory, from
/// `/api/v1/projects/{pid}/terminals/{tid}/files/*`.
struct ProjectTerminalRoot(PathBuf);

/// The shared body of both terminal roots. Only the extractors differ, because
/// only the address differs.
macro_rules! terminal_root {
    ($ty:ident) => {
        impl EditorRoot for $ty {
            fn path(&self) -> &StdPath {
                &self.0
            }
            fn changes_session(&self) -> Option<String> {
                None
            }
            fn search_walk(
                root: &StdPath,
                max_files: usize,
            ) -> anyhow::Result<dux_core::git::WorktreeFileList> {
                dux_core::git::rooted_files(root, max_files)
            }
        }
    };
}

terminal_root!(StandaloneTerminalRoot);
terminal_root!(ProjectTerminalRoot);

impl FromRequestParts<AppState> for StandaloneTerminalRoot {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let ApiPath(tid) = ApiPath::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| unknown_terminal())?;
        resolve_terminal_root(state, TerminalRoute::Standalone, &tid)
            .await
            .map(Self)
    }
}

impl FromRequestParts<AppState> for ProjectTerminalRoot {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let ApiPath((pid, tid)) = ApiPath::<(String, String)>::from_request_parts(parts, state)
            .await
            .map_err(|_| unknown_terminal())?;
        if !id_within_bound(&pid) {
            return Err(unknown_terminal());
        }
        resolve_terminal_root(state, TerminalRoute::Project(&pid), &tid)
            .await
            .map(Self)
    }
}

/// The one place a terminal id becomes an editor root, for every terminal
/// address there is.
///
/// It takes the ROUTE NAMESPACE, never just the id, and hands it to the
/// exhaustive [`dux_core::model::TerminalOwner::is_at_route`] that the terminal
/// delete routes already enforce membership with. That is what makes a session
/// id, a project terminal at the un-nested address, or a terminal belonging to
/// another project a 404 rather than a way to read a directory the address
/// never named.
async fn resolve_terminal_root(
    state: &AppState,
    route: TerminalRoute<'_>,
    tid: &str,
) -> Result<PathBuf, Response> {
    if !id_within_bound(tid) {
        return Err(unknown_terminal());
    }
    match state.engine.terminal_root(tid.to_string()).await {
        Some((owner, root)) if owner.is_at_route(route) => Ok(root),
        _ => Err(unknown_terminal()),
    }
}

fn unknown_terminal() -> Response {
    (StatusCode::NOT_FOUND, "unknown terminal").into_response()
}

/// Tell an agent's changes pane that a file moved underneath it, when there is
/// an agent to tell. A terminal root answers no session, so a write there emits
/// nothing: there is no changes pane listing that directory, and inventing a
/// broadcast would mean naming an agent whose worktree the file is not in.
fn refresh_root_changes<R: EditorRoot>(state: &AppState, root: &R, worktree: &StdPath) {
    if let Some(session_id) = root.changes_session() {
        crate::git_routes::refresh_changed_files_now(state, session_id, worktree);
    }
}

/// The editor file routes for one address, generic over what that address roots
/// at. Registering the same handlers three times is the whole mechanism: the
/// guards live in the extractor and nothing below this line knows which
/// namespace it is serving.
fn editor_file_routes<R>(prefix: &str) -> Router<AppState>
where
    R: EditorRoot + FromRequestParts<AppState, Rejection = Response>,
{
    Router::new()
        .route(&format!("{prefix}/list"), post(list_files::<R>))
        .route(&format!("{prefix}/tree"), post(list_tree::<R>))
        .route(&format!("{prefix}/read"), post(read_file::<R>))
        .route(&format!("{prefix}/raw"), get(read_raw::<R>))
        .route(
            &format!("{prefix}/write"),
            // Set EXPLICITLY, or the framework's 2 MB default applies and a file
            // the editor was willing to OPEN cannot be saved. See
            // [`MAX_EDIT_WRITE_BYTES`] for the size and why it is not simply the
            // read cap.
            post(write_file::<R>).layer(DefaultBodyLimit::max(MAX_EDIT_WRITE_BYTES)),
        )
        .route(&format!("{prefix}/info"), post(entry_info::<R>))
        .route(&format!("{prefix}/create-file"), post(create_file::<R>))
        .route(&format!("{prefix}/create-dir"), post(create_dir::<R>))
        .route(&format!("{prefix}/rename"), post(rename_entry::<R>))
        .route(&format!("{prefix}/delete"), post(delete_entry::<R>))
        .route(
            &format!("{prefix}/open-in-editor"),
            post(open_in_editor::<R>),
        )
}

/// The editor file routes. These are path-keyed: the id is a path segment,
/// validated by `id_within_bound` and resolved to a root by the address's own
/// extractor before any handler runs (mirroring the other resource-nested REST
/// routes). The `raw` proxy is a GET with a root-relative `?path=` query.
pub fn routes() -> Router<AppState> {
    let session = "/api/v1/sessions/{id}/files";
    Router::new()
        .merge(editor_file_routes::<SessionRoot>(session))
        // `diff` is registered for the AGENT address only. A terminal-rooted
        // editor has no diff mode, so the route it would call does not exist:
        // the affordance is absent rather than disabled, and there is no
        // half-working endpoint for a client to find.
        .route(&format!("{session}/diff"), post(diff_contents))
        .merge(editor_file_routes::<StandaloneTerminalRoot>(
            "/api/v1/terminals/{tid}/files",
        ))
        .merge(editor_file_routes::<ProjectTerminalRoot>(
            "/api/v1/projects/{pid}/terminals/{tid}/files",
        ))
}

/// The flat search walk. Bounded by `state.tree_list_semaphore`, the same permit
/// `list_tree` takes: this is the more expensive of the two (a whole recursive
/// walk against one `read_dir`), so leaving it unbounded while the cheap one was
/// capped had the accounting backwards.
async fn list_files<R: EditorRoot>(State(state): State<AppState>, root: R) -> Response {
    let _permit = match tree_list_permit(&state).await {
        Ok(permit) => permit,
        Err(resp) => return resp,
    };
    let worktree = root.path().to_path_buf();
    let max_files = state.search_index_max_files;
    match tokio::task::spawn_blocking(move || R::search_walk(&worktree, max_files)).await {
        Ok(Ok(listing)) => Json(FileList {
            files: listing.files,
            truncated: listing.truncated,
        })
        .into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list task failed: {e}"),
        )
            .into_response(),
    }
}

/// A permit from `state.tree_list_semaphore` (`[server] tree_list_max_concurrency`)
/// so a burst of directory work cannot exhaust the server's blocking-thread
/// pool. A request beyond the limit WAITS for a free permit
/// (`acquire_owned().await`) rather than being rejected: this is a small, fast
/// unit of background work, not a long-lived connection like the
/// `ws_*_semaphore` classes, which 503 on exhaustion instead. `None` means the
/// config value is 0 (unlimited) and no permit is taken at all.
///
/// One config edge: the permit also serializes `list_files`, whose walk is
/// bounded by `[server] search_index_max_files`, and with that cap set to 0
/// (disabled) a `/`-rooted terminal editor can hold a permit for a very long
/// walk; the default cap bounds the hold, and an operator who disables it has
/// chosen unbounded work.
async fn tree_list_permit(
    state: &AppState,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, Response> {
    match &state.tree_list_semaphore {
        Some(sem) => match Arc::clone(sem).acquire_owned().await {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "tree listing semaphore closed unexpectedly",
            )
                .into_response()),
        },
        None => Ok(None),
    }
}

/// Lazy tree listing: one directory per request, no recursion, no cap. The
/// blocking `read_dir` runs off the async reactor via `spawn_blocking`, bounded
/// by [`tree_list_permit`].
async fn list_tree<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<TreeOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
    let _permit = match tree_list_permit(&state).await {
        Ok(permit) => permit,
        Err(resp) => return resp,
    };
    let dir = op.dir;
    let dir_echo = dir.clone();
    match tokio::task::spawn_blocking(move || dux_core::git::list_dir(&worktree, &dir)).await {
        Ok(Ok(entries)) => Json(TreeList {
            dir: dir_echo,
            entries,
        })
        .into_response(),
        // list_dir errors are containment/traversal/missing-dir conditions the
        // client caused — surface them as a 400, not a server error.
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("tree task failed: {e}"),
        )
            .into_response(),
    }
}

async fn read_file<R: EditorRoot>(root: R, Json(op): Json<ReadOp>) -> Response {
    let worktree = root.path().to_path_buf();
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
async fn diff_contents(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<ReadOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    let worktree = match resolve_worktree(&state, id).await {
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

/// Serve a worktree file's raw bytes for the markdown preview's relative-image
/// proxy (an `<img src>` backed by this GET). Uses the same read-permissive
/// resolver as `read_file` so `.git/` assets and symlinked images reach this
/// proxy. Symlinks are followed: `canonicalize()` resolves the real target,
/// then `read_nofollow` re-opens it with `O_NOFOLLOW` to close the TOCTOU
/// window between the canonicalize and the read. The write path is unaffected.
/// Content-Type is guessed from the extension; SVGs served to `<img>` never
/// run scripts. Served like every other `/api/v1/sessions/:id/files/*` route,
/// with the host-allowlist and same-origin guard applied app-wide.
///
/// Containment is enforced in two stages:
///
/// 1. `resolve_worktree_path_for_read` catches outside-resolving symlinks at
///    the resolution stage and sets `is_outside = true`. We reject those
///    immediately — the image proxy must not serve files outside the worktree.
/// 2. After following a leaf symlink with `canonicalize()` we re-verify that
///    the resolved target is still inside the worktree's canonical root. This
///    closes any TOCTOU gap between the resolver's containment check and the
///    moment we actually read the file (a symlink could be replaced between the
///    two calls).
///
/// Note: `read_file` intentionally ALLOWS outside-resolving symlinks (marking
/// them `read_only: true`) so the editor can display them. We do NOT change
/// that behaviour here; this restriction is image-proxy–only.
async fn read_raw<R: EditorRoot>(root: R, Query(q): Query<RawQuery>) -> Response {
    let worktree = root.path().to_path_buf();
    let path = q.path;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(&'static str, Vec<u8>)> {
        // Use the read-permissive resolver so symlinked images inside the
        // worktree reach this proxy. The image proxy intentionally does NOT
        // serve .git/ internals — those never contain renderable assets and
        // exposing them (e.g. .git/config, pack files) is unnecessary risk.
        let (abs, is_git, is_outside) =
            dux_core::worktree_file::resolve_worktree_path_for_read(&worktree, &path)?;

        // Stage 1: reject immediately when the resolver determined the path
        // escapes the worktree via a symlink. The image proxy must not serve
        // host files outside the worktree.
        if is_outside {
            anyhow::bail!("refusing to serve path outside the worktree");
        }

        // Also refuse .git/ internals — they are not renderable image assets.
        if is_git {
            anyhow::bail!("refusing to serve git internal path via image proxy");
        }

        let meta = std::fs::symlink_metadata(&abs)?;
        // Resolve symlinks to get the real target for the size check and read.
        let real = if meta.file_type().is_symlink() {
            std::fs::canonicalize(&abs)? // follows the link; dangling → error
        } else {
            abs.clone()
        };

        // Stage 2: re-verify containment after following a leaf symlink.
        // The resolver canonicalizes the joined path (worktree + rel_path);
        // if a leaf symlink was swapped between the resolver call and here,
        // the canonicalize above reflects the NEW target. Re-check it against
        // the canonical worktree root to guarantee the target is still inside.
        if meta.file_type().is_symlink() {
            let wt_real = std::fs::canonicalize(&worktree)
                .map_err(|e| anyhow::anyhow!("cannot canonicalize worktree: {e}"))?;
            if !real.starts_with(&wt_real) {
                anyhow::bail!("refusing to serve symlink target outside worktree");
            }
        }

        let real_meta = std::fs::metadata(&real)?;
        if real_meta.len() > MAX_RAW_BYTES {
            anyhow::bail!(
                "file too large to serve: {} bytes (limit {MAX_RAW_BYTES})",
                real_meta.len()
            );
        }
        // Use read_nofollow (O_NOFOLLOW) to close the TOCTOU window between
        // canonicalize() above and the actual read. If `real` was swapped to a
        // symlink in the interim, the open fails safely rather than following it.
        let bytes = dux_core::worktree_file::read_nofollow(&real)?;
        Ok((mime_for_path(&path), bytes))
    })
    .await;
    match result {
        Ok(Ok((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime),
                // Working-copy content can change between views; don't let a stale
                // image stick in the browser cache.
                (header::CACHE_CONTROL, "no-cache"),
                // Defense against a same-origin stored XSS: an `<img src>` never
                // runs scripts, but navigating DIRECTLY to this URL ("open image in
                // new tab") would render the response as a top-level document in
                // dux's origin — and an SVG document can carry <script>. CSP sandbox
                // strips script execution from such a top-level render; nosniff
                // blocks MIME-confusion; attachment makes a direct navigation
                // download instead of render. None of these affect <img> subresource
                // rendering, so legit markdown images still display.
                (header::CONTENT_SECURITY_POLICY, "sandbox"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::CONTENT_DISPOSITION, "attachment"),
            ],
            bytes,
        )
            .into_response(),
        // Path/containment/symlink/size are client-actionable.
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("raw task failed: {e}"),
        )
            .into_response(),
    }
}

/// Best-effort Content-Type from a path's extension — enough for the image types
/// markdown references; anything else falls back to a generic binary type.
fn mime_for_path(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// The root extractor runs before the body is read, so an address problem (an
/// unknown or malformed id) answers 404 before any body diagnostic, oversize
/// included; that extractor-first ordering is deliberate and pinned by
/// `a_bad_id_beats_the_body_diagnostic` below.
async fn write_file<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    op: Result<Json<WriteOp>, JsonRejection>,
) -> Response {
    // Taken as a Result so the OVER-SIZE case can say what happened. Left to the
    // framework it is a bare "length limit exceeded" with no number in it and no
    // hint that the file is simply too big to save, which is the one rejection a
    // user can actually provoke by opening a large file and typing in it.
    let Json(op) = match op {
        Ok(op) => op,
        Err(rejection) => return write_rejection(rejection),
    };
    let worktree = root.path().to_path_buf();
    let wt = worktree.clone();
    let path = op.path;
    let content = op.content;
    // Both halves or nothing; see `WriteOp::expected_modified`.
    let expected = op
        .expected_size
        .zip(op.expected_modified)
        .map(|(size, modified)| dux_core::worktree_file::FileStamp {
            modified: Some(modified),
            size,
        });
    // write_file's errors are path/containment validation — client conditions, so
    // map them to 400. The one exception is a freshness conflict, which is a
    // 409 carrying the file's CURRENT stamp so the browser can offer a choice
    // (overwrite, reload, cancel) without another round trip.
    let stamp = match tokio::task::spawn_blocking(move || {
        dux_core::worktree_file::write_file_checked(&wt, &path, &content, expected.as_ref())
    })
    .await
    {
        Ok(Ok(stamp)) => stamp,
        Ok(Err(e)) => {
            if let Some(conflict) = e.downcast_ref::<dux_core::worktree_file::WriteConflict>() {
                return (
                    StatusCode::CONFLICT,
                    Json(WriteConflictBody {
                        modified: conflict.current.as_ref().and_then(|s| s.modified.clone()),
                        size: conflict.current.as_ref().map(|s| s.size),
                        deleted: conflict.deleted,
                    }),
                )
                    .into_response();
            }
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write task failed: {e}"),
            )
                .into_response();
        }
    };
    refresh_root_changes(&state, &root, &worktree);
    Json(WriteResult {
        modified: stamp.modified,
        size: stamp.size,
    })
    .into_response()
}

/// Turn a save-route body rejection into a response the user can act on.
///
/// Only the size case is reworded; everything else (a malformed body, a wrong
/// content type) keeps the framework's own wording, which is already accurate
/// and is not something a user provokes. The size case reports the cap in MB and
/// says the two things that are true: the file is too large to save through the
/// editor, and it can still be edited outside dux.
fn write_rejection(rejection: JsonRejection) -> Response {
    let too_large = matches!(rejection, JsonRejection::BytesRejection(_))
        || rejection.status() == StatusCode::PAYLOAD_TOO_LARGE;
    if too_large {
        let limit_mb = MAX_EDIT_WRITE_BYTES / (1024 * 1024);
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "This file is too large for the editor to save: the request body \
                 is over the {limit_mb} MB limit. Edit it outside dux instead."
            ),
        )
            .into_response();
    }
    (rejection.status(), rejection.body_text()).into_response()
}

/// Describe one worktree entry for the editor's read-only file-info panel:
/// path, kind, size, modified time, permissions, and what git says about it.
/// Read-only and side-effect free, so unlike every mutating route below it does
/// NOT touch the changed-files cache. Containment is
/// `dux_core::worktree_file::entry_info`'s, which is the same boundary the
/// write path uses. The git lookup shells out, so the whole thing runs off the
/// async reactor.
async fn entry_info<R: EditorRoot>(root: R, Json(op): Json<PathOp>) -> Response {
    let worktree = root.path().to_path_buf();
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::entry_info(&worktree, &path))
        .await
    {
        Ok(Ok(info)) => Json(info).into_response(),
        // A path that resolved cleanly but is GONE answers 404, while a path
        // that was refused (traversal, `.git`, an escaping symlink) answers
        // 400. The browser's info panel self-dismisses on the 404 only.
        Ok(Err(e)) => {
            let status = if e
                .downcast_ref::<dux_core::worktree_file::EntryMissing>()
                .is_some()
            {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, e.to_string()).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("info task failed: {e}"),
        )
            .into_response(),
    }
}

/// Create a new empty file at `op.path`. Refuses an already-existing entry and
/// a missing parent, same as `write_file`'s create arm. See
/// `dux_core::worktree_file::create_file` for the containment guards.
async fn create_file<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<PathOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
    let wt = worktree.clone();
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::create_file(&wt, &path))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create-file task failed: {e}"),
            )
                .into_response();
        }
    }
    refresh_root_changes(&state, &root, &worktree);
    StatusCode::OK.into_response()
}

/// Create a new directory at `op.path`, creating missing intermediate
/// components. See `dux_core::worktree_file::create_dir`.
async fn create_dir<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<PathOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
    let wt = worktree.clone();
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::create_dir(&wt, &path)).await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create-dir task failed: {e}"),
            )
                .into_response();
        }
    }
    refresh_root_changes(&state, &root, &worktree);
    StatusCode::OK.into_response()
}

/// Rename/move `op.from` to `op.to` (file or directory). See
/// `dux_core::worktree_file::rename_entry`.
async fn rename_entry<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<RenameOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
    let wt = worktree.clone();
    let from = op.from;
    let to = op.to;
    match tokio::task::spawn_blocking(move || {
        dux_core::worktree_file::rename_entry(&wt, &from, &to)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("rename task failed: {e}"),
            )
                .into_response();
        }
    }
    refresh_root_changes(&state, &root, &worktree);
    StatusCode::OK.into_response()
}

/// Delete `op.path` (file or, recursively, a directory). Permanent: no trash on
/// the server. See `dux_core::worktree_file::delete_entry`.
async fn delete_entry<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<PathOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
    let wt = worktree.clone();
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::delete_entry(&wt, &path))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("delete task failed: {e}"),
            )
                .into_response();
        }
    }
    refresh_root_changes(&state, &root, &worktree);
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
async fn open_in_editor<R: EditorRoot>(
    State(state): State<AppState>,
    root: R,
    Json(op): Json<OpenInEditorOp>,
) -> Response {
    let worktree = root.path().to_path_buf();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_for_path_maps_image_extensions_case_insensitively() {
        assert_eq!(mime_for_path("assets/logo.png"), "image/png");
        assert_eq!(mime_for_path("a/b/Photo.JPG"), "image/jpeg");
        assert_eq!(mime_for_path("x.jpeg"), "image/jpeg");
        assert_eq!(mime_for_path("icon.svg"), "image/svg+xml");
        assert_eq!(mime_for_path("anim.GIF"), "image/gif");
        assert_eq!(mime_for_path("p.webp"), "image/webp");
    }

    #[test]
    fn mime_for_path_falls_back_for_unknown_or_extensionless() {
        assert_eq!(mime_for_path("README"), "application/octet-stream");
        assert_eq!(mime_for_path("notes.txt"), "application/octet-stream");
        // A dot in a directory name must not be read as the file's extension.
        assert_eq!(mime_for_path("v1.2/Makefile"), "application/octet-stream");
    }

    /// Symlink containment tests for the `read_raw` image proxy.
    ///
    /// These tests exercise the two-stage containment logic in `read_raw`'s
    /// blocking closure at the filesystem level, mirroring what the handler does:
    ///
    /// Stage 1 — `resolve_worktree_path_for_read` sets `is_outside = true` when
    ///            the symlink resolves outside the worktree.
    /// Stage 2 — After `canonicalize()` on a leaf symlink, we re-verify the
    ///            target is still under the canonical worktree root.
    ///
    /// RED before fix: outside symlinks would pass through; GREEN after fix: they
    /// are rejected, while in-worktree symlinks are still served.
    mod symlink_containment {
        use std::fs;
        use std::path::PathBuf;

        use dux_core::worktree_file::resolve_worktree_path_for_read;

        /// Build a minimal temp directory layout:
        ///   <tmp>/
        ///     worktree/
        ///       real.png          ← the actual image inside the worktree
        ///       inlink.png        → real.png          (symlink INSIDE)
        ///       outlink.png       → <tmp>/outside.png (symlink OUTSIDE)
        ///     outside.png         ← file that must NOT be reachable via the proxy
        fn setup_dirs() -> (tempfile::TempDir, PathBuf) {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path();

            let worktree = root.join("worktree");
            fs::create_dir_all(&worktree).unwrap();

            // A real file inside the worktree.
            let real = worktree.join("real.png");
            fs::write(&real, b"\x89PNG\r\n\x1a\n").unwrap(); // minimal PNG header

            // In-worktree symlink: worktree/inlink.png → worktree/real.png
            let inlink = worktree.join("inlink.png");
            std::os::unix::fs::symlink(&real, &inlink).unwrap();

            // Outside file: <tmp>/outside.png
            let outside = root.join("outside.png");
            fs::write(&outside, b"SECRET").unwrap();

            // Out-of-worktree symlink: worktree/outlink.png → <tmp>/outside.png
            let outlink = worktree.join("outlink.png");
            std::os::unix::fs::symlink(&outside, &outlink).unwrap();

            (tmp, worktree)
        }

        /// Stage 1 containment: a symlink whose target is OUTSIDE the worktree
        /// must have `is_outside = true` so the handler can reject it immediately.
        #[test]
        fn outside_symlink_is_flagged_by_resolver() {
            let (_tmp, worktree) = setup_dirs();
            let (_, _, is_outside) =
                resolve_worktree_path_for_read(&worktree, "outlink.png").unwrap();
            assert!(
                is_outside,
                "outlink.png resolves outside the worktree — is_outside must be true"
            );
        }

        /// Stage 1 containment: the handler must REFUSE an outside-resolving symlink.
        /// This mirrors the `if is_outside { bail!(...) }` guard in `read_raw`.
        #[test]
        fn outside_symlink_is_refused_by_read_raw_logic() {
            let (_tmp, worktree) = setup_dirs();
            let result: anyhow::Result<()> = (|| {
                let (_, _, is_outside) = resolve_worktree_path_for_read(&worktree, "outlink.png")?;
                if is_outside {
                    anyhow::bail!("refusing to serve path outside the worktree");
                }
                Ok(())
            })();
            assert!(
                result.is_err(),
                "read_raw must refuse a symlink whose target is outside the worktree"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("refusing"),
                "error message should say 'refusing', got: {msg}"
            );
        }

        /// Stage 2 containment: after `canonicalize()` on a leaf symlink, we
        /// re-verify the target is inside the worktree. This mirrors the
        /// `!real.starts_with(&wt_real)` guard in `read_raw`.
        #[test]
        fn stage2_rejects_out_of_tree_canonicalized_target() {
            let (_tmp, worktree) = setup_dirs();
            let outlink = worktree.join("outlink.png");

            // Replicate stage-2 logic exactly as written in the handler.
            let result: anyhow::Result<()> = (|| {
                let real = std::fs::canonicalize(&outlink)?;
                let wt_real = std::fs::canonicalize(&worktree)
                    .map_err(|e| anyhow::anyhow!("cannot canonicalize worktree: {e}"))?;
                if !real.starts_with(&wt_real) {
                    anyhow::bail!("refusing to serve symlink target outside worktree");
                }
                Ok(())
            })();

            assert!(
                result.is_err(),
                "stage-2 check must reject a canonicalized target outside the worktree"
            );
        }

        /// A symlink whose target is INSIDE the worktree must NOT be flagged as
        /// outside, and the stage-2 canonicalize check must pass — the proxy must
        /// continue to serve in-worktree images correctly.
        #[test]
        fn inside_symlink_is_allowed() {
            let (_tmp, worktree) = setup_dirs();

            // Stage 1: resolver must NOT flag the in-worktree link as outside.
            let (abs, _, is_outside) =
                resolve_worktree_path_for_read(&worktree, "inlink.png").unwrap();
            assert!(
                !is_outside,
                "inlink.png resolves inside the worktree — is_outside must be false"
            );

            // Stage 2: canonicalize and re-verify containment.
            let meta = std::fs::symlink_metadata(&abs).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "inlink.png must be a symlink"
            );

            let real = std::fs::canonicalize(&abs).unwrap();
            let wt_real = std::fs::canonicalize(&worktree).unwrap();
            assert!(
                real.starts_with(&wt_real),
                "canonicalized target of inlink.png must be inside the worktree"
            );
        }
    }

    /// `read_raw` git-dir guard: the image proxy must refuse `.git/` paths even
    /// though `resolve_worktree_path_for_read` permits them (for the text editor).
    mod git_dir_guard {
        use dux_core::worktree_file::resolve_worktree_path_for_read;

        fn setup_worktree_with_git() -> tempfile::TempDir {
            let dir = tempfile::tempdir().expect("tempdir");
            let wt = dir.path();
            // Minimal .git directory with a config file (stands in for any git internal).
            std::fs::create_dir(wt.join(".git")).unwrap();
            std::fs::write(
                wt.join(".git/config"),
                "[core]\n\trepositoryformatversion = 0\n",
            )
            .unwrap();
            // A normal image inside the worktree.
            std::fs::write(wt.join("logo.png"), b"\x89PNG\r\n\x1a\n").unwrap();
            dir
        }

        /// `.git/config` must have `is_git = true` from the resolver so the
        /// guard in `read_raw` can reject it.
        #[test]
        fn git_config_is_flagged_as_git_dir() {
            let dir = setup_worktree_with_git();
            let (_, is_git, _) = resolve_worktree_path_for_read(dir.path(), ".git/config").unwrap();
            assert!(
                is_git,
                ".git/config must be flagged as a git-dir path by the resolver"
            );
        }

        /// Mirroring `read_raw`'s `if is_git { bail! }` guard: a `.git/` path
        /// must be refused by the image proxy logic.
        #[test]
        fn read_raw_refuses_git_internal_path() {
            let dir = setup_worktree_with_git();
            let result: anyhow::Result<()> = (|| {
                let (_, is_git, is_outside) =
                    resolve_worktree_path_for_read(dir.path(), ".git/config")?;
                if is_outside {
                    anyhow::bail!("refusing to serve path outside the worktree");
                }
                if is_git {
                    anyhow::bail!("refusing to serve git internal path via image proxy");
                }
                Ok(())
            })();
            assert!(result.is_err(), "read_raw must refuse .git/ paths");
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("git internal"),
                "error should mention 'git internal', got: {msg}"
            );
        }

        /// A normal image inside the worktree must NOT be refused.
        #[test]
        fn read_raw_allows_normal_in_worktree_image() {
            let dir = setup_worktree_with_git();
            let (_, is_git, is_outside) =
                resolve_worktree_path_for_read(dir.path(), "logo.png").unwrap();
            assert!(!is_outside, "logo.png must not be flagged as outside");
            assert!(!is_git, "logo.png must not be flagged as git-dir");
        }
    }

    /// Endpoint tests for the four VS Code-style file management routes:
    /// create-file, create-dir, rename, delete. Boots a real engine with a
    /// session pointed at a real worktree directory (not a git repo: these
    /// operations don't need git, only the containment guards in
    /// `dux_core::worktree_file`), mirroring `changes.rs`'s `boot()` helper but
    /// serving requests through the real axum router.
    mod fs_op_endpoints {
        use axum::body::to_bytes;
        use axum::http::{Request, StatusCode};
        use dux_core::config::{DuxPaths, ProjectConfig};
        use dux_core::storage::SessionStore;
        use tower::ServiceExt;

        fn now() -> chrono::DateTime<chrono::Utc> {
            chrono::Utc::now()
        }

        fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
            let n = now();
            dux_core::model::AgentSession {
                id: id.to_string(),
                provider: dux_core::model::ProviderKind::new("claude"),
                title: None,
                started_providers: Vec::new(),
                desired_running: true,
                auto_reopen_enabled: false,
                status: dux_core::model::SessionStatus::Detached,
                created_at: n,
                updated_at: n,
                last_focused_tab: None,
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: worktree.to_string(),
                    },
                ),
            }
        }

        /// Boots a real router with session "s1" pointed at a fresh worktree
        /// directory containing `hello.txt`. Returns the temp root (kept alive
        /// for the test), the worktree path, and the router.
        async fn router_with_session() -> (tempfile::TempDir, std::path::PathBuf, axum::Router) {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            let wt = root.join("wt");
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join("hello.txt"), "hi\n").unwrap();

            let paths = DuxPaths {
                root: root.clone(),
                config_path: root.join("config.toml"),
                sessions_db_path: root.join("sessions.sqlite3"),
                worktrees_root: root.join("worktrees"),
                lock_path: root.join("dux.lock"),
            };
            std::fs::create_dir_all(&paths.worktrees_root).unwrap();
            {
                let store = SessionStore::open(&paths.sessions_db_path).unwrap();
                store
                    .upsert_project(&ProjectConfig {
                        id: "p1".to_string(),
                        path: root.to_string_lossy().into_owned(),
                        name: Some("p1".to_string()),
                        default_provider: None,
                        leading_branch: None,
                        auto_reopen_agents: None,
                        startup_command: None,
                        env: Default::default(),
                    })
                    .unwrap();
                store
                    .upsert_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
                    .unwrap();
            }
            let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
            let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
            let app = crate::server::router(handle);
            (tmp, wt, app)
        }

        fn json_req(uri: &str, body: serde_json::Value) -> Request<axum::body::Body> {
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        }

        #[tokio::test]
        async fn create_file_endpoint_creates_an_empty_file() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-file",
                    serde_json::json!({ "path": "new.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(std::fs::read_to_string(wt.join("new.txt")).unwrap(), "");
        }

        #[tokio::test]
        async fn create_file_endpoint_rejects_overwrite_with_400() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-file",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn create_file_endpoint_rejects_git_path() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-file",
                    serde_json::json!({ "path": ".git/evil" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn create_dir_endpoint_creates_a_directory() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-dir",
                    serde_json::json!({ "path": "a/b/c" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(wt.join("a/b/c").is_dir());
        }

        #[tokio::test]
        async fn create_dir_endpoint_rejects_existing_entry_with_400() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-dir",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn rename_endpoint_renames_a_file() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/rename",
                    serde_json::json!({ "from": "hello.txt", "to": "renamed.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(!wt.join("hello.txt").exists());
            assert!(wt.join("renamed.txt").exists());
        }

        #[tokio::test]
        async fn rename_endpoint_rejects_existing_destination_with_400() {
            let (_tmp, wt, app) = router_with_session().await;
            std::fs::write(wt.join("dst.txt"), "already here\n").unwrap();
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/rename",
                    serde_json::json!({ "from": "hello.txt", "to": "dst.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            // The refused rename must not overwrite the destination's content
            // (no partial/silent clobber on a rejected no-overwrite request).
            assert_eq!(
                std::fs::read_to_string(wt.join("dst.txt")).unwrap(),
                "already here\n"
            );
        }

        #[tokio::test]
        async fn rename_endpoint_rejects_git_destination_with_400() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/rename",
                    serde_json::json!({ "from": "hello.txt", "to": ".git/evil" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn delete_endpoint_deletes_a_file() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/delete",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(!wt.join("hello.txt").exists());
        }

        #[tokio::test]
        async fn delete_endpoint_rejects_worktree_root_with_400() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/delete",
                    serde_json::json!({ "path": "." }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            assert!(wt.exists());
        }

        #[tokio::test]
        async fn delete_endpoint_rejects_git_path_with_400() {
            let (_tmp, wt, app) = router_with_session().await;
            std::fs::create_dir(wt.join(".git")).unwrap();
            std::fs::write(wt.join(".git/config"), "[core]\n").unwrap();
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/delete",
                    serde_json::json!({ "path": ".git/config" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            assert!(wt.join(".git/config").exists());
        }

        #[tokio::test]
        async fn create_file_endpoint_unknown_session_is_404() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/does-not-exist/files/create-file",
                    serde_json::json!({ "path": "new.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }

        /// A successful mutation must invalidate the REST changed-files cache so a
        /// subsequent GET recomputes instead of serving a stale snapshot from
        /// before the mutation. We can't easily observe the `/ws/events` push in
        /// a oneshot test, so this asserts the response is OK and the body is
        /// empty (the documented no-content success contract every route shares).
        #[tokio::test]
        async fn create_file_endpoint_returns_no_content_body_on_success() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/create-file",
                    serde_json::json!({ "path": "empty-body.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            assert!(bytes.is_empty());
        }

        /// The save route must accept a file the read route was willing to open.
        /// The default limit is 2 MB and the read cap is 5 MB, so a 3 MB file
        /// used to open and then fail to save.
        #[tokio::test]
        async fn a_file_larger_than_the_framework_default_still_saves() {
            let (_tmp, wt, app) = router_with_session().await;
            let content = "a".repeat(3 * 1024 * 1024);
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({ "path": "big.txt", "content": content }),
                ))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "a 3 MB save is inside the editor's read cap and must go through"
            );
            assert_eq!(
                std::fs::metadata(wt.join("big.txt")).unwrap().len(),
                3 * 1024 * 1024
            );
        }

        async fn body_json(resp: axum::response::Response) -> serde_json::Value {
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice(&bytes).expect("a JSON body")
        }

        /// The read route must hand the editor the token a later save sends
        /// back, or the guard below has nothing to compare against.
        #[tokio::test]
        async fn read_returns_the_files_stamp() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/read",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp).await;
            assert_eq!(body["size"], 3);
            assert!(
                body["modified"].is_string(),
                "the read must carry an mtime: {body}"
            );
        }

        /// A save's success body is the client's new baseline. Without it the
        /// user's OWN save moves the changed-files signal and the editor would
        /// then believe somebody else edited the file.
        #[tokio::test]
        async fn a_successful_write_returns_the_fresh_stamp() {
            let (_tmp, _wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({ "path": "hello.txt", "content": "typed\n" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp).await;
            assert_eq!(body["size"], 6);
            assert!(body["modified"].is_string());
        }

        #[tokio::test]
        async fn a_write_with_a_matching_stamp_is_accepted() {
            let (_tmp, wt, app) = router_with_session().await;
            let read = app
                .clone()
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/read",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            let stamp = body_json(read).await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({
                        "path": "hello.txt",
                        "content": "mine\n",
                        "expected_modified": stamp["modified"],
                        "expected_size": stamp["size"],
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(
                std::fs::read_to_string(wt.join("hello.txt")).unwrap(),
                "mine\n"
            );
        }

        /// The data-loss fix, end to end: the agent edits the file while the
        /// editor holds an older buffer, and the save is refused with the facts
        /// the browser needs to offer a choice.
        #[tokio::test]
        async fn a_write_with_a_stale_stamp_is_409_with_the_current_stamp() {
            let (_tmp, wt, app) = router_with_session().await;
            let read = app
                .clone()
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/read",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            let stamp = body_json(read).await;
            std::fs::write(wt.join("hello.txt"), "the agent's work\n").unwrap();

            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({
                        "path": "hello.txt",
                        "content": "clobber\n",
                        "expected_modified": stamp["modified"],
                        "expected_size": stamp["size"],
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CONFLICT);
            let body = body_json(resp).await;
            assert_eq!(body["deleted"], false);
            assert_eq!(body["size"], "the agent's work\n".len());
            assert!(body["modified"].is_string());
            assert_eq!(
                std::fs::read_to_string(wt.join("hello.txt")).unwrap(),
                "the agent's work\n",
                "a refused save must leave the file exactly as it was"
            );
        }

        #[tokio::test]
        async fn a_write_onto_a_file_deleted_underneath_is_409_and_creates_nothing() {
            let (_tmp, wt, app) = router_with_session().await;
            let read = app
                .clone()
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/read",
                    serde_json::json!({ "path": "hello.txt" }),
                ))
                .await
                .unwrap();
            let stamp = body_json(read).await;
            std::fs::remove_file(wt.join("hello.txt")).unwrap();

            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({
                        "path": "hello.txt",
                        "content": "back\n",
                        "expected_modified": stamp["modified"],
                        "expected_size": stamp["size"],
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CONFLICT);
            let body = body_json(resp).await;
            assert_eq!(body["deleted"], true);
            assert!(body["modified"].is_null());
            assert!(!wt.join("hello.txt").exists());
        }

        /// A client that sends no token is every other writer and every older
        /// page: it must behave exactly as it did before the guard existed.
        #[tokio::test]
        async fn a_write_with_no_stamp_still_overwrites_unconditionally() {
            let (_tmp, wt, app) = router_with_session().await;
            std::fs::write(wt.join("hello.txt"), "changed by someone else\n").unwrap();
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({ "path": "hello.txt", "content": "overwritten\n" }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(
                std::fs::read_to_string(wt.join("hello.txt")).unwrap(),
                "overwritten\n"
            );
        }

        /// Half a token is no token: guarding on a size with no mtime (or the
        /// other way round) would compare against a value nobody supplied.
        #[tokio::test]
        async fn a_half_specified_stamp_is_ignored_rather_than_half_enforced() {
            let (_tmp, wt, app) = router_with_session().await;
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({
                        "path": "hello.txt",
                        "content": "written\n",
                        "expected_size": 999_999,
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(
                std::fs::read_to_string(wt.join("hello.txt")).unwrap(),
                "written\n"
            );
        }

        /// Past the cap the refusal must SAY it is a size problem and name the
        /// limit, rather than the framework's bare length message.
        #[tokio::test]
        async fn an_over_limit_save_is_refused_with_a_message_naming_the_limit() {
            let (_tmp, wt, app) = router_with_session().await;
            let content = "a".repeat(crate::file_routes::MAX_EDIT_WRITE_BYTES + 1);
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/s1/files/write",
                    serde_json::json!({ "path": "huge.txt", "content": content }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
            let body = String::from_utf8(
                to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(
                body.contains("too large") && body.contains("10 MB"),
                "the refusal must name the cause and the limit: {body}"
            );
            assert!(
                !wt.join("huge.txt").exists(),
                "a refused save must write nothing"
            );
        }

        /// The root extractor runs before the body is read, so a request that
        /// is wrong in BOTH ways (unknown id and an over-limit body) answers
        /// for the address first: 404, never the body diagnostic. This pins
        /// the extractor-first ordering the `write_file` doc comment names.
        #[tokio::test]
        async fn a_bad_id_beats_the_body_diagnostic() {
            let (_tmp, _wt, app) = router_with_session().await;
            let content = "a".repeat(crate::file_routes::MAX_EDIT_WRITE_BYTES + 1);
            let resp = app
                .oneshot(json_req(
                    "/api/v1/sessions/ghost/files/write",
                    serde_json::json!({ "path": "huge.txt", "content": content }),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
    }
}
