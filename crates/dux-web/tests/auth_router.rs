//! The web login against the real router: an adversarial matrix over every
//! route family, HTTP and WebSocket, public-route methods, forwarding headers
//! forged, missing and duplicated, the Host and Origin checks, sign-in,
//! sign-out, the password route, the blocklist with and without a password,
//! live revocation of open sockets, and a login flood that must leave ordinary
//! requests answering.
//!
//! Requests reach the router two ways. Plain HTTP goes through `oneshot` with
//! the connection's two ends set by hand ([`Arrival`]), which is exactly what a
//! serve leg records for each accepted connection, so a test can be a client
//! from the network, the tailnet or this machine. WebSockets go over a real
//! loopback listener served the way every leg is; a forwarding header makes
//! such a client the network.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use dux_web::auth::Arrival;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{AppState, RouterParams, build_app};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PASSWORD: &str = "orbit velvet quarry lantern cobalt";
const OTHER_PASSWORD: &str = "harbor cinnamon glacier tundra mosaic";

fn hash_of(password: &str) -> String {
    static HASHES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let mut hashes = HASHES.lock().unwrap();
    if let Some((_, hash)) = hashes.iter().find(|(p, _)| p == password) {
        return hash.clone();
    }
    let hash = dux_core::auth::hash_password(&dux_core::auth::Password::new(password.into()))
        .expect("hash");
    hashes.push((password.to_string(), hash.clone()));
    hash
}

/// Who a request is, by the two ends of its connection.
const NETWORK: Arrival = Arrival {
    peer: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 7)),
        50000,
    ),
    local: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 10)),
        3890,
    ),
};
const THIS_MACHINE: Arrival = Arrival {
    peer: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 50000),
    local: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 3890),
};
const TAILNET: Arrival = Arrival {
    peer: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(100, 101, 102, 104)),
        50000,
    ),
    local: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(100, 101, 102, 103)),
        3890,
    ),
};

/// One dux router over a scratch workspace whose config.toml holds `auth` as
/// its `[server.auth]` section.
struct Dux {
    app: Router,
    tmp: dux_core::test_scratch::ScratchDir,
    reloads: Arc<std::sync::atomic::AtomicUsize>,
}

impl Dux {
    fn start(auth: &str) -> Self {
        Self::start_tuned(auth, |params| params)
    }

    fn start_tuned(auth: &str, tune: impl FnOnce(RouterParams) -> RouterParams) -> Self {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        std::fs::write(
            &paths.config_path,
            format!("# a comment that must survive\n\n[server.auth]\n{auth}\n"),
        )
        .unwrap();
        let mut engine = bootstrap_engine(&paths).unwrap();
        dux_core::test_provider::defuse_config(&mut engine.config);
        let (handle, _join) = spawn_engine_thread(engine);
        let reloads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&reloads);
        let app = build_app(
            handle,
            Router::<AppState>::new(),
            tune(
                RouterParams::plain_http().with_auth_reload(Arc::new(move || {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })),
            ),
        );
        Self { app, tmp, reloads }
    }

    fn with_password(extra: &str) -> Self {
        Self::start(&format!(
            "password_hash = \"{}\"\n{extra}",
            hash_of(PASSWORD)
        ))
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.tmp.path().join("config.toml")).unwrap()
    }

    async fn send(&self, from: Arrival, request: Req) -> Answer {
        let names: Vec<String> = request
            .headers
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        let mut builder = Request::builder()
            .method(request.method.clone())
            .uri(&request.path);
        if !names.iter().any(|n| n == "host") {
            builder = builder.header("host", "localhost");
        }
        if request.method != Method::GET
            && request.method != Method::HEAD
            && !names.iter().any(|n| n == "origin")
        {
            builder = builder.header("origin", "http://localhost");
        }
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(cookie) = &request.cookie {
            builder = builder.header("cookie", cookie.as_str());
        }
        let body = match &request.body {
            Some(body) => {
                builder = builder.header("content-type", "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let mut built = builder.body(body).unwrap();
        built
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(from));
        let response = self.app.clone().oneshot(built).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        Answer {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn get(&self, from: Arrival, path: &str) -> Answer {
        self.send(from, Req::new(Method::GET, path)).await
    }

    async fn login(&self, from: Arrival, password: &str) -> Answer {
        self.send(
            from,
            Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "password": password })),
        )
        .await
    }

    /// Sign in and answer the `Cookie` header value the session rides on.
    async fn signed_in(&self, from: Arrival) -> String {
        let login = self.login(from, PASSWORD).await;
        assert_eq!(login.status, StatusCode::NO_CONTENT, "{}", login.body);
        login.cookie()
    }

    async fn status(&self, from: Arrival, cookie: Option<&str>) -> Value {
        let mut req = Req::new(Method::GET, "/api/v1/auth/status");
        req.cookie = cookie.map(str::to_string);
        let answer = self.send(from, req).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        answer.json()
    }
}

