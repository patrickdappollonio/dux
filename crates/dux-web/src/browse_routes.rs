//! Two stateless "utility" reads the add-project / new-agent dialogs need
//! (Phase 6 of the REST-first migration). These used to ride the retired `/ws`
//! request/reply pairs (`browse_dir` → `dir_entries`, `generate_agent_name` →
//! `agent_name`); they are now plain unauthenticated GETs.
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
//! the old handler's precedent.
//!
//! # Access model: read this before extending `?path=`
//!
//! `GET /api/v1/browse` has NO authentication, NO root restriction and NO
//! sandbox: any client that can reach the server can list ANY directory the
//! server process can read, anywhere on the host, by passing an absolute
//! `?path=`. That is deliberate, not an oversight. dux is single-tenant
//! trusted-access (CLAUDE.md): the picker exists so the operator can point the
//! server at any repo on their own machine, and every client is assumed to be
//! that operator.
//!
//! The app-wide guards are NOT authentication and do not narrow what is
//! browsable:
//!
//! - A **Host-header allowlist** stops a malicious web page from rebinding DNS to
//!   this server's address and reaching it through the victim's browser.
//! - A **same-origin check** stops another site driving these routes from a
//!   visitor's browser, but it applies to MUTATIONS only, so it covers
//!   `POST /api/v1/browse/mkdir` and NOT the `GET` listings above. A client that
//!   sends no `Origin` header at all (curl, a script) skips it by design.
//!
//! So the only real boundary is who can reach the listening address: keep it on
//! loopback, a trusted tailnet, or behind an authenticating proxy. Do not add a
//! feature here that assumes mutually-distrusting web users without first
//! designing the per-user isolation model CLAUDE.md calls for.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::server::AppState;

/// Upper bound on the `?path=` query value before any filesystem touch. Generous
/// (well above `PATH_MAX` on supported platforms) so it rejects only an abusive
/// string, never a legitimate directory path.
const MAX_PATH_LEN: usize = 4096;

/// The utility read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/browse", get(browse))
        .route("/api/v1/browse/mkdir", post(mkdir))
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
    is_parent: bool,
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
                is_parent: e.is_parent,
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

#[derive(Deserialize)]
struct MkdirBody {
    parent: String,
    name: String,
}

/// The mkdir reply: the created directory's full path.
#[derive(Serialize)]
struct MkdirReply {
    path: String,
}

