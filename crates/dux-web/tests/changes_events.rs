//! End-to-end tests for the changed-files vertical: the REST read
//! (`GET /api/v1/sessions/:id/changes`), the `/ws/events` subscription/interest
//! model + two-connection isolation, and per-connection status-toast scoping on
//! `/ws/events` (driven by the `X-Connection-Id` header on REST mutations).

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let n = now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        project_id: "p1".to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: format!("{id}-branch"),
        worktree_path: worktree.to_string(),
        title: None,
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: n,
        updated_at: n,
    }
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("spawn git")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Make `dir` a git repo with a committed `f.txt` plus an uncommitted edit so it
/// has exactly one unstaged change.
fn init_repo_with_unstaged(dir: &std::path::Path) {
    run_git(dir, &["init", "-q"]);
    run_git(dir, &["config", "user.email", "t@example.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "line1\nline2\n").unwrap();
    run_git(dir, &["add", "f.txt"]);
    run_git(dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("f.txt"), "line1\nCHANGED\n").unwrap();
}

/// Boot a server (auth OFF) with:
/// - `s1`, `s2`: real git repos each with one unstaged change,
/// - `s_nonrepo`: a session whose worktree exists but is NOT a git repo (so a
///   changed-files read fails -> 409).
///
/// Also injects a test-only gated probe route `/api/_interest` returning the bus's
/// currently-interested session ids (so a test can assert interest exactness).
async fn boot() -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let wt1 = root.join("wt1");
    let wt2 = root.join("wt2");
    let wt_nonrepo = root.join("wt_nonrepo");
    std::fs::create_dir_all(&wt1).unwrap();
    std::fs::create_dir_all(&wt2).unwrap();
    std::fs::create_dir_all(&wt_nonrepo).unwrap();
    init_repo_with_unstaged(&wt1);
    init_repo_with_unstaged(&wt2);
    // A standalone git-repo directory for a second project `p2`, so a
    // project-scoped command (`checkout_project_default_branch`) is not rejected as
    // path-missing. It lives OUTSIDE the session worktrees so it does not perturb
    // their changed-files reads (notably `wt_nonrepo`, which must stay a non-repo).
    let proj2 = root.join("proj2");
    std::fs::create_dir_all(&proj2).unwrap();
    init_repo_with_unstaged(&proj2);

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
        // A second project whose path IS a git repo, for the project-scoped
        // command-scoping test.
        store
            .upsert_project(&ProjectConfig {
                id: "p2".to_string(),
                path: proj2.to_string_lossy().into_owned(),
                name: Some("p2".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session("s1", wt1.to_string_lossy().as_ref()))
            .unwrap();
        store
            .upsert_session(&sample_session("s2", wt2.to_string_lossy().as_ref()))
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s_nonrepo",
                wt_nonrepo.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);

    let auth = dux_web::auth::shared_auth(&[], false);
    let probe: Router<AppState> = Router::new().route(
        "/api/_interest",
        get(|State(state): State<AppState>| async move {
            let mut ids = state.event_bus.interested_sessions();
            ids.sort();
            axum::Json(ids)
        }),
    );
    let app = build_app(handle, auth, probe, RouterParams::plain_http()).0;

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

/// Read text frames from a WS until `pred` matches one or the deadline elapses.
/// Returns the first matching frame text, or `None` on timeout.
async fn wait_for_frame<S>(ws: &mut S, secs: u64, pred: impl Fn(&str) -> bool) -> Option<String>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(t) = m.into_text()
            && pred(t.as_str())
        {
            return Some(t.to_string());
        }
    }
    None
}

/// Read frames until the `connected` first-frame arrives and return its id. The
/// connection id moved onto `/ws/events` at cutover: its first frame is
/// `{"event":"connected","id":...}`.
async fn read_connection_id<S>(ws: &mut S) -> String
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let frame = wait_for_frame(ws, 5, |t| t.contains("\"event\":\"connected\""))
        .await
        .expect("a /ws/events connection must send a `connected` first frame");
    let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
    v["id"]
        .as_str()
        .expect("connected frame carries a string id")
        .to_string()
}

