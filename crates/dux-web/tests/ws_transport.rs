//! End-to-end WebSocket transport tests (the "debug client").

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

/// Like `boot()`, but writes a real (minimal) `config.toml` into `root` first so
/// the config-sync patch path in the engine actor has a file to patch. Returns
/// the temp dir's config.toml path so the test can read it back.
async fn boot_for_config() -> (SocketAddr, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let config_path = root.join("config.toml");
    std::fs::write(&config_path, "# dux config\n").unwrap();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: config_path.clone(),
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
    let engine = bootstrap_engine(&paths).unwrap();
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
    (addr, tmp, config_path)
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

/// Like `boot()`, but the session's worktree is a REAL git repo with a STAGED
/// change, and the provider is overridden to a deterministic one-shot command
/// (`bash -c 'echo …'`) that ignores the prompt and prints a known string to
/// stdout. This lets the commit-message test assert the exact generated message
/// streams back over the WebSocket without depending on a real AI provider.
async fn boot_for_commit_message() -> (SocketAddr, tempfile::TempDir) {
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
    std::fs::write(root.join("f.txt"), "line1\nline2\nline3\n").expect("write file");
    // Stage the file so `git diff --cached` has content.
    run(&["add", "f.txt"]);

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
    // Deterministic one-shot provider: ignores the prompt and prints a fixed
    // marker to stdout, so `run_oneshot` returns exactly "DETERMINISTIC-COMMIT-MSG".
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "bash".to_string(),
            args: vec![],
            resume_args: None,
            oneshot_args: vec![
                "-c".to_string(),
                "echo DETERMINISTIC-COMMIT-MSG".to_string(),
            ],
            ..Default::default()
        },
    );
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

/// Generating a commit message over the wire: the synchronous `command_result`
/// (or a broadcast `status`) carries the Busy "Generating an AI commit message"
/// notice, and the one-shot provider's deterministic output streams back later as
/// a `commit_message` frame. This exercises the full path: WireCommand ->
/// Command::GenerateCommitMessage -> spawned run_oneshot ->
/// WorkerEvent::CommitMessageGenerated -> EventReaction -> broadcast channel ->
/// commit-message-forwarder -> ServerMessage::CommitMessage.
#[tokio::test]
async fn generate_commit_message_streams_result() {
    let (addr, _tmp) = boot_for_commit_message().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"command","command":"generate_commit_message","args":{"session_id":"s1"}}"#
            .into(),
    ))
    .await
    .unwrap();

    let mut saw_busy = false;
    let mut commit_frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline && (!saw_busy || commit_frame.is_empty()) {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
        {
            if (t.contains("\"type\":\"command_result\"") || t.contains("\"type\":\"status\""))
                && t.contains("Generating an AI commit message")
            {
                saw_busy = true;
            }
            if t.contains("\"type\":\"commit_message\"") && t.contains("DETERMINISTIC-COMMIT-MSG") {
                commit_frame = t.to_string();
            }
        }
    }

    assert!(saw_busy, "never received the Busy generating status");
    assert!(
        !commit_frame.is_empty(),
        "never received the generated commit_message frame"
    );

    // CF2: the frame is session-scoped so the frontend routes it to the matching
    // commit dialog. The session it was generated for ("s1") must travel with it.
    let v: serde_json::Value = serde_json::from_str(&commit_frame).expect("parse commit frame");
    assert_eq!(
        v["session_id"].as_str(),
        Some("s1"),
        "commit_message frame must carry its originating session id: {commit_frame}"
    );
}

