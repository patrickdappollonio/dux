//! Static-asset serving, including the guard that the page baked into the binary
//! is a REAL frontend build.
//!
//! ## Why the real-build guard exists
//!
//! `crates/dux-web/build.rs` used to turn a failed `npm run build` into a
//! placeholder page plus a `cargo:warning`, and let `cargo build` succeed. The
//! test that should have caught it asserted the served page contained
//! `<!doctype html` OR `id="root"`, and the placeholder contained BOTH, so a
//! release could have shipped four platform binaries with no web UI and every
//! check green. `doctype_and_root_element_are_not_evidence_of_a_build` pins that
//! finding so nobody reintroduces the weak assertion, and
//! `index_references_a_real_hashed_asset_that_is_actually_served` is the check
//! that a placeholder cannot satisfy: it demands a content-hashed bundle
//! reference in the page AND that fetching that bundle succeeds.
//!
//! ## The skip
//!
//! With `DUX_DISABLE_UI_BUILD` set and no previously built `web/dist`, there is no
//! real build to assert on, so the tests that need one SKIP via
//! [`require_real_ui_build`] rather than pass. A silently-passing test is exactly
//! how the original defect survived, so the reason is printed, and build.rs also
//! emits a `cargo:warning` on that path so the reason surfaces even when the test
//! harness captures stdout. A skipped test is still a hiding place, which is why
//! the release workflow refuses to build with `DUX_DISABLE_UI_BUILD` set at all.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use dux_core::config::DuxPaths;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use tower::ServiceExt;

/// Return early from a test that requires a genuine frontend build, printing why.
/// Run with `cargo test -- --nocapture` to see the line; the build script's
/// `cargo:warning` says the same thing unconditionally.
macro_rules! require_real_ui_build {
    ($test:literal) => {
        if dux_web::web_assets::ui_build_skipped() {
            println!(
                "SKIPPED {}: this binary was built with DUX_DISABLE_UI_BUILD set and no \
                 previously built web/dist, so the embedded page is build.rs's \"web UI not \
                 built\" notice and there is no real frontend build to assert on. Unset \
                 DUX_DISABLE_UI_BUILD and rebuild to run this test.",
                $test
            );
            return;
        }
    };
}

fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
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
    (tmp, paths)
}

#[tokio::test]
async fn serves_embedded_index_at_root() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    assert!(
        html.contains("<!doctype html") || html.contains("id=\"root\""),
        "not the SPA index: {html}"
    );
}

#[tokio::test]
async fn unknown_path_falls_back_to_index() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/some/client/route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Build a router backed by a throwaway engine for static-asset assertions.
fn test_router() -> (tempfile::TempDir, axum::Router) {
    let (tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    (tmp, router(handle))
}

async fn get(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn manifest_served_with_manifest_mime() {
    require_real_ui_build!("manifest_served_with_manifest_mime");
    let (_tmp, app) = test_router();
    let resp = get(app, "/manifest.webmanifest").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "content-type").as_deref(),
        Some("application/manifest+json"),
        "the web manifest must advertise the PWA manifest MIME type"
    );
}

#[tokio::test]
async fn service_worker_served_no_cache_and_js_mime() {
    require_real_ui_build!("service_worker_served_no_cache_and_js_mime");
    let (_tmp, app) = test_router();
    let resp = get(app, "/sw.js").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "cache-control").as_deref(),
        Some("no-cache"),
        "the service worker must not be cached so SW updates are picked up promptly"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("javascript"),
        "sw.js must be served with a JavaScript MIME type, got {ctype}"
    );
}

#[tokio::test]
async fn missing_hashed_asset_returns_404_not_spa_fallback() {
    // A request for a hashed bundle chunk that does not exist must 404, NOT fall
    // back to index.html. Serving HTML for a `*.js` import() makes the browser
    // reject it as a module, which unmounts the React tree (white screen). This
    // happens after a rebuild+restart while a stale tab is still open.
    let (_tmp, app) = test_router();
    let resp = get(app, "/assets/nonexistent-deadbeef.js").await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a missing hashed asset must 404 so the client can recover, not get the SPA shell"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        !ctype.contains("html"),
        "a missing asset must not be served as text/html, got {ctype}"
    );
}

#[tokio::test]
async fn unknown_non_asset_path_still_serves_spa_shell() {
    // Client-side routes (anything outside `assets/`) must keep falling back to
    // the SPA index so deep links and the router keep working.
    let (_tmp, app) = test_router();
    let resp = get(app, "/some/client/route").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("html"),
        "an unknown non-asset path must serve the SPA shell as text/html, got {ctype}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    assert!(
        html.contains("<!doctype html") || html.contains("id=\"root\""),
        "the SPA shell must be served for client routes"
    );
}

