//! Release-notes fetching, as user journeys against a REAL HTTP server.
//!
//! Nothing here is mocked and nothing here touches the network: each test binds
//! a throwaway server on `127.0.0.1:0`, serves the captured v0.6.0 release body
//! (or a deliberate failure), and points the fetcher at it through the
//! injectable API base URL.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use dux_core::first_load::{self, FirstLoad, NotesOutcome};
use dux_core::release_notes::{self, CacheLookup, ReleaseNotes};
use dux_core::storage::SessionStore;

const SAMPLE: &str = include_str!("fixtures/sample_release_notes.md");

/// The request line plus headers of one served request, so a test can assert on
/// what dux actually sent.
#[derive(Debug)]
struct Recorded {
    request_line: String,
    headers: Vec<(String, String)>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A one-shot local HTTP server. It accepts a single connection, records the
/// request, and writes back a canned response.
struct TestServer {
    base_url: String,
    handle: Option<JoinHandle<Option<Recorded>>>,
    seen: mpsc::Receiver<Recorded>,
}

impl TestServer {
    fn start(status_line: &'static str, content_type: &'static str, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let (tx, seen) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().ok()?;
            let recorded = read_request(&mut stream)?;
            let response = format!(
                "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = tx.send(recorded);
            None
        });
        Self {
            base_url: format!("http://{addr}"),
            handle: Some(handle),
            seen,
        }
    }

    /// Serves the real captured release body as GitHub would return it.
    fn serving_sample_release(tag: &str, html_url: &str) -> Self {
        let json = serde_json::json!({
            "tag_name": tag,
            "name": tag,
            "body": SAMPLE,
            "html_url": html_url,
            // A field dux does not read, proving unknown fields are tolerated
            // (GitHub's payload is far larger than the three fields we want).
            "author": { "login": "patrickdappollonio" },
        });
        Self::start("HTTP/1.1 200 OK", "application/json", json.to_string())
    }

