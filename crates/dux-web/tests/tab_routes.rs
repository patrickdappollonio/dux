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
        initial_branch: format!("{id}-branch"),
        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
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

/// Like `boot()`, but also configures a `"broken"` provider whose command is a
/// nonexistent binary, so a tab created against it fails its async launch
/// instead of coming up live. Used by the G-T2 async-launch-failure test.
async fn boot_with_broken_provider() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt1 = root.join("wt1");
    std::fs::create_dir_all(&wt1).unwrap();

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
    engine.config.providers.commands.insert(
        "broken".to_string(),
        ProviderCommandConfig {
            command: "/nonexistent/dux-test-broken-provider-binary".to_string(),
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
        dux_core::config::DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT,
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

/// Poll `/api/v1/sessions/:id` until `pred` matches the decoded body, or give
/// up after ~5s. Used to wait out the async tab-launch job before asserting on
/// its liveness, rather than racing it.
async fn wait_for_session<F>(
    client: &reqwest::Client,
    addr: SocketAddr,
    id: &str,
    pred: F,
) -> serde_json::Value
where
    F: Fn(&serde_json::Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let session: serde_json::Value = client
            .get(format!("http://{addr}/api/v1/sessions/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if pred(&session) || tokio::time::Instant::now() >= deadline {
            return session;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn tab_has_live_process(session: &serde_json::Value, tab_id: &str) -> bool {
    session["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"].as_str() == Some(tab_id) && t["has_live_process"] == true)
}

/// The drop-paste profile a tab launched with rides the SPINE, per tab, and
/// retires with the process.
///
/// It used to ride the BOOTSTRAP document, which a browser refetches only on a
/// config change, so a client's copy went stale for the whole life of a process:
/// a tab relaunched under a different provider was still quoted for the previous
/// one until a reconnect. A launch and a termination both refresh the spine, so
/// that is where it belongs, and this drives a REAL launch and a REAL close to
/// say so.
///
/// It also pins the identity half. This harness's `[providers.claude]` block runs
/// the command `cat`, and the published profile must say `cat`: the block NAME is
/// free text and the COMMAND is what says which CLI is reading the paste.
#[tokio::test]
async fn a_launched_tab_publishes_its_drop_paste_profile_on_the_spine() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let support = create_support_tab(&client, addr, "s1").await;
    let session =
        wait_for_session(&client, addr, "s1", |s| tab_has_live_process(s, &support)).await;

    let tab = session["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(&support))
        .unwrap_or_else(|| panic!("the launched tab is missing from the spine: {session}"));
    assert_eq!(
        tab["drop_paste"]["form"], "bare",
        "a live tab publishes the form it launched with: {session}"
    );
    assert_eq!(
        tab["drop_paste"]["command_name"], "cat",
        "the CLI is named by the COMMAND the block runs, not by the block's own \
         name (this provider is called claude and runs cat): {session}"
    );

    // The config-derived fallback says the same thing about the command, and it
    // is keyed by the block name because that is what a tab with nothing live
    // has to look itself up by.
    let bootstrap: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/bootstrap"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        bootstrap["provider_drop_paste"]["claude"]["command_name"], "cat",
        "config publishes the command's file name, not the block name: {bootstrap}"
    );
    assert!(
        bootstrap.get("tab_web_dragdrop_paste").is_none(),
        "the per-tab map must not be back on the bootstrap document: {bootstrap}"
    );

    // Close the tab. The process goes, and so does the profile.
    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/{support}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let session = wait_for_session(&client, addr, "s1", |s| {
        !s["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"].as_str() == Some(&support))
    })
    .await;
    assert!(
        !session["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"].as_str() == Some(&support)),
        "the closed tab is gone from the spine, and its profile with it: {session}"
    );
}

#[tokio::test]
async fn delete_main_tab_detaches_when_no_other_tab_is_live() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/s1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["detached"], true);
}

