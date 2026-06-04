//! End-to-end tests for the in-process flip entry point `serve_with_engine`:
//! it serves an EXISTING engine on the caller's thread and returns that same
//! engine when the status-screen tick asks to stop. These complement the
//! dedicated-thread `ws_transport` suite (which proves `spawn_engine_thread`
//! still behaves byte-identically after the shared-loop refactor).

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::engine::Engine;
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::{ServerExit, ServerTick, serve_with_engine};
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

/// Build a minimal engine seeded with project `p1` and session `s1`, with the
/// `claude` provider and companion terminal both overridden to `cat` so any
/// spawned PTY is a runnable echo program in CI. Returns the engine plus the
/// temp dir that must outlive it.
fn build_engine() -> (Engine, tempfile::TempDir) {
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
    (engine, tmp)
}

/// A ws client connects and receives a `view_model` frame, then a programmatic
/// `ReturnToTui` tick stops serving: the engine comes back (proved by the
/// returned `ServerExit::ReturnToTui` and a surviving in-memory terminal), and
/// the port can be re-bound (the server is actually down and released it).
#[tokio::test]
async fn serve_with_engine_returns_to_tui_and_closes_the_port() {
    let (mut engine, _tmp) = build_engine();

    // Create a live companion terminal BEFORE serving so we can prove the
    // ReturnToTui path preserves running PTYs (the engine is never dropped).
    let (terminal_id, _label) = engine
        .create_companion_terminal("s1")
        .expect("create terminal");
    assert!(
        engine.companion_terminals.contains_key(&terminal_id),
        "terminal should exist before serving"
    );

    // Bind the std listener in the test (this is what the TUI pre-flight does)
    // and hand it through the flip.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    // The test toggles this once it has confirmed a view_model frame; the
    // serve thread's tick reads it to return ReturnToTui.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let terminal_id_for_thread = terminal_id.clone();

    // serve_with_engine runs the engine loop on THIS thread, so it must run on a
    // dedicated thread (the engine is `!Send`, hence we build it here too). The
    // thread reports back only `Send` values: the exit reason plus a flag for
    // whether the live terminal survived the round-trip.
    let (result_tx, result_rx) = std::sync::mpsc::channel::<(ServerExit, bool)>();
    let serve_thread = std::thread::spawn(move || {
        let (returned_engine, exit) = serve_with_engine(engine, listener, || {
            if stop_for_thread.load(Ordering::SeqCst) {
                ServerTick::ReturnToTui
            } else {
                ServerTick::Continue
            }
        })
        .expect("serve_with_engine");

        // The SAME engine came back: the terminal we created is still live and
        // still writable (PTYs untouched by ReturnToTui).
        let terminal = returned_engine
            .companion_terminals
            .get(&terminal_id_for_thread);
        let survived = match terminal {
            Some(t) => !t.client.is_exited() && t.client.write_bytes(b"ping\n").is_ok(),
            None => false,
        };
        result_tx.send((exit, survived)).unwrap();
    });

    // Connect a ws client and confirm a view_model frame arrives.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    let mut saw_view_model = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_view_model {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
            && t.contains("\"id\":\"s1\"")
        {
            saw_view_model = true;
        }
    }
    assert!(saw_view_model, "never received a view_model frame");

    // Ask the status-screen tick to flip back to the TUI.
    stop.store(true, Ordering::SeqCst);

    // The serve thread should return promptly with the engine intact.
    let (exit, survived) = result_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("serve thread reported a result");
    assert!(
        matches!(exit, ServerExit::ReturnToTui),
        "expected ReturnToTui exit"
    );
    assert!(
        survived,
        "the live companion terminal did not survive the flip"
    );

    serve_thread.join().expect("serve thread joined");

    // The server is actually down: re-binding the same port must succeed (the
    // listener was released when serving stopped).
    let reconnect = TcpListener::bind(addr);
    assert!(
        reconnect.is_ok(),
        "port {addr} should be free again after the server stopped"
    );
}

