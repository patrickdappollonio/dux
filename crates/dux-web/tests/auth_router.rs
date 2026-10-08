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
const NETWORK: Arrival = Arrival::Tcp {
    peer: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 7)),
        50000,
    ),
    local: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 10)),
        3890,
    ),
};
const THIS_MACHINE: Arrival = Arrival::Tcp {
    peer: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 50000),
    local: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 3890),
};
/// A request over the control socket, from this machine's own user.
const CONTROL_SOCKET: Arrival = Arrival::Unix { uid: 1000 };
const TAILNET: Arrival = Arrival::Tcp {
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
    handle: dux_web::engine_actor::EngineHandle,
}

/// An engine surface that judges a file the terminal UI's way, as the flip's
/// and the background server's engine does. Its reload is never driven here.
struct TerminalUiSurface;

impl dux_core::engine::ConfigSurface for TerminalUiSurface {
    fn start_surface(&self) -> dux_core::config::Surface {
        dux_core::config::Surface::TerminalUi
    }

    fn reload(
        &self,
        _paths: dux_core::config::DuxPaths,
        worker_tx: std::sync::mpsc::Sender<dux_core::worker::WorkerEvent>,
    ) {
        dux_core::engine::ReloadCompletionGuard::new(worker_tx)
            .complete(Ok(dux_core::config::Config::default()));
    }

    fn recover_render(&self, config: &dux_core::config::Config) -> String {
        dux_core::config_write::render_config_plain(config)
    }
}

/// This machine's Tailscale address, as a successful look reports it.
fn own_tailscale_ips() -> Vec<std::net::IpAddr> {
    vec![TAILNET.local().unwrap().ip()]
}

/// What every router here starts with unless a test says otherwise: a
/// successful Tailscale look that found nothing reaching dux, naming this
/// machine's Tailscale address, which is what makes `TAILNET` the tailnet.
fn tailnet_exposure() -> dux_web::exposure::ExposureCell {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    exposure
}

impl Dux {
    fn start(auth: &str) -> Self {
        Self::start_tuned(auth, |params| params)
    }

    fn start_tuned(auth: &str, tune: impl FnOnce(RouterParams) -> RouterParams) -> Self {
        Self::start_on(auth, tune, false)
    }

    /// A router whose engine reloads the way the terminal UI's does, as under
    /// the flip and the background server.
    fn start_as_terminal_ui(auth: &str) -> Self {
        Self::start_on(auth, |params| params, true)
    }

    fn start_on(
        auth: &str,
        tune: impl FnOnce(RouterParams) -> RouterParams,
        terminal_ui: bool,
    ) -> Self {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        std::fs::write(
            &paths.config_path,
            format!("# a comment that must survive\n\n[server.auth]\n{auth}\n"),
        )
        .unwrap();
        let mut engine = bootstrap_engine(&paths).unwrap();
        dux_core::test_provider::defuse_config(&mut engine.config);
        if terminal_ui {
            engine.surface = Box::new(TerminalUiSurface);
        }
        let (handle, _join) = spawn_engine_thread(engine);
        let reloads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&reloads);
        // The server log every way of serving writes, which the log route reads.
        let log = dux_core::logger::open_server_log(
            &dux_core::config::ServerConfig::default(),
            &handle.paths(),
        )
        .unwrap();
        let app = build_app(
            handle.clone(),
            Router::<AppState>::new(),
            tune(
                RouterParams::plain_http()
                    .with_console(
                        dux_web::console::Console::server_log_only(Arc::new(log)),
                        false,
                    )
                    .with_live_exposure(tailnet_exposure())
                    .with_auth_reload(Arc::new(move || {
                        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    })),
            ),
        );
        Self {
            app,
            tmp,
            reloads,
            handle,
        }
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

    async fn cli_login(&self, from: Arrival, password: &str) -> Answer {
        self.send(
            from,
            Req::new(Method::POST, "/api/v1/auth/cli-login")
                .json(json!({ "password": password, "label": "a laptop" })),
        )
        .await
    }

