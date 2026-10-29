//! End-to-end tests for the session routes that touch an agent's branch, against
//! a real router, a real git repository and real worktrees in a temp dir:
//!
//! - `GET    /api/v1/sessions/:id/branch-unpushed`: the branches a delete would
//!   offer, each pinned, and how much of their work exists only here.
//! - `DELETE /api/v1/sessions/:id`: an absent answer, the older
//!   `?delete_branch=`, and the body carrying both boxes' pinned answers.
//! - `GET    /api/v1/sessions/:id/branch-pushed`: whether a branch rename would
//!   leave a pushed copy behind on the old name.
//! - `PATCH  /api/v1/sessions/:id` with `rename_branch`: the rename dialog's
//!   branch box, running `git branch -m` and its refusals.
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
        .expect("run git");
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

/// Save the project and the three agents `boot` serves.
fn seed_store(paths: &DuxPaths, repo: &Path, drifted_wt: &Path, duxs_wt: &Path, folder: &Path) {
    let store = SessionStore::open(&paths.sessions_db_path).expect("open the session store");
    store
        .upsert_project(&ProjectConfig {
            id: "p1".to_string(),
            path: repo.to_string_lossy().into_owned(),
            name: Some("repo".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
        })
        .expect("save the test project");
    store
        .create_session(&managed_session(
            "drifted",
            drifted_wt.to_string_lossy().as_ref(),
            "develop-next",
            "develop",
            dux_core::model::BranchProvenance::AttachedExisting,
        ))
        .expect("save the test agent");
    store
        .create_session(&managed_session(
            "duxs",
            duxs_wt.to_string_lossy().as_ref(),
            "dux/made-this",
            "dux/made-this",
            dux_core::model::BranchProvenance::CreatedByDux,
        ))
        .expect("save the test agent");
    store
        .create_session(&standalone_session(
            "folder",
            folder.to_string_lossy().as_ref(),
        ))
        .expect("save the test agent");
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
    std::fs::create_dir_all(&repo).expect("create the test directory");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let managed_root = root.join("worktrees").join("repo");
    std::fs::create_dir_all(&managed_root).expect("create the test directory");

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
    std::fs::create_dir_all(&folder).expect("create the test directory");

    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    seed_store(&paths, &repo, &drifted_wt, &duxs_wt, &folder);
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
        repo,
        _tmp: tmp,
    }
}

async fn branch_unpushed(addr: SocketAddr, id: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::get(format!(
        "http://{addr}/api/v1/sessions/{id}/branch-unpushed"
    ))
    .await
    .expect("send the request");
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
        .expect("send the request")
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
        .expect("open the websocket");
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
    frame["message"]
        .as_str()
        .expect("the status frame carries a message")
        .to_string()
}

fn branches(repo: &Path) -> Vec<String> {
    let out = dux_core::test_git::fixture_git()
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(repo)
        .output()
        .expect("run git");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(std::string::ToString::to_string)
        .collect()
}

/// The session is dropped from the workspace before the removal worker is
/// dispatched, so by the time its outcome message lands this is a plain
/// assertion rather than something to wait for.
async fn assert_session_gone(addr: SocketAddr, id: &str) {
    let status = reqwest::get(format!("http://{addr}/api/v1/sessions/{id}"))
        .await
        .expect("send the request")
        .status()
        .as_u16();
    assert_eq!(
        status, 404,
        "session {id} was still present after the delete"
    );
}

/// The route offers the branch the worktree is on and every other branch it
/// has been on, each pinned to the commit a tick would delete and with who
/// made it, and counts each box's work on its own.
#[tokio::test]
async fn branch_unpushed_offers_both_boxes_pinned() {
    let f = boot().await;
    let (status, body) = branch_unpushed(f.addr, "drifted").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["current"]["name"], "develop-next", "{body}");
    assert_eq!(
        body["current"]["oid"].as_str().unwrap(),
        rev(&f.repo, "develop-next"),
        "{body}"
    );
    // Made just before the agent's record was: before it, or in the same
    // second, which proves nothing either way. Never dux's.
    assert!(
        matches!(
            body["current"]["origin"].as_str(),
            Some("before" | "unknown")
        ),
        "{body}"
    );
    assert_eq!(body["others"][0]["name"], "develop", "{body}");
    assert_eq!(body["others"][0]["origin"], "attached", "{body}");
    assert_eq!(body["others"].as_array().unwrap().len(), 1, "{body}");
    // Each count is what that box's deletion loses: the commit made on
    // `develop-next` is on no other branch, while the initial commit `develop`
    // holds is on `main` too, so deleting `develop` loses nothing.
    assert_eq!(body["current_unpushed"]["count"], 1, "{body}");
    assert_eq!(body["others_unpushed"]["count"], 0, "{body}");
    // Both ticked loses what neither box leaves behind.
    assert_eq!(body["both_unpushed"]["count"], 1, "{body}");
    assert_eq!(body["current_name"], "develop-next", "{body}");
    assert_eq!(body["protected"], serde_json::json!([]), "{body}");
    assert_eq!(body["others_unpushed"]["has_remote_refs"], false, "{body}");
}