#[derive(Clone)]
struct Req {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    cookie: Option<String>,
    body: Option<Value>,
}

impl Req {
    fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_string(),
            headers: Vec::new(),
            cookie: None,
            body: None,
        }
    }

    fn json(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn cookie(mut self, cookie: &str) -> Self {
        self.cookie = Some(cookie.to_string());
        self
    }
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    fn error(&self) -> Option<String> {
        self.json()["error"].as_str().map(str::to_string)
    }

    fn set_cookie(&self) -> String {
        self.headers
            .get("set-cookie")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }

    /// The `name=value` pair of the cookie this answer set.
    fn cookie(&self) -> String {
        self.set_cookie()
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string()
    }
}

fn assert_auth_required(answer: &Answer, what: &str) {
    assert_eq!(
        (answer.status, answer.error().as_deref()),
        (StatusCode::UNAUTHORIZED, Some("auth_required")),
        "{what}: {}",
        answer.body
    );
}

/// Every protected route family, by method and a path the router knows (or a
/// path it does not, which the SPA fallback would otherwise serve).
fn protected_routes() -> Vec<Req> {
    let get = |p: &str| Req::new(Method::GET, p);
    let post = |p: &str| Req::new(Method::POST, p).json(json!({}));
    let put = |p: &str| Req::new(Method::PUT, p).json(json!({}));
    let patch = |p: &str| Req::new(Method::PATCH, p).json(json!({}));
    let delete = |p: &str| Req::new(Method::DELETE, p);
    vec![
        get("/api/v1/bootstrap"),
        get("/api/v1/build"),
        get("/api/v1/workspace"),
        get("/api/v1/projects"),
        post("/api/v1/projects"),
        get("/api/v1/projects/p1/branches"),
        delete("/api/v1/projects/p1"),
        get("/api/v1/sessions"),
        post("/api/v1/sessions"),
        get("/api/v1/sessions/s1/changes"),
        get("/api/v1/sessions/s1/startup-logs"),
        get("/api/v1/sessions/s1/tabs"),
        post("/api/v1/sessions/s1/kill"),
        get("/api/v1/sessions/s1/files/tree"),
        get("/api/v1/sessions/s1/git/log"),
        get("/api/v1/terminals"),
        post("/api/v1/terminals"),
        get("/api/v1/resources"),
        get("/api/v1/browse"),
        get("/api/v1/release-notes"),
        get("/api/v1/config/raw"),
        put("/api/v1/config/raw"),
        patch("/api/v1/config/settings"),
        post("/api/v1/config/reload"),
        put("/api/v1/macros"),
        post("/api/v1/server/tailscale-mode"),
        post("/api/v1/file-drop"),
        post("/api/v1/auth/logout"),
        post("/api/v1/auth/password"),
        post("/api/v1/auth/dismiss-no-auth-warning"),
        // A path no route names, which the SPA fallback would answer with the
        // page: protected like the rest, because only the declared files are
        // public.
        get("/agent/s1"),
        get("/some/where/else"),
        // Public paths by another method.
        post("/api/v1/auth/status"),
        get("/api/v1/auth/login"),
        post("/healthz"),
        post("/index.html"),
        delete("/"),
    ]
}

const PUBLIC_GETS: &[&str] = &[
    "/",
    "/index.html",
    "/favicon.png",
    "/sw.js",
    "/manifest.webmanifest",
    "/offline.html",
    "/icon-192.png",
    "/icon-512.png",
    "/icon-maskable-512.png",
    "/dux-logo.png",
    "/icons.svg",
    "/healthz",
    "/api/v1/auth/status",
];

