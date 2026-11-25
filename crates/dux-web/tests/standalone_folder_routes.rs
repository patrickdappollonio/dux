//! End-to-end tests for the routes about a standalone agent's folder, against a
//! real router and real folders in a temp dir:
//!
//! - `GET    /api/v1/standalone-agents/new-folder?name=`: the folder a new
//!   standalone agent of that name would get, and whether it is taken.
//! - `GET    /api/v1/sessions/:id/folder-files`: whether dux created the
//!   agent's folder and how many files it holds.
//! - `DELETE /api/v1/sessions/:id?delete_folder=`: the folder answer, judged by
//!   who made the folder rather than by the request.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::Router;
use dux_core::config::DuxPaths;
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

fn standalone_session(
    id: &str,
    folder: dux_core::model::FolderWorkspace,
) -> dux_core::model::AgentSession {
    let n = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: Some(id.to_string()),
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: n,
        updated_at: n,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Folder(folder),
    }
}

struct Fixture {
    addr: SocketAddr,
    /// The folder dux created for agent `made`, holding two files.
    made: PathBuf,
    /// The folder the user chose for agent `chosen`, holding one file.
    chosen: PathBuf,
    _tmp: dux_core::test_scratch::ScratchDir,
}

/// Boot a server with two standalone agents: `made`, in a folder dux created
/// in the agents folder, and `chosen`, in a folder the user already had.
async fn boot() -> Fixture {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    let created =
        dux_core::agent_folders::create_agent_folder(&root.join("agents"), "made").expect("create");
    std::fs::create_dir(created.path.join("notes")).expect("seed");
    std::fs::write(created.path.join("notes/a.md"), "a").expect("seed");
    std::fs::write(created.path.join("b.md"), "b").expect("seed");
    std::fs::create_dir(root.join("agents/taken")).expect("seed");
    let chosen = root.join("my-notes");
    std::fs::create_dir(&chosen).expect("seed");
    std::fs::write(chosen.join("mine.txt"), "mine").expect("seed");

    let store = SessionStore::open(&paths.sessions_db_path).expect("open the session store");
    store
        .create_session(&standalone_session(
            "made",
            dux_core::model::FolderWorkspace {
                folder_path: created.path.to_string_lossy().into_owned(),
                origin: dux_core::model::FolderOrigin::CreatedByDux(created.identity),
            },
        ))
        .expect("save the test agent");
    store
        .create_session(&standalone_session(
            "chosen",
            dux_core::model::FolderWorkspace::chosen(chosen.to_string_lossy().into_owned()),
        ))
        .expect("save the test agent");
    drop(store);

    let mut engine = bootstrap_engine(&paths).expect("bootstrap the test engine");
    dux_core::test_provider::defuse_config(&mut engine.config);
    let (handle, _join) = spawn_engine_thread(engine);
    let app = build_app(
        handle,
        Router::<AppState>::new(),
        RouterParams::plain_http(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener.local_addr().expect("read the bound address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve the test router");
    });
    Fixture {
        addr,
        made: created.path,
        chosen,
        _tmp: tmp,
    }
}

async fn get_json(addr: SocketAddr, path: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::get(format!("http://{addr}{path}"))
        .await
        .expect("send the request");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn delete(addr: SocketAddr, id: &str, query: &str) -> (u16, String) {
    let resp = reqwest::Client::new()
        .delete(format!("http://{addr}/api/v1/sessions/{id}?{query}"))
        .send()
        .await
        .expect("send the request");
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap_or_default())
}

async fn session_exists(addr: SocketAddr, id: &str) -> bool {
    get_json(addr, &format!("/api/v1/sessions/{id}")).await.0 == 200
}

/// Wait until `done` holds, or fail after a while.
async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn the_new_folder_route_names_the_folder_and_says_whether_it_is_taken() {
    let fx = boot().await;
    let (status, body) = get_json(fx.addr, "/api/v1/standalone-agents/new-folder?name=otter").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body["folder_label"]
            .as_str()
            .unwrap()
            .ends_with("/agents/otter"),
        "{body}"
    );
    assert_eq!(body["taken"], false, "{body}");
    let (_, body) = get_json(fx.addr, "/api/v1/standalone-agents/new-folder?name=taken").await;
    assert_eq!(body["taken"], true, "{body}");
    let (_, body) = get_json(fx.addr, "/api/v1/standalone-agents/new-folder?name=").await;
    assert!(body["folder_label"].is_null(), "{body}");
    assert!(
        body["root_label"].as_str().unwrap().ends_with("/agents"),
        "{body}"
    );
}

#[tokio::test]
async fn the_folder_files_route_counts_only_a_folder_dux_created() {
    let fx = boot().await;
    let (status, body) = get_json(fx.addr, "/api/v1/sessions/made/folder-files").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["created_by_dux"], true, "{body}");
    assert_eq!(body["files"]["state"], "counted", "{body}");
    assert_eq!(body["files"]["count"], 2, "{body}");
    let (status, body) = get_json(fx.addr, "/api/v1/sessions/chosen/folder-files").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["created_by_dux"], false, "{body}");
    assert!(body["files"].is_null(), "{body}");
}

/// A request to delete a folder the user chose is refused whole, whoever sends
/// it: the agent stays and so does every file.
#[tokio::test]
async fn deleting_a_chosen_folder_is_refused_and_nothing_goes() {
    let fx = boot().await;
    let (status, body) = delete(fx.addr, "chosen", "delete_folder=true").await;
    assert!((400..500).contains(&status), "{status} {body}");
    assert!(
        body.contains("dux only deletes folders it created"),
        "{body}"
    );
    assert!(session_exists(fx.addr, "chosen").await);
    assert!(fx.chosen.join("mine.txt").exists());
}

/// A delete with no folder answer leaves a folder the user chose alone.
#[tokio::test]
async fn an_unasked_delete_keeps_a_chosen_folder() {
    let fx = boot().await;
    let (status, body) = delete(fx.addr, "chosen", "delete_worktree=false").await;
    assert!((200..300).contains(&status), "{status} {body}");
    assert!(fx.chosen.join("mine.txt").exists());
}

#[tokio::test]
async fn keeping_the_folder_dux_created_keeps_it() {
    let fx = boot().await;
    let (status, body) = delete(fx.addr, "made", "delete_folder=false").await;
    assert!((200..300).contains(&status), "{status} {body}");
    let addr = fx.addr;
    let mut gone = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gone && Instant::now() < deadline {
        gone = !session_exists(addr, "made").await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(gone, "the agent is deleted");
    assert!(fx.made.join("b.md").exists());
}

#[tokio::test]
async fn deleting_the_folder_dux_created_deletes_it() {
    let fx = boot().await;
    let (status, body) = delete(fx.addr, "made", "delete_folder=true").await;
    assert!((200..300).contains(&status), "{status} {body}");
    let made: &Path = &fx.made;
    eventually("the folder's removal", || !made.exists()).await;
}
