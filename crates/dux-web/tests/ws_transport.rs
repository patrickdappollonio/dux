//! End-to-end WebSocket transport tests (the "debug client").
//!
//! Post-cutover (Phase 6) the legacy `/ws` socket, the broadcast ViewModel, and
//! the `ServerMessage`/`ClientMessage` command protocol are gone. Reads and actions
//! are REST (`/api/v1/...`); change/status signals ride `/ws/events`; terminal byte
//! streams ride the nested per-PTY sockets.

use std::net::SocketAddr;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn sample_session(
    id: &str,
    project_id: &str,
    branch: &str,
    worktree: &str,
) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: project_id.to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            initial_branch: branch.to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

async fn boot() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
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
        // Seed the owning project so session-delete (which looks up the project)
        // can take the inline path.
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: root.to_string_lossy().into_owned(),
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s1",
                "p1",
                "feat",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    // The sample session's provider is "claude", which isn't on PATH in CI. Override
    // it with `cat`, a runnable program that echoes stdin so the real launch flow
    // spawns a streaming PTY (the marker the streaming tests send is echoed back).
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    // Companion terminals run `config.terminal.command`; override it with `cat` so a
    // created terminal echoes input back the same way the provider override does.
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, tmp)
}

/// Like `boot()`, but the session's worktree is a REAL git repo: `f.txt` is
/// committed with three lines, then its working copy is modified WITHOUT a
/// commit so a working-tree-vs-HEAD diff exists.
async fn boot_with_repo() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    // Build the git repo at the worktree root.
    let run = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "line1\nline2\nline3\n").expect("write file");
    run(&["add", "f.txt"]);
    run(&["commit", "-q", "-m", "init"]);
    // Modify the working copy without committing so HEAD != working tree.
    std::fs::write(root.join("f.txt"), "line1\nCHANGED\nline3\n").expect("overwrite");

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
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s1",
                "p1",
                "feat",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, tmp)
}

/// HTTP `POST /api/v1/sessions/:id/files/diff` returns both raw sides of a changed
/// file and rejects a path that escapes the worktree — the HTTP-layer coverage that
/// replaced the deleted WS `get_diff` test (route wiring, session resolution, the
/// boundary, and JSON shape).
#[tokio::test]
async fn http_file_diff_returns_sides_and_rejects_traversal() {
    let (addr, _tmp) = boot_with_repo().await;
    let client = reqwest::Client::new();

    // The seeded repo has f.txt = "line2" at HEAD, "CHANGED" in the working copy.
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/diff"))
        .json(&serde_json::json!({ "path": "f.txt" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["binary"], false, "{body}");
    assert!(
        body["original"].as_str().unwrap().contains("line2"),
        "original (HEAD) side missing committed content: {body}"
    );
    assert!(
        body["modified"].as_str().unwrap().contains("CHANGED"),
        "modified (working) side missing the edit: {body}"
    );
    // Pin which side each string lands on — catch an original/modified swap.
    assert!(
        !body["original"].as_str().unwrap().contains("CHANGED"),
        "original (HEAD) side must not carry the working edit: {body}"
    );
    assert!(
        !body["modified"].as_str().unwrap().contains("line2"),
        "modified (working) side must not carry the replaced HEAD line: {body}"
    );

    // A path escaping the worktree is rejected at the boundary → 400.
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/diff"))
        .json(&serde_json::json!({ "path": "../escape" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "path traversal must be rejected");
}

/// HTTP `GET /api/v1/sessions/:id/files/raw` (the markdown-preview image proxy)
/// serves a worktree file's bytes with a guessed content type, and rejects a path
/// that escapes the worktree.
#[tokio::test]
async fn http_file_raw_serves_bytes_and_rejects_traversal() {
    let (addr, _tmp) = boot_with_repo().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{addr}/api/v1/sessions/s1/files/raw?path=f.txt"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // No image extension → generic content type; body is the working copy on disk.
    assert_eq!(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/octet-stream")
    );
    // Hardening headers must be present so a direct navigation to a worktree .svg
    // can't run script in dux's origin (same-origin stored XSS).
    let header = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(header(reqwest::header::CONTENT_SECURITY_POLICY), "sandbox");
    assert_eq!(header(reqwest::header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert!(
        header(reqwest::header::CONTENT_DISPOSITION).contains("attachment"),
        "raw responses must be Content-Disposition: attachment"
    );
    assert_eq!(resp.text().await.unwrap(), "line1\nCHANGED\nline3\n");

    let resp = client
        .get(format!(
            "http://{addr}/api/v1/sessions/s1/files/raw?path=../escape"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "path traversal must be rejected");
}

/// Like `boot()`, but project `p1`'s path is a REAL git repo (init + commit) so
/// `git worktree add` succeeds, and no session is seeded (the test creates one).
/// `pull_before_creating_agent_by_default` is disabled because the test repo has
/// no remote, so a pre-create pull would fail.
async fn boot_for_create_agent() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let run = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "line1\n").expect("write file");
    run(&["add", "f.txt"]);
    run(&["commit", "-q", "-m", "init"]);

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
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    // The spawned agent provider defaults to "claude"; override with `cat` so the
    // launch flow spawns a runnable PTY in CI.
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    // The test repo has no remote, so a pre-create pull would fail; disable it.
    engine.config.defaults.pull_before_creating_agent_by_default = false;
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, tmp)
}

/// Poll `GET /api/v1/workspace` until `pred` holds or the deadline lapses. The
/// document is also pushed over `/ws/events`, but polling the REST read is the
/// simpler synchronization here: these tests care that the server's state
/// settled, not how a client learns it (the push has its own tests in
/// `tests/workspace_push.rs`). `true` if `pred` ever held.
async fn wait_for_workspace<F>(addr: SocketAddr, pred: F) -> bool
where
    F: Fn(&serde_json::Value) -> bool,
{
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .get(format!("http://{addr}/api/v1/workspace"))
            .send()
            .await
            && let Ok(v) = resp.json::<serde_json::Value>().await
            && pred(&v)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// Whether the spine's ONE flat `terminals` collection carries `terminal_id`
/// tagged as owned by a session. Terminals arrive flat rather than nested
/// inside the session or project that owns them, each carrying a tagged owner,
/// so this and its sibling below read the tag instead of the nesting.
fn spine_has_terminal(spine: &serde_json::Value, terminal_id: &str) -> bool {
    spine_terminal_has_owner_kind(spine, terminal_id, "session")
}

/// Whether the spine's flat `terminals` collection carries `terminal_id` tagged
/// as owned by a project.
fn spine_has_project_terminal(spine: &serde_json::Value, terminal_id: &str) -> bool {
    spine_terminal_has_owner_kind(spine, terminal_id, "project")
}

fn spine_terminal_has_owner_kind(spine: &serde_json::Value, terminal_id: &str, kind: &str) -> bool {
    spine["terminals"]
        .as_array()
        .map(|terminals| {
            terminals.iter().any(|t| {
                t["id"].as_str() == Some(terminal_id) && t["owner"]["kind"].as_str() == Some(kind)
            })
        })
        .unwrap_or(false)
}

// ── REST action endpoints + the events/PTY sockets ───────────────────────────

/// Concrete type for a connected test WebSocket.
type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect a `/ws/events` client and read its server-assigned connection id from
/// the first `connected` frame (used as the `X-Connection-Id` header so a REST
/// action's status toasts scope back to it).
async fn connect_events(addr: SocketAddr) -> (ClientWs, String) {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await
            && let Ok(t) = m.into_text()
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
            && v["event"] == "connected"
        {
            return (ws, v["id"].as_str().unwrap().to_string());
        }
    }
    panic!("never received the connected frame");
}

/// Whether a `status` event whose message contains `needle` arrives within the
/// window (`/ws/events` status shape `{"event":"status",...,"message":...}`).
async fn saw_status(ws: &mut ClientWs, needle: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"event\":\"status\"")
            && t.contains(needle)
        {
            return true;
        }
    }
    false
}

/// Whether a `status` event of the given tone arrives within the window,
/// regardless of its wording. Used by the replay test, which cares that an
/// outcome of that tone reached (or did not reach) a connection, not about the
/// exact sentence a git failure produced.
async fn saw_status_tone(ws: &mut ClientWs, tone: &str, timeout: Duration) -> Option<String> {
    let needle = format!("\"tone\":\"{tone}\"");
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"event\":\"status\"")
            && t.contains(&needle)
        {
            return Some(t.to_string());
        }
    }
    None
}

/// Boot with `p1` carrying a startup command that BLOCKS on a FIFO the caller
/// controls, so a keyed `Busy` stays provably up for exactly as long as the test
/// wants. This is the "hold the window open with a real dependency" technique
/// CLAUDE.md mandates: no sleep, no polling for a race, just a process parked on
/// a read that nothing but the test can satisfy.
///
/// Returns the FIFO path; writing anything to it lets the command exit and the
/// keyed final arrive.
async fn boot_with_gated_startup_command() -> (SocketAddr, std::path::PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let gate = root.join("startup-gate");
    let made = std::process::Command::new("mkfifo")
        .arg(&gate)
        .status()
        .expect("spawn mkfifo");
    assert!(made.success(), "mkfifo {} failed", gate.display());

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
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                // Blocks until the test opens the FIFO for writing.
                startup_command: Some(format!("cat {}", gate.display())),
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s1",
                "p1",
                "feat",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    // Pin the shell so the gate command is interpreted identically everywhere,
    // rather than depending on whatever login shell the host defaults to.
    engine.config.startup_command_terminal.command = "sh".to_string();
    engine.config.startup_command_terminal.args = vec!["-c".to_string()];
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, gate, tmp)
}

/// Release the gated startup command by opening its FIFO for writing. Opening a
/// FIFO for write blocks until a reader is present, so this is only called once
/// the busy status has proved the command is running.
async fn release_gate(gate: std::path::PathBuf) {
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::write(&gate, b"go\n");
    })
    .await
    .expect("gate writer");
}

