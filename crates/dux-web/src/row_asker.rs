//! Who an HTTP request asks as when it would change a row of the agents list
//! that a device drives, and the one answer every such refusal gets.
//!
//! [`identify`] wraps every route. It works out the asker once per request,
//! the browser tab whose events connection the request names (the attachment
//! guard's own rule, [`crate::rest_common::exempt_requester`]) and whether the
//! request said to go ahead over everybody (`force_connected=true`), and keeps
//! it in a task local for the length of the request. The engine handle reads
//! it for every change it sends ([`RowAsker::current`]), so no route passes it
//! along and none can leave it out; the question itself is the engine's
//! ([`dux_core::engine::row_guard`]). A refusal is written back into the asker,
//! and [`identify`] answers it as `423 Locked` with the engine's sentence,
//! whatever the route made of the error.
//!
//! Outside a request there is no asker: a change sent from there is judged as
//! a client with no connection, which a driven row refuses.

use std::sync::{Arc, OnceLock};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::rest_common::{
    ForceConnectedQuery, RouteRejection, exempt_requester, scope_from_headers,
};
use crate::server::{AppState, SocketClient};

tokio::task_local! {
    static ASKING: RowAsker;
}

/// Who is asking for a change to a row, and where its refusal is left.
#[derive(Clone, Debug, Default)]
pub(crate) struct RowAsker {
    /// The asking tab's events connection, `None` for a client with none.
    connection: Option<String>,
    /// Whether the request said to go ahead over everybody.
    force: bool,
    refused: Arc<OnceLock<String>>,
}

impl RowAsker {
    /// The asker of the request this task is answering, or a client with no
    /// connection when it is answering none.
    pub(crate) fn current() -> Self {
        ASKING.try_with(Clone::clone).unwrap_or_default()
    }

    /// The asking tab's events connection.
    pub(crate) fn connection(&self) -> Option<&str> {
        self.connection.as_deref()
    }

    /// Whether the request said to go ahead over everybody.
    pub(crate) fn forces(&self) -> bool {
        self.force
    }

    /// The engine refused this asker's change with `sentence`.
    pub(crate) fn refuse(&self, sentence: String) {
        let _ = self.refused.set(sentence);
    }
}

/// `423 Locked` with the engine's sentence: another device drives the row.
pub(crate) fn driven_refusal(sentence: &str) -> RouteRejection {
    (StatusCode::LOCKED, sentence.to_string())
        .into_response()
        .into()
}

/// Run the request with its asker set, and answer any change the engine
/// refused because another device drives the row.
pub(crate) async fn identify(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let scope = scope_from_headers(&parts.headers, &state.connections);
    let client = SocketClient::from_parts(&parts);
    let asker = RowAsker {
        connection: exempt_requester(
            state.engine.attachments(),
            &scope,
            client.address(),
            std::time::Instant::now(),
        ),
        force: axum::extract::Query::<ForceConnectedQuery>::try_from_uri(&parts.uri)
            .is_ok_and(|axum::extract::Query(force)| force.over_everybody()),
        refused: Arc::default(),
    };
    let refused = Arc::clone(&asker.refused);
    let response = ASKING
        .scope(asker, next.run(Request::from_parts(parts, body)))
        .await;
    match refused.get() {
        Some(sentence) => driven_refusal(sentence).into_response(),
        None => response,
    }
}
