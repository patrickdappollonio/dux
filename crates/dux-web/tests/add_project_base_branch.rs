//! User journeys for the add-project dialog's "Check out the default branch
//! before adding" box, against a real router, a real engine, real SQLite and a
//! real git repository whose `origin` is a local bare repository.
//!
//! The repository's remote default (`origin/HEAD`) is `main`, but the working
//! copy sits on `feature`, which carries a commit `main` does not have. That
//! commit is how each journey tells which branch a new agent's worktree was
//! started from: the dialog promises "New worktrees will branch from ..." and
//! these tests hold it to that sentence.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dux_core::config::{DuxPaths, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    addr: SocketAddr,
    tmp: dux_core::test_scratch::ScratchDir,
    repo: PathBuf,
    /// The commit only `feature` has.
    feature_commit: String,
}

impl Fixture {
    fn db_path(&self) -> PathBuf {
        self.tmp.path().join("dux").join("sessions.sqlite3")
    }
}

/// A clone of a bare `origin` whose HEAD names `main`, checked out on `feature`
/// with one commit of its own, pushed so the pre-create pull has an upstream.
fn repo_on_a_feature_branch(root: &Path) -> (PathBuf, String) {
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@example.com"]);
    git(&seed, &["config", "user.name", "Test"]);
    std::fs::write(seed.join("README.md"), "base\n").unwrap();
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-q", "-m", "base"]);

    let origin = root.join("origin.git");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_string_lossy().as_ref(),
            origin.to_string_lossy().as_ref(),
        ],
    );

    let repo = root.join("repo");
    git(
        root,
        &[
            "clone",
            "-q",
            origin.to_string_lossy().as_ref(),
            repo.to_string_lossy().as_ref(),
        ],
    );
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    // A clone records origin/HEAD, which is how dux KNOWS the default branch.
    assert_eq!(
        git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );
    git(&repo, &["switch", "-q", "-c", "feature"]);
    std::fs::write(repo.join("feature.txt"), "only on feature\n").unwrap();
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-q", "-m", "feature work"]);
    git(&repo, &["push", "-q", "-u", "origin", "feature"]);
    let feature_commit = git(&repo, &["rev-parse", "HEAD"]);
    (repo, feature_commit)
}

async fn boot() -> Fixture {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let (repo, feature_commit) = repo_on_a_feature_branch(tmp.path());

    let state = tmp.path().join("dux");
    let paths = DuxPaths {
        root: state.clone(),
        config_path: state.join("config.toml"),
        sessions_db_path: state.join("sessions.sqlite3"),
        worktrees_root: state.join("worktrees"),
        lock_path: state.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    // The agent CLI is the one thing that cannot run here; `cat` stands in for
    // it so the create journey spawns a real PTY.
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
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
        tmp,
        repo,
        feature_commit,
    }
}

