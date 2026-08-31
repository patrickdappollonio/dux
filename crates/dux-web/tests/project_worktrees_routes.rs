//! End-to-end tests for the project worktree-manager routes against a real
//! router, a real git repository and real worktrees in a temp dir:
//!
//! - `GET    /api/v1/projects/:id/worktrees`      — the listing, now carrying the
//!   dirty flag and the holding agent's id.
//! - `DELETE /api/v1/projects/:id/worktrees?path=` — remove one managed worktree.
//! - `GET    /api/v1/projects/worktree-counts`     — per-project managed counts
//!   for the project picker's row labels.
//!
//! The delete route's refusals are the point of most of this file: a worktree an
//! agent is holding, and a worktree that is not a managed worktree of that
//! project, must both be refused at the ROUTE, not only in the UI.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
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

fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let n = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: Some("held-agent".to_string()),
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
            branch_name: "held".to_string(),
            initial_branch: "held".to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

struct Fixture {
    addr: SocketAddr,
    _tmp: tempfile::TempDir,
    /// An adoptable, clean managed worktree.
    free: PathBuf,
    /// An adoptable managed worktree holding an untracked file.
    dirty: PathBuf,
    /// A managed worktree an agent (`s1`) is attached to.
    held: PathBuf,
    /// A worktree of the same repo that lives OUTSIDE dux's managed root.
    external: PathBuf,
}

/// Boot a server over a real repo (project `p1`, name `repo`) with three managed
/// worktrees plus one external one, and a single session holding `held`.
async fn boot() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    // Managed worktrees live under `<worktrees_root>/<project name>/`, which is
    // exactly what `classify_project_worktrees` keys "managed" off.
    let managed = root.join("worktrees").join("repo");
    std::fs::create_dir_all(&managed).unwrap();
    let free = managed.join("free");
    let dirty = managed.join("dirty");
    let held = managed.join("held");
    let external = root.join("outside").join("ext");
    for (path, branch) in [
        (&free, "free"),
        (&dirty, "dirty"),
        (&held, "held"),
        (&external, "ext"),
    ] {
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_string_lossy().as_ref(),
            ],
        );
    }
    std::fs::write(dirty.join("scratch.txt"), "unsaved work").unwrap();

    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
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
            .create_session(&sample_session("s1", held.to_string_lossy().as_ref()))
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

    Fixture {
        addr,
        _tmp: tmp,
        free,
        dirty,
        held,
        external,
    }
}

async fn list(addr: SocketAddr, project: &str) -> (u16, serde_json::Value) {
    let resp = reqwest::get(format!("http://{addr}/api/v1/projects/{project}/worktrees"))
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn delete(addr: SocketAddr, project: &str, path: &Path) -> (u16, String) {
    delete_with(addr, project, path, "").await
}

/// The delete reply body as JSON (the route answers 200 with a small document
/// reporting what happened to the branch).
fn reply(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body is not JSON ({e}): {body}"))
}

/// The delete request with extra query, e.g. `"&delete_branch=true"`.
async fn delete_with(addr: SocketAddr, project: &str, path: &Path, extra: &str) -> (u16, String) {
    let resp = reqwest::Client::new()
        .delete(format!(
            "http://{addr}/api/v1/projects/{project}/worktrees?path={}{extra}",
            // Temp-dir paths are plain ASCII with slashes, which need no
            // percent-encoding inside a query value.
            path.to_string_lossy()
        ))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap_or_default())
}

/// Whether the repo still has a local branch by this name.
fn branch_exists(repo: &Path, branch: &str) -> bool {
    let out = std::process::Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line == branch)
}

fn entry<'a>(body: &'a serde_json::Value, branch: &str) -> &'a serde_json::Value {
    body["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .find(|e| e["branch_name"] == branch)
        .unwrap_or_else(|| panic!("no entry for {branch} in {body}"))
}

#[tokio::test]
async fn listing_reports_dirtiness_and_the_holding_agent() {
    let f = boot().await;
    let (status, body) = list(f.addr, "p1").await;
    assert_eq!(status, 200, "got {body}");

    let free = entry(&body, "free");
    assert_eq!(free["adoptable"], true);
    assert_eq!(free["dirty"], false, "a clean worktree is not dirty");
    assert!(free["agent_id"].is_null());

    let dirty = entry(&body, "dirty");
    assert_eq!(
        dirty["dirty"], true,
        "an untracked file must be reported as uncommitted work"
    );

    let held = entry(&body, "held");
    assert_eq!(held["adoptable"], false);
    assert_eq!(
        held["agent_id"], "s1",
        "an attached row must name the agent holding it so the UI can point there"
    );
}

