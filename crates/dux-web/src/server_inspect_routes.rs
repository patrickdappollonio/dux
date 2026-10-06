//! What the command line asks about the server itself: who is connected
//! (`GET /api/v1/server/connections`) and what its log says
//! (`GET /api/v1/server/log`).
//!
//! The log route answers `{"lines":[…],"cursor":N}` for the last `lines` lines
//! of `server.log`, or for the lines after `since`, a count of the file's
//! complete lines as a previous answer's `cursor` gave it. With `follow=true`
//! it instead streams the same starting lines as plain text and then every
//! line written after, as a chunked reply with no end of its own; it starts from
//! at most as many lines as one answer holds. The stream
//! stops when the client goes away and when the session it was opened under
//! ends (a sign-out, a password change), through the same watch every
//! WebSocket holds.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dux_core::file_follow::{FOLLOW_INTERVAL, FileFollower, LogError, MAX_LINES, open_log};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::auth::SocketAuth;
use crate::server::AppState;

/// How many of the last lines a log read starts from when it does not say.
const DEFAULT_LINES: usize = 100;

/// The log streams open right now.
static OPEN_FOLLOWS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
fn open_follows() -> usize {
    OPEN_FOLLOWS.load(Ordering::SeqCst)
}

/// Counts one open stream for as long as it lives.
struct FollowCount;

impl FollowCount {
    fn new() -> Self {
        OPEN_FOLLOWS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for FollowCount {
    fn drop(&mut self) {
        OPEN_FOLLOWS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/server/connections", get(get_connections))
        .route("/api/v1/server/log", get(get_log))
}

async fn get_connections(State(state): State<AppState>) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.engine.attachments().connections(Instant::now())),
    )
        .into_response()
}

#[derive(Deserialize)]
struct LogQuery {
    since: Option<String>,
    lines: Option<usize>,
    #[serde(default)]
    follow: bool,
}

async fn get_log(
    State(state): State<AppState>,
    auth: SocketAuth,
    Query(query): Query<LogQuery>,
) -> Response {
    // The file this serve's own logger opened, never one named by the config
    // again: a setting changed since, or a link put there, cannot move it.
    let Some(path) = state.console.server_log_path() else {
        return (
            StatusCode::NOT_FOUND,
            dux_core::client::server_inspect::NO_SERVER_LOG,
        )
            .into_response();
    };
    let tail = query.lines.unwrap_or(DEFAULT_LINES);
    // A followed read starts from at most as many lines as one answer holds.
    let since = query.since;
    let read =
        tokio::task::spawn_blocking(move || open_log(&path, since.as_deref(), tail, MAX_LINES))
            .await;
    let (read, follower) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(error @ LogError::BadCursor)) => {
            return (StatusCode::BAD_REQUEST, error.to_string()).into_response();
        }
        Ok(Err(error)) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the log could not be read",
            )
                .into_response();
        }
    };
    if !query.follow {
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({
                "lines": read.lines,
                "cursor": read.cursor,
                "note": read.note,
            })),
        )
            .into_response();
    }
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    tokio::spawn(follow(read.lines, follower, auth, tx, FollowCount::new()));
    let body = Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|bytes| (Ok::<_, Infallible>(bytes), rx))
    }));
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            // A reverse proxy must pass each line on as it comes.
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}

/// `lines` as the body text a client reads them in: each ends its line.
fn text_of(lines: &[String]) -> Bytes {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    Bytes::from(text)
}