/// Fetching the diff for a changed file returns hunks carrying the insert/delete
/// content; a path-traversal request returns an error frame with a null diff.
#[tokio::test]
async fn get_diff_returns_hunks_for_a_changed_file() {
    let (addr, _tmp) = boot_with_repo().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"get_diff","session_id":"s1","path":"f.txt"}"#.into(),
    ))
    .await
    .unwrap();

    let mut diff_frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && diff_frame.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"diff\"")
        {
            diff_frame = t.to_string();
        }
    }
    assert!(!diff_frame.is_empty(), "never received a diff frame");

    let v: serde_json::Value = serde_json::from_str(&diff_frame).expect("parse diff frame");
    assert!(v["diff"].is_object(), "diff missing: {diff_frame}");
    assert!(v["error"].is_null(), "unexpected error: {diff_frame}");
    let hunks = v["diff"]["hunks"].as_array().expect("hunks array");
    assert!(!hunks.is_empty(), "hunks empty: {diff_frame}");
    assert!(
        diff_frame.contains("CHANGED"),
        "missing inserted content: {diff_frame}"
    );
    assert!(
        diff_frame.contains("line2"),
        "missing deleted content: {diff_frame}"
    );

    // A path that escapes the worktree must yield an error frame with a null diff.
    ws.send(Message::Text(
        r#"{"type":"get_diff","session_id":"s1","path":"../escape"}"#.into(),
    ))
    .await
    .unwrap();

    let mut err_frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && err_frame.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"diff\"")
            && t.contains("../escape")
        {
            err_frame = t.to_string();
        }
    }
    assert!(
        !err_frame.is_empty(),
        "never received the escape diff frame"
    );
    let v: serde_json::Value = serde_json::from_str(&err_frame).expect("parse escape frame");
    assert!(v["diff"].is_null(), "diff should be null: {err_frame}");
    assert!(!v["error"].is_null(), "error should be set: {err_frame}");
}

/// Anti-regression for the empty web changed-files pane: the web never set
/// `watched_worktree`, so the poller read `None` forever and the ViewModel's
/// `changed_files` stayed empty. Sending `watch_changed_files` for a session
/// whose worktree has changes must make a subsequent ViewModel frame carry
/// non-empty `changed_files` tagged with the watched session id. The
/// `boot_with_repo` session `s1` has an uncommitted edit to `f.txt`, so it is
/// an unstaged change.
#[tokio::test]
async fn watch_changed_files_populates_view_model_changed_files() {
    let (addr, _tmp) = boot_with_repo().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    // The initial ViewModel frame should have an EMPTY changed-files list and no
    // watched session — the exact pre-fix state.
    let initial = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("ws error")
        .into_text()
        .expect("text");
    assert!(
        initial.contains("\"type\":\"view_model\""),
        "expected view_model: {initial}"
    );
    let v: serde_json::Value = serde_json::from_str(&initial).expect("parse initial frame");
    assert!(
        v["data"]["changed_files"]["unstaged"]
            .as_array()
            .is_some_and(|a| a.is_empty()),
        "expected empty unstaged before watch: {initial}"
    );
    assert!(
        v["data"]["changed_files"]["watched_session_id"].is_null(),
        "expected no watched session before watch: {initial}"
    );

    // Ask the server to watch session s1's worktree.
    ws.send(Message::Text(
        r#"{"type":"command","command":"watch_changed_files","args":{"session_id":"s1"}}"#.into(),
    ))
    .await
    .unwrap();

    // A subsequent ViewModel frame must carry the unstaged f.txt change tagged
    // with the watched session id.
    let mut populated = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !populated {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse frame");
            let watched = v["data"]["changed_files"]["watched_session_id"].as_str();
            let unstaged = v["data"]["changed_files"]["unstaged"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if watched == Some("s1") && unstaged.iter().any(|f| f["path"].as_str() == Some("f.txt"))
            {
                populated = true;
            }
        }
    }
    assert!(
        populated,
        "never saw a ViewModel frame with the watched session's changed files"
    );
}

#[tokio::test]
async fn client_receives_view_model_on_connect() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("stream end")
        .expect("ws error");
    let text = msg.into_text().expect("text");
    assert!(text.contains("\"type\":\"view_model\""), "got: {text}");
    assert!(
        text.contains("\"id\":\"s1\""),
        "session not present: {text}"
    );
}

#[tokio::test]
async fn client_can_apply_a_command() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model
    let cmd = r#"{"type":"command","command":"toggle_agent_auto_reopen","args":{"session_id":"s1","enabled":true}}"#;
    ws.send(Message::Text(cmd.into())).await.unwrap();
    let mut saw_result = false;
    let mut saw_toggle = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline && !(saw_result && saw_toggle) {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
        {
            if t.contains("\"type\":\"command_result\"") {
                saw_result = true;
            }
            if t.contains("\"auto_reopen_enabled\":true") {
                saw_toggle = true;
            }
        }
    }
    assert!(saw_result, "no command_result");
    assert!(saw_toggle, "toggle not reflected in view_model");
}

