//! End-to-end tests for the TLS / ACME serve path (slice T2).
//!
//! Real ACME can't run in tests (it needs a public IP, DNS, and a live CA), so
//! the acceptor is the injectable seam: production serves the app router through
//! `axum_server::bind(addr).handle(h).acceptor(acc).serve(make_svc)` with
//! rustls-acme's `AxumAcceptor`; here we drive the SAME serving construction with
//! a plain self-signed `RustlsAcceptor`. That proves the whole HTTPS path —
//! connect-info, graceful shutdown, WS-over-TLS, the Secure cookie, the Host
//! allowlist, the per-IP rate limiter — works end to end over real TLS. The :80
//! challenge/redirect router is the production router verbatim
//! (`tls::build_http_challenge_router` + `tls::serve_http_challenge`).
//!
//! The self-signed cert carries BOTH `dux.test` (DNS SAN, for reqwest's
//! `resolve()` + Host allowlist) and `127.0.0.1` (IP SAN, so the wss client can
//! connect to the loopback IP with no DNS while still validating the cert).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::auth::shared_auth;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::{AuthReloadContext, spawn_engine_thread_with_auth};
use dux_web::server::{RouterParams, build_app};
use dux_web::tls;
use futures_util::StreamExt;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TEST_DOMAIN: &str = "dux.test";

// ── Cert + rustls plumbing ─────────────────────────────────────────────────

/// Generate a self-signed cert valid for both `dux.test` and `127.0.0.1`, return
/// (cert DER, key DER) so the server builds a `RustlsAcceptor` and the clients
/// trust it.
fn self_signed() -> (Vec<u8>, Vec<u8>) {
    let certified =
        rcgen::generate_simple_self_signed(vec![TEST_DOMAIN.to_string(), "127.0.0.1".to_string()])
            .expect("generate self-signed cert");
    let cert_der = certified.cert.der().to_vec();
    let key_der = certified.key_pair.serialize_der();
    (cert_der, key_der)
}

/// Install the ring crypto provider as the process default exactly once (rustls
/// 0.23 requires a default provider when building a `ServerConfig`/`ClientConfig`
/// without one explicitly).
fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A rustls `ClientConfig` that trusts ONLY our self-signed cert. Used by the
/// wss client so it validates the server cert (no insecure "accept anything").
fn client_config_trusting(cert_der: &[u8]) -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(cert_der.to_vec()))
        .expect("add self-signed cert to the client root store");
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

// ── Server harness ──────────────────────────────────────────────────────────

fn sample_session(id: &str, project_id: &str) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        project_path: None,
        provider: dux_core::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: "feat".to_string(),
        worktree_path: "/tmp".to_string(),
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
        store.upsert_session(&sample_session("s1", "p1")).unwrap();
    }
    paths
}

fn user_entry(name: &str, password: &str) -> String {
    let hash = dux_core::auth::hash_password(password).unwrap();
    format!("{name}:{hash}")
}

/// A booted TLS server: the HTTPS app (Secure cookie + Host allowlist) over a
/// self-signed `RustlsAcceptor`, plus the :80 challenge/redirect router. Returns
/// the bound addresses, the cert (so clients trust it), and keep-alives.
struct TlsServer {
    https_addr: SocketAddr,
    http_addr: SocketAddr,
    cert_der: Vec<u8>,
    https_handle: axum_server::Handle<SocketAddr>,
    http_handle: axum_server::Handle<SocketAddr>,
    _tmp: tempfile::TempDir,
}