    /// Sign in from the command line and answer the bearer token.
    async fn cli_token(&self, from: Arrival) -> String {
        let login = self.cli_login(from, PASSWORD).await;
        assert_eq!(login.status, StatusCode::OK, "{}", login.body);
        login.json()["token"].as_str().unwrap().to_string()
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

    fn bearer(self, token: &str) -> Self {
        self.header("authorization", &format!("Bearer {token}"))
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
        get("/api/v1/server/connections"),
        get("/api/v1/server/log"),
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
        post("/api/v1/auth/cli-logout"),
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
    // The Funnel marker is the internet on loopback, the only way tailscaled
    // delivers Funnel traffic; from a direct peer it is ignored, so the peer
    // stays what its connection says.
    let funnel =
        || Req::new(Method::GET, "/api/v1/auth/status").header("tailscale-funnel-request", "?1");
    let answer = dux.send(THIS_MACHINE, funnel()).await;
    assert_eq!(answer.json()["client_class"], json!("internet"));
    assert_eq!(answer.json()["required_here"], json!(true));
    let direct = dux.send(TAILNET, funnel()).await;
    assert_eq!(direct.json()["client_class"], json!("tailnet"));
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
async fn a_cli_token_authorizes_as_a_bearer_and_ends_on_logout_and_on_a_password_change() {
    let dux = Dux::with_password("");
    let login = dux.cli_login(NETWORK, PASSWORD).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.body);
    assert_eq!(login.json()["expires_at"], Value::Null);
    assert!(
        login.set_cookie().is_empty(),
        "a token rides in a header, not a cookie"
    );
    let token = login.json()["token"].as_str().unwrap().to_string();
    let projects = |token: &str| Req::new(Method::GET, "/api/v1/projects").bearer(token);
    assert_eq!(
        dux.send(NETWORK, projects(&token)).await.status,
        StatusCode::OK
    );
    assert_auth_required(
        &dux.send(
            NETWORK,
            projects("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        )
        .await,
        "a token dux never minted",
    );

    // Signing out ends that token and no other.
    let other = dux.cli_token(NETWORK).await;
    let logout = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/cli-logout").bearer(&token),
        )
        .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_auth_required(
        &dux.send(NETWORK, projects(&token)).await,
        "the token after cli-logout",
    );
    assert_eq!(
        dux.send(NETWORK, projects(&other)).await.status,
        StatusCode::OK
    );

    // A password change, made with a token itself, ends every token.
    let change = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/password")
                .bearer(&other)
                .json(json!({ "current": PASSWORD, "new": OTHER_PASSWORD })),
        )
        .await;
    assert_eq!(change.status, StatusCode::NO_CONTENT, "{}", change.body);
    assert_auth_required(
        &dux.send(NETWORK, projects(&other)).await,
        "the token after a password change",
    );
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
    let wrong = dux.cli_login(NETWORK, "not the password at all").await;
    assert_eq!(
        (wrong.status, wrong.error().as_deref()),
        (StatusCode::UNAUTHORIZED, Some("wrong_password"))
    );
    let slowed = dux.cli_login(NETWORK, PASSWORD).await;
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
    for attempt in 0..2 {
        tokio::time::sleep(Duration::from_millis(2100)).await;
        // The browser's login and the command line's count against one limit.
        last = Some(if attempt == 0 {
            dux.login(NETWORK, "not the password at all").await
        } else {
            dux.cli_login(NETWORK, "not the password at all").await
        });
    }
    let last = last.unwrap();
    assert_eq!(
        (last.status, last.error().as_deref()),
        (StatusCode::FORBIDDEN, Some("blocked")),
        "{}",
        last.body
    );
    // The refusal is the code alone: the page names the setting itself, and no
    // path reaches the blocked client.
    assert_eq!(last.json(), json!({ "error": "blocked" }), "{}", last.body);
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
    let banned = dux.cli_login(NETWORK, PASSWORD).await;
    assert_eq!(
        (banned.status, banned.error().as_deref()),
        (StatusCode::FORBIDDEN, Some("blocked"))
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
    let mapped = Arrival::Tcp {
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

    // A followed server log holds its session the way a socket does.
    std::fs::write(server._dux.tmp.path().join("server.log"), "hello\n").unwrap();
    let client = reqwest::Client::new();
    let follow = |cookie: String| {
        let client = client.clone();
        let url = format!("http://{}/api/v1/server/log?follow=true", server.addr);
        async move {
            let mut reply = client
                .get(url)
                .header("x-forwarded-for", "198.51.100.9")
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), 200);
            let first = reply.chunk().await.unwrap().expect("the log's first line");
            assert_eq!(&first[..], b"hello\n");
            reply
        }
    };
    // Whether the stream ends within `within`; the log lines the server
    // writes meanwhile (who signed in or out) are read past.
    let ended = |mut reply: reqwest::Response, within: Duration| async move {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(deadline, reply.chunk()).await {
                Ok(Ok(Some(_))) => continue,
                Ok(Ok(None)) => return true,
                _ => return false,
            }
        }
    };
    let a_follow = follow(a.clone()).await;
    let b_follow = follow(b.clone()).await;

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
    assert!(
        ended(a_follow, Duration::from_secs(3)).await,
        "signing out ends the log stream opened under that session"
    );
    assert_eq!(
        wait_close(&mut b_events, Duration::from_millis(500)).await,
        None,
        "the other session is untouched"
    );
    // Not yet: the other session's stream is still open.
    let (b_still_open, b_follow) = {
        let mut b_follow = b_follow;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
        let mut open = true;
        while let Ok(chunk) = tokio::time::timeout_at(deadline, b_follow.chunk()).await {
            if !matches!(chunk, Ok(Some(_))) {
                open = false;
                break;
            }
        }
        (open, b_follow)
    };
    assert!(b_still_open, "the other session's stream stays open");

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
    assert!(
        ended(b_follow, Duration::from_secs(3)).await,
        "a password change ends the log stream opened under the old password"
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
    // A wrong current password is accounted like a failed login, so the right
    // one waits out the same slow-down first, then changes it.
    let change = || {
        Req::new(Method::POST, "/api/v1/auth/password")
            .json(json!({ "current": PASSWORD, "new": OTHER_PASSWORD }))
            .cookie(&cookie)
    };
    let early = dux.send(NETWORK, change()).await;
    assert_eq!(
        early.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        early.body
    );
    let wait = early.json()["retry_after_seconds"]
        .as_u64()
        .expect("a wait");
    tokio::time::sleep(Duration::from_secs(wait) + Duration::from_millis(100)).await;
    let changed = dux.send(NETWORK, change()).await;
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

/// Through the reload dux really asks for after its own write, which reads
/// nothing else, so neither a "reloaded" nor a "refreshing" sentence is said.
#[tokio::test]
async fn dismissing_the_warning_writes_the_setting_and_keeps_the_comments() {
    let dux = Dux::start_tuned("", |mut params| {
        params.auth_reload = None;
        params
    });
    let mut reloads = dux.handle.subscribe_config_reloads();
    let mut statuses = dux.handle.subscribe_status();
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
    // Browsers are told to refetch once the engine has adopted the file.
    tokio::time::timeout(Duration::from_secs(10), reloads.recv())
        .await
        .expect("the engine reloaded")
        .expect("config reload broadcast");
    let mut said = Vec::new();
    while let Ok(status) = statuses.try_recv() {
        said.push(status.message);
    }
    assert_eq!(said, Vec::<String>::new());
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
                let from = Arrival::Tcp {
                    peer: SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, n as u8)),
                        40000,
                    ),
                    local: NETWORK.local().unwrap(),
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

/// "Not in force" is the serving surface's own verdict: a problem only the
/// terminal UI refuses leaves the password in force under `dux server`, and
/// is not in force under the flip or the background server, whose engine
/// reloads the terminal UI's way.
#[tokio::test]
async fn not_in_force_is_decided_by_the_surface_serving_the_request() {
    let broken_env = "[env]\nA = \"${\"\n";
    let first_password =
        || Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD }));

    let server = Dux::start("");
    std::fs::write(server.tmp.path().join("config.toml"), broken_env).unwrap();
    let answer = server.send(THIS_MACHINE, first_password()).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    assert_eq!(
        server.status(NETWORK, None).await["password_set"],
        json!(true),
        "in force under dux server"
    );

    let tui = Dux::start_as_terminal_ui("");
    std::fs::write(tui.tmp.path().join("config.toml"), broken_env).unwrap();
    let answer = tui.send(THIS_MACHINE, first_password()).await;
    assert_eq!(
        (answer.status, answer.error().as_deref()),
        (StatusCode::CONFLICT, Some("password_not_in_force")),
        "{}",
        answer.body
    );
    assert!(tui.config().contains("password_hash = \"$argon2id$"));
    assert_eq!(
        tui.status(NETWORK, None).await["password_set"],
        json!(false)
    );

    // A problem only dux server refuses is the mirror image: port 0 on its
    // own stops a `dux server` start, but a running one was started past it
    // (its reload judges the port by the command line), and the terminal UI
    // does not mind it at all.
    let tui = Dux::start_as_terminal_ui("");
    std::fs::write(tui.tmp.path().join("config.toml"), "[server]\nport = 0\n").unwrap();
    let answer = tui.send(THIS_MACHINE, first_password()).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
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
        move |p| {
            p.with_live_exposure(cell).with_host_allowlist(
                vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)],
                Vec::new(),
                false,
            )
        }
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
    // One short line, setting names marked for the browser's chips.
    assert!(reason.contains("`[server] tailscale`"), "{reason}");
    assert!(reason.ends_with('.') && reason.len() < 200, "{reason}");
    assert!(guarded.status(NETWORK, None).await["required_reason"].is_null());

    // The control socket is this machine's own user whatever the exposure
    // says: no session, and no Host or Origin check.
    let foreign_host = Req::new(Method::GET, "/api/v1/projects").header("host", "evil.example");
    assert_eq!(
        guarded.send(THIS_MACHINE, foreign_host).await.status,
        StatusCode::FORBIDDEN,
        "the Host guard is on for TCP"
    );
    let socket = guarded
        .send(
            CONTROL_SOCKET,
            Req::new(Method::GET, "/api/v1/projects").header("host", "evil.example"),
        )
        .await;
    assert_eq!(socket.status, StatusCode::OK, "{}", socket.body);
    let socket_status = guarded.status(CONTROL_SOCKET, None).await;
    assert_eq!(socket_status["required_here"], json!(false));
    assert_eq!(socket_status["client_class"], json!("this_machine"));
    let cross_site = guarded
        .send(
            CONTROL_SOCKET,
            Req::new(Method::POST, "/api/v1/auth/logout").header("origin", "http://evil.example"),
        )
        .await;
    assert_ne!(
        cross_site.status,
        StatusCode::FORBIDDEN,
        "{}",
        cross_site.body
    );
    let everywhere = Dux::with_password("require = \"everywhere\"");
    assert_auth_required(
        &everywhere.get(THIS_MACHINE, "/api/v1/projects").await,
        "loopback, when every client must sign in",
    );
    assert_eq!(
        everywhere
            .get(CONTROL_SOCKET, "/api/v1/projects")
            .await
            .status,
        StatusCode::OK,
        "the control socket never asks for the password"
    );
    // A followed log over the control socket is not ended for want of a
    // session it was never asked for: the stream stays open.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(800),
            everywhere.get(CONTROL_SOCKET, "/api/v1/server/log?follow=true"),
        )
        .await
        .is_err(),
        "the control socket's log stream stays open when every client must sign in"
    );

    let open = Dux::start_tuned("", move |p| p.with_live_exposure(unchecked));
    let open_status = open.status(THIS_MACHINE, None).await;
    assert_eq!(open_status["can_set_first_password"], json!(false));
    assert_eq!(
        open_status["required_reason"], status["required_reason"],
        "the same line explains why the first password cannot be set here"
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

// ── First adversarial review: relays, posing proxies, abandoned checks and
//    addresses a client chooses ─────────────────────────────────────────────

/// A relay on this machine aimed at dux's own Tailscale address (socat, `ssh
/// -L`, a reverse proxy, `tailscale funnel --tcp`) connects FROM that address,
/// so peer and listener are the same Tailscale address. dux cannot tell that
/// from a person, so it is the network.
#[tokio::test]
async fn a_connection_from_dux_own_tailscale_address_is_the_network() {
    let dux = Dux::with_password("");
    let ts: std::net::IpAddr = "100.101.102.103".parse().unwrap();
    let relayed = Arrival::Tcp {
        peer: SocketAddr::new(ts, 41000),
        local: SocketAddr::new(ts, 3890),
    };
    assert_auth_required(&dux.get(relayed, "/api/v1/projects").await, "a self-relay");
    assert_eq!(
        dux.status(relayed, None).await["client_class"],
        json!("network")
    );
    assert_eq!(
        dux.get(TAILNET, "/api/v1/projects").await.status,
        StatusCode::OK
    );
}

/// While dux knows of a forward to its port, the Tailscale listener is
/// distrusted like loopback: the forward may be aimed at either.
#[tokio::test]
async fn a_known_forward_distrusts_the_tailscale_listener_too() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Funnel);
    exposure.set_identity(Some(IdentityFacts {
        funnel_any: true,
        forward_to_dux: true,
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        let cell = exposure.clone();
        move |p| p.with_live_exposure(cell)
    });
    assert_auth_required(
        &dux.get(TAILNET, "/api/v1/projects").await,
        "tailnet under a forward",
    );
}

/// A LAN client through another proxy on this machine that keeps its Host and
/// passes its headers cannot pose as `tailscale serve`: that proxy's
/// X-Forwarded-For names a LAN address, which tailscale serve never reports.
#[tokio::test]
async fn a_lan_client_through_another_local_proxy_is_not_tailscale_serve() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        let cell = exposure.clone();
        move |p| p.with_live_exposure(cell)
    });
    let via = |xff: &str, extra: Option<(&str, &str)>| {
        let mut req = Req::new(Method::GET, "/api/v1/projects")
            .header("host", "box.tail0000.ts.net")
            .header("tailscale-user-login", "owner@example.com")
            .header("x-forwarded-for", xff);
        if let Some((name, value)) = extra {
            req = req.header(name, value);
        }
        req
    };
    assert_auth_required(
        &dux.send(THIS_MACHINE, via("192.168.1.50", None)).await,
        "LAN XFF",
    );
    // X-Real-IP and Forwarded are not part of the proof: tailscale serve
    // passes a client's own copies through, so they decide nothing here. The
    // nearest hop's X-Forwarded-For above is what refuses the LAN client.
    for extra in [
        ("x-real-ip", "192.168.1.50"),
        ("forwarded", "for=192.168.1.50"),
    ] {
        assert_eq!(
            dux.send(THIS_MACHINE, via("100.64.0.9", Some(extra)))
                .await
                .status,
            StatusCode::OK,
            "{extra:?}"
        );
    }
    assert_eq!(
        dux.send(THIS_MACHINE, via("100.64.0.9", None)).await.status,
        StatusCode::OK,
        "the real tailscale serve shape is still the tailnet"
    );
}

/// The blocklist applies to every address a request names: behind a proxy
/// that names the client in X-Real-IP or Forwarded, the client is blocked.
#[tokio::test]
async fn the_blocklist_applies_to_any_address_a_request_names() {
    let dux = Dux::with_password("blocked_addresses = [\"198.51.100.0/24\", \"2001:db8::/32\"]");
    for (name, value) in [
        ("x-real-ip", "198.51.100.9"),
        ("forwarded", "for=198.51.100.9"),
        ("forwarded", "for=\"[2001:db8::7]:4711\";proto=https"),
        ("forwarded", "for=192.0.2.1, for=198.51.100.9:80"),
        ("x-forwarded-for", "198.51.100.9, 192.0.2.4"),
    ] {
        let answer = dux
            .send(
                THIS_MACHINE,
                Req::new(Method::GET, "/api/v1/auth/status").header(name, value),
            )
            .await;
        assert_eq!(
            answer.error().as_deref(),
            Some("blocked"),
            "{name}: {value}"
        );
    }
}

/// A client cannot get an address of its choosing written into the owner's
/// blocklist: a proxy that sets only X-Real-IP passes the client's own
/// X-Forwarded-For through, so neither is verified, and such a ban stays out
/// of config.toml while the guesses are still slowed.
#[tokio::test]
async fn a_client_chosen_address_is_never_written_to_the_blocklist() {
    let dux = Dux::with_password("max_failed_logins = 2\nfailed_login_delay_seconds = 0");
    for _ in 0..3 {
        let answer = dux
            .send(
                THIS_MACHINE,
                Req::new(Method::POST, "/api/v1/auth/login")
                    .json(json!({ "password": "wrong" }))
                    .header("x-real-ip", "203.0.113.66")
                    .header("x-forwarded-for", "192.0.2.200"),
            )
            .await;
        assert!(answer.status.is_client_error(), "{}", answer.body);
    }
    let config = dux.config();
    assert!(
        !config.contains("192.0.2.200") && !config.contains("203.0.113.66"),
        "{config}"
    );
}

