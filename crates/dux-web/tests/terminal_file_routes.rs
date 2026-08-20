//! End-to-end tests for the TERMINAL-rooted editor file routes: the same
//! handlers the agent editor is served by, registered under a terminal prefix
//! and rooted at the directory the terminal was SPAWNED in.
//!
//! Two prefixes exist, and the pair is the point. `/api/v1/terminals/{tid}/files/*`
//! serves standalone terminals only, exactly as the un-nested delete route does,
//! and `/api/v1/projects/{pid}/terminals/{tid}/files/*` serves that project's
//! terminals. Everything else is a 404, so an id cannot be walked from one
//! namespace into another. There is deliberately no session-nested prefix: a
//! session-owned terminal shares its agent's worktree, so its editor is the
//! agent's editor.

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

struct Harness {
    addr: SocketAddr,
    repo: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

/// Boot a server with two projects (`p1` at a populated repo root, `p2` empty)
/// and one session `s1`. The terminal command is `cat`, so every terminal in
/// these tests is a cheap long-lived child rather than a real shell.
async fn boot() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let repo = root.join("repo");
    let other = root.join("other");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(repo.join(".bashrc"), "export X=1\n").unwrap();
    std::fs::create_dir(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/child.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir(repo.join(".cache")).unwrap();
    std::fs::write(repo.join(".cache/blob"), "junk\n").unwrap();
    let wt1 = root.join("wt1");
    std::fs::create_dir_all(&wt1).unwrap();
    std::fs::write(wt1.join("agent.txt"), "agent\n").unwrap();

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
        for (id, path) in [("p1", &repo), ("p2", &other)] {
            store
                .upsert_project(&ProjectConfig {
                    id: id.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    name: Some(id.to_string()),
                    default_provider: None,
                    leading_branch: None,
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .unwrap();
        }
        store
            .upsert_session(&sample_session("s1", wt1.to_string_lossy().as_ref()))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
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
    Harness {
        addr,
        repo,
        _tmp: tmp,
    }
}

async fn create_project_terminal(addr: SocketAddr, project: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/projects/{project}/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["terminal_id"].as_str().unwrap().to_string()
}

async fn create_standalone_terminal(addr: SocketAddr) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/terminals"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["terminal_id"].as_str().unwrap().to_string()
}

async fn post(addr: SocketAddr, path: &str, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_project_terminal_serves_the_editor_rooted_at_its_spawn_directory() {
    // The whole journey on one terminal: the tree lists the repo root it was
    // spawned in, a read returns that root's file, and a write lands on disk
    // there.
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let prefix = format!("/api/v1/projects/p1/terminals/{tid}/files");

    let resp = post(h.addr, &format!("{prefix}/tree"), serde_json::json!({})).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<String> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"Cargo.toml".to_string()), "got {names:?}");
    assert!(names.contains(&"sub".to_string()), "got {names:?}");

    let resp = post(
        h.addr,
        &format!("{prefix}/read"),
        serde_json::json!({ "path": "sub/child.rs" }),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["content"], "fn main() {}\n");

    let resp = post(
        h.addr,
        &format!("{prefix}/write"),
        serde_json::json!({ "path": "sub/child.rs", "content": "fn main() { 1; }\n" }),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        std::fs::read_to_string(h.repo.join("sub/child.rs")).unwrap(),
        "fn main() { 1; }\n"
    );
}

#[tokio::test]
async fn a_terminal_root_refuses_a_path_that_leaves_it() {
    // The containment layer is root-agnostic and is not relaxed for terminals:
    // the same traversal refusal the agent editor gets applies here.
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let prefix = format!("/api/v1/projects/p1/terminals/{tid}/files");
    for path in ["../other/escape.txt", "/etc/passwd"] {
        let resp = post(
            h.addr,
            &format!("{prefix}/read"),
            serde_json::json!({ "path": path }),
        )
        .await;
        assert_eq!(resp.status(), 400, "reading {path} must be refused");
    }
    let resp = post(
        h.addr,
        &format!("{prefix}/tree"),
        serde_json::json!({ "dir": ".." }),
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn a_terminal_search_prunes_dot_directories_but_keeps_dotfiles() {
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let prefix = format!("/api/v1/projects/p1/terminals/{tid}/files");
    let resp = post(h.addr, &format!("{prefix}/list"), serde_json::json!({})).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let files: Vec<String> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect();
    assert!(files.contains(&".bashrc".to_string()), "got {files:?}");
    assert!(files.contains(&"sub/child.rs".to_string()), "got {files:?}");
    assert!(
        !files.iter().any(|f| f.starts_with(".cache/")),
        "a dot directory must be pruned under a terminal root: {files:?}"
    );
}

#[tokio::test]
async fn a_terminal_rooted_editor_has_no_diff_route_at_all() {
    // No diff mode for a terminal root: the affordance is absent, not disabled,
    // and the route it would call does not exist.
    // An unregistered API path falls through to the SPA static handler, so the
    // proof is that `diff` behaves exactly like a path that was never a route,
    // and unlike `read`, which is registered and answers from the handler.
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let prefix = format!("/api/v1/projects/p1/terminals/{tid}/files");
    let control = post(
        h.addr,
        &format!("{prefix}/definitely-not-a-route"),
        serde_json::json!({}),
    )
    .await
    .status();
    // A path that does not exist, so a registered handler answers 400 and the
    // static fallback still answers whatever it answers for any other unrouted
    // address.
    let missing = serde_json::json!({ "path": "nothing-here.txt" });
    let diff = post(h.addr, &format!("{prefix}/diff"), missing.clone())
        .await
        .status();
    assert_eq!(diff, control, "diff must not be registered for a terminal");
    let read = post(h.addr, &format!("{prefix}/read"), missing)
        .await
        .status();
    assert_ne!(
        read, control,
        "read must reach the handler, or the comparison above proves nothing"
    );
}

#[tokio::test]
async fn an_owned_terminal_is_a_404_at_the_un_nested_prefix() {
    // The un-nested address serves standalone terminals ONLY, exactly like the
    // un-nested delete: an owned terminal cannot be reached through it.
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let resp = post(
        h.addr,
        &format!("/api/v1/terminals/{tid}/files/read"),
        serde_json::json!({ "path": "Cargo.toml" }),
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn a_session_id_is_a_404_in_every_terminal_namespace() {
    // A session id is not a terminal id, whichever terminal prefix it is posted
    // to: the resolver takes the namespace, never just the id.
    let h = boot().await;
    for prefix in [
        "/api/v1/terminals/s1/files".to_string(),
        "/api/v1/projects/p1/terminals/s1/files".to_string(),
    ] {
        let resp = post(
            h.addr,
            &format!("{prefix}/read"),
            serde_json::json!({ "path": "agent.txt" }),
        )
        .await;
        assert_eq!(resp.status(), 404, "session id served at {prefix}");
    }
}

#[tokio::test]
async fn a_project_terminal_is_a_404_under_a_different_project() {
    let h = boot().await;
    let tid = create_project_terminal(h.addr, "p1").await;
    let resp = post(
        h.addr,
        &format!("/api/v1/projects/p2/terminals/{tid}/files/read"),
        serde_json::json!({ "path": "Cargo.toml" }),
    )
    .await;
    assert_eq!(resp.status(), 404);
    let resp = post(
        h.addr,
        &format!("/api/v1/projects/nope/terminals/{tid}/files/read"),
        serde_json::json!({ "path": "Cargo.toml" }),
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn a_standalone_terminal_is_a_404_at_the_project_nested_prefix() {
    // The mirror of the case above: the nested address names an owner, and a
    // terminal that has none is not reachable through it.
    let h = boot().await;
    let tid = create_standalone_terminal(h.addr).await;
    let resp = post(
        h.addr,
        &format!("/api/v1/projects/p1/terminals/{tid}/files/read"),
        serde_json::json!({ "path": "Cargo.toml" }),
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn an_unknown_terminal_id_is_a_404_in_both_namespaces() {
    let h = boot().await;
    for prefix in [
        "/api/v1/terminals/term-999/files".to_string(),
        "/api/v1/projects/p1/terminals/term-999/files".to_string(),
    ] {
        let resp = post(h.addr, &format!("{prefix}/tree"), serde_json::json!({})).await;
        assert_eq!(resp.status(), 404, "unknown terminal served at {prefix}");
    }
}
