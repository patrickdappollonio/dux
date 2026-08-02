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
use dux_core::model::GhStatus;
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};

/// Run a git command that cannot read the developer's configuration. The
/// production read is deliberately left alone: a rewrite SHOULD apply there,
/// because the rewritten address is the one git would really contact.
fn git_isolated(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
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
async fn boot(projects: &[(&str, &str, &str)], hosts: &[&str]) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
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
    // Stand in for a settled `gh auth status` probe. The probe itself has its
    // own tests in dux-core against a stand-in `gh`; what is under test here is
    // resolution, so the answer is placed rather than raced for.
    engine.github_integration_enabled = true;
    engine.gh_status = GhStatus::Available;
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
    let (addr, _tmp) = boot(
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
