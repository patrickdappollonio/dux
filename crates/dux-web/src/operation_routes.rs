//! `GET /api/v1/operations/{id}`: how a change a client asked for really
//! ended. Every change route answers `?operation=1` with the id of a record in
//! the engine's registry (see [`dux_core::operations`]); this route reads it.
//!
//! `?wait_seconds=N` holds the reply until the record has an outcome (a record
//! past its unknown threshold has none yet) or `N` seconds pass, capped at
//! [`MAX_WAIT`], then answers the record as it stands. An id the registry does
//! not know (never opened, or past its retention) is `404 {"error":"unknown_operation"}`.

use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use crate::rest_common::id_within_bound;
use crate::server::AppState;

/// The longest a single read waits for an outcome.
pub const MAX_WAIT: Duration = Duration::from_secs(25);

/// How often a waiting read looks at the record again.
const POLL: Duration = Duration::from_millis(100);

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/operations/{id}", get(get_operation))
}

#[derive(Deserialize)]
struct WaitQuery {
    #[serde(default)]
    wait_seconds: u64,
}

async fn get_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_operation();
    }
    let deadline = wait_deadline(Instant::now(), query.wait_seconds);
    // While this core hands the engine over, answer the record as it stands rather
    // than hold the reply into a teardown; the client polls the next core by id.
    let mut handing_over = state.hand_over.subscribe();
    loop {
        let Some(view) = state.engine.operations().view(&id, Instant::now()) else {
            return unknown_operation();
        };
        if view.state.is_final() || Instant::now() >= deadline || *handing_over.borrow_and_update()
        {
            return Json(view).into_response();
        }
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            _ = handing_over.changed() => {}
        }
    }
}

/// When a read asked at `now` to wait `wait_seconds` answers at the latest.
fn wait_deadline(now: Instant, wait_seconds: u64) -> Instant {
    now + Duration::from_secs(wait_seconds).min(MAX_WAIT)
}

fn unknown_operation() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "unknown_operation" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use dux_core::operations::{OperationKind, OperationPolicy};
    use dux_core::statusline::StatusTone;
    use tower::ServiceExt;

    use super::*;

    const POLICY: OperationPolicy = OperationPolicy {
        unknown_after: Duration::from_secs(60),
        retention: Duration::from_secs(60),
    };

    async fn get(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn a_wait_asked_past_the_cap_is_cut_to_twenty_five_seconds() {
        let now = Instant::now();
        assert_eq!(wait_deadline(now, 1000), now + Duration::from_secs(25));
        assert_eq!(wait_deadline(now, 3), now + Duration::from_secs(3));
    }

    #[tokio::test]
    async fn an_id_nobody_opened_is_unknown() {
        let (_tmp, app) = crate::test_support::router_no_auth();
        let (status, body) = get(app, "/api/v1/operations/op-404?wait_seconds=5").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, serde_json::json!({ "error": "unknown_operation" }));
    }

    #[tokio::test]
    async fn a_wait_answers_when_the_record_ends_and_a_short_wait_answers_it_running() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let handle = crate::test_support::test_engine_handle(tmp.path());
        let app = crate::server::router(handle.clone());
        let operations = handle.operations().clone();
        operations.open("op-w", OperationKind::TabClose, POLICY, Instant::now());

        let (status, body) = get(app.clone(), "/api/v1/operations/op-w").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "running", "no wait answers at once");

        let finisher = operations.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            finisher.finish("op-w", StatusTone::Info, "Closed.", None, Instant::now());
        });
        let started = Instant::now();
        let (status, body) = get(app, "/api/v1/operations/op-w?wait_seconds=20").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "succeeded");
        assert_eq!(body["message"], "Closed.");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the wait ends with the record, not at its limit: {:?}",
            started.elapsed()
        );
    }
}