#[tokio::test]
async fn under_everywhere_every_protected_route_needs_a_session_and_the_public_ones_do_not() {
    let dux = Dux::with_password("require = \"everywhere\"");
    for from in [THIS_MACHINE, TAILNET, NETWORK] {
        for path in PUBLIC_GETS {
            let answer = dux.get(from, path).await;
            assert_ne!(answer.status, StatusCode::UNAUTHORIZED, "{path} is public");
            let head = dux.send(from, Req::new(Method::HEAD, path)).await;
            assert_ne!(
                head.status,
                StatusCode::UNAUTHORIZED,
                "HEAD {path} is public"
            );
        }
        for request in protected_routes() {
            let what = format!("{} {}", request.method, request.path);
            assert_auth_required(&dux.send(from, request).await, &what);
        }
    }
    // With a session, the same requests pass the layer (whatever the route
    // itself then answers).
    // Signing out is last: it ends the session the others ride on.
    let cookie = dux.signed_in(NETWORK).await;
    let (sign_out, rest): (Vec<Req>, Vec<Req>) = protected_routes()
        .into_iter()
        .partition(|r| r.path == "/api/v1/auth/logout");
    for request in rest.into_iter().chain(sign_out) {
        let what = format!("{} {}", request.method, request.path);
        let answer = dux.send(NETWORK, request.cookie(&cookie)).await;
        assert_ne!(
            answer.error().as_deref(),
            Some("auth_required"),
            "{what}: {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn with_no_password_nothing_needs_a_session_from_anywhere() {
    let dux = Dux::start("");
    let status = dux.status(NETWORK, None).await;
    assert_eq!(
        status["no_auth_warning"],
        json!(true),
        "a client from the network proves dux is reachable from it"
    );
    for from in [THIS_MACHINE, TAILNET, NETWORK] {
        for request in protected_routes() {
            let what = format!("{} {}", request.method, request.path);
            let answer = dux.send(from, request).await;
            assert_ne!(
                answer.status,
                StatusCode::UNAUTHORIZED,
                "{what}: {}",
                answer.body
            );
        }
    }
    assert_eq!(status["password_set"], json!(false));
    assert_eq!(status["required_here"], json!(false));
    assert_eq!(status["can_set_first_password"], json!(false), "{status}");
    assert_eq!(
        dux.status(TAILNET, None).await["can_set_first_password"],
        json!(true)
    );
    assert_eq!(
        dux.status(THIS_MACHINE, None).await["can_set_first_password"],
        json!(true)
    );
}

#[tokio::test]
async fn require_decides_who_needs_a_session_and_the_status_agrees() {
    for (require, machine, tailnet) in [
        ("network", false, false),
        ("tailnet", false, true),
        ("everywhere", true, true),
    ] {
        let dux = Dux::with_password(&format!("require = \"{require}\""));
        for (from, wants, class) in [
            (THIS_MACHINE, machine, "this_machine"),
            (TAILNET, tailnet, "tailnet"),
            (NETWORK, true, "network"),
        ] {
            let answer = dux.get(from, "/api/v1/projects").await;
            if wants {
                assert_auth_required(&answer, &format!("{require} {class}"));
            } else {
                assert_eq!(
                    answer.status,
                    StatusCode::OK,
                    "{require} {class}: {}",
                    answer.body
                );
            }
            let status = dux.status(from, None).await;
            assert_eq!(
                status["required_here"],
                json!(wants),
                "{require} {class}: {status}"
            );
            assert_eq!(status["client_class"], json!(class));
            assert_eq!(status["password_set"], json!(true));
            assert!(status["auth_broken"].is_null());
            assert_eq!(status["minimum_password_length"], json!(12));
            assert_eq!(status["minimum_password_score"], json!(2));
        }
        assert_eq!(
            dux.status(NETWORK, None).await["transport_encrypted"],
            json!(false)
        );
        assert_eq!(
            dux.status(TAILNET, None).await["transport_encrypted"],
            json!(true)
        );
        assert_eq!(
            dux.status(THIS_MACHINE, None).await["transport_encrypted"],
            json!(true)
        );
    }
}

#[tokio::test]
async fn forwarding_headers_never_make_a_request_more_trusted() {
    let dux = Dux::with_password("");
    // Plain loopback: this machine, no password needed under `network`.
    assert_eq!(
        dux.get(THIS_MACHINE, "/api/v1/projects").await.status,
        StatusCode::OK
    );
    for headers in [
        vec![("x-forwarded-for", "198.51.100.9")],
        vec![("x-forwarded-for", "127.0.0.1")],
        vec![("x-forwarded-for", "100.101.102.104")],
        vec![
            ("x-forwarded-for", "127.0.0.1"),
            ("x-forwarded-for", "127.0.0.1"),
        ],
        vec![("x-forwarded-for", "not-an-address")],
        vec![("forwarded", "for=127.0.0.1")],
        vec![("x-real-ip", "127.0.0.1")],
        vec![
            ("x-forwarded-for", "100.101.102.104"),
            ("tailscale-user-login", "owner@example.com"),
            ("tailscale-user-name", "Owner"),
        ],
        vec![("tailscale-funnel-request", "?1")],
    ] {
        let mut request = Req::new(Method::GET, "/api/v1/projects");
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        assert_auth_required(
            &dux.send(THIS_MACHINE, request).await,
            &format!("{headers:?}"),
        );
    }
    // Tailscale's identity headers from a network client prove nothing.
    let posing = Req::new(Method::GET, "/api/v1/projects")
        .header("tailscale-user-login", "owner@example.com");
    assert_auth_required(&dux.send(NETWORK, posing).await, "posing as tailnet");
    // The Funnel marker is the internet, even on the Tailscale listener.
    let funnel =
        Req::new(Method::GET, "/api/v1/auth/status").header("tailscale-funnel-request", "?1");
    let answer = dux.send(TAILNET, funnel).await;
    assert_eq!(answer.json()["client_class"], json!("internet"));
    assert_eq!(answer.json()["required_here"], json!(true));
}

#[tokio::test]
async fn a_session_is_a_host_only_strict_cookie_and_signing_out_revokes_it_on_the_server() {
    let dux = Dux::with_password("");
    let login = dux.login(NETWORK, PASSWORD).await;
    assert_eq!(login.status, StatusCode::NO_CONTENT);
    let set = login.set_cookie();
    let lower = set.to_ascii_lowercase();
    assert!(set.starts_with("dux_session_3890="), "{set}");
    for flag in ["httponly", "samesite=strict", "path=/"] {
        assert!(lower.contains(flag), "{flag}: {set}");
    }
    assert!(!lower.contains("domain"), "{set}");
    assert!(!lower.contains("secure"), "plain HTTP: {set}");
    let cookie = login.cookie();
    assert_eq!(
        dux.status(NETWORK, Some(&cookie)).await["signed_in"],
        json!(true)
    );
    let ok = dux
        .send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").cookie(&cookie),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);

    let logout = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/logout").cookie(&cookie),
        )
        .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert!(
        logout.set_cookie().contains("Max-Age=0"),
        "{}",
        logout.set_cookie()
    );
    assert_auth_required(
        &dux.send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").cookie(&cookie),
        )
        .await,
        "the replayed cookie after signing out",
    );
    // A cookie for another port, or one dux never minted, is not a session.
    for forged in [
        cookie.replace("dux_session_3890", "dux_session_4000"),
        "dux_session_3890=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
        "dux_session_3890=garbage".to_string(),
    ] {
        assert_auth_required(
            &dux.send(
                NETWORK,
                Req::new(Method::GET, "/api/v1/projects").cookie(&forged),
            )
            .await,
            &forged,
        );
    }
}