#[tokio::test]
async fn deleting_an_adoptable_worktree_removes_it_from_disk_and_from_git() {
    let f = boot().await;
    assert!(f.free.exists());
    let (status, body) = delete(f.addr, "p1", &f.free).await;
    assert_eq!(status, 200, "got {body}");
    assert!(
        reply(&body)["branch"].is_null(),
        "no branch deletion was asked for, so the reply claims nothing: {body}"
    );
    assert!(!f.free.exists(), "the worktree directory must be gone");

    // Gone from the listing too, which also proves git's registry was updated
    // rather than the directory merely unlinked.
    let (_, body) = list(f.addr, "p1").await;
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["branch_name"] != "free"),
        "the removed worktree must be gone from the listing: {body}"
    );
}

#[tokio::test]
async fn deleting_a_worktree_does_not_delete_its_branch() {
    // The user asked to remove a WORKTREE. Deleting the branch is a second act
    // of destruction (`git branch -D` force-deletes unmerged commits), so the
    // route must not do it.
    let f = boot().await;
    let (status, body) = delete(f.addr, "p1", &f.free).await;
    assert_eq!(status, 200, "got {body}");
    let out = std::process::Command::new("git")
        .args(["branch", "--list", "free"])
        .current_dir(f._tmp.path().join("repo"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("free"),
        "the branch must survive the worktree removal"
    );
}

#[tokio::test]
async fn deleting_a_worktree_with_delete_branch_removes_the_branch_too() {
    // The other half of the same journey: the manager's confirmation carries a
    // checkbox, and when the user leaves it on the branch goes with the
    // worktree. Without this a deleted worktree leaves its branch behind, and
    // recreating an agent under that name fails with "branch already exists".
    let f = boot().await;
    let repo = f._tmp.path().join("repo");
    assert!(branch_exists(&repo, "free"));

    let (status, body) = delete_with(f.addr, "p1", &f.free, "&delete_branch=true").await;

    assert_eq!(status, 200, "got {body}");
    let branch = &reply(&body)["branch"];
    assert_eq!(branch["name"], "free", "got {body}");
    assert_eq!(
        branch["outcome"], "deleted",
        "the reply must report the OUTCOME, which is what the toast reads: {body}"
    );
    assert!(!f.free.exists(), "the worktree directory must be gone");
    assert!(
        !branch_exists(&repo, "free"),
        "the branch must be gone when the request asked for it"
    );
}

#[tokio::test]
async fn deleting_a_worktree_reports_a_branch_git_refused_to_delete() {
    // `git branch -D` does not only fail when the branch is already gone: it
    // also refuses a branch that is CHECKED OUT somewhere. The route used to
    // answer a bare 204 and the client toasted "and deleted its branch" off its
    // own checkbox, which is the exact opposite of what happened. The reply now
    // carries the outcome, and the branch really does survive.
    //
    // The state is built deterministically with plumbing: pointing the source
    // checkout's HEAD at `free` is what makes git refuse, and it is the same
    // refusal a user reaches by having the branch checked out elsewhere.
    let f = boot().await;
    let repo = f._tmp.path().join("repo");
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/free"]);

    let (status, body) = delete_with(f.addr, "p1", &f.free, "&delete_branch=true").await;

    assert_eq!(status, 200, "got {body}");
    let branch = &reply(&body)["branch"];
    assert_eq!(branch["name"], "free", "got {body}");
    assert_eq!(branch["outcome"], "refused", "got {body}");
    assert!(
        branch["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("free"),
        "git's own reason must reach the client: {body}"
    );
    assert!(
        !f.free.exists(),
        "the worktree still goes; only the branch deletion was refused"
    );
    assert!(
        branch_exists(&repo, "free"),
        "the refused branch is still there, which is what the reply now says"
    );
}

#[tokio::test]
async fn deleting_a_worktree_with_delete_branch_false_keeps_the_branch() {
    // An explicit `false` must behave exactly like the absent parameter, or the
    // checkbox has no off position.
    let f = boot().await;
    let repo = f._tmp.path().join("repo");

    let (status, body) = delete_with(f.addr, "p1", &f.free, "&delete_branch=false").await;

    assert_eq!(status, 200, "got {body}");
    assert!(
        reply(&body)["branch"].is_null(),
        "nothing was attempted, so the reply reports no branch outcome: {body}"
    );
    assert!(branch_exists(&repo, "free"), "the branch must survive");
}

#[tokio::test]
async fn deleting_a_detached_worktree_with_delete_branch_still_works() {
    // Detaching HEAD inside a worktree is an ordinary thing to do, and the
    // client sends no `delete_branch` for such a row. A request that asks for it
    // anyway (a hand-written one, a stale tab) must not be an error: there is
    // simply no branch to delete, so the worktree goes and nothing else does.
    let f = boot().await;
    let repo = f._tmp.path().join("repo");
    let loose = f._tmp.path().join("worktrees").join("repo").join("loose");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            loose.to_string_lossy().as_ref(),
        ],
    );

    let (status, body) = list(f.addr, "p1").await;
    assert_eq!(status, 200, "got {body}");
    let detached = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["worktree_path"].as_str() == Some(loose.to_string_lossy().as_ref()))
        .unwrap_or_else(|| panic!("no entry for the detached worktree in {body}"));
    assert!(
        detached["branch"].is_null(),
        "a detached worktree has no branch: {detached}"
    );

    let (status, body) = delete_with(f.addr, "p1", &loose, "&delete_branch=true").await;

    assert_eq!(status, 200, "got {body}");
    assert!(
        reply(&body)["branch"].is_null(),
        "a detached worktree has no branch, so nothing is claimed: {body}"
    );
    assert!(!loose.exists(), "the worktree directory must be gone");
    assert!(
        branch_exists(&repo, "main"),
        "nothing else may be deleted in its place"
    );
}