/// Poll `/api/_interest` until it equals `want` (sorted) or the deadline elapses.
async fn wait_for_interest(client: &reqwest::Client, addr: SocketAddr, want: &[&str]) -> bool {
    let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    want.sort();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        let got: Vec<String> = client
            .get(format!("http://{addr}/api/_interest"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if got == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    false
}

#[tokio::test]
async fn connected_frame_is_first_and_carries_an_id() {
    let (addr, _tmp) = boot().await;
    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let id = read_connection_id(&mut a).await;
    assert!(!id.is_empty(), "the connection id must be non-empty");
}

/// A non-pull REST action (`POST /api/v1/projects/:id/checkout-default`, a deferred
/// `HandlerStatusOp`) mints a busy scoped to the originating connection (via the
/// `X-Connection-Id` header): A sees it, B does not — proving the HandlerStatusOp
/// scoping on `/ws/events`, not just the pull path.
#[tokio::test]
async fn non_pull_command_status_is_scoped_to_origin() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let a_id = read_connection_id(&mut a).await;
    let _ = read_connection_id(&mut b).await;

    // Trigger the checkout via REST, scoped to A by echoing its connection id.
    let resp = client
        .post(format!("http://{addr}/api/v1/projects/p2/checkout-default"))
        .header("x-connection-id", &a_id)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "checkout-default must dispatch");

    let a_saw = wait_for_frame(&mut a, 5, |t| {
        t.contains("\"event\":\"status\"") && t.contains("default branch")
    })
    .await;
    assert!(
        a_saw.is_some(),
        "the originating connection must see its checkout busy"
    );

    let b_saw = wait_for_frame(&mut b, 2, |t| {
        t.contains("\"event\":\"status\"") && t.contains("default branch")
    })
    .await;
    assert!(
        b_saw.is_none(),
        "a scoped HandlerStatusOp toast must not leak to another connection: {b_saw:?}"
    );
}

