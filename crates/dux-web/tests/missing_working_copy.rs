//! An agent that deletes its own working copy, end to end.
//!
//! The case is real: a coding CLI merges its branch, removes the worktree it is
//! running inside, and the directory dux polls is gone. What the server must not
//! do is keep reporting that as a git error once per cycle; what it must do is
//! answer with the working copy's own verdict, and offer the way back.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{RouterParams, build_app};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn run_git(cwd: &Path, args: &[&str]) {
    let out = dux_core::test_git::fixture_git()
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn sample_session(id: &str, worktree: &str, branch: &str) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            initial_branch: branch.to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

/// A server with one managed agent in a REAL worktree of a real project
/// repository, so the recreate has a repository to check the branch out from.
async fn boot() -> (
    SocketAddr,
    dux_core::test_scratch::ScratchDir,
    std::path::PathBuf,
) {
    let (addr, tmp, worktree, _repo) = boot_with_repo().await;
    (addr, tmp, worktree)
}

/// The same server, with the project repository's path handed back too, for the
/// tests that need to look at what git holds.
async fn boot_with_repo() -> (
    SocketAddr,
    dux_core::test_scratch::ScratchDir,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main", "."]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    run_git(&repo, &["add", "seed.txt"]);
    run_git(&repo, &["commit", "-qm", "init"]);

    let worktree = root.join("worktrees").join("repo").join("feat");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    dux_core::git::add_worktree_new_branch_at(&repo, &worktree, "feat", Some("main"))
        .expect("the agent's working copy");

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
                name: Some("p1".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .create_session(&sample_session(
                "s1",
                worktree.to_string_lossy().as_ref(),
                "feat",
            ))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    let (handle, _join) = spawn_engine_thread(engine);
    let app = build_app(handle, axum::Router::new(), RouterParams::plain_http());
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
    (addr, tmp, worktree, repo)
}

/// Poll until the server has noticed what is at the agent's directory.
///
/// Asking for the agent's changes is what refreshes the classification, exactly
/// as an open changes panel does; the answer runs off the engine thread, so the
/// first ask reports the previous verdict by design.
async fn wait_for_missing(addr: SocketAddr, missing: bool) -> serde_json::Value {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        let _ = client
            .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
            .send()
            .await;
        let body: serde_json::Value = client
            .get(format!("http://{addr}/api/v1/sessions"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let workspace = body[0]["workspace"].clone();
        if workspace["worktree_missing"] == serde_json::Value::Bool(missing) {
            return workspace;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never reported worktree_missing = {missing}");
}

/// The whole point: a directory that is gone answers with its own verdict, in a
/// sentence that names the path and never says the repository is busy, and the
/// changed-files read stays a success with nothing in it rather than a 409 per
/// poll cycle.
#[tokio::test]
async fn a_deleted_working_copy_answers_with_its_own_verdict() {
    let (addr, _tmp, worktree) = boot().await;
    let client = reqwest::Client::new();

    // A browser looking at this agent, which is what makes the poller ask about
    // it every cycle and is exactly the situation the bug was reported from.
    // Read, not merely opened: what the browser is told is half the bug, and a
    // socket nobody drains proves nothing about what travels on it.
    let (events, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let (mut sink, stream) = events.split();
    sink.send(Message::Text(
        r#"{"subscribe":["session:s1:changes","sessions"]}"#.into(),
    ))
    .await
    .unwrap();
    let frames = collect_frames(stream);

    // Precondition: while the directory is there, everything is ordinary.
    let resp = client
        .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The agent removes its own working copy from inside it.
    std::fs::remove_dir_all(&worktree).unwrap();

    let workspace = wait_for_missing(addr, true).await;
    let reason = workspace["quiet_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains(worktree.to_string_lossy().as_ref()),
        "the sentence names the path: {reason}"
    );
    assert!(
        !reason.to_lowercase().contains("busy"),
        "and never calls a directory that is gone a busy repository: {reason}"
    );
    assert!(
        reason.contains("recreated"),
        "and says what it takes to get the agent running again: {reason}"
    );

    // The read settles into a success with nothing in it: the panel is quiet and
    // the sentence above is what explains it. The ONE cycle that ran before dux
    // had looked is a real error and reports as one; what this pins is that it
    // does not repeat, which is the whole bug.
    let mut settled = false;
    for _ in 0..200 {
        let resp = client
            .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
            .send()
            .await
            .unwrap();
        if resp.status() == 200 {
            let body: serde_json::Value = resp.json().await.unwrap();
            assert!(body["staged"].as_array().unwrap().is_empty());
            assert!(body["unstaged"].as_array().unwrap().is_empty());
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        settled,
        "a directory that is gone must stop being reported as a git error"
    );
    for _ in 0..5 {
        let resp = client
            .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "and must stay settled");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Several more poll cycles with nobody touching anything: whatever the
    // browser is told about this agent has to stop too.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let seen = frames.lock().unwrap().clone();

    let statuses: Vec<&serde_json::Value> = seen
        .iter()
        .filter(|f| f["event"] == "status" && f["tone"] != "busy")
        .collect();
    assert!(
        statuses.len() <= 1,
        "a directory that is gone is one transition, not one message per poll \
         cycle; got {statuses:#?}"
    );
    for status in &statuses {
        let message = status["message"].as_str().unwrap_or_default();
        assert!(
            !message.to_lowercase().contains("busy"),
            "and never calls it a busy repository: {message}"
        );
    }

    // And the row marker travels: the browser learns the working copy is gone
    // from the pushed workspace, not only by asking for it.
    let marked = seen.iter().any(|f| {
        f["event"] == "workspace"
            && f["workspace"]["sessions"]
                .as_array()
                .is_some_and(|sessions| {
                    sessions
                        .iter()
                        .any(|s| s["id"] == "s1" && s["workspace"]["worktree_missing"] == true)
                })
    });
    assert!(
        marked,
        "the row marker must reach an open browser on the wire; frames: {seen:#?}"
    );
}

/// Drain an events socket into a shared list, so a test can look at everything
/// the server said rather than waiting for one frame it expects.
fn collect_frames<S>(mut stream: S) -> std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&frames);
    tokio::spawn(async move {
        while let Some(Ok(frame)) = stream.next().await {
            let Ok(text) = frame.into_text() else {
                continue;
            };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                sink.lock().unwrap().push(value);
            }
        }
    });
    frames
}

/// The way out: the branch is still in the repository, so the working copy comes
/// back at the SAME path, which is what lets the CLI find its conversation.
#[tokio::test]
async fn recreating_the_working_copy_puts_it_back_at_the_same_path() {
    let (addr, _tmp, worktree) = boot().await;
    let client = reqwest::Client::new();
    std::fs::remove_dir_all(&worktree).unwrap();
    wait_for_missing(addr, true).await;

    let resp = client
        .post(format!(
            "http://{addr}/api/v1/sessions/s1/recreate-working-copy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    for _ in 0..100 {
        if worktree.join("seed.txt").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        worktree.join("seed.txt").exists(),
        "the working copy is back at {}",
        worktree.display()
    );
    wait_for_missing(addr, false).await;
}

/// An id with nothing to recreate is refused in a sentence rather than acted on:
/// the same act is a menu item, a palette command and this route.
#[tokio::test]
async fn recreating_a_working_copy_that_is_there_is_refused() {
    let (addr, _tmp, _worktree) = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!(
            "http://{addr}/api/v1/sessions/s1/recreate-working-copy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("only recreates a working copy it manages"),
        "{body}"
    );

    // An id nobody has is a 404, not a sentence about an agent that is not
    // there.
    let unknown = client
        .post(format!(
            "http://{addr}/api/v1/sessions/nope/recreate-working-copy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

/// The editor is another door onto the same directory. It used to open its own
/// hole and answer with whatever the filesystem said, which named no path and
/// offered no way back; it now refuses with the working copy's own sentence.
#[tokio::test]
async fn the_editor_refuses_a_directory_that_is_gone_in_the_same_sentence() {
    let (addr, _tmp, worktree) = boot().await;
    let client = reqwest::Client::new();

    // Precondition: the tree is browsable while the directory is there.
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/list"))
        .json(&serde_json::json!({ "path": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    std::fs::remove_dir_all(&worktree).unwrap();
    wait_for_missing(addr, true).await;

    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/list"))
        .json(&serde_json::json!({ "path": "" }))
        .send()
        .await
        .unwrap();
    // 409 like the git routes: the agent exists and the route is real, and only
    // the directory cannot answer.
    assert_eq!(resp.status(), 409);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains(worktree.to_string_lossy().as_ref()),
        "the refusal names the path: {body}"
    );
    assert!(body.contains("recreated"), "and the way back: {body}");

    // The write door is shut too, not only the read one.
    let resp = client
        .post(format!("http://{addr}/api/v1/sessions/s1/files/write"))
        .json(&serde_json::json!({ "path": "seed.txt", "content": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

/// Recreating one agent's working copy must not sever another's. Measured on
/// git 2.55: `git worktree prune` removes EVERY registration whose directory is
/// unreachable at that instant, so a sibling agent on a mount that is down lost
/// its registration permanently and `git status` in its restored directory
/// answered that it is not a git repository.
#[tokio::test]
async fn recreating_one_working_copy_leaves_an_unreachable_sibling_alone() {
    let (addr, _tmp, worktree, repo) = boot_with_repo().await;
    let client = reqwest::Client::new();

    let sibling = worktree.parent().unwrap().join("other");
    dux_core::git::add_worktree_new_branch_at(&repo, &sibling, "other", Some("main"))
        .expect("a sibling agent's working copy");
    let stashed = worktree.parent().unwrap().join("other-unreachable");

    std::fs::remove_dir_all(&worktree).unwrap();
    std::fs::rename(&sibling, &stashed).expect("the sibling's mount goes away");
    wait_for_missing(addr, true).await;

    let resp = client
        .post(format!(
            "http://{addr}/api/v1/sessions/s1/recreate-working-copy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    wait_for_missing(addr, false).await;

    std::fs::rename(&stashed, &sibling).expect("the sibling's mount comes back");
    let listed = dux_core::test_git::fixture_git()
        .args(["-C", &repo.to_string_lossy(), "worktree", "list"])
        .output()
        .expect("git runs");
    let listing = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listing.contains("[other]"),
        "the sibling's registration survived: {listing}"
    );
    let status = dux_core::test_git::fixture_git()
        .args(["-C", &sibling.to_string_lossy(), "status", "--porcelain=v1"])
        .output()
        .expect("git runs");
    assert!(
        status.status.success(),
        "and its directory still works: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}
