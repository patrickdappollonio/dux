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
use dux_web::engine_actor::{
    AuthReloadContext, spawn_engine_thread, spawn_engine_thread_with_auth,
};
use dux_web::server::{build_router_with_recheck, router_with_auth};
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
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
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
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

/// Removing a user from config + `reload_config` revokes that user's LIVE
/// session without a server restart: their existing cookie stops authorizing the
/// gated `/ws` upgrade (401) and `/api/me` reports 401 (not the stale username).
/// The gate and `/api/me` re-verify the session's username against the current
/// auth snapshot on every request, so the live-reload that drops the user takes
/// effect immediately.
#[tokio::test]
async fn reload_config_removing_user_revokes_live_session() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    // Two users on disk: alice (whose session we'll revoke) and bob (kept, so
    // the gate stays ENABLED after the reload — proving revocation is per-user,
    // not "auth turned off").
    let alice = user_entry("alice", "secret-pw");
    let bob = user_entry("bob", "bob-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\", \"{bob}\"]\n"),
    )
    .unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
    let users_at_boot = engine.config.auth.users.clone();
    assert_eq!(users_at_boot, vec![alice.clone(), bob.clone()]);
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
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

    // alice logs in and gets a working session: /api/me 200 and a cookie'd /ws
    // upgrade succeeds.
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = session_cookie(&login).expect("cookie");

    let me = c
        .get(format!("http://{addr}/api/me"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200, "alice's session must work before removal");

    // Remove alice from config (keep bob), then trigger reload over bob's
    // authenticated WS (alice's session is what we're revoking, so we can't use
    // it to send the reload command reliably; bob drives it).
    std::fs::write(&paths.config_path, format!("[auth]\nusers = [\"{bob}\"]\n")).unwrap();

    let bob_login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"bob","password":"bob-pw"}))
        .send()
        .await
        .unwrap();
    let bob_cookie = session_cookie(&bob_login).expect("bob cookie");
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut()
        .insert("cookie", bob_cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws");
    let _ = ws.next().await; // initial view_model
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"type":"command","command":"reload_config","args":{}}"#.into(),
    ))
    .await
    .unwrap();

    // Poll until alice's /api/me reports 401 (the reload re-read config, the
    // engine applied it, and the loop rebuilt the shared auth snapshot WITHOUT
    // alice — the gate's per-request username re-check now fails).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut alice_revoked = false;
    while tokio::time::Instant::now() < deadline {
        let resp = c
            .get(format!("http://{addr}/api/me"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        if resp.status() == 401 {
            alice_revoked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        alice_revoked,
        "alice's /api/me must report 401 after she is removed and config is reloaded"
    );

    // And her cookie no longer authorizes the gated /ws upgrade.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let err = tokio_tungstenite::connect_async(req)
        .await
        .expect_err("removed user's WS upgrade must be rejected without a restart");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(
            resp.status(),
            401,
            "the gate must 401 a removed user's live session"
        ),
        other => panic!("expected 401 after user removal, got {other:?}"),
    }
}

/// Removing the LAST user from config + `reload_config` turns the login gate OFF
/// on a running server (Finding 2(b)): the rebuild transitions enabled→disabled,
/// so `/api/me` reports `auth:disabled` instead of `401`. (The loud warning is a
/// side effect of the rebuild; here we assert the observable gate transition the
/// warning is meant to flag.)
///
/// This server binds 127.0.0.1 (loopback), so the S2 rule ALLOWS the downgrade —
/// a reachable bind (public OR Tailscale) would instead REFUSE it (covered by the
/// `AuthState::rebuild` unit tests). The `boot_with_users` helper passes
/// `host_only: true`, matching the loopback listener, so this test exercises the
/// allow-with-warning branch.
#[tokio::test]
async fn reload_config_removing_last_user_disables_gate_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    // One user on disk; the gate is ON at boot.
    let alice = user_entry("alice", "secret-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\"]\n"),
    )
    .unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
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
    assert!(
        auth.read().unwrap().enabled,
        "gate must be ON before removal"
    );
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
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
    // alice logs in and drives the reload over her own (still-valid) WS.
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = session_cookie(&login).expect("cookie");

    // Empty the users list on disk, then trigger the reload.
    std::fs::write(&paths.config_path, "[auth]\nusers = []\n").unwrap();

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

    // Poll until /api/me reports auth disabled (200 with the disabled marker),
    // proving the rebuild flipped the gate OFF live.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut gate_off = false;
    while tokio::time::Instant::now() < deadline {
        let resp = c.get(format!("http://{addr}/api/me")).send().await.unwrap();
        if resp.status() == 200 {
            let body = resp.text().await.unwrap();
            if body.contains("\"auth\":\"disabled\"") {
                gate_off = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        gate_off,
        "removing the last user via reload must turn the login gate OFF on the running server"
    );
}

/// Changing a rebind-relevant `[server]` setting (here `port`) and reloading
/// emits the warn-tone "restart the server" status, because the listeners are
/// bound at startup and reload-config cannot rebind them. We watch the status
/// broadcast the server itself publishes.
#[tokio::test]
async fn reload_config_changing_server_setting_warns_restart_needed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    // Config on disk with one user and an explicit [server] port; reload re-reads
    // THIS file, so changing the port on disk drives the drift.
    let alice = user_entry("alice", "secret-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\"]\n\n[server]\nport = 8080\n"),
    )
    .unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
    let users_at_boot = engine.config.auth.users.clone();
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
    // Subscribe to the server's status broadcast BEFORE moving the handle into
    // the router so we catch the warning the reload emits.
    let mut status_rx = handle.subscribe_status();
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
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&login).expect("cookie");

    // Change the [server] port on disk, then trigger the reload over alice's WS.
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\"]\n\n[server]\nport = 9090\n"),
    )
    .unwrap();

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

    // Wait for the drift warning on the status stream.
    let warned = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match status_rx.recv().await {
                Ok(status) => {
                    if status.tone == "warning"
                        && status
                            .message
                            .contains("Server listen/TLS settings changed")
                    {
                        break true;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        warned,
        "changing a [server] rebind setting + reload must warn that a restart is needed"
    );
}

/// A reload that changes ONLY a non-server section (here adding a user) must NOT
/// emit the restart-needed warning — it still emits the ordinary
/// "Configuration reloaded" info, so we prove the warning is absent by asserting
/// the info arrives without any preceding drift warning.
#[tokio::test]
async fn reload_config_non_server_change_does_not_warn_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    let alice = user_entry("alice", "secret-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\"]\n\n[server]\nport = 8080\n"),
    )
    .unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
    let users_at_boot = engine.config.auth.users.clone();
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
    let mut status_rx = handle.subscribe_status();
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
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&login).expect("cookie");

    // Add a user but leave [server] port unchanged.
    let bob = user_entry("bob", "bob-pw");
    std::fs::write(
        &paths.config_path,
        format!("[auth]\nusers = [\"{alice}\", \"{bob}\"]\n\n[server]\nport = 8080\n"),
    )
    .unwrap();

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

    // The "Configuration reloaded" info confirms the reload was applied. Reaching
    // it WITHOUT seeing a drift warning first proves the warning was suppressed.
    let outcome = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match status_rx.recv().await {
                Ok(status) => {
                    if status.tone == "warning"
                        && status
                            .message
                            .contains("Server listen/TLS settings changed")
                    {
                        break "warned";
                    }
                    if status.tone == "info" && status.message.contains("Configuration reloaded") {
                        break "reloaded";
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break "closed",
            }
        }
    })
    .await
    .unwrap_or("timeout");
    assert_eq!(
        outcome, "reloaded",
        "a non-server reload must apply without the restart-needed warning"
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

// --- S1: a live socket dies on user revocation -----------------------------

/// Boot a real server whose `/ws` user-existence recheck uses a SHORT period so
/// the revocation test is fast and deterministic, with a real `config.toml` on
/// disk (so `reload_config` re-reads it). Returns the bound address and the temp
/// dir that must outlive the server. `loopback` is true (127.0.0.1), but we keep
/// at least one user across the reload so the gate STAYS enabled — the recheck
/// closes a per-user revocation, not a whole-gate downgrade.
async fn boot_for_recheck(
    initial_users: Vec<String>,
    recheck: Duration,
) -> (SocketAddr, std::path::PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = seed_paths(&root);

    let line = initial_users
        .iter()
        .map(|u| format!("\"{u}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(&paths.config_path, format!("[auth]\nusers = [{line}]\n")).unwrap();

    let mut engine = bootstrap_engine(&paths).unwrap();
    let users_at_boot = engine.config.auth.users.clone();
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
    let (handle, _join) = spawn_engine_thread_with_auth(
        engine,
        AuthReloadContext {
            shared: auth.clone(),
            disable_auth: false,
            host_only: true,
            console: dux_web::console::Console::noop(),
        },
    );
    let app = build_router_with_recheck(handle, auth, axum::Router::new(), recheck);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config_path = paths.config_path.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, config_path, tmp)
}

/// S1: a live, ALREADY-UPGRADED WebSocket must close when its user is revoked —
/// the HTTP gate can't revoke an open socket, so the periodic in-socket recheck
/// closes it. alice opens a socket and receives a frame; we remove alice (bob
/// stays so the gate is still ON) and reload config; alice's socket must close
/// within a few recheck windows.
#[tokio::test]
async fn live_ws_closes_when_user_revoked() {
    let recheck = Duration::from_millis(200);
    let alice = user_entry("alice", "secret-pw");
    let bob = user_entry("bob", "bob-pw");
    let (addr, config_path, _tmp) =
        boot_for_recheck(vec![alice.clone(), bob.clone()], recheck).await;
    let c = client();

    // alice logs in and opens an authenticated socket, proving it streams.
    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = session_cookie(&login).expect("cookie");

    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("alice's authenticated WS must connect");
    assert!(
        wait_for_view_model(&mut ws).await,
        "alice's socket must stream at least one frame before revocation"
    );

    // Remove alice (keep bob → gate stays ON), reload config over bob's socket.
    std::fs::write(&config_path, format!("[auth]\nusers = [\"{bob}\"]\n")).unwrap();
    let bob_login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"bob","password":"bob-pw"}))
        .send()
        .await
        .unwrap();
    let bob_cookie = session_cookie(&bob_login).expect("bob cookie");
    let mut bob_req = format!("ws://{addr}/ws").into_client_request().unwrap();
    bob_req
        .headers_mut()
        .insert("cookie", bob_cookie.parse().unwrap());
    let (mut bob_ws, _) = tokio_tungstenite::connect_async(bob_req)
        .await
        .expect("bob ws");
    let _ = bob_ws.next().await; // initial view_model
    use futures_util::SinkExt;
    bob_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"command","command":"reload_config","args":{}}"#.into(),
        ))
        .await
        .unwrap();

    // alice's socket must close (the recheck sees her gone). Drain frames until
    // the stream ends; bounded so a hang fails the test rather than blocking.
    let closed = wait_for_socket_close(&mut ws, Duration::from_secs(8)).await;
    assert!(
        closed,
        "a revoked user's already-open socket must close once the recheck runs"
    );
}