/// The connect-time snapshot replay, in both directions, over a real socket.
///
/// This glue had NO coverage: replacing the replay loop in `ws_events` with a
/// discarded call broke nothing in the whole suite, even though it is the only
/// thing that tells a client about work it did not personally start. The
/// retention split makes it MORE load-bearing, since an in-flight `Busy` is now
/// the main thing the snapshot exists to carry.
///
/// Both statuses are raised WITHOUT an `X-Connection-Id`, so their scope is
/// `All` and per-connection filtering cannot make this pass for the wrong
/// reason.
#[tokio::test]
async fn a_new_connection_is_told_about_work_in_flight_and_about_the_outcome_it_missed() {
    let (addr, gate, _tmp) = boot_with_gated_startup_command().await;

    // The browser that is already open when the operation starts.
    let (mut ws_a, _id_a) = connect_events(addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{addr}/api/v1/sessions/s1/rerun-startup-command"
        ))
        .send()
        .await
        .expect("POST rerun-startup-command");
    assert_eq!(resp.status().as_u16(), 200, "the command is accepted");

    // The operation is now parked on the FIFO, so the busy is provably still in
    // flight for the rest of this block.
    assert!(
        saw_status_tone(&mut ws_a, "busy", Duration::from_secs(10))
            .await
            .is_some(),
        "the attached connection must see the busy live"
    );

    // DIRECTION ONE: a browser that opens mid-operation is handed the spinner
    // out of the connect-time snapshot, having seen no live broadcast at all.
    let (mut ws_b, _id_b) = connect_events(addr).await;
    let replayed_busy = saw_status_tone(&mut ws_b, "busy", Duration::from_secs(5)).await;
    assert!(
        replayed_busy.is_some(),
        "a connection opened mid-operation must be replayed the in-flight busy"
    );

    // Let the startup command finish; its keyed final replaces the busy.
    release_gate(gate).await;
    assert!(
        saw_status_tone(&mut ws_a, "info", Duration::from_secs(15))
            .await
            .is_some(),
        "the attached connection must see the final live"
    );

    // DIRECTION TWO: the dropped-socket journey. A connection made after the
    // operation ended is still inside `FINAL_REPLAY_WINDOW`, so it is handed the
    // outcome rather than an empty snapshot. Without this a tab whose socket
    // blipped across the operation would sit on a spinner forever and never
    // learn what happened.
    let (mut ws_c, _id_c) = connect_events(addr).await;
    let replayed_final = saw_status_tone(&mut ws_c, "info", Duration::from_secs(5)).await;
    assert!(
        replayed_final.is_some(),
        "a connection made just after the operation must be replayed its outcome"
    );
    // …and the spinner must NOT come back with it: the final retired it.
    assert!(
        saw_status_tone(&mut ws_c, "busy", Duration::from_millis(800))
            .await
            .is_none(),
        "a finished operation must not be replayed as still running"
    );
}

/// A final stays continuously replayable from the moment it is broadcast: two
/// fresh connections, one straight after the final and one a second and a half
/// later, must BOTH be handed it.
///
/// This exists because a container run reported a snapshot that was empty ~1s
/// after a final and populated ~2.5s after it, which is non-monotonic and so
/// cannot be the replay window. Nothing in this tree reproduces it, and this
/// test is the guard that says so in the shape the report described: a gap of
/// any length in that first stretch fails it. It drives the real actor through
/// the real create path rather than the controller, because the reported
/// journey was an ASYNC final delivered by a worker followup, not a synchronous
/// command result.
#[tokio::test]
async fn a_final_is_replayable_continuously_from_the_moment_it_is_broadcast() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let (mut ws_a, _id_a) = connect_events(addr).await;

    // No `X-Connection-Id`, so the create's statuses are `All`-scoped and every
    // connection is entitled to them.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"replay"}))
        .send()
        .await
        .expect("POST create");
    assert!(
        resp.status().is_success(),
        "create must be accepted, got {}",
        resp.status()
    );

    // Wait for the create's INFO final on the live socket, so the rest of the
    // test is timed from the broadcast rather than from the POST.
    assert!(
        saw_status_tone(&mut ws_a, "info", Duration::from_secs(20))
            .await
            .is_some(),
        "the attached connection must see the create final"
    );

    // Straight after the final.
    let (mut ws_b, _id_b) = connect_events(addr).await;
    assert!(
        saw_status_tone(&mut ws_b, "info", Duration::from_secs(5))
            .await
            .is_some(),
        "a connection made immediately after the final must be replayed it"
    );

    // And again after the interval the report said was empty. Presence at both
    // ends brackets the reported gap: the snapshot cannot have dropped the final
    // and picked it back up in between, because a drop is permanent until a new
    // `set` on the key, and no second create runs here.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (mut ws_c, _id_c) = connect_events(addr).await;
    assert!(
        saw_status_tone(&mut ws_c, "info", Duration::from_secs(5))
            .await
            .is_some(),
        "a connection made 1.5s after the final must still be replayed it"
    );

    // Nothing may have dismissed it either: a server-driven `status_cleared` for
    // a finished operation is what the retention split removed.
    let cleared = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let Some(Ok(m)) = ws_a.next().await
                && let Ok(t) = m.into_text()
                && t.contains("status_cleared")
            {
                return t.to_string();
            }
        }
    })
    .await;
    assert!(
        cleared.is_err(),
        "no status_cleared may be sent for a finished create, got {cleared:?}"
    );
}

/// A failed delete is reported to the connection that was watching, and the
/// error arrives STICKY end to end, from the engine resolver to the wire.
///
/// (What happens to that error THIRTY SECONDS later, when it leaves the replay
/// window, is pinned in `dux_core::statusline` and in the emitter tests, which
/// can drive the clock. It is deliberately not tested here: a thirty-second
/// sleep in the suite would cost more than the coverage is worth.)
#[tokio::test]
async fn a_half_done_delete_reports_a_sticky_error_to_the_watching_connection() {
    let (addr, _tmp) = boot().await;
    let (mut ws_a, _id_a) = connect_events(addr).await;

    // Deleting s1 with its worktree runs an async removal whose git call fails
    // (the seeded worktree path is a plain directory, not a linked worktree).
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/s1?delete_worktree=true"
        ))
        .send()
        .await
        .expect("DELETE session");
    assert_eq!(
        resp.status().as_u16(),
        204,
        "the delete is accepted; the failure arrives as a status"
    );

    let seen = saw_status_tone(&mut ws_a, "error", Duration::from_secs(10)).await;
    let seen = seen.expect("the attached connection must receive the broadcast error");

    // A failed worktree removal leaves an orphaned directory on disk that only
    // the user can clear, so this toast must wait for them rather than time out.
    assert!(
        seen.contains("\"sticky\":true"),
        "a half-done delete must be marked sticky on the wire, got {seen}"
    );
}

/// THE WHOLE JOURNEY for a standalone agent, over the real HTTP surface: create
/// it in a plain folder, see it on the wire as a folder workspace, watch every
/// branch-identity route refuse it, and delete it without the folder being
/// touched.
///
/// End to end deliberately. Each half is unit-tested, but the thing a user
/// actually does crosses the REST layer, the engine actor, the create worker and
/// the spine projection, and the failure this guards against (an empty branch
/// field arriving at a screen, or a delete removing the folder) only shows up
/// once those are joined.
#[tokio::test]
async fn a_standalone_agent_runs_in_a_plain_folder_and_survives_delete_untouched() {
    let (addr, _tmp) = boot_for_create_agent().await;
    // A PLAIN folder: no repository anywhere near it. That is the ordinary case
    // and the one the add-project validator would have rejected.
    let folder = tempfile::tempdir().expect("folder");
    std::fs::write(folder.path().join("notes.txt"), "mine\n").expect("seed a file");
    let folder_path = folder.path().to_string_lossy().to_string();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .json(&serde_json::json!({
            "kind": "standalone",
            "folder": folder_path,
            "name": "notes",
        }))
        .send()
        .await
        .expect("POST create");
    assert_eq!(
        resp.status().as_u16(),
        201,
        "a plain folder is the ordinary case for a standalone agent"
    );
    let body: serde_json::Value = resp.json().await.expect("created session json");
    let id = body["id"].as_str().expect("created session id").to_string();

    // THE WIRE SHAPE: a folder workspace, and NO git fields to be mistaken for
    // a branch. This is the guarantee the tagged union exists for.
    assert_eq!(body["workspace"]["kind"].as_str(), Some("folder"));
    assert_eq!(
        body["workspace"]["folder_path"].as_str(),
        Some(folder_path.as_str())
    );
    for absent in [
        "branch_name",
        "initial_branch",
        "source_branch",
        "project_id",
    ] {
        assert!(
            body["workspace"][absent].is_null(),
            "{absent} must not exist on a folder workspace, got {body}"
        );
    }
    assert_eq!(body["title"].as_str(), Some("notes"));

    // It joins the ordinary flat agent list, and no sidebar group claims it.
    assert!(
        wait_for_workspace(addr, |v| {
            let listed = v["sessions"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["id"].as_str() == Some(id.as_str())));
            let grouped = v["sidebar"]["groups"].as_array().is_some_and(|groups| {
                groups.iter().any(|g| {
                    g["session_ids"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|i| i.as_str() == Some(id.as_str())))
                })
            });
            listed && !grouped
        })
        .await,
        "a standalone agent belongs to no project group, and must still be listed"
    );

    // EVERY branch-identity route refuses it on the server. Hiding the buttons
    // is not an answer when each of these is reachable from a command line.
    for (method, path) in [
        ("POST", format!("/api/v1/sessions/{id}/git/push")),
        ("POST", format!("/api/v1/sessions/{id}/git/pull")),
    ] {
        let resp = client
            .request(method.parse().unwrap(), format!("http://{addr}{path}"))
            .send()
            .await
            .expect("git route");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        assert!(
            (400..500).contains(&status),
            "{path} must refuse a standalone agent, got {status}: {text}"
        );
        assert!(
            text.contains("standalone agent"),
            "{path} must say what this agent is, got {text}"
        );
    }

    // A worktree-removing delete is refused OUT LOUD rather than quietly
    // downgraded: silent success theater about a destructive request is how a
    // user comes to believe dux cleaned something up.
    let resp = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/{id}?delete_worktree=true"
        ))
        .send()
        .await
        .expect("DELETE with removal");
    assert!(
        (400..500).contains(&resp.status().as_u16()),
        "asking dux to remove the user's folder must be refused"
    );
    assert!(
        folder.path().exists(),
        "and the folder must still be there afterwards"
    );

    // The ordinary delete removes dux's record and nothing else.
    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/{id}"))
        .send()
        .await
        .expect("DELETE session");
    assert_eq!(resp.status().as_u16(), 204);
    assert!(
        wait_for_workspace(addr, |v| {
            v["sessions"]
                .as_array()
                .is_some_and(|s| s.iter().all(|s| s["id"].as_str() != Some(id.as_str())))
        })
        .await,
        "the agent record must be gone"
    );
    // THE PIN: the folder and its contents are exactly as they were.
    assert!(folder.path().exists(), "the folder must survive the delete");
    assert_eq!(
        std::fs::read_to_string(folder.path().join("notes.txt")).expect("the file survives"),
        "mine\n",
        "and so must everything in it"
    );
}

/// A second standalone agent in a folder that already has one is refused, with a
/// message that points at the shape dux is built for. Coding CLIs resume their
/// conversation history per directory, so the second agent would silently pick
/// up the first one's conversation.
#[tokio::test]
async fn a_second_standalone_agent_in_one_folder_is_refused_over_http() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let folder = tempfile::tempdir().expect("folder");
    let folder_path = folder.path().to_string_lossy().to_string();
    let client = reqwest::Client::new();

    let first = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .json(&serde_json::json!({"kind":"standalone","folder":folder_path}))
        .send()
        .await
        .expect("POST create");
    assert_eq!(first.status().as_u16(), 201);
    let id = first.json::<serde_json::Value>().await.expect("json")["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert!(
        wait_for_workspace(addr, |v| {
            v["sessions"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["id"].as_str() == Some(id.as_str())))
        })
        .await,
        "the first agent has to be registered before the refusal can see it"
    );

    let second = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .json(&serde_json::json!({"kind":"standalone","folder":folder_path}))
        .send()
        .await
        .expect("POST create");
    let status = second.status().as_u16();
    let text = second.text().await.unwrap_or_default();
    assert!(
        (400..500).contains(&status),
        "a second agent in the same folder must be refused, got {status}: {text}"
    );
    assert!(
        text.contains("conversation"),
        "the refusal must say why it matters, got {text}"
    );
    assert!(
        text.contains("as a project"),
        "and point at the multi-agent shape, got {text}"
    );
}

