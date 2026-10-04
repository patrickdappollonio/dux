//! The ONE auth layer around the whole router, inside the Host guard.
//!
//! Every request, HTTP or WebSocket upgrade, is classified here, then:
//!
//! 1. A blocked client is refused (`403 {error:"blocked", where}`), on every
//!    route, with or without a password.
//! 2. A request to a declared public route passes ([`is_public`]).
//! 3. With no password set, everything else passes too.
//! 4. A request that needs a session and has none is refused
//!    (`401 {error:"auth_required"}`); one whose `[server.auth]` cannot be
//!    used gets `503 {error:"auth_config_invalid", detail}`.
//!
//! A refused WebSocket upgrade is ACCEPTED and then closed with 4401 (signed
//! out) or 4403 (blocked), because a browser reports a refused upgrade as a
//! bare network drop (1006) and could not tell why.
//!
//! What the layer worked out rides on the request as a
//! [`super::RequestAuth`] extension for the auth routes and the sockets.

use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::provenance::{Arrival, RequestFacts};
use super::{BLOCKED_WHERE, RequestAuth};
use crate::server::AppState;

/// Files the browser needs before it can show the login page. Static UI assets
/// are not secret.
const PUBLIC_FILES: &[&str] = &[
    "/",
    "/index.html",
    "/favicon.png",
    "/sw.js",
    "/manifest.webmanifest",
    "/offline.html",
    "/icon-192.png",
    "/icon-512.png",
    "/icon-maskable-512.png",
    "/dux-logo.png",
    "/icons.svg",
    "/healthz",
];

/// Whether a request is to a declared public route: the files above and
/// `/assets/*` by GET or HEAD, the auth status by GET or HEAD, and the login by
/// POST. Any other method on those paths is an ordinary protected request.
pub(crate) fn is_public(method: &Method, path: &str) -> bool {
    let read = method == Method::GET || method == Method::HEAD;
    if read && (PUBLIC_FILES.contains(&path) || is_asset(path)) {
        return true;
    }
    (read && path == "/api/v1/auth/status")
        || (method == Method::POST && path == "/api/v1/auth/login")
}

/// A path under `/assets/` with nothing in it that could step outside.
fn is_asset(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/assets/") else {
        return false;
    };
    let lower = rest.to_ascii_lowercase();
    !rest.is_empty() && !rest.split('/').any(|part| part == "..") && !lower.contains('%')
}

fn is_websocket_upgrade(request: &Request) -> bool {
    request
        .headers()
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// The JSON refusals of the contract.
pub(crate) fn auth_required() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({ "error": "auth_required" })),
    )
        .into_response()
}

pub(crate) fn blocked() -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(json!({ "error": "blocked", "where": BLOCKED_WHERE })),
    )
        .into_response()
}

fn broken(detail: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({ "error": "auth_config_invalid", "detail": detail })),
    )
        .into_response()
}

/// Accept the upgrade and close it with `code`, so the browser hears why.
async fn close_upgrade(request: Request, code: u16) -> Response {
    let (mut parts, _) = request.into_parts();
    if !crate::server::same_origin_allowed(&parts.headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws
            .on_upgrade(move |mut socket| async move {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code,
                        reason: super::socket::close_reason(code).into(),
                    })))
                    .await;
            })
            .into_response(),
        Err(rejection) => rejection.into_response(),
    }
}

/// The layer itself. See the module doc.
pub(crate) async fn auth_layer(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // The serve records both ends of the connection; the handlers that only
    // want the peer read it the way they always have.
    let arrival = request
        .extensions()
        .get::<ConnectInfo<Arrival>>()
        .map(|info| info.0);
    if let Some(arrival) = arrival
        && request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .is_none()
    {
        request.extensions_mut().insert(ConnectInfo(arrival.peer));
    }
    let facts = RequestFacts::of(arrival, request.headers());
    let assessment = state.auth.assess(facts, request.headers()).await;
    let upgrade = is_websocket_upgrade(&request);

    if assessment.blocked {
        return if upgrade {
            close_upgrade(request, super::socket::CLOSE_BLOCKED).await
        } else {
            blocked()
        };
    }
    state.auth.note_proxy(&assessment);
    if !is_public(request.method(), request.uri().path()) {
        if let Some(detail) = assessment.snapshot.broken.as_deref() {
            return broken(detail);
        }
        if assessment.required && assessment.session.is_none() {
            return if upgrade {
                close_upgrade(request, super::socket::CLOSE_SIGNED_OUT).await
            } else {
                auth_required()
            };
        }
    }
    request
        .extensions_mut()
        .insert(RequestAuth(std::sync::Arc::new(assessment)));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_declared_routes_and_methods_are_public() {
        for path in PUBLIC_FILES {
            assert!(is_public(&Method::GET, path), "{path}");
            assert!(is_public(&Method::HEAD, path), "{path}");
            assert!(!is_public(&Method::POST, path), "{path}");
        }
        assert!(is_public(&Method::GET, "/assets/index-abc123.js"));
        assert!(is_public(&Method::GET, "/api/v1/auth/status"));
        assert!(!is_public(&Method::POST, "/api/v1/auth/status"));
        assert!(is_public(&Method::POST, "/api/v1/auth/login"));
        assert!(!is_public(&Method::GET, "/api/v1/auth/login"));
        for protected in [
            "/api/v1/auth/logout",
            "/api/v1/auth/password",
            "/api/v1/auth/dismiss-no-auth-warning",
            "/api/v1/projects",
            "/api/v1/bootstrap",
            "/ws/events",
            "/agent/x",
            "/assets/",
            "/assets/../api/v1/projects",
            "/assets/%2e%2e/api/v1/projects",
            "/index.html/",
            "//index.html",
        ] {
            assert!(!is_public(&Method::GET, protected), "{protected}");
        }
    }
}
