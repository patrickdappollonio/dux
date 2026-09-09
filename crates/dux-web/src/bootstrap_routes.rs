//! `GET /api/v1/bootstrap`: the build- and config-static snapshot a web client
//! needs once on load. These fields change only on a config reload, so a client
//! refetches when a `config.changed` event fires.
//!
//! Status codes:
//! - 200 with the [`dux_core::viewmodel::BootstrapView`] JSON.
//! - 503 if the engine actor is gone, so a dead engine is distinguishable from a
//!   real (always non-empty) payload.

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
            // Web-server state, not engine state: the gate runs once per launch and
            // the answer is parked in `AppState`, so the engine's projection always
            // leaves this `None` and a browser arriving later still gets the screen.
            view.pending_first_load = state.first_load.pending();
            // `--no-tailscale` belongs to the process that parsed the command
            // line, not to the engine's config, so it is injected here for the
            // same reason the first-load screen is.
            view.tailscale_forced_no = state.tailscale_forced_no;
            Json(view).into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response(),
    }
}