/// F9 regression: `KillSessionPty` on the session-slot tab detaches the agent
/// only when it was the LAST live tab. With a live Support-tab sibling, the
/// agent stays Active and `detached` must be false, not the old hardcoded
/// `true`.
#[tokio::test]
async fn delete_main_tab_with_live_sibling_does_not_detach() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    // A Support tab exists alongside Main; the Main detach must not close it.
    let support = create_support_tab(&client, addr, "s1").await;
    // Wait for the async tab-launch job to actually spawn the sibling's PTY,
    // so the assertion below reflects a truly live sibling, not a race.
    let session =
        wait_for_session(&client, addr, "s1", |s| tab_has_live_process(s, &support)).await;
    assert!(
        tab_has_live_process(&session, &support),
        "support tab never came up live; got {session}"
    );

    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/s1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["detached"], false,
        "closing Main while a sibling tab is live must not report detached"
    );

    // The session still exists and its Support tab survived the Main close,
    // still with a live process.
    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        tab_has_live_process(&session, &support),
        "the live sibling tab must survive the Main close: {session}"
    );
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
    // G15: an extra-tab close now returns 200 + `{ "detached": bool }` (matching
    // the session-slot branch) instead of a bare 204, so a caller can learn
    // whether this close detached the agent without a follow-up poll. Here it
    // was the agent's only live tab, so it must report detached.
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["detached"], true);

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
async fn delete_support_tab_with_live_sibling_does_not_detach() {
    // G15 companion case: closing an extra tab while a sibling (here, the
    // session-slot tab) is still live must report `detached: false`, not just
    // default to `true` because the closed tab itself is gone.
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;
    // Launch the session-slot tab too, so a live sibling remains after the
    // extra tab closes.
    let launch_resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/reconnect"))
        .json(&serde_json::json!({ "force": false }))
        .send()
        .await
        .unwrap();
    assert!(
        launch_resp.status().is_success(),
        "reconnect should launch the session-slot tab: {}",
        launch_resp.status()
    );
    wait_for_session(&client, addr, "s1", |s| tab_has_live_process(s, "s1")).await;

    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/{tab}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["detached"], false,
        "the session-slot tab is still live, so the agent must not detach"
    );
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