/// Stream the first lines, then each new one, until the client goes or the session the
/// stream was opened under ends; returning drops the sender, which ends the reply.
async fn follow(
    first: Vec<String>,
    mut follower: FileFollower,
    mut auth: SocketAuth,
    tx: mpsc::Sender<Bytes>,
    _count: FollowCount,
) {
    let mut sleep = tokio::time::interval(FOLLOW_INTERVAL);
    sleep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next = first;
    loop {
        if !next.is_empty() {
            let sent = tokio::select! {
                biased;
                _ = auth.revoked() => return,
                sent = tx.send(text_of(&next)) => sent,
            };
            if sent.is_err() {
                return;
            }
        }
        tokio::select! {
            biased;
            _ = auth.revoked() => return,
            _ = tx.closed() => return,
            _ = sleep.tick() => {}
        }
        let Ok((lines, back)) = tokio::task::spawn_blocking(move || {
            let lines = follower.poll();
            (lines, follower)
        })
        .await
        else {
            return;
        };
        follower = back;
        // A link put where the log was ends the stream after what was read
        // before it, with nothing of what it points at.
        if follower.refused() {
            if !lines.is_empty() {
                let _ = tx.send(text_of(&lines)).await;
            }
            return;
        }
        next = lines;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::Request;
    use dux_core::attachments::{ConnectionFacts, Surface, Target, TargetKind};
    use futures_util::StreamExt;
    use tower::ServiceExt;

    use super::*;

    /// Tests that count open streams take turns.
    static TURNS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn next_text(
        body: &mut (impl futures_util::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
    ) -> String {
        let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .expect("a chunk within five seconds")
            .expect("the stream is still open")
            .unwrap();
        String::from_utf8(chunk.to_vec()).unwrap()
    }

    async fn gauge_settles_at(want: usize) -> bool {
        for _ in 0..100 {
            if open_follows() == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn the_connections_route_lists_what_the_registry_holds() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let handle = crate::test_support::test_engine_handle(tmp.path());
        let attachments = handle.attachments().clone();
        attachments.register(
            "e1",
            ConnectionFacts {
                surface: Surface::Browser,
                device: Some("Firefox".to_string()),
                address: Some("198.51.100.7".parse().unwrap()),
                verified: false,
                events: true,
            },
            None,
        );
        attachments
            .attach(
                "e1",
                Target {
                    kind: TargetKind::Tab,
                    id: "s1-tab".to_string(),
                    agent: Some("s1".to_string()),
                },
                None,
                None,
            )
            .unwrap();
        let app = crate::server::router(handle);

        let (status, body) = get_json(app, "/api/v1/server/connections").await;
        assert_eq!(status, StatusCode::OK);
        let listed = body.as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], "e1");
        assert_eq!(listed[0]["surface"], "browser");
        assert_eq!(listed[0]["address"], "198.51.100.7");
        assert_eq!(listed[0]["verified"], false);
        assert_eq!(listed[0]["attachments"][0]["kind"], "tab");
        assert_eq!(listed[0]["attachments"][0]["id"], "s1-tab");
        assert_eq!(listed[0]["attachments"][0]["agent"], "s1");
    }

    /// A router whose console writes the server log under `tmp`, as every way
    /// of serving does, and the engine behind it.
    fn app_logging(tmp: &std::path::Path) -> axum::Router {
        let handle = crate::test_support::test_engine_handle(tmp);
        let log = dux_core::logger::open_server_log(
            &dux_core::config::ServerConfig::default(),
            &handle.paths(),
        )
        .unwrap();
        crate::server::build_app(
            handle,
            axum::Router::new(),
            crate::server::RouterParams::plain_http().with_console(
                crate::console::Console::server_log_only(std::sync::Arc::new(log)),
                false,
            ),
        )
    }

    async fn get_text(app: axum::Router, uri: &str) -> (StatusCode, String) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn the_log_route_answers_the_last_lines_or_those_after_a_cursor() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let app = app_logging(tmp.path());

        let (status, body) = get_json(app.clone(), "/api/v1/server/log").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["lines"], serde_json::json!([]));
        assert_eq!(body["note"], serde_json::Value::Null);

        std::fs::write(tmp.path().join("server.log"), "one\ntwo\nthree\nfou").unwrap();
        let (_, body) = get_json(app.clone(), "/api/v1/server/log?lines=2").await;
        assert_eq!(body["lines"], serde_json::json!(["two", "three"]));
        let cursor = body["cursor"].as_str().unwrap().to_string();
        assert!(cursor.ends_with(":3"), "{cursor}");
        let (generation, _) = cursor.split_once(':').unwrap();
        let (_, body) = get_json(
            app.clone(),
            &format!("/api/v1/server/log?since={generation}:1&lines=1"),
        )
        .await;
        assert_eq!(body["lines"], serde_json::json!(["two", "three"]));
        let (_, body) = get_json(app.clone(), &format!("/api/v1/server/log?since={cursor}")).await;
        assert_eq!(body["lines"], serde_json::json!([]));
        let (status, sentence) = get_text(app.clone(), "/api/v1/server/log?since=3").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(sentence.contains("<generation>:<line>"), "{sentence}");

        // One answer is one bounded document, and says when it was cut.
        let many: String = (0..10_001).map(|n| format!("line {n}\n")).collect();
        std::fs::write(tmp.path().join("server.log"), many).unwrap();
        let (_, body) = get_json(app, "/api/v1/server/log?lines=20000").await;
        let lines = body["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 10_000);
        assert_eq!(lines[9_999], "line 10000");
        assert!(body["note"].as_str().unwrap().contains("last 10000 lines"));
    }

    #[tokio::test]
    async fn the_log_route_reads_the_file_the_serve_opened_and_never_a_link_put_there() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        // A dux with no server log of its own says so.
        let quiet = dux_core::test_scratch::ScratchDir::new();
        let silent = crate::server::router(crate::test_support::test_engine_handle(quiet.path()));
        let (status, sentence) = get_text(silent, "/api/v1/server/log").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(sentence.contains("not writing a server log"), "{sentence}");

        let app = app_logging(tmp.path());
        let log = tmp.path().join("server.log");
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, "secret line\n").unwrap();
        std::fs::remove_file(&log).unwrap();
        std::os::unix::fs::symlink(&secret, &log).unwrap();
        for uri in [
            "/api/v1/server/log",
            "/api/v1/server/log?follow=true",
            "/api/v1/server/log?since=1.0:0",
        ] {
            let (status, sentence) = get_text(app.clone(), uri).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
            assert!(sentence.contains("symbolic link"), "{uri}: {sentence}");
            assert!(!sentence.contains("secret line"), "{uri}");
        }
    }

    #[tokio::test]
    async fn a_followed_log_streams_whole_lines_as_they_are_written_and_ends_with_the_client() {
        let _turn = TURNS.lock().await;
        let before = open_follows();
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let log = tmp.path().join("server.log");
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
        let app = app_logging(tmp.path());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/server/log?follow=true&lines=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        let mut body = resp.into_body().into_data_stream();
        assert_eq!(next_text(&mut body).await, "two\nthree\n");

        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, b"four\npar").unwrap();
        assert_eq!(next_text(&mut body).await, "four\n");
        std::io::Write::write_all(&mut file, b"tial\n").unwrap();
        assert_eq!(next_text(&mut body).await, "partial\n");

        assert_eq!(open_follows(), before + 1);
        drop(body);
        assert!(
            gauge_settles_at(before).await,
            "the stream ends once its client has gone"
        );

        // A follow starts from at most as many lines as one answer holds.
        let many: String = (0..10_001).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&log, many).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/server/log?follow=true&lines=20000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut body = resp.into_body().into_data_stream();
        let first = next_text(&mut body).await;
        assert_eq!(first.lines().count(), 10_000);
        assert_eq!(first.lines().next(), Some("line 1"));
        drop(body);
        assert!(gauge_settles_at(before).await);
    }

    #[tokio::test]
    async fn a_followed_log_ends_when_its_client_hangs_up_on_a_quiet_log() {
        let _turn = TURNS.lock().await;
        let before = open_follows();
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let app = app_logging(tmp.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });
        std::fs::write(tmp.path().join("server.log"), "hello\n").unwrap();

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /api/v1/server/log?follow=true&lines=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        while !String::from_utf8_lossy(&seen).contains("hello") {
            let read = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buffer),
            )
            .await
            .expect("the first line within five seconds")
            .unwrap();
            assert!(read > 0, "{}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buffer[..read]);
        }
        assert_eq!(open_follows(), before + 1);

        // Nothing is written to the log from here on: only the closed
        // connection can end the stream.
        drop(stream);
        assert!(
            gauge_settles_at(before).await,
            "a stream whose client closed the connection must end with no line to write"
        );
    }
}