/// A branch the worktree was on but that no longer exists is not offered: there
/// is nothing left to orphan, or to count.
#[tokio::test]
async fn branch_unpushed_does_not_offer_a_branch_that_is_gone() {
    let f = boot().await;
    git(&f.repo, &["branch", "-D", "develop"]);
    let (status, body) = branch_unpushed(f.addr, "drifted").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["others"], serde_json::json!([]), "{body}");
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

fn rev(repo: &Path, branch: &str) -> String {
    let out = dux_core::test_git::fixture_git()
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .current_dir(repo)
        .output()
        .expect("run git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The delete with both boxes' answers in its body.
async fn delete_with_body(
    addr: SocketAddr,
    id: &str,
    body: &serde_json::Value,
    connection: &str,
) -> u16 {
    reqwest::Client::new()
        .delete(format!(
            "http://{addr}/api/v1/sessions/{id}?delete_worktree=true"
        ))
        .header("x-connection-id", connection)
        .json(body)
        .send()
        .await
        .expect("send the request")
        .status()
        .as_u16()
}

/// Nobody was asked: neither the branch the worktree moved onto nor the one it
/// was attached to is dux's, so both survive, each named with its reason.
#[tokio::test]
async fn an_absent_branch_answer_keeps_every_branch_dux_did_not_make() {
    let f = boot().await;
    let (mut ws, connection) = events_socket(f.addr).await;
    assert_eq!(
        delete(f.addr, "drifted", "delete_worktree=true", &connection).await,
        204
    );
    let status = wait_for_delete_status(&mut ws, "drifted").await;
    assert!(
        status.contains("Its branch \"develop-next\" was kept: this agent was on it, but nobody")
            && status.contains("\"develop\" existed before this agent and was kept"),
        "the outcome must name both kept branches with their reasons: {status}"
    );
    assert_session_gone(f.addr, "drifted").await;
    let listed = branches(&f.repo);
    assert!(
        listed.contains(&"develop".to_string()) && listed.contains(&"develop-next".to_string()),
        "{listed:?} ({status})"
    );
}

/// The older single answer still works: an explicit `false` spares a branch
/// dux created, which unasked it would delete.
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
        "the answer must override the default: {:?} ({status})",
        branches(&f.repo)
    );
}

/// Both boxes ticked over what the route offered delete exactly those branches,
/// each only at the commit it was offered at: one that moved since is kept.
#[tokio::test]
async fn ticked_boxes_delete_exactly_the_pinned_branches_still_where_they_were() {
    let f = boot().await;
    let (_, offer) = branch_unpushed(f.addr, "drifted").await;
    let body = serde_json::json!({ "branches": {
        "current": { "delete": true, "branches": [ {
            "name": offer["current"]["name"], "oid": offer["current"]["oid"] } ] },
        "others": { "delete": true, "branches": offer["others"].as_array().unwrap().iter()
            .map(|o| serde_json::json!({ "name": o["name"], "oid": o["oid"] }))
            .collect::<Vec<_>>() },
    } });
    // `develop` moves after the dialog read it.
    git(
        &f.repo,
        &[
            "update-ref",
            "refs/heads/develop",
            "refs/heads/develop-next",
        ],
    );

    let (mut ws, connection) = events_socket(f.addr).await;
    assert_eq!(
        delete_with_body(f.addr, "drifted", &body, &connection).await,
        204
    );
    let status = wait_for_delete_status(&mut ws, "drifted").await;
    assert!(
        status.contains("deleted its branch \"develop-next\"")
            && status.contains("\"develop\" was kept because it moved after you were shown it"),
        "{status}"
    );
    let listed = branches(&f.repo);
    assert!(!listed.contains(&"develop-next".to_string()), "{listed:?}");
    assert!(listed.contains(&"develop".to_string()), "{listed:?}");
}

/// A ticked box that pins nothing, or pins a short commit, is refused whole:
/// a malformed answer is never read as a delete without a pin.
#[tokio::test]
async fn a_malformed_branch_answer_is_refused_and_deletes_nothing() {
    let f = boot().await;
    let (_, connection) = events_socket(f.addr).await;
    for body in [
        serde_json::json!({ "branches": { "current": { "delete": true, "branches": [] } } }),
        serde_json::json!({ "branches": { "current": { "delete": true,
            "branches": [ { "name": "develop-next", "oid": "abc123" } ] } } }),
    ] {
        assert_eq!(
            delete_with_body(f.addr, "drifted", &body, &connection).await,
            400,
            "{body}"
        );
    }
    let (status, _) = branch_unpushed(f.addr, "drifted").await;
    assert_eq!(status, 200, "the agent is still there");
}