#[tokio::test]
async fn the_cookie_is_secure_by_the_setting_and_never_by_a_forwarded_header() {
    let dux = Dux::with_password("require = \"everywhere\"");
    let forged = Req::new(Method::POST, "/api/v1/auth/login")
        .json(json!({ "password": PASSWORD }))
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-for", "198.51.100.9");
    let answer = dux.send(THIS_MACHINE, forged).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert!(
        !answer.set_cookie().contains("Secure"),
        "{}",
        answer.set_cookie()
    );
    let always = Dux::with_password("cookie_secure = \"always\"");
    assert!(
        always
            .login(NETWORK, PASSWORD)
            .await
            .set_cookie()
            .contains("Secure")
    );
}

#[tokio::test]
async fn a_wrong_password_is_slowed_then_blocked_and_this_machine_never_is() {
    let dux = Dux::with_password("max_failed_logins = 3\nfailed_login_delay_seconds = 1");
    let wrong = dux.login(NETWORK, "not the password at all").await;
    assert_eq!(
        (wrong.status, wrong.error().as_deref()),
        (StatusCode::UNAUTHORIZED, Some("wrong_password"))
    );
    let slowed = dux.login(NETWORK, PASSWORD).await;
    assert_eq!(
        slowed.status,
        StatusCode::TOO_MANY_REQUESTS,
        "even the right one waits"
    );
    let wait: u64 = slowed.headers["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(slowed.json()["retry_after_seconds"], json!(wait));
    assert_eq!(wait, 1);

    let mut last = None;
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_millis(2100)).await;
        last = Some(dux.login(NETWORK, "not the password at all").await);
    }
    let last = last.unwrap();
    assert_eq!(
        (last.status, last.error().as_deref()),
        (StatusCode::FORBIDDEN, Some("blocked")),
        "{}",
        last.body
    );
    let place = last.json()["where"].as_str().unwrap().to_string();
    assert!(
        place.contains("blocked_addresses") && place.contains("config.toml"),
        "{place}"
    );
    assert!(
        !place.contains('/'),
        "no path reaches the blocked client: {place}"
    );
    let config = dux.config();
    assert!(config.contains("\"198.51.100.7\""), "{config}");
    assert!(config.contains("# a comment that must survive"), "{config}");
    assert_eq!(dux.reloads.load(std::sync::atomic::Ordering::SeqCst), 1);

    for path in ["/api/v1/auth/status", "/healthz", "/", "/api/v1/projects"] {
        let answer = dux.get(NETWORK, path).await;
        assert_eq!(
            (answer.status, answer.error().as_deref()),
            (StatusCode::FORBIDDEN, Some("blocked")),
            "{path}"
        );
    }
    assert_eq!(
        dux.login(NETWORK, PASSWORD).await.status,
        StatusCode::FORBIDDEN
    );

    // This machine fails just as often and is only ever slowed.
    for _ in 0..4 {
        let answer = dux.login(THIS_MACHINE, "not the password at all").await;
        assert!(
            matches!(
                answer.status,
                StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
            ),
            "{}",
            answer.body
        );
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }
    assert!(!dux.config().contains("127.0.0.1"));
}