/// A web client can pull latest changes for a session's worktree with the
/// `pull` command. The pull runs in a background worker, which posts a Busy
/// "Pulling…" status that the engine actor broadcasts on the status stream
/// (the synchronous `command_result` carries no status for worker-spawning
/// commands). The background pull itself fails here (the boot worktree has no
/// remote), but the busy status is what we assert reaches the client.
#[tokio::test]
async fn pull_returns_busy_status() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"command","command":"pull","args":{"session_id":"s1"}}"#.into(),
    ))
    .await
    .unwrap();

    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !found {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && (t.contains("\"type\":\"command_result\"") || t.contains("\"type\":\"status\""))
            && t.contains("Pulling")
        {
            found = true;
        }
    }

    assert!(found, "never received a Pulling busy status");
}

#[tokio::test]
async fn client_streams_pty_bytes() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model
    ws.send(Message::Text(
        r#"{"type":"subscribe","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Binary(b"echo dux-stream-marker\n".to_vec()))
        .await
        .unwrap();
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-stream-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-stream-marker"),
        "no PTY echo seen; got {} bytes",
        acc.len()
    );
}

/// Regression: subscribing twice in a row (React StrictMode double-mount / session switching)
/// must not break streaming and must not duplicate PTY output. We can't reliably count exact
/// occurrences because the shell echoes the typed input line itself, so we assert that streaming
/// still works after the re-subscribe (the marker appears) and that the connection didn't hang.
#[tokio::test]
async fn double_subscribe_does_not_break_streaming() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    // Subscribe twice in a row, simulating the double-subscribe that caused doubled output.
    ws.send(Message::Text(
        r#"{"type":"subscribe","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        r#"{"type":"subscribe","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();

    // Drive the shell to produce a unique marker via stdout (not just terminal echo).
    ws.send(Message::Binary(b"printf dux-no-dup-marker\\n\n".to_vec()))
        .await
        .unwrap();

    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-no-dup-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-no-dup-marker"),
        "streaming broke after re-subscribe; got {} bytes",
        acc.len()
    );
}

/// Subscribing now launches/resumes the REAL agent provider (no demo shell). With the
/// test provider overridden to `cat`, the launch flow spawns a streaming PTY; we prove a
/// provider was actually launched and is streaming by echoing a marker through it.
#[tokio::test]
async fn subscribe_launches_a_provider() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model
    ws.send(Message::Text(
        r#"{"type":"subscribe","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Binary(b"dux-launched-provider-marker\n".to_vec()))
        .await
        .unwrap();
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-launched-provider-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-launched-provider-marker"),
        "real provider launch did not stream; got {} bytes",
        acc.len()
    );
}

/// Companion terminals are created on demand (distinct from the agent provider) and stream
/// over the same binary input path. Create one, subscribe to it, echo a marker through it
/// (the `cat` terminal override), and assert the marker streams back.
#[tokio::test]
async fn create_and_stream_a_terminal() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"create_terminal","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();

    // Read until the terminal_created message arrives, extracting the terminal_id.
    let mut terminal_id = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && terminal_id.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"terminal_created\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse terminal_created");
            terminal_id = v["terminal_id"].as_str().expect("terminal_id").to_string();
        }
    }
    assert!(!terminal_id.is_empty(), "never received terminal_created");

    ws.send(Message::Text(format!(
        r#"{{"type":"subscribe_terminal","terminal_id":"{terminal_id}"}}"#
    )))
    .await
    .unwrap();
    ws.send(Message::Binary(b"dux-terminal-marker\n".to_vec()))
        .await
        .unwrap();

    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-terminal-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-terminal-marker"),
        "companion terminal did not stream; got {} bytes",
        acc.len()
    );
}

/// A companion terminal whose child process exits (here `cat` receiving EOF) is
/// reaped by the engine's per-tick prune, so its id disappears from the
/// ViewModel. Confirm the id is present after creation, then absent afterward.
#[tokio::test]
async fn exited_terminal_is_pruned_from_view_model() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"create_terminal","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();

    // Read until the terminal_created message arrives, extracting the terminal_id.
    let mut terminal_id = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && terminal_id.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"terminal_created\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse terminal_created");
            terminal_id = v["terminal_id"].as_str().expect("terminal_id").to_string();
        }
    }
    assert!(!terminal_id.is_empty(), "never received terminal_created");

    ws.send(Message::Text(format!(
        r#"{{"type":"subscribe_terminal","terminal_id":"{terminal_id}"}}"#
    )))
    .await
    .unwrap();

    // Ctrl-D (EOF) makes the `cat` companion terminal exit; the engine prunes it.
    ws.send(Message::Binary(b"\x04".to_vec())).await.unwrap();

    // Watch view_model frames: first one with the id, then a later one without it.
    let mut saw_with_id = false;
    let mut saw_without_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"view_model\"")
        {
            if t.contains(&terminal_id) {
                saw_with_id = true;
            } else if saw_with_id {
                saw_without_id = true;
                break;
            }
        }
    }

    assert!(
        saw_with_id,
        "view_model never contained the created terminal id {terminal_id}"
    );
    assert!(
        saw_without_id,
        "view_model still contained terminal id {terminal_id} after it exited"
    );
}

