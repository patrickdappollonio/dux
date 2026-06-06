//! End-to-end tests for the login gate (slice A2): session-backed auth over the
//! same dedicated-thread server the `ws_transport` suite exercises, plus the WS
//! Origin check that applies regardless of auth.
//!
//! HTTP (login/logout/me) goes through `reqwest`; the WebSocket handshake goes
//! through `tokio-tungstenite`. Both hit the SAME real listening server so the
//! session cookie minted by `/api/login` is valid on the gated `/ws` upgrade.
//! Cookies are threaded MANUALLY (capture `set-cookie`, resend as `cookie`) so
//! the round-trip is explicit and asserted, including the anti-fixation id change.

use std::net::SocketAddr;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::auth::shared_auth;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::{spawn_engine_thread, spawn_engine_thread_with_auth};
use dux_web::server::router_with_auth;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

fn sample_session(
    id: &str,
    project_id: &str,
    branch: &str,
    worktree: &str,
) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: branch.to_string(),
        worktree_path: worktree.to_string(),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
    }
}

fn seed_paths(root: &std::path::Path) -> DuxPaths {
    let paths = DuxPaths {
        root: root.to_path_buf(),
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
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .upsert_session(&sample_session(
                "s1",
                "p1",
                "feat",
                root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }
    paths
}

/// Boot a real server with the given `[auth]` users (already `"name:hash"`),
/// serving with connect-info so the per-IP backoff has a peer address. Returns
/// the bound address and the temp dir that must outlive the server.
async fn boot_with_users(users: Vec<String>) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = seed_paths(tmp.path());
    let mut engine = bootstrap_engine(&paths).unwrap();
    engine.config.auth.users = users.clone();
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];

    let auth = shared_auth(&users, false);
    let (handle, _join) = spawn_engine_thread_with_auth(engine, (auth.clone(), false));
    let app = router_with_auth(handle, auth);
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

/// A throwaway bcrypt hash for "secret-pw" — generated per call (cheap enough at
/// cost 12 for a test, and keeps the plaintext local to each test).
fn user_entry(name: &str, password: &str) -> String {
    let hash = dux_core::auth::hash_password(password).unwrap();
    format!("{name}:{hash}")
}

/// HTTP client with NO automatic cookie jar — we thread cookies manually so the
/// session round-trip is explicit and assertable.
fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

/// Pull the `dux_session` cookie pair (`name=value`) out of a `set-cookie`
/// header so it can be resent as a `Cookie` request header.
fn session_cookie(resp: &reqwest::Response) -> Option<String> {
    let raw = resp.headers().get("set-cookie")?.to_str().ok()?;
    // `name=value; Path=/; HttpOnly; ...` — keep only the first pair.
    let pair = raw.split(';').next()?.trim();
    if pair.starts_with("dux_session=") {
        Some(pair.to_string())
    } else {
        None
    }
}

// --- No-users default: gate is off, today's UX ----------------------------

#[tokio::test]
async fn no_users_ws_open_without_auth() {
    // Mirrors the existing suites: with no [auth] users the gate is off, so an
    // unauthenticated, no-Origin WS connects and gets a view_model frame.
    let tmp = tempfile::tempdir().unwrap();
    let paths = seed_paths(tmp.path());
    let mut engine = bootstrap_engine(&paths).unwrap();
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
    let app = router_with_auth(handle, shared_auth(&[], false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("ws should connect when auth is off");
    let got = wait_for_view_model(&mut ws).await;
    assert!(got, "expected a view_model frame with auth off");
}

#[tokio::test]
async fn no_users_me_reports_disabled() {
    let (addr, _tmp) = boot_with_users(vec![]).await;
    let resp = client()
        .get(format!("http://{addr}/api/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["auth"], "disabled");
}

// --- Auth on: the full login -> session -> gate lifecycle -----------------

#[tokio::test]
async fn unauthenticated_ws_rejected_with_401_before_upgrade() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    // A raw WS handshake without a session cookie must get a clean HTTP 401, not
    // an opened-then-closed socket. tungstenite surfaces the non-101 status as a
    // handshake error carrying the response.
    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect_err("unauthenticated upgrade must be rejected");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), 401, "gate must reject before upgrade");
        }
        other => panic!("expected an HTTP 401 rejection, got {other:?}"),
    }
}

