//! End-to-end tests for the editor's two file-management journeys added for
//! the move modal and the file-info panel.
//!
//! MOVE deliberately has no route of its own: a move IS a rename, so the
//! browser sends the existing `POST /api/v1/sessions/:id/files/rename` with a
//! destination in a different directory. These tests exercise it as the move
//! flow uses it (a destination directory plus the source's own basename) and
//! pin the containment answers a move can provoke that a same-directory rename
//! cannot: escaping the worktree, and landing through a symlinked directory.
//!
//! INFO is a new route, `POST /api/v1/sessions/:id/files/info`, backed by
//! `dux_core::worktree_file::entry_info`.
//!
//! Both run against a REAL router over a REAL TCP listener with a REAL git
//! repository on disk, following `file_tree_routes.rs`'s shape.

use std::net::SocketAddr;
use std::path::Path;

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

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Boot a server with one session (`s1`) whose worktree is a real git
/// repository holding a committed file, a subdirectory to move into, and a
/// file with a non-Latin name.
async fn boot() -> (SocketAddr, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt1");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::create_dir(wt.join("src")).unwrap();
    std::fs::create_dir(wt.join("dest")).unwrap();
    std::fs::write(wt.join("src/moveme.txt"), "contents\n").unwrap();
    std::fs::write(wt.join("src/файл.txt"), "текст\n").unwrap();
    git(&wt, &["init", "-b", "main"]);
    git(&wt, &["config", "user.name", "test"]);
    git(&wt, &["config", "user.email", "t@t"]);
    git(&wt, &["add", "src/moveme.txt"]);
    git(&wt, &["commit", "-m", "init"]);

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
            .upsert_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
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
    (addr, tmp, wt)
}

async fn post(
    addr: SocketAddr,
    session: &str,
    action: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/sessions/{session}/files/{action}"
        ))
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn do_move(addr: SocketAddr, from: &str, to: &str) -> reqwest::Response {
    post(
        addr,
        "s1",
        "rename",
        serde_json::json!({ "from": from, "to": to }),
    )
    .await
}

async fn get_info(addr: SocketAddr, session: &str, path: &str) -> reqwest::Response {
    post(addr, session, "info", serde_json::json!({ "path": path })).await
}

#[tokio::test]
async fn moving_a_file_into_a_sibling_directory_succeeds() {
    let (addr, _tmp, wt) = boot().await;
    let resp = do_move(addr, "src/moveme.txt", "dest/moveme.txt").await;
    assert_eq!(resp.status(), 200);
    assert!(!wt.join("src/moveme.txt").exists());
    assert_eq!(
        std::fs::read_to_string(wt.join("dest/moveme.txt")).unwrap(),
        "contents\n"
    );
}

#[tokio::test]
async fn moving_a_file_to_the_worktree_root_succeeds() {
    let (addr, _tmp, wt) = boot().await;
    let resp = do_move(addr, "src/moveme.txt", "moveme.txt").await;
    assert_eq!(resp.status(), 200);
    assert!(wt.join("moveme.txt").is_file());
}

/// A non-Latin name must survive the move byte-for-byte: dux validates names,
/// it never rewrites them.
#[tokio::test]
async fn a_non_latin_name_survives_the_move_unchanged() {
    let (addr, _tmp, wt) = boot().await;
    let resp = do_move(addr, "src/файл.txt", "dest/файл.txt").await;
    assert_eq!(resp.status(), 200);
    assert!(
        wt.join("dest/файл.txt").is_file(),
        "the destination must carry the exact original name"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("dest/файл.txt")).unwrap(),
        "текст\n"
    );
}

#[tokio::test]
async fn moving_outside_the_worktree_is_refused() {
    let (addr, _tmp, wt) = boot().await;
    let outside = wt.parent().unwrap().join("stolen.txt");
    let resp = do_move(addr, "src/moveme.txt", "../stolen.txt").await;
    assert_eq!(resp.status(), 400);
    assert!(!outside.exists(), "nothing may land outside the worktree");
    assert!(
        wt.join("src/moveme.txt").exists(),
        "a refused move must leave the source alone"
    );
}