    /// The request dux made, or `None` if it never made one.
    fn request(&self) -> Option<Recorded> {
        self.seen.recv_timeout(Duration::from_millis(200)).ok()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            // The accept loop ends after one connection; if nothing ever
            // connected the thread is still blocked, so do not join on it.
            if self.seen.try_recv().is_ok() || h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Some(Recorded {
        request_line: request_line.trim_end().to_string(),
        headers,
    })
}

/// An address nothing can be listening on. Any connection attempt is refused
/// immediately, which is the fail-fast property
/// `a_refused_connection_is_an_error_with_no_hang` depends on.
///
/// This used to bind `127.0.0.1:0`, drop the listener, and hand back the
/// now-free ephemeral port, and that RACED with the other tests in this file.
/// Freeing a port makes it available again, the kernel hands out ephemeral ports
/// from a small range and reuses recently freed ones readily (12% of 500 fresh
/// binds landed on a just-released port when this was measured), and `TestServer`
/// is one-shot: its thread does exactly one `accept()` and exits. So a test using
/// the dead URL could connect to a CONCURRENT test's server that had just been
/// handed the same port and eat its single `accept()`. That server's own fetch
/// was then refused, a refusal is a TRANSIENT error, and transient plus a stale
/// cache returns the stale entry, so
/// `a_definitive_404_does_not_fall_back_to_a_stale_entry` failed on its
/// `expect_err`.
///
/// Port 1 cannot be caught up in that. It is privileged, so nothing in the test
/// suite can bind it, and it sits far below the ephemeral range the kernel
/// allocates from (`net.ipv4.ip_local_port_range` is 32768-60999 on the machine
/// this was measured on), so no `TestServer` can ever be handed it.
fn dead_base_url() -> String {
    "http://127.0.0.1:1".to_string()
}

fn cache_file(dir: &Path) -> std::path::PathBuf {
    release_notes::cache_path(dir)
}

// ---------------------------------------------------------------------------
// The journeys
// ---------------------------------------------------------------------------

#[test]
fn a_successful_fetch_returns_the_parsed_release_and_sends_the_headers_github_requires() {
    let server = TestServer::serving_sample_release(
        "v0.6.0",
        "https://github.com/patrickdappollonio/dux/releases/tag/v0.6.0",
    );

    let notes = release_notes::fetch_latest(&server.base_url).expect("fetch should succeed");

    assert_eq!(notes.version, "v0.6.0");
    assert_eq!(notes.headline, "Quieter plumbing, louder failures");
    assert_eq!(notes.paragraphs.len(), 2);
    assert_eq!(notes.sections.len(), 6);
    assert_eq!(
        notes.html_url, "https://github.com/patrickdappollonio/dux/releases/tag/v0.6.0",
        "the API's own html_url is preferred over a constructed one"
    );

    let req = server.request().expect("the server saw a request");
    assert_eq!(
        req.request_line, "GET /repos/patrickdappollonio/dux/releases/latest HTTP/1.1",
        "unauthenticated latest-release endpoint, one request"
    );
    // GitHub rejects requests with no User-Agent outright.
    let ua = req.header("user-agent").expect("a User-Agent is required");
    assert!(ua.contains("dux"), "the User-Agent must identify dux: {ua}");
    assert_eq!(
        req.header("accept"),
        Some("application/vnd.github+json"),
        "pin the API media type"
    );
    assert!(
        req.header("authorization").is_none(),
        "the fetch is unauthenticated by design"
    );
}

#[test]
fn a_release_with_no_html_url_falls_back_to_the_releases_index() {
    let json = serde_json::json!({ "tag_name": "v9.9.9", "body": SAMPLE });
    let server = TestServer::start("HTTP/1.1 200 OK", "application/json", json.to_string());

    let notes = release_notes::fetch_latest(&server.base_url).expect("fetch");
    assert_eq!(notes.html_url, dux_core::urls::RELEASES);
}

#[test]
fn the_startup_path_asks_for_the_running_versions_own_tag_never_the_latest_release() {
    // THE regression: a user who upgrades to v0.7.0 while v0.8.0 is already
    // published must be told about v0.7.0, the version they actually have.
    let server = TestServer::serving_sample_release(
        "v0.7.0",
        "https://github.com/patrickdappollonio/dux/releases/tag/v0.7.0",
    );

    let notes = release_notes::fetch_release_by_tag(&server.base_url, "v0.7.0").expect("fetch");
    assert_eq!(notes.version, "v0.7.0");
    assert_eq!(
        notes.html_url,
        "https://github.com/patrickdappollonio/dux/releases/tag/v0.7.0"
    );

    let req = server.request().expect("the server saw a request");
    assert_eq!(
        req.request_line, "GET /repos/patrickdappollonio/dux/releases/tags/v0.7.0 HTTP/1.1",
        "the by-tag endpoint, not /releases/latest"
    );
    assert!(
        !req.request_line.contains("/releases/latest"),
        "asking for /latest is the bug this test exists to catch"
    );
    let ua = req.header("user-agent").expect("a User-Agent is required");
    assert!(ua.contains("dux"), "{ua}");
    assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
    assert!(req.header("authorization").is_none());
}

#[test]
fn an_http_error_status_is_transient_not_definitive() {
    // 403 is what a rate-limited unauthenticated client gets (60/hour per IP).
    // It might work in an hour, so it must NOT settle the version.
    let server = TestServer::start(
        "HTTP/1.1 403 Forbidden",
        "application/json",
        r#"{"message":"API rate limit exceeded"}"#.to_string(),
    );

    let err =
        release_notes::fetch_release_by_tag(&server.base_url, "v0.6.0").expect_err("403 must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("403") || msg.to_lowercase().contains("status"),
        "the error should name the status: {msg}"
    );
    assert!(!err.is_definitive(), "a rate limit is retryable");
    assert_eq!(err.outcome(), NotesOutcome::TemporarilyUnavailable);
}

#[test]
fn a_5xx_is_transient_too_so_the_notes_get_another_chance() {
    let server = TestServer::start(
        "HTTP/1.1 502 Bad Gateway",
        "text/plain",
        "upstream sad".to_string(),
    );
    let err = release_notes::fetch_release_by_tag(&server.base_url, "v0.6.0").expect_err("502");
    assert!(!err.is_definitive());
    assert_eq!(err.outcome(), NotesOutcome::TemporarilyUnavailable);
}

#[test]
fn a_404_is_definitive_and_settles_the_version_instead_of_being_re_asked_forever() {
    // A locally built or not-yet-published tagged binary has no release page.
    let server = TestServer::start(
        "HTTP/1.1 404 Not Found",
        "application/json",
        r#"{"message":"Not Found"}"#.to_string(),
    );

    let err = release_notes::fetch_release_by_tag(&server.base_url, "v9.9.9")
        .expect_err("no such release");
    assert!(
        matches!(err, release_notes::FetchError::NoSuchRelease { ref tag } if tag == "v9.9.9"),
        "the error must name the tag: {err:?}"
    );
    assert!(err.is_definitive());
    assert_eq!(err.outcome(), NotesOutcome::NoSuchRelease);
    // The message must explain itself rather than showing a bare 404.
    assert!(err.to_string().contains("v9.9.9"), "{err}");

    // And the gate turns that into "show nothing, but stop asking".
    let plan = first_load::plan(Some("v0.6.0"), "v9.9.9", false, false);
    let plan = first_load::after_fetch(plan, err.outcome());
    assert_eq!(plan.screen, FirstLoad::Nothing);
    assert!(plan.mark_seen, "a definitive answer must not be re-asked");
}

#[test]
fn a_tag_that_could_rewrite_the_request_path_is_refused_without_any_request() {
    // Never interpolate an unvalidated tag into a URL path. Refused locally, so
    // the (one-shot) server below never sees a connection at all.
    let server = TestServer::serving_sample_release("x", "https://example.invalid/x");
    for tag in ["../../users/octocat", "v1.0?foo=bar", "release/1.0", ""] {
        let err = release_notes::fetch_release_by_tag(&server.base_url, tag)
            .expect_err("unsafe tag must be refused");
        assert!(err.is_definitive(), "{tag} should be definitive: {err}");
    }
    assert!(
        server.request().is_none(),
        "no request should have been made for an unsafe tag"
    );
}

#[test]
fn the_development_build_path_still_asks_for_the_newest_release() {
    // A dev build has no tag to look up, so `/releases/latest` is the right
    // question there — and the only place it is still asked.
    let server = TestServer::serving_sample_release("v0.9.0", "https://example.invalid/v0.9.0");
    let notes = release_notes::fetch_latest(&server.base_url).expect("fetch");
    assert_eq!(notes.version, "v0.9.0");
    assert_eq!(
        server.request().expect("a request").request_line,
        "GET /repos/patrickdappollonio/dux/releases/latest HTTP/1.1"
    );
}

#[test]
fn malformed_json_is_an_error_and_never_panics() {
    let server = TestServer::start(
        "HTTP/1.1 200 OK",
        "application/json",
        "this is not json at all {{{".to_string(),
    );
    let err = release_notes::fetch_latest(&server.base_url).expect_err("bad JSON must fail");
    assert!(
        err.to_string().to_lowercase().contains("json")
            || err.to_string().to_lowercase().contains("parse")
            || err.to_string().contains("release"),
        "unhelpful error: {err}"
    );
}

#[test]
fn json_missing_the_tag_name_is_an_error() {
    // A payload shaped like JSON but not like a release must not yield an empty
    // ReleaseNotes that the screen would render as a blank modal.
    let server = TestServer::start(
        "HTTP/1.1 200 OK",
        "application/json",
        r#"{"message":"Not Found","documentation_url":"https://docs.github.com"}"#.to_string(),
    );
    assert!(release_notes::fetch_latest(&server.base_url).is_err());
}

#[test]
fn a_refused_connection_is_an_error_with_no_hang() {
    let base = dead_base_url();
    let started = std::time::Instant::now();
    assert!(release_notes::fetch_latest(&base).is_err());
    assert!(
        started.elapsed() < release_notes::FETCH_TIMEOUT + Duration::from_secs(2),
        "a refused connection must fail fast, took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Caching
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_cache_hit_for_the_running_version_makes_no_request_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    let now = Utc::now();

    // Seed the cache as a previous launch would have.
    let seeded = ReleaseNotes {
        version: "v0.6.0".to_string(),
        headline: "From the cache".to_string(),
        paragraphs: vec!["cached prose".to_string()],
        sections: vec!["cached section".to_string()],
        html_url: "https://example.invalid/tag/v0.6.0".to_string(),
    };
    release_notes::write_cache(&cache, &seeded, now).unwrap();

    // A base URL with nothing listening: if the fetcher touched the network at
    // all this call would fail, so returning the notes PROVES the cache short
    // circuit.
    let got = release_notes::load_or_fetch_tag(
        &dead_base_url(),
        &cache,
        "v0.6.0",
        release_notes::CACHE_TTL,
        now,
    )
    .expect("the fresh cache entry is authoritative");
    assert_eq!(got, seeded);
}

#[test]
fn a_cache_entry_older_than_the_ttl_is_refetched_so_a_fixed_typo_reaches_the_user() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    let then = Utc::now() - ChronoDuration::hours(48);

    release_notes::write_cache(
        &cache,
        &ReleaseNotes {
            version: "v0.6.0".to_string(),
            headline: "Typo in teh headline".to_string(),
            ..Default::default()
        },
        then,
    )
    .unwrap();

    let server = TestServer::serving_sample_release("v0.6.0", "https://example.invalid/v0.6.0");
    let got = release_notes::load_or_fetch_tag(
        &server.base_url,
        &cache,
        "v0.6.0",
        release_notes::CACHE_TTL,
        Utc::now(),
    )
    .expect("stale entry refetched");

    assert_eq!(got.headline, "Quieter plumbing, louder failures");
    assert!(server.request().is_some(), "a request was actually made");

    // The refreshed copy replaced the stale one on disk.
    let now = Utc::now();
    match release_notes::cached_notes(&cache, "v0.6.0", release_notes::CACHE_TTL, now) {
        CacheLookup::Fresh(n) => assert_eq!(n.headline, "Quieter plumbing, louder failures"),
        other => panic!("expected a fresh entry after a refetch, got {other:?}"),
    }
}

#[test]
fn a_failed_refresh_falls_back_to_the_stale_cache_rather_than_showing_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    release_notes::write_cache(
        &cache,
        &ReleaseNotes {
            version: "v0.6.0".to_string(),
            headline: "Stale but true".to_string(),
            ..Default::default()
        },
        Utc::now() - ChronoDuration::hours(48),
    )
    .unwrap();