#[tokio::test]
async fn login_wrong_password_is_401_generic() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let resp = client()
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"WRONG"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    // Generic message: no hint about whether the user exists.
    let body = resp.text().await.unwrap();
    assert!(
        !body.to_lowercase().contains("password is") && !body.to_lowercase().contains("no such"),
        "failure message must not hint at user existence: {body}"
    );
}

#[tokio::test]
async fn login_unknown_user_is_401() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let resp = client()
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"mallory","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn login_right_sets_cookie_and_me_reports_user_and_ws_works() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();

    // /api/me before login: auth on, no session -> 401.
    let me = c.get(format!("http://{addr}/api/me")).send().await.unwrap();
    assert_eq!(
        me.status(),
        401,
        "me must be 401 before login when auth is on"
    );

    // Login with the right password -> 200 + a session cookie + the username.
    let resp = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = session_cookie(&resp).expect("login must set the dux_session cookie");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "alice");

    // /api/me WITH the cookie -> 200 {username}.
    let me = c
        .get(format!("http://{addr}/api/me"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me_body: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me_body["username"], "alice");

    // The cookie'd WS upgrade now succeeds end-to-end (view_model frame).
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("authenticated WS upgrade should succeed");
    assert!(
        wait_for_view_model(&mut ws).await,
        "authenticated WS must stream a view_model frame"
    );
}

#[tokio::test]
async fn logout_kills_the_session_and_ws_is_401_again() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();

    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&login).expect("cookie");

    // Logout destroys the session.
    let logout = c
        .post(format!("http://{addr}/api/logout"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);

    // The old cookie no longer authenticates the WS upgrade.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let err = tokio_tungstenite::connect_async(req)
        .await
        .expect_err("WS must be rejected after logout");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(resp.status(), 401),
        other => panic!("expected 401 after logout, got {other:?}"),
    }
}

#[tokio::test]
async fn session_id_rotates_on_login_anti_fixation() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();

    // A first successful login establishes a session id (the "planted" id an
    // attacker might try to fixate). Logging in AGAIN while presenting that id
    // must rotate it: `cycle_id()` runs on every successful login, so the second
    // login's cookie carries a DIFFERENT id even though it reused the first.
    let first = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let planted = session_cookie(&first).expect("first login must set a session cookie");

    let second = c
        .post(format!("http://{addr}/api/login"))
        .header("cookie", &planted)
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let rotated = session_cookie(&second).expect("re-login must set a fresh session cookie");

    assert_ne!(
        planted, rotated,
        "the session id must change across a login (anti-fixation via cycle_id)"
    );

    // The rotated cookie authenticates.
    let me = c
        .get(format!("http://{addr}/api/me"))
        .header("cookie", &rotated)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
}

// --- Origin check (applies with auth on AND off) --------------------------

#[tokio::test]
async fn bad_origin_rejected_with_403_when_auth_off() {
    let (addr, _tmp) = boot_with_users(vec![]).await;
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", "http://evil.example.com".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req)
        .await
        .expect_err("cross-origin WS must be rejected even with auth off");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403 for a bad Origin, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_origin_rejected_with_403_when_auth_on() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&login).expect("cookie");

    // Even WITH a valid session, a cross-origin upgrade is rejected (the Origin
    // check runs regardless of auth; here it must fire before/independent of the
    // session gate).
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut()
        .insert("origin", "http://evil.example.com".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req)
        .await
        .expect_err("cross-origin WS must be rejected even when authenticated");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403 for a bad Origin, got {other:?}"),
    }
}

#[tokio::test]
async fn same_origin_allowed_when_auth_off() {
    let (addr, _tmp) = boot_with_users(vec![]).await;
    // A matching Origin (same host:port as Host) is allowed.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", format!("http://{addr}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("same-origin WS should connect");
    assert!(wait_for_view_model(&mut ws).await);
}