#[tokio::test]
async fn blocked_addresses_apply_with_no_password_and_close_sockets_with_4403() {
    let dux = Dux::start("blocked_addresses = [\"198.51.100.0/24\", \"::ffff:203.0.113.5\"]");
    for path in ["/", "/healthz", "/api/v1/auth/status", "/api/v1/projects"] {
        let answer = dux.get(NETWORK, path).await;
        assert_eq!(answer.error().as_deref(), Some("blocked"), "{path}");
    }
    let mapped = Arrival {
        peer: "[::ffff:203.0.113.5]:4000".parse().unwrap(),
        local: "[::ffff:192.0.2.10]:3890".parse().unwrap(),
    };
    assert_eq!(
        dux.get(mapped, "/healthz").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        dux.get(TAILNET, "/api/v1/projects").await.status,
        StatusCode::OK
    );
    let everything = Dux::start("blocked_addresses = [\"0.0.0.0/0\", \"::/0\"]");
    assert_eq!(
        everything
            .get(THIS_MACHINE, "/api/v1/projects")
            .await
            .status,
        StatusCode::OK,
        "this machine is never blocked"
    );

    // A socket from a blocked client is accepted and closed with 4403.
    let server = Serve::start(dux).await;
    let code = server
        .ws_close_code("/ws/events", &[("x-forwarded-for", "198.51.100.9")], None)
        .await;
    assert_eq!(code, Some(4403));
}

/// The router on a real loopback listener, served the way every leg is.
struct Serve {
    addr: SocketAddr,
    _dux: Dux,
}

impl Serve {
    async fn start(dux: Dux) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = dux.app.clone();
        tokio::spawn(async move {
            let _ = axum::serve(
                dux_web::auth::provenance::Recorded(listener),
                app.into_make_service_with_connect_info::<Arrival>(),
            )
            .await;
        });
        Self { addr, _dux: dux }
    }

    async fn connect(
        &self,
        path: &str,
        headers: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{}{path}", self.addr)
            .into_client_request()
            .unwrap();
        for (name, value) in headers {
            request.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                    .unwrap(),
                value.parse().unwrap(),
            );
        }
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request)
            .await
            .expect("upgrade accepted")
            .0
    }

    /// Open `path` and answer the close code the server sends within a few
    /// seconds, or `None` when it stays open.
    async fn ws_close_code(
        &self,
        path: &str,
        headers: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> Option<u16> {
        let mut ws = self.connect(path, headers, cookie).await;
        wait_close(&mut ws, Duration::from_secs(3)).await
    }

    /// The `Cookie` header for a session signed in over this listener as the
    /// network (a forwarded request), so the cookie names this port.
    async fn signed_in(&self) -> String {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/api/v1/auth/login", self.addr))
            .header("x-forwarded-for", "198.51.100.20")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }
}

async fn wait_close<S>(ws: &mut S, within: Duration) -> Option<u16>
where
    S: StreamExt<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use tokio_tungstenite::tungstenite::Message;
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Some(frame.map_or(1005, |f| u16::from(f.code)));
            }
            Ok(None) | Ok(Some(Err(_))) => return Some(1006),
            _ => continue,
        }
    }
    None
}