#[tokio::test]
async fn the_listing_publishes_the_real_branch_so_the_checkbox_knows_it_exists() {
    // The row label falls back to "detached <sha>" for a branchless worktree, so
    // the client cannot tell a branch from a label. `branch` is the real answer,
    // and it is what decides whether the delete confirmation offers a checkbox.
    let f = boot().await;
    let (status, body) = list(f.addr, "p1").await;
    assert_eq!(status, 200, "got {body}");
    assert_eq!(entry(&body, "free")["branch"], "free");
}

#[tokio::test]
async fn deleting_an_attached_worktree_is_refused() {
    // Defence in depth: the UI offers no delete on an attached row, but the
    // route must refuse it too. Deleting a worktree from under a live agent
    // leaves a broken session; deleting the agent is the supported route.
    let f = boot().await;
    let (status, body) = delete(f.addr, "p1", &f.held).await;
    assert_eq!(status, 409, "got {body}");
    assert!(f.held.exists(), "the attached worktree must survive");
}

#[tokio::test]
async fn deleting_a_worktree_outside_the_project_is_refused() {
    let f = boot().await;
    // A real worktree of the same repo, but not under dux's managed root: not
    // dux's to remove.
    let (status, body) = delete(f.addr, "p1", &f.external).await;
    assert_eq!(status, 404, "got {body}");
    assert!(f.external.exists());

    // And a path that is not a worktree at all.
    let (status, _) = delete(f.addr, "p1", Path::new("/tmp/definitely-not-a-worktree")).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn deleting_under_an_unknown_project_is_404() {
    let f = boot().await;
    let (status, _) = delete(f.addr, "nope", &f.free).await;
    assert_eq!(status, 404);
    assert!(f.free.exists());
}

#[tokio::test]
async fn deleting_without_a_path_is_400() {
    let f = boot().await;
    let resp = reqwest::Client::new()
        .delete(format!("http://{}/api/v1/projects/p1/worktrees", f.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn worktree_counts_reports_every_project() {
    let f = boot().await;
    let resp = reqwest::get(format!("http://{}/api/v1/projects/worktree-counts", f.addr))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Three managed worktrees; the external one and the source checkout are not
    // counted, because neither is something the manager can show.
    assert_eq!(body["counts"]["p1"], 3, "got {body}");
    assert!(f.dirty.exists());
}