/// `POST /api/v1/sessions` (kind=new) creates a session and returns 201 + the new
/// session object, and its status toasts are scoped to the originating connection
/// (`X-Connection-Id`): the originating `/ws/events` sees the create status, a
/// different connection does not.
#[tokio::test]
async fn rest_create_session_returns_201_and_scopes_status() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let (mut ws_a, id_a) = connect_events(addr).await;
    let (mut ws_b, _id_b) = connect_events(addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .header("x-connection-id", &id_a)
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"scoped"}))
        .send()
        .await
        .expect("POST create");
    assert_eq!(resp.status().as_u16(), 201, "create must return 201");
    assert_eq!(
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.starts_with("/api/v1/sessions/")),
        Some(true),
        "201 carries a Location header",
    );
    let body: serde_json::Value = resp.json().await.expect("created session json");
    let new_id = body["id"].as_str().expect("created session id");
    assert!(!new_id.is_empty());
    assert_eq!(body["workspace"]["kind"].as_str(), Some("managed"));
    assert_eq!(body["workspace"]["project_id"].as_str(), Some("p1"));

    // The originating connection sees the create's status…
    assert!(
        saw_status(&mut ws_a, "Creating a new agent", Duration::from_secs(8)).await,
        "the originating connection must see the scoped create status"
    );
    // …but a different connection must NOT (the toast is scoped to id_a).
    assert!(
        !saw_status(
            &mut ws_b,
            "Creating a new agent",
            Duration::from_millis(800)
        )
        .await,
        "a different connection must not receive the scoped create status"
    );
}

/// A retried `POST /api/v1/sessions` carrying the same `Idempotency-Key` returns
/// the SAME session (200 replay) and does NOT create a second one.
#[tokio::test]
async fn rest_create_session_idempotency_replays_same_session() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .header("idempotency-key", "abc-123")
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"idem"}))
        .send()
        .await
        .expect("first create");
    assert_eq!(first.status().as_u16(), 201);
    let first_body: serde_json::Value = first.json().await.unwrap();
    let id1 = first_body["id"].as_str().unwrap().to_string();

    let second = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .header("idempotency-key", "abc-123")
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"idem"}))
        .send()
        .await
        .expect("second create");
    assert_eq!(
        second.status().as_u16(),
        200,
        "an idempotent replay returns 200, not a second 201"
    );
    let second_body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(
        second_body["id"].as_str(),
        Some(id1.as_str()),
        "the replay must return the same session id"
    );

    // Exactly one session exists under p1 — the replay created nothing new.
    let sessions: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let p1_count = sessions
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|s| s["workspace"]["project_id"].as_str() == Some("p1"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(p1_count, 1, "idempotent replay must not create a duplicate");
}

/// Like `boot()`, but seeds TWO sessions (`s1`, `s2`) under `p1`, so the nested
/// terminal PTY socket's session-ownership enforcement can be exercised (a `:tid`
/// created under `s1` must be rejected on the `s2` path).
async fn boot_two_sessions() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
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
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s1",
                "p1",
                "feat",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s2",
                "p1",
                "feat2",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, tmp)
}

/// Create a companion terminal on `session_id` over REST and return its id.
async fn create_terminal_via_rest(addr: SocketAddr, session_id: &str) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{addr}/api/v1/sessions/{session_id}/terminals"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201, "terminal create should be 201");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["terminal_id"]
        .as_str()
        .expect("terminal_id in create response")
        .to_string()
}

/// The nested agent PTY socket (`/ws/sessions/:id/pty`) launches/resumes the
/// provider, replays the repaint, and streams raw PTY bytes both ways: a Binary
/// stdin frame echoes back through the `cat` provider override.
#[tokio::test]
async fn nested_agent_pty_socket_streams_bytes() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");

    // Claim sizing+input ownership first: a non-owner's stdin is dropped (the
    // per-PTY active-owner model), and a fresh socket owns nothing until it sends
    // a size, exactly as the real client does on a foreground attach.
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(
        b"dux-nested-agent-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();

    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-nested-agent-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-nested-agent-marker"),
        "nested agent PTY socket did not stream; got {} bytes",
        acc.len()
    );
}

#[tokio::test]
async fn first_binary_input_claims_an_unowned_pty_and_announces_its_owner() {
    let (addr, _tmp) = boot().await;
    let (mut events, _) = connect_events(addr).await;
    events
        .send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();

    let (mut pty, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");
    let hello = next_event_frame(&mut pty, "connected", Duration::from_secs(8))
        .await
        .expect("the pty handshake");
    let connection_id = hello["id"]
        .as_str()
        .expect("the handshake names this connection");

    pty.send(Message::Binary(b"dux-first-input-marker\n".to_vec().into()))
        .await
        .unwrap();
    let echoed = accumulate_until(&mut pty, "dux-first-input-marker", Duration::from_secs(8)).await;
    assert!(
        String::from_utf8_lossy(&echoed).contains("dux-first-input-marker"),
        "first input on an unowned pty must be forwarded"
    );

    let owner = next_event_frame(&mut events, "pty.owner", Duration::from_secs(8))
        .await
        .expect("first input must announce the new owner");
    assert_eq!(owner["id"].as_str(), Some("s1"));
    assert_eq!(owner["owner"].as_str(), Some(connection_id));
}

/// Read Binary frames off `ws` until the accumulated bytes contain `needle` or
/// the deadline passes; returns everything read. Text frames (the `connected`
/// handshake) and pings are skipped, exactly as the browser client ignores them
/// for the purposes of terminal content.
async fn accumulate_until(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    needle: &str,
    within: Duration,
) -> Vec<u8> {
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains(needle) {
                break;
            }
        }
    }
    acc
}