/// When a companion terminal's child exits, the engine prunes it on the next
/// tick and broadcasts a status event. Confirm a `status` message reaches the
/// client carrying the terminal-closed notice.
#[tokio::test]
async fn exited_terminal_emits_status() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"create_terminal","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();

    // Read until the terminal_created message arrives, extracting the terminal_id.
    let mut terminal_id = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && terminal_id.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"terminal_created\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse terminal_created");
            terminal_id = v["terminal_id"].as_str().expect("terminal_id").to_string();
        }
    }
    assert!(!terminal_id.is_empty(), "never received terminal_created");

    ws.send(Message::Text(format!(
        r#"{{"type":"subscribe_terminal","terminal_id":"{terminal_id}"}}"#
    )))
    .await
    .unwrap();

    // Ctrl-D (EOF) makes the `cat` companion terminal exit; the next tick prunes
    // it and broadcasts a terminal-closed status event.
    ws.send(Message::Binary(b"\x04".to_vec())).await.unwrap();

    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !found {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"status\"")
            && t.contains("closed")
        {
            found = true;
        }
    }

    assert!(found, "never received a terminal-closed status event");
}

/// A web client can close a companion terminal with the `delete_terminal`
/// command. Confirm the id is present in the ViewModel after creation, then
/// after deletion: a closed-terminal status surfaces (via `command_result` or a
/// broadcast `status` frame) and a later ViewModel no longer carries the id.
#[tokio::test]
async fn deleted_terminal_disappears_from_view_model() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"create_terminal","session_id":"s1"}"#.into(),
    ))
    .await
    .unwrap();

    // Read until the terminal_created message arrives, extracting the terminal_id.
    let mut terminal_id = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && terminal_id.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"terminal_created\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse terminal_created");
            terminal_id = v["terminal_id"].as_str().expect("terminal_id").to_string();
        }
    }
    assert!(!terminal_id.is_empty(), "never received terminal_created");

    // Confirm the terminal appears in a view_model before we delete it.
    let mut saw_with_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_with_id {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"view_model\"")
            && t.contains(&terminal_id)
        {
            saw_with_id = true;
        }
    }
    assert!(
        saw_with_id,
        "view_model never contained the created terminal id {terminal_id}"
    );

    ws.send(Message::Text(format!(
        r#"{{"type":"command","command":"delete_terminal","args":{{"terminal_id":"{terminal_id}"}}}}"#
    )))
    .await
    .unwrap();

    // A closed-terminal status should arrive (synchronously in command_result or
    // via a broadcast status frame), and a later view_model should drop the id.
    let mut saw_status = false;
    let mut saw_without_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !(saw_status && saw_without_id) {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
        {
            if (t.contains("\"type\":\"command_result\"") || t.contains("\"type\":\"status\""))
                && t.contains("Closed terminal")
            {
                saw_status = true;
            }
            if t.contains("\"view_model\"") && !t.contains(&terminal_id) {
                saw_without_id = true;
            }
        }
    }

    assert!(
        saw_status,
        "never received a closed-terminal status for {terminal_id}"
    );
    assert!(
        saw_without_id,
        "view_model still contained terminal id {terminal_id} after deletion"
    );
}