/// `POST /api/v1/browse/mkdir`: create ONE new directory inside an existing
/// parent, for the add-project picker's "New folder" affordance (built for the
/// terminal-less phone-over-Tailscale case).
///
/// Safety argument, from the threat model rather than borrowed helpers: the
/// server is single-tenant/trusted by design, and every client can already browse
/// the entire filesystem via this module's GET (no containment, by design), so
/// this endpoint's job is shape discipline and non-destructiveness, not
/// containment. `name` is validated to a single path component (no `/`, no
/// NUL, not `.`/`..`), so there is NO path arithmetic to defeat (the dde64db
/// lesson: that escape lived in containment math over a multi-segment path);
/// one `join` of an absolute parent with a vetted component. A symlinked
/// parent resolves exactly as if the user had browsed there, which the GET
/// already permits. `create_dir` never overwrites, follows, or removes; the
/// worst case is a new empty directory where the operator's own account can
/// write. As a POST it sits inside `rest_mutation_origin_check` and the host
/// allowlist layered in `server.rs`, guarding cross-site requests.
async fn mkdir(State(_state): State<AppState>, Json(body): Json<MkdirBody>) -> Response {
    let parent = body.parent;
    // Mirror the inspect endpoint's path checks, check for check.
    if parent.is_empty() {
        return (StatusCode::BAD_REQUEST, "parent is required").into_response();
    }
    if !std::path::Path::new(&parent).is_absolute() {
        // A relative parent would silently resolve against the server cwd.
        return (StatusCode::BAD_REQUEST, "path must be absolute").into_response();
    }
    if parent.chars().count() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "path is too long").into_response();
    }
    // `name` must be exactly one path component.
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "folder name is required").into_response();
    }
    if name.len() > 255 {
        return (StatusCode::BAD_REQUEST, "folder name is too long").into_response();
    }
    if name.contains('/') || name.contains('\0') {
        return (
            StatusCode::BAD_REQUEST,
            "folder name can't contain path separators",
        )
            .into_response();
    }
    if name == "." || name == ".." {
        return (StatusCode::BAD_REQUEST, "that folder name is reserved").into_response();
    }
    if name.starts_with('.') {
        // The picker hides dotfolders, so a dot-named folder would be created
        // invisible and unreachable.
        return (
            StatusCode::BAD_REQUEST,
            "folder names starting with a dot are hidden in the picker; pick another name",
        )
            .into_response();
    }

    // Filesystem write off the reactor (the browse precedent). `create_dir`,
    // not `create_dir_all`: the picker only navigates existing directories, so
    // a missing parent is an error, not a request.
    let result = tokio::task::spawn_blocking(move || {
        let target = std::path::Path::new(&parent).join(&name);
        std::fs::create_dir(&target).map(|()| target.to_string_lossy().to_string())
    })
    .await;

    match result {
        Ok(Ok(path)) => Json(MkdirReply { path }).into_response(),
        Ok(Err(err)) => match err.kind() {
            // Measured: AlreadyExists covers an existing dir, file, dangling
            // symlink, and symlink-to-dir, with no follow-through.
            std::io::ErrorKind::AlreadyExists => (
                StatusCode::CONFLICT,
                "a file or folder with that name already exists",
            )
                .into_response(),
            std::io::ErrorKind::NotFound => (
                StatusCode::BAD_REQUEST,
                "the parent folder no longer exists",
            )
                .into_response(),
            _ => (
                StatusCode::BAD_REQUEST,
                format!("couldn't create the folder: {err}"),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("mkdir failed: {e}"),
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

        // The parent ("../") row carries is_parent == true; the real child
        // directories carry is_parent == false. This is the typed flag the web
        // picker branches on rather than matching the "../" label string.
        let entries = value["entries"].as_array().unwrap();
        let parent = entries
            .iter()
            .find(|e| e["label"] == "../")
            .expect("a parent row is synthesized");
        assert_eq!(parent["is_parent"], true);
        let alpha = entries
            .iter()
            .find(|e| e["label"] == "alpha/")
            .expect("alpha is listed");
        assert_eq!(alpha["is_parent"], false);
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

    async fn post_mkdir(app: axum::Router, parent: &str, name: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/browse/mkdir")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "parent": parent, "name": name }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn mkdir_creates_a_folder_and_returns_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().to_string_lossy().to_string();
        let (_tmp, app) = router_no_auth();
        let resp = post_mkdir(app, &parent, "projects").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let created = value["path"].as_str().unwrap();
        assert_eq!(created, dir.path().join("projects").to_string_lossy());
        assert!(dir.path().join("projects").is_dir());
    }

    #[tokio::test]
    async fn mkdir_rejects_malformed_names_and_parents() {
        // Catches traversal-by-name, cwd-relative writes, and invisible
        // dot-folders (the picker hides them).
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().to_string_lossy().to_string();
        for bad_name in ["a/b", ".", "..", ".hidden", ""] {
            let (_tmp, app) = router_no_auth();
            let resp = post_mkdir(app, &parent, bad_name).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "name {bad_name:?} must be rejected"
            );
        }
        let (_tmp, app) = router_no_auth();
        let resp = post_mkdir(app, "relative/parent", "ok").await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "a relative parent must be rejected"
        );
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "no rejected request may have created anything"
        );
    }

    #[tokio::test]
    async fn mkdir_conflicts_on_existing_and_400s_on_missing_parent() {
        // Catches clobbering: an existing entry (dir or file) is a 409, never
        // an overwrite; a vanished parent is a 400, not a create_dir_all.
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().to_string_lossy().to_string();
        std::fs::create_dir(dir.path().join("taken")).unwrap();
        std::fs::write(dir.path().join("file"), b"x").unwrap();

        let (_tmp, app) = router_no_auth();
        let resp = post_mkdir(app, &parent, "taken").await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let (_tmp, app) = router_no_auth();
        let resp = post_mkdir(app, &parent, "file").await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), b"x");

        let missing = dir.path().join("gone").to_string_lossy().to_string();
        let (_tmp, app) = router_no_auth();
        let resp = post_mkdir(app, &missing, "child").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn mkdir_with_a_mismatched_origin_is_403() {
        // Catches cross-site directory creation: the new POST must sit inside
        // the layered `rest_mutation_origin_check`.
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().to_string_lossy().to_string();
        let tmp = tempfile::tempdir().unwrap();
        let handle = crate::test_support::test_engine_handle(tmp.path());
        let app = crate::server::build_app(
            handle,
            axum::Router::new(),
            crate::server::RouterParams::plain_http(),
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/browse/mkdir")
                    .header("Host", "localhost")
                    .header("Origin", "http://evil.example.com")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({ "parent": parent, "name": "pwned" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            !dir.path().join("pwned").exists(),
            "the cross-origin request must not have created anything"
        );
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