/// Rotating the claimed address does not escape the slow-down: every
/// unverified forwarded request shares one bucket as well as its own.
#[tokio::test]
async fn rotating_claimed_addresses_shares_one_slow_down() {
    let dux = Dux::with_password("failed_login_delay_seconds = 5");
    let try_from = |n: u8| {
        Req::new(Method::POST, "/api/v1/auth/login")
            .json(json!({ "password": "wrong" }))
            .header("x-forwarded-for", &format!("192.0.2.{n}"))
    };
    let first = dux.send(THIS_MACHINE, try_from(1)).await;
    assert_eq!(first.status, StatusCode::UNAUTHORIZED, "{}", first.body);
    let second = dux.send(THIS_MACHINE, try_from(2)).await;
    assert_eq!(
        second.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        second.body
    );
}

/// A client that sends a login and hangs up while it is checked must not free
/// the check's slot: the Argon2 run keeps going, so the slot is held until it
/// finishes, and its wrong guess is still counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_login_holds_its_slot_until_the_check_ends_and_still_counts() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = Serve::start(Dux::with_password(
        "max_concurrent_password_checks = 1\npassword_check_queue = 0\n\
         failed_login_delay_seconds = 5\nmax_failed_logins_per_minute = 0",
    ))
    .await;
    let body = json!({ "password": "a wrong guess" }).to_string();
    let request = |xff: &str| {
        format!(
            "POST /api/v1/auth/login HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: {xff}\r\n\
             Content-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let send = |xff: String| {
        let addr = server.addr;
        let req = request(&xff);
        async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(req.as_bytes()).await.unwrap();
            let mut answer = String::new();
            s.read_to_string(&mut answer).await.unwrap();
            answer.lines().next().unwrap_or_default().to_string()
        }
    };
    let mut abandoned = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    abandoned
        .write_all(request("198.51.100.77").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    drop(abandoned);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let busy = send("198.51.100.78".to_string()).await;
    assert!(
        busy.contains("429"),
        "a second check ran beside the abandoned one: {busy}"
    );
    // Once it has finished, its failure is counted: the shared slow-down for
    // unverified forwarded requests now asks this one to wait too.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = send("198.51.100.79".to_string()).await;
    assert!(
        after.contains("429"),
        "the abandoned guess was not counted: {after}"
    );
}

// ── Forwards, the Funnel marker and loopback entries ─────────────────────

/// A raw TCP forward onto dux's port (here a Funnel TCP forward, which carries
/// no Funnel marker) beside a non-Funnel `tailscale serve` HTTPS route: a
/// stranger who comes in through the forward lands on loopback and can write
/// every header the serve route would have written. dux distrusts loopback
/// and the Tailscale listener while it knows of the forward, so it must not
/// take such a forged "tailscale serve" request for the tailnet either.
#[tokio::test]
async fn a_known_forward_never_lets_forged_serve_headers_pass_as_the_tailnet() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Funnel);
    exposure.set_identity(Some(IdentityFacts {
        funnel_any: true,
        forward_to_dux: true,
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        let cell = exposure.clone();
        move |p| p.with_live_exposure(cell)
    });
    // The bare stream the forward hands dux, with headers the stranger chose.
    let forged = Req::new(Method::GET, "/api/v1/projects")
        .header("host", "box.tail0000.ts.net")
        .header("tailscale-user-login", "owner@example.com")
        .header("x-forwarded-for", "100.64.0.9");
    // Unforwarded loopback is already the network here.
    assert_auth_required(
        &dux.get(THIS_MACHINE, "/api/v1/projects").await,
        "plain loopback under a forward",
    );
    assert_auth_required(
        &dux.send(THIS_MACHINE, forged).await,
        "forged serve headers through the forward",
    );
}

/// A client on the network whose direct address dux can see is blocked after
/// `max_failed_logins`. Adding a Tailscale Funnel marker header (which nothing
/// stops a LAN client from sending) must not turn that verified address into
/// an unverified one that is only slowed and never blocked.
#[tokio::test]
async fn a_lan_client_cannot_dodge_the_block_with_a_funnel_header() {
    let dux = Dux::with_password(
        "max_failed_logins = 2\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0",
    );
    for _ in 0..4 {
        let _ = dux
            .send(
                NETWORK,
                Req::new(Method::POST, "/api/v1/auth/login")
                    .json(json!({ "password": "wrong guess" }))
                    .header("tailscale-funnel-request", "?1"),
            )
            .await;
    }
    let config = dux.config();
    assert!(
        config.contains("198.51.100.7"),
        "the direct LAN peer was never blocked: {config}"
    );
    let after = dux.get(NETWORK, "/api/v1/auth/status").await;
    assert_eq!(after.error().as_deref(), Some("blocked"), "{}", after.body);
}

/// Control: the same client without the header is blocked.
#[tokio::test]
async fn a_lan_client_without_the_funnel_header_is_blocked() {
    let dux = Dux::with_password(
        "max_failed_logins = 2\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0",
    );
    for _ in 0..4 {
        let _ = dux
            .send(
                NETWORK,
                Req::new(Method::POST, "/api/v1/auth/login")
                    .json(json!({ "password": "wrong guess" })),
            )
            .await;
    }
    assert!(dux.config().contains("198.51.100.7"));
}

/// "This machine is never blocked", and, by the admission layer's own word, a
/// loopback address is never blocked even when dux cannot vouch for it. A
/// blocklist entry covering loopback must not lock the owner's own browser on
/// this machine out while dux merely cannot check Tailscale, nor refuse every
/// request that `tailscale serve` relays from loopback.
#[tokio::test]
async fn a_loopback_peer_is_never_refused_by_the_blocklist() {
    use dux_web::exposure::{ExposureCell, FunnelState};
    let unchecked = ExposureCell::new(FunnelState::Unchecked);
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nblocked_addresses = [\"127.0.0.0/8\", \"::1\"]",
            hash_of(PASSWORD)
        ),
        {
            let cell = unchecked.clone();
            move |p| p.with_live_exposure(cell)
        },
    );
    let mine = dux.get(THIS_MACHINE, "/api/v1/auth/status").await;
    assert_ne!(
        mine.error().as_deref(),
        Some("blocked"),
        "the owner on this machine was refused as blocked: {}",
        mine.body
    );
}

/// The second half of the same rule: `tailscale serve` always connects from
/// loopback, so an entry covering loopback refuses every tailnet device it
/// relays, though dux never blocks a loopback address.
#[tokio::test]
async fn a_loopback_entry_never_refuses_a_tailscale_serve_request() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let served = ExposureCell::new(FunnelState::Open);
    served.set_identity(Some(IdentityFacts {
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nblocked_addresses = [\"127.0.0.0/8\"]",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(served),
    );
    let via_serve = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::GET, "/api/v1/projects")
                .header("host", "box.tail0000.ts.net")
                .header("tailscale-user-login", "owner@example.com")
                .header("x-forwarded-for", "100.64.0.9"),
        )
        .await;
    assert_ne!(
        via_serve.error().as_deref(),
        Some("blocked"),
        "a tailnet device through tailscale serve was refused for the loopback hop: {}",
        via_serve.body
    );
}

/// The forged serve request of the test above, wherever it is used.
fn forged_serve(request: Req) -> Req {
    request
        .header("host", "box.tail0000.ts.net")
        .header("origin", "https://box.tail0000.ts.net")
        .header("tailscale-user-login", "owner@example.com")
        .header("x-forwarded-for", "100.64.0.9")
}

/// A serve route to dux beside a known raw forward, or beside an exposure dux
/// could not confirm.
fn exposure_with_route(
    funnel: dux_web::exposure::FunnelState,
    forward_to_dux: bool,
) -> dux_web::exposure::ExposureCell {
    use dux_web::exposure::{ExposureCell, IdentityFacts};
    let exposure = ExposureCell::new(funnel);
    exposure.set_identity(Some(IdentityFacts {
        forward_to_dux,
        own_ips: own_tailscale_ips(),
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        ..IdentityFacts::default()
    }));
    exposure
}

/// While dux cannot confirm what reaches its port, a request bearing every
/// serve header is the network too: the gate runs before any branch can trust.
#[tokio::test]
async fn an_unconfirmed_exposure_never_lets_serve_headers_pass_as_the_tailnet() {
    use dux_web::exposure::FunnelState;
    for funnel in [
        FunnelState::Checking,
        FunnelState::Unconfirmed,
        FunnelState::CliNotFound,
        FunnelState::Unchecked,
    ] {
        let exposure = exposure_with_route(funnel, false);
        let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
            move |p| p.with_live_exposure(exposure)
        });
        assert_auth_required(
            &dux.send(
                THIS_MACHINE,
                forged_serve(Req::new(Method::GET, "/api/v1/projects")),
            )
            .await,
            &format!("serve headers under {funnel:?}"),
        );
    }
}

/// Through a known forward, forged serve headers cannot set the first
/// password: the request is the network.
#[tokio::test]
async fn a_known_forward_never_lets_forged_serve_headers_set_the_first_password() {
    use dux_web::exposure::FunnelState;
    let exposure = exposure_with_route(FunnelState::Open, true);
    let dux = Dux::start_tuned("", move |p| p.with_live_exposure(exposure));
    let refused = dux
        .send(
            THIS_MACHINE,
            forged_serve(
                Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
            ),
        )
        .await;
    assert_eq!(
        refused.error().as_deref(),
        Some("first_password_not_here"),
        "{}",
        refused.body
    );
    assert!(!dux.config().contains("$argon2id$"));
}

/// Through a known forward, the tailnet address forged serve headers name is
/// only a claim, so failed sign-ins never write it to the blocklist.
#[tokio::test]
async fn a_known_forward_never_bans_the_tailnet_address_forged_headers_name() {
    use dux_web::exposure::FunnelState;
    let exposure = exposure_with_route(FunnelState::Open, true);
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nmax_failed_logins = 2\nfailed_login_delay_seconds = 0\n\
             max_failed_logins_per_minute = 0",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(exposure),
    );
    for _ in 0..3 {
        let answer = dux
            .send(
                THIS_MACHINE,
                forged_serve(
                    Req::new(Method::POST, "/api/v1/auth/login")
                        .json(json!({ "password": "wrong" })),
                ),
            )
            .await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{}", answer.body);
    }
    let config = dux.config();
    assert!(!config.contains("100.64.0.9"), "{config}");
}

fn network_peer(last: u8) -> Arrival {
    Arrival::Tcp {
        peer: SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, last)),
            50000,
        ),
        local: NETWORK.local().unwrap(),
    }
}