#[tokio::test]
async fn offline_page_reachable_and_not_shadowed_by_spa_fallback() {
    require_real_ui_build!("offline_page_reachable_and_not_shadowed_by_spa_fallback");
    let (_tmp, app) = test_router();
    let resp = get(app, "/offline.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    // The real offline page, not the SPA index served by the fallback.
    assert!(
        html.contains("dux is unreachable"),
        "the offline page itself must be served, not the SPA index fallback"
    );
    assert!(
        !html.contains("id=\"root\""),
        "offline.html should not be the SPA shell"
    );
}

// ---------------------------------------------------------------------------
// The real-build guard.
// ---------------------------------------------------------------------------

/// Every `assets/...` path the given HTML references whose filename carries a
/// content hash, in document order.
///
/// Vite fingerprints its bundle output as `<name>-<hash>.<ext>`, so the test for
/// "this came out of a real build" is: the final dash-separated segment of the
/// filename stem is at least [`MIN_HASH_LEN`] characters of `[A-Za-z0-9_]`. That
/// accepts `assets/index-x8kEp4D8.js` and `assets/rolldown-runtime-QTnfLwEv.js`
/// (multiple dashes, the LAST segment is the hash) and rejects a hand-written
/// `assets/dux-logo.png`, along with any page that references no bundle at all.
///
/// Deliberately no digit or mixed-case requirement: a base64url hash that happens
/// to be all lowercase letters is unlikely but possible, and a gate that fails
/// once in a few thousand builds is worse than a slightly looser shape check. The
/// caller closes the remaining gap by FETCHING what it finds.
fn hashed_asset_refs(html: &str) -> Vec<String> {
    /// Vite's default hash length. Anything shorter is a human-chosen name.
    const MIN_HASH_LEN: usize = 8;

    let mut found = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("assets/") {
        rest = &rest[at..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '>' | ')' | '(' | '`'))
            .unwrap_or(rest.len());
        let (candidate, tail) = rest.split_at(end);
        rest = if tail.is_empty() { "" } else { &tail[1..] };
        let Some(file) = candidate.strip_prefix("assets/") else {
            continue;
        };
        // Nested asset directories are fine; the hash lives in the last segment.
        let file = file.rsplit('/').next().unwrap_or(file);
        let Some((stem, _ext)) = file.rsplit_once('.') else {
            continue;
        };
        let Some((_name, hash)) = stem.rsplit_once('-') else {
            continue;
        };
        if hash.chars().count() >= MIN_HASH_LEN
            && hash.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            found.push(candidate.to_string());
        }
    }
    found
}

/// The exact placeholder `build.rs` used to write on a failed frontend build, kept
/// verbatim so the assertions below are about the real thing and not a paraphrase.
const HISTORICAL_PLACEHOLDER: &str = "<!doctype html><title>dux</title><div id=\"root\">web assets not built - run npm run build in crates/dux-web/web</div>";

#[test]
fn doctype_and_root_element_are_not_evidence_of_a_build() {
    // THE FINDING, pinned. The assertion this suite used to make passes happily on
    // the placeholder, which is why a placeholder release would have been green.
    let placeholder = HISTORICAL_PLACEHOLDER.to_lowercase();
    assert!(
        placeholder.contains("<!doctype html") && placeholder.contains("id=\"root\""),
        "the placeholder satisfies both halves of the old assertion, so the old \
         assertion could never have caught it"
    );
    // The real check rejects it, because it references no built bundle.
    assert!(
        hashed_asset_refs(HISTORICAL_PLACEHOLDER).is_empty(),
        "the placeholder must not look like a build"
    );
}

