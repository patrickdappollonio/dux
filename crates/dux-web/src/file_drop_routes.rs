//! The upload half of dropping a file onto a terminal or agent pane.
//!
//! # This route saves bytes. It never writes to a terminal.
//!
//! That split is the whole security design, so it is stated here rather than
//! left to be re-derived. Writing to a terminal is gated on being the connection
//! that currently HOLDS INPUT, and that gate lives on the websocket, enforced
//! server-side in `PtySizeOwners::may_write`. An upload handler that injected
//! the path itself would walk straight past it: it is not that connection, it is
//! a plain HTTP request. So this route saves the file and returns where it
//! landed, and the BROWSER pastes that path over its own already-gated socket,
//! exactly like every other write reaching a terminal.
//!
//! The route does check whether someone else currently holds input, and that
//! check is a **courtesy**: it turns "your file was saved and then silently not
//! pasted" into a clear refusal. It is not the protection, it must never be
//! described as one, and removing it would weaken nothing.
//!
//! **The courtesy check needs the right identifier, and there are two.** The
//! `X-Connection-Id` header the other REST routes carry names the EVENTS socket,
//! and `rest_common::scope_from_headers` deliberately refuses a PTY-class id in
//! it. Input ownership is tracked against a different number, minted when the
//! terminal socket connects and handed to the browser in its first frame. The
//! upload carries THAT one, in its own `conn` parameter. Reusing the header
//! would be checking a different thing entirely.
//!
//! # Where the file lands
//!
//! On an agent, the root of that agent's worktree, so git can see it and it can
//! be committed. On a terminal, wherever the terminal ACTUALLY is, discovered
//! live because a shell's directory changes the moment someone types `cd`. Both
//! resolve through `Engine::file_drop_destination`; the probing and the writing
//! run on a blocking pool, like every other filesystem call in this crate.

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

use crate::rest_common::id_within_bound;
use crate::server::AppState;

/// Query parameters of a file-drop upload. The bytes are the raw request body:
/// one file per request, which is what the paste rule wants anyway (a pasted
/// path is only treated as an attachment when it parses as exactly one token, so
/// several files means several pastes in sequence). Multipart would be a new
/// dependency for no gain.
#[derive(Deserialize)]
struct DropQuery {
    /// The PTY the pane is attached to: a terminal id, an agent's session id, or
    /// an extra tab's id. The engine resolves all three.
    pty: String,
    /// The dropped filename, as the browser reported it. Validated, never
    /// rewritten.
    filename: String,
    /// The TERMINAL SOCKET's connection id (not the events-socket id in
    /// `X-Connection-Id`). Optional: a browser that has not yet received its
    /// first frame simply skips the courtesy check.
    conn: Option<u64>,
}

/// Where a dropped file ended up.
#[derive(Serialize)]
struct SavedDropBody {
    /// The absolute path, which is what the browser pastes.
    path: String,
    /// The name it was saved under. Differs from `requested_name` on a
    /// collision.
    saved_name: String,
    /// The name as dropped, so the browser can report the pair rather than a
    /// count when they differ.
    requested_name: String,
    /// The absolute directory it landed in.
    folder: String,
    /// The directory shortened with `~`, for the toast. Shortened here because
    /// the SERVER is the machine whose home directory it is; the browser has no
    /// way to know.
    folder_label: String,
    /// True when a collision forced a different name.
    renamed: bool,
}

/// The file-drop route, with both of its limits attached.
///
/// Takes `state` rather than being state-generic like the other route modules,
/// because both layers need configured values: the body limit needs the
/// configured size cap, and the permit layer needs the shared semaphore.
pub fn routes(state: &AppState) -> Router<AppState> {
    let max_bytes = state.file_drop_max_bytes;
    Router::new().route(
        "/api/v1/file-drop",
        post(upload_dropped_file)
            // Set EXPLICITLY, because the framework's own default is 2 MB and
            // would reject an ordinary screenshot from a high-resolution
            // display. A `0` cap is handled inside the handler as "file drop is
            // off" rather than as a zero-byte limit, so the refusal can say so.
            .layer(DefaultBodyLimit::max(max_bytes.max(1)))
            // OUTERMOST, and that placement is the point: the request body is
            // buffered in full before the handler's first line runs, so a permit
            // taken inside the handler would be taken after the memory was
            // already spent. Taken here, it bounds how much upload exists at
            // once. A request beyond the limit WAITS rather than being refused.
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                hold_a_file_drop_permit,
            )),
    )
}