/// Throttling is per trust level: a flood from the internet and from
/// unverified forwards never slows or refuses a verified network or tailnet
/// device, and a flood from verified network devices never touches the
/// tailnet.
#[tokio::test]
async fn a_flood_from_less_trusted_clients_never_slows_a_verified_device() {
    let dux = Dux::with_password(
        "max_failed_logins = 0\nfailed_login_delay_seconds = 30\nmax_failed_logins_per_minute = 2",
    );
    let wrong = || Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "password": "x" }));
    for n in 0..4u8 {
        let _ = dux
            .send(
                THIS_MACHINE,
                wrong().header("x-forwarded-for", &format!("192.0.2.{n}")),
            )
            .await;
        let _ = dux
            .send(
                THIS_MACHINE,
                wrong()
                    .header("tailscale-funnel-request", "?1")
                    .header("x-forwarded-for", &format!("203.0.113.{n}")),
            )
            .await;
    }
    // The unverified traffic is held to its own limit.
    let held = dux
        .send(
            THIS_MACHINE,
            wrong().header("x-forwarded-for", "192.0.2.99"),
        )
        .await;
    assert_eq!(held.status, StatusCode::TOO_MANY_REQUESTS, "{}", held.body);
    assert_eq!(
        dux.login(NETWORK, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        dux.login(TAILNET, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    // Verified network devices fill their own global limit...
    for n in 20..24u8 {
        let _ = dux.send(network_peer(n), wrong()).await;
    }
    let network_held = dux.login(network_peer(30), PASSWORD).await;
    assert_eq!(
        network_held.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        network_held.body
    );
    // ...and the tailnet is still let in.
    assert_eq!(
        dux.login(TAILNET, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

// ── Claimed against verified, the tailnet by look, own addresses ─────────

const MY_LAPTOP: Arrival = Arrival::Tcp {
    peer: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 7)),
        50000,
    ),
    local: SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 10)),
        3890,
    ),
};

/// A wrong login through a proxy on this machine dux cannot vouch for (plain
/// nginx `proxy_pass` passes the client's own X-Forwarded-For through), the
/// claimed address chosen by the attacker.
async fn forged_wrong_login(dux: &Dux, claimed: &str) -> Answer {
    dux.send(
        THIS_MACHINE,
        Req::new(Method::POST, "/api/v1/auth/login")
            .json(json!({ "password": "wrong guess" }))
            .header("x-forwarded-for", claimed),
    )
    .await
}

/// Unverified failures must never lock out a verified device. An attacker
/// whose forwarded requests CLAIM a verified device's address piles failures
/// onto the same counter the verified device is judged by, so that device's
/// first honest typo blocks it and writes its address to config.toml.
#[tokio::test]
async fn claimed_failures_never_count_toward_a_verified_device_ban() {
    let dux = Dux::with_password(
        "max_failed_logins = 3\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0",
    );
    for _ in 0..3 {
        let answer = forged_wrong_login(&dux, "198.51.100.7").await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{}", answer.body);
    }
    assert!(
        !dux.config().contains("198.51.100.7"),
        "the unverified claim itself is never written"
    );
    // The real device at that address, verified, types its password wrong once.
    let typo = dux
        .send(
            MY_LAPTOP,
            Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "password": "typo" })),
        )
        .await;
    assert_eq!(
        (typo.status, typo.error().as_deref()),
        (StatusCode::UNAUTHORIZED, Some("wrong_password")),
        "one typo from the verified device was answered: {}",
        typo.body
    );
    assert!(
        !dux.config().contains("198.51.100.7"),
        "the verified device was banned and written to config.toml after one typo: {}",
        dux.config()
    );
}

/// The same collision with the slow-down on: one forged failure makes the
/// verified device wait before its correct password is even checked.
#[tokio::test]
async fn a_claimed_failure_never_slows_the_verified_device_at_that_address() {
    let dux =
        Dux::with_password("failed_login_delay_seconds = 5\nmax_failed_logins_per_minute = 0");
    let forged = forged_wrong_login(&dux, "198.51.100.7").await;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED, "{}", forged.body);
    let mine = dux.login(MY_LAPTOP, PASSWORD).await;
    assert_eq!(
        mine.status,
        StatusCode::NO_CONTENT,
        "the verified device's correct password was refused: {}",
        mine.body
    );
}

/// A peer counts as the tailnet only when dux can prove it. A connection
/// whose two ends merely fall in 100.64.0.0/10 (carrier-grade NAT, Cloudflare
/// WARP, a cloud VPC) on an address that is NOT this machine's Tailscale
/// address is not a tailnet device, yet it skips the password under the
/// default `require`.
#[tokio::test]
async fn a_cgnat_peer_on_a_non_tailscale_address_is_not_the_tailnet() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    // A successful look: this machine's Tailscale address is 100.101.102.103.
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        own_ips: vec!["100.101.102.103".parse().unwrap()],
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        move |p| p.with_live_exposure(exposure)
    });
    // dux bound to every address; this connection arrived on the machine's
    // carrier-grade NAT interface address, from another host on that segment.
    let cgnat = Arrival::Tcp {
        peer: "100.72.9.9:50000".parse().unwrap(),
        local: "100.64.20.5:3890".parse().unwrap(),
    };
    assert_auth_required(
        &dux.get(cgnat, "/api/v1/projects").await,
        "a CGNAT neighbour on a non-Tailscale interface",
    );
    let status = dux.status(cgnat, None).await;
    assert_eq!(status["client_class"], "network", "{status}");
}

/// The same with Tailscale not installed at all (a failed look for a missing
/// CLI leaves the exposure open with no identity): nothing proves a tailnet.
#[tokio::test]
async fn without_tailscale_a_cgnat_peer_is_not_the_tailnet() {
    use dux_web::exposure::{ExposureCell, FunnelState};
    let exposure = ExposureCell::new(FunnelState::Open);
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        move |p| p.with_live_exposure(exposure)
    });
    let cgnat = Arrival::Tcp {
        peer: "100.72.9.9:50000".parse().unwrap(),
        local: "100.64.20.5:3890".parse().unwrap(),
    };
    assert_auth_required(
        &dux.get(cgnat, "/api/v1/projects").await,
        "a CGNAT neighbour with no Tailscale on this machine",
    );
}

/// "This machine is never blocked." A connection this machine makes to its own
/// non-loopback address (the owner opening the LAN URL, or any relay on this
/// machine aimed there) comes FROM that address, and dux bans and writes it.
#[tokio::test]
async fn this_machine_through_its_own_lan_address_is_never_written_to_the_blocklist() {
    let dux = Dux::with_password(
        "max_failed_logins = 2\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0",
    );
    let own = Arrival::Tcp {
        peer: "192.0.2.10:50000".parse().unwrap(),
        local: "192.0.2.10:3890".parse().unwrap(),
    };
    for _ in 0..3 {
        let _ = dux
            .send(
                own,
                Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "password": "nope" })),
            )
            .await;
    }
    assert!(
        !dux.config().contains("192.0.2.10"),
        "this machine's own address was written to the blocklist: {}",
        dux.config()
    );
}

// ── Buckets by arrival, own addresses, the current password ─────────────

/// A Funnel exposure whose look succeeded: dux knows a Funnel publishes it, so
/// this machine's own loopback requests are asked for the password.
fn funnel_exposure() -> dux_web::exposure::ExposureCell {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Funnel);
    exposure.set_identity(Some(IdentityFacts {
        funnel_any: true,
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    exposure
}

/// While a Funnel publishes dux and a password is set, the owner on this
/// machine must sign in too (loopback is distrusted). Their sign-in shares the
/// "unverified" slow-down bucket with every anonymous Funnel visitor, so ONE
/// wrong guess from the internet makes the owner's correct password wait, and
/// a guesser that keeps retrying the moment its own wait ends keeps the owner
/// out indefinitely. Default settings throughout.
#[tokio::test]
async fn a_funnel_guesser_never_slows_the_owner_signing_in_on_this_machine() {
    let exposure = funnel_exposure();
    let dux = Dux::start_tuned(&format!("password_hash = \"{}\"", hash_of(PASSWORD)), {
        move |p| p.with_live_exposure(exposure)
    });
    // The owner on this machine is indeed asked to sign in while the Funnel stands.
    assert_auth_required(
        &dux.get(THIS_MACHINE, "/api/v1/projects").await,
        "this machine under a Funnel",
    );
    // One wrong guess from an internet visitor through the Funnel.
    let guess = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/login")
                .json(json!({ "password": "wrong guess" }))
                .header("tailscale-funnel-request", "?1")
                .header("x-forwarded-for", "203.0.113.50"),
        )
        .await;
    assert_eq!(guess.status, StatusCode::UNAUTHORIZED, "{}", guess.body);
    // The owner, on this machine, types the right password.
    let mine = dux.login(THIS_MACHINE, PASSWORD).await;
    assert_eq!(
        mine.status,
        StatusCode::NO_CONTENT,
        "the owner's correct password on this machine was refused because an internet \
         visitor guessed wrong: {}",
        mine.body
    );
}

/// The same shared bucket, through the per-minute cap: Funnel visitors that
/// fill it lock the owner on this machine out for the rest of the minute.
#[tokio::test]
async fn a_funnel_flood_never_locks_out_the_owner_on_this_machine() {
    let exposure = funnel_exposure();
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 3",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(exposure),
    );
    for n in 0..3u8 {
        let _ = dux
            .send(
                THIS_MACHINE,
                Req::new(Method::POST, "/api/v1/auth/login")
                    .json(json!({ "password": "wrong guess" }))
                    .header("tailscale-funnel-request", "?1")
                    .header("x-forwarded-for", &format!("203.0.113.{n}")),
            )
            .await;
    }
    let mine = dux.login(THIS_MACHINE, PASSWORD).await;
    assert_eq!(
        mine.status,
        StatusCode::NO_CONTENT,
        "the owner on this machine was rate-limited by internet visitors: {}",
        mine.body
    );
}

/// "This machine is never blocked." The owner opening dux's LAN URL in a
/// browser on the machine itself connects FROM that LAN address. A blocklist
/// range the owner wrote for a hostile neighbourhood that happens to cover
/// the machine's own address refuses the owner's own browser.
#[tokio::test]
async fn this_machine_through_its_own_lan_address_is_never_refused_by_the_blocklist() {
    let dux = Dux::start("blocked_addresses = [\"192.0.2.0/24\"]");
    let own = Arrival::Tcp {
        peer: "192.0.2.10:50000".parse().unwrap(),
        local: "192.0.2.10:3890".parse().unwrap(),
    };
    let answer = dux.get(own, "/api/v1/auth/status").await;
    assert_ne!(
        answer.error().as_deref(),
        Some("blocked"),
        "this machine, on its own LAN address, was refused as blocked: {}",
        answer.body
    );
}

