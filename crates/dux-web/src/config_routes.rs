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
//! - `POST /api/v1/defaults/toggle-randomized-pet-name` — flip the random
//!   pet-name default.
//! - `POST /api/v1/ui/toggle-pr-banner-position` — swap the PR banner top/bottom.
//! - `POST /api/v1/ui/toggle-github-integration` — flip GitHub PR integration.
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
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};

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
        .route(
            "/api/v1/defaults/toggle-randomized-pet-name",
            post(toggle_randomized_pet_name_default),
        )
        .route(
            "/api/v1/ui/toggle-pr-banner-position",
            post(toggle_pr_banner_position),
        )
        .route(
            "/api/v1/ui/toggle-github-integration",
            post(toggle_github_integration),
        )
        .route(
            "/api/v1/config/raw",
            // A config.toml is a few KB; 256 KB is generous. The cap stops a
            // client from streaming a multi-MB body that the engine thread would
            // then parse and fsync.
            get(read_raw_config)
                .put(write_raw_config)
                .layer(axum::extract::DefaultBodyLimit::max(256 * 1024)),
        )
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

// ── Preference toggles ───────────────────────────────────────────────────────
//
// These mirror the TUI palette toggles. Each is a parameterless POST: the server
// owns the current value and flips it (so two surfaces never disagree about the
// "next" state), persists, and emits `config.changed` so every client refetches
// the bootstrap document. The frontend confirms via the routed status toast.

/// `POST /api/v1/defaults/toggle-randomized-pet-name`. Flip the random pet-name
/// default (`defaults.enable_randomized_pet_name_by_default`).
async fn toggle_randomized_pet_name_default(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::ToggleRandomizedPetNameDefault {},
    )
    .await
}

/// `POST /api/v1/ui/toggle-pr-banner-position`. Swap the PR banner between the
/// top and bottom of the agent pane (`ui.pr_banner_position`).
async fn toggle_pr_banner_position(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::TogglePrBannerPosition {}).await
}

/// `POST /api/v1/ui/toggle-github-integration`. Flip GitHub PR integration
/// (`ui.github_integration`) and its engine-side PR-sync side effects.
async fn toggle_github_integration(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::ToggleGithubIntegration {}).await
}

// ── Raw config editor (Monaco) ───────────────────────────────────────────────

#[derive(Serialize)]
struct RawConfigBody {
    /// The raw `config.toml` text, verbatim from disk (or the plain render of the
    /// running config when no file exists yet).
    content: String,
}

#[derive(Deserialize)]
struct WriteRawConfigBody {
    content: String,
}

/// `GET /api/v1/config/raw`. Return the raw `config.toml` text for the Monaco
/// editor. Reading is gated like every other config route but takes no body. A
/// read failure (permission/IO, or the engine being gone) is a `503` so the
/// editor surfaces an error instead of opening on blank content.
async fn read_raw_config(State(state): State<AppState>) -> Response {
    match state.engine.read_raw_config().await {
        Ok(content) => Json(RawConfigBody { content }).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

/// `PUT /api/v1/config/raw`. Validate (`toml::from_str::<Config>`) and write the
/// raw `config.toml` text verbatim. `200 OK` on success; `400` with the parse/IO
/// error otherwise. This PERSISTS only — the engine does NOT adopt the change and
/// emits no `config.changed`; the running config is untouched until the user
/// explicitly runs `POST /api/v1/config/reload`. Reload is the single apply point.
async fn write_raw_config(
    State(state): State<AppState>,
    Json(body): Json<WriteRawConfigBody>,
) -> Response {
    match state.engine.write_raw_config(body.content).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
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

    use crate::test_support::router_no_auth;

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
    async fn read_raw_config_returns_ok() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config/raw")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_then_write_round_trips_with_200() {
        let (_tmp, app) = router_no_auth();
        // Read the current raw config and confirm the body carries `content`.
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config/raw")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(get.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let content = parsed["content"]
            .as_str()
            .expect("read body must carry a content string")
            .to_string();
        assert!(!content.is_empty(), "content must not be empty");

        // Write it back unchanged: valid TOML with an unchanged [server] section,
        // so the happy path returns 200 (exercises the Ok arm of the persist).
        let body = serde_json::json!({ "content": content }).to_string();
        let put = app
            .oneshot(json_req("PUT", "/api/v1/config/raw", &body))
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn write_raw_config_rejects_invalid_toml_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/config/raw",
                r#"{"content":"this is = = not valid toml"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn preference_toggles_accept_a_post_with_no_body() {
        for uri in [
            "/api/v1/defaults/toggle-randomized-pet-name",
            "/api/v1/ui/toggle-pr-banner-position",
            "/api/v1/ui/toggle-github-integration",
        ] {
            let (_tmp, app) = router_no_auth();
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "POST {uri}");
        }
    }
}
