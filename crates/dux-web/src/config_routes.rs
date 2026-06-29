//! REST write verbs for the config-mutating operations the palette / dialogs
//! trigger (Phase 6 of the REST-first migration). These used to ride the retired
//! `/ws` `command` channel (`update_macros`, `persist_global_env`,
//! `set_changes_pane_visible`, `reload_config`); they are now scoped REST verbs,
//! each dispatching the matching [`WireCommand`] via
//! [`EngineHandle::apply_wire_scoped`] with a per-connection [`StatusScope`]
//! derived from the optional `X-Connection-Id` header (the Phase 4 pattern).
//!
//! Routes (all gated):
//! - `PUT  /api/v1/macros`           — replace the macro set wholesale.
//! - `PUT  /api/v1/global-env`       — replace the workspace-wide env map.
//! - `PUT  /api/v1/ui/changes-pane`  — set the Changes-pane visibility flag.
//! - `POST /api/v1/config/reload`    — re-read `config.toml` from disk.
//!
//! On a successful config change the engine emits a `config.changed` event (via
//! the Phase 2 forwarder in `server.rs`), so subscribed clients refetch
//! `/api/v1/bootstrap` — these handlers do not echo the new state in their reply.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
};
use serde::Deserialize;

use dux_core::wire::{WireCommand, WireMacroEntry};

use crate::rest_common::scope_from_headers;
use crate::server::AppState;

/// The gated config-mutation routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/macros", put(update_macros))
        .route("/api/v1/global-env", put(persist_global_env))
        .route("/api/v1/ui/changes-pane", put(set_changes_pane))
        .route("/api/v1/config/reload", post(reload_config))
}

// ── Macros ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateMacrosBody {
    /// The whole macro set, in order. `WireMacroEntry` is `{name, text, surface}`,
    /// matching the frontend's `MacroView`. The engine validates wholesale
    /// (empty/duplicate names, empty text, unknown surface all rejected).
    entries: Vec<WireMacroEntry>,
}

async fn update_macros(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateMacrosBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::UpdateMacros {
            entries: body.entries,
        },
    )
    .await
}

// ── Global env ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GlobalEnvBody {
    /// The whole workspace-wide env map (replace-wholesale).
    env: BTreeMap<String, String>,
}

async fn persist_global_env(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GlobalEnvBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::PersistGlobalEnv { env: body.env },
    )
    .await
}

// ── Changes pane ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChangesPaneBody {
    visible: bool,
}

async fn set_changes_pane(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangesPaneBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::SetChangesPaneVisible {
            visible: body.visible,
        },
    )
    .await
}

// ── Reload ─────────────────────────────────────────────────────────────────────

/// `POST /api/v1/config/reload`. No body is required (the frontend sends `{}`),
/// so no `Json` extractor — a config reload re-reads `config.toml` from disk.
async fn reload_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::ReloadConfig {}).await
}

// ── Shared dispatch ─────────────────────────────────────────────────────────────

/// Dispatch a config-mutating wire command, scoping its status toasts to the
/// originating connection. `200 OK` on success; `400` with the engine's
/// user-facing validation message otherwise.
async fn dispatch(state: &AppState, headers: &HeaderMap, cmd: WireCommand) -> Response {
    match state
        .engine
        .apply_wire_scoped(cmd, scope_from_headers(headers, &state.connections))
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::test_support::{router_no_auth, router_with_auth};

    fn json_req(method: &str, uri: &str, body: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn update_macros_accepts_a_valid_set() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/macros",
                r#"{"entries":[{"name":"greet","text":"hi","surface":"agent"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_macros_rejects_an_empty_name_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/macros",
                r#"{"entries":[{"name":"","text":"hi","surface":"agent"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn persist_global_env_accepts_a_map() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/global-env",
                r#"{"env":{"FOO":"bar"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn set_changes_pane_accepts_a_flag() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/ui/changes-pane",
                r#"{"visible":false}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reload_config_accepts_an_empty_body() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req("POST", "/api/v1/config/reload", "{}"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn config_mutations_are_gated() {
        let cases: [(&str, &str, &str); 4] = [
            ("PUT", "/api/v1/macros", r#"{"entries":[]}"#),
            ("PUT", "/api/v1/global-env", r#"{"env":{}}"#),
            ("PUT", "/api/v1/ui/changes-pane", r#"{"visible":true}"#),
            ("POST", "/api/v1/config/reload", "{}"),
        ];
        for (method, uri, body) in cases {
            let (_tmp, app) = router_with_auth();
            let resp = app.oneshot(json_req(method, uri, body)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must be gated"
            );
        }
    }
}