/// The three halves of the server contract that the web client's take-over
/// leans on, pinned over a real socket.
///
/// Take-over on the web is now a RECONNECT carrying intent: reopen the socket,
/// then let the first resize frame of the new connection claim, flagged. That
/// works because (1) a SECOND connection to a PTY another connection currently
/// owns is still replayed the scrollback on open, which is the only thing that
/// repaints a black viewport (the child's SIGWINCH redraw is a no-op when the
/// size it is handed already matches); (2) a PLAIN resize from that second
/// connection claims NOTHING, which is what stops an ordinary attach from
/// stealing the prompt; and (3) a resize carrying `takeover` DOES claim, so its
/// stdin is forwarded from then on. None of it is obvious from the route code,
/// and a change to any of it would break take-over on the web while every
/// client-side test stayed green.
#[tokio::test]
async fn a_second_pty_connection_is_replayed_scrollback_and_claims_only_by_taking_over() {
    let (addr, _tmp) = boot().await;
    let url = format!("ws://{addr}/ws/sessions/s1/pty");

    // Connection A attaches, claims by sizing, and puts something in the
    // scrollback so the replay has content worth asserting on.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect first pty socket");
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws_a.send(Message::Binary(b"dux-owner-a-marker\n".to_vec().into()))
        .await
        .unwrap();
    let echoed = accumulate_until(&mut ws_a, "dux-owner-a-marker", Duration::from_secs(8)).await;
    assert!(
        String::from_utf8_lossy(&echoed).contains("dux-owner-a-marker"),
        "the owning connection's stdin never echoed; got {} bytes",
        echoed.len()
    );

    // Connection B attaches while A still owns input. It must be replayed the
    // scrollback unconditionally: this is the frame a reopened socket relies on
    // to repaint after a take-over.
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect second pty socket");
    let replay = accumulate_until(&mut ws_b, "dux-owner-a-marker", Duration::from_secs(8)).await;
    assert!(
        String::from_utf8_lossy(&replay).contains("dux-owner-a-marker"),
        "a second connection was not replayed the scrollback while another owns the PTY; got {} bytes",
        replay.len()
    );

    // B sends a PLAIN resize, the way any foregrounded attach or window change
    // does. It must claim NOTHING: B's stdin is still dropped server-side, so the
    // ABSENCE of an echo is the proof that attaching did not steal. (A's echo of
    // the same bytes would not appear on B's socket either, so this asserts on a
    // marker only B could have produced.)
    ws_b.send(Message::Text(r#"{"rows":30,"cols":100}"#.into()))
        .await
        .unwrap();
    ws_b.send(Message::Binary(
        b"dux-attach-steal-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();
    let stolen =
        accumulate_until(&mut ws_b, "dux-attach-steal-marker", Duration::from_secs(2)).await;
    assert!(
        !String::from_utf8_lossy(&stolen).contains("dux-attach-steal-marker"),
        "a PLAIN resize from a second connection stole input ownership; attaching \
         must never steal"
    );

    // B now takes over EXPLICITLY. Ownership transfers and its stdin is forwarded
    // from then on; the echo IS the proof that ownership flipped.
    ws_b.send(Message::Text(
        r#"{"rows":30,"cols":100,"takeover":true}"#.into(),
    ))
    .await
    .unwrap();
    ws_b.send(Message::Binary(b"dux-taker-b-marker\n".to_vec().into()))
        .await
        .unwrap();
    let after = accumulate_until(&mut ws_b, "dux-taker-b-marker", Duration::from_secs(8)).await;
    assert!(
        String::from_utf8_lossy(&after).contains("dux-taker-b-marker"),
        "the second connection's flagged take-over did not claim input ownership; \
         got {} bytes",
        after.len()
    );
}

/// THE DELAYED GHOST SUCCESSION, over two real sockets.
///
/// The one press-less re-claim the design keeps is a returning owner succeeding
/// its own dead connection, and it rides an ordinary flagged resize. On a mobile
/// network that frame can arrive seconds late, by which time another device may
/// legitimately own the pty. `expected_owner` is what stops it becoming a steal:
/// the server transfers only when the named predecessor still holds the pty.
///
/// Asserted the same way the take-over test above asserts ownership: by whether
/// the sender's stdin echoes back. A refused claim forwards nothing.
#[tokio::test]
async fn a_flagged_resize_naming_a_stale_expected_owner_does_not_steal_the_pty() {
    let (addr, _tmp) = boot().await;
    let url = format!("ws://{addr}/ws/sessions/s1/pty");

    // A attaches and claims by sizing; its `connected` handshake tells it the
    // connection id it is about to become the ghost of.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect the first pty socket");
    let hello_a = next_event_frame(&mut ws_a, "connected", Duration::from_secs(8))
        .await
        .expect("the first handshake");
    let ghost_id = hello_a["id"]
        .as_str()
        .expect("the handshake names this connection")
        .to_string();
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    let echoed =
        accumulate_until(&mut ws_a, "dux-ghost-owner-marker", Duration::from_secs(1)).await;
    assert!(
        !String::from_utf8_lossy(&echoed).contains("dux-ghost-owner-marker"),
        "nothing has typed that marker yet"
    );

    // A's socket dies, and B claims the freed pty with a plain resize.
    drop(ws_a);
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect the second pty socket");
    let _ = next_event_frame(&mut ws_b, "connected", Duration::from_secs(8))
        .await
        .expect("the second handshake");
    // Retry the plain claim until the server has reaped A's socket: until then
    // the pty is still owned by the ghost and B's plain resize is refused, which
    // is the correct behaviour and merely a matter of timing here.
    let mut b_owns = false;
    for attempt in 0..40 {
        ws_b.send(Message::Text(r#"{"rows":30,"cols":100}"#.into()))
            .await
            .unwrap();
        ws_b.send(Message::Binary(
            format!("dux-owner-b-marker-{attempt}\n")
                .into_bytes()
                .into(),
        ))
        .await
        .unwrap();
        let needle = format!("dux-owner-b-marker-{attempt}");
        let seen = accumulate_until(&mut ws_b, &needle, Duration::from_millis(500)).await;
        if String::from_utf8_lossy(&seen).contains(&needle) {
            b_owns = true;
            break;
        }
    }
    assert!(
        b_owns,
        "the second connection must claim the pty once the first one is reaped"
    );

    // A comes back on a fresh socket and its ghost succession finally lands,
    // naming the dead connection it believes still owns the pty. B owns it now,
    // so the claim must be refused whole and A's stdin must still be dropped.
    let (mut ws_c, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect the returning owner's socket");
    let _ = next_event_frame(&mut ws_c, "connected", Duration::from_secs(8))
        .await
        .expect("the returning owner's handshake");
    ws_c.send(Message::Text(
        format!(r#"{{"rows":24,"cols":80,"takeover":true,"expected_owner":"{ghost_id}"}}"#).into(),
    ))
    .await
    .unwrap();
    ws_c.send(Message::Binary(b"dux-stale-ghost-marker\n".to_vec().into()))
        .await
        .unwrap();
    let stolen =
        accumulate_until(&mut ws_c, "dux-stale-ghost-marker", Duration::from_secs(2)).await;
    assert!(
        !String::from_utf8_lossy(&stolen).contains("dux-stale-ghost-marker"),
        "a flagged resize naming a predecessor that no longer owns the pty stole \
         input ownership; only a pressed take-over may do that"
    );

    // And the PRESSED take-over, which names no expectation, still wins.
    ws_c.send(Message::Text(
        r#"{"rows":24,"cols":80,"takeover":true}"#.into(),
    ))
    .await
    .unwrap();
    ws_c.send(Message::Binary(b"dux-pressed-marker\n".to_vec().into()))
        .await
        .unwrap();
    let after = accumulate_until(&mut ws_c, "dux-pressed-marker", Duration::from_secs(8)).await;
    assert!(
        String::from_utf8_lossy(&after).contains("dux-pressed-marker"),
        "a pressed take-over carries no expectation and must still claim; got {} bytes",
        after.len()
    );
}

/// The periodic frame the browser sends is ANSWERED, which is what lets it
/// measure a round trip and force a plain reconnect when a socket has silently
/// half-died. The answer echoes the client's own number, so an answer to a stale
/// beat cannot be counted as an answer to the current one.
#[tokio::test]
async fn a_beat_frame_is_answered_with_the_same_number() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect the pty socket");
    let _ = next_event_frame(&mut ws, "connected", Duration::from_secs(8))
        .await
        .expect("the handshake");

    // A WATCHER's beat: `viewed` false, and it must still be answered.
    ws.send(Message::Text(r#"{"beat":41,"viewed":false}"#.into()))
        .await
        .unwrap();
    let answer = next_event_frame(&mut ws, "beat", Duration::from_secs(8))
        .await
        .expect("a watcher's beat must be answered too");
    assert_eq!(answer["n"].as_u64(), Some(41));

    // And the owner's, carrying the viewed half.
    ws.send(Message::Text(r#"{"beat":42,"viewed":true}"#.into()))
        .await
        .unwrap();
    let answer = next_event_frame(&mut ws, "beat", Duration::from_secs(8))
        .await
        .expect("the owner's beat must be answered");
    assert_eq!(answer["n"].as_u64(), Some(42));
}

#[tokio::test]
async fn an_unknown_text_frame_is_ignored_without_poisoning_the_socket() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect the pty socket");
    let _ = next_event_frame(&mut ws, "connected", Duration::from_secs(8))
        .await
        .expect("the handshake");

    ws.send(Message::Text(
        r#"{"unknown_control":"must-not-reach-stdin"}"#.into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(
        b"dux-after-unknown-control\n".to_vec().into(),
    ))
    .await
    .unwrap();

    let output =
        accumulate_until(&mut ws, "dux-after-unknown-control", Duration::from_secs(8)).await;
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("dux-after-unknown-control"));
    assert!(!output.contains("must-not-reach-stdin"));
}

/// Read the next Text frame that carries the given `event` value, or `None` if
/// none arrives inside `within`. Binary frames (the scrollback replay and live
/// PTY output) and pings are skipped, as is any Text frame of another kind.
///
/// Both halves of this helper are load-bearing for the tests below: finding a
/// frame proves a broadcast happened, and finding NONE inside a window is how a
/// refused resize is proven to have announced nothing.
async fn next_event_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    event: &str,
    within: Duration,
) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
            && v["event"].as_str() == Some(event)
        {
            return Some(v);
        }
    }
    None
}

/// Every socket attached to a PTY is told when that PTY's grid actually moves,
/// and a refused resize tells nobody anything.
///
/// ONE PTY HAS ONE AUTHORITATIVE GRID, the owner's, and every other attached
/// browser renders the same byte stream into its own differently sized xterm.
/// Before this the wire never told a non-owner the PTY's size at all, so a
/// viewer could not know that what it was rendering was wrapped and clamped
/// garbage. The `connected` handshake carries the grid at attach and this
/// event carries every change after it.
///
/// The refusal half matters just as much: a non-owner's plain resize applies
/// nothing, so announcing it would tell every viewer the grid had moved to a
/// size the child never took, and each of them would then heal itself towards a
/// lie.
#[tokio::test]
async fn an_applied_resize_tells_every_attached_socket_the_ptys_new_grid() {
    let (addr, _tmp) = boot().await;
    let url = format!("ws://{addr}/ws/sessions/s1/pty");

    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect first pty socket");
    let hello_a = next_event_frame(&mut ws_a, "connected", Duration::from_secs(8))
        .await
        .expect("the connected handshake");
    assert!(
        hello_a["rows"].as_u64().is_some() && hello_a["cols"].as_u64().is_some(),
        "the handshake must carry the pty's grid; without it an arriving viewer \
         cannot tell whether it agrees with the child's geometry: {hello_a}"
    );

    // A claims the unowned pty by sizing it. B attaches as a watcher.
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect second pty socket");
    let _ = next_event_frame(&mut ws_b, "connected", Duration::from_secs(8))
        .await
        .expect("the second socket's connected handshake");

    // The owner resizes. The WATCHER must hear about it: this is the frame that
    // lets it heal its grid to the pty's true size.
    ws_a.send(Message::Text(r#"{"rows":40,"cols":120}"#.into()))
        .await
        .unwrap();
    let size_b = next_event_frame(&mut ws_b, "size", Duration::from_secs(8))
        .await
        .expect("a granted resize must be announced to the other attached socket");
    assert_eq!(size_b["rows"].as_u64(), Some(40));
    assert_eq!(size_b["cols"].as_u64(), Some(120));

    // The announcement goes to EVERY socket attached to the pty, the resizer
    // included (its grid already agrees, so it costs it nothing, and one rule
    // for all attached sockets is simpler than an exception). Drain A up to the
    // change it just made, so the refusal assertion below is reading an empty
    // queue rather than A's own backlog.
    loop {
        let own = next_event_frame(&mut ws_a, "size", Duration::from_secs(8))
            .await
            .expect("the resizing socket hears its own applied grid too");
        if own["rows"].as_u64() == Some(40) {
            break;
        }
    }

    // A watcher's PLAIN resize is refused whole, so it must announce nothing.
    ws_b.send(Message::Text(r#"{"rows":10,"cols":30}"#.into()))
        .await
        .unwrap();
    let refused = next_event_frame(&mut ws_a, "size", Duration::from_secs(2)).await;
    assert!(
        refused.is_none(),
        "a REFUSED resize changed nothing and must announce nothing; the owner \
         was told the grid moved to {refused:?}, which the child never took"
    );

    // A fresh arrival's handshake reports the size the owner actually applied,
    // read off the live PTY rather than remembered from a frame. The resize
    // crosses the engine actor's queue, so this polls rather than assuming the
    // child has been resized by the time the assertion runs.
    let mut seen = serde_json::Value::Null;
    for _ in 0..10 {
        let (mut ws_c, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect a fresh pty socket");
        seen = next_event_frame(&mut ws_c, "connected", Duration::from_secs(8))
            .await
            .expect("the fresh arrival's connected handshake");
        let _ = ws_c.close(None).await;
        if seen["rows"].as_u64() == Some(40) && seen["cols"].as_u64() == Some(120) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        (seen["rows"].as_u64(), seen["cols"].as_u64()),
        (Some(40), Some(120)),
        "the handshake grid must be the LIVE pty's, or a viewer starts out \
         believing it agrees with a geometry the child left behind"
    );
}

#[tokio::test]
async fn a_grid_change_is_forwarded_only_to_sockets_for_that_pty() {
    let (addr, _tmp) = boot().await;
    let terminal_id = create_terminal_via_rest(addr, "s1").await;
    let (mut first, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
            .await
            .expect("connect first pty socket");
    let (mut second, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/s1/terminals/{terminal_id}/pty"
    ))
    .await
    .expect("connect companion terminal pty socket");
    let _ = next_event_frame(&mut first, "connected", Duration::from_secs(8))
        .await
        .expect("first handshake");
    let _ = next_event_frame(&mut second, "connected", Duration::from_secs(8))
        .await
        .expect("second handshake");

    first
        .send(Message::Text(r#"{"rows":37,"cols":111}"#.into()))
        .await
        .unwrap();
    let size = next_event_frame(&mut first, "size", Duration::from_secs(8))
        .await
        .expect("the resized pty must receive its grid change");
    assert_eq!(size["rows"].as_u64(), Some(37));
    assert_eq!(size["cols"].as_u64(), Some(111));
    assert!(
        next_event_frame(&mut second, "size", Duration::from_secs(2))
            .await
            .is_none(),
        "a socket for another pty must not receive the grid change"
    );
}

/// The owner leaving is BROADCAST, so a watcher's card stops naming a browser
/// tab that closed.
///
/// Ownership no longer follows focus, so nothing else corrects a departed owner:
/// before this, the next device to attach or alt-tab silently stole the pty and
/// that theft was what cleared the stale card. With the theft gone, an
/// owner-cleared `pty.owner` is the only signal that the driver has left, and
/// without it "Active on another device" is a permanent lie. It carries an epoch,
/// strictly newer than the claim it retires, or the client's epoch ordering would
/// discard it as a stale duplicate.
#[tokio::test]
async fn an_owner_disconnecting_broadcasts_an_owner_cleared_pty_owner() {
    let (addr, _tmp) = boot().await;

    // A watcher on the events socket, subscribed to the coarse `sessions` topic
    // that carries `pty.owner`.
    let (mut events, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .expect("connect the events socket");
    events
        .send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();

    /// Drain events until a `pty.owner` for `s1` arrives, or give up.
    async fn next_pty_owner(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(Message::Text(t)))) =
                tokio::time::timeout(Duration::from_millis(300), ws.next()).await
                && t.contains(r#""event":"pty.owner""#)
            {
                return Some(serde_json::from_str(&t).expect("pty.owner is JSON"));
            }
        }
        None
    }

    // The driver attaches and claims the unowned pty with an ordinary resize.
    let url = format!("ws://{addr}/ws/sessions/s1/pty");
    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect the driving pty socket");
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();

    let claimed = next_pty_owner(&mut events)
        .await
        .expect("the claim must broadcast a pty.owner");
    assert_eq!(claimed["id"].as_str(), Some("s1"));
    let claim_owner = claimed["owner"]
        .as_str()
        .expect("a claim names its owner")
        .to_string();
    let claim_epoch = claimed["epoch"].as_u64().expect("a claim carries an epoch");

    // The driver goes away. Its ownership is released, and that release is
    // announced.
    ws_a.close(None).await.unwrap();

    let cleared = next_pty_owner(&mut events)
        .await
        .expect("the owner's disconnect must broadcast an owner-cleared pty.owner");
    assert_eq!(cleared["id"].as_str(), Some("s1"));
    assert!(
        cleared.get("owner").is_none() || cleared["owner"].is_null(),
        "an owner-cleared event names nobody, so every client reads it as 'not \
         me'; got {cleared}"
    );
    assert!(
        cleared["epoch"].as_u64().expect("an epoch") > claim_epoch,
        "the cleared event must be strictly newer than the claim it retires \
         (owner was {claim_owner})"
    );
}

/// The `connected` handshake tells an arriving client whether it is joining as
/// the driver or as a watcher.
///
/// This is the frame amendment 1 of the take-over plan exists for. With a plain
/// claim now refused SILENTLY, nothing else would ever correct a foregrounded
/// arrival's optimistic "I must be the owner" guess: it would render typing
/// surfaces over a pty whose every keystroke the server drops, and no take-over
/// card would ever appear. The first connection must see `owner: null` (nobody
/// driving), and a second one arriving after that first connection has claimed
/// must see the first connection's id.
#[tokio::test]
async fn the_connected_handshake_names_the_ptys_current_owner() {
    let (addr, _tmp) = boot().await;
    let url = format!("ws://{addr}/ws/sessions/s1/pty");

    // A watcher on the events socket, to capture the epoch the claim's own
    // `pty.owner` broadcast carries. The handshake's `owner_epoch` must be read
    // from the SAME counter in the same lock acquisition as the owner, so the
    // two values must agree.
    let (mut events, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .expect("connect the events socket");
    events
        .send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();

    /// The first Text frame on a PTY socket is the `connected` handshake; the
    /// Binary frames around it are the scrollback replay and live output.
    async fn first_text(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(Message::Text(t)))) =
                tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            {
                return serde_json::from_str(&t).expect("the connected frame is JSON");
            }
        }
        panic!("no connected frame arrived");
    }

    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect first pty socket");
    let hello_a = first_text(&mut ws_a).await;
    assert_eq!(
        hello_a["owner"],
        serde_json::Value::Null,
        "the first arrival joins an unowned pty, and the key must be PRESENT and \
         null rather than absent, or it cannot be told from an older server"
    );
    let epoch_a = hello_a["owner_epoch"].as_u64().expect(
        "the handshake must stamp its owner snapshot with the ownership epoch, \
         or a client cannot order it against the pty.owner broadcasts on the \
         other socket",
    );
    let conn_a = hello_a["id"].as_str().expect("a connection id").to_string();

    // A claims the unowned pty with an ordinary resize (this is the one case a
    // plain resize still claims).
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws_a.send(Message::Binary(b"dux-hello-owner-marker\n".to_vec().into()))
        .await
        .unwrap();
    let _ = accumulate_until(&mut ws_a, "dux-hello-owner-marker", Duration::from_secs(8)).await;

    // The claim's own `pty.owner` broadcast, and the epoch it was stamped with.
    let claim_epoch = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let mut epoch = None;
        while tokio::time::Instant::now() < deadline && epoch.is_none() {
            if let Ok(Some(Ok(Message::Text(t)))) =
                tokio::time::timeout(Duration::from_millis(300), events.next()).await
                && t.contains(r#""event":"pty.owner""#)
            {
                let ev: serde_json::Value = serde_json::from_str(&t).expect("pty.owner is JSON");
                epoch = ev["epoch"].as_u64();
            }
        }
        epoch.expect("the claim must broadcast a pty.owner carrying an epoch")
    };

    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect second pty socket");
    let hello_b = first_text(&mut ws_b).await;
    assert_eq!(
        hello_b["owner"].as_str(),
        Some(conn_a.as_str()),
        "the second arrival must be told which connection is driving, or it \
         wedges itself as a phantom owner"
    );
    assert_eq!(
        hello_b["owner_epoch"].as_u64(),
        Some(claim_epoch),
        "the handshake's owner_epoch must equal the epoch of the claim it \
         reports, because both are read from the one counter under the owners \
         lock; a drift here means the snapshot was not taken atomically"
    );
    assert!(
        claim_epoch > epoch_a,
        "the claim must be strictly newer than the pre-claim handshake snapshot"
    );
}

/// The `connected` handshake also names the owner's DEVICE: the `User-Agent`
/// the owning connection presented at its upgrade, recorded at claim time and
/// read under the same owners-lock acquisition as `owner` and `owner_epoch`.
///
/// This is the frame's answer to the mere-attach case: a watcher that simply
/// opens the pane hears no `pty.owner` broadcast (attaching never steals and a
/// refused claim emits nothing), so without this key its take-over card can
/// only say "Active on another device" instead of naming the driving device.
/// An unowned pty omits the key, so the pre-claim shape stays byte-identical
/// to what an older client already parses.
#[tokio::test]
async fn the_connected_handshake_names_the_owners_device() {
    let (addr, _tmp) = boot().await;
    let url = format!("ws://{addr}/ws/sessions/s1/pty");

    /// The first Text frame on a PTY socket is the `connected` handshake; the
    /// Binary frames around it are the scrollback replay and live output.
    async fn first_text(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(Message::Text(t)))) =
                tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            {
                return serde_json::from_str(&t).expect("the connected frame is JSON");
            }
        }
        panic!("no connected frame arrived");
    }

    // The driver connects with a `User-Agent`, as every real browser does.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.clone().into_client_request().expect("a client request");
    request.headers_mut().insert(
        axum::http::header::USER_AGENT,
        "Test Driver UA".parse().expect("a header value"),
    );
    let (mut ws_a, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect the driving pty socket");
    let hello_a = first_text(&mut ws_a).await;
    assert!(
        hello_a.get("owner_device").is_none(),
        "an unowned pty has no device to name, and the key is omitted; got {hello_a}"
    );

    // A claims the unowned pty, then proves the claim was processed by echoing
    // a marker through the pty (frames on one socket are handled in order).
    ws_a.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws_a.send(Message::Binary(b"dux-device-marker\n".to_vec().into()))
        .await
        .unwrap();
    let _ = accumulate_until(&mut ws_a, "dux-device-marker", Duration::from_secs(8)).await;

    // A second, plain arrival must be told the driver's device on its handshake,
    // with no pty.owner broadcast involved anywhere on its path.
    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect second pty socket");
    let hello_b = first_text(&mut ws_b).await;
    assert_eq!(
        hello_b["owner"].as_str(),
        hello_a["id"].as_str(),
        "the second arrival joins the pty the first connection drives"
    );
    assert_eq!(
        hello_b["owner_device"].as_str(),
        Some("Test Driver UA"),
        "the handshake must name the owner's device, or a watcher that merely \
         attached can only title its card 'Active on another device'"
    );
}

/// One byte past the 16 MiB message cap dux is expected to configure, written
/// as a LITERAL and deliberately NOT derived from
/// `dux_web::server::MAX_WS_MESSAGE_SIZE`.
///
/// Deriving it is what made the cap tests self-referential: a payload sized
/// `MAX_WS_MESSAGE_SIZE + 1` grows with the constant, so raising the constant
/// raised the payload too and the socket refused it at the new value just as
/// happily. Both tests stayed green through a measured 64 MiB mutation. A fixed
/// size cannot do that: raise the constant and this payload becomes an ordinary
/// under-cap message that the server accepts and `cat` echoes back.
///
/// The pair is completed by `the_configured_message_cap_is_16_mib`, which pins
/// the constant itself, so a drift between the literal here and the value in
/// `server.rs` fails loudly rather than silently making these tests vacuous.
const OVER_MESSAGE_CAP_BYTES: usize = 16 * 1024 * 1024 + 1;

/// The configured cap is the 16 MiB that `OVER_MESSAGE_CAP_BYTES` is one byte
/// past. Keep the two in step: if the cap is ever deliberately changed, this
/// assertion is the one that must be updated, and updating it forces a look at
/// the literal the refusal tests send.
#[test]
fn the_configured_message_cap_is_16_mib() {
    assert_eq!(
        dux_web::server::MAX_WS_MESSAGE_SIZE,
        16 * 1024 * 1024,
        "MAX_WS_MESSAGE_SIZE moved; the fixed over-cap payload the refusal tests \
         send is no longer over the cap"
    );
    assert_eq!(
        OVER_MESSAGE_CAP_BYTES,
        dux_web::server::MAX_WS_MESSAGE_SIZE + 1
    );
}

/// A single frame past the message cap is refused by the socket, not delivered.
///
/// Be precise about what this pins and what it does NOT. The cap is 16 MiB,
/// which is exactly tungstenite's DEFAULT `max_frame_size`, so an unfragmented
/// frame this large is refused by the frame cap dux never configures, and this
/// test keeps passing with dux's own `.max_message_size(..)` calls deleted or
/// raised. It was measured: raising every socket to 64 MiB leaves this test
/// green and an identical trace, and that stays true now the payload is a fixed
/// size, because the frame cap is unmoved at 16 MiB either way. So it proves the
/// end-to-end refusal is real (an over-cap frame never reaches the PTY and the
/// socket ends rather than quietly ignoring it), and nothing about dux's
/// constant.
///
/// `a_fragmented_message_past_the_message_cap_is_refused` is the one that pins
/// the configured number, and it can only do so because its payload is the fixed
/// `OVER_MESSAGE_CAP_BYTES`; keep them together.
///
/// The provider override in `boot` is `cat`, which echoes whatever reaches the
/// PTY, so the assertion is direct: the marker inside the oversized frame must
/// NEVER come back.
#[tokio::test]
async fn an_oversized_frame_is_refused_and_never_reaches_the_pty() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");

    // Claim ownership first, so a dropped write could only be the size cap and
    // not the non-owner rule.
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();

    // One byte over the cap, STARTING with the marker so an echo shows up in the
    // first bytes `cat` sends back rather than after 16 MiB have travelled: on a
    // slow machine a trailing marker might not arrive inside the deadline, and
    // an accepted payload would look like a refused one.
    let marker = b"dux-oversize-marker\n";
    let mut payload = marker.to_vec();
    payload.resize(OVER_MESSAGE_CAP_BYTES, b'x');
    assert_eq!(payload.len(), OVER_MESSAGE_CAP_BYTES);
    // The send itself may fail if the server has already torn the socket down,
    // which is just as good an answer as a later close.
    let _ = ws.send(Message::Binary(payload.into())).await;

    let mut acc = Vec::new();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => acc.extend_from_slice(&b),
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                ended = true;
                break;
            }
            _ => {}
        }
        // As in the fragmented sibling: the marker leads the payload, so an
        // accepted frame is visible immediately and there is nothing to gain by
        // reading the rest of it.
        if String::from_utf8_lossy(&acc).contains("dux-oversize-marker") {
            break;
        }
    }

    assert!(
        !String::from_utf8_lossy(&acc).contains("dux-oversize-marker"),
        "an over-cap frame was written to the PTY and echoed back"
    );
    assert!(
        ended,
        "the socket stayed open after an over-cap frame; the cap must refuse it, \
         not ignore it"
    );
}

/// A FRAGMENTED message past `MAX_WS_MESSAGE_SIZE` is refused, which is what
/// actually pins dux's configured cap.
///
/// The WebSocket protocol has two independent limits and dux only sets one of
/// them. `max_frame_size` bounds a single frame and dux leaves it at
/// tungstenite's 16 MiB default; `max_message_size` bounds the REASSEMBLED
/// message across a continuation chain, defaults to 64 MiB, and is the one dux
/// lowers to `MAX_WS_MESSAGE_SIZE`. Because the two numbers coincide at 16 MiB,
/// no single frame can tell them apart, which is how the sibling test above
/// stayed green through a 64 MiB mutation.
///
/// So this sends the payload as continuation frames of 4 MiB each. Every frame
/// is comfortably under the frame cap, and only the message cap can refuse the
/// total. The payload is the FIXED `OVER_MESSAGE_CAP_BYTES` and not
/// `MAX_WS_MESSAGE_SIZE + 1`: derived from the constant it grew with any
/// mutation and the test pinned nothing, which was measured, twice, at 64 MiB.
///
/// Mutation proof, both halves measured: raise `MAX_WS_MESSAGE_SIZE` and the now
/// under-cap message is accepted and echoes back off `cat`, failing on the
/// marker (and `the_configured_message_cap_is_16_mib` fails alongside it);
/// delete the `.max_message_size(..)` call from the agent PTY socket and the
/// 64 MiB default lets the same message through, failing the same way.
#[tokio::test]
async fn a_fragmented_message_past_the_message_cap_is_refused() {
    use tokio_tungstenite::tungstenite::protocol::frame::Frame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};

    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");

    // Claim ownership first, so a dropped write could only be the size cap and
    // not the non-owner rule.
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();

    // One byte over the MESSAGE cap in total, chopped into fragments far below
    // the frame cap. The fragment size is fixed rather than "half the payload"
    // so every frame stays under the frame cap regardless of the total.
    //
    // The marker leads the payload rather than trailing it: it therefore rides
    // the FIRST fragment, so an accepted message starts echoing immediately
    // instead of after 16 MiB have travelled. A trailing marker made the failure
    // path take 8.4s against an 8s deadline under mutation, one slow machine away
    // from an accepted payload being scored as a refusal.
    const FRAGMENT: usize = 4 * 1024 * 1024;
    let marker = b"dux-fragmented-oversize-marker\n";
    let mut payload = marker.to_vec();
    payload.resize(OVER_MESSAGE_CAP_BYTES, b'x');
    assert_eq!(payload.len(), OVER_MESSAGE_CAP_BYTES);

    // A Binary opener that is not final, then Continue frames with only the last
    // one final: one logical message assembled server-side, which is where
    // `max_message_size` is checked.
    let chunks: Vec<&[u8]> = payload.chunks(FRAGMENT).collect();
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        let opcode = if i == 0 {
            OpCode::Data(Data::Binary)
        } else {
            OpCode::Data(Data::Continue)
        };
        // A send may fail once the server has already torn the socket down,
        // which is just as good an answer as a later close.
        let _ = ws
            .send(Message::Frame(Frame::message(
                chunk.to_vec(),
                opcode,
                i == last,
            )))
            .await;
    }

    let mut acc = Vec::new();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => acc.extend_from_slice(&b),
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                ended = true;
                break;
            }
            _ => {}
        }
        // A leading marker means an accepted payload is identifiable from the
        // first bytes back, so stop rather than spending the whole deadline
        // reading 16 MiB of `x` to reach a conclusion already reached.
        if String::from_utf8_lossy(&acc).contains("dux-fragmented-oversize-marker") {
            break;
        }
    }

    assert!(
        !String::from_utf8_lossy(&acc).contains("dux-fragmented-oversize-marker"),
        "a fragmented message past MAX_WS_MESSAGE_SIZE reached the PTY and echoed \
         back; the configured message cap is not being applied"
    );
    assert!(
        ended,
        "the socket stayed open after an over-cap fragmented message; the cap must \
         refuse it, not ignore it"
    );
}