#[tokio::test]
async fn every_socket_family_is_accepted_then_closed_with_4401_without_a_session() {
    let server = Serve::start(Dux::with_password("")).await;
    let forwarded = [("x-forwarded-for", "198.51.100.9")];
    for path in [
        "/ws/events",
        "/ws/sessions/s1/pty",
        "/ws/sessions/s1/terminals/t1/pty",
        "/ws/projects/p1/terminals/t1/pty",
        "/ws/terminals/t1/pty",
        "/ws/sessions/s1/tabs/t1/pty",
    ] {
        assert_eq!(
            server.ws_close_code(path, &forwarded, None).await,
            Some(4401),
            "{path}"
        );
    }
    // From this machine under `network`, the events socket opens and stays.
    assert_eq!(server.ws_close_code("/ws/events", &[], None).await, None);
    // A cross-site page cannot even open one to be told why.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{}/ws/events", server.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "http://evil.example".parse().unwrap());
    request
        .headers_mut()
        .insert("x-forwarded-for", "198.51.100.9".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
}

#[tokio::test]
async fn signing_out_and_a_password_change_close_the_sockets_that_held_the_session() {
    let server = Serve::start(Dux::with_password("")).await;
    let forwarded = [("x-forwarded-for", "198.51.100.9")];
    let a = server.signed_in().await;
    let b = server.signed_in().await;
    let mut a_events = server.connect("/ws/events", &forwarded, Some(&a)).await;
    let mut b_events = server.connect("/ws/events", &forwarded, Some(&b)).await;
    assert_eq!(
        wait_close(&mut a_events, Duration::from_millis(300)).await,
        None
    );

    let client = reqwest::Client::new();
    let logout = client
        .post(format!("http://{}/api/v1/auth/logout", server.addr))
        .header("x-forwarded-for", "198.51.100.9")
        .header("cookie", &a)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    assert_eq!(
        wait_close(&mut a_events, Duration::from_secs(3)).await,
        Some(4401)
    );
    assert_eq!(
        wait_close(&mut b_events, Duration::from_millis(500)).await,
        None,
        "the other session is untouched"
    );

    let change = client
        .post(format!("http://{}/api/v1/auth/password", server.addr))
        .header("x-forwarded-for", "198.51.100.9")
        .header("cookie", &b)
        .json(&json!({ "current": PASSWORD, "new": OTHER_PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(change.status(), 204);
    assert_eq!(
        wait_close(&mut b_events, Duration::from_secs(3)).await,
        Some(4401)
    );
}

#[tokio::test]
async fn the_first_password_is_set_only_from_this_machine_or_the_tailnet() {
    let dux = Dux::start("");
    let refused = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(refused.json()["message"].is_string());
    assert!(!dux.config().contains("$argon2id$"));

    let weak = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": "password1234" })),
        )
        .await;
    assert_eq!(
        weak.error().as_deref(),
        Some("weak_password"),
        "{}",
        weak.body
    );
    let body = weak.json();
    assert!(
        body["score"].is_number() && body["message"].is_string(),
        "{body}"
    );
    assert!(body["feedback"]["suggestions"].is_array(), "{body}");
    let short = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": "Qz7#vL9p" })),
        )
        .await;
    assert_eq!(short.error().as_deref(), Some("password_too_short"));
    assert_eq!(short.json()["minimum"], json!(12));

    let set = dux
        .send(
            TAILNET,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
        )
        .await;
    assert_eq!(set.status, StatusCode::NO_CONTENT, "{}", set.body);
    let config = dux.config();
    assert!(config.contains("password_hash = \"$argon2id$"), "{config}");
    assert!(!config.contains(PASSWORD));
    assert!(config.contains("# a comment that must survive"));
    assert_eq!(dux.status(NETWORK, None).await["password_set"], json!(true));
    assert_eq!(dux.reloads.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Changing it needs the current one: missing is not "signed out", wrong
    // is its own refusal, and neither changes anything.
    let cookie = dux.signed_in(NETWORK).await;
    let missing = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/password")
                .json(json!({ "new": OTHER_PASSWORD }))
                .cookie(&cookie),
        )
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST, "{}", missing.body);
    let wrong = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/password")
                .json(json!({ "current": "wrong-current-password-123", "new": OTHER_PASSWORD }))
                .cookie(&cookie),
        )
        .await;
    assert_eq!(
        (wrong.status, wrong.error().as_deref()),
        (StatusCode::FORBIDDEN, Some("wrong_current_password"))
    );
    let still = dux
        .send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").cookie(&cookie),
        )
        .await;
    assert_eq!(still.status, StatusCode::OK);
    // The right one changes it at once: a wrong current password counts toward
    // the block, but the wait after a failed LOGIN is not the change's to serve.
    let changed = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/password")
                .json(json!({ "current": PASSWORD, "new": OTHER_PASSWORD }))
                .cookie(&cookie),
        )
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.body);
    assert_auth_required(
        &dux.send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").cookie(&cookie),
        )
        .await,
        "a session from before the change",
    );
    assert_eq!(
        dux.login(NETWORK, PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
}

/// Wrong current passwords are guesses too: they count toward the block like
/// failed logins do, so a stolen session cannot be used to guess the password.
#[tokio::test]
async fn wrong_current_passwords_count_toward_the_block() {
    let dux = Dux::with_password("max_failed_logins = 2\nfailed_login_delay_seconds = 0");
    let cookie = dux.signed_in(NETWORK).await;
    let wrong = || {
        Req::new(Method::POST, "/api/v1/auth/password")
            .json(json!({ "current": "wrong-current-password-123", "new": OTHER_PASSWORD }))
            .cookie(&cookie)
    };
    assert_eq!(
        dux.send(NETWORK, wrong()).await.error().as_deref(),
        Some("wrong_current_password")
    );
    assert_eq!(
        dux.send(NETWORK, wrong()).await.error().as_deref(),
        Some("blocked")
    );
    assert!(dux.config().contains("\"198.51.100.7\""));
}

#[tokio::test]
async fn a_weak_password_signs_in_and_the_status_says_so_to_the_signed_in_only() {
    let dux = Dux::start(&format!(
        "password_hash = \"{}\"\nminimum_password_score = 2",
        hash_of("password1234")
    ));
    assert_eq!(
        dux.status(NETWORK, None).await["weak_password"],
        json!(false)
    );
    let login = dux.login(NETWORK, "password1234").await;
    assert_eq!(login.status, StatusCode::NO_CONTENT);
    let cookie = login.cookie();
    assert_eq!(
        dux.status(NETWORK, Some(&cookie)).await["weak_password"],
        json!(true)
    );
    assert_eq!(
        dux.status(NETWORK, None).await["weak_password"],
        json!(false),
        "not to a client that has not signed in"
    );
}

#[tokio::test]
async fn dismissing_the_warning_writes_the_setting_and_keeps_the_comments() {
    let dux = Dux::start("");
    assert_eq!(
        dux.status(NETWORK, None).await["no_auth_warning"],
        json!(true)
    );
    let answer = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/dismiss-no-auth-warning"),
        )
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    let config = dux.config();
    assert!(
        config.contains("disable_no_auth_warning = true"),
        "{config}"
    );
    assert!(config.contains("# a comment that must survive"));
    assert_eq!(
        dux.status(NETWORK, None).await["no_auth_warning"],
        json!(false)
    );
}