/// A web client can delete an agent session with the `delete_session` command.
/// With `delete_worktree:false` the engine takes the inline path (no git), so the
/// "Deleted agent" status returns synchronously as the command_result and a later
/// view_model no longer carries the session id.
#[tokio::test]
async fn deleted_session_inline_disappears_from_view_model() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    // Confirm the session appears in a view_model before we delete it.
    let mut saw_with_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_with_id {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"view_model\"")
            && t.contains("\"id\":\"s1\"")
        {
            saw_with_id = true;
        }
    }
    assert!(saw_with_id, "view_model never contained the session id s1");

    ws.send(Message::Text(
        r#"{"type":"command","command":"delete_session","args":{"session_id":"s1","delete_worktree":false}}"#.into(),
    ))
    .await
    .unwrap();

    // The inline finish returns the "Deleted agent" status synchronously as the
    // command_result, and a later view_model should drop the session id.
    let mut saw_status = false;
    let mut saw_without_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !(saw_status && saw_without_id) {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
        {
            if (t.contains("\"type\":\"command_result\"") || t.contains("\"type\":\"status\""))
                && t.contains("Deleted agent")
            {
                saw_status = true;
            }
            if t.contains("\"view_model\"") && !t.contains("\"id\":\"s1\"") {
                saw_without_id = true;
            }
        }
    }

    assert!(saw_status, "never received a Deleted agent status for s1");
    assert!(
        saw_without_id,
        "view_model still contained session id s1 after deletion"
    );
}

/// Updating a project's startup command over the wire persists to BOTH the
/// in-memory ViewModel (reflected back to clients) AND config.toml (the portable
/// source of truth). The engine actor writes config.toml synchronously right
/// after applying the persistence outcome.
#[tokio::test]
async fn update_project_startup_command_persists_to_config() {
    let (addr, _tmp, config_path) = boot_for_config().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    let cmd = r#"{"type":"command","command":"update_project_startup_command","args":{"project_id":"p1","startup_command":"echo hi"}}"#;
    ws.send(Message::Text(cmd.into())).await.unwrap();

    // Poll for a view_model showing the in-memory update.
    let mut saw_startup = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_startup {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
            && t.contains("\"startup_command\":\"echo hi\"")
        {
            saw_startup = true;
        }
    }
    assert!(
        saw_startup,
        "view_model never reflected the startup command"
    );

    // The config write happens synchronously in the actor; poll the file briefly
    // in case the OS write lands just after the view_model frame.
    let mut config_has_value = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && !config_has_value {
        if let Ok(saved) = std::fs::read_to_string(&config_path)
            && saved.contains("echo hi")
        {
            config_has_value = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        config_has_value,
        "config.toml never received the startup command: {saved}"
    );
}

/// Reloading config re-reads config.toml from disk and APPLIES it to the running
/// engine: a "Configuration reloaded" status reaches the client, and a value
/// changed on disk after boot (here `defaults.provider`) becomes observable in a
/// later ViewModel frame (project p1 inherits the new global default provider).
#[tokio::test]
async fn reload_config_reapplies_and_reports() {
    let (addr, _tmp, config_path) = boot_for_config().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    // Boot's config has no [defaults] override, so p1 inherits the built-in
    // default provider ("claude"). Rewrite config.toml on disk with a different
    // global default before asking the engine to reload.
    std::fs::write(&config_path, "[defaults]\nprovider = \"codex\"\n").unwrap();

    ws.send(Message::Text(
        r#"{"type":"command","command":"reload_config","args":{}}"#.into(),
    ))
    .await
    .unwrap();

    // Look for BOTH the reload status and a view_model showing the applied change.
    let mut saw_status = false;
    let mut saw_applied = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_status && saw_applied) {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
        {
            if (t.contains("\"type\":\"command_result\"") || t.contains("\"type\":\"status\""))
                && t.contains("Configuration reloaded")
            {
                saw_status = true;
            }
            if t.contains("\"type\":\"view_model\"")
                && t.contains("\"id\":\"p1\"")
                && t.contains("\"default_provider\":\"codex\"")
            {
                saw_applied = true;
            }
        }
    }

    assert!(saw_status, "never received a Configuration reloaded status");
    assert!(
        saw_applied,
        "view_model never reflected the reloaded default provider"
    );
}

/// Create a real git repo (init + commit) at a fresh temp dir, returning it.
fn init_repo_with_commit() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.txt"), "hello\n").expect("write file");
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