async fn branch_pushed(addr: SocketAddr, id: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::get(format!("http://{addr}/api/v1/sessions/{id}/branch-pushed"))
        .await
        .expect("send the request");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// A rename, scoped to an events connection so its statuses come back on it.
async fn rename(
    addr: SocketAddr,
    id: &str,
    body: serde_json::Value,
    connection: &str,
) -> (u16, String) {
    let resp = reqwest::Client::new()
        .patch(format!("http://{addr}/api/v1/sessions/{id}"))
        .header("x-connection-id", connection)
        .json(&body)
        .send()
        .await
        .expect("send the request");
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap_or_default())
}

async fn session_title(addr: SocketAddr, id: &str) -> serde_json::Value {
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/v1/sessions/{id}"))
        .await
        .expect("send the request")
        .json()
        .await
        .expect("the response body is JSON");
    body["title"].clone()
}

/// The rename dialog's warning reads whether the branch has a pushed copy, from
/// this clone's refs; a standalone agent has no branch and is not answered.
#[tokio::test]
async fn branch_pushed_reads_the_clones_remote_tracking_refs() {
    let f = boot().await;
    let (status, body) = branch_pushed(f.addr, "duxs").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["pushed"], false, "{body}");

    git(&f.repo, &["remote", "add", "origin", "/nowhere"]);
    git(
        &f.repo,
        &["update-ref", "refs/remotes/origin/dux/made-this", "HEAD"],
    );
    let (_, body) = branch_pushed(f.addr, "duxs").await;
    assert_eq!(body["pushed"], true, "{body}");

    assert_eq!(branch_pushed(f.addr, "folder").await.0, 404);
    assert_eq!(branch_pushed(f.addr, "nobody").await.0, 404);
}

/// With the box ticked the web rename runs `git branch -m` through the same
/// engine path the terminal UI uses: the branch moves, the final says both
/// names, and no custom title is left behind, so the label follows the branch.
#[tokio::test]
async fn a_rename_with_the_branch_box_renames_the_git_branch() {
    let f = boot().await;
    let (mut ws, connection) = events_socket(f.addr).await;
    let (status, body) = rename(
        f.addr,
        "duxs",
        serde_json::json!({ "title": "dux/renamed", "rename_branch": true }),
        &connection,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let frame = next_frame(&mut ws, |v| {
        v["event"] == "status"
            && v["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("Renamed agent and its git branch"))
    })
    .await
    .expect("the branch rename reports its outcome on the events socket");
    assert_eq!(
        frame["message"],
        "Renamed agent and its git branch to \"dux/renamed\". The branch was called \"dux/made-this\"."
    );
    let listed = branches(&f.repo);
    assert!(listed.contains(&"dux/renamed".to_string()), "{listed:?}");
    assert!(!listed.contains(&"dux/made-this".to_string()), "{listed:?}");
    assert_eq!(session_title(f.addr, "duxs").await, serde_json::Value::Null);
}

/// An empty name cannot be a branch: the route refuses with the terminal UI's
/// sentence and renames nothing.
#[tokio::test]
async fn a_branch_rename_to_an_empty_name_is_refused() {
    let f = boot().await;
    let (_ws, connection) = events_socket(f.addr).await;
    let (status, body) = rename(
        f.addr,
        "duxs",
        serde_json::json!({ "title": "  ", "rename_branch": true }),
        &connection,
    )
    .await;
    assert_eq!(status, 400);
    assert!(
        body.contains("untick \"Also rename the git branch\""),
        "{body}"
    );
    assert!(branches(&f.repo).contains(&"dux/made-this".to_string()));
    assert_eq!(session_title(f.addr, "duxs").await, "duxs");
}

/// Without the box the branch is left alone, and a standalone agent's name is
/// held to the agent-name rules, whatever the box says.
#[tokio::test]
async fn a_rename_without_the_box_leaves_the_branch_alone() {
    let f = boot().await;
    let (_ws, connection) = events_socket(f.addr).await;
    let (status, body) = rename(
        f.addr,
        "duxs",
        serde_json::json!({ "title": "shiny" }),
        &connection,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(session_title(f.addr, "duxs").await, "shiny");
    assert!(branches(&f.repo).contains(&"dux/made-this".to_string()));

    let (status, body) = rename(
        f.addr,
        "folder",
        serde_json::json!({ "title": "My Notes (2026)", "rename_branch": true }),
        &connection,
    )
    .await;
    assert_eq!(status, 400, "{body}");

    let (status, body) = rename(
        f.addr,
        "folder",
        serde_json::json!({ "title": "My-Notes-2026", "rename_branch": true }),
        &connection,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(session_title(f.addr, "folder").await, "My-Notes-2026");
}
