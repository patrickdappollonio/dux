//! End-to-end tests for the delete dialog's two session routes against a real
//! router, a real git repository and real worktrees in a temp dir:
//!
//! - `GET    /api/v1/sessions/:id/branch-unpushed`: the branches a delete would
//!   remove and how much of their work exists only here.
//! - `DELETE /api/v1/sessions/:id?delete_branch=`:  where an ABSENT answer and
//!   an explicit `false` part company, which is the compatibility claim the
//!   parameter rests on and the one a client actually enters through.
//!
//! The route is where these matter: an HTTP caller reaches them without passing
//! through either dialog, so neither the naming nor the default may live in a
//! surface.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};
use futures_util::StreamExt;

fn git(dir: &Path, args: &[&str]) {
    let out = dux_core::test_git::fixture_git()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A managed agent on `branch_name`, born on `initial_branch`.
fn managed_session(
    id: &str,
    worktree: &str,
    branch_name: &str,
    initial_branch: &str,
    provenance: dux_core::model::BranchProvenance,
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
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: branch_name.to_string(),
            initial_branch: initial_branch.to_string(),
            branch_provenance: provenance,
            worktree_path: worktree.to_string(),
        }),
    }
}

/// A standalone agent: a folder the user already had, with no branch at all.
fn standalone_session(id: &str, folder: &str) -> dux_core::model::AgentSession {
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
        workspace: dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
            folder_path: folder.to_string(),
        }),
    }
}

struct Fixture {
    addr: SocketAddr,
    repo: std::path::PathBuf,
    _tmp: dux_core::test_scratch::ScratchDir,
}

/// Boot a server over a real repo with three agents:
///
/// - `drifted`, attached to `develop` and since moved onto `develop-next`,
/// - `duxs`, on a branch dux created for it,
/// - `folder`, a standalone agent.
async fn boot() -> Fixture {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let managed_root = root.join("worktrees").join("repo");
    std::fs::create_dir_all(&managed_root).unwrap();

    // The drifted agent: born on `develop`, its worktree now on `develop-next`,
    // which carries one commit `develop` does not.
    let drifted_wt = managed_root.join("develop");
    git(&repo, &["branch", "develop"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            drifted_wt.to_string_lossy().as_ref(),
            "develop",
        ],
    );
    git(&drifted_wt, &["switch", "-q", "-c", "develop-next"]);
    git(
        &drifted_wt,
        &["commit", "-q", "--allow-empty", "-m", "work"],
    );

    // An agent on a branch dux made for it.
    let duxs_wt = managed_root.join("duxs");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "dux/made-this",
            duxs_wt.to_string_lossy().as_ref(),
        ],
    );

    let folder = root.join("someones-folder");
    std::fs::create_dir_all(&folder).unwrap();

    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: repo.to_string_lossy().into_owned(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .create_session(&managed_session(
                "drifted",
                drifted_wt.to_string_lossy().as_ref(),
                "develop-next",
                "develop",
                dux_core::model::BranchProvenance::AttachedExisting,
            ))
            .unwrap();
        store
            .create_session(&managed_session(
                "duxs",
                duxs_wt.to_string_lossy().as_ref(),
                "dux/made-this",
                "dux/made-this",
                dux_core::model::BranchProvenance::CreatedByDux,
            ))
            .unwrap();
        store
            .create_session(&standalone_session(
                "folder",
                folder.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
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

    Fixture {
        addr,
        repo,
        _tmp: tmp,
    }
}

async fn branch_unpushed(addr: SocketAddr, id: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::get(format!(
        "http://{addr}/api/v1/sessions/{id}/branch-unpushed"
    ))
    .await
    .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// The delete, scoped to an events connection so its outcome message comes back
/// on that socket.
async fn delete(addr: SocketAddr, id: &str, query: &str, connection: &str) -> u16 {
    reqwest::Client::new()
        .delete(format!("http://{addr}/api/v1/sessions/{id}?{query}"))
        .header("x-connection-id", connection)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// An open `/ws/events` connection and the id that scopes a request's statuses
/// to it, read off the handshake's `connected` first frame.
async fn events_socket(
    addr: SocketAddr,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let frame = next_frame(&mut ws, |v| v["event"] == "connected")
        .await
        .expect("a /ws/events connection must send a `connected` first frame");
    let id = frame["id"]
        .as_str()
        .expect("the connected frame carries a string id")
        .to_string();
    (ws, id)
}

/// Read frames until one satisfies `pred`, or give up after ten seconds.
async fn next_frame<S>(
    ws: &mut S,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> Option<serde_json::Value>
where
    S: StreamExt<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(message))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
        else {
            continue;
        };
        let Ok(text) = message.into_text() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if pred(&value) {
            return Some(value);
        }
    }
    None
}

/// The delete's own final status message, and THE SETTLEMENT SIGNAL for every
/// assertion about branches.
///
/// The route answers 204 while a worker is still removing the worktree and
/// running the two `git branch -D` calls, so the agent's record is already gone
/// from the workspace (a 404 on its id) while the branches have not settled: a
/// list read then can catch the delete exactly between its two branch removals.
/// This message is emitted when that worker completes, and unlike a branch list
/// it carries git's own words when git REFUSED to delete one, so a refusal fails
/// a test with its reason rather than with a bare list.
///
/// Every final variant opens with "Deleted" and names the agent, the busy that
/// precedes it ("Removing worktree for agent …") does neither, so matching that
/// opening picks the final whatever it went on to say.
async fn wait_for_delete_status<S>(ws: &mut S, name: &str) -> String
where
    S: StreamExt<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    let names_agent = format!("agent \"{name}\"");
    let frame = next_frame(ws, |v| {
        v["event"] == "status"
            && v["message"].as_str().is_some_and(|message| {
                message.starts_with("Deleted ") && message.contains(&names_agent)
            })
    })
    .await
    .unwrap_or_else(|| panic!("the delete of {name} must report its outcome on the events socket"));
    frame["message"].as_str().unwrap().to_string()
}

fn branches(repo: &Path) -> Vec<String> {
    let out = dux_core::test_git::fixture_git()
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| line.to_string())
        .collect()
}

