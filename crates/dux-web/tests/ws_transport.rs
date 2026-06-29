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
        worktree_path: worktree.to_string(),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
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

/// HTTP `GET`/`POST /api/v1/file/diff` returns both raw sides of a changed file and
/// rejects a path that escapes the worktree — the HTTP-layer coverage that replaced
/// the deleted WS `get_diff` test (route wiring, session resolution, the boundary,
/// and JSON shape).
#[tokio::test]
async fn http_file_diff_returns_sides_and_rejects_traversal() {
    let (addr, _tmp) = boot_with_repo().await;
    let client = reqwest::Client::new();

    // The seeded repo has f.txt = "line2" at HEAD, "CHANGED" in the working copy.
    let resp = client
        .post(format!("http://{addr}/api/v1/file/diff"))
        .json(&serde_json::json!({ "session_id": "s1", "path": "f.txt" }))
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
        .post(format!("http://{addr}/api/v1/file/diff"))
        .json(&serde_json::json!({ "session_id": "s1", "path": "../escape" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "path traversal must be rejected");
}

/// HTTP `GET /api/v1/file/raw` (the markdown-preview image proxy) serves a worktree
/// file's bytes with a guessed content type, and rejects a path that escapes the
/// worktree.
#[tokio::test]
async fn http_file_raw_serves_bytes_and_rejects_traversal() {
    let (addr, _tmp) = boot_with_repo().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{addr}/api/v1/file/raw?session_id=s1&path=f.txt"
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
            "http://{addr}/api/v1/file/raw?session_id=s1&path=../escape"
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

/// Whether the spine carries a session whose `terminals` include `terminal_id`.
fn spine_has_terminal(spine: &serde_json::Value, terminal_id: &str) -> bool {
    spine["sessions"]
        .as_array()
        .map(|sessions| {
            sessions.iter().any(|s| {
                s["terminals"]
                    .as_array()
                    .map(|ts| ts.iter().any(|t| t["id"].as_str() == Some(terminal_id)))
                    .unwrap_or(false)
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
    ws.send(Message::Binary(b"dux-nested-agent-marker\n".to_vec()))
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
    ws.send(Message::Binary(b"dux-after-resize-marker\n".to_vec()))
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
    ws.send(Message::Binary(b"dux-owned-terminal-marker\n".to_vec()))
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

/// The `/api/v1` git and file routes reach their handlers over real HTTP (a stage
/// against an unknown session 404s), and the retired unversioned `/api/git/*` /
/// `/api/file/*` paths no longer reach the handler (they fall through to the SPA
/// fallback, which never returns the handler's 404).
#[tokio::test]
async fn rest_v1_git_and_file_routes_resolve() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let v1_git = client
        .post(format!("http://{addr}/api/v1/git/stage"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(v1_git.status().as_u16(), 404, "/api/v1/git/stage");

    let v1_file = client
        .post(format!("http://{addr}/api/v1/file/read"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(v1_file.status().as_u16(), 404, "/api/v1/file/read");

    // The retired legacy aliases no longer reach the handler (no 404 from it).
    let legacy_git = client
        .post(format!("http://{addr}/api/git/stage"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        legacy_git.status().as_u16(),
        404,
        "the legacy /api/git/* alias must be gone"
    );

    let legacy_file = client
        .post(format!("http://{addr}/api/file/read"))
        .json(&serde_json::json!({"session_id":"nope","path":"a.txt"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        legacy_file.status().as_u16(),
        404,
        "the legacy /api/file/* alias must be gone"
    );
}
