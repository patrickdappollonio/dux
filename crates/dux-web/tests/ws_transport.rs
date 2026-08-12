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
        project_id: project_id.to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: branch.to_string(),
        initial_branch: branch.to_string(),
        worktree_path: worktree.to_string(),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
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

/// Poll `GET /api/v1/spine` until `pred` holds or the deadline lapses. The
/// projects/sessions/sidebar spine is a REST read (the matching
/// `projects.changed` / `sessions.changed` event rides `/ws/events`). `true` if
/// `pred` ever held.
async fn wait_for_spine<F>(addr: SocketAddr, pred: F) -> bool
where
    F: Fn(&serde_json::Value) -> bool,
{
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .get(format!("http://{addr}/api/v1/spine"))
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
/// tagged as owned by a session. Terminals no longer arrive nested inside the
/// session or project that owns them; they arrive flat, each carrying a tagged
/// owner, so these two helpers now read the tag instead of the nesting.
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

/// The replay bug, as the user hit it: an operation fails, its error toast is
/// broadcast to everyone watching, and then a NEW browser opens the page. That
/// browser missed the event and must not be told about it, or an hour-old
/// failure is re-raised as a fresh toast on every page load, every new tab and
/// every reconnect.
///
/// The status is deliberately raised WITHOUT an `X-Connection-Id`, so its scope
/// is `All` and per-connection filtering cannot make this pass for the wrong
/// reason. The already-attached connection seeing it is the control.
#[tokio::test]
async fn a_finished_operations_error_is_never_replayed_to_a_later_connection() {
    let (addr, _tmp) = boot().await;

    // The browser that is already open when the operation runs.
    let (mut ws_a, _id_a) = connect_events(addr).await;

    // Deleting s1 with its worktree runs an async removal whose git call fails
    // (the seeded worktree path is a plain directory, not a linked worktree), so
    // the keyed busy resolves into a real error final: exactly the shape of the
    // toast users were seeing resurrected on every page load.
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

    // Control: the connection that was watching DID receive the error.
    let seen = saw_status_tone(&mut ws_a, "error", Duration::from_secs(10)).await;
    let seen = seen.expect("the attached connection must receive the broadcast error");

    // And it arrives STICKY, end to end from the engine resolver to the wire: a
    // failed worktree removal leaves an orphaned directory on disk that only the
    // user can clear, so this toast must wait for them rather than time out.
    assert!(
        seen.contains("\"sticky\":true"),
        "a half-done delete must be marked sticky on the wire, got {seen}"
    );

    // Let the actor settle so any snapshot write has landed; the point of the
    // test is that time passing does NOT make the error replayable.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The journey: a new browser opens the page afterwards.
    let (mut ws_b, _id_b) = connect_events(addr).await;
    let replayed = saw_status_tone(&mut ws_b, "error", Duration::from_secs(2)).await;
    assert!(
        replayed.is_none(),
        "a connection that opened after the failure must not be told about it, got {replayed:?}"
    );

    // And a reconnect is the same journey again: a third connection is likewise
    // owed nothing, so the error cannot be resurrected by reconnecting either.
    let (mut ws_c, _id_c) = connect_events(addr).await;
    assert!(
        saw_status_tone(&mut ws_c, "error", Duration::from_secs(1))
            .await
            .is_none(),
        "the error must not come back on any later connection"
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
    assert_eq!(body["project_id"].as_str(), Some("p1"));

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
                .filter(|s| s["project_id"].as_str() == Some("p1"))
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
        wait_for_spine(addr, |spine| spine_has_terminal(spine, &terminal_id)).await,
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
        wait_for_spine(addr, |spine| !spine_has_terminal(spine, &terminal_id)).await,
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
        wait_for_spine(addr, |spine| spine_has_project_terminal(
            spine,
            &terminal_id
        ))
        .await,
        "spine's project never carried the REST-created project terminal {terminal_id}"
    );
    // The owner filter must not leak it onto a session.
    let spine: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/spine"))
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
        wait_for_spine(addr, |spine| !spine_has_project_terminal(
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
        wait_for_spine(addr, |spine| {
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
        wait_for_spine(addr, |spine| !spine_has_standalone_terminal(spine, &tid)).await,
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
        wait_for_spine(addr, |spine| {
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
        wait_for_spine(addr, |spine| {
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
/// `/api/v1/spine`. These three endpoints are documented separately, as the
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
    assert_eq!(one["project_id"].as_str(), Some("p1"));

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
/// `TerminalView` grew an `owner` tag when terminals moved to one flat
/// collection, so these responses changed. Adding a field is additive and is the
/// normal way an API grows, and `owner` earns its place because it says what the
/// terminal belongs to, which the nesting used to say implicitly. What was
/// actually wrong is that nothing here could have NOTICED: every assertion in
/// this file checked ids and lengths, so a field appearing or disappearing on a
/// documented endpoint was invisible until somebody's script broke.
///
/// So these are characterisation tests. They are not a claim that the shape must
/// never change; they are the thing that makes a change to it a decision, taken
/// in a diff, rather than a surprise.
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
    assert!(session_keys.contains(&"project_id".to_string()));
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
