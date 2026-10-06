//! review9: an editor move of a folder around a worktree whose removal is running.

use std::net::SocketAddr;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

fn session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
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
            branch_name: "agent".to_string(),
            initial_branch: "agent".to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

struct Fixture {
    ops: dux_core::worktree_ops::WorktreeOps,
    _tmp: dux_core::test_scratch::ScratchDir,
    _worktree: std::path::PathBuf,
    repo: std::path::PathBuf,
    client: reqwest::Client,
    prefix: String,
}

async fn fixture() -> Fixture {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();
    let repo = root.join("repo");
    let worktree = repo.join(".worktrees").join("agent");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("a.txt"), "a\n").unwrap();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: repo.to_string_lossy().into_owned(),
                name: Some("p1".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .create_session(&session("s1", worktree.to_string_lossy().as_ref()))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let ops = engine.worktree_ops().clone();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = build_app(
        handle,
        Router::<AppState>::new(),
        RouterParams::plain_http(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/v1/projects/p1/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tid = body["terminal_id"].as_str().unwrap().to_string();
    let prefix = format!("http://{addr}/api/v1/projects/p1/terminals/{tid}/files");

    Fixture {
        ops,
        _tmp: tmp,
        _worktree: worktree,
        repo,
        client,
        prefix,
    }
}

#[tokio::test]
async fn review9_editor_move_waits_for_a_removal_running_inside_the_folder() {
    let Fixture {
        ops,
        _tmp,
        repo,
        client,
        prefix,
        ..
    } = fixture().await;
    // Agent "gone" was deleted with its worktree: its record is gone, its
    // processes have ended, and its removal holds the claim on the folder
    // while git removes it.
    let doomed = repo.join("kept-worktrees").join("gone");
    std::fs::create_dir_all(&doomed).unwrap();
    std::fs::write(doomed.join("f.txt"), "x\n").unwrap();
    let dux_core::worktree_ops::RemovalClaim::Lead(running) = ops.announce_removal(&doomed) else {
        panic!("leads");
    };
    // The user moves the folder around it from the project terminal's tree.
    let resp = client
        .post(format!("{prefix}/rename"))
        .json(&serde_json::json!({ "from": "kept-worktrees", "to": "old-worktrees" }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    drop(running);
    assert!(
        doomed.exists(),
        "the editor moved {} away (answered {status}) while dux's own removal of the worktree \
         inside it was still running",
        doomed.display()
    );
}