/// A client joining MID-OPERATION must NOT receive another connection's in-progress
/// (or persisted error) status in its on-connect snapshot — the snapshot is scope
/// filtered exactly like the live status arm.
#[tokio::test]
async fn new_client_snapshot_excludes_other_connections_status() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let a_id = read_connection_id(&mut a).await;

    // A pulls s1 (no remote → a Busy then an error, both scoped to A) via REST.
    let resp = client
        .post(format!("http://{addr}/api/v1/git/pull"))
        .header("x-connection-id", &a_id)
        .json(&serde_json::json!({"session_id": "s1"}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "pull must dispatch");
    // Ensure A's op was actually minted before a new client joins.
    let a_saw = wait_for_frame(&mut a, 5, |t| {
        t.contains("\"event\":\"status\"") && t.contains("Pull")
    })
    .await;
    assert!(a_saw.is_some(), "A must see its own pull status");

    // A SECOND client joins now: its on-connect status snapshot must carry no trace
    // of A's pull (the status is scoped to A).
    let (mut c, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let _ = read_connection_id(&mut c).await;
    let leaked = wait_for_frame(&mut c, 2, |t| {
        t.contains("\"event\":\"status\"") && t.contains("Pull")
    })
    .await;
    assert!(
        leaked.is_none(),
        "a new client must not receive another connection's status in its snapshot: {leaked:?}"
    );
}

/// A CLEAN WebSocket Close (a Close frame, not a TCP drop) must drain the held
/// interest, exactly like the drop path.
#[tokio::test]
async fn events_interest_drains_on_clean_close() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    a.send(Message::Text(
        r#"{"subscribe":["session:s1:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_interest(&client, addr, &["s1"]).await,
        "interest must register after subscribe"
    );

    // Send a clean Close frame (then drop), distinct from a bare TCP drop.
    a.send(Message::Close(None)).await.unwrap();
    drop(a);

    assert!(
        wait_for_interest(&client, addr, &[]).await,
        "a clean WebSocket Close must drain the held interest"
    );
}

/// Two-connection isolation synchronized by the `/api/_interest` probe (no fixed
/// sleep): each connection receives only its own session's `session.changes`.
#[tokio::test]
async fn events_two_connection_isolation_synced_by_interest() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    a.send(Message::Text(
        r#"{"subscribe":["session:s1:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    b.send(Message::Text(
        r#"{"subscribe":["session:s2:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    // Synchronize on the interest probe rather than a fixed sleep.
    assert!(
        wait_for_interest(&client, addr, &["s1", "s2"]).await,
        "both subscriptions must register before the emit-triggering GETs"
    );

    let _ = client
        .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
        .send()
        .await
        .unwrap();
    let _ = client
        .get(format!("http://{addr}/api/v1/sessions/s2/changes"))
        .send()
        .await
        .unwrap();

    assert!(
        wait_for_frame(&mut a, 5, |t| t.contains("\"id\":\"s1\""))
            .await
            .is_some(),
        "A must receive its s1 event"
    );
    assert!(
        wait_for_frame(&mut a, 1, |t| t.contains("\"id\":\"s2\""))
            .await
            .is_none(),
        "A must not receive s2 events"
    );
    assert!(
        wait_for_frame(&mut b, 5, |t| t.contains("\"id\":\"s2\""))
            .await
            .is_some(),
        "B must receive its s2 event"
    );
    assert!(
        wait_for_frame(&mut b, 1, |t| t.contains("\"id\":\"s1\""))
            .await
            .is_none(),
        "B must not receive s1 events"
    );
}

#[tokio::test]
async fn rest_changes_200_lists_unstaged_and_404_unknown() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["rev"].as_u64().unwrap() >= 1,
        "rev must advance: {body}"
    );
    let unstaged = body["unstaged"].as_array().cloned().unwrap_or_default();
    assert!(
        unstaged.iter().any(|f| f["path"] == "f.txt"),
        "expected f.txt in unstaged: {body}"
    );
    assert!(
        body.get("watched_session_id").is_none(),
        "the dedicated body must NOT carry the global watched_session_id: {body}"
    );

    let unknown = client
        .get(format!("http://{addr}/api/v1/sessions/nope/changes"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test]
async fn rest_changes_409_with_retry_after_on_git_error() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    // `s_nonrepo`'s worktree exists (so the 404 worktree-resolve passes) but is not
    // a git repo, so the changed-files compute fails -> 409 + Retry-After.
    let resp = client
        .get(format!("http://{addr}/api/v1/sessions/s_nonrepo/changes"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
    assert!(
        resp.headers().get(reqwest::header::RETRY_AFTER).is_some(),
        "a 409 must carry Retry-After"
    );
}

#[tokio::test]
async fn events_two_connection_isolation() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();

    a.send(Message::Text(
        r#"{"subscribe":["session:s1:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    b.send(Message::Text(
        r#"{"subscribe":["session:s2:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    // Let the server register both subscriptions before the emit-triggering GETs.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A GET computes and emits `session.changes` for that session (first compute
    // detects a change vs the empty cache).
    let _ = client
        .get(format!("http://{addr}/api/v1/sessions/s1/changes"))
        .send()
        .await
        .unwrap();
    let _ = client
        .get(format!("http://{addr}/api/v1/sessions/s2/changes"))
        .send()
        .await
        .unwrap();

    // A is subscribed to s1 only: it must receive an s1 event and NEVER an s2 one.
    let a_frame = wait_for_frame(&mut a, 5, |t| {
        t.contains("\"event\":\"session.changes\"") && t.contains("\"id\":\"s1\"")
    })
    .await;
    assert!(a_frame.is_some(), "conn A never received its s1 event");
    let a_leak = wait_for_frame(&mut a, 1, |t| t.contains("\"id\":\"s2\"")).await;
    assert!(
        a_leak.is_none(),
        "conn A must NOT receive s2 events: {a_leak:?}"
    );

    // B is subscribed to s2 only: the mirror image.
    let b_frame = wait_for_frame(&mut b, 5, |t| {
        t.contains("\"event\":\"session.changes\"") && t.contains("\"id\":\"s2\"")
    })
    .await;
    assert!(b_frame.is_some(), "conn B never received its s2 event");
    let b_leak = wait_for_frame(&mut b, 1, |t| t.contains("\"id\":\"s1\"")).await;
    assert!(
        b_leak.is_none(),
        "conn B must NOT receive s1 events: {b_leak:?}"
    );
}

#[tokio::test]
async fn events_interest_is_exact_and_drains_on_socket_close() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let interest = || async {
        client
            .get(format!("http://{addr}/api/_interest"))
            .send()
            .await
            .unwrap()
            .json::<Vec<String>>()
            .await
            .unwrap()
    };

    assert!(
        interest().await.is_empty(),
        "no interest before any subscribe"
    );

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    // Subscribe to the SAME topic twice in one frame and again in a second frame:
    // a duplicate must not inflate the refcount.
    a.send(Message::Text(
        r#"{"subscribe":["session:s1:changes","session:s1:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    a.send(Message::Text(
        r#"{"subscribe":["session:s1:changes"]}"#.into(),
    ))
    .await
    .unwrap();

    // Poll until the interest registers (one entry, exactly).
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        seen = interest().await;
        if seen == vec!["s1".to_string()] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        seen,
        vec!["s1".to_string()],
        "duplicate subscribe must register exactly one interest"
    );

    // Closing the real socket must drain the refcount back to zero (the single
    // cleanup path on loop exit drops all held fine-topic interests).
    drop(a);
    let mut drained = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if interest().await.is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        drained,
        "interest must drain to zero after the socket closes"
    );
}

#[tokio::test]
async fn unknown_session_subscription_registers_no_interest() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    a.send(Message::Text(
        r#"{"subscribe":["session:ghost:changes"]}"#.into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let ids: Vec<String> = client
        .get(format!("http://{addr}/api/_interest"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        ids.is_empty(),
        "a phantom-session subscription must not inflate the poll set: {ids:?}"
    );
}

/// One client's operation toast (a pull busy) is delivered ONLY back to it, not to
/// a second `/ws/events` client — the F2 leak fix. The operation is triggered via
/// REST scoped to A's connection id (`X-Connection-Id`).
#[tokio::test]
async fn status_toast_is_scoped_to_origin_connection() {
    let (addr, _tmp) = boot().await;
    let client = reqwest::Client::new();

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    // Drain each connection's `connected` first frame (and grab A's id to scope to).
    let a_id = read_connection_id(&mut a).await;
    let _ = read_connection_id(&mut b).await;

    // A issues a pull via REST: the worker posts a Busy "Pulling…" status scoped to A.
    let resp = client
        .post(format!("http://{addr}/api/v1/git/pull"))
        .header("x-connection-id", &a_id)
        .json(&serde_json::json!({"session_id": "s1"}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "pull must dispatch");

    // A must see the Pulling status...
    let a_saw = wait_for_frame(&mut a, 5, |t| {
        t.contains("\"event\":\"status\"") && t.contains("Pulling")
    })
    .await;
    assert!(
        a_saw.is_some(),
        "the originating connection must see its toast"
    );

    // ...and B must NOT, within a generous window.
    let b_saw = wait_for_frame(&mut b, 2, |t| {
        t.contains("\"event\":\"status\"") && t.contains("Pulling")
    })
    .await;
    assert!(
        b_saw.is_none(),
        "a scoped operation toast must not leak to another connection: {b_saw:?}"
    );
}
