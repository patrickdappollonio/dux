//! Adversarial review (fifth pass): see the test.
//! Derived from the second pass: an editor write holds its editor ROOT,
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

fn standalone(id: &str, folder: &str) -> dux_core::model::AgentSession {
    let mut s = session(id, "");
    s.title = Some("mine".to_string());
    s.workspace = dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
        folder_path: folder.to_string(),
    });
    s
}

async fn serve(
    tmp: &dux_core::test_scratch::ScratchDir,
    repo: &std::path::Path,
    agent: dux_core::model::AgentSession,
) -> (reqwest::Client, String) {
    let root = tmp.path().to_path_buf();
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
        store.create_session(&agent).unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
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
    (
        client,
        format!("http://{addr}/api/v1/projects/p1/terminals/{tid}/files"),
    )
}

/// A standalone agent's folder, as dux recorded it, is a symbolic link in a
/// project's repository (the user pointed the agent at `repo/work`, a link
/// to their real checkout). The project terminal's editor deletes `work`:
/// the editor skips the occupancy check for any link, so the agent's folder
/// is gone from under it, though the one occupancy rule says the agent lives
/// exactly there.
#[tokio::test]
async fn review11_the_editor_does_not_delete_the_link_that_is_a_standalone_agents_folder() {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let repo = tmp.path().join("repo");
    let real = tmp.path().join("real-checkout");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("notes.txt"), "x").unwrap();
    let link = repo.join("work");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (client, prefix) = serve(&tmp, &repo, standalone("s1", link.to_str().unwrap())).await;
    let resp = client
        .post(format!("{prefix}/delete"))
        .json(&serde_json::json!({ "path": "work" }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "standalone agent s1's folder {} was deleted from the editor (answered {status})",
        link.display()
    );
}

/// The same link, moved.
#[tokio::test]
async fn review11_the_editor_does_not_move_the_link_that_is_a_standalone_agents_folder() {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let repo = tmp.path().join("repo");
    let real = tmp.path().join("real-checkout");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    let link = repo.join("work");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (client, prefix) = serve(&tmp, &repo, standalone("s1", link.to_str().unwrap())).await;
    let resp = client
        .post(format!("{prefix}/rename"))
        .json(&serde_json::json!({ "from": "work", "to": "elsewhere" }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "standalone agent s1's folder {} was moved away from the editor (answered {status})",
        link.display()
    );
}