/// `max_failed_logins = 0` "never blocks anyone automatically; the slow-down
/// below still applies" (the setting's own comment). A wrong CURRENT password
/// skips the per-address slow-down, and the tailnet has no shared per-minute
/// limit, so with that setting a tailnet device (which needs no session under
/// the default `require`) can guess the password through the password route
/// as fast as dux hashes, with no slow-down at all.
#[tokio::test]
async fn wrong_current_passwords_are_slowed_down_too() {
    let dux = Dux::with_password("max_failed_logins = 0\nfailed_login_delay_seconds = 5");
    let mut answers = Vec::new();
    for n in 0..4 {
        let answer = dux
            .send(
                TAILNET,
                Req::new(Method::POST, "/api/v1/auth/password")
                    .json(json!({ "current": format!("guess number {n}"), "new": OTHER_PASSWORD })),
            )
            .await;
        answers.push((answer.status, answer.error()));
    }
    assert!(
        answers
            .iter()
            .skip(1)
            .any(|(status, _)| *status == StatusCode::TOO_MANY_REQUESTS),
        "four wrong current passwords in a row, none slowed down: {answers:?}"
    );
}

/// A Funnel flood never slows this machine reaching dux through one of its
/// own addresses either: that route has its own bucket.
#[tokio::test]
async fn a_funnel_flood_never_locks_out_this_machine_on_its_own_address() {
    let exposure = funnel_exposure();
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nfailed_login_delay_seconds = 5\nmax_failed_logins_per_minute = 2",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(exposure),
    );
    for n in 0..3u8 {
        let _ = dux
            .send(
                THIS_MACHINE,
                Req::new(Method::POST, "/api/v1/auth/login")
                    .json(json!({ "password": "wrong guess" }))
                    .header("tailscale-funnel-request", "?1")
                    .header("x-forwarded-for", &format!("203.0.113.{n}")),
            )
            .await;
    }
    let own = Arrival::Tcp {
        peer: "192.0.2.10:50000".parse().unwrap(),
        local: "192.0.2.10:3890".parse().unwrap(),
    };
    assert_eq!(
        dux.login(own, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// The documented limit: through a raw TCP forward onto dux's port, outsiders
/// arrive as plain loopback with nothing to tell them from the owner, so they
/// share the owner's bucket. This pins the limit the config comment states.
#[tokio::test]
async fn through_a_raw_forward_outsiders_share_the_plain_loopback_bucket() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        forward_to_dux: true,
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nfailed_login_delay_seconds = 5\nmax_failed_logins_per_minute = 0",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(exposure),
    );
    // An outsider through the forward: plain loopback, wrong guess.
    let guess = dux.login(THIS_MACHINE, "wrong guess").await;
    assert_eq!(guess.status, StatusCode::UNAUTHORIZED, "{}", guess.body);
    // The owner on plain loopback now waits with it.
    let mine = dux.login(THIS_MACHINE, PASSWORD).await;
    assert_eq!(mine.status, StatusCode::TOO_MANY_REQUESTS, "{}", mine.body);
}

// ── Concurrent guesses, serve from this machine, serve headers ───────────

/// After a failure an address waits before its next attempt is checked. Two
/// guesses sent at once from the same verified address must not both be
/// checked: the second is the "next attempt" and must wait (429), whatever
/// order the two arrive in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_guesses_from_one_address_are_not_all_checked() {
    let dux = Dux::with_password(
        "failed_login_delay_seconds = 30\nfailed_login_max_delay_seconds = 30\n\
         max_failed_logins = 0\nmax_failed_logins_per_minute = 0",
    );
    let (a, b) = tokio::join!(
        dux.login(NETWORK, "first wrong guess here"),
        dux.login(NETWORK, "second wrong guess here"),
    );
    let checked = [&a, &b]
        .iter()
        .filter(|answer| answer.status == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!(
        checked, 1,
        "both concurrent guesses were checked with no wait between them: {} {} / {} {}",
        a.status, a.body, b.status, b.body
    );
}

/// A `tailscale serve` request whose client is THIS machine's own Tailscale
/// address (a browser or a relay on this machine opening the serve URL). The
/// one own-address rule says such a peer is the network, never banned and
/// never written to config.toml. Failed sign-ins from it must not put this
/// machine's own Tailscale address into blocked_addresses.
#[tokio::test]
async fn serve_from_this_machines_own_tailscale_address_is_never_written_to_the_blocklist() {
    use dux_web::exposure::FunnelState;
    let own = own_tailscale_ips()[0].to_string();
    let exposure = exposure_with_route(FunnelState::Open, false);
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nmax_failed_logins = 2\nfailed_login_delay_seconds = 0\n\
             max_failed_logins_per_minute = 0",
            hash_of(PASSWORD)
        ),
        move |p| p.with_live_exposure(exposure),
    );
    let via_serve = |req: Req| {
        req.header("host", "box.tail0000.ts.net")
            .header("origin", "https://box.tail0000.ts.net")
            .header("tailscale-user-login", "owner@example.com")
            .header("x-forwarded-for", &own)
    };
    for _ in 0..3 {
        let _ = dux
            .send(
                THIS_MACHINE,
                via_serve(
                    Req::new(Method::POST, "/api/v1/auth/login")
                        .json(json!({ "password": "wrong" })),
                ),
            )
            .await;
    }
    let config = dux.config();
    assert!(
        !config.contains(&format!("\"{own}\"")),
        "this machine's own Tailscale address was written to the blocklist:\n{config}"
    );
}

/// While a Funnel reaches dux, a request from this machine over plain
/// loopback is the network and signs in. The same machine opening the
/// `tailscale serve` URL arrives from its own Tailscale address, which the
/// own-address rule also makes the network, so it must sign in as well
/// rather than pass as the tailnet with no password.
#[tokio::test]
async fn under_a_funnel_this_machine_through_the_serve_url_still_signs_in() {
    use dux_web::exposure::FunnelState;
    let own = own_tailscale_ips()[0].to_string();
    let exposure = exposure_with_route(FunnelState::Funnel, false);
    let dux = Dux::start_tuned(
        &format!("password_hash = \"{}\"", hash_of(PASSWORD)),
        move |p| p.with_live_exposure(exposure),
    );
    assert_auth_required(
        &dux.get(THIS_MACHINE, "/api/v1/projects").await,
        "plain loopback under a Funnel",
    );
    let answer = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::GET, "/api/v1/projects")
                .header("host", "box.tail0000.ts.net")
                .header("tailscale-user-login", "owner@example.com")
                .header("x-forwarded-for", &own),
        )
        .await;
    assert_auth_required(
        &answer,
        "this machine's own Tailscale address through tailscale serve under a Funnel",
    );
}

/// A tailnet device signing in through `tailscale serve` is verified by the
/// address Tailscale's proxy wrote, and is blocked after `max_failed_logins`.
/// Tailscale's proxy passes a client's own `X-Real-IP` through untouched, so a
/// guesser on the tailnet that adds one must not turn its verified, bannable
/// address into a mere claim that is only ever slowed: the same dodge the
/// Funnel marker was refused for on a direct connection.
#[tokio::test]
async fn a_tailnet_guesser_through_serve_cannot_dodge_the_block_with_x_real_ip() {
    use dux_web::exposure::FunnelState;
    let start = || {
        let exposure = exposure_with_route(FunnelState::Open, false);
        Dux::start_tuned(
            &format!(
                "password_hash = \"{}\"\nrequire = \"tailnet\"\nmax_failed_logins = 2\n\
                 failed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0",
                hash_of(PASSWORD)
            ),
            move |p| p.with_live_exposure(exposure),
        )
    };
    let guess = |extra: Option<(&'static str, &'static str)>| {
        let mut req = forged_serve(
            Req::new(Method::POST, "/api/v1/auth/login").json(json!({ "password": "wrong" })),
        );
        if let Some((name, value)) = extra {
            req = req.header(name, value);
        }
        req
    };

    // Control: the plain serve request is blocked at the limit and written.
    let plain = start();
    for _ in 0..2 {
        let _ = plain.send(THIS_MACHINE, guess(None)).await;
    }
    let third = plain.send(THIS_MACHINE, guess(None)).await;
    assert_eq!(third.error().as_deref(), Some("blocked"), "{}", third.body);
    assert!(plain.config().contains("\"100.64.0.9\""));

    // The same device adding X-Real-IP keeps guessing and is never blocked.
    let dodging = start();
    let mut answers = Vec::new();
    for _ in 0..4 {
        let answer = dodging
            .send(THIS_MACHINE, guess(Some(("x-real-ip", "203.0.113.1"))))
            .await;
        answers.push(format!("{} {}", answer.status, answer.body));
    }
    assert!(
        dodging.config().contains("\"100.64.0.9\""),
        "the tailnet guesser was never blocked after {} failures: {answers:#?}",
        answers.len()
    );
}

/// A right password whose address is blocked while its check runs gets no
/// session: admission is judged again when the session is issued. The wrong
/// guess holds the one check slot, so the right one is checked only after the
/// wrong one has blocked the address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_right_password_racing_a_block_gets_no_session() {
    let dux = Dux::with_password(
        "max_failed_logins = 1\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 0\n\
         max_concurrent_password_checks = 1\npassword_check_queue = 1",
    );
    let wrong = dux.login(NETWORK, "wrong guess");
    let right = async {
        tokio::time::sleep(Duration::from_millis(5)).await;
        dux.login(NETWORK, PASSWORD).await
    };
    let (wrong, right) = tokio::join!(wrong, right);
    assert_eq!(wrong.error().as_deref(), Some("blocked"), "{}", wrong.body);
    assert_eq!(right.error().as_deref(), Some("blocked"), "{}", right.body);
    assert!(
        right.headers.get("set-cookie").is_none(),
        "a blocked address was handed a session: {:?}",
        right.headers
    );
}

// ── IPv6 prefixes, planted cookies, the first-password route ────────────

fn v6(peer: &str) -> Arrival {
    Arrival::Tcp {
        peer: SocketAddr::new(peer.parse().unwrap(), 50000),
        local: "[2001:db8:ffff::10]:3890".parse().unwrap(),
    }
}

