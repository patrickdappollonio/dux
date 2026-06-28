//! `GET /api/v1/bootstrap` — the REST read for the build-/config-static snapshot a
//! web client needs once on load: version, configured providers, welcome tips,
//! macros, palette commands, the relevant `ui.*` flags, GitHub availability, and
//! the global env.
//!
//! These fields used to ride inside every per-tick `ViewModel` broadcast even
//! though they change only on a config reload. They now live on
//! [`dux_core::viewmodel::BootstrapView`], served once here and refetched by the
//! client when a `config.changed` event fires (emitted by the web layer on a
//! successful reload — see `server.rs`).
//!
//! Status codes:
//! - 200 with the [`dux_core::viewmodel::BootstrapView`] JSON.
//! - 503 if the engine actor is gone (the handle round-trip failed), so a dead
//!   engine is distinguishable from a real (always-non-empty) payload.
//!
//! Merged into the authenticated (gated) sub-router in `server.rs`, so an
//! unauthenticated request 401s before reaching this handler.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};

use crate::server::AppState;

/// The gated bootstrap read route.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/bootstrap", get(get_bootstrap))
}

async fn get_bootstrap(State(state): State<AppState>) -> Response {
    match state.engine.bootstrap().await {
        Some(view) => Json(view).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response(),
    }
}
