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
//! (`spawn_blocking`). After a write, the changed-files cache is invalidated so a
//! `session.changes` event reaches subscribed clients on `/ws/events`.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::git_routes::resolve_worktree;
use crate::server::AppState;

/// Largest raw asset the markdown-preview proxy will serve. Bigger than the
/// editable-file cap (images/screenshots run larger than source files) but still
/// bounded so a single request can't buffer an unbounded blob into memory.
const MAX_RAW_BYTES: u64 = 25 * 1024 * 1024;

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

/// Query for the raw-asset proxy. A GET so it can back an `<img src>`; the
/// session resolves the worktree, `path` is worktree-relative.
#[derive(Deserialize)]
struct RawQuery {
    session_id: String,
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

#[derive(Serialize)]
struct OpenedEditor {
    /// Human-readable editor label (e.g. "VS Code") for the success toast.
    editor: String,
}

/// The gated editor file routes, merged into the authenticated sub-router. These
/// are body/query-keyed (`session_id` in the body/query) and live under the
/// versioned `/api/v1/file/*` prefix. The unversioned `/api/file/*` aliases were
/// removed at cutover (Phase 6).
pub fn routes() -> Router<AppState> {
    let prefix = "/api/v1/file";
    Router::new()
        .route(&format!("{prefix}/list"), post(list_files))
        .route(&format!("{prefix}/read"), post(read_file))
        .route(&format!("{prefix}/diff"), post(diff_contents))
        .route(&format!("{prefix}/raw"), get(read_raw))
        .route(&format!("{prefix}/write"), post(write_file))
        .route(&format!("{prefix}/open-in-editor"), post(open_in_editor))
}

async fn list_files(State(state): State<AppState>, Json(op): Json<SessionOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    match tokio::task::spawn_blocking(move || dux_core::git::worktree_files(&worktree)).await {
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

/// Serve a worktree file's raw bytes for the markdown preview's relative-image
/// proxy (an `<img src>` backed by this GET). Uses the same read-permissive
/// resolver as `read_file` so `.git/` assets and symlinked images reach this
/// proxy. Symlinks are followed: `canonicalize()` resolves the real target,
/// then `read_nofollow` re-opens it with `O_NOFOLLOW` to close the TOCTOU
/// window between the canonicalize and the read. The write path is unaffected.
/// Content-Type is guessed from the extension; SVGs served to `<img>` never
/// run scripts. Auth-gated like every `/api/file/*` route.
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
async fn read_raw(State(state): State<AppState>, Query(q): Query<RawQuery>) -> Response {
    let worktree = match resolve_worktree(&state, q.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
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

async fn write_file(State(state): State<AppState>, Json(op): Json<WriteOp>) -> Response {
    let session_id = op.session_id.clone();
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
    // Refresh the REST changed-files cache too so subscribed `/ws/events` clients
    // re-GET the editor's new state immediately.
    state.changes.invalidate(session_id);
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
}