    let got = release_notes::load_or_fetch_tag(
        &dead_base_url(),
        &cache,
        "v0.6.0",
        release_notes::CACHE_TTL,
        Utc::now(),
    )
    .expect("a stale hit beats nothing when the network is down");
    assert_eq!(got.headline, "Stale but true");
}

#[test]
fn a_cache_entry_for_a_different_version_is_never_used() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    let now = Utc::now();
    release_notes::write_cache(
        &cache,
        &ReleaseNotes {
            version: "v0.5.0".to_string(),
            headline: "Last release".to_string(),
            ..Default::default()
        },
        now,
    )
    .unwrap();

    assert!(matches!(
        release_notes::cached_notes(&cache, "v0.6.0", release_notes::CACHE_TTL, now),
        CacheLookup::Missing
    ));
    // And with no network the mismatch is a hard failure, not the wrong notes.
    assert!(
        release_notes::load_or_fetch_tag(
            &dead_base_url(),
            &cache,
            "v0.6.0",
            release_notes::CACHE_TTL,
            now
        )
        .is_err()
    );
}

#[test]
fn what_is_cached_is_what_the_next_lookup_asks_for_so_the_ttl_actually_takes_effect() {
    // THE consequential bug: keying the cache on the running version while
    // fetching /releases/latest meant that whenever the newest published tag
    // differed, EVERY lookup missed and dux refetched on every single launch,
    // defeating the TTL and pressing on the rate limit.
    //
    // The server is one-shot: after it answers once its port is closed, so a
    // second call that tried to fetch again would FAIL. Succeeding proves the
    // write and the lookup agree on the key.
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    let now = Utc::now();
    let server = TestServer::serving_sample_release("v0.7.0", "https://example.invalid/v0.7.0");

    let first = release_notes::load_or_fetch_tag(
        &server.base_url,
        &cache,
        "v0.7.0",
        release_notes::CACHE_TTL,
        now,
    )
    .expect("first launch fetches");
    assert_eq!(first.version, "v0.7.0");
    assert!(
        server.request().is_some(),
        "the first launch made a request"
    );

    let second = release_notes::load_or_fetch_tag(
        &server.base_url,
        &cache,
        "v0.7.0",
        release_notes::CACHE_TTL,
        now + ChronoDuration::minutes(5),
    )
    .expect("the second launch is served from cache, making no request");
    assert_eq!(second, first);
}