/// A single LAN device that owns an IPv6 /64 (every SLAAC host does) rotates
/// the address it sends from. The /64's combined failures reaching
/// `max_failed_logins` write the whole /64 as one range, so a fresh address in
/// it is refused too, while the owner on another network signs in.
#[tokio::test]
async fn rotating_ipv6_addresses_in_one_slash64_gets_the_slash64_blocked() {
    let dux = Dux::with_password(
        "max_failed_logins = 5\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 30",
    );
    for n in 1..=5u32 {
        let answer = dux
            .login(
                v6(&format!("2001:db8:1:1::{n:x}")),
                "not the password at all",
            )
            .await;
        assert!(answer.status.is_client_error(), "{}", answer.body);
    }
    let config = dux.config();
    assert!(config.contains("\"2001:db8:1:1::/64\""), "{config}");
    assert!(!config.contains("\"2001:db8:1:1::5\""), "{config}");
    let fresh = dux.login(v6("2001:db8:1:1::abcd"), PASSWORD).await;
    assert_eq!(fresh.error().as_deref(), Some("blocked"), "{}", fresh.body);
    // The owner, from another network, with the right password.
    let owner = dux.login(v6("2001:db8:2:2::5"), PASSWORD).await;
    assert_eq!(owner.status, StatusCode::NO_CONTENT, "{}", owner.body);

    // The wait follows the /64 too, and says so.
    let slowed = Dux::with_password("failed_login_delay_seconds = 30\nmax_failed_logins = 0");
    let first = slowed.login(v6("2001:db8:1:1::1"), "wrong").await;
    assert_eq!(first.status, StatusCode::UNAUTHORIZED, "{}", first.body);
    let rotated = slowed.login(v6("2001:db8:1:1::2"), "wrong").await;
    assert_eq!(
        rotated.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        rotated.body
    );
    assert_eq!(
        rotated.json()["from"],
        json!("from this address's network (its IPv6 /64)")
    );
}

/// A refusal caused by a shared count says so: the verified network's
/// per-minute limit is "from the network", never "from this address", which
/// would be false for the owner's first attempt.
#[tokio::test]
async fn a_shared_limit_never_says_from_this_address() {
    let dux = Dux::with_password(
        "max_failed_logins = 0\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 2",
    );
    for n in 1..=2u8 {
        let peer = Arrival::Tcp {
            peer: SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, n)),
                50000,
            ),
            local: NETWORK.local().unwrap(),
        };
        let _ = dux.login(peer, "wrong").await;
    }
    let owner = dux.login(NETWORK, PASSWORD).await;
    assert_eq!(
        owner.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        owner.body
    );
    let message = owner.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!message.contains("this address"), "{message}");
    assert!(message.contains("from the network"), "{message}");
    assert_eq!(owner.json()["from"], json!("from the network"));

    // Its own wait still says it is its own.
    let own = Dux::with_password("failed_login_delay_seconds = 30\nmax_failed_logins = 0");
    let _ = own.login(NETWORK, "wrong").await;
    let again = own.login(NETWORK, "wrong").await;
    assert_eq!(again.json()["from"], json!("from this address"));
}

/// Cookies are not isolated by port, so any page served from the same host
/// name on another port (an agent's dev server the owner previews, say) can
/// set `dux_session_<port>` with a more specific `Path`. The browser then
/// sends that one first, beside the real session. dux reads only the first
/// non-empty value, so the owner's valid session is ignored: every request is
/// 401, and signing in again cannot help (the new cookie is `Path=/`, and the
/// planted one still comes first).
#[tokio::test]
async fn a_planted_cookie_with_the_same_name_hides_a_valid_session() {
    let dux = Dux::with_password("");
    let valid = dux.signed_in(NETWORK).await;
    let (name, _) = valid.split_once('=').unwrap();
    // A well-formed token dux never issued, as a page on another port can set.
    let planted = format!("{name}={}", "A".repeat(43));
    for both in [format!("{planted}; {valid}"), format!("{valid}; {planted}")] {
        let status = dux.status(NETWORK, Some(&both)).await;
        let projects = dux
            .send(
                NETWORK,
                Req::new(Method::GET, "/api/v1/projects").cookie(&both),
            )
            .await;
        assert!(
            status["signed_in"] == json!(true) && projects.status == StatusCode::OK,
            "the browser holds a valid session, yet dux answers signed_in = {} and {} {} ({both})",
            status["signed_in"],
            projects.status,
            projects.body
        );
    }
    // Signing out with both revokes the valid one, whichever came first.
    let both = format!("{planted}; {valid}");
    let out = dux
        .send(
            NETWORK,
            Req::new(Method::POST, "/api/v1/auth/logout").cookie(&both),
        )
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.body);
    assert!(out.set_cookie().contains("Path=/"), "{}", out.set_cookie());
    assert_auth_required(
        &dux.send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").cookie(&valid),
        )
        .await,
        "the session after signing out",
    );
}

/// The first-password refusal explains itself with "This browser reached dux
/// over loopback" whenever classification recorded a reason, but that reason
/// is also recorded for connections that never touched loopback: this machine
/// opening its own LAN address, and a tailnet device on the Tailscale
/// listener while dux cannot confirm its exposure. Both are told something
/// false about how they connected.
#[tokio::test]
async fn the_first_password_refusal_never_says_loopback_for_a_connection_that_was_not() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let unconfirmed = ExposureCell::new(FunnelState::Unconfirmed);
    unconfirmed.set_identity(Some(IdentityFacts {
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned("", |params| params.with_live_exposure(unconfirmed));
    let own_lan = Arrival::Tcp {
        peer: "192.0.2.10:40000".parse().unwrap(),
        local: "192.0.2.10:3890".parse().unwrap(),
    };
    for (from, what) in [
        (TAILNET, "the Tailscale listener"),
        (own_lan, "this machine's own LAN address"),
    ] {
        let answer = dux
            .send(
                from,
                Req::new(Method::POST, "/api/v1/auth/password").json(json!({ "new": PASSWORD })),
            )
            .await;
        assert_eq!(
            answer.error().as_deref(),
            Some("first_password_not_here"),
            "{what}: {}",
            answer.body
        );
        let message = answer.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains("over loopback"),
            "{what} never reached dux over loopback, yet is told: {message}"
        );
    }
}

// ── The tailnet over IPv6 ─────────────────────────────────────────────────

/// Every Tailscale node's IPv6 address shares one /64
/// (fd7a:115c:a1e0:ab12:4843:cd96::/96), and verified IPv6 failures are
/// counted per /64, so the whole tailnet is ONE failure counter over IPv6:
/// four wrong guesses from any tailnet device (a shared node, a guest's
/// laptop) and the owner's single typo from their own tailnet device write
/// the OWNER's address to `blocked_addresses`, after which the owner is
/// refused everywhere, the right password included.
#[tokio::test]
async fn a_tailnet_guesser_over_ipv6_never_gets_the_owners_device_banned() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let own: std::net::IpAddr = "fd7a:115c:a1e0:ab12:4843:cd96:6265:6501".parse().unwrap();
    let arrival = |peer: &str| Arrival::Tcp {
        peer: SocketAddr::new(peer.parse().unwrap(), 50000),
        local: SocketAddr::new(own, 3890),
    };
    let guesser = arrival("fd7a:115c:a1e0:ab12:4843:cd96:6258:b240");
    let owner = arrival("fd7a:115c:a1e0:ab12:4843:cd96:626b:430b");
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nrequire = \"tailnet\"\nfailed_login_delay_seconds = 0\n",
            hash_of(PASSWORD)
        ),
        move |params| {
            let exposure = ExposureCell::new(FunnelState::Open);
            exposure.set_identity(Some(IdentityFacts {
                own_ips: vec![own],
                ..IdentityFacts::default()
            }));
            params.with_live_exposure(exposure)
        },
    );
    assert_eq!(
        dux.status(owner, None).await["client_class"],
        "tailnet",
        "the owner's device is the tailnet"
    );
    for _ in 0..4 {
        let wrong = dux.login(guesser, OTHER_PASSWORD).await;
        assert_eq!(wrong.status, StatusCode::UNAUTHORIZED, "{}", wrong.body);
    }
    let typo = dux.login(owner, OTHER_PASSWORD).await;
    let right = dux.login(owner, PASSWORD).await;
    assert_eq!(
        (typo.status, right.status),
        (StatusCode::UNAUTHORIZED, StatusCode::NO_CONTENT),
        "the owner's one typo after another device's guesses banned the owner: {} / {}\n{}",
        typo.body,
        right.body,
        dux.config()
    );
}

// ── Pending writes by file identity, process-wide admission ─────────────

/// Ask the running engine to reload and wait until `done` says the reload
/// landed (or give up after a few seconds).
async fn reload_until(dux: &Dux, done: impl Fn(&Value) -> bool) -> bool {
    let reload = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/config/reload").json(json!({})),
        )
        .await;
    assert_eq!(reload.status, StatusCode::OK, "{}", reload.body);
    for _ in 0..50 {
        let status = dux.status(THIS_MACHINE, None).await;
        if done(&status) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// The owner mistypes their password from their own device and is blocked.
/// dux's own follow-up reload is refused because the file already had an
/// unrelated `[server.auth]` problem (a ban is written whatever the file's
/// existing problems are). The owner then does exactly what the log line tells
/// them: removes their address from `blocked_addresses` (fixing the other
/// problem too) and reloads. The reload is accepted, the file no longer lists
/// the address, and yet the address stays refused: the in-memory ban is a
/// pending change that was never retired, and a stored list equal to the one
/// before the ban is taken as "read before the write", so the ban is put back.
#[tokio::test]
async fn removing_a_ban_and_reloading_lifts_it_even_when_the_first_reload_failed() {
    let dux = Dux::with_password("max_failed_logins = 2\nfailed_login_delay_seconds = 0");
    let path = dux.tmp.path().join("config.toml");
    // A hand edit not yet reloaded: an invalid value in [server.auth].
    let good = dux.config();
    std::fs::write(
        &path,
        good.replace(
            "max_failed_logins = 2",
            "max_failed_logins = 2\nrequire = \"nowhere\"",
        ),
    )
    .unwrap();

    for _ in 0..2 {
        dux.login(NETWORK, "not the password at all").await;
    }
    assert_eq!(
        dux.get(NETWORK, "/api/v1/auth/status").await.status,
        StatusCode::FORBIDDEN,
        "blocked"
    );
    assert!(
        dux.config().contains("\"198.51.100.7\""),
        "{}",
        dux.config()
    );

    // dux's own reload after the ban: refused, the file has a problem.
    let _ = reload_until(&dux, |_| false).await;

    // The owner removes the address, fixes the problem, and reloads. The
    // minimum length moves too, so the test can see the reload landed.
    let fixed = good.replace(
        "max_failed_logins = 2",
        "max_failed_logins = 2\nminimum_password_length = 13",
    );
    assert!(!fixed.contains("198.51.100.7"));
    std::fs::write(&path, &fixed).unwrap();
    assert!(
        reload_until(&dux, |s| s["minimum_password_length"] == json!(13)).await,
        "the owner's reload landed"
    );
    let answer = dux.get(NETWORK, "/api/v1/auth/status").await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "the file no longer lists the address and the reload landed, yet it is still \
         refused: {}",
        answer.body
    );
}