#[tokio::test]
async fn host_and_origin_checks_still_run_before_and_around_the_login() {
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), |p| {
        p.with_host_allowlist(vec!["127.0.0.1".parse().unwrap()], vec![], false)
    });
    let foreign = dux
        .send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/auth/status").header("host", "evil.example"),
        )
        .await;
    assert_eq!(
        foreign.status,
        StatusCode::FORBIDDEN,
        "a foreign Host never reaches the login"
    );
    let cross = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/login")
                .json(json!({ "password": PASSWORD }))
                .header("origin", "http://evil.example"),
        )
        .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN, "{}", cross.body);
    assert!(
        cross.set_cookie().is_empty(),
        "a cross-site login sets nothing"
    );
}

#[tokio::test]
async fn an_oversized_or_malformed_login_is_refused_before_any_check() {
    let dux = Dux::with_password("max_password_bytes = 64");
    let long = dux.login(NETWORK, &"x".repeat(65)).await;
    assert_eq!(
        long.error().as_deref(),
        Some("password_too_long"),
        "{}",
        long.body
    );
    let huge = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/login")
                .json(json!({ "password": "y".repeat(100_000) })),
        )
        .await;
    assert_eq!(huge.error().as_deref(), Some("password_too_long"));
    let garbage = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "nope": 1 })),
        )
        .await;
    assert_eq!(garbage.status, StatusCode::BAD_REQUEST);
    // None of these counted as a failure: the right password still signs in
    // at once.
    assert_eq!(
        dux.login(NETWORK, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// A flood of sign-ins from many addresses must not take the server with it:
/// at most `max_concurrent_password_checks` run at once, `password_check_queue`
/// more wait, the rest are told "too many requests" at once, and ordinary
/// requests keep answering promptly all the while.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_flood_leaves_ordinary_requests_answering() {
    let dux = Arc::new(Dux::with_password(
        "max_concurrent_password_checks = 1\npassword_check_queue = 2\n\
         failed_login_delay_seconds = 0\nmax_failed_logins = 0\nmax_failed_logins_per_minute = 0",
    ));
    let cookie = dux.signed_in(NETWORK).await;
    let flood: Vec<_> = (0..60)
        .map(|n| {
            let dux = Arc::clone(&dux);
            tokio::spawn(async move {
                let from = Arrival {
                    peer: SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, n as u8)),
                        40000,
                    ),
                    local: NETWORK.local,
                };
                let started = std::time::Instant::now();
                let answer = dux.login(from, "not the password at all").await;
                (answer.status, started.elapsed())
            })
        })
        .collect();
    // While the flood runs, a signed-in request and the health check answer.
    let mut worst = Duration::ZERO;
    for _ in 0..20 {
        let started = std::time::Instant::now();
        let answer = dux
            .send(
                NETWORK,
                Req::new(Method::GET, "/api/v1/projects").cookie(&cookie),
            )
            .await;
        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(dux.get(NETWORK, "/healthz").await.status, StatusCode::OK);
        worst = worst.max(started.elapsed());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut refused = 0;
    let mut checked = 0;
    let mut slowest_refusal = Duration::ZERO;
    for task in flood {
        let (status, took) = task.await.unwrap();
        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                refused += 1;
                slowest_refusal = slowest_refusal.max(took);
            }
            StatusCode::UNAUTHORIZED => checked += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert!(refused > 0, "the queue bound refused some at once");
    assert!(checked >= 3, "and checked the ones it let in: {checked}");
    assert!(
        worst < Duration::from_millis(500),
        "ordinary requests stayed responsive: worst {worst:?}"
    );
    assert!(
        slowest_refusal < Duration::from_millis(500),
        "a refusal never waits for a check: {slowest_refusal:?}"
    );
}