/// Boot the TLS server with the given `[auth]` users. The challenge router uses
/// the configured `https_port` so the :80 redirect points at the real TLS port.
async fn boot_tls(users: Vec<String>) -> TlsServer {
    install_crypto_provider();
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
            host_only: false,
        },
    );

    let domains = vec![TEST_DOMAIN.to_string()];

    // HTTPS app: Secure cookie ON (this is the TLS path) + Host allowlist.
    let (https_app, _store) = build_app(
        handle.clone(),
        Arc::clone(&auth),
        axum::Router::new(),
        RouterParams::tls(),
    );
    let https_app = tls::host_allowlist_layer(https_app, domains.clone());

    // Build the rustls acceptor from the self-signed cert.
    let (cert_der, key_der) = self_signed();
    let rustls_config =
        axum_server::tls_rustls::RustlsConfig::from_der(vec![cert_der.clone()], key_der)
            .await
            .expect("build RustlsConfig from der");
    let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(rustls_config);

    // Bind both listeners on loopback:0 first so we know the ports before the
    // :80 router (the redirect needs the real https port). We bind the std
    // listeners, read the addrs, then hand them to axum-server via from_tcp.
    let https_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // tokio refuses to register a blocking std socket; mark them non-blocking
    // before handing them to axum-server (`from_tcp`), exactly as serve_with_engine
    // does for the flip's pre-bound listeners.
    https_listener.set_nonblocking(true).unwrap();
    http_listener.set_nonblocking(true).unwrap();
    let https_addr = https_listener.local_addr().unwrap();
    let http_addr = http_listener.local_addr().unwrap();

    let https_handle = axum_server::Handle::new();
    let http_handle = axum_server::Handle::new();

    // :443 — the SAME serving construction production uses, with the injected
    // self-signed acceptor (production injects rustls-acme's AxumAcceptor).
    {
        let h = https_handle.clone();
        let app = https_app;
        tokio::spawn(async move {
            let _ = axum_server::from_tcp(https_listener)
                .unwrap()
                .handle(h)
                .acceptor(acceptor)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await;
        });
    }

    // :80 — the production challenge/redirect router verbatim. We need an
    // AcmeState to mount the real challenge tower service; build one against a
    // throwaway cache dir (staging directory, never contacted in tests).
    {
        let plan = tls::AcmePlan {
            http_addr,
            https_addr,
            domains: domains.clone(),
            email: String::new(),
            production: false,
            cache_dir: tmp.path().join("acme-cache"),
        };
        let (acme_state, norm_domains) = tls::build_acme_state(&plan).expect("build acme state");
        let http_router =
            tls::build_http_challenge_router(&acme_state, https_addr.port(), norm_domains);
        // Drive the state so the challenge resolver is live (no network in tests;
        // it just needs polling to exist). Abort it with the server via the handle
        // teardown at the end of the test process.
        let _acme_task = tls::spawn_acme_event_task(acme_state);
        std::mem::forget(_acme_task); // test-lifetime task; process exit cleans up
        let h = http_handle.clone();
        tokio::spawn(async move {
            let _ = axum_server::from_tcp(http_listener)
                .unwrap()
                .handle(h)
                .serve(http_router.into_make_service_with_connect_info::<SocketAddr>())
                .await;
        });
    }

    // Give the acceptors a moment to start listening.
    tokio::time::sleep(Duration::from_millis(50)).await;

    TlsServer {
        https_addr,
        http_addr,
        cert_der,
        https_handle,
        http_handle,
        _tmp: tmp,
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.https_handle.shutdown();
        self.http_handle.shutdown();
    }
}

/// An HTTPS reqwest client that trusts our self-signed cert and resolves
/// `dux.test` to the HTTPS listener (so SNI/Host are `dux.test`, matching the
/// cert SAN and the allowlist). No automatic cookie jar — cookies are threaded
/// manually so the round-trip is explicit.
fn https_client(server: &TlsServer) -> reqwest::Client {
    let cert = reqwest::Certificate::from_der(&server.cert_der).unwrap();
    reqwest::Client::builder()
        .add_root_certificate(cert)
        .resolve(TEST_DOMAIN, server.https_addr)
        .build()
        .unwrap()
}

/// A plain HTTP client for the :80 listener that does NOT follow redirects (so we
/// can assert the 308 + Location ourselves) and resolves `dux.test` to it.
fn http_client(server: &TlsServer) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(TEST_DOMAIN, server.http_addr)
        .build()
        .unwrap()
}

fn base_https(server: &TlsServer) -> String {
    format!("https://{TEST_DOMAIN}:{}", server.https_addr.port())
}