// G-T7: PATCH retarget only had a negative (unconfigured-provider) test; add the
// success path. `codex` is a default-configured provider. Retargeting a tab
// while its process is still live only PINS the previous provider for display
// (the pane title must not lie about what's on screen until relaunch), so the
// tab is killed (dormant, no live process) first — only then does the tab
// view's `provider` field reflect the persisted retarget directly.
#[tokio::test]
async fn patch_tab_retargets_to_a_valid_provider() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;

    // Wait for the tab's async launch to come up, then detach the whole agent
    // (kills every tab's process but keeps the `agent_tabs` rows) so the tab is
    // dormant before retargeting.
    wait_for_session(&client, addr, "s1", |s| tab_has_live_process(s, &tab)).await;
    let kill_resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/kill"))
        .send()
        .await
        .unwrap();
    assert_eq!(kill_resp.status(), 200);
    wait_for_session(&client, addr, "s1", |s| !tab_has_live_process(s, &tab)).await;

    let resp = client
        .patch(format!("http://{addr}/api/v1/sessions/s1/tabs/{tab}"))
        .json(&serde_json::json!({ "provider": "codex" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let retargeted = session["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(tab.as_str()))
        .expect("retargeted tab must still be present");
    assert_eq!(
        retargeted["provider"], "codex",
        "a dormant tab's retarget must be reflected directly (no live-process pin)"
    );
}

// G21: an out-of-bound `:id` must be reported as an unknown SESSION, not an
// unknown TAB — the two checks used to be collapsed into one tab-worded 404
// regardless of which path segment was actually bad.
#[tokio::test]
async fn delete_tab_with_bad_session_id_is_unknown_session_not_unknown_tab() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let bad_id = "x".repeat(dux_web::rest_common::MAX_ID_LEN + 1);
    let resp = client
        .delete(format!("http://{addr}/api/v1/sessions/{bad_id}/tabs/tab-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, "unknown session",
        "an out-of-bound session id must be reported as an unknown session"
    );
}

#[tokio::test]
async fn patch_tab_with_bad_session_id_is_unknown_session_not_unknown_tab() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let bad_id = "x".repeat(dux_web::rest_common::MAX_ID_LEN + 1);
    let resp = client
        .patch(format!("http://{addr}/api/v1/sessions/{bad_id}/tabs/tab-1"))
        .json(&serde_json::json!({ "provider": "codex" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, "unknown session",
        "an out-of-bound session id must be reported as an unknown session"
    );
}

// ── PUT /api/v1/sessions/:id/focused-tab: remembered tab-focus persistence ───

#[tokio::test]
async fn put_focused_tab_persists_and_is_readable_from_the_session() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;

    let resp = client
        .put(format!("http://{addr}/api/v1/sessions/s1/focused-tab"))
        .json(&serde_json::json!({ "tab_id": tab }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["last_focused_tab"], tab);
}

#[tokio::test]
async fn put_focused_tab_null_clears_the_memory() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;
    client
        .put(format!("http://{addr}/api/v1/sessions/s1/focused-tab"))
        .json(&serde_json::json!({ "tab_id": tab }))
        .send()
        .await
        .unwrap();

    let resp = client
        .put(format!("http://{addr}/api/v1/sessions/s1/focused-tab"))
        .json(&serde_json::json!({ "tab_id": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["last_focused_tab"], serde_json::Value::Null);
}

#[tokio::test]
async fn put_focused_tab_rejects_a_tab_owned_by_another_session() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let foreign_tab = create_support_tab(&client, addr, "s2").await;

    let resp = client
        .put(format!("http://{addr}/api/v1/sessions/s1/focused-tab"))
        .json(&serde_json::json!({ "tab_id": foreign_tab }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Normalized to no-memory rather than an error, matching the engine's
    // silent-normalization contract for a foreign/unknown tab id.
    let session: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/sessions/s1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["last_focused_tab"], serde_json::Value::Null);
}

#[tokio::test]
async fn put_focused_tab_with_bad_session_id_is_unknown_session() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();
    let bad_id = "x".repeat(dux_web::rest_common::MAX_ID_LEN + 1);
    let resp = client
        .put(format!(
            "http://{addr}/api/v1/sessions/{bad_id}/focused-tab"
        ))
        .json(&serde_json::json!({ "tab_id": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "unknown session");
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

// G-T3: the socket-reap behavior was only tested for a whole-agent kill
// (`ws_transport.rs::tearing_down_agent_pty_closes_its_attached_socket`);
// exercise the single-tab close path too: `DELETE .../tabs/:tab` on an EXTRA
// tab must proactively close that tab's own nested PTY socket (not merely go
// quiet), and the per-agent socket sub-quota slot it held must be released,
// not leaked.
#[tokio::test]
async fn deleting_a_tab_closes_its_attached_socket_and_frees_the_sub_quota() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    // Per-agent cap of one live tab socket, so a stuck/leaked slot from the
    // deleted tab would be directly observable: a second tab's socket would
    // never be able to connect.
    let (addr, _tmp) = boot_with_tab_per_agent(1).await;
    let client = reqwest::Client::new();
    let tab = create_support_tab(&client, addr, "s1").await;

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/sessions/s1/tabs/{tab}/pty"))
            .await
            .expect("connect the tab's pty socket");

    // Delete just this tab (not the whole agent).
    let del = client
        .delete(format!("http://{addr}/api/v1/sessions/s1/tabs/{tab}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);

    // The socket must close on its own (Close frame or stream end), not merely
    // go quiet.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut closed = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(300), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                closed = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        closed,
        "the deleted tab's pty socket was not proactively closed"
    );

    // The per-agent sub-quota slot the deleted tab's socket held must be
    // released: a brand-new tab's socket must be able to connect under the
    // same cap of 1, proving nothing was leaked.
    let tab2 = create_support_tab(&client, addr, "s1").await;
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
        "deleting a tab must free its per-agent socket sub-quota slot"
    );
}

// G-T2: every existing async-tab-launch test used `cat`, which always comes up
// live, so the actual ASYNC launch-failure path (as opposed to the synchronous
// 400 for an unconfigured provider) was never exercised. Use a provider whose
// command is a nonexistent binary: the create call still 201s (the row is
// minted synchronously; only the launch is async), but the tab must never
// reach `has_live_process`, and the failure must be surfaced by removing the
// dead row rather than leaving a tab that looks alive but never is.
#[tokio::test]
async fn tab_with_a_failing_async_launch_never_looks_live_and_is_cleaned_up() {
    let (addr, _tmp) = boot_with_broken_provider().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/tabs"))
        .json(&serde_json::json!({ "provider": "broken" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        201,
        "create still 201s; only the launch is async"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let tab = body["tab_id"].as_str().unwrap().to_string();

    // Poll until the row is gone (the fresh-create failure path deletes it) or
    // time out. Throughout, it must never report a live process.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut saw_row_removed = false;
    loop {
        let session: serde_json::Value = client
            .get(format!("http://{addr}/api/v1/sessions/s1"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let tabs = session["tabs"].as_array().unwrap();
        match tabs.iter().find(|t| t["id"].as_str() == Some(tab.as_str())) {
            Some(row) => {
                assert_ne!(
                    row["has_live_process"], true,
                    "a tab whose launch failed must never look live"
                );
            }
            None => {
                saw_row_removed = true;
                break;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        saw_row_removed,
        "a fresh tab whose first launch failed must be cleaned up, not left as a \
         permanently dead-looking row"
    );
}