/// A password written into a file that already has problems stopping a start
/// is stored, but cannot be in force: the reload that would apply it refuses
/// the file. The answer says so, and the running dux keeps no password rather
/// than claiming the new one.
#[tokio::test]
async fn a_password_stored_beside_old_problems_is_said_not_to_be_in_force() {
    let dux = Dux::start("");
    std::fs::write(
        dux.tmp.path().join("config.toml"),
        "[server.auth]\nsession_idle_seconds = 0\n",
    )
    .unwrap();
    let answer = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error().as_deref()),
        (StatusCode::CONFLICT, Some("password_not_in_force")),
        "{}",
        answer.body
    );
    assert!(
        answer.json()["message"]
            .as_str()
            .unwrap()
            .contains("session_idle_seconds")
    );
    assert!(dux.config().contains("password_hash = \"$argon2id$"));
    assert_eq!(
        dux.status(NETWORK, None).await["password_set"],
        json!(false)
    );
}

/// With `[server] tailscale = "no"` dux cannot check for a Funnel or forward, so
/// it does not trust loopback: under the default `require` this machine signs
/// in too, the status says why, and the first password cannot be set from it.
#[tokio::test]
async fn when_dux_cannot_check_tailscale_this_machine_signs_in_and_is_told_why() {
    use dux_web::exposure::{ExposureCell, FunnelState};
    let unchecked = ExposureCell::new(FunnelState::Unchecked);
    let guarded = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        let cell = unchecked.clone();
        move |p| p.with_live_exposure(cell)
    });
    assert_auth_required(
        &guarded.get(THIS_MACHINE, "/api/v1/projects").await,
        "loopback",
    );
    let status = guarded.status(THIS_MACHINE, None).await;
    assert_eq!(status["required_here"], json!(true));
    assert_eq!(status["client_class"], json!("network"));
    assert_eq!(status["transport_encrypted"], json!(false));
    let reason = status["required_reason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(reason.contains("tailscale = \"no\""), "{reason}");
    assert!(guarded.status(NETWORK, None).await["required_reason"].is_null());

    let open = Dux::start_tuned("", move |p| p.with_live_exposure(unchecked));
    assert_eq!(
        open.status(THIS_MACHINE, None).await["can_set_first_password"],
        json!(false)
    );
    let refused = open
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(
        refused.json()["message"]
            .as_str()
            .unwrap()
            .contains("tailscale"),
        "{}",
        refused.body
    );
}

/// Funnel traffic is always HTTPS (Tailscale terminates TLS for every Funnel),
/// and `cookie_secure = "always"` is the owner saying browsers reach dux over
/// HTTPS through something dux cannot see into: neither gets the plain-HTTP
/// warning. Any other forwarded request keeps it.
#[tokio::test]
async fn https_paths_dux_knows_of_are_not_warned_about_plain_http() {
    let dux = Dux::with_password("");
    let funnel = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::GET, "/api/v1/auth/status")
                .header("tailscale-funnel-request", "?1")
                .header("x-forwarded-for", "198.51.100.30"),
        )
        .await
        .json();
    assert_eq!(funnel["client_class"], json!("internet"));
    assert_eq!(funnel["transport_encrypted"], json!(true), "{funnel}");
    let proxied = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::GET, "/api/v1/auth/status").header("x-forwarded-for", "198.51.100.30"),
        )
        .await
        .json();
    assert_eq!(proxied["transport_encrypted"], json!(false), "{proxied}");
    assert_eq!(
        dux.status(NETWORK, None).await["transport_encrypted"],
        json!(false)
    );

    let fronted = Dux::with_password("cookie_secure = \"always\"");
    assert_eq!(
        fronted.status(NETWORK, None).await["transport_encrypted"],
        json!(true)
    );
    let forwarded = fronted
        .send(
            THIS_MACHINE,
            Req::new(Method::GET, "/api/v1/auth/status").header("x-forwarded-for", "198.51.100.30"),
        )
        .await
        .json();
    assert_eq!(forwarded["transport_encrypted"], json!(true), "{forwarded}");
}
