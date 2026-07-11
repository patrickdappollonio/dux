//! End-to-end tests for the lazy file-tree route: `POST
//! /api/v1/sessions/:id/files/tree` lists exactly one worktree directory per
//! request (no recursion, no cap), refuses traversal, and 404s unknown sessions.

use std::net::SocketAddr;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
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

/// Boot a server with one session (`s1`) whose worktree holds a dotfile, a
/// plain file, and a subdirectory with a child file, so the tree listing has
/// content to assert on.
async fn boot() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt1 = root.join("wt1");
    std::fs::create_dir_all(&wt1).unwrap();
    std::fs::write(wt1.join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(wt1.join(".dotfile"), "hidden\n").unwrap();
    std::fs::create_dir(wt1.join("sub")).unwrap();
    std::fs::write(wt1.join("sub/child.rs"), "fn main() {}\n").unwrap();

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
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
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
    (addr, tmp)
}

async fn post_tree(addr: SocketAddr, session: &str, dir: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/sessions/{session}/files/tree"
        ))
        .json(&serde_json::json!({ "dir": dir }))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn tree_root_lists_entries_dirs_first_including_dotfiles() {
    let (addr, _tmp) = boot().await;
    let resp = post_tree(addr, "s1", "").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["dir"], "");
    let entries = body["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    // Dirs first, then files case-insensitively: sub, then .dotfile, Cargo.toml.
    assert_eq!(names, vec!["sub", ".dotfile", "Cargo.toml"]);
    let sub = &entries[0];
    assert_eq!(sub["is_dir"], true);
    assert_eq!(sub["expandable"], true);
    assert_eq!(sub["path"], "sub");
}

#[tokio::test]
async fn tree_subdir_lists_children_with_worktree_relative_paths() {
    let (addr, _tmp) = boot().await;
    let resp = post_tree(addr, "s1", "sub").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["dir"], "sub");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "child.rs");
    assert_eq!(entries[0]["path"], "sub/child.rs");
    assert_eq!(entries[0]["is_dir"], false);
    assert_eq!(entries[0]["expandable"], false);
}

#[tokio::test]
async fn tree_rejects_traversal_with_400() {
    let (addr, _tmp) = boot().await;
    let resp = post_tree(addr, "s1", "..").await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn tree_unknown_session_is_404() {
    let (addr, _tmp) = boot().await;
    let resp = post_tree(addr, "nope", "").await;
    assert_eq!(resp.status(), 404);
}