/// Control for the probe above: with no pre-existing problem, dux's own reload
/// is accepted, and the owner's removal then lifts the ban.
#[tokio::test]
async fn removing_a_ban_lifts_it_when_the_first_reload_landed() {
    let dux = Dux::with_password("max_failed_logins = 2\nfailed_login_delay_seconds = 0");
    let path = dux.tmp.path().join("config.toml");
    let good = dux.config();
    for _ in 0..2 {
        dux.login(NETWORK, "not the password at all").await;
    }
    assert_eq!(
        dux.get(NETWORK, "/api/v1/auth/status").await.status,
        StatusCode::FORBIDDEN
    );
    // dux's own reload after the ban lands (moving nothing else).
    let with_ban = dux.config();
    std::fs::write(
        &path,
        with_ban.replace(
            "max_failed_logins = 2",
            "max_failed_logins = 2\nminimum_password_length = 14",
        ),
    )
    .unwrap();
    assert!(reload_until(&dux, |s| s["minimum_password_length"] == json!(14)).await);
    let fixed = good.replace(
        "max_failed_logins = 2",
        "max_failed_logins = 2\nminimum_password_length = 13",
    );
    std::fs::write(&path, &fixed).unwrap();
    assert!(reload_until(&dux, |s| s["minimum_password_length"] == json!(13)).await);
    assert_eq!(
        dux.get(NETWORK, "/api/v1/auth/status").await.status,
        StatusCode::OK
    );
}

/// A ban that could not be written (here `blocked_addresses` is already at
/// `max_blocked_addresses`) is said to hold "until dux restarts". It is held
/// by the serve's own auth state, though, and every new serve in the same
/// dux process (the terminal UI turning its background server off and on, or
/// a second start-web-server flip) builds a fresh one over the same engine:
/// the blocked address is let straight back in while dux keeps running.
#[tokio::test]
async fn a_ban_held_in_memory_survives_a_new_serve_in_the_same_dux() {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();
    let paths = dux_core::config::DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    std::fs::write(
        &paths.config_path,
        format!(
            "[server.auth]\npassword_hash = \"{}\"\nmax_failed_logins = 2\n\
             failed_login_delay_seconds = 0\nmax_blocked_addresses = 1\n\
             blocked_addresses = [\"203.0.113.250\"]\n",
            hash_of(PASSWORD)
        ),
    )
    .unwrap();
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    let (handle, _join) = spawn_engine_thread(engine);
    let serve = |handle: dux_web::engine_actor::EngineHandle| {
        build_app(
            handle,
            Router::<AppState>::new(),
            RouterParams::plain_http()
                .with_live_exposure(tailnet_exposure())
                .with_auth_reload(Arc::new(|| {})),
        )
    };
    let first = Dux {
        app: serve(handle.clone()),
        tmp,
        reloads: Arc::default(),
        handle: handle.clone(),
    };
    for _ in 0..2 {
        first.login(NETWORK, "not the password at all").await;
    }
    assert_eq!(
        first.get(NETWORK, "/api/v1/auth/status").await.status,
        StatusCode::FORBIDDEN,
        "blocked for this run, in memory only"
    );
    assert!(!first.config().contains("198.51.100.7"));

    // The same dux serves again.
    let again = Dux {
        app: serve(handle),
        ..first
    };
    let answer = again.get(NETWORK, "/api/v1/auth/status").await;
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "the ban was said to hold until dux restarts, but dux did not restart: {}",
        answer.body
    );
}

/// A PTY socket checks its session immediately before its handshake and its
/// replay: a session revoked while the socket was subscribing gets the 4401
/// close and not one byte of the terminal. The hook holds the socket in exactly
/// that window while the session is signed out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pty_socket_signed_out_while_subscribing_gets_no_bytes() {
    use tokio_tungstenite::tungstenite::Message;
    let (reached_tx, mut reached_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let release = Arc::new(tokio::sync::Notify::new());
    let hook: dux_web::auth::OpeningHook = {
        let release = Arc::clone(&release);
        Arc::new(move || {
            let reached_tx = reached_tx.clone();
            let release = Arc::clone(&release);
            Box::pin(async move {
                let _ = reached_tx.send(());
                release.notified().await;
            })
        })
    };
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nrequire = \"everywhere\"",
            hash_of(PASSWORD)
        ),
        move |p| p.with_socket_opening_hook(hook),
    );
    let cookie = dux.signed_in(THIS_MACHINE).await;
    let created = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/terminals").cookie(&cookie),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let tid = created.json()["terminal_id"].as_str().unwrap().to_string();
    let server = Serve::start(dux).await;
    // The cookie is named for the port a browser reached dux on.
    let (_, value) = cookie.split_once('=').unwrap();
    let on_server = format!("dux_session_{}={value}", server.addr.port());
    let mut socket = server
        .connect(&format!("/ws/terminals/{tid}/pty"), &[], Some(&on_server))
        .await;
    // The socket has subscribed and is about to send its handshake.
    if tokio::time::timeout(Duration::from_secs(10), reached_rx.recv())
        .await
        .is_err()
    {
        let first = tokio::time::timeout(Duration::from_secs(1), socket.next()).await;
        panic!("the socket never reached its opening check: {first:?}");
    }
    let out = server
        ._dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/logout").cookie(&cookie),
        )
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.body);
    release.notify_waiters();
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the socket answered");
    match first {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4401),
        other => panic!("the first frame after a sign-out was not the 4401 close: {other:?}"),
    }
}

// ── Header bytes that are not text ────────────────────────────────────────

/// Send `path` from `from` with raw header bytes (values that are legal HTTP
/// obs-text but not visible ASCII).
async fn send_raw(dux: &Dux, from: Arrival, path: &str, headers: &[(&str, &[u8])]) -> Answer {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        builder = builder.header("host", "localhost");
    }
    for (name, value) in headers {
        builder = builder.header(
            *name,
            axum::http::HeaderValue::from_bytes(value).expect("legal obs-text header value"),
        );
    }
    let mut built = builder.body(Body::empty()).unwrap();
    built
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(from));
    let response = dux.app.clone().oneshot(built).await.unwrap();
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

/// A blocked client behind a proxy on this machine (nginx's
/// `$proxy_add_x_forwarded_for` appends the real client to whatever the client
/// sent, in ONE header value) puts a single non-ASCII byte in its own
/// X-Forwarded-For. The value no longer reads as text, so dux drops the whole
/// header, the real address the proxy appended included, and the blocklist
/// never sees it.
#[tokio::test]
async fn an_obs_text_byte_in_x_forwarded_for_must_not_hide_a_blocked_client() {
    let dux = Dux::start("blocked_addresses = [\"203.0.113.9\"]");
    // Control: the plain shape is refused.
    let plain = send_raw(
        &dux,
        THIS_MACHINE,
        "/api/v1/projects",
        &[("x-forwarded-for", b"10.0.0.1, 203.0.113.9")],
    )
    .await;
    assert_eq!(plain.error().as_deref(), Some("blocked"), "{}", plain.body);
    // The same request with one obs-text byte in the client's own part.
    let poisoned = send_raw(
        &dux,
        THIS_MACHINE,
        "/api/v1/projects",
        &[("x-forwarded-for", b"\xff, 203.0.113.9")],
    )
    .await;
    assert_eq!(
        poisoned.error().as_deref(),
        Some("blocked"),
        "a blocked client escaped the blocklist with one byte: {} {}",
        poisoned.status,
        poisoned.body
    );
}

/// A tailnet device banned by dux comes back through `tailscale serve` with a
/// non-ASCII byte in an X-Forwarded-For of its own, which the serve proxy
/// joins with the address it appends. dux then no longer sees the banned
/// address at all and serves it.
#[tokio::test]
async fn a_banned_tailnet_device_must_stay_banned_through_tailscale_serve() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned("blocked_addresses = [\"100.64.0.9\"]", {
        let cell = exposure.clone();
        move |p| p.with_live_exposure(cell)
    });
    let serve = |xff: &'static [u8]| -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("host", b"box.tail0000.ts.net"),
            ("tailscale-user-login", b"owner@example.com"),
            ("x-forwarded-for", xff),
        ]
    };
    let plain = send_raw(
        &dux,
        THIS_MACHINE,
        "/api/v1/projects",
        &serve(b"100.64.0.9"),
    )
    .await;
    assert_eq!(plain.error().as_deref(), Some("blocked"), "{}", plain.body);
    let poisoned = send_raw(
        &dux,
        THIS_MACHINE,
        "/api/v1/projects",
        &serve(b"\xc3\xa9, 100.64.0.9"),
    )
    .await;
    assert_eq!(
        poisoned.error().as_deref(),
        Some("blocked"),
        "the banned tailnet device got in: {} {}",
        poisoned.status,
        poisoned.body
    );
}

/// Cookies are not isolated by port, so any other web app on the same host
/// (or a page planting one, the case review 6 was about) can put a cookie
/// whose value carries UTF-8 into the one Cookie header the browser sends.
/// dux then reads no cookie at all and the owner's real session is ignored:
/// every request is 401 and signing in again changes nothing.
#[tokio::test]
async fn a_utf8_cookie_from_another_app_must_not_hide_the_session() {
    let dux = Dux::with_password("require = \"everywhere\"");
    let cookie = dux.signed_in(NETWORK).await;
    let ok = send_raw(
        &dux,
        NETWORK,
        "/api/v1/projects",
        &[("cookie", cookie.as_bytes())],
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "control: {}", ok.body);
    let header = format!("theme=caf\u{e9}; {cookie}");
    let answer = send_raw(
        &dux,
        NETWORK,
        "/api/v1/projects",
        &[("cookie", header.as_bytes())],
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "a valid session beside another app's UTF-8 cookie was ignored: {}",
        answer.body
    );
}