/// A destination reached through a symlinked directory that escapes the
/// worktree must be refused rather than followed: the link is inside the tree
/// but what it points at is not.
#[tokio::test]
async fn moving_through_an_escaping_symlinked_directory_is_refused() {
    let (addr, _tmp, wt) = boot().await;
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), wt.join("escape")).unwrap();
    let resp = do_move(addr, "src/moveme.txt", "escape/moveme.txt").await;
    assert_eq!(resp.status(), 400);
    assert!(
        !outside.path().join("moveme.txt").exists(),
        "the symlink must not be followed out of the worktree"
    );
    assert!(wt.join("src/moveme.txt").exists());
}

/// The overwrite decision, pinned: a move that would land on an existing entry
/// is REFUSED outright, and the destination's bytes are untouched.
#[tokio::test]
async fn moving_onto_an_existing_file_is_refused_and_does_not_clobber_it() {
    let (addr, _tmp, wt) = boot().await;
    std::fs::write(wt.join("dest/moveme.txt"), "do not lose me\n").unwrap();
    let resp = do_move(addr, "src/moveme.txt", "dest/moveme.txt").await;
    assert_eq!(resp.status(), 400);
    assert_eq!(
        std::fs::read_to_string(wt.join("dest/moveme.txt")).unwrap(),
        "do not lose me\n",
        "a refused move must not overwrite a single byte"
    );
    assert!(wt.join("src/moveme.txt").exists());
}

#[tokio::test]
async fn moving_into_the_git_directory_is_refused() {
    let (addr, _tmp, wt) = boot().await;
    let resp = do_move(addr, "src/moveme.txt", ".git/moveme.txt").await;
    assert_eq!(resp.status(), 400);
    assert!(!wt.join(".git/moveme.txt").exists());
}

#[tokio::test]
async fn info_reports_path_size_modified_permissions_and_git_status() {
    let (addr, _tmp, wt) = boot().await;
    std::fs::write(wt.join("src/moveme.txt"), "changed contents\n").unwrap();
    let resp = get_info(addr, "s1", "src/moveme.txt").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["path"], "src/moveme.txt");
    assert_eq!(body["kind"], "file");
    assert_eq!(body["size"], 17);
    assert!(
        body["modified"].as_str().is_some_and(|s| s.contains('T')),
        "modified must be an RFC 3339 timestamp, got {:?}",
        body["modified"]
    );
    assert_eq!(body["mode"], "644");
    assert_eq!(body["permissions"], "rw-r--r--");
    assert_eq!(body["git"]["state"], "changed");
    assert_eq!(body["git"]["unstaged"], "M");
}

#[tokio::test]
async fn info_reports_a_clean_tracked_file_as_clean() {
    let (addr, _tmp, _wt) = boot().await;
    let resp = get_info(addr, "s1", "src/moveme.txt").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["git"]["state"], "clean");
}

#[tokio::test]
async fn info_reports_an_untracked_file_with_the_question_mark_code() {
    let (addr, _tmp, _wt) = boot().await;
    let resp = get_info(addr, "s1", "src/файл.txt").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["path"], "src/файл.txt");
    assert_eq!(body["git"]["state"], "changed");
    assert_eq!(body["git"]["unstaged"], "?");
}

#[tokio::test]
async fn info_on_a_directory_reports_no_size_and_no_git_state() {
    let (addr, _tmp, _wt) = boot().await;
    let resp = get_info(addr, "s1", "dest").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "dir");
    assert!(body["size"].is_null());
    assert_eq!(body["git"]["state"], "not_applicable");
}

#[tokio::test]
async fn info_refuses_the_git_directory_and_traversal() {
    let (addr, _tmp, _wt) = boot().await;
    assert_eq!(get_info(addr, "s1", ".git/config").await.status(), 400);
    assert_eq!(get_info(addr, "s1", "../secrets").await.status(), 400);
}