#[test]
fn a_definitive_404_does_not_fall_back_to_a_stale_entry() {
    // A stale entry is a reasonable answer when the network is down, but a 404
    // means the release is GONE (unpublished, or the tag was deleted). Showing
    // the old copy would contradict GitHub.
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    release_notes::write_cache(
        &cache,
        &ReleaseNotes {
            version: "v0.6.0".to_string(),
            headline: "Was published once".to_string(),
            ..Default::default()
        },
        Utc::now() - ChronoDuration::hours(48),
    )
    .unwrap();

    let server = TestServer::start(
        "HTTP/1.1 404 Not Found",
        "application/json",
        r#"{"message":"Not Found"}"#.to_string(),
    );
    let err = release_notes::load_or_fetch_tag(
        &server.base_url,
        &cache,
        "v0.6.0",
        release_notes::CACHE_TTL,
        Utc::now(),
    )
    .expect_err("a 404 must not be papered over with a stale entry");
    assert!(err.is_definitive());
}

#[test]
fn a_missing_or_corrupt_cache_file_is_not_an_error_it_is_just_a_miss() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = cache_file(tmp.path());
    let now = Utc::now();
    assert!(matches!(
        release_notes::cached_notes(&cache, "v0.6.0", release_notes::CACHE_TTL, now),
        CacheLookup::Missing
    ));

    std::fs::write(&cache, b"\x00 not json").unwrap();
    assert!(matches!(
        release_notes::cached_notes(&cache, "v0.6.0", release_notes::CACHE_TTL, now),
        CacheLookup::Missing
    ));

    // A corrupt file must still be replaceable, not poison the cache forever.
    let notes = ReleaseNotes {
        version: "v0.6.0".to_string(),
        ..Default::default()
    };
    release_notes::write_cache(&cache, &notes, now).unwrap();
    assert!(matches!(
        release_notes::cached_notes(&cache, "v0.6.0", release_notes::CACHE_TTL, now),
        CacheLookup::Fresh(_)
    ));
}