/// Add the repository through the real route and return the new project's id,
/// waiting out the checkout worker when the box was ticked.
async fn add_project(f: &Fixture, checkout_default: bool) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/api/v1/projects", f.addr))
        .json(&serde_json::json!({
            "path": f.repo.to_string_lossy(),
            "name": "repo",
            "checkout_default": checkout_default,
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(status.is_success(), "add must succeed, got {status}");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let projects: serde_json::Value =
            reqwest::get(format!("http://{}/api/v1/projects", f.addr))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        if let Some(project) = projects.as_array().and_then(|list| list.first()) {
            return project["id"].as_str().unwrap().to_string();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the project never appeared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Create an agent in the project through the real route and return its
/// worktree path once the engine has one on disk.
async fn create_agent(f: &Fixture, project_id: &str) -> PathBuf {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/api/v1/sessions", f.addr))
        .json(&serde_json::json!({"kind": "new", "project_id": project_id, "name": "agent-one"}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_success(),
        "create must succeed, got {status}: {}",
        resp.text().await.unwrap_or_default()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let sessions: serde_json::Value =
            reqwest::get(format!("http://{}/api/v1/sessions", f.addr))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        let worktree = sessions.as_array().and_then(|list| {
            list.iter().find_map(|s| {
                s["workspace"]["worktree_path"]
                    .as_str()
                    .map(PathBuf::from)
                    .filter(|path| path.join(".git").exists())
            })
        });
        if let Some(worktree) = worktree {
            return worktree;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the agent's worktree never appeared: {sessions}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn contains_commit(worktree: &Path, commit: &str) -> bool {
    dux_core::test_git::fixture_git()
        .args(["merge-base", "--is-ancestor", commit, "HEAD"])
        .current_dir(worktree)
        .status()
        .unwrap()
        .success()
}

fn stored_base(f: &Fixture, project_id: &str) -> Option<String> {
    SessionStore::open(&f.db_path())
        .unwrap()
        .load_projects()
        .unwrap()
        .into_iter()
        .find(|p| p.id == project_id)
        .and_then(|p| p.leading_branch)
}

#[tokio::test]
async fn adding_without_the_checkout_branches_new_worktrees_from_the_current_branch() {
    let f = boot().await;

    let project_id = add_project(&f, false).await;

    assert_eq!(
        stored_base(&f, &project_id).as_deref(),
        Some("feature"),
        "an unticked box records the branch the folder is on as the project's base"
    );
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        contains_commit(&worktree, &f.feature_commit),
        "the dialog said new worktrees branch from \"feature\", so the worktree must carry its commit"
    );
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature",
        "the user's folder stays where it was"
    );
}

#[tokio::test]
async fn adding_with_the_checkout_branches_new_worktrees_from_the_default_branch() {
    let f = boot().await;

    let project_id = add_project(&f, true).await;

    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "main",
        "a ticked box checks the default branch out in the user's folder"
    );
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("main"));
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        !contains_commit(&worktree, &f.feature_commit),
        "new worktrees branch from \"main\", which does not have the feature commit"
    );
}

#[tokio::test]
async fn checking_out_the_default_branch_later_moves_the_project_base_to_it() {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/projects/{project_id}/checkout-default",
            f.addr
        ))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while stored_base(&f, &project_id).as_deref() != Some("main") {
        assert!(
            std::time::Instant::now() < deadline,
            "the project's base never moved to the default branch"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(git(&f.repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        !contains_commit(&worktree, &f.feature_commit),
        "after checking out the default branch, new worktrees branch from it"
    );
}

/// The confirmation a person reads in the browser names the branch and the
/// project, and it arrives over the real events socket carrying the parts it was
/// built from, so the toast draws both as chips. The plain sentence beside them
/// is the terminal UI's, byte for byte, quotes included.
#[tokio::test]
async fn the_checkout_confirmation_reaches_the_browser_with_its_names_as_parts() {
    use futures_util::StreamExt;

    let f = boot().await;
    let project_id = add_project(&f, false).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws/events", f.addr))
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/projects/{project_id}/checkout-default",
            f.addr
        ))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let frame = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the checkout never reported its outcome on the events socket"
        );
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
        if value["event"] == "status"
            && value["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("Checked out"))
        {
            break value;
        }
    };

    assert_eq!(
        frame["message"],
        "Checked out \"main\" for project \"repo\". New worktrees branch from \"main\" now."
    );
    assert_eq!(
        frame["segments"],
        serde_json::json!([
            "Checked out ",
            {"name": "main", "quoted": true},
            " for project ",
            {"name": "repo", "quoted": true},
            ". New worktrees branch from ",
            {"name": "main", "quoted": true},
            " now."
        ])
    );
}

// ── Change base branch ──────────────────────────────────────────────────────
//
// The project is added on `feature`, so its base is `feature`. Each journey then
// moves the base elsewhere through the real routes and holds the result to what
// the confirmation promises: the folder is on the branch, SQLite records it as
// the base, and a new agent's worktree starts from it.

/// Create local branch `branch` from `main` with a commit of its own touching
/// `file`, leaving the folder on `feature`. Returns that commit.
fn local_branch_off_main(repo: &Path, branch: &str, file: &str) -> String {
    git(repo, &["switch", "-q", "-c", branch, "main"]);
    std::fs::write(repo.join(file), format!("only on {branch}\n")).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", &format!("{branch} work")]);
    let commit = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["switch", "-q", "feature"]);
    commit
}

async fn list_branches(f: &Fixture, project_id: &str) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = reqwest::get(format!(
        "http://{}/api/v1/projects/{project_id}/branches",
        f.addr
    ))
    .await
    .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
    (status, body)
}

fn branch_entry<'a>(listing: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    listing["branches"]
        .as_array()
        .expect("a branches array")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed: {listing}"))
}

async fn change_base(f: &Fixture, project_id: &str, branch: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/projects/{project_id}/base-branch",
            f.addr
        ))
        .json(&serde_json::json!({ "branch": branch }))
        .send()
        .await
        .unwrap()
}