/// A Text frame `{"rows":R,"cols":C}` on a PTY socket is routed to resize, NOT
/// written to the PTY as stdin: the `cat` provider echoes stdin, so if the resize
/// JSON were mistakenly written it would echo back. We assert the resize JSON never
/// appears in the stream while a subsequent Binary marker still echoes — proving the
/// text frame was consumed as a resize and streaming survived it.
#[tokio::test]
async fn nested_pty_socket_resize_text_frame_is_not_stdin() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");

    // Send a resize control frame, then a Binary stdin marker.
    ws.send(Message::Text(r#"{"rows":40,"cols":120}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(
        b"dux-after-resize-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();

    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-after-resize-marker") {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&acc);
    assert!(
        text.contains("dux-after-resize-marker"),
        "streaming broke after a resize frame; got {} bytes",
        acc.len()
    );
    assert!(
        !text.contains("\"rows\":40"),
        "the resize JSON was echoed as stdin — it was not routed to resize: {text}"
    );
}

/// The nested terminal PTY socket enforces that `:tid` belongs to `:id`: a terminal
/// created under `s1` streams on the `s1` path but is REJECTED (no upgrade) on the
/// `s2` path, even though `s2` is itself a valid session.
#[tokio::test]
async fn nested_terminal_pty_socket_enforces_session_ownership() {
    let (addr, _tmp) = boot_two_sessions().await;
    let terminal_id = create_terminal_via_rest(addr, "s1").await;

    // The matching session path attaches and streams (the `cat` terminal echoes).
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/s1/terminals/{terminal_id}/pty"
    ))
    .await
    .expect("connect terminal pty on the owning session");
    // Claim ownership first so this connection's stdin is forwarded (non-owner
    // stdin is dropped under the per-PTY active-owner model).
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(
        b"dux-owned-terminal-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-owned-terminal-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-owned-terminal-marker"),
        "owning-session terminal socket did not stream; got {} bytes",
        acc.len()
    );

    // The WRONG session path is rejected before upgrade (404 → connect error),
    // even though s2 is a real session — the terminal belongs to s1.
    let foreign = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/s2/terminals/{terminal_id}/pty"
    ))
    .await;
    assert!(
        foreign.is_err(),
        "a terminal must not be attachable through a different session's path"
    );

    // An unknown terminal id on a valid session is likewise rejected.
    let unknown = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/s1/terminals/does-not-exist/pty"
    ))
    .await;
    assert!(unknown.is_err(), "an unknown terminal id must be rejected");
}