#[test]
fn hashed_asset_refs_finds_vite_bundles_and_ignores_hand_named_files() {
    let real = r#"<!doctype html><html><head>
        <link rel="icon" type="image/png" href="./favicon.png" />
        <script type="module" crossorigin src="./assets/index-x8kEp4D8.js"></script>
        <link rel="modulepreload" crossorigin href="./assets/rolldown-runtime-QTnfLwEv.js">
        <link rel="stylesheet" crossorigin href="./assets/index-TXcOh_oH.css">
        <img src="./assets/dux-logo.png">
        </head><body><div id="root"></div></body></html>"#;
    assert_eq!(
        hashed_asset_refs(real),
        vec![
            "assets/index-x8kEp4D8.js".to_string(),
            "assets/rolldown-runtime-QTnfLwEv.js".to_string(),
            "assets/index-TXcOh_oH.css".to_string(),
        ],
        "the hashed bundle entries must be found and the hand-named logo skipped"
    );
    // Names a human chose, of the shape a notice page or a static site would use.
    assert!(hashed_asset_refs(r#"<img src="assets/logo.png">"#).is_empty());
    assert!(hashed_asset_refs(r#"<script src="assets/main.js">"#).is_empty());
    assert!(hashed_asset_refs(r#"<script src="assets/app-v2.js">"#).is_empty());
    assert!(hashed_asset_refs("no assets at all").is_empty());
}

#[tokio::test]
async fn index_references_a_real_hashed_asset_that_is_actually_served() {
    // The one assertion a placeholder cannot satisfy: the page must reference a
    // content-hashed bundle, and that bundle must actually be embedded. A doctype
    // and a root element prove nothing (see the test above).
    require_real_ui_build!("index_references_a_real_hashed_asset_that_is_actually_served");

    let (_tmp, app) = test_router();
    let resp = get(app.clone(), "/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    // No Accept-Encoding was sent, so the handler inflates the build-time gzip for
    // us and this is the real HTML either way.
    let html = String::from_utf8_lossy(&bytes).to_string();

    let refs = hashed_asset_refs(&html);
    assert!(
        !refs.is_empty(),
        "the embedded index.html references no content-hashed bundle, so it is not a \
         real frontend build. Run `npm run build` in crates/dux-web/web and rebuild. \
         Served page:\n{html}"
    );

    // Every reference the page makes must resolve, not just the first: a page that
    // names three chunks and can only serve one is a broken build too.
    for asset in &refs {
        let resp = get(app.clone(), &format!("/{asset}")).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the index references /{asset} but the binary does not serve it"
        );
        let ctype = header(&resp, "content-type").unwrap_or_default();
        assert!(
            !ctype.contains("html"),
            "/{asset} was served as {ctype}; the SPA fallback answered instead of the asset"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() > 64,
            "/{asset} is only {} bytes, which is not a real bundle",
            body.len()
        );
    }
}

#[tokio::test]
async fn served_index_is_not_the_ui_not_built_notice() {
    // The inverse framing, so a regression that reintroduces a placeholder-on-
    // failure path fails here even if it changes the page's wording enough to
    // dodge the hashed-asset check.
    require_real_ui_build!("served_index_is_not_the_ui_not_built_notice");

    let (_tmp, app) = test_router();
    let resp = get(app, "/").await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    for marker in [
        "not built",
        "web assets not built",
        "dux_disable_ui_build",
        "npm run build",
    ] {
        assert!(
            !html.contains(marker),
            "the served page contains {marker:?}, so it is a not-built notice rather \
             than the web UI"
        );
    }
}

/// The composite "a real server on a real port serves the real page AND accepts a
/// websocket handshake", which is the in-repo mirror of the release workflow's
/// archive smoke test.
///
/// This is NOT a duplicate of the existing websocket coverage. The handshake alone
/// is well covered (`serve_with_engine.rs` and `first_load_routes.rs` both await a
/// `connected` frame on `/ws/events`, and `tab_routes.rs` opens per-PTY sockets),
/// and every other test in THIS file drives the router through `tower::oneshot`,
/// which never binds a socket. What nothing covered is the pair together against
/// one live listener: a build whose page is a placeholder, or a router where the
/// static fallback swallows `/ws/...`, is exactly the shape that survives when the
/// page and the socket are only ever checked apart. The release workflow checks the
/// same three things on the shipping archive; this test makes the PR fail first.
#[tokio::test]
async fn live_server_serves_the_built_page_and_accepts_a_websocket() {
    use std::net::SocketAddr;

    use futures_util::StreamExt;

    require_real_ui_build!("live_server_serves_the_built_page_and_accepts_a_websocket");

    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    // 1. The page loads over real HTTP.
    let resp = reqwest::get(format!("http://{addr}/"))
        .await
        .expect("GET /");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.expect("index body");

    // 2. It references a content-hashed bundle, and that bundle really downloads.
    let refs = hashed_asset_refs(&html);
    let asset = refs.first().unwrap_or_else(|| {
        panic!("the live server served a page with no hashed bundle reference:\n{html}")
    });
    let asset_resp = reqwest::get(format!("http://{addr}/{asset}"))
        .await
        .expect("GET asset");
    assert_eq!(
        asset_resp.status(),
        reqwest::StatusCode::OK,
        "the live server does not serve /{asset}, which its own index references"
    );
    assert!(
        asset_resp.bytes().await.expect("asset body").len() > 64,
        "/{asset} came back too small to be a real bundle"
    );

    // 3. The websocket handshake completes on the same server and the engine
    // answers, so the static fallback has not swallowed the socket route.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .expect("websocket handshake on /ws/events");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut saw_connected = false;
    while !saw_connected && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(message))) =
            tokio::time::timeout(std::time::Duration::from_millis(300), ws.next()).await
            && let Ok(text) = message.into_text()
            && text.contains("\"event\":\"connected\"")
        {
            saw_connected = true;
        }
    }
    assert!(
        saw_connected,
        "the websocket connected but never delivered the `connected` frame"
    );
}