async fn wait_for_base(f: &Fixture, project_id: &str, branch: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while stored_base(f, project_id).as_deref() != Some(branch) {
        assert!(
            std::time::Instant::now() < deadline,
            "the project's base never moved to {branch}: still {:?}",
            stored_base(f, project_id)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

type EventsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn events_socket(f: &Fixture) -> EventsSocket {
    tokio_tungstenite::connect_async(format!("ws://{}/ws/events", f.addr))
        .await
        .unwrap()
        .0
}

/// The first status frame on the events socket whose message starts with
/// `prefix`.
async fn status_frame(ws: &mut EventsSocket, prefix: &str) -> serde_json::Value {
    use futures_util::StreamExt;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no status starting with {prefix:?} reached the events socket"
        );
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
        if value["event"] == "status"
            && value["message"]
                .as_str()
                .is_some_and(|m| m.starts_with(prefix))
        {
            return value;
        }
    }
}

#[tokio::test]
async fn changing_the_base_to_a_local_branch_switches_the_folder_and_new_worktrees_branch_from_it()
{
    let f = boot().await;
    let develop_commit = local_branch_off_main(&f.repo, "develop", "develop.txt");
    let project_id = add_project(&f, false).await;
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));

    let (status, listing) = list_branches(&f, &project_id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{listing}");
    assert_eq!(branch_entry(&listing, "develop")["location"], "local");
    assert_eq!(
        branch_entry(&listing, "develop")["held_by"],
        serde_json::Value::Null
    );
    assert_eq!(listing["fetched"], true, "{listing}");

    let mut ws = events_socket(&f).await;
    let resp = change_base(&f, &project_id, "develop").await;
    assert!(resp.status().is_success(), "got {}", resp.status());

    let done = status_frame(&mut ws, "Checked out").await;
    assert_eq!(
        done["message"],
        "Checked out \"develop\" for project \"repo\". New worktrees branch from \"develop\" now."
    );
    wait_for_base(&f, &project_id, "develop").await;
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "develop"
    );
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        contains_commit(&worktree, &develop_commit),
        "new worktrees branch from \"develop\" now"
    );
    assert!(
        !contains_commit(&worktree, &f.feature_commit),
        "and no longer from \"feature\""
    );

    // The base is SQLite's alone: a later config write for the same project
    // still leaves it out of the portable config.
    let resp = reqwest::Client::new()
        .patch(format!("http://{}/api/v1/projects/{project_id}", f.addr))
        .json(&serde_json::json!({ "auto_reopen_agents": true }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());
    let config_path = f.tmp.path().join("dux").join("config.toml");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let saved = loop {
        let saved = std::fs::read_to_string(&config_path).unwrap_or_default();
        if saved.contains("auto_reopen_agents = true") {
            break saved;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the project settings never reached config.toml: {saved}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        !saved.contains("leading_branch") && !saved.contains("develop"),
        "the base must never be written to config: {saved}"
    );
}

#[tokio::test]
async fn changing_the_base_to_a_branch_only_origin_has_creates_the_local_branch_even_without_guessing()
 {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    // Someone pushes `release` after this clone was made, so only the listing's
    // fetch can bring it in; and guessing is off, so only an explicit tracking
    // branch can check it out.
    let seed = f.tmp.path().join("seed");
    git(&seed, &["switch", "-q", "-c", "release"]);
    std::fs::write(seed.join("release.txt"), "only on release\n").unwrap();
    git(&seed, &["add", "release.txt"]);
    git(&seed, &["commit", "-q", "-m", "release work"]);
    let release_commit = git(&seed, &["rev-parse", "HEAD"]);
    let origin = f.tmp.path().join("origin.git");
    git(
        &seed,
        &["push", "-q", origin.to_string_lossy().as_ref(), "release"],
    );
    git(&f.repo, &["config", "checkout.guess", "false"]);

    let (status, listing) = list_branches(&f, &project_id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{listing}");
    assert_eq!(branch_entry(&listing, "release")["location"], "remote");

    let resp = change_base(&f, &project_id, "release").await;
    assert!(resp.status().is_success(), "got {}", resp.status());
    wait_for_base(&f, &project_id, "release").await;

    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "release"
    );
    assert_eq!(
        git(
            &f.repo,
            &["rev-parse", "--abbrev-ref", "release@{upstream}"]
        ),
        "origin/release",
        "the local branch tracks the one on origin"
    );
    let worktree = create_agent(&f, &project_id).await;
    assert!(contains_commit(&worktree, &release_commit));
}

#[tokio::test]
async fn a_folder_with_conflicting_changes_refuses_the_switch_and_keeps_the_base() {
    let f = boot().await;
    local_branch_off_main(&f.repo, "develop", "README.md");
    let project_id = add_project(&f, false).await;
    // An uncommitted edit to the very file `develop` changes: git will not
    // carry it across the switch.
    std::fs::write(f.repo.join("README.md"), "edited in the folder\n").unwrap();

    let mut ws = events_socket(&f).await;
    let resp = change_base(&f, &project_id, "develop").await;
    assert!(resp.status().is_success(), "got {}", resp.status());

    let frame = status_frame(&mut ws, "Couldn't check out").await;
    assert_eq!(
        frame["message"],
        format!(
            "Couldn't check out \"develop\" in {}. Resolve in your terminal and retry.",
            f.repo.display()
        )
    );
    assert_eq!(frame["tone"], "error");
    assert_eq!(frame["sticky"], true, "the fix is outside the toast");
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
}

#[tokio::test]
async fn a_branch_an_agent_holds_is_listed_as_held_and_refused_if_posted() {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    let worktree = create_agent(&f, &project_id).await;

    let (status, listing) = list_branches(&f, &project_id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{listing}");
    let held = branch_entry(&listing, "agent-one");
    let holder = PathBuf::from(held["held_by"].as_str().expect("a holder"));
    assert_eq!(
        holder.canonicalize().unwrap(),
        worktree.canonicalize().unwrap(),
        "{listing}"
    );
    assert_eq!(
        branch_entry(&listing, "feature")["held_by"],
        serde_json::Value::Null,
        "the branch the project folder is on is not held by anything else"
    );

    let mut ws = events_socket(&f).await;
    let resp = change_base(&f, &project_id, "agent-one").await;
    assert!(resp.status().is_success(), "got {}", resp.status());
    let frame = status_frame(&mut ws, "Can't change the base branch").await;
    assert_eq!(
        frame["message"],
        format!(
            "Can't change the base branch of project \"repo\" to \"agent-one\": it is checked \
             out in the worktree at {}, and git checks a branch out in one place at a time. \
             Pick another branch, or remove that worktree first.",
            holder.display()
        )
    );
    assert_eq!(frame["tone"], "error");
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
}

#[tokio::test]
async fn the_branch_listing_survives_a_failed_fetch_and_says_so() {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    // origin is gone: a local path that does not exist, so no network is tried.
    let gone = f.tmp.path().join("gone.git");
    git(
        &f.repo,
        &[
            "remote",
            "set-url",
            "origin",
            gone.to_string_lossy().as_ref(),
        ],
    );

    let (status, listing) = list_branches(&f, &project_id).await;

    assert_eq!(status, reqwest::StatusCode::OK, "{listing}");
    assert_eq!(listing["fetched"], false, "{listing}");
    assert!(
        listing["fetch_error"]
            .as_str()
            .is_some_and(|e| e.contains("git fetch origin failed")),
        "{listing}"
    );
    assert_eq!(branch_entry(&listing, "feature")["location"], "local");
    assert_eq!(branch_entry(&listing, "main")["location"], "local");
}

#[tokio::test]
async fn both_base_branch_routes_refuse_a_project_whose_folder_is_gone() {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    std::fs::rename(&f.repo, f.tmp.path().join("moved-away")).unwrap();

    let (status, body) = list_branches(&f, &project_id).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body,
        serde_json::Value::String(
            "Cannot change base branch: path not found for \"repo\"".to_string()
        )
    );

    // The engine may not have noticed the folder is gone yet, so the change is
    // accepted and its worker is the one that finds out, in the same words.
    let mut ws = events_socket(&f).await;
    let resp = change_base(&f, &project_id, "main").await;
    let accepted = resp.status();
    if accepted.is_success() {
        let frame = status_frame(&mut ws, "Cannot change base branch").await;
        assert_eq!(
            frame["message"],
            "Cannot change base branch: path not found for \"repo\""
        );
        assert_eq!(frame["tone"], "error");
    } else {
        assert_eq!(accepted, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.text().await.unwrap(),
            "Cannot change base branch: path not found for \"repo\""
        );
    }
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));
}