/// The session is dropped from the workspace before the removal worker is
/// dispatched, so by the time its outcome message lands this is a plain
/// assertion rather than something to wait for.
async fn assert_session_gone(addr: SocketAddr, id: &str) {
    let status = reqwest::get(format!("http://{addr}/api/v1/sessions/{id}"))
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(
        status, 404,
        "session {id} was still present after the delete"
    );
}

/// The route names every branch the delete would remove, and counts them as one
/// set: a drifted agent gives up both, so an answer about one of them would
/// understate what the tick costs.
#[tokio::test]
async fn branch_unpushed_names_both_branches_of_a_drifted_agent() {
    let f = boot().await;
    let (status, body) = branch_unpushed(f.addr, "drifted").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["branches"],
        serde_json::json!(["develop-next", "develop"]),
        "{body}"
    );
    // No remote-tracking refs at all here, so the count is the whole history:
    // the initial commit plus the one made on `develop-next`.
    assert_eq!(body["unpushed"]["count"], 2, "{body}");
    assert_eq!(body["unpushed"]["has_remote_refs"], false, "{body}");
}

/// A standalone agent has no branch, so there is no question to answer. It is
/// refused rather than answered with an empty list, which a client would render
/// as a checkbox naming nothing.
#[tokio::test]
async fn branch_unpushed_refuses_a_standalone_agent() {
    let f = boot().await;
    let (status, _) = branch_unpushed(f.addr, "folder").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn branch_unpushed_refuses_a_session_that_does_not_exist() {
    let f = boot().await;
    let (status, _) = branch_unpushed(f.addr, "nobody").await;
    assert_eq!(status, 404);
}

/// THE COMPATIBILITY CLAIM, at the layer a client actually enters through: an
/// ABSENT `delete_branch` keeps the provenance default, so a caller written
/// before the parameter existed behaves exactly as it did. `develop` predates
/// the agent, so it survives, and so does the branch the worktree drifted onto.
#[tokio::test]
async fn an_absent_branch_answer_keeps_the_provenance_default() {
    let f = boot().await;
    let (mut ws, connection) = events_socket(f.addr).await;
    assert_eq!(
        delete(f.addr, "drifted", "delete_worktree=true", &connection).await,
        204
    );
    let status = wait_for_delete_status(&mut ws, "drifted").await;
    assert!(
        status.contains("\"develop-next\" was created inside this agent's worktree and was kept")
            && status.contains("\"develop\" existed before this agent and was kept"),
        "the outcome must name both kept branches with their reasons: {status}"
    );
    assert_session_gone(f.addr, "drifted").await;
    let listed = branches(&f.repo);
    assert!(
        listed.contains(&"develop".to_string()),
        "a branch that predates the agent is not dux's to delete: {listed:?} ({status})"
    );
    assert!(
        listed.contains(&"develop-next".to_string()),
        "and neither is the one the worktree drifted onto: {listed:?} ({status})"
    );
}

/// The same request with an explicit `false` is a different thing from an
/// absent one only when the default would have deleted: dux made this branch,
/// so unasked it would go, and the answer is what spares it.
#[tokio::test]
async fn an_explicit_false_spares_a_branch_dux_created() {
    let f = boot().await;
    let (mut ws, connection) = events_socket(f.addr).await;
    assert_eq!(
        delete(
            f.addr,
            "duxs",
            "delete_worktree=true&delete_branch=false",
            &connection
        )
        .await,
        204
    );
    let status = wait_for_delete_status(&mut ws, "duxs").await;
    assert!(
        status.contains("\"dux/made-this\" was kept because you chose to keep it"),
        "the outcome must say the answer is what spared the branch: {status}"
    );
    assert_session_gone(f.addr, "duxs").await;
    assert!(
        branches(&f.repo).contains(&"dux/made-this".to_string()),
        "the answer must override the provenance default: {:?} ({status})",
        branches(&f.repo)
    );
}

/// And in the other direction: a branch dux would have kept goes when the user
/// asks, which is the only way to remove it once the worktree is gone. Both of
/// the drifted agent's branches go, because both were named.
#[tokio::test]
async fn an_explicit_true_removes_every_branch_the_route_named() {
    let f = boot().await;
    let (_, body) = branch_unpushed(f.addr, "drifted").await;
    let named: Vec<String> = body["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap().to_string())
        .collect();
    let before = branches(&f.repo);

    let (mut ws, connection) = events_socket(f.addr).await;
    assert_eq!(
        delete(
            f.addr,
            "drifted",
            "delete_worktree=true&delete_branch=true",
            &connection
        )
        .await,
        204
    );
    // The outcome message first: it settles the branches AND says in git's own
    // words when git refused one, which the list below cannot.
    let status = wait_for_delete_status(&mut ws, "drifted").await;
    assert!(
        status.contains("deleted its branch \"develop-next\"")
            && status.contains("Its original branch \"develop\" was deleted too"),
        "the outcome must report both branches deleted: {status}"
    );
    assert_session_gone(f.addr, "drifted").await;

    let after = branches(&f.repo);
    let mut gone: Vec<String> = before
        .iter()
        .filter(|b| !after.contains(b))
        .cloned()
        .collect();
    gone.sort();
    let mut named = named;
    named.sort();
    assert_eq!(
        gone, named,
        "the route must remove exactly the branches it named ({status})"
    );
}