#[tokio::test]
async fn missing_origin_allowed() {
    // The existing suites rely on this: a non-browser client (no Origin) is
    // allowed. Auth off here so the only gate is the Origin check.
    let (addr, _tmp) = boot_with_users(vec![]).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("missing-Origin WS should connect");
    assert!(wait_for_view_model(&mut ws).await);
}

// --- Rate limit -----------------------------------------------------------

#[tokio::test]
async fn rate_limit_kicks_in_after_repeated_failures() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();

    // 5 failures are allowed (the limit); the 6th must be throttled with 429.
    let mut saw_429 = false;
    for i in 0..6 {
        let resp = c
            .post(format!("http://{addr}/api/login"))
            .json(&serde_json::json!({"username":"alice","password":"WRONG"}))
            .send()
            .await
            .unwrap();
        if i < 5 {
            assert_eq!(resp.status(), 401, "attempt {i} should be a normal 401");
        } else {
            saw_429 = resp.status() == 429;
            assert!(saw_429, "the 6th rapid failure must be throttled (429)");
            assert!(
                resp.headers().get("retry-after").is_some(),
                "a 429 must carry Retry-After"
            );
        }
    }
    assert!(saw_429);
}

#[tokio::test]
async fn happy_path_login_is_never_throttled() {
    // A single correct login is well under the limit and must always succeed,
    // proving the limiter doesn't block the happy path.
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let resp = client()
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// --- Live config-reload refresh -------------------------------------------

/// Adding a user to config + `reload_config` makes that user able to log in
/// without restarting the server — the gate's shared snapshot is rebuilt when the
/// engine applies the reload.
#[tokio::test]
async fn reload_config_picks_up_new_user_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    // Real config.toml on disk with one user; reload re-reads THIS file.
    let alice = user_entry("alice", "secret-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\"]\n"),
    )
    .unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
    // bootstrap already loaded the config (with alice); confirm the snapshot
    // reflects exactly that.
    let users_at_boot = engine.config.auth.users.clone();
    assert_eq!(users_at_boot, vec![alice.clone()]);
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );

    let auth = shared_auth(&users_at_boot, false);
    let (handle, _join) = spawn_engine_thread_with_auth(engine, (auth.clone(), false));
    let app = router_with_auth(handle, auth);
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

    let c = client();
    // bob cannot log in yet.
    let before = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"bob","password":"bob-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(before.status(), 401, "bob must not exist before the reload");

    // Add bob to config.toml, then trigger reload over an authenticated WS.
    let bob = user_entry("bob", "bob-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\", \"{bob}\"]\n"),
    )
    .unwrap();

    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&login).expect("cookie");

    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws");
    let _ = ws.next().await; // initial view_model
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"type":"command","command":"reload_config","args":{}}"#.into(),
    ))
    .await
    .unwrap();

    // Poll until bob can log in (the reload worker re-reads config, the engine
    // applies it, and the loop rebuilds the shared auth snapshot).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut bob_in = false;
    while tokio::time::Instant::now() < deadline {
        let resp = c
            .post(format!("http://{addr}/api/login"))
            .json(&serde_json::json!({"username":"bob","password":"bob-pw"}))
            .send()
            .await
            .unwrap();
        if resp.status() == 200 {
            bob_in = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        bob_in,
        "bob should be able to log in after config reload (live gate refresh)"
    );
}

/// The concrete connected client stream type from the dev-dependency
/// `tokio-tungstenite` (axum carries a different internal version, so a generic
/// here would be ambiguous — we pin the exact type instead).
type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Wait (bounded) for a `view_model` frame on the socket, returning whether one
/// arrived.
async fn wait_for_view_model(ws: &mut ClientWs) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await
            && let Ok(t) = m.into_text()
            && t.contains("\"type\":\"view_model\"")
        {
            return true;
        }
    }
    false
}
