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
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp)
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
