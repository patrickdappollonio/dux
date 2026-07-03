//! End-to-end tests for the agent-tab REST routes: create/close/retarget under
//! `/api/v1/sessions/:id/tabs`, the Main-tab detach special-case, Support-tab
//! ownership (cross-session 404), and provider validation.

use std::net::SocketAddr;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let n = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        project_id: "p1".to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: format!("{id}-branch"),
        worktree_path: worktree.to_string(),
        title: None,
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: n,
        updated_at: n,
    }
}

/// Boot a server (auth OFF) with two sessions (`s1`, `s2`) in project `p1`. The
/// provider `claude` is overridden to `cat` (a runnable program that holds its
/// stdin open) so a created tab's async launch succeeds and its row persists for
/// the assertions that follow.
async fn boot() -> (SocketAddr, tempfile::TempDir) {
    boot_with_tab_per_agent(dux_core::config::DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT).await
}

async fn boot_with_tab_per_agent(tab_per_agent: u32) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt1 = root.join("wt1");
    let wt2 = root.join("wt2");
    std::fs::create_dir_all(&wt1).unwrap();
    std::fs::create_dir_all(&wt2).unwrap();

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
            .upsert_session(&sample_session("s1", wt1.to_string_lossy().as_ref()))
            .unwrap();
        store
            .upsert_session(&sample_session("s2", wt2.to_string_lossy().as_ref()))
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
    let params = RouterParams::plain_http().with_max_websocket_connections(
        dux_core::config::DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
        dux_core::config::DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
        dux_core::config::DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
        dux_core::config::DEFAULT_MAX_WEBSOCKET_TAB_CONNECTIONS,
        tab_per_agent,
    );
    let app = build_app(handle, Router::<AppState>::new(), params);
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

async fn create_support_tab(client: &reqwest::Client, addr: SocketAddr, session: &str) -> String {
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/{session}/tabs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "create tab should 201");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["tab_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn post_tabs_creates_and_returns_an_attachable_id() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/tabs"))
        .json(&serde_json::json!({ "provider": "claude" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["tab_id"].as_str().unwrap().is_empty());
    assert_eq!(body["provider"], "claude");
}

#[tokio::test]
async fn post_tabs_rejects_an_unconfigured_provider() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/tabs"))
        .json(&serde_json::json!({ "provider": "bogus-provider" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn delete_main_tab_detaches_and_keeps_the_session() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    // A Support tab exists alongside Main; the Main detach must not close it.
    let support = create_support_tab(&client, addr, "s1").await;

    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/s1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["detached"], true);

    // The session still exists and its Support tab survived the Main detach.
    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tab_ids: Vec<&str> = session["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(tab_ids.contains(&support.as_str()));
}

#[tokio::test]
async fn delete_support_tab_removes_its_row() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;

    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/{tab}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tab_ids: Vec<&str> = session["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(!tab_ids.contains(&tab.as_str()));
}

#[tokio::test]
async fn cross_session_tab_delete_is_404() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;
    // The tab belongs to s1; deleting it under s2 must 404 (never cross-session).
    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s2/tabs/{tab}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn patch_tab_rejects_an_unconfigured_provider() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;
    let resp = client
        .patch(format!("http://{addr}/api/v1/sessions/s1/tabs/{tab}"))
        .json(&serde_json::json!({ "provider": "bogus-provider" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── WebSocket route: ownership + per-agent socket cap ────────────────────────

/// A Support-tab PTY socket under the WRONG session id is rejected before the
/// upgrade, while the owning session connects — the WS counterpart to the REST
/// `cross_session_tab_delete_is_404` check.
#[tokio::test]
async fn nested_tab_pty_socket_enforces_session_ownership() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;

    let owning =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/tabs/{tab}/pty"))
            .await;
    assert!(
        owning.is_ok(),
        "the owning session's tab socket should connect"
    );
    drop(owning);

    let foreign =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s2/tabs/{tab}/pty"))
            .await;
    assert!(
        foreign.is_err(),
        "a tab addressed under the wrong session must be rejected before upgrade"
    );
}

#[tokio::test]
async fn tab_pty_socket_cap_refuses_beyond_the_per_agent_limit() {
    // Per-agent cap of one live tab socket.
    let (addr, _tmp) = boot_with_tab_per_agent(1).await;
    let client = reqwest::Client::new();
    let tab1 = create_support_tab(&client, addr, "s1").await;
    let tab2 = create_support_tab(&client, addr, "s1").await;

    // The first tab socket for agent s1 connects and is held open.
    let sock1 =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/tabs/{tab1}/pty"))
            .await;
    assert!(
        sock1.is_ok(),
        "first tab socket for the agent should connect"
    );
    let sock1 = sock1.unwrap().0;

    // A second concurrent tab socket for the SAME agent exceeds the per-agent
    // sub-quota and is refused before upgrade (503 -> connect_async errors).
    let sock2 =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/tabs/{tab2}/pty"))
            .await;
    assert!(
        sock2.is_err(),
        "a second concurrent tab socket for the same agent must be refused at the per-agent cap"
    );

    // Closing the first frees the agent's slot; a new tab socket then succeeds,
    // proving the guard's Drop decrement runs. Retry briefly for the async close.
    drop(sock1);
    let mut reconnected = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/tabs/{tab2}/pty"))
            .await
            .is_ok()
        {
            reconnected = true;
            break;
        }
    }
    assert!(
        reconnected,
        "after closing the first tab socket the freed slot should allow a new one"
    );
}