// ---------------------------------------------------------------------------
// The gate, wired to a real database and a real (failing) fetch
// ---------------------------------------------------------------------------

#[test]
fn an_offline_launch_shows_nothing_and_leaves_the_stored_version_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionStore::open(&tmp.path().join("sessions.sqlite3")).unwrap();
    store.set_last_seen_version("v0.6.0").unwrap();

    // The gate wants the what's-new screen...
    let plan = first_load::plan(
        store.last_seen_version().unwrap().as_deref(),
        "v0.7.0",
        false,
        false,
    );
    assert_eq!(plan.screen, FirstLoad::WhatsNew);

    // ...but the fetch fails (no network).
    let fetched = release_notes::load_or_fetch_tag(
        &dead_base_url(),
        &cache_file(tmp.path()),
        "v0.7.0",
        release_notes::CACHE_TTL,
        Utc::now(),
    );
    assert!(fetched.is_err());
    assert_eq!(
        release_notes::outcome_of(&fetched),
        NotesOutcome::TemporarilyUnavailable,
        "a refused connection is retryable, not a definitive answer"
    );

    let plan = first_load::after_fetch(plan, release_notes::outcome_of(&fetched));
    assert_eq!(plan.screen, FirstLoad::Nothing, "nothing to show");
    assert!(!plan.mark_seen, "the version must NOT be marked seen");

    // The surface only writes when told to, so the stored version is unchanged
    // and v0.7.0's notes get another chance on the next launch.
    if plan.mark_seen {
        store.set_last_seen_version("v0.7.0").unwrap();
    }
    assert_eq!(
        store.last_seen_version().unwrap(),
        Some("v0.6.0".to_string()),
        "an offline launch must not consume the release notes"
    );
}