/// Pull the `dux_session` cookie pair out of a `set-cookie` header (the whole
/// attribute string, so Secure/SameSite can be asserted).
fn raw_set_cookie(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get("set-cookie")?
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

fn session_cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().trim().to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn https_request_succeeds_over_tls() {
    let server = boot_tls(vec![]).await; // auth off: /healthz + /api/me are open
    let c = https_client(&server);
    let resp = c
        .get(format!("{}/healthz", base_https(&server)))
        .send()
        .await
        .expect("HTTPS request should succeed over TLS");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn login_sets_secure_cookie_over_tls() {
    let server = boot_tls(vec![user_entry("alice", "secret-pw")]).await;
    let c = https_client(&server);
    let resp = c
        .post(format!("{}/api/login", base_https(&server)))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let set_cookie = raw_set_cookie(&resp).expect("login must set a cookie");
    assert!(
        set_cookie.to_lowercase().contains("secure"),
        "the session cookie must carry the Secure attribute on the TLS path: {set_cookie}"
    );
    assert!(
        set_cookie.contains("dux_session="),
        "the cookie must be the dux session cookie: {set_cookie}"
    );
}

#[tokio::test]
async fn ws_upgrade_works_over_tls() {
    let server = boot_tls(vec![user_entry("alice", "secret-pw")]).await;
    let c = https_client(&server);

    // Log in to get a session cookie (gated /ws needs it).
    let login = c
        .post(format!("{}/api/login", base_https(&server)))
        .json(&serde_json::json!({"username":"alice","password":"secret-pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = session_cookie_pair(&raw_set_cookie(&login).unwrap());

    // Connect wss:// to the loopback IP (cert has the 127.0.0.1 IP SAN, so no DNS
    // is needed), spoofing Host + Origin to dux.test so the allowlist and the
    // same-origin WS check both pass.
    let url = format!("wss://127.0.0.1:{}/ws", server.https_addr.port());
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("host", TEST_DOMAIN.parse().unwrap());
    req.headers_mut()
        .insert("origin", format!("https://{TEST_DOMAIN}").parse().unwrap());
    req.headers_mut().insert("cookie", cookie.parse().unwrap());

    let connector = Connector::Rustls(Arc::new(client_config_trusting(&server.cert_der)));
    let (mut ws, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(connector))
            .await
            .expect("authenticated wss upgrade should succeed over TLS");

    assert!(
        wait_for_view_model(&mut ws).await,
        "the WS over TLS must stream a view_model frame"
    );
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn wrong_host_is_421_over_tls() {
    // Connect to the loopback IP (the cert's IP SAN validates the TLS handshake)
    // but override ONLY the Host header to a value NOT in the allowlist → the app
    // answers 421 Misdirected Request. This isolates the allowlist decision (on
    // the Host header) from cert validation (on the connection name).
    let server = boot_tls(vec![]).await;
    install_crypto_provider();
    let cert = reqwest::Certificate::from_der(&server.cert_der).unwrap();
    let c = reqwest::Client::builder()
        .add_root_certificate(cert)
        .build()
        .unwrap();
    let resp = c
        .get(format!(
            "https://127.0.0.1:{}/healthz",
            server.https_addr.port()
        ))
        .header("host", "evil.example.com")
        .send()
        .await
        .expect("the TLS connection itself succeeds; the app rejects the Host");
    assert_eq!(
        resp.status(),
        421,
        "a Host outside the allowlist must get 421 Misdirected Request"
    );
}

#[tokio::test]
async fn http_port_80_redirects_to_https_with_query_preserved() {
    let server = boot_tls(vec![]).await;
    let c = http_client(&server);
    let resp = c
        .get(format!(
            "http://{TEST_DOMAIN}:{}/some/path?a=1&b=2",
            server.http_addr.port()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 308, "the :80 fallback must 308 to HTTPS");
    let location = resp
        .headers()
        .get("location")
        .and_then(|l| l.to_str().ok())
        .expect("a Location header");
    let expected = format!(
        "https://{TEST_DOMAIN}:{}/some/path?a=1&b=2",
        server.https_addr.port()
    );
    assert_eq!(
        location, expected,
        "the redirect must point at HTTPS on the real port, preserving path+query"
    );
}

#[tokio::test]
async fn acme_challenge_unknown_token_is_404_via_real_tower_service() {
    let server = boot_tls(vec![]).await;
    let c = http_client(&server);
    let resp = c
        .get(format!(
            "http://{TEST_DOMAIN}:{}/.well-known/acme-challenge/this-token-does-not-exist",
            server.http_addr.port()
        ))
        .send()
        .await
        .unwrap();
    // The REAL rustls-acme tower service answers 404 for an unknown token (it is
    // NOT redirected to HTTPS — the challenge route is matched before the
    // redirect fallback and is exempt from the allowlist).
    assert_eq!(
        resp.status(),
        404,
        "an unknown ACME challenge token must 404 via the real tower service, not redirect"
    );
}

#[tokio::test]
async fn per_ip_rate_limit_still_keyed_by_connect_info_under_axum_server() {
    // Connect-info must survive the axum-server + acceptor path so the per-IP
    // login backoff keeps working. Hammer failed logins from the same peer and
    // expect a 429 with a retry-after.
    let server = boot_tls(vec![user_entry("alice", "secret-pw")]).await;
    let c = https_client(&server);

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = c
            .post(format!("{}/api/login", base_https(&server)))
            .json(&serde_json::json!({"username":"alice","password":"WRONG"}))
            .send()
            .await
            .unwrap();
        if resp.status() == 429 {
            assert!(
                resp.headers().get("retry-after").is_some(),
                "a throttled response must carry retry-after"
            );
            saw_429 = true;
            break;
        }
        assert_eq!(
            resp.status(),
            401,
            "a wrong password is 401 until the per-IP backoff trips"
        );
    }
    assert!(
        saw_429,
        "the per-IP rate limiter must trip under axum-server (connect-info preserved)"
    );
}

// ── Helpers ─────────────────────────────────────────────────────────────────

type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
