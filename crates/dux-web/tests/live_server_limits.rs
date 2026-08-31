//! `[server] search_index_max_files` is read off the shared live limits on every
//! search walk, so a config reload bounds the next one without a restart.

use std::net::SocketAddr;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::{LiveServerLimits, spawn_engine_thread};
use dux_web::server::{AppState, RouterParams, build_app};

fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let n = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: None,
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: n,
        updated_at: n,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: format!("{id}-branch"),
            initial_branch: format!("{id}-branch"),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

/// A server with one session whose worktree holds several files, so a lowered
/// cap has something to truncate.
async fn boot() -> (
    SocketAddr,
    std::sync::Arc<LiveServerLimits>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt1 = root.join("wt1");
    std::fs::create_dir_all(&wt1).unwrap();
    for i in 0..8 {
        std::fs::write(wt1.join(format!("file{i}.txt")), "x\n").unwrap();
    }

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
            .create_session(&sample_session("s1", wt1.to_string_lossy().as_ref()))
            .unwrap();
    }
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let limits = handle.live_limits();
    let app = build_app(
        handle,
        Router::<AppState>::new(),
        RouterParams::plain_http(),
    );
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
    (addr, limits, tmp)
}

async fn list_files(addr: SocketAddr) -> serde_json::Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/sessions/s1/files/list"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_reloaded_search_index_cap_bounds_the_next_walk() {
    let (addr, limits, _tmp) = boot().await;

    // `truncated` is omitted when false, so absence is the uncapped answer.
    let full = list_files(addr).await;
    assert!(full["truncated"].as_bool() != Some(true));
    assert_eq!(full["files"].as_array().unwrap().len(), 8);

    limits.set_search_index_max_files(3);

    let capped = list_files(addr).await;
    assert_eq!(
        capped["truncated"], true,
        "the walk must honor the reloaded cap without a restart"
    );
    assert_eq!(capped["files"].as_array().unwrap().len(), 3);
}