/// Browsing a server-side directory returns a `dir_entries` frame listing its
/// subdirectories (git repos and plain dirs alike) plus the leading `../`
/// parent entry. A child dir is created so the listing is deterministic and the
/// path is passed explicitly so the test doesn't depend on `$HOME`.
#[tokio::test]
async fn browse_dir_lists_entries() {
    let (addr, _tmp) = boot().await;
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir(parent.path().join("child-dir")).unwrap();
    let parent_path = parent.path().to_string_lossy().to_string();

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(format!(
        r#"{{"type":"browse_dir","path":"{parent_path}"}}"#
    )))
    .await
    .unwrap();

    let mut frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && frame.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"dir_entries\"")
        {
            frame = t.to_string();
        }
    }
    assert!(!frame.is_empty(), "never received a dir_entries frame");

    let v: serde_json::Value = serde_json::from_str(&frame).expect("parse dir_entries frame");
    assert!(v["error"].is_null(), "unexpected error: {frame}");
    let entries = v["entries"].as_array().expect("entries array");
    let labels: Vec<&str> = entries.iter().filter_map(|e| e["label"].as_str()).collect();
    assert!(labels.contains(&"../"), "missing parent entry: {labels:?}");
    assert!(
        labels.iter().any(|l| l.starts_with("child-dir")),
        "missing child dir: {labels:?}"
    );
}

/// A web client can request a freshly generated agent name. Generation is pure
/// (no engine thread), so the reply is an `agent_name` frame carrying a two-word
/// dashed name made of ASCII alphanumerics and dashes.
#[tokio::test]
async fn generate_agent_name_returns_a_pet_name() {
    let (addr, _tmp) = boot().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(r#"{"type":"generate_agent_name"}"#.into()))
        .await
        .unwrap();

    let mut frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && frame.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"agent_name\"")
        {
            frame = t.to_string();
        }
    }
    assert!(!frame.is_empty(), "never received an agent_name frame");

    let v: serde_json::Value = serde_json::from_str(&frame).expect("parse agent_name frame");
    let name = v["name"].as_str().expect("name string");
    assert!(!name.is_empty(), "name should be non-empty: {frame}");
    assert!(
        name.split('-').count() >= 2,
        "expected a two-word dashed name, got {name:?}"
    );
    assert!(
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "name should be ascii alphanumeric/dash only, got {name:?}"
    );
}

/// A web client can register an existing git repo as a project with the
/// `add_project` command, see it appear in the ViewModel, then remove it with
/// `remove_project`. The actor syncs config.toml after each persistence; the
/// ViewModel assertions are the core of the test.
#[tokio::test]
async fn add_and_remove_project() {
    let (addr, _tmp, _config_path) = boot_for_config().await;
    let repo = init_repo_with_commit();
    let repo_path = repo.path().to_string_lossy().to_string();

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(format!(
        r#"{{"type":"command","command":"add_project","args":{{"path":"{repo_path}","name":"webproj"}}}}"#
    )))
    .await
    .unwrap();

    // Poll for a view_model whose projects include "webproj"; capture its id.
    let mut project_id = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && project_id.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
            && t.contains("\"webproj\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse view_model");
            if let Some(projects) = v["data"]["projects"].as_array()
                && let Some(p) = projects
                    .iter()
                    .find(|p| p["name"].as_str() == Some("webproj"))
            {
                project_id = p["id"].as_str().expect("project id").to_string();
            }
        }
    }
    assert!(
        !project_id.is_empty(),
        "view_model never contained the added project webproj"
    );

    ws.send(Message::Text(format!(
        r#"{{"type":"command","command":"remove_project","args":{{"project_id":"{project_id}"}}}}"#
    )))
    .await
    .unwrap();

    // Poll for a view_model whose projects no longer include that id.
    let mut saw_without_id = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && !saw_without_id {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse view_model");
            if let Some(projects) = v["data"]["projects"].as_array()
                && !projects
                    .iter()
                    .any(|p| p["id"].as_str() == Some(project_id.as_str()))
            {
                saw_without_id = true;
            }
        }
    }
    assert!(
        saw_without_id,
        "view_model still contained project id {project_id} after removal"
    );
}