/// The nested agent PTY socket rejects (no upgrade) an unknown session id.
#[tokio::test]
async fn nested_agent_pty_socket_rejects_unknown_session() {
    let (addr, _tmp) = boot().await;
    let result =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/does-not-exist/pty"))
            .await;
    assert!(
        result.is_err(),
        "an unknown session must not yield a PTY socket upgrade"
    );
}

/// The companion-terminal REST verbs create and delete a terminal: `POST` returns
/// 201 with the new id (which then appears in the spine), `DELETE` returns 204 and
/// the terminal disappears, and `DELETE` of an unknown terminal is 404.
#[tokio::test]
async fn terminal_rest_create_and_delete() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let terminal_id = create_terminal_via_rest(addr, "s1").await;
    assert!(
        wait_for_workspace(addr, |spine| spine_has_terminal(spine, &terminal_id)).await,
        "spine never contained the REST-created terminal {terminal_id}"
    );

    // Deleting an unknown terminal on a valid session is a 404.
    let missing = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/s1/terminals/does-not-exist"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404, "unknown terminal → 404");

    // Deleting on an unknown session is a 404.
    let missing_session = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/nope/terminals/{terminal_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing_session.status().as_u16(),
        404,
        "unknown session → 404"
    );

    // The real delete returns 204 and the terminal disappears from the spine.
    let deleted = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/s1/terminals/{terminal_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status().as_u16(), 204, "delete → 204");
    assert!(
        wait_for_workspace(addr, |spine| !spine_has_terminal(spine, &terminal_id)).await,
        "spine still contained terminal {terminal_id} after delete"
    );

    // Creating on an unknown session is a 404.
    let bad_create = client
        .post(format!("http://{addr}/api/v1/sessions/nope/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad_create.status().as_u16(),
        404,
        "create on unknown session → 404"
    );
}

/// Create a project terminal on `project_id` over REST and return its id.
async fn create_project_terminal_via_rest(addr: SocketAddr, project_id: &str) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{addr}/api/v1/projects/{project_id}/terminals"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        201,
        "project terminal create should be 201"
    );
    assert!(
        resp.headers().contains_key("location"),
        "201 must carry a Location header"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    body["terminal_id"]
        .as_str()
        .expect("terminal_id in create response")
        .to_string()
}