#[test]
fn an_online_launch_shows_the_notes_and_then_stores_the_version() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionStore::open(&tmp.path().join("sessions.sqlite3")).unwrap();
    store.set_last_seen_version("v0.6.0").unwrap();
    let server = TestServer::serving_sample_release("v0.7.0", "https://example.invalid/v0.7.0");

    let plan = first_load::plan(
        store.last_seen_version().unwrap().as_deref(),
        "v0.7.0",
        false,
        false,
    );
    let notes = release_notes::load_or_fetch_tag(
        &server.base_url,
        &cache_file(tmp.path()),
        "v0.7.0",
        release_notes::CACHE_TTL,
        Utc::now(),
    );
    let plan = first_load::after_fetch(plan, release_notes::outcome_of(&notes));
    assert_eq!(plan.screen, FirstLoad::WhatsNew);
    assert!(plan.mark_seen);
    // The notes describe the version the user is RUNNING, and link to its page.
    let notes = notes.expect("fetched");
    assert_eq!(notes.version, "v0.7.0");
    assert_eq!(release_notes::notes_url(Some(&notes)), notes.html_url);

    // Per the module contract, a shown screen stamps on DISMISSAL, not here.
    if plan.mark_seen {
        store.set_last_seen_version("v0.7.0").unwrap();
    }

    // Second launch of the same version: nothing, and no request needed.
    let plan = first_load::plan(
        store.last_seen_version().unwrap().as_deref(),
        "v0.7.0",
        false,
        false,
    );
    assert_eq!(plan.screen, FirstLoad::Nothing);
    assert!(!plan.mark_seen);
}

#[test]
fn a_wildly_oversized_body_is_refused_rather_than_read_into_memory() {
    // A hostile or broken endpoint must not be able to balloon dux's memory.
    let mut body = String::from("{\"tag_name\":\"v1\",\"body\":\"");
    body.push_str(&"A".repeat(release_notes::MAX_BODY_BYTES + 1024));
    body.push_str("\"}");
    let server = TestServer::start("HTTP/1.1 200 OK", "application/json", body);
    assert!(release_notes::fetch_latest(&server.base_url).is_err());
}

#[test]
fn the_served_fixture_is_the_real_captured_release_body() {
    // Guards the fixture itself: if it is ever replaced by a toy sample, the
    // parser assertions above stop meaning anything.
    let mut f = std::fs::File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_release_notes.md"),
    )
    .unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    assert_eq!(s, SAMPLE);
    assert!(s.contains("## What's Changed"), "the autogenerated section");
    assert!(s.contains("## Installation"), "the appended boilerplate");
    assert!(s.len() > 3_000, "the real body is substantial");
}
