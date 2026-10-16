//! End-to-end tests for `POST /api/v1/pull-requests/resolve`, written as the
//! journeys a person takes when they paste a pull request link into the web UI.
//!
//! Every project here is a REAL git repository on disk with a real `origin`, and
//! the address is read by the same production call the surfaces make, so a
//! developer's own `insteadOf` rewrite applies exactly as it would in
//! production. The repositories are created through git commands that cannot
//! see any developer configuration, so nothing but the test decides what gets
//! written down.

use std::net::SocketAddr;
use std::path::Path;

use axum::Router;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::gh::GithubHostPolicy;
use dux_core::gh::probe_test_support::stand_in_gh_with_pull_request_and_repository;
use dux_core::model::GhStatus;
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

/// Run a git command that cannot read the developer's configuration. The
/// production read is deliberately left alone: a rewrite SHOULD apply there,
/// because the rewritten address is the one git would really contact.
fn git_isolated(dir: &Path, args: &[&str]) {
    let out = dux_core::test_git::fixture_git()
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Boot a server whose projects are `(id, name, origin address)`, with GitHub
/// integration on and the given hosts eligible.
async fn boot(
    projects: &[(&str, &str, &str)],
    hosts: &[&str],
) -> (SocketAddr, dux_core::test_scratch::ScratchDir) {
    boot_with_lookup(
        projects,
        hosts,
        Err("no pull request answers here\u{202E}"),
        Err("no repository answers here"),
    )
    .await
}

/// [`boot`] with `gh pr view` answering `pull_request` and `gh repo view`
/// answering `repository`: each its JSON, or a message on stderr with a
/// failing exit.
async fn boot_with_lookup(
    projects: &[(&str, &str, &str)],
    hosts: &[&str],
    pull_request: Result<&str, &str>,
    repository: Result<&str, &str>,
) -> (SocketAddr, dux_core::test_scratch::ScratchDir) {
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
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();

    {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        for (id, name, origin) in projects {
            let dir = root.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            git_isolated(&dir, &["init", "-b", "main"]);
            git_isolated(&dir, &["remote", "add", "origin", origin]);
            store
                .upsert_project(&ProjectConfig {
                    id: (*id).to_string(),
                    path: dir.to_string_lossy().into_owned(),
                    name: Some((*name).to_string()),
                    default_provider: None,
                    leading_branch: None,
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .unwrap();
        }
    }

    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    engine.github_integration_enabled = true;
    // Point the probe at a stand-in `gh` that reports exactly `hosts`. Starting
    // the engine thread starts the REAL host probe, so placing an answer here
    // and hoping is not enough: on a machine with no authenticated `gh` the
    // probe overwrites the placed answer mid-test and every resolve is refused.
    // With the stand-in, the production probe code path runs unchanged and the
    // answer it gets is the one this test wrote.
    let signed_in: Vec<(&str, &str)> = hosts.iter().map(|h| (*h, "ssh")).collect();
    engine.gh_probe.program =
        stand_in_gh_with_pull_request_and_repository(&root, &signed_in, pull_request, repository)
            .into();
    // The placed pair covers the window BEFORE the probe reports, so the answer
    // is the same at every instant of the test rather than only after it lands.
    engine.gh_status = GhStatus::Available;
    engine.set_github_git_protocols(
        hosts
            .iter()
            .map(|h| ((*h).to_string(), dux_core::gh::GitProtocol::Ssh))
            .collect(),
    );
    engine.set_github_host_policy(GithubHostPolicy::Hosts(
        hosts.iter().map(|h| (*h).to_string()).collect(),
    ));

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

async fn resolve(addr: SocketAddr, reference: &str) -> (reqwest::StatusCode, String) {
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/pull-requests/resolve"))
        .json(&serde_json::json!({ "reference": reference }))
        .send()
        .await
        .expect("resolve");
    let status = res.status();
    (status, res.text().await.expect("body"))
}

async fn check_destination(
    addr: SocketAddr,
    path: &Path,
    owner_repo: &str,
) -> (reqwest::StatusCode, String) {
    let res = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/pull-requests/clone-destination"
        ))
        .json(&serde_json::json!({
            "path": path,
            "host": "github.com",
            "owner_repo": owner_repo,
        }))
        .send()
        .await
        .expect("check the folder");
    let status = res.status();
    (status, res.text().await.expect("body"))
}

fn matched_names(body: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    value["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|p| p["name"].as_str().expect("name").to_string())
        .collect()
}

#[tokio::test]
async fn a_pasted_pull_request_link_resolves_to_the_project_that_repository_is_open_in() {
    let (addr, _tmp) = boot(
        &[
            ("p1", "widget", "git@github.com:acme/widget.git"),
            ("p2", "gadget", "git@github.com:acme/gadget.git"),
        ],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://github.com/acme/widget/pull/17").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["widget"]);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["number"], 17);
    assert_eq!(value["repository"], "github.com/acme/widget");
    // The lookup only runs when no project matches; this boot's lookup fails,
    // so running it here would put a sentence on the reply.
    assert!(value.get("clone").is_none(), "{body}");
    assert!(value.get("clone_unavailable").is_none(), "{body}");
}

#[tokio::test]
async fn a_browser_url_with_issues_on_the_end_still_resolves() {
    let (addr, _tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://github.com/acme/widget/issues").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["widget"]);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        value["number"].is_null(),
        "an issues route carries no pull request number: {body}"
    );
}

#[tokio::test]
async fn a_repository_dux_does_not_have_answers_with_its_name_and_no_projects() {
    let (addr, tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://github.com/acme/unknown/pull/3").await;
    assert_eq!(status, 200, "{body}");
    assert!(matched_names(&body).is_empty());
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["repository"], "github.com/acme/unknown",
        "the client has to be able to say WHICH repository dux has no project for"
    );
    // The pull request cannot be found: still a 200 with the projects, no
    // offer, and the lookup's own sentence as data.
    assert!(value.get("clone").is_none(), "{body}");
    assert_eq!(
        value["clone_unavailable"],
        "Failed to resolve PR #3 from acme/unknown: no pull request answers here"
    );

    // A reference with no number has nothing to look up, so neither key exists.
    let (status, body) = resolve(addr, "https://github.com/acme/unknown").await;
    assert_eq!(status, 200, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value.get("clone").is_none(), "{body}");
    assert!(value.get("clone_unavailable").is_none(), "{body}");

    // The folder the clone would go into: a checkout somebody made by hand is
    // a checkout, another repository's working copy is refused, and a folder
    // that does not exist yet is free. Nothing is created.
    let hand_made = tmp.path().join("hand-made");
    std::fs::create_dir(&hand_made).unwrap();
    git_isolated(&hand_made, &["init", "-b", "feature"]);
    git_isolated(
        &hand_made,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/unknown.git",
        ],
    );
    git_isolated(
        &hand_made,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let (status, body) = check_destination(addr, &hand_made, "acme/unknown").await;
    assert_eq!(status, 200, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["state"], "checkout");
    // The answer names the folder by its canonical spelling, which on macOS
    // turns the temporary directory's /var into /private/var.
    let canonical = std::fs::canonicalize(&hand_made).unwrap();
    assert_eq!(value["path"], canonical.to_string_lossy().as_ref());
    let (status, body) = check_destination(addr, &hand_made, "acme/other").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body,
        format!(
            "{} already holds something that isn't a checkout of github.com/acme/other. \
             Pick another folder.",
            canonical.display()
        )
    );
    let not_yet = tmp.path().join("not-yet");
    let (status, body) = check_destination(addr, &not_yet, "acme/unknown").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["state"],
        "free"
    );
    assert!(!not_yet.exists(), "the check created the folder");

    // For a checkout it also says whether the agent will need a review copy,
    // from what the folder holds: a branch checked out in the folder or
    // diverged from the pull request needs one, a branch the pull request has
    // moved past or one that contains it does not, and a head commit the folder
    // does not have leaves the question open. Nothing is fetched or written.
    let commit = |message: &str| {
        git_isolated(
            &hand_made,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                message,
            ],
        )
    };
    git_isolated(&hand_made, &["branch", "behind"]);
    commit("pull request work");
    let head = String::from_utf8(
        dux_core::test_git::fixture_git()
            .args(["-C"])
            .arg(&hand_made)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git_isolated(&hand_made, &["checkout", "-q", "-b", "ahead"]);
    commit("more work");
    git_isolated(&hand_made, &["checkout", "-q", "behind"]);
    git_isolated(&hand_made, &["checkout", "-q", "-b", "side"]);
    commit("my own work");
    git_isolated(&hand_made, &["checkout", "-q", "feature"]);
    let review_copy = |branch: &'static str, oid: String| {
        let (hand_made, canonical) = (hand_made.clone(), canonical.clone());
        async move {
            let res = reqwest::Client::new()
                .post(format!(
                    "http://{addr}/api/v1/pull-requests/clone-destination"
                ))
                .json(&serde_json::json!({
                    "path": hand_made,
                    "host": "github.com",
                    "owner_repo": "acme/unknown",
                    "number": 3,
                    "head_branch": branch,
                    "head_oid": oid,
                }))
                .send()
                .await
                .expect("check the folder");
            assert_eq!(res.status(), 200);
            let value: serde_json::Value = res.json().await.unwrap();
            assert_eq!(value["path"], canonical.to_string_lossy().as_ref());
            value["review_copy"].clone()
        }
    };
    let checked_out = review_copy("feature", head.clone()).await;
    assert_eq!(
        checked_out,
        serde_json::json!({
            "state": "needed",
            "reason": "busy",
            "branch": "feature",
            "copy_branch": "feature-review",
            "warning": [
                "Branch ",
                { "name": "feature", "quoted": true },
                " is checked out in the project folder ",
                { "name": canonical.to_string_lossy(), "quoted": true },
                ". Creating this agent from PR ",
                { "name": "#3", "quoted": false },
                " needs a separate review copy on ",
                { "name": "feature-review", "quoted": true },
                ", and pushes from this agent would land on ",
                { "name": "feature-review", "quoted": true },
                ", not on the pull request.",
            ],
        })
    );
    let diverged = review_copy("side", head.clone()).await;
    assert_eq!(diverged["state"], "needed");
    assert_eq!(diverged["reason"], "diverged");
    assert_eq!(diverged["copy_branch"], "side-review");
    for (branch, why) in [
        ("nothing-like-this", "no branch of that name"),
        ("behind", "a branch the pull request has moved past"),
        ("ahead", "a branch that contains the pull request"),
    ] {
        assert_eq!(
            review_copy(branch, head.clone()).await,
            serde_json::json!({ "state": "not_needed" }),
            "{why}"
        );
    }
    assert_eq!(
        review_copy("behind", "f".repeat(40)).await,
        serde_json::json!({ "state": "unknown" }),
        "a head commit the folder does not have cannot be compared"
    );
    // A caller that sends no pull request facts gets the answer it always did.
    let (_, body) = check_destination(addr, &hand_made, "acme/unknown").await;
    assert!(
        serde_json::from_str::<serde_json::Value>(&body)
            .unwrap()
            .get("review_copy")
            .is_none(),
        "{body}"
    );

    // With a pull request gh can find, the reply carries the offer: the
    // address follows the host's ssh setting, and it says up front whether the
    // agent will need a review copy. A clone checks out the repository's
    // default branch, so a pull request from that branch will.
    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
    let found = format!(
        r#"{{"number":3,"title":"Add a thing","state":"OPEN","headRefName":"add-thing","headRefOid":"{HEAD}"}}"#
    );
    let offer = |repository: Result<&'static str, &'static str>| {
        let found = found.clone();
        async move {
            let (addr, tmp) = boot_with_lookup(
                &[("p1", "widget", "git@github.com:acme/widget.git")],
                &["github.com"],
                Ok(&found),
                repository,
            )
            .await;
            let (status, body) = resolve(addr, "https://github.com/acme/unknown/pull/3").await;
            assert_eq!(status, 200, "{body}");
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert!(value.get("clone_unavailable").is_none(), "{body}");
            (value["clone"].clone(), tmp)
        }
    };

    let (clone, _tmp) = offer(Ok(r#"{"defaultBranchRef":{"name":"main"}}"#)).await;
    assert_eq!(
        clone,
        serde_json::json!({
            "address": "git@github.com:acme/unknown.git",
            "host": "github.com",
            "owner_repo": "acme/unknown",
            "number": 3,
            "title": "Add a thing",
            "state": "OPEN",
            "head_branch": "add-thing",
            "head_oid": HEAD,
            "review_copy": { "state": "not_needed" },
        })
    );

    let (clone, _tmp) = offer(Ok(r#"{"defaultBranchRef":{"name":"add-thing"}}"#)).await;
    assert_eq!(
        clone["review_copy"],
        serde_json::json!({
            "state": "needed",
            "reason": "busy",
            "branch": "add-thing",
            "copy_branch": "add-thing-review",
            "warning": [
                "Branch ",
                { "name": "add-thing", "quoted": true },
                " will be checked out in the project folder dux clones into. Creating this agent from PR ",
                { "name": "#3", "quoted": false },
                " needs a separate review copy on ",
                { "name": "add-thing-review", "quoted": true },
                ", and pushes from this agent would land on ",
                { "name": "add-thing-review", "quoted": true },
                ", not on the pull request.",
            ],
        })
    );

    // A default branch gh cannot name leaves the offer standing with the
    // question open, rather than turning the offer into a failure.
    let (clone, _tmp) = offer(Err("boom")).await;
    assert_eq!(clone["head_branch"], "add-thing");
    assert_eq!(
        clone["review_copy"],
        serde_json::json!({ "state": "unknown" })
    );
}

#[tokio::test]
async fn the_same_repository_checked_out_twice_answers_with_both() {
    let (addr, _tmp) = boot(
        &[
            ("p1", "widget", "git@github.com:acme/widget.git"),
            ("p2", "widget-review", "git@github.com:acme/widget.git"),
        ],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "acme/widget#8").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["widget", "widget-review"]);
}

#[tokio::test]
async fn owner_repo_hash_number_resolves_to_the_company_server_rather_than_github() {
    let (addr, _tmp) = boot(
        &[
            ("p1", "widget", "git@git.company.example:acme/widget.git"),
            ("p2", "gadget", "git@github.com:acme/gadget.git"),
        ],
        &["github.com", "git.company.example"],
    )
    .await;

    let (status, body) = resolve(addr, "acme/widget#123").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["widget"]);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["repository"], "acme/widget",
        "it named no host, and dux must not invent one"
    );
}

#[tokio::test]
async fn a_bare_number_is_refused_with_the_reason_and_never_resolved() {
    let (addr, _tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    for reference in ["123", "#123"] {
        let (status, body) = resolve(addr, reference).await;
        assert_eq!(status, 400, "{reference}: {body}");
        assert!(
            body.contains("does not say which repository"),
            "{reference}: {body}"
        );
        assert!(
            body.contains("choose an existing project"),
            "the refusal must point at the way out: {body}"
        );
    }
}

#[tokio::test]
async fn text_that_names_nothing_is_refused_before_any_git_call() {
    let (addr, _tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "not a reference").await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("Enter a pull request URL"), "{body}");
}

#[tokio::test]
async fn a_host_dux_may_not_ask_about_is_refused_by_name() {
    let (addr, _tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://gitlab.com/acme/widget/pull/1").await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("gitlab.com"), "{body}");
}

#[tokio::test]
async fn a_project_it_could_not_inspect_is_reported_rather_than_counted_as_a_non_match() {
    // The only project is on a host `gh` is not signed in to, so dux never
    // compared it against anything. An empty match list here does NOT mean no
    // project is a checkout of `acme/widget`, and the reply has to say so or
    // the client will tell the user something dux never found out.
    let (addr, _tmp) = boot(
        &[("p1", "mirror", "git@gitlab.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "acme/widget#5").await;
    assert_eq!(status, 200, "{body}");
    assert!(matched_names(&body).is_empty());
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["uninspected_count"], 1, "{body}");
    assert_eq!(
        value["uninspected_summary"], "1 is on a host dux may not ask about",
        "and it must say WHY, so the message can name the gap: {body}"
    );
}

#[tokio::test]
async fn a_complete_answer_reports_nothing_uninspected() {
    let (addr, _tmp) = boot(
        &[("p1", "widget", "git@github.com:acme/widget.git")],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://github.com/acme/unknown/pull/3").await;
    assert_eq!(status, 200, "{body}");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["uninspected_count"], 0, "{body}");
    assert!(
        value["uninspected_summary"].is_null(),
        "every project was inspected, so \"none of them\" really means none: {body}"
    );
}

#[tokio::test]
async fn the_same_repository_on_two_allowed_hosts_is_told_apart_by_the_host_written_down() {
    let (addr, _tmp) = boot(
        &[
            (
                "p1",
                "widget-company",
                "git@git.company.example:acme/widget.git",
            ),
            ("p2", "widget-github", "git@github.com:acme/widget.git"),
        ],
        &["github.com", "git.company.example"],
    )
    .await;

    let (status, body) = resolve(addr, "acme/widget#4").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        matched_names(&body),
        vec!["widget-company", "widget-github"],
        "owner/repo#123 names no host, so both are candidates and the user picks"
    );

    let (status, body) = resolve(addr, "https://github.com/acme/widget/pull/4").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        matched_names(&body),
        vec!["widget-github"],
        "a host that was written down is a host that must be honoured"
    );

    let (status, body) = resolve(addr, "https://git.company.example/acme/widget/pull/4").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["widget-company"]);
}

/// The typed side must read a URL the way a browser reads it, all the way
/// through the server. Hand-splitting answered `acme/widget` here, which is a
/// repository this workspace really has, so dux would have proceeded confidently
/// against the wrong one.
#[tokio::test]
async fn a_dot_dot_in_a_pasted_link_resolves_to_the_repository_a_browser_would_open() {
    let (addr, _tmp) = boot(
        &[
            ("p1", "widget", "git@github.com:acme/widget.git"),
            ("p2", "gadget", "git@github.com:acme/gadget.git"),
        ],
        &["github.com"],
    )
    .await;

    let (status, body) = resolve(addr, "https://github.com/acme/widget/../gadget/pull/9").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(matched_names(&body), vec!["gadget"]);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["number"], 9);
    assert_eq!(value["repository"], "github.com/acme/gadget");
}