/// With a password set, the same byte turns a banned tailnet device into an
/// unverified network client that may guess again (and, unverified, can never
/// be banned again).
#[tokio::test]
async fn a_banned_tailnet_device_cannot_guess_again_through_tailscale_serve() {
    use dux_web::exposure::{ExposureCell, FunnelState, IdentityFacts};
    let exposure = ExposureCell::new(FunnelState::Open);
    exposure.set_identity(Some(IdentityFacts {
        routes: vec![dux_core::tailscale::ServeRoute {
            url: "https://box.tail0000.ts.net".to_string(),
            funnel: false,
        }],
        own_ips: own_tailscale_ips(),
        ..IdentityFacts::default()
    }));
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nblocked_addresses = [\"100.64.0.9\"]",
            hash_of(PASSWORD)
        ),
        {
            let cell = exposure.clone();
            move |p| p.with_live_exposure(cell)
        },
    );
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("host", "box.tail0000.ts.net")
        .header("origin", "https://box.tail0000.ts.net")
        .header("content-type", "application/json")
        .header("tailscale-user-login", "owner@example.com");
    builder = builder.header(
        "x-forwarded-for",
        axum::http::HeaderValue::from_bytes(b"\xff, 100.64.0.9").unwrap(),
    );
    let mut built = builder
        .body(Body::from(
            json!({ "password": "a wrong guess" }).to_string(),
        ))
        .unwrap();
    built
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(THIS_MACHINE));
    let response = dux.app.clone().oneshot(built).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "the banned device's password guess was checked"
    );
}

/// The events socket checks its session before its opening frames too: one
/// signed out while it opens gets the 4401 close, not its connection id, the
/// status snapshot or the late warnings.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_events_socket_signed_out_while_opening_gets_nothing() {
    use tokio_tungstenite::tungstenite::Message;
    let (reached_tx, mut reached_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let release = Arc::new(tokio::sync::Notify::new());
    let hook: dux_web::auth::OpeningHook = {
        let release = Arc::clone(&release);
        Arc::new(move || {
            let reached_tx = reached_tx.clone();
            let release = Arc::clone(&release);
            Box::pin(async move {
                let _ = reached_tx.send(());
                release.notified().await;
            })
        })
    };
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nrequire = \"everywhere\"",
            hash_of(PASSWORD)
        ),
        move |p| p.with_socket_opening_hook(hook),
    );
    let cookie = dux.signed_in(THIS_MACHINE).await;
    let server = Serve::start(dux).await;
    let (_, value) = cookie.split_once('=').unwrap();
    let on_server = format!("dux_session_{}={value}", server.addr.port());
    let mut socket = server.connect("/ws/events", &[], Some(&on_server)).await;
    tokio::time::timeout(Duration::from_secs(10), reached_rx.recv())
        .await
        .expect("the socket reached its opening check")
        .unwrap();
    let out = server
        ._dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/logout").cookie(&cookie),
        )
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.body);
    release.notify_waiters();
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the socket answered");
    match first {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4401),
        other => panic!("the first frame after a sign-out was not the 4401 close: {other:?}"),
    }
}

/// A PTY socket judges its session before it subscribes, since a subscribe
/// can launch an agent's provider: signed out in that window it gets 4401,
/// never the provider-gone 4001 that a failed subscribe (here, a terminal
/// deleted meanwhile) would answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pty_socket_signed_out_before_subscribing_never_subscribes() {
    use tokio_tungstenite::tungstenite::Message;
    let (reached_tx, mut reached_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let release = Arc::new(tokio::sync::Notify::new());
    let hook: dux_web::auth::OpeningHook = {
        let release = Arc::clone(&release);
        Arc::new(move || {
            let reached_tx = reached_tx.clone();
            let release = Arc::clone(&release);
            Box::pin(async move {
                let _ = reached_tx.send(());
                release.notified().await;
            })
        })
    };
    let dux = Dux::start_tuned(
        &format!(
            "password_hash = \"{}\"\nrequire = \"everywhere\"",
            hash_of(PASSWORD)
        ),
        move |p| p.with_socket_opening_hook(hook),
    );
    let cookie = dux.signed_in(THIS_MACHINE).await;
    let created = dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/terminals").cookie(&cookie),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let tid = created.json()["terminal_id"].as_str().unwrap().to_string();
    let server = Serve::start(dux).await;
    let (_, value) = cookie.split_once('=').unwrap();
    let on_server = format!("dux_session_{}={value}", server.addr.port());
    let mut socket = server
        .connect(&format!("/ws/terminals/{tid}/pty"), &[], Some(&on_server))
        .await;
    tokio::time::timeout(Duration::from_secs(10), reached_rx.recv())
        .await
        .expect("the socket reached its opening check before subscribing")
        .unwrap();
    // The terminal goes away and the session ends: a subscribe now would fail
    // and answer 4001, so 4401 proves the session was judged first.
    let gone = server
        ._dux
        .send(
            THIS_MACHINE,
            Req::new(Method::DELETE, &format!("/api/v1/terminals/{tid}")).cookie(&cookie),
        )
        .await;
    assert!(gone.status.is_success(), "{} {}", gone.status, gone.body);
    let out = server
        ._dux
        .send(
            THIS_MACHINE,
            Req::new(Method::POST, "/api/v1/auth/logout").cookie(&cookie),
        )
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.body);
    release.notify_waiters();
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the socket answered");
    match first {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4401),
        other => panic!("a signed-out socket was not closed with 4401: {other:?}"),
    }
}

// ── a /64 ban on the server's own LAN segment ─────────────────

/// A device on dux's own LAN, where SLAAC hands every host an address in the
/// same /64 dux itself is on.
fn lan(peer: &str) -> Arrival {
    Arrival::Tcp {
        peer: SocketAddr::new(peer.parse().unwrap(), 50000),
        local: "[2001:db8:1:1::10]:3890".parse().unwrap(),
    }
}

/// A guest on the owner's Wi-Fi (one address, never rotating) guesses until it
/// is banned as itself; one typo from the owner's phone afterwards then bans
/// the whole LAN /64, the segment dux's own address is on, and the owner's
/// laptop, which never failed once, is locked out with the right password.
#[tokio::test]
async fn a_guest_and_one_owner_typo_lock_the_owners_laptop_out() {
    let dux = Dux::with_password(
        "max_failed_logins = 5\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 30",
    );
    // The owner's laptop signs in first and is working.
    let cookie = dux.signed_in(lan("2001:db8:1:1::cccc")).await;
    // A guest on the same Wi-Fi guesses from ONE address and is banned as itself.
    for _ in 0..5 {
        let _ = dux.login(lan("2001:db8:1:1::bbbb"), "guess").await;
    }
    assert!(
        dux.config().contains("\"2001:db8:1:1::bbbb\""),
        "{}",
        dux.config()
    );
    // The owner's phone mistypes once.
    let typo = dux.login(lan("2001:db8:1:1::aaaa"), "typo").await;
    let config = dux.config();
    assert!(
        !config.contains("\"2001:db8:1:1::/64\""),
        "one typo by the owner banned the whole LAN /64, the one dux's own \
         address 2001:db8:1:1::10 is on ({}): {config}",
        typo.body
    );
    let mut req = Req::new(Method::GET, "/api/v1/auth/status");
    req.cookie = Some(cookie);
    let laptop = dux.send(lan("2001:db8:1:1::cccc"), req).await;
    assert_ne!(laptop.status, StatusCode::FORBIDDEN, "{}", laptop.body);
}

/// A /64 dux is not on still gets the range ban, on the same dux that keeps
/// its own LAN out of it: rotating through it writes the /64 as one entry.
#[tokio::test]
async fn a_slash64_dux_is_not_on_still_gets_the_range_ban() {
    let dux = Dux::with_password(
        "max_failed_logins = 3\nfailed_login_delay_seconds = 0\nmax_failed_logins_per_minute = 30",
    );
    let outside = |peer: &str| Arrival::Tcp {
        peer: SocketAddr::new(peer.parse().unwrap(), 50000),
        local: "[2001:db8:1:1::10]:3890".parse().unwrap(),
    };
    for n in 1..=3u32 {
        let _ = dux
            .login(outside(&format!("2001:db8:9:9::{n:x}")), "guess")
            .await;
    }
    assert!(
        dux.config().contains("\"2001:db8:9:9::/64\""),
        "{}",
        dux.config()
    );
    let fresh = dux.login(outside("2001:db8:9:9::abcd"), PASSWORD).await;
    assert_eq!(fresh.error().as_deref(), Some("blocked"), "{}", fresh.body);
}

// ── What a blocked address and a misconfigured section are told ──────────

/// A document navigation from a blocked address (a fresh page load) gets a
/// small HTML page that says so, with status 403 and no asset to fetch; every
/// other request from it keeps the JSON refusal the app reads.
#[tokio::test]
async fn a_blocked_page_load_gets_html_and_everything_else_json() {
    let dux = Dux::start("blocked_addresses = [\"198.51.100.7\"]");
    for (name, value) in [
        ("accept", "text/html,application/xhtml+xml,*/*;q=0.8"),
        ("sec-fetch-mode", "navigate"),
    ] {
        let page = dux
            .send(NETWORK, Req::new(Method::GET, "/").header(name, value))
            .await;
        assert_eq!(page.status, StatusCode::FORBIDDEN, "{name}");
        let kind = page
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(kind.starts_with("text/html"), "{name}: {kind}");
        assert!(
            page.body.contains("This address is blocked"),
            "{}",
            page.body
        );
        assert!(page.body.contains("blocked_addresses"), "{}", page.body);
        // The duck rides in the page as a data URI, because a blocked address
        // is refused /favicon.png like everything else.
        assert!(
            page.body.contains("<img src=\"data:image/png;base64,"),
            "the page carries the logo inline: {}",
            &page.body[..page.body.len().min(2000)]
        );
        let fetching: Vec<&str> = page
            .body
            .match_indices("src=")
            .chain(page.body.match_indices("href="))
            .chain(page.body.match_indices("url("))
            .chain(page.body.match_indices("@import"))
            .map(|(at, _)| &page.body[at..(at + 24).min(page.body.len())])
            .filter(|attr| !attr.starts_with("src=\"data:image/"))
            .collect();
        assert!(
            fetching.is_empty(),
            "the page fetches nothing: {fetching:?}"
        );
    }
    let api = dux
        .send(
            NETWORK,
            Req::new(Method::GET, "/api/v1/projects").header("accept", "application/json"),
        )
        .await;
    assert_eq!(api.status, StatusCode::FORBIDDEN);
    assert_eq!(api.error().as_deref(), Some("blocked"), "{}", api.body);
    // An asset is refused as JSON too: a blocked address is refused everything.
    let asset = dux.get(NETWORK, "/favicon.png").await;
    assert_eq!(asset.error().as_deref(), Some("blocked"), "{}", asset.body);
}
