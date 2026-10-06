//! What the command line asks about the server itself: who is connected
//! (`GET /api/v1/server/connections`) and what its log says
//! (`GET /api/v1/server/log`).
//!
//! The log route answers `{"lines":[…],"cursor":N}` for the last `lines` lines
//! of `server.log`, or for the lines after `since`, a count of the file's
//! complete lines as a previous answer's `cursor` gave it. With `follow=true`
//! it instead streams the same starting lines as plain text and then every
//! line written after, as a chunked reply with no end of its own. The stream
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
use dux_core::file_follow::{FOLLOW_INTERVAL, FileFollower, open_log};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::auth::SocketAuth;
use crate::server::AppState;

/// How many of the last lines a log read starts from when it does not say.
const DEFAULT_LINES: usize = 100;

/// The log streams open right now.
static OPEN_FOLLOWS: AtomicUsize = AtomicUsize::new(0);

/// How many log streams are open now.
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
    since: Option<u64>,
    lines: Option<usize>,
    #[serde(default)]
    follow: bool,
}

async fn get_log(
    State(state): State<AppState>,
    auth: SocketAuth,
    Query(query): Query<LogQuery>,
) -> Response {
    let path = state.engine.server_log_path().to_path_buf();
    let tail = query.lines.unwrap_or(DEFAULT_LINES);
    let since = query.since;
    let read = tokio::task::spawn_blocking(move || open_log(&path, since, tail)).await;
    let Ok((lines, cursor, follower)) = read else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the log could not be read",
        )
            .into_response();
    };
    if !query.follow {
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({ "lines": lines, "cursor": cursor })),
        )
            .into_response();
    }
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    tokio::spawn(follow(lines, follower, auth, tx, FollowCount::new()));
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

/// Feed `tx` the first lines, then every line `follower` finds, until the
/// client goes (`tx` closes) or the session this stream was opened under ends.
/// Ending drops `tx`, which ends the reply.
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

    #[tokio::test]
    async fn the_log_route_answers_the_last_lines_or_those_after_a_cursor() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let app = crate::server::router(crate::test_support::test_engine_handle(tmp.path()));

        let (status, body) = get_json(app.clone(), "/api/v1/server/log").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "lines": [], "cursor": 0 }));

        std::fs::write(tmp.path().join("server.log"), "one\ntwo\nthree\nfou").unwrap();
        let (_, body) = get_json(app.clone(), "/api/v1/server/log?lines=2").await;
        assert_eq!(
            body,
            serde_json::json!({ "lines": ["two", "three"], "cursor": 3 })
        );
        let (_, body) = get_json(app.clone(), "/api/v1/server/log?since=1&lines=1").await;
        assert_eq!(
            body,
            serde_json::json!({ "lines": ["two", "three"], "cursor": 3 })
        );
        let (_, body) = get_json(app, "/api/v1/server/log?since=3").await;
        assert_eq!(body, serde_json::json!({ "lines": [], "cursor": 3 }));
    }

    #[tokio::test]
    async fn a_followed_log_streams_whole_lines_as_they_are_written_and_ends_with_the_client() {
        let _turn = TURNS.lock().await;
        let before = open_follows();
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let log = tmp.path().join("server.log");
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
        let app = crate::server::router(crate::test_support::test_engine_handle(tmp.path()));

        let resp = app
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
    }

    #[tokio::test]
    async fn a_followed_log_ends_when_its_client_hangs_up_on_a_quiet_log() {
        let _turn = TURNS.lock().await;
        let before = open_follows();
        let (tmp, addr) = crate::test_support::boot_plain_test_server().await;
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