/// Hold one file-drop permit for the whole request, body included.
///
/// The binding must be NAMED (not `_`), or the permit would be dropped
/// immediately and the layer would bound nothing at all.
async fn hold_a_file_drop_permit(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let _permit = state.file_drop_semaphore.clone().acquire_owned().await;
    next.run(req).await
}

async fn upload_dropped_file(
    State(state): State<AppState>,
    Query(query): Query<DropQuery>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if state.file_drop_max_bytes == 0 {
        return (
            StatusCode::FORBIDDEN,
            "File drop is switched off on this server. Set [server] \
             file_drop_max_bytes in config.toml to a size in bytes and restart \
             to turn it back on."
                .to_string(),
        )
            .into_response();
    }
    if !id_within_bound(&query.pty) {
        return (StatusCode::NOT_FOUND, "unknown terminal or agent").into_response();
    }

    // The courtesy check. See the module docs: the websocket's own write check
    // is what actually enforces input authority. This only exists so a viewer
    // who cannot paste is told before a file is written rather than after.
    if let Some(conn) = query.conn
        && state.input_held_by_someone_else(&query.pty, conn)
    {
        return (
            StatusCode::CONFLICT,
            "Another device is driving this terminal, so the path could not be \
             pasted. Take over input and drop the file again."
                .to_string(),
        )
            .into_response();
    }

    let bytes = match body {
        Ok(b) => b,
        Err(_) => {
            // The limit is enforced by the body-limit layer, which rejects with
            // its own terse message; replace it with one that names the setting.
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "That file is over the {} byte limit for a dropped file. \
                     Nothing was written. Raise [server] file_drop_max_bytes in \
                     config.toml and restart to allow bigger files.",
                    state.file_drop_max_bytes
                ),
            )
                .into_response();
        }
    };

    let Some(destination) = state.engine.file_drop_destination(query.pty.clone()).await else {
        return (StatusCode::NOT_FOUND, "unknown terminal or agent").into_response();
    };

    let filename = query.filename.clone();
    // Everything from here is filesystem work: pinning the directory (a /proc
    // read, or an `lsof` process on macOS) and writing the file. Off the async
    // reactor, exactly like the editor's file routes.
    let saved = tokio::task::spawn_blocking(move || {
        // A destination that cannot be used is a refusal in its OWN words: a
        // path that could not be sent to the terminal, or a process dux is not
        // allowed to read. Flattening those into "could not write the file"
        // would describe the wrong problem.
        let dir = destination
            .open()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let saved = dux_core::file_drop::save_drop(&dir, &filename, &bytes, &stamp)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok::<_, std::io::Error>((saved, dir.path().to_path_buf()))
    })
    .await;

    match saved {
        Ok(Ok((saved, folder))) => {
            let body = SavedDropBody {
                path: saved.path.to_string_lossy().into_owned(),
                saved_name: saved.saved_name,
                requested_name: query.filename,
                folder: folder.to_string_lossy().into_owned(),
                folder_label: dux_core::home_path::shorten_home(&folder),
                renamed: saved.renamed,
            };
            axum::Json(body).into_response()
        }
        // A refusal (an unusable name, a symlink in the way, a destination that
        // cannot be written) is a client condition and names its reason, so the
        // browser can put that reason in the toast rather than a generic one.
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("file drop task failed: {e}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use dux_core::config::{DuxPaths, ProjectConfig};
    use dux_core::storage::SessionStore;
    use tower::ServiceExt;

    fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
        let n = chrono::Utc::now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            project_id: "p1".to_string(),
            project_path: None,
            provider: dux_core::model::ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "feat".to_string(),
            initial_branch: "feat".to_string(),
            worktree_path: worktree.to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: false,
            status: dux_core::model::SessionStatus::Detached,
            created_at: n,
            updated_at: n,
            last_focused_tab: None,
        }
    }

    /// A real router with agent "s1" pointed at a fresh worktree, and file drop
    /// configured with the given limits.
    async fn router_with_limits(
        max_bytes: usize,
        max_concurrency: u32,
    ) -> (tempfile::TempDir, std::path::PathBuf, axum::Router) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();

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
        let app = crate::server::build_app(
            handle,
            Router::<AppState>::new(),
            crate::server::RouterParams::plain_http()
                .with_file_drop_limits(max_bytes, max_concurrency),
        );
        (tmp, wt, app)
    }

    async fn router() -> (tempfile::TempDir, std::path::PathBuf, axum::Router) {
        router_with_limits(1024 * 1024, 4).await
    }

    fn drop_req(query: &str, body: Vec<u8>) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/file-drop?{query}"))
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    async fn body_text(resp: axum::response::Response) -> String {
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned()
    }

    #[tokio::test]
    async fn dropping_a_screenshot_on_an_agent_saves_it_at_the_worktree_root() {
        // The journey: someone drags a screenshot onto their agent's pane. The
        // file has to be somewhere git can see, and the route has to hand back
        // the path, because the path is what the browser pastes.
        let (_tmp, wt, app) = router().await;
        let resp = app
            .oneshot(drop_req(
                "pty=s1&filename=Screen%20Shot.png",
                b"\x89PNG-ish".to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();

        assert_eq!(body["saved_name"], "Screen Shot.png");
        assert_eq!(body["requested_name"], "Screen Shot.png");
        assert_eq!(body["renamed"], false);
        let path = std::path::PathBuf::from(body["path"].as_str().unwrap());
        assert_eq!(path.parent().unwrap(), wt, "must land at the worktree root");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\x89PNG-ish",
            "the file must be readable at the path the route reported"
        );
    }

    #[tokio::test]
    async fn dropping_the_same_name_twice_is_reported_with_the_new_name() {
        let (_tmp, _wt, app) = router().await;
        let first = app
            .clone()
            .oneshot(drop_req("pty=s1&filename=shot.png", b"one".to_vec()))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = app
            .oneshot(drop_req("pty=s1&filename=shot.png", b"two".to_vec()))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_text(second).await).unwrap();
        assert_eq!(body["renamed"], true);
        assert_eq!(body["requested_name"], "shot.png");
        assert_ne!(body["saved_name"], "shot.png");
        // The browser reports the PAIR, so both halves have to come back.
        assert_eq!(
            std::fs::read(body["path"].as_str().unwrap()).unwrap(),
            b"two"
        );
    }

    #[tokio::test]
    async fn a_traversing_filename_is_refused_and_writes_nothing() {
        let (tmp, _wt, app) = router().await;
        let resp = app
            .oneshot(drop_req(
                "pty=s1&filename=..%2F..%2Fescaped.png",
                b"x".to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let msg = body_text(resp).await;
        assert!(
            msg.contains("path separator"),
            "the refusal must name its reason, got: {msg}"
        );
        assert!(!tmp.path().join("escaped.png").exists());
    }

    #[tokio::test]
    async fn an_over_size_file_is_refused_with_a_message_naming_the_setting() {
        // A terse framework rejection ("length limit exceeded") tells the user
        // nothing they can act on, so the route replaces it with one that names
        // the limit and the setting that moves it.
        let (_tmp, wt, app) = router_with_limits(16, 4).await;
        let resp = app
            .oneshot(drop_req("pty=s1&filename=big.png", vec![0u8; 4096]))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let msg = body_text(resp).await;
        assert!(msg.contains("16 byte limit"), "got: {msg}");
        assert!(msg.contains("file_drop_max_bytes"), "got: {msg}");
        assert!(msg.contains("Nothing was written"), "got: {msg}");
        assert!(
            std::fs::read_dir(&wt).unwrap().next().is_none(),
            "a refused upload must leave the destination untouched"
        );
    }

    #[tokio::test]
    async fn a_zero_size_cap_switches_file_drop_off() {
        let (_tmp, wt, app) = router_with_limits(0, 4).await;
        let resp = app
            .oneshot(drop_req("pty=s1&filename=shot.png", b"x".to_vec()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let msg = body_text(resp).await;
        assert!(msg.contains("switched off"), "got: {msg}");
        assert!(std::fs::read_dir(&wt).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn an_unknown_pty_is_not_found() {
        let (_tmp, _wt, app) = router().await;
        let resp = app
            .oneshot(drop_req("pty=nobody&filename=shot.png", b"x".to_vec()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dropping_on_a_terminal_lands_where_the_shell_actually_is_after_a_cd() {
        // The journey the spawn directory would get wrong: open a terminal, type
        // `cd` somewhere else, then drop a file on it. The file has to land where
        // the user actually IS. Asserting only that it lands in the worktree
        // would pass with the stored spawn directory, which is the bug.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let wt = root.join("wt");
        let elsewhere = wt.join("deep").join("nested");
        std::fs::create_dir_all(&elsewhere).unwrap();

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
        let mut engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        // A plain, always-present shell, so the test does not depend on whatever
        // the machine's own terminal setting happens to be.
        engine.config.terminal.command = "/bin/sh".to_string();
        engine.config.terminal.args = vec![];
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::build_app(
            handle.clone(),
            Router::<AppState>::new(),
            crate::server::RouterParams::plain_http(),
        );

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/s1/terminals")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created: serde_json::Value = serde_json::from_str(&body_text(created).await).unwrap();
        let terminal_id = created["terminal_id"].as_str().unwrap().to_string();

        // Type the `cd`, then wait for the shell to have actually done it by
        // watching its own working directory change. Watching the state we care
        // about is deterministic in a way a fixed sleep is not.
        handle.write_pty(terminal_id.clone(), b"cd deep/nested\n".to_vec());
        let want = std::fs::canonicalize(&elsewhere).unwrap();
        let mut arrived = false;
        for _ in 0..200 {
            if let Some(dest) = handle.file_drop_destination(terminal_id.clone()).await
                && let Ok(dir) = dest.open()
                && std::fs::canonicalize(dir.path()).ok() == Some(want.clone())
            {
                arrived = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(arrived, "the shell never reported the new directory");

        let resp = app
            .oneshot(drop_req(
                &format!("pty={terminal_id}&filename=shot.png"),
                b"png".to_vec(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        let path = std::path::PathBuf::from(body["path"].as_str().unwrap());
        assert_eq!(
            std::fs::canonicalize(path.parent().unwrap()).unwrap(),
            want,
            "the file landed where the terminal was opened, not where it is now"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
    }

    #[tokio::test]
    async fn the_permit_is_taken_before_the_body_is_read() {
        // The bug this guards is subtle: a permit taken INSIDE the handler still
        // serializes the work and still looks correct, but by then the body has
        // already been buffered in full, so the memory the cap exists to bound
        // was already spent. Asserting that two requests merely serialise would
        // pass with the permit in the wrong place.
        //
        // So the proof is about the BODY: one upload holds the only permit with
        // a body that never finishes, and a second request's body must not be
        // consumed at all while it waits.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (_tmp, _wt, app) = router_with_limits(1024 * 1024, 1).await;

        // The first upload's body yields one chunk and then never completes, so
        // its handler stays parked inside the buffering step, holding the permit.
        let (blocker_tx, blocker_rx) = tokio::sync::oneshot::channel::<()>();
        let first_body = axum::body::Body::from_stream(futures_util::stream::once(async move {
            let _ = blocker_rx.await;
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"never finishes"))
        }));
        let first = tokio::spawn(
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/file-drop?pty=s1&filename=slow.png")
                    .body(first_body)
                    .unwrap(),
            ),
        );

        // The second upload's body flags the instant anything polls it.
        let polled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&polled);
        let second_body = axum::body::Body::from_stream(futures_util::stream::once(async move {
            flag.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"second"))
        }));
        let second = tokio::spawn(
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/file-drop?pty=s1&filename=second.png")
                    .body(second_body)
                    .unwrap(),
            ),
        );

        // Give both tasks every chance to run. Yielding rather than sleeping
        // keeps this deterministic and instant.
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert!(
            !polled.load(Ordering::SeqCst),
            "the second upload's body was consumed while another upload held the \
             only permit, so the permit is being taken after the body is buffered \
             and bounds no memory at all"
        );

        // Release the first, and the second must then go through: the permit
        // gates, it does not reject.
        let _ = blocker_tx.send(());
        let first = first.await.unwrap().unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = second.await.unwrap().unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        assert!(
            polled.load(Ordering::SeqCst),
            "the second upload should have run once the permit freed"
        );
    }
}