/// S1 control: an AUTH-OFF server's socket is unaffected by the recheck
/// machinery — `recheck_user` is `None`, so the socket keeps streaming and never
/// self-closes. We boot with no users (gate off), open a no-Origin socket, prove
/// it streams, then assert it does NOT close within several recheck windows.
#[tokio::test]
async fn auth_off_socket_unaffected_by_recheck() {
    let recheck = Duration::from_millis(100);
    let (addr, _config_path, _tmp) = boot_for_recheck(vec![], recheck).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("auth-off WS should connect");
    assert!(
        wait_for_view_model(&mut ws).await,
        "auth-off socket must stream a frame"
    );

    // Wait well beyond several recheck periods; the socket must NOT close.
    let closed = wait_for_socket_close(&mut ws, Duration::from_millis(1500)).await;
    assert!(
        !closed,
        "an auth-off socket must never be closed by the recheck (recheck_user is None)"
    );
}

/// Finding 1: on a LOOPBACK server, removing the LAST user (whole-gate downgrade
/// to disabled) must NOT close the operator's own live socket. A whole-gate
/// downgrade is a loosening, not a per-user revocation — the recheck must
/// short-circuit on "gate off" and keep streaming. alice opens a socket and
/// drives the reload that empties the users list; her socket must keep streaming
/// (a subsequent frame arrives) while `/api/me` reports auth disabled.
#[tokio::test]
async fn live_ws_keeps_streaming_when_gate_downgrades_to_disabled() {
    let recheck = Duration::from_millis(200);
    let alice = user_entry("alice", "secret-pw");
    // Single user: removing them is a WHOLE-GATE downgrade (enabled → disabled),
    // not a per-user revocation.
    let (addr, config_path, _tmp) = boot_for_recheck(vec![alice.clone()], recheck).await;
    let c = client();

    let login = c
        .post(format!("http://{addr}/api/login"))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = session_cookie(&login).expect("cookie");

    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("alice's authenticated WS must connect");
    assert!(
        wait_for_view_model(&mut ws).await,
        "alice's socket must stream a frame before the downgrade"
    );

    // Empty the users list, then trigger the reload over alice's OWN socket
    // (the operator turning their own auth off). On a loopback bind the gate
    // downgrades to disabled with a warning.
    std::fs::write(&config_path, "[auth]\nusers = []\n").unwrap();
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"type":"command","command":"reload_config","args":{}}"#.into(),
    ))
    .await
    .unwrap();

    // Poll until /api/me reports auth disabled, proving the gate actually
    // downgraded (so the recheck is now in the "gate off" regime).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut gate_off = false;
    while tokio::time::Instant::now() < deadline {
        let resp = c.get(format!("http://{addr}/api/me")).send().await.unwrap();
        if resp.status() == 200 {
            let body = resp.text().await.unwrap();
            if body.contains("\"auth\":\"disabled\"") {
                gate_off = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gate_off,
        "removing the last user must flip the loopback gate to disabled"
    );

    // The operator's socket must KEEP streaming across several recheck windows:
    // it must NOT self-close on the whole-gate downgrade. Prove liveness by
    // waiting for a subsequent frame (the recheck short-circuits on gate-off and
    // continues, so the ViewModel/status stream stays alive).
    let still_streaming = wait_for_any_frame(&mut ws, Duration::from_secs(3)).await;
    assert!(
        still_streaming,
        "the operator's own socket must keep streaming after they turn auth off \
         (a whole-gate downgrade is a loosening, not a revocation)"
    );
}

/// Wait (bounded) for ANY non-close frame on the socket, returning whether one
/// arrived. A `Close` frame, a stream end, or a transport error all count as
/// "not streaming" (the socket died), so they return `false`.
async fn wait_for_any_frame(ws: &mut ClientWs, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(None) => return false, // stream ended → not streaming
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))) => return false,
            Ok(Some(Ok(_))) => return true,   // a live frame arrived
            Ok(Some(Err(_))) => return false, // transport error → closed
            Err(_) => {}                      // timeout this round; loop until deadline
        }
    }
    false
}

/// Drain a socket until it closes (returns true) or the deadline elapses
/// (returns false). A `Close` frame or a stream end both count as closed.
async fn wait_for_socket_close(ws: &mut ClientWs, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(None) => return true, // stream ended
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))) => return true,
            Ok(Some(Ok(_))) => {}            // some other frame; keep draining
            Ok(Some(Err(_))) => return true, // transport error = closed
            Err(_) => {}                     // timeout this round; loop until deadline
        }
    }
    false
}

// --- Q4: blank/empty username falls through to a clean 401 ------------------

/// A login with a whitespace-only or empty username must return a clean 401 (the
/// generic failure), exercising the unknown-user dummy-verify path rather than
/// 500ing. Both bodies are well-formed JSON, so they reach the verify; no such
/// user exists, so the result is the same generic 401 as any bad login.
#[tokio::test]
async fn blank_username_login_is_clean_401() {
    let (addr, _tmp) = boot_with_users(vec![user_entry("alice", "secret-pw")]).await;
    let c = client();
    for username in ["  ", ""] {
        let resp = c
            .post(format!("http://{addr}/api/login"))
            .json(&serde_json::json!({"username": username, "password": "x"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "a blank username ({username:?}) must fall through to a clean 401"
        );
    }
}