/// A web client can create a new agent in a project with the `create_agent`
/// command. The create worker does real git worktree creation (off the project's
/// committed branch) and spawns the `cat` provider; the resulting session and
/// provider reach the web via the existing worker-drain path, so a later
/// ViewModel carries a new session under p1 whose branch name is "webagent".
#[tokio::test]
async fn create_agent_adds_a_session() {
    let (addr, _tmp) = boot_for_create_agent().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    // Drain the initial view_model and record p1's existing session ids (likely none).
    let mut initial_ids: Vec<String> = Vec::new();
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(m))) =
                tokio::time::timeout(Duration::from_millis(300), ws.next()).await
                && let Ok(t) = m.into_text()
                && t.contains("\"type\":\"view_model\"")
            {
                let v: serde_json::Value = serde_json::from_str(&t).expect("parse view_model");
                if let Some(sessions) = v["data"]["sessions"].as_array() {
                    initial_ids = sessions
                        .iter()
                        .filter(|s| s["project_id"].as_str() == Some("p1"))
                        .filter_map(|s| s["id"].as_str().map(|id| id.to_string()))
                        .collect();
                }
                break;
            }
        }
    }

    ws.send(Message::Text(
        r#"{"type":"command","command":"create_agent","args":{"project_id":"p1","name":"webagent"}}"#
            .into(),
    ))
    .await
    .unwrap();

    // Poll for a view_model with a NEW session under p1 whose branch name carries
    // "webagent". The create worker does real git work then spawns `cat`, so give
    // it a generous deadline.
    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !found {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("parse view_model");
            if let Some(sessions) = v["data"]["sessions"].as_array()
                && sessions.iter().any(|s| {
                    s["project_id"].as_str() == Some("p1")
                        && s["id"]
                            .as_str()
                            .map(|id| !initial_ids.iter().any(|prev| prev == id))
                            .unwrap_or(false)
                        && s["branch_name"]
                            .as_str()
                            .map(|b| b.contains("webagent"))
                            .unwrap_or(false)
                })
            {
                found = true;
            }
        }
    }

    assert!(
        found,
        "view_model never contained a new p1 session with branch 'webagent'"
    );
}

/// Like `boot()`, but project `p1`'s path is a REAL git repo with a committed
/// branch, and a managed worktree is added under the dux worktrees root
/// (`<root>/worktrees/<project_name>/orphan`) so `classify_project_worktrees`
/// returns a genuine adoptable managed entry. No session is seeded, so the
/// worktree is orphaned (adoptable).
async fn boot_for_worktree_listing() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let run = |cwd: &std::path::Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed in {}", cwd.display());
    };
    run(&repo, &["init", "-q"]);
    run(&repo, &["config", "user.email", "t@example.com"]);
    run(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f.txt"), "line1\n").expect("write file");
    run(&repo, &["add", "f.txt"]);
    run(&repo, &["commit", "-q", "-m", "init"]);

    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    };
    // Managed worktree under <worktrees_root>/<project name "p1-name">/orphan.
    let managed_root = paths.worktrees_root.join("p1-name");
    std::fs::create_dir_all(&managed_root).unwrap();
    let worktree_path = managed_root.join("orphan");
    run(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "orphan",
            worktree_path.to_string_lossy().as_ref(),
        ],
    );
    {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: repo.to_string_lossy().into_owned(),
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
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
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

/// A web client can list a project's managed worktrees with
/// `list_project_worktrees`. The server resolves the project, classifies the
/// worktrees in spawn_blocking (real `git worktree list`), and replies with a
/// `project_worktrees` frame carrying the orphaned managed worktree as an
/// adoptable entry (branch "orphan", no agent yet).
#[tokio::test]
async fn list_project_worktrees_returns_adoptable_entry() {
    let (addr, _tmp) = boot_for_worktree_listing().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let _ = ws.next().await; // initial view_model

    ws.send(Message::Text(
        r#"{"type":"list_project_worktrees","project_id":"p1"}"#.into(),
    ))
    .await
    .unwrap();

    let mut frame = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while tokio::time::Instant::now() < deadline && frame.is_empty() {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"project_worktrees\"")
        {
            frame = t.to_string();
        }
    }
    assert!(
        !frame.is_empty(),
        "never received a project_worktrees frame"
    );

    let v: serde_json::Value = serde_json::from_str(&frame).expect("parse project_worktrees frame");
    assert!(v["error"].is_null(), "unexpected error: {frame}");
    assert_eq!(v["project_id"].as_str(), Some("p1"));
    let entries = v["entries"].as_array().expect("entries array");
    let orphan = entries
        .iter()
        .find(|e| e["branch_name"].as_str() == Some("orphan"))
        .unwrap_or_else(|| panic!("missing orphan worktree entry: {frame}"));
    assert_eq!(orphan["adoptable"].as_bool(), Some(true), "{frame}");
    assert!(orphan["reason"].is_null(), "{frame}");
}