/// A `QuitProcess` tick stops serving and runs the quit teardown
/// (`shutdown_ptys`), so a live agent/terminal child is SIGTERMed. We assert
/// the exit reason is `QuitProcess` and the previously-live terminal is now
/// marked exited on the returned engine.
#[tokio::test]
async fn serve_with_engine_quit_process_shuts_down_ptys() {
    let (mut engine, _tmp) = build_engine();
    let (terminal_id, _label) = engine
        .create_companion_terminal("s1")
        .expect("create terminal");
    assert!(!engine.companion_terminals[&terminal_id].client.is_exited());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let quit = Arc::new(AtomicBool::new(false));
    let quit_for_thread = Arc::clone(&quit);
    let terminal_id_for_thread = terminal_id.clone();

    let (result_tx, result_rx) = std::sync::mpsc::channel::<(ServerExit, bool)>();
    let serve_thread = std::thread::spawn(move || {
        let (returned_engine, exit) = serve_with_engine(engine, listener, || {
            if quit_for_thread.load(Ordering::SeqCst) {
                ServerTick::QuitProcess
            } else {
                ServerTick::Continue
            }
        })
        .expect("serve_with_engine");
        // After QuitProcess teardown the child should have been SIGTERMed; the
        // terminal entry stays in the map but its PTY child is gone.
        let exited = returned_engine
            .companion_terminals
            .get(&terminal_id_for_thread)
            .map(|t| t.client.is_exited())
            .unwrap_or(true);
        result_tx.send((exit, exited)).unwrap();
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    let _ = tokio::time::timeout(Duration::from_secs(3), ws.next()).await;

    quit.store(true, Ordering::SeqCst);

    let (exit, exited) = result_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("serve thread reported a result");
    assert!(
        matches!(exit, ServerExit::QuitProcess),
        "expected QuitProcess exit"
    );
    assert!(exited, "QuitProcess should have shut down the PTY child");

    serve_thread.join().expect("serve thread joined");
}

/// Regression for the flip hang: a subscribed PTY forwarder must not wedge the
/// runtime teardown on ReturnToTui. A ws client subscribes to a live `cat`
/// companion terminal and gets an echoed binary frame (so the forwarder is live
/// and parked on its blocking `recv_timeout`, with the engine — and thus the
/// PtyClient `Sender` — still alive). Then a `ReturnToTui` tick stops serving,
/// and the serve thread must JOIN within a tight bound. Before the fix the
/// forwarder parked on `recv()` forever (the Sender never dropped on
/// ReturnToTui), so dropping the multi-thread runtime hung indefinitely.
#[tokio::test]
async fn return_to_tui_does_not_hang_with_a_subscribed_pty() {
    let (mut engine, _tmp) = build_engine();

    // A live `cat`-backed companion terminal: writing to it echoes back, which
    // gives us a forwarder binary frame to await.
    let (terminal_id, _label) = engine
        .create_companion_terminal("s1")
        .expect("create terminal");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let terminal_id_for_thread = terminal_id.clone();

    let (result_tx, result_rx) = std::sync::mpsc::channel::<(ServerExit, bool)>();
    let serve_thread = std::thread::spawn(move || {
        let (returned_engine, exit) = serve_with_engine(engine, listener, || {
            if stop_for_thread.load(Ordering::SeqCst) {
                ServerTick::ReturnToTui
            } else {
                ServerTick::Continue
            }
        })
        .expect("serve_with_engine");
        // ReturnToTui keeps PTYs alive: the terminal must still be running.
        let alive = returned_engine
            .companion_terminals
            .get(&terminal_id_for_thread)
            .map(|t| !t.client.is_exited())
            .unwrap_or(false);
        result_tx.send((exit, alive)).unwrap();
    });

    // Subscribe to the companion terminal and prove the forwarder is live by
    // writing input and awaiting the `cat` echo as a binary frame.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    let _ = ws.next().await; // initial view_model
    ws.send(Message::Text(format!(
        r#"{{"type":"subscribe_terminal","terminal_id":"{terminal_id}"}}"#
    )))
    .await
    .expect("send subscribe_terminal");
    ws.send(Message::Binary(b"dux-flip-marker\n".to_vec()))
        .await
        .expect("send pty input");

    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains("dux-flip-marker") {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("dux-flip-marker"),
        "forwarder never streamed the echoed PTY bytes; got {} bytes",
        acc.len()
    );

    // Flip back to the TUI. The serve thread must join PROMPTLY despite the
    // parked forwarder. Measure the latency for the report; the outer recv
    // timeout is the real guard against an infinite hang.
    let flip_started = std::time::Instant::now();
    stop.store(true, Ordering::SeqCst);

    let (exit, alive) = result_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("serve thread must join (it hung before the interruptible-forwarder fix)");
    let flip_latency = flip_started.elapsed();
    eprintln!("ReturnToTui flip latency with a subscribed PTY: {flip_latency:?}");

    assert!(
        matches!(exit, ServerExit::ReturnToTui),
        "expected ReturnToTui exit"
    );
    assert!(alive, "ReturnToTui must keep the companion terminal alive");
    assert!(
        flip_latency < Duration::from_secs(5),
        "flip took too long ({flip_latency:?}); the forwarder likely wedged the runtime"
    );

    serve_thread.join().expect("serve thread joined");
}