/// The project-terminal REST verbs create and delete a project terminal: `POST`
/// returns 201 (+Location) with the new id (which then appears on the PROJECT in
/// the spine, never on a session), `DELETE` returns 204, and unknown ids 404.
#[tokio::test]
async fn project_terminal_rest_create_and_delete() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let terminal_id = create_project_terminal_via_rest(addr, "p1").await;
    assert!(
        wait_for_workspace(addr, |spine| spine_has_project_terminal(
            spine,
            &terminal_id
        ))
        .await,
        "spine's project never carried the REST-created project terminal {terminal_id}"
    );
    // The owner filter must not leak it onto a session.
    let spine: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/workspace"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !spine_has_terminal(&spine, &terminal_id),
        "a project terminal must never appear in a session's terminals"
    );

    // Unknown terminal id on a valid project is a 404.
    let missing = client
        .delete(format!(
            "http://{addr}/api/v1/projects/p1/terminals/does-not-exist"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404, "unknown terminal → 404");

    // Unknown project is a 404.
    let missing_project = client
        .delete(format!(
            "http://{addr}/api/v1/projects/nope/terminals/{terminal_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing_project.status().as_u16(),
        404,
        "unknown project → 404"
    );

    // The real delete returns 204 and the terminal disappears from the spine.
    let deleted = client
        .delete(format!(
            "http://{addr}/api/v1/projects/p1/terminals/{terminal_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status().as_u16(), 204, "delete → 204");
    assert!(
        wait_for_workspace(addr, |spine| !spine_has_project_terminal(
            spine,
            &terminal_id
        ))
        .await,
        "spine still carried project terminal {terminal_id} after delete"
    );

    // Creating on an unknown project is a 404.
    let bad_create = client
        .post(format!("http://{addr}/api/v1/projects/nope/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad_create.status().as_u16(),
        404,
        "create on unknown project → 404"
    );
}

/// Create a standalone terminal over REST and return its id. No owner id,
/// because there is no owner: this is the whole shape of the un-nested address.
async fn create_standalone_terminal_via_rest(addr: SocketAddr) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        201,
        "standalone terminal create should be 201"
    );
    assert!(
        resp.headers().contains_key("location"),
        "201 must carry a Location header"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    body["terminal_id"]
        .as_str()
        .expect("terminal_id in create response")
        .to_string()
}

/// Whether the spine's flat `terminals` collection carries `terminal_id` tagged
/// as owned by nothing.
fn spine_has_standalone_terminal(spine: &serde_json::Value, terminal_id: &str) -> bool {
    spine_terminal_has_owner_kind(spine, terminal_id, "standalone")
}

/// The journey: a user opens a terminal that belongs to nothing, works in it,
/// and closes it, all through addresses that name no owner. It reaches the
/// browser as part of the one flat collection, tagged `standalone` and carrying
/// the directory it opened in.
#[tokio::test]
async fn a_standalone_terminal_opens_streams_and_closes_at_un_nested_addresses() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let tid = create_standalone_terminal_via_rest(addr).await;

    // It arrives in the ONE flat collection, tagged as owned by nothing, and
    // carrying the `~`-shortened directory its row names it by.
    assert!(
        wait_for_workspace(addr, |spine| {
            spine_has_standalone_terminal(spine, &tid)
                && spine["terminals"]
                    .as_array()
                    .map(|ts| {
                        ts.iter().any(|t| {
                            t["id"].as_str() == Some(tid.as_str())
                                && t["owner"]["cwd_label"]
                                    .as_str()
                                    .is_some_and(|l| !l.is_empty())
                        })
                    })
                    .unwrap_or(false)
        })
        .await,
        "a standalone terminal must reach the browser tagged, with its directory"
    );

    // Its own websocket, un-nested, streams its bytes.
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/terminals/{tid}/pty"))
            .await
            .expect("connect the standalone terminal pty");
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(b"dux-standalone-marker\n".to_vec().into()))
        .await
        .unwrap();
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-standalone-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-standalone-marker"),
        "the standalone terminal socket did not stream; got {} bytes",
        acc.len()
    );
    let _ = ws.close(None).await;

    // And it closes at its own un-nested address.
    let del = client
        .delete(format!("http://{addr}/api/v1/terminals/{tid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        del.status().as_u16(),
        204,
        "standalone delete should be 204"
    );
    assert!(
        wait_for_workspace(addr, |spine| !spine_has_standalone_terminal(spine, &tid)).await,
        "the deleted standalone terminal must leave the spine"
    );
}

/// The un-nested address is not a back door. An OWNED terminal is a 404 there,
/// on both verbs and on the socket, and a standalone terminal is a 404 on both
/// nested addresses, so the existing cross-owner rejections still refuse with a
/// third kind in play.
#[tokio::test]
async fn the_un_nested_terminal_address_refuses_owned_terminals_and_vice_versa() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let session_tid = create_terminal_via_rest(addr, "s1").await;
    let project_tid = create_project_terminal_via_rest(addr, "p1").await;
    let standalone_tid = create_standalone_terminal_via_rest(addr).await;

    for owned in [&session_tid, &project_tid] {
        let resp = client
            .delete(format!("http://{addr}/api/v1/terminals/{owned}"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            404,
            "an owned terminal must 404 on the un-nested delete"
        );
        assert!(
            tokio_tungstenite::connect_async(format!("ws://{addr}/ws/terminals/{owned}/pty"))
                .await
                .is_err(),
            "an owned terminal must not attach at the un-nested socket"
        );
    }

    // And the other direction: the standalone terminal is a 404 under both owners.
    let via_session = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/s1/terminals/{standalone_tid}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        via_session.status().as_u16(),
        404,
        "a standalone terminal must 404 on the session route"
    );
    let via_project = client
        .delete(format!(
            "http://{addr}/api/v1/projects/p1/terminals/{standalone_tid}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        via_project.status().as_u16(),
        404,
        "a standalone terminal must 404 on the project route"
    );
    assert!(
        tokio_tungstenite::connect_async(format!(
            "ws://{addr}/ws/sessions/s1/terminals/{standalone_tid}/pty"
        ))
        .await
        .is_err(),
        "a standalone terminal must not attach through a session path"
    );

    // Nothing was deleted by any of those refusals.
    assert!(
        wait_for_workspace(addr, |spine| {
            spine_has_terminal(spine, &session_tid)
                && spine_has_project_terminal(spine, &project_tid)
                && spine_has_standalone_terminal(spine, &standalone_tid)
        })
        .await,
        "the cross-owner 404s must not delete anything"
    );
}

/// Ownership is enforced per VARIANT, both directions: a project terminal is a
/// 404 on the session-nested route, and a session terminal is a 404 on the
/// project-nested route. A raw-id comparison across owner kinds would pass one
/// of these.
#[tokio::test]
async fn terminal_delete_routes_404_across_owner_kinds() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let session_tid = create_terminal_via_rest(addr, "s1").await;
    let project_tid = create_project_terminal_via_rest(addr, "p1").await;

    // A project terminal through the session route: 404, and it survives.
    let cross1 = client
        .delete(format!(
            "http://{addr}/api/v1/sessions/s1/terminals/{project_tid}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross1.status().as_u16(),
        404,
        "a project terminal must 404 on the session route"
    );

    // A session terminal through the project route: 404, and it survives.
    let cross2 = client
        .delete(format!(
            "http://{addr}/api/v1/projects/p1/terminals/{session_tid}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross2.status().as_u16(),
        404,
        "a session terminal must 404 on the project route"
    );

    // Both terminals still exist (the cross-owner 404s deleted nothing).
    assert!(
        wait_for_workspace(addr, |spine| {
            spine_has_terminal(spine, &session_tid)
                && spine_has_project_terminal(spine, &project_tid)
        })
        .await,
        "cross-owner 404s must not delete either terminal"
    );
}

/// The project-nested terminal PTY socket streams the project terminal's bytes
/// and enforces per-variant ownership: a session terminal is rejected on the
/// project path, a project terminal is rejected on the session path, and an
/// unknown project is rejected outright.
#[tokio::test]
async fn nested_project_terminal_pty_socket_enforces_project_ownership() {
    let (addr, _tmp) = boot().await;
    let project_tid = create_project_terminal_via_rest(addr, "p1").await;
    let session_tid = create_terminal_via_rest(addr, "s1").await;

    // The matching project path attaches and streams (the `cat` terminal echoes).
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/projects/p1/terminals/{project_tid}/pty"
    ))
    .await
    .expect("connect project terminal pty on the owning project");
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(
        b"dux-project-terminal-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-project-terminal-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-project-terminal-marker"),
        "owning-project terminal socket did not stream; got {} bytes",
        acc.len()
    );

    // A session-owned terminal through the project path is rejected pre-upgrade.
    let cross = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/projects/p1/terminals/{session_tid}/pty"
    ))
    .await;
    assert!(
        cross.is_err(),
        "a session terminal must not attach through a project path"
    );

    // A project terminal through the session path is rejected pre-upgrade.
    let cross_back = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/s1/terminals/{project_tid}/pty"
    ))
    .await;
    assert!(
        cross_back.is_err(),
        "a project terminal must not attach through a session path"
    );

    // An unknown project is rejected outright.
    let unknown = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/projects/nope/terminals/{project_tid}/pty"
    ))
    .await;
    assert!(unknown.is_err(), "an unknown project must be rejected");
}

