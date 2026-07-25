//! The two first-load screens, as USER JOURNEYS against a real server on a real
//! port over real HTTP.
//!
//! Nothing is mocked. The store is a real SQLite file in a temp directory (dux
//! has no external service, so a temp dir IS the real dependency), and the
//! release-notes fetch points at a throwaway local HTTP server through the
//! injectable API base — no test ever contacts api.github.com.
//!
//! # What CANNOT be covered here, and why
//!
//! `dux_core::display_version()` is a COMPILE-TIME constant (`env!`), and a build
//! without `DUX_RELEASE_BUILD=1` reports `"development"`. A development build
//! deliberately never auto-shows the what's-new screen (`first_load::plan`), so
//! the automatic upgrade journey — "a launch whose stored version differs from a
//! real running version shows the notes" — is unreachable from a test binary. It
//! is covered by the exhaustive decision-table tests in
//! `dux_core::first_load` and by `dux-core/tests/release_notes_fetch.rs`.
//! What IS covered here is everything the web surface adds: the pending screen
//! surviving in server memory for a later client, the dismissal write, the
//! stamp-on-dismissal contract, and the on-demand release-notes read (which a dev
//! build DOES serve, from the newest release).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::{Duration, Instant};

use axum::Router;
use dux_core::config::DuxPaths;
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::{RouterParams, build_app};

const SAMPLE: &str = include_str!("../../dux-core/tests/fixtures/sample_release_notes.md");

// ── a local stand-in for the GitHub releases API ─────────────────────────────

/// A tiny always-on HTTP server that answers every request with the same canned
/// response. Multi-request (unlike the one-shot server in dux-core's suite)
/// because one journey can both resolve the startup gate and serve an on-demand
/// read.
struct FakeGithub {
    base_url: String,
}

impl FakeGithub {
    fn start(status_line: &'static str, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // Read just enough to let the client finish sending; the body is
                // canned, so the request content does not matter.
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            base_url: format!("http://{addr}"),
        }
    }

    /// Serves the captured real release body, shaped as GitHub returns it.
    fn serving_release(tag: &str) -> Self {
        let json = serde_json::json!({
            "tag_name": tag,
            "name": tag,
            "body": SAMPLE,
            "html_url": format!("https://github.com/patrickdappollonio/dux/releases/tag/{tag}"),
        });
        Self::start("HTTP/1.1 200 OK", json.to_string())
    }

    /// GitHub's definitive "no release for this tag".
    fn serving_no_release() -> Self {
        Self::start(
            "HTTP/1.1 404 Not Found",
            serde_json::json!({ "message": "Not Found" }).to_string(),
        )
    }
}

// ── harness ──────────────────────────────────────────────────────────────────

struct Server {
    addr: SocketAddr,
    paths: DuxPaths,
    _tmp: tempfile::TempDir,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn store(&self) -> SessionStore {
        SessionStore::open(&self.paths.sessions_db_path).expect("open store")
    }

    fn last_seen(&self) -> Option<String> {
        self.store().last_seen_version().expect("read last seen")
    }
}

/// Boot a real server. `config` is written to `config.toml` verbatim (empty means
/// no file at all, the true fresh-install shape) and `seed_last_seen` seeds the
/// SQLite row a previous launch would have left behind.
async fn boot(config: &str, seed_last_seen: Option<&str>, api_base: &str) -> Server {
    boot_with(config, seed_last_seen, api_base, |p| p).await
}

