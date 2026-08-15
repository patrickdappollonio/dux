//! End-to-end tests for the pushed workspace document.
//!
//! The workspace document (projects, sessions, terminals, sidebar) is still
//! fetchable at `GET /api/v1/workspace`, but the server now also PUSHES it over
//! `/ws/events` as a `workspace` frame whenever the engine rebuilds its cached
//! serialization. These tests pin the four things that push has to get right on
//! the wire: a subscriber is handed the current document immediately, a mutation
//! reaches every subscriber with a higher `rev`, a connection that never
//! subscribed to the coarse topics is handed nothing, and the fetched body
//! carries the same `rev` so a client can order a fetch against a push.

use std::net::SocketAddr;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn sample_session(id: &str, project_id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: format!("{id}-branch"),
        initial_branch: format!("{id}-branch"),
        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
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

/// Boot a server with one project and one session, both companion terminals and
/// the provider overridden to `cat` so a create actually spawns something.
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
            .upsert_session(&sample_session("s1", "p1", root.to_string_lossy().as_ref()))
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

type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Read text frames until one satisfies `pred` or `secs` elapse.
async fn wait_for_frame(
    ws: &mut ClientWs,
    secs: u64,
    pred: impl Fn(&str) -> bool,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(t) = m.into_text()
            && pred(t.as_str())
        {
            return Some(t.to_string());
        }
    }
    None
}

/// Connect a `/ws/events` client and consume its `connected` handshake.
async fn connect_events(addr: SocketAddr) -> ClientWs {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    wait_for_frame(&mut ws, 5, |t| t.contains("\"event\":\"connected\""))
        .await
        .expect("a /ws/events connection must send a `connected` first frame");
    ws
}

/// Wait for a `workspace` push frame and return it parsed.
async fn next_workspace_frame(ws: &mut ClientWs, secs: u64) -> Option<serde_json::Value> {
    let text = wait_for_frame(ws, secs, |t| t.contains("\"event\":\"workspace\"")).await?;
    Some(serde_json::from_str(&text).expect("a workspace frame must be JSON"))
}

/// Create a companion terminal over REST: a spine mutation with a visible
/// consequence in the document (a new entry in the flat `terminals` collection).
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

/// Subscribing to a coarse topic hands the client the CURRENT document at once.
/// Without this replay a client would sit on whatever its boot fetch returned
/// until the next change, and the whole point of the push is that it does not
/// have to fetch at all after boot.
#[tokio::test]
async fn subscribing_replays_the_current_workspace_document() {
    let (addr, _tmp) = boot().await;
    let mut ws = connect_events(addr).await;
    ws.send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();

    let frame = next_workspace_frame(&mut ws, 5)
        .await
        .expect("subscribing to `sessions` must replay the current workspace document");
    assert!(
        frame["rev"].as_u64().unwrap_or(0) >= 1,
        "the replayed frame must carry a real rev: {frame}"
    );
    let doc = &frame["workspace"];
    assert!(
        doc["sessions"]
            .as_array()
            .expect("the pushed document carries a sessions array")
            .iter()
            .any(|s| s["id"] == "s1"),
        "the pushed document must be the real workspace: {doc}"
    );
    assert_eq!(
        doc["rev"].as_u64(),
        frame["rev"].as_u64(),
        "the rev is embedded in the document itself, so a fetched body and a \
         pushed frame are orderable against each other"
    );
}

/// A mutation reaches EVERY subscribed connection with a higher rev. Two
/// connections, because the whole reason for the push is that N tabs no longer
/// each pull the same document.
#[tokio::test]
async fn a_mutation_pushes_a_higher_rev_to_every_subscribed_connection() {
    let (addr, _tmp) = boot().await;
    let mut a = connect_events(addr).await;
    let mut b = connect_events(addr).await;
    for ws in [&mut a, &mut b] {
        ws.send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
            .await
            .unwrap();
    }
    let a_first = next_workspace_frame(&mut a, 5).await.expect("A replay");
    let b_first = next_workspace_frame(&mut b, 5).await.expect("B replay");
    let base_rev = a_first["rev"].as_u64().unwrap();
    assert_eq!(base_rev, b_first["rev"].as_u64().unwrap());

    let terminal_id = create_terminal_via_rest(addr, "s1").await;

    for (name, ws) in [("A", &mut a), ("B", &mut b)] {
        let frame = loop {
            let frame = next_workspace_frame(ws, 5)
                .await
                .unwrap_or_else(|| panic!("{name} must be pushed the changed document"));
            if frame["rev"].as_u64().unwrap() > base_rev {
                break frame;
            }
        };
        assert!(
            frame["workspace"]["terminals"]
                .as_array()
                .expect("terminals array")
                .iter()
                .any(|t| t["id"] == terminal_id.as_str()),
            "{name}'s pushed document must contain the new terminal: {frame}"
        );
    }
}

/// A connection that holds neither coarse topic is handed nothing. The push is
/// filtered per connection exactly like every other event on this socket.
#[tokio::test]
async fn an_unsubscribed_connection_is_pushed_nothing() {
    let (addr, _tmp) = boot().await;
    let mut subscriber = connect_events(addr).await;
    let mut bystander = connect_events(addr).await;
    subscriber
        .send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();
    let base = next_workspace_frame(&mut subscriber, 5)
        .await
        .expect("the subscriber gets its replay");

    create_terminal_via_rest(addr, "s1").await;

    // The subscriber's push is the synchronization point: once it has landed,
    // the bystander's silence is a fact rather than a race with the engine.
    let pushed = loop {
        let frame = next_workspace_frame(&mut subscriber, 5)
            .await
            .expect("the subscriber must be pushed the change");
        if frame["rev"].as_u64().unwrap() > base["rev"].as_u64().unwrap() {
            break frame;
        }
    };
    assert!(pushed["rev"].as_u64().unwrap() > base["rev"].as_u64().unwrap());
    assert!(
        next_workspace_frame(&mut bystander, 1).await.is_none(),
        "a connection holding no coarse topic must be pushed no workspace document"
    );
}

/// The REST read carries the same rev the push carries, so a boot fetch and a
/// subscribe replay landing in either order can be ordered by the client.
#[tokio::test]
async fn the_rest_read_carries_the_same_rev_as_the_push() {
    let (addr, _tmp) = boot().await;
    let mut ws = connect_events(addr).await;
    ws.send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();
    let frame = next_workspace_frame(&mut ws, 5).await.expect("replay");

    let fetched: serde_json::Value = reqwest::get(format!("http://{addr}/api/v1/workspace"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        fetched["rev"].as_u64(),
        frame["rev"].as_u64(),
        "the fetched body and the pushed frame describe the same revision"
    );
    assert_eq!(
        fetched["sessions"], frame["workspace"]["sessions"],
        "and they are the same document, from the one cached serialization"
    );
}
