//! Adversarial review (second pass): an editor write holds its editor ROOT,
//! never the path it writes. A terminal's editor is rooted at the folder the
//! terminal started in, and that folder can contain an agent's worktree (a
//! standalone terminal starts in the home folder, which holds dux's worktrees
//! root by default; a project terminal starts at the repository root, which
//! holds any worktree kept inside the repository, such as `.worktrees/`). A
//! write through such an editor into a worktree whose removal has begun is
//! neither refused nor waited for, and a folder created there after git
//! removed the worktree brings the worktree's folder back.

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

#[tokio::test]
async fn a_terminal_editor_cannot_write_into_a_worktree_being_removed() {
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

    // The agent's worktree is being removed: its delete has claimed it.
    let _removal = ops.announce_removal(&worktree);
    assert!(ops.is_being_removed(&worktree));

    // The agent's own editor is refused, as the branch intends...
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/write"))
        .json(&serde_json::json!({ "path": "own.txt", "content": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409, "the agent's own editor is refused");

    // ...but the same write through the project terminal's editor lands.
    let resp = client
        .post(format!("{prefix}/write"))
        .json(&serde_json::json!({ "path": ".worktrees/agent/new.txt", "content": "x" }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let landed = worktree.join("new.txt").exists();
    // And once git has removed the folder, a folder created through the same
    // editor brings the worktree's folder back.
    std::fs::remove_dir_all(&worktree).unwrap();
    let resp = client
        .post(format!("{prefix}/create-dir"))
        .json(&serde_json::json!({ "path": ".worktrees/agent/sub" }))
        .send()
        .await
        .unwrap();
    let recreated = worktree.exists();
    let recreate_status = resp.status();
    assert!(
        status == 409 && !landed && !recreated,
        "while the removal of {} was under way: a write through the project terminal's \
         editor answered {status} (landed: {landed}); after the folder was gone, creating a \
         folder answered {recreate_status} and brought the worktree folder back: {recreated}",
        worktree.display()
    );
}