/// `boot`, plus a hook to tune the router params (the concurrency cap below).
async fn boot_with(
    config: &str,
    seed_last_seen: Option<&str>,
    api_base: &str,
    tune: impl FnOnce(RouterParams) -> RouterParams,
) -> Server {
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
    if !config.is_empty() {
        std::fs::write(&paths.config_path, config).unwrap();
    }
    if let Some(version) = seed_last_seen {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store.set_last_seen_version(version).unwrap();
    }

    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = build_app(
        handle,
        Router::new(),
        tune(RouterParams::plain_http().with_release_notes_api_base(api_base)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    Server {
        addr,
        paths,
        _tmp: tmp,
    }
}

/// A closed port: the fastest honest way to produce a TRANSIENT fetch failure.
const OFFLINE_BASE: &str = "http://127.0.0.1:1";

async fn get_json(url: &str) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = reqwest::get(url).await.expect("request");
    let status = resp.status();
    let body = resp.text().await.expect("body");
    let json = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn bootstrap(server: &Server) -> serde_json::Value {
    let (status, json) = get_json(&server.url("/api/v1/bootstrap")).await;
    assert_eq!(status, 200, "bootstrap must serve: {json}");
    json
}

/// Poll bootstrap until the gate has parked a screen, or fail with a clear
/// message. The resolver is a single actor round-trip, so this settles in
/// milliseconds; the deadline only stops a hang from becoming a silent pass.
async fn await_pending_screen(server: &Server) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let doc = bootstrap(server).await;
        if !doc["pending_first_load"].is_null() {
            return doc["pending_first_load"].clone();
        }
        if Instant::now() >= deadline {
            panic!("no first-load screen was ever offered: {doc}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll until the stored version matches `expected`, proving the resolver's
/// immediate stamp landed.
async fn await_last_seen(server: &Server, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if server.last_seen().as_deref() == Some(expected) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "the stored version never became {expected:?} (it is {:?})",
                server.last_seen()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── journey: a fresh install ─────────────────────────────────────────────────

/// A brand new install connects and is offered the welcome screen, with the copy
/// and the numbered steps ready to render — and the stored version is still
/// EMPTY, because a shown screen stamps on dismissal, not when the plan is made.
#[tokio::test]
async fn a_fresh_install_is_offered_the_welcome_screen_and_nothing_is_stamped_yet() {
    let server = boot("", None, OFFLINE_BASE).await;

    let pending = await_pending_screen(&server).await;
    assert_eq!(pending["screen"], "welcome");
    assert!(
        pending["notes"].is_null(),
        "the welcome screen carries no release notes: {pending}"
    );

    // The copy the dialog renders rides the bootstrap document unconditionally.
    let doc = bootstrap(&server).await;
    let welcome = &doc["welcome_screen"];
    assert!(
        welcome["tagline"].as_str().is_some_and(|t| !t.is_empty()),
        "the tagline must be projected: {welcome}"
    );
    let steps = welcome["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 3, "the agreed three numbered steps: {welcome}");
    assert_eq!(steps[0]["number"], 1);
    assert_eq!(steps[0]["title"], "Add a project");
    assert_eq!(steps[2]["title"], "Launch");
    // The last paragraph names THIS machine's config path.
    let prose = welcome["paragraphs"].to_string();
    assert!(
        prose.contains("config.toml"),
        "the prose must name the config file: {prose}"
    );
    assert_eq!(doc["website_url"], dux_core::urls::WEBSITE);

    // THE contract: not stamped until the user dismisses it. If this write moved
    // to startup, a browser opened a minute later would see nothing at all.
    assert_eq!(
        server.last_seen(),
        None,
        "a screen that is merely PENDING must not have stamped the version"
    );
}

/// The user closes the welcome screen. That stamps the running version, and a
/// second browser connecting afterwards is offered nothing — the dismissal is
/// shared through the one SQLite row, which is also the row the TUI reads.
#[tokio::test]
async fn dismissing_the_welcome_screen_stamps_the_version_and_the_next_client_sees_nothing() {
    let server = boot("", None, OFFLINE_BASE).await;
    await_pending_screen(&server).await;

    let resp = reqwest::Client::new()
        .post(server.url("/api/v1/first-load/dismiss"))
        .send()
        .await
        .expect("dismiss");
    assert_eq!(resp.status(), 200);

    // Written to SQLite, which is what makes the dismissal shared with the TUI.
    assert_eq!(
        server.last_seen().as_deref(),
        Some(dux_core::display_version()),
        "dismissal must record the RUNNING version"
    );

    // A second client (same launch) is offered nothing.
    let doc = bootstrap(&server).await;
    assert!(
        doc["pending_first_load"].is_null(),
        "a dismissed screen must not be re-offered: {doc}"
    );

    // And a fresh launch over the same store agrees.
    let engine = bootstrap_engine(&server.paths);
    assert!(
        engine.is_err(),
        "the single-instance lock is still held by the running server"
    );
}

// ── journey: nothing to show ─────────────────────────────────────────────────

/// A launch that has already seen this version is offered nothing, and writes
/// nothing.
#[tokio::test]
async fn a_launch_that_already_saw_this_version_is_offered_nothing() {
    let already = dux_core::display_version();
    let server = boot("", Some(already), OFFLINE_BASE).await;

    // Give the resolver a real chance to do the wrong thing before asserting it
    // did not: there is no positive signal to await on a no-op plan.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let doc = bootstrap(&server).await;
    assert!(
        doc["pending_first_load"].is_null(),
        "the same version must show neither screen: {doc}"
    );
    assert_eq!(server.last_seen().as_deref(), Some(already));
}

/// A launch that cannot produce notes must leave the stored version UNTOUCHED, so
/// the notes get another chance later. On a development build this is the
/// dev-build rule doing that work (`plan` returns no screen and no stamp), which
/// is the same property an offline launch relies on: an unshowable release must
/// never be silently consumed.
#[tokio::test]
async fn a_launch_that_cannot_show_notes_leaves_the_stored_version_untouched() {
    // A stored version that differs from the running one, and no reachable API.
    let server = boot("", Some("v0.6.0"), OFFLINE_BASE).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let doc = bootstrap(&server).await;
    assert!(
        doc["pending_first_load"].is_null(),
        "no notes means no screen: {doc}"
    );
    assert_eq!(
        server.last_seen().as_deref(),
        Some("v0.6.0"),
        "stamping here would hide v0.6.0's successor's notes forever"
    );
}

/// With the welcome suppressed by config there is no screen to dismiss, so the
/// version is stamped IMMEDIATELY — the other half of the timing contract. The
/// proof it matters: the user stays on a moving version rather than being pinned
/// at "never seen anything".
#[tokio::test]
async fn suppressing_the_welcome_stamps_the_version_immediately() {
    let server = boot(
        "[ui]\ndisable_automated_welcome_screen = true\n",
        None,
        OFFLINE_BASE,
    )
    .await;

    await_last_seen(&server, dux_core::display_version()).await;
    let doc = bootstrap(&server).await;
    assert!(
        doc["pending_first_load"].is_null(),
        "the suppressed welcome must not appear: {doc}"
    );
    // The flag is surfaced so the Preferences row can render its true state.
    assert_eq!(doc["disable_automated_welcome_screen"], true);
    assert_eq!(doc["disable_release_notes"], false);
}

// ── journey: opening the notes on demand ─────────────────────────────────────

/// The app menu's "What's new…" fetches the notes on demand and gets back plain
/// prose and feature titles — parsed by core, so the client needs no Markdown
/// renderer.
#[tokio::test]
async fn the_on_demand_read_returns_parsed_release_notes() {
    let github = FakeGithub::serving_release("v0.6.0");
    let server = boot("", None, &github.base_url).await;

    let (status, notes) = get_json(&server.url("/api/v1/release-notes")).await;
    assert_eq!(status, 200, "notes must be served: {notes}");
    assert_eq!(notes["version"], "v0.6.0");
    assert_eq!(notes["headline"], "Quieter plumbing, louder failures");
    let sections = notes["sections"].as_array().expect("sections");
    assert_eq!(sections.len(), 6, "the feature titles: {sections:?}");
    assert!(
        notes["html_url"]
            .as_str()
            .is_some_and(|u| u.contains("/releases/tag/v0.6.0")),
        "the notes must link to their own release page: {notes}"
    );
    // Parsing happened server-side: no Markdown heading markers survive.
    assert!(
        !notes["headline"].as_str().unwrap().contains('#'),
        "core hands over plain prose: {notes}"
    );
}

/// The suppression flags disable the AUTOMATIC screens only. With both set, the
/// on-demand read still works — otherwise a user who turned the startup screen
/// off could never look the notes up again.
#[tokio::test]
async fn the_on_demand_read_works_even_with_the_automatic_screens_disabled() {
    let github = FakeGithub::serving_release("v0.6.0");
    let server = boot(
        "[ui]\ndisable_automated_welcome_screen = true\ndisable_release_notes = true\n",
        Some("v0.5.0"),
        &github.base_url,
    )
    .await;

    let doc = bootstrap(&server).await;
    assert_eq!(doc["disable_release_notes"], true);

    let (status, notes) = get_json(&server.url("/api/v1/release-notes")).await;
    assert_eq!(
        status, 200,
        "the flag suppresses the automatic screen, not the menu entry: {notes}"
    );
    assert_eq!(notes["version"], "v0.6.0");
}

/// A definitive "no such release" is a 404 with an explanation, not a silent
/// empty screen. Retrying cannot change the answer, so the client must be able to
/// say so.
#[tokio::test]
async fn an_unpublished_release_answers_404_with_a_readable_reason() {
    let github = FakeGithub::serving_no_release();
    let server = boot("", None, &github.base_url).await;

    let resp = reqwest::get(server.url("/api/v1/release-notes"))
        .await
        .expect("request");
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.expect("body");
    assert!(
        body.to_lowercase().contains("no published release"),
        "the failure must explain itself: {body}"
    );
}

/// An unreachable GitHub is a 502 (retryable), distinct from the 404 above, and
/// it never silently succeeds with empty notes.
#[tokio::test]
async fn an_unreachable_github_answers_502_and_never_empty_notes() {
    let server = boot("", None, OFFLINE_BASE).await;

    let resp = reqwest::get(server.url("/api/v1/release-notes"))
        .await
        .expect("request");
    assert_eq!(
        resp.status(),
        502,
        "a transient failure must be distinguishable from a definitive one"
    );
    let body = resp.text().await.expect("body");
    assert!(!body.trim().is_empty(), "an error must carry a reason");
}

/// The on-demand read is bounded by `[server] release_notes_max_concurrency`, and
/// like the `/files/tree` cap it must WAIT for a permit rather than reject: at
/// capacity 1, two concurrently fired reads must BOTH succeed, serialized onto the
/// single permit. An unbounded handler would let a burst of clicks pile blocking
/// HTTPS fetches onto the server's blocking-thread pool.
#[tokio::test]
async fn the_on_demand_read_at_capacity_one_waits_instead_of_rejecting() {
    let github = FakeGithub::serving_release("v0.6.0");
    let server = boot_with("", None, &github.base_url, |p| {
        p.with_release_notes_max_concurrency(1)
    })
    .await;

    let url = server.url("/api/v1/release-notes");
    let (a, b) = tokio::join!(get_json(&url), get_json(&url));
    assert_eq!(a.0, 200, "the first read must succeed: {}", a.1);
    assert_eq!(
        b.0, 200,
        "the contended read must WAIT for the permit, never be refused: {}",
        b.1
    );
    assert_eq!(a.1["version"], "v0.6.0");
    assert_eq!(b.1["version"], "v0.6.0");
}

/// A dismissal in ONE browser tab must settle the screen in every other open tab.
/// `config.changed` is the only event that drives a client bootstrap refetch, so
/// without it a second tab keeps its dialog up indefinitely — which would break
/// the promise that dismissal is shared. Proven with a receiver that is already
/// subscribed to `/ws/events` BEFORE the dismissal is posted.
#[tokio::test]
async fn dismissing_tells_every_other_open_client_through_config_changed() {
    use futures_util::{SinkExt, StreamExt};

    let server = boot("", None, OFFLINE_BASE).await;
    await_pending_screen(&server).await;

    // The other tab, already listening on the coarse `config` topic — the topic
    // `config.changed` is delivered on, and the one the real client holds.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws/events", server.addr))
        .await
        .expect("subscribe to /ws/events");
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"subscribe":["config"]}"#.into(),
    ))
    .await
    .expect("send the subscribe frame");

    // Drain anything already in flight (the resolver's own emissions), so the
    // frame asserted on below can only be the dismissal's.
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(50), ws.next()).await;
    }

    let resp = reqwest::Client::new()
        .post(server.url("/api/v1/first-load/dismiss"))
        .send()
        .await
        .expect("dismiss");
    assert_eq!(resp.status(), 200);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw = false;
    while !saw && Instant::now() < deadline {
        if let Ok(Some(Ok(frame))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(text) = frame.into_text()
        {
            saw = text.contains("\"config.changed\"");
        }
    }
    assert!(
        saw,
        "a dismissal must emit config.changed so an already-open tab refetches \
         the bootstrap and closes its dialog"
    );
}
