//! `GET /api/v1/bootstrap` — the REST read for the build-/config-static snapshot a
//! web client needs once on load: version, configured providers, welcome tips,
//! macros, palette commands, the relevant `ui.*` flags, GitHub availability, and
//! the global env.
//!
//! These fields change only on a config reload, so they live on
//! [`dux_core::viewmodel::BootstrapView`], served once here and refetched by the
//! client when a `config.changed` event fires (emitted by the web layer on a
//! successful reload — see `server.rs`).
//!
//! Status codes:
//! - 200 with the [`dux_core::viewmodel::BootstrapView`] JSON.
//! - 503 if the engine actor is gone (the handle round-trip failed), so a dead
//!   engine is distinguishable from a real (always-non-empty) payload.
//!
//! Served like every other API route. dux has NO authentication of any kind, so
//! nothing here ever 401s. That open access is deliberate: the single-tenant
//! trusted-access model documented in CLAUDE.md. The two app-wide guards are a
//! Host-header allowlist, which stops a malicious web page from rebinding DNS
//! into this server, and a same-origin check that applies to MUTATIONS only, so
//! this GET is not behind it. Neither guard is authentication.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};

use crate::server::AppState;

/// The bootstrap read route.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/bootstrap", get(get_bootstrap))
}

async fn get_bootstrap(State(state): State<AppState>) -> Response {
    match state.engine.bootstrap().await {
        Some(mut view) => {
            // The pending first-load screen is web-server state, not engine
            // state: the gate runs once per LAUNCH and the answer is parked in
            // `AppState` (see `first_load_routes`). The engine's projection
            // always leaves this `None`, so injecting it here is what lets a
            // browser connecting at any point in the server's life still receive
            // the screen.
            view.pending_first_load = state.first_load.pending();
            Json(view).into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response(),
    }
}