/// Tearing an agent's PTY down server-side (here via `POST .../kill`, which
/// hard-drops the provider) while a client is still attached to its PTY socket
/// must proactively close that socket instead of leaving it dangling until the
/// client disconnects on its own. Before the `pty_forwarder` completion arm was
/// added to `handle_pty_socket`'s `select!` loop, the forwarder task ending
/// (because the PTY was torn down) was invisible to the loop, so the socket
/// (and its connection-cap permit/guard) would linger. The kill path drops the
/// `PtyClient` immediately (hard SIGKILL), so the forwarder ends deterministically.
#[tokio::test]
async fn tearing_down_agent_pty_closes_its_attached_socket() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    // Connecting subscribes + launches the `cat` provider, so after this the
    // agent has a live PTY the kill can tear down.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/pty"))
        .await
        .expect("connect agent pty socket");
    // Claim ownership and echo a marker so we know the PTY is fully up before
    // we kill it (avoids racing the launch).
    ws.send(Message::Text(r#"{"rows":24,"cols":80}"#.into()))
        .await
        .unwrap();
    ws.send(Message::Binary(b"dux-kill-marker\n".to_vec().into()))
        .await
        .unwrap();
    let up_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut acc = Vec::new();
    while tokio::time::Instant::now() < up_deadline {
        if let Ok(Some(Ok(Message::Binary(b)))) =
            tokio::time::timeout(Duration::from_millis(300), ws.next()).await
        {
            acc.extend_from_slice(&b);
            if String::from_utf8_lossy(&acc).contains("dux-kill-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-kill-marker"),
        "agent PTY never came up"
    );

    let killed = client
        .post(format!("http://{addr}/api/v1/sessions/s1/kill"))
        .send()
        .await
        .unwrap();
    assert_eq!(killed.status().as_u16(), 200, "kill → 200");

    // The socket must close on its own (Close frame or stream end) well within
    // the liveness-ping window, not merely go quiet.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut closed = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                closed = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        closed,
        "pty socket for a torn-down agent was not proactively closed"
    );
}

/// The session-nested git and file routes reach their handlers over real HTTP (a
/// stage against an unknown session 404s), and the old body-keyed `/api/v1/git/*` /
/// `/api/v1/file/*` paths no longer reach the handler (they fall through to the SPA
/// fallback, which never returns the handler's 404).
#[tokio::test]
async fn rest_nested_git_and_file_routes_resolve() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let git = client
        .post(format!("http://{addr}/api/v1/sessions/nope/git/stage"))
        .json(&serde_json::json!({"path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(git.status().as_u16(), 404, "/api/v1/sessions/:id/git/stage");

    let file = client
        .post(format!("http://{addr}/api/v1/sessions/nope/files/read"))
        .json(&serde_json::json!({"path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        file.status().as_u16(),
        404,
        "/api/v1/sessions/:id/files/read"
    );

    // The retired body-keyed paths no longer reach the handler (no 404 from it).
    let old_git = client
        .post(format!("http://{addr}/api/v1/git/stage"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        old_git.status().as_u16(),
        404,
        "the old body-keyed /api/v1/git/* path must be gone"
    );

    let old_file = client
        .post(format!("http://{addr}/api/v1/file/read"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        old_file.status().as_u16(),
        404,
        "the old body-keyed /api/v1/file/* path must be gone"
    );
}

/// The thin programmability reads (`GET /api/v1/sessions/:id`, `/api/v1/sessions`,
/// `/api/v1/projects`) keep nesting each owner's terminals.
///
/// The flat, owner-tagged collection is a change to what the BROWSER receives on
/// `/api/v1/workspace`. These three endpoints are documented separately, as the
/// shapes a script or an integration reads, and they carried a `terminals` array
/// on the owner. Dropping it would take information away from every such
/// consumer with no way to get it back short of moving to the spine endpoint,
/// which is a different document with different invalidation.
#[tokio::test]
async fn thin_reads_still_nest_terminals_for_both_owner_kinds() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    // Before anything is spawned: an owner with no terminals carries an EMPTY
    // array, never a missing field, exactly as the nested shape always did.
    let empty: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        empty["terminals"].as_array().map(|a| a.len()),
        Some(0),
        "an owner with no terminals must carry an empty array"
    );

    let session_tid = create_terminal_via_rest(addr, "s1").await;
    let project_tid = create_project_terminal_via_rest(addr, "p1").await;

    let one: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        one["terminals"].as_array().map(|a| a
            .iter()
            .filter_map(|t| t["id"].as_str())
            .collect::<Vec<_>>()),
        Some(vec![session_tid.as_str()]),
        "the per-session read must still nest that session's terminals"
    );
    // The session's own fields are untouched alongside it.
    assert_eq!(one["id"].as_str(), Some("s1"));
    assert_eq!(one["workspace"]["project_id"].as_str(), Some("p1"));

    let sessions: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s1 = sessions
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["id"].as_str() == Some("s1")))
        .expect("s1 in the sessions list");
    assert_eq!(
        s1["terminals"].as_array().map(|a| a
            .iter()
            .filter_map(|t| t["id"].as_str())
            .collect::<Vec<_>>()),
        Some(vec![session_tid.as_str()]),
        "the sessions list must still nest each session's terminals"
    );
    let projects: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let p1 = projects
        .as_array()
        .and_then(|arr| arr.iter().find(|p| p["id"].as_str() == Some("p1")))
        .expect("p1 in the projects list");
    assert_eq!(
        p1["terminals"].as_array().map(|a| a
            .iter()
            .filter_map(|t| t["id"].as_str())
            .collect::<Vec<_>>()),
        Some(vec![project_tid.as_str()]),
        "the projects list must still nest each project's own project terminals"
    );
    // The project's terminals are ITS OWN: the session terminal above belongs to
    // s1, not to p1, so it must not be folded in here.
    assert_eq!(
        p1["terminals"].as_array().map(|a| a.len()),
        Some(1),
        "a project must not absorb its sessions' terminals"
    );
}

/// `GET /api/v1/build` is the probe a reconnecting tab uses to decide whether the
/// server it got back to is the one that served it.
///
/// What this pins is the property an obvious implementation gets wrong: the run
/// id is minted ONCE per process, so every read within one server run agrees. A
/// per-request uuid would tell two servers apart just as well, and would also
/// tell the SAME tab that the server changed on every single reconnect, hard
/// reloading it forever. Two separately booted routers here share one process on
/// purpose, which is exactly the "same server, reconnected" case.
///
/// It also answers with NO engine round-trip, which is why a client can ask right
/// after a reconnect while the engine is still coming up, and it forbids caching,
/// since a cached answer would be the tab telling itself what it already believes.
#[tokio::test]
async fn build_identity_is_stable_within_one_server_run_and_is_never_cached() {
    let (first_addr, _tmp_a) = boot().await;
    let (second_addr, _tmp_b) = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{first_addr}/api/v1/build"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "a cached build probe cannot notice that the server changed"
    );
    let first: serde_json::Value = resp.json().await.unwrap();

    // A second read, and a read from another router in the same process: both
    // must agree, or a reconnecting tab would reload on every blip.
    let again: serde_json::Value = client
        .get(format!("http://{first_addr}/api/v1/build"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other: serde_json::Value = client
        .get(format!("http://{second_addr}/api/v1/build"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first, again, "two reads of one server run must agree");
    assert_eq!(
        first, other,
        "one process is one run, whatever is serving it"
    );

    // The documented shape, exactly: two keys, both non-empty strings.
    let mut keys: Vec<&str> = first
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["process", "version"]);
    assert!(!first["process"].as_str().unwrap().is_empty());
    assert!(!first["version"].as_str().unwrap().is_empty());
}

/// The exact key set of a terminal entry on the documented thin reads, and of
/// the session object it hangs off.
///
/// Every other assertion in this file checks ids and lengths, so a field
/// appearing or disappearing on a documented endpoint is invisible here until
/// somebody's script breaks. This key set is not a claim that the shape must
/// never change; it is what makes a change to it a decision taken in a diff.
const TERMINAL_KEYS: &[&str] = &[
    "created_at",
    "foreground_cmd",
    "has_output",
    "id",
    "label",
    "owner",
    "sort_order",
    "typing",
    "updated_at",
    "working",
];

/// Sorted key set of a JSON object, for comparison against the pins above.
fn keys_of(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Assert `value` is a terminal entry with exactly the pinned key set.
fn assert_terminal_shape(value: &serde_json::Value, where_: &str) {
    assert_eq!(
        keys_of(value),
        TERMINAL_KEYS,
        "the terminal entry on {where_} does not carry the documented key set"
    );
    // The owner tag itself is part of the documented shape: an internally tagged
    // enum whose payload key names the owner kind.
    let owner_keys = keys_of(&value["owner"]);
    assert!(
        owner_keys == vec!["kind", "session_id"] || owner_keys == vec!["kind", "project_id"],
        "the owner tag on {where_} must be {{kind, session_id}} or {{kind, project_id}}, got {owner_keys:?}"
    );
}

/// `GET /api/v1/sessions/:id`, `GET /api/v1/sessions` and `GET /api/v1/projects`
/// all nest terminals, and all three serve the same terminal entry shape.
#[tokio::test]
async fn thin_reads_pin_the_exact_terminal_key_set() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let session_tid = create_terminal_via_rest(addr, "s1").await;
    let project_tid = create_project_terminal_via_rest(addr, "p1").await;

    let one: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entry = &one["terminals"][0];
    assert_eq!(entry["id"].as_str(), Some(session_tid.as_str()));
    assert_terminal_shape(entry, "GET /api/v1/sessions/:id");
    assert_eq!(entry["owner"]["kind"].as_str(), Some("session"));
    assert_eq!(entry["owner"]["session_id"].as_str(), Some("s1"));

    let sessions: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s1 = sessions
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["id"].as_str() == Some("s1")))
        .expect("s1 in the sessions list");
    assert_terminal_shape(&s1["terminals"][0], "GET /api/v1/sessions");

    let projects: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let p1 = projects
        .as_array()
        .and_then(|arr| arr.iter().find(|p| p["id"].as_str() == Some("p1")))
        .expect("p1 in the projects list");
    let project_entry = &p1["terminals"][0];
    assert_eq!(project_entry["id"].as_str(), Some(project_tid.as_str()));
    assert_terminal_shape(project_entry, "GET /api/v1/projects");
    assert_eq!(project_entry["owner"]["kind"].as_str(), Some("project"));
    assert_eq!(project_entry["owner"]["project_id"].as_str(), Some("p1"));

    // The per-session read and the list must serve the SAME object for the same
    // terminal: they are two ways of asking one question, and a shape that drifts
    // between them is the bug this pins.
    assert_eq!(
        one["terminals"][0], s1["terminals"][0],
        "the per-session read and the sessions list must agree field for field"
    );
    // And the session object around it keeps its own key set, with `terminals`
    // present rather than merged away by the `flatten`.
    let session_keys = keys_of(&one);
    assert!(
        session_keys.contains(&"terminals".to_string()),
        "the nested terminals array must survive: {session_keys:?}"
    );
    assert!(session_keys.contains(&"id".to_string()));
    // The project id lives inside the tagged workspace now, not beside it.
    assert!(session_keys.contains(&"workspace".to_string()));
}

/// `POST /api/v1/sessions` and its idempotent replay serve the same nested shape
/// as the per-session read, terminal entries included. The replay always does;
/// the create does so only when the session view is available, falling back to a
/// minimal id-only body when it is not, which is why this test asserts it took
/// the full branch before pinning anything there.
///
/// The replay is taken AFTER the new session has acquired a terminal, which is
/// the only way to see a terminal entry on that path at all: a session is created
/// with none, so a replay issued immediately would assert against an empty array
/// and prove nothing about the entry shape.
#[tokio::test]
async fn session_create_and_its_replay_pin_the_same_terminal_key_set() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .header("idempotency-key", "shape-1")
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"shape"}))
        .send()
        .await
        .expect("create");
    assert_eq!(created.status().as_u16(), 201);
    let created_body: serde_json::Value = created.json().await.unwrap();
    let session_id = created_body["id"].as_str().expect("created id").to_string();
    // The 201 has TWO shapes: this full session view, and a minimal id-only
    // fallback for when that view is unavailable. This scenario always takes the
    // full branch, so assert that it did and pin what is there. It still cannot
    // pin a terminal ENTRY, because a session created a moment ago owns none;
    // the entry shape comes from the replay below, which shares this branch's
    // type.
    let mut created_keys: Vec<&str> = created_body
        .as_object()
        .expect("201 object")
        .keys()
        .map(String::as_str)
        .collect();
    created_keys.sort_unstable();
    assert!(
        created_keys.len() > 1,
        "this scenario must take the full-view branch, not the id-only fallback: {created_keys:?}"
    );
    assert!(
        created_keys.contains(&"terminals"),
        "the 201 must carry a terminals array: {created_keys:?}"
    );
    assert_eq!(
        created_body["terminals"].as_array().map(|a| a.len()),
        Some(0),
        "a brand new session carries an empty terminals array, never a missing field"
    );

    let terminal_id = create_terminal_via_rest(addr, &session_id).await;

    let replay = client
        .post(format!("http://{addr}/api/v1/sessions"))
        .header("idempotency-key", "shape-1")
        .json(&serde_json::json!({"kind":"new","project_id":"p1","name":"shape"}))
        .send()
        .await
        .expect("replay");
    assert_eq!(
        replay.status().as_u16(),
        200,
        "an idempotent replay returns 200, not a second 201"
    );
    let replay_body: serde_json::Value = replay.json().await.unwrap();
    assert_eq!(replay_body["id"].as_str(), Some(session_id.as_str()));
    let entry = &replay_body["terminals"][0];
    assert_eq!(entry["id"].as_str(), Some(terminal_id.as_str()));
    assert_terminal_shape(entry, "the POST /api/v1/sessions idempotent replay");
    assert_eq!(entry["owner"]["kind"].as_str(), Some("session"));
    assert_eq!(
        entry["owner"]["session_id"].as_str(),
        Some(session_id.as_str())
    );

    // A replay and a later GET of the same session agree field for field, which
    // is the promise the replay path makes in its own comment.
    let read: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/{session_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        keys_of(&read),
        keys_of(&replay_body),
        "the replay and the per-session read must serve the same session key set"
    );
    assert_terminal_shape(
        &read["terminals"][0],
        "GET /api/v1/sessions/:id after create",
    );
}