/// A path that is not there any more is a 404, distinct from the 400 a
/// refused path gets. The browser keys the info dialog's self-close on that
/// status: "the file vanished while you were looking at it" is a different
/// answer from "you may not look at that", and only the first should silently
/// dismiss the panel.
#[tokio::test]
async fn info_on_a_missing_file_is_404_not_400() {
    let (addr, _tmp, _wt) = boot().await;
    assert_eq!(get_info(addr, "s1", "src/gone.txt").await.status(), 404);
}

/// A file deleted underneath an OPEN info panel takes the same 404 path: this
/// is the exact journey the dialog's vanished-target guard reacts to.
#[tokio::test]
async fn info_on_a_file_deleted_underneath_the_panel_is_404() {
    let (addr, _tmp, wt) = boot().await;
    assert_eq!(get_info(addr, "s1", "src/moveme.txt").await.status(), 200);
    std::fs::remove_file(wt.join("src/moveme.txt")).unwrap();
    assert_eq!(get_info(addr, "s1", "src/moveme.txt").await.status(), 404);
}

/// An IGNORED file is listed by no `git status` at all, so before it had its
/// own state the panel reported everything under `node_modules` as tracked and
/// unmodified. The editor's tree is a plain filesystem browser with no ignore
/// filter, so that path is one right-click away.
#[tokio::test]
async fn info_reports_an_ignored_file_as_ignored_not_unmodified() {
    let (addr, _tmp, wt) = boot().await;
    std::fs::write(wt.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::create_dir(wt.join("node_modules")).unwrap();
    std::fs::write(wt.join("node_modules/a.js"), "x\n").unwrap();
    let resp = get_info(addr, "s1", "node_modules/a.js").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["git"]["state"], "ignored");
}

/// A file inside a NESTED repository is invisible to the outer repository's
/// status for the same reason, and calling a vendored subrepo "unmodified" is
/// the same lie.
#[tokio::test]
async fn info_reports_a_file_in_a_nested_repository_as_belonging_to_another() {
    let (addr, _tmp, wt) = boot().await;
    let nested = wt.join("vendor");
    std::fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-b", "main"]);
    std::fs::write(nested.join("inner.txt"), "x\n").unwrap();
    let resp = get_info(addr, "s1", "vendor/inner.txt").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["git"]["state"], "other_repository");
}

/// A DANGLING symlink escaping the worktree is refused like a live one.
/// `exists()` follows the link, so a link whose target had been removed used to
/// skip every containment check and the panel answered 200 with the target
/// path printed in full.
#[tokio::test]
async fn info_refuses_a_dangling_symlink_that_escapes_the_worktree() {
    let (addr, _tmp, wt) = boot().await;
    std::os::unix::fs::symlink("/root/.ssh/id_ed25519", wt.join("stolen")).unwrap();
    assert_eq!(get_info(addr, "s1", "stolen").await.status(), 400);
}

/// A symlink whose target escapes the worktree can be DELETED, so it can be
/// moved: both act on the directory entry, neither touches the target.
#[tokio::test]
async fn moving_a_symlink_whose_target_escapes_the_worktree_moves_the_link() {
    let (addr, _tmp, wt) = boot().await;
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "top secret\n").unwrap();
    std::os::unix::fs::symlink(&secret, wt.join("escape-link")).unwrap();

    let resp = do_move(addr, "escape-link", "dest/escape-link").await;
    assert_eq!(resp.status(), 200);
    let moved = wt.join("dest/escape-link");
    assert!(
        moved.symlink_metadata().unwrap().file_type().is_symlink(),
        "the LINK moves, as itself"
    );
    assert_eq!(std::fs::read_link(&moved).unwrap(), secret);
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "top secret\n",
        "what it points at, outside the worktree, is untouched"
    );
}

#[tokio::test]
async fn info_unknown_session_is_404() {
    let (addr, _tmp, _wt) = boot().await;
    assert_eq!(get_info(addr, "nope", "src/moveme.txt").await.status(), 404);
}
