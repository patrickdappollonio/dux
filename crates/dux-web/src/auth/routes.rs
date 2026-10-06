//! `/api/v1/auth/*`: the status read and the login (public), and the sign-out,
//! the password change and the no-password warning's dismissal (protected like
//! any other route). Every one sits under the same-origin check for mutations.
//!
//! The answers follow the contract the browser reads (`lib/authApi.ts`,
//! `lib/authActions.ts`, `lib/authErrors.ts`): a refusal is JSON with an
//! `error` code and, where the browser shows one, a `message` in words.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use dux_core::auth::{MinimumFailure, Password};
use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use super::provenance::ClientClass;
use super::{RequestAuth, Verify, cookie};
use crate::server::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/status", get(status))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/password", post(change_password))
        .route(
            "/api/v1/auth/dismiss-no-auth-warning",
            post(dismiss_no_auth_warning),
        )
}

/// What the status says about a `[server.auth]` that cannot be used: the
/// reason, to this machine and the tailnet, or only `true` to anyone else,
/// whose page then shows no detail of its own (decided).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum BrokenDoc {
    Reason(String),
    Broken(bool),
}

/// `GET /api/v1/auth/status`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct StatusDoc {
    pub(crate) password_set: bool,
    pub(crate) required_here: bool,
    pub(crate) signed_in: bool,
    pub(crate) client_class: &'static str,
    pub(crate) transport_encrypted: bool,
    pub(crate) no_auth_warning: bool,
    pub(crate) weak_password: bool,
    pub(crate) can_set_first_password: bool,
    pub(crate) auth_broken: Option<BrokenDoc>,
    pub(crate) minimum_password_length: u32,
    pub(crate) minimum_password_score: u8,
    /// Why this device, which reached dux over loopback, is treated as the
    /// network (so it signs in, and cannot set the first password): dux could
    /// not rule out that the request was relayed from elsewhere. One line, with
    /// setting names in backticks. `null` otherwise. Shown, never decided on.
    pub(crate) required_reason: Option<String>,
}

async fn status(
    State(state): State<AppState>,
    Extension(auth): Extension<RequestAuth>,
) -> Response {
    let mut response = Json(state.auth.status(&auth.0)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn refusal(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn rate_limited(limited: super::admission::Limited) -> Response {
    let seconds = limited.seconds;
    let from = limited.held_by.words();
    let mut response = refusal(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "error": "rate_limited",
            "retry_after_seconds": seconds,
            // Whose failures this waits out, in words, for the browser's own
            // sentence: never "from this address" for a shared count.
            "from": from,
            "message": format!(
                "Too many sign-in attempts {from} right now. Try again in {seconds} {}.",
                if seconds == 1 { "second" } else { "seconds" }
            ),
        }),
    );
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&seconds.to_string()).expect("digits"),
    );
    response
}

fn busy() -> Response {
    let mut response = refusal(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "error": "rate_limited",
            "retry_after_seconds": 1,
            "message": "dux is checking too many passwords right now. Try again shortly.",
        }),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

fn bad_request(message: &str) -> Response {
    refusal(
        StatusCode::BAD_REQUEST,
        json!({ "error": "bad_request", "message": message }),
    )
}

fn too_long(maximum: u32) -> Response {
    refusal(
        StatusCode::BAD_REQUEST,
        json!({
            "error": "password_too_long",
            "maximum": maximum,
            "message": format!(
                "That password is longer than dux accepts ({maximum} bytes, max_password_bytes)."
            ),
        }),
    )
}

fn server_error(message: String) -> Response {
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "internal", "message": message }),
    )
}

/// Read a request body of at most `limit` bytes into a buffer that is wiped
/// when dropped. A body that is larger, or that fails to arrive, is `None`.
async fn bounded_body(body: Body, limit: usize) -> Option<Zeroizing<Vec<u8>>> {
    let bytes = axum::body::to_bytes(body, limit).await.ok()?;
    Some(Zeroizing::new(bytes.to_vec()))
}

/// The most a body carrying `fields` passwords of at most `max_bytes` each can
/// need: JSON may escape every byte of a password as six (`\u00XX`), plus room
/// for the keys and punctuation. Applied before anything is parsed or hashed.
fn body_limit(max_bytes: u32, fields: usize) -> usize {
    (max_bytes as usize)
        .saturating_mul(6)
        .saturating_mul(fields)
        + 1024
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

/// `POST /api/v1/auth/login` with `{password}`.
async fn login(
    State(state): State<AppState>,
    Extension(auth): Extension<RequestAuth>,
    request: Request,
) -> Response {
    let snapshot = state.auth.snapshot();
    let config = &snapshot.config;
    let Some(raw) = bounded_body(
        request.into_body(),
        body_limit(config.max_password_bytes, 1),
    )
    .await
    else {
        return too_long(config.max_password_bytes);
    };
    let Ok(body) = serde_json::from_slice::<LoginBody>(&raw) else {
        return bad_request("The sign-in request must be JSON with a password field.");
    };
    drop(raw);
    let password = Password::new(body.password);
    if password.byte_len() > config.max_password_bytes as usize {
        return too_long(config.max_password_bytes);
    }
    if !snapshot.has_password() {
        return refusal(
            StatusCode::CONFLICT,
            json!({
                "error": "no_password",
                "message": "No password is set on this dux, so there is nothing to sign in to.",
            }),
        );
    }
    let a = &auth.0;
    match state.auth.verify(&a.classification, password).await {
        Verify::Right { generation, weak } => {
            state.auth.note_strength(&generation, weak);
            let token = match state.auth.issue_session(&a.facts, &generation).await {
                Ok(super::Issued::Session(token)) => token,
                Ok(super::Issued::Blocked) => return super::middleware::blocked(),
                Ok(super::Issued::Stale) => return password_changed_meanwhile(),
                Err(error) => return server_error(format!("Could not start a session: {error:#}")),
            };
            let secure = cookie::secure(config.cookie_secure, a.classification.https_serve_route);
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                cookie::set(a.cookie_port, &token.cookie_value, secure),
            );
            response
        }
        Verify::Wrong => refusal(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "wrong_password", "message": "That password is not right." }),
        ),
        Verify::Blocked => super::middleware::blocked(),
        Verify::Wait(limited) => rate_limited(limited),
        Verify::Busy => busy(),
        Verify::NoPassword => refusal(
            StatusCode::CONFLICT,
            json!({
                "error": "no_password",
                "message": "No password is set on this dux, so there is nothing to sign in to.",
            }),
        ),
        Verify::Stale => password_changed_meanwhile(),
        Verify::Failed(error) => server_error(format!("Could not check the password: {error}")),
    }
}

/// The answer to a sign-in whose password changed while it was checked.
fn password_changed_meanwhile() -> Response {
    refusal(
        StatusCode::CONFLICT,
        json!({
            "error": "password_changed",
            "message": "The password changed while this sign-in was being checked. Sign in with the new one.",
        }),
    )
}

/// `POST /api/v1/auth/logout`: end this browser's session and clear its
/// cookie. Answers 204 even when there was no session, because the browser
/// asked for exactly that outcome.
async fn logout(
    State(state): State<AppState>,
    Extension(auth): Extension<RequestAuth>,
) -> Response {
    let a = &auth.0;
    // Every session the browser presented, not just the one that let it in:
    // signing out must leave nothing behind (decided, after review).
    for digest in &a.presented {
        state.auth.end_session(*digest).await;
    }
    let secure = cookie::secure(
        a.snapshot.config.cookie_secure,
        a.classification.https_serve_route,
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie::clear(a.cookie_port, secure));
    response
}

#[derive(Deserialize)]
struct PasswordBody {
    #[serde(default)]
    current: Option<String>,
    new: String,
}

/// Why a new password is refused, in the contract's shape: too short names the
/// minimum, too weak carries the score and zxcvbn's feedback.
fn below_minimums(check: &dux_core::auth::MinimumCheck) -> Response {
    for failure in &check.failures {
        match failure {
            MinimumFailure::TooShort { minimum, .. } => {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": "password_too_short",
                        "minimum": minimum,
                        "message": format!(
                            "That password is too short: dux asks for at least {minimum} characters."
                        ),
                    }),
                );
            }
            MinimumFailure::TooLong { maximum, .. } => return too_long(*maximum),
            // A browser's password field cannot type one, so such a password
            // could never be used to sign in here.
            MinimumFailure::ControlCharacter => {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": "password_control_character",
                        "message": "That password contains a line break, a tab or another \
                                    control character, which a browser's password field \
                                    cannot type, so dux refused it.",
                    }),
                );
            }
            MinimumFailure::TooWeak { .. } => {}
        }
    }
    let strength = &check.strength;
    refusal(
        StatusCode::BAD_REQUEST,
        json!({
            "error": "weak_password",
            "score": strength.score,
            "message": format!(
                "That password is too easy to guess (it rates {}), so dux refused it.",
                strength.label.as_str()
            ),
            "feedback": {
                "warning": strength.warning,
                "suggestions": strength.suggestions,
            },
        }),
    )
}

/// `POST /api/v1/auth/password` with `{current?, new}`: set the first password
/// (only from this machine or the tailnet; anywhere else it is `dux config set`'s
/// job), or change the password, which needs the current one. Every browser is
/// signed out by the change, this one included.
///
/// This guards against a change made by accident or through the API, not
/// against someone who already has a terminal through dux: they can edit the
/// config file itself.
async fn change_password(
    State(state): State<AppState>,
    Extension(auth): Extension<RequestAuth>,
    request: Request,
) -> Response {
    let snapshot = state.auth.snapshot();
    let config = &snapshot.config;
    let Some(raw) = bounded_body(
        request.into_body(),
        body_limit(config.max_password_bytes, 2),
    )
    .await
    else {
        return too_long(config.max_password_bytes);
    };
    let Ok(body) = serde_json::from_slice::<PasswordBody>(&raw) else {
        return bad_request(
            "The password change must be JSON with a new field (and current, to change one).",
        );
    };
    drop(raw);
    let new = Password::new(body.new);
    let current = body.current.map(Password::new);
    let a = &auth.0;
    let Some(config_path) = state.auth.config_path().cloned() else {
        return server_error("This server has no config file to write the password to.".into());
    };

    // Which hash the write may replace: none for a first password, else the
    // one the current password was checked against.
    let expected = match snapshot.config.password_hash() {
        None => {
            if !matches!(
                a.classification.class,
                ClientClass::ThisMachine | ClientClass::Tailnet
            ) {
                let because = a
                    .classification
                    .loopback_distrusted
                    .map(|cause| format!(" {}", route_refusal(a.classification.via, cause)))
                    .unwrap_or_default();
                return refusal(
                    StatusCode::FORBIDDEN,
                    json!({
                        "error": "first_password_not_here",
                        "message": format!(
                            "The first password can only be set from the machine dux runs on, \
                             from your tailnet, or with `dux config set \
                             server.auth.password`.{because}"
                        ),
                    }),
                );
            }
            String::new()
        }
        Some(hash) => {
            let Some(current) = current else {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": "current_password_required",
                        "message": "Changing the password needs the current one.",
                    }),
                );
            };
            if current.byte_len() > config.max_password_bytes as usize {
                return too_long(config.max_password_bytes);
            }
            match state.auth.verify(&a.classification, current).await {
                Verify::Right { .. } => hash.to_string(),
                Verify::Wrong => {
                    return refusal(
                        StatusCode::FORBIDDEN,
                        json!({ "error": "wrong_current_password" }),
                    );
                }
                Verify::Blocked => return super::middleware::blocked(),
                Verify::Wait(limited) => return rate_limited(limited),
                Verify::Busy => return busy(),
                Verify::NoPassword | Verify::Stale => {
                    return refusal(
                        StatusCode::CONFLICT,
                        json!({
                            "error": "password_changed",
                            "message": "The password changed while this was being checked, so nothing was changed. Try again.",
                        }),
                    );
                }
                Verify::Failed(error) => {
                    return server_error(format!("Could not check the current password: {error}"));
                }
            }
        }
    };

    if new.byte_len() > config.max_password_bytes as usize {
        return too_long(config.max_password_bytes);
    }
    let policy = config.password_policy();
    let words = dux_core::auth::guess_words();
    let surface = state.engine.reload_surface();
    let written = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        // Measured against the live minimums first, so the answer describes
        // the minimums the browser was shown; the write re-checks the file's.
        let check = dux_core::auth::check_minimums(&new, &policy, &refs);
        if !check.passes() {
            return Err(dux_core::config_keys::SetPasswordError::BelowMinimums(
                check,
            ));
        }
        let set =
            dux_core::config_keys::set_password_if_current(&config_path, &expected, &new, &refs)?;
        let file_write = dux_core::config_write::take_last_write();
        // Not in force only when the reload of THIS run refuses the file: the
        // problems that stop the surface whose reload applies it, judged the
        // way `start_refusal` judges them, never "any problem remains" (a file
        // only the other surface refuses still loads here).
        let stopping: Vec<String> = set
            .remaining_problems
            .into_iter()
            .filter(|problem| stops_reload(problem, surface))
            .map(|problem| problem.detail)
            .collect();
        if !stopping.is_empty() {
            return Ok(Err(stopping));
        }
        // The hash now in the file is the one to apply at once; the reload
        // that follows brings the rest along.
        Ok(Ok((
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|raw| dux_core::config::auth_section_of(&raw).ok())
                .map(|section| section.password_hash),
            file_write,
        )))
    })
    .await;
    match written {
        // Stored, but the file has problems that stop dux starting with it (they
        // were there before; the write added none), so the reload that would
        // put it in force is refused and the old password stays the one.
        Ok(Ok(Err(problems))) => refusal(
            StatusCode::CONFLICT,
            json!({
                "error": "password_not_in_force",
                "message": format!(
                    "The new password is saved in config.toml, but dux cannot use it until \
                     these problems in that file are fixed: {}. Until then the old password \
                     stays in force.",
                    problems.join("; ")
                ),
            }),
        ),
        Ok(Ok(Ok((hash, file_write)))) => {
            match hash {
                Some(hash) => state.auth.applied(
                    move |config| config.password_hash = hash.clone(),
                    file_write,
                ),
                None => state.auth.applied(|_| {}, file_write),
            }
            let mut response = StatusCode::NO_CONTENT.into_response();
            let secure = cookie::secure(config.cookie_secure, a.classification.https_serve_route);
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie::clear(a.cookie_port, secure));
            response
        }
        Ok(Err(dux_core::config_keys::SetPasswordError::BelowMinimums(check))) => {
            below_minimums(&check)
        }
        Ok(Err(dux_core::config_keys::SetPasswordError::ChangedMeanwhile)) => refusal(
            StatusCode::CONFLICT,
            json!({
                "error": "password_changed",
                "message": "The password was changed by someone else meanwhile, so nothing was changed. Try again.",
            }),
        ),
        Ok(Err(dux_core::config_keys::SetPasswordError::Failed(error))) => {
            server_error(format!("Could not write the password: {error:#}"))
        }
        Err(error) => server_error(format!("The password change stopped: {error}")),
    }
}

/// `POST /api/v1/auth/dismiss-no-auth-warning`: the red banner's "don't show
/// again", which writes `disable_no_auth_warning = true`.
///
/// With no password set this is reachable by anyone who can reach dux, and that
/// is decided: the warning exists only while there is no password, so whoever
/// can press it can already drive every terminal and edit the config itself.
/// The single-owner trust model has nothing more to protect here, and the
/// terminal's own warnings are not silenced by it.
async fn dismiss_no_auth_warning(State(state): State<AppState>) -> Response {
    let Some(path) = state.auth.config_path().cloned() else {
        return server_error("This server has no config file to save that choice to.".into());
    };
    let written = tokio::task::spawn_blocking(
        move || -> anyhow::Result<Option<dux_core::config_write::FileWrite>> {
            let key = dux_core::config_keys::lookup("server.auth.disable_no_auth_warning")
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            dux_core::config_keys::set_plain(&path, &key, "true")?;
            Ok(dux_core::config_write::take_last_write())
        },
    )
    .await;
    match written {
        Ok(Ok(file_write)) => {
            state
                .auth
                .applied(|config| config.disable_no_auth_warning = true, file_write);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(error)) => server_error(format!("Could not save that choice: {error:#}")),
        Err(error) => server_error(format!("Saving that choice stopped: {error}")),
    }
}

/// Whether `problem` makes the reload of `surface` refuse the file: exactly
/// the rule [`dux_core::config::start_refusal`] applies, under which a
/// `dux server` problem its command line takes the place of is not one.
fn stops_reload(
    problem: &dux_core::config::StartProblem,
    surface: dux_core::config::Surface,
) -> bool {
    problem.stops(surface)
        && !(surface == dux_core::config::Surface::DuxServer
            && problem.dux_server_override.is_some())
}

/// Why a device that might have been this machine or the tailnet was not,
/// named by the route it really took (decided, after review: the sentence
/// once said "over loopback" for connections that never touched it). An
/// exhaustive match, so a new route cannot inherit another's words.
fn route_refusal(via: super::provenance::Via, cause: &str) -> String {
    use super::provenance::Via;
    match via {
        Via::PlainLoopback => format!(
            "This browser reached dux over loopback, but {cause}, so dux cannot tell it is \
             this machine."
        ),
        Via::OwnAddress => "This browser reached dux from one of this machine's own addresses, \
             which a relay on this machine (socat, `ssh -L`, a proxy) looks exactly like, so dux \
             cannot tell it is this machine."
            .to_string(),
        Via::Direct => format!(
            "This device reached dux over the tailnet, but {cause}, so dux cannot treat it as \
             the tailnet."
        ),
        Via::Forwarded => format!(
            "This browser reached dux through tailscale serve, but {cause}, so dux cannot treat \
             it as the tailnet."
        ),
        Via::ControlSocket => {
            format!("This client reached dux over its control socket, but {cause}.")
        }
    }
}

#[cfg(test)]
mod route_refusal_tests {
    use super::*;
    use crate::auth::provenance::Via;

    #[test]
    fn only_the_loopback_route_is_said_to_be_loopback() {
        for via in [
            Via::PlainLoopback,
            Via::OwnAddress,
            Via::Direct,
            Via::Forwarded,
        ] {
            let text = route_refusal(via, "something is unconfirmed");
            assert_eq!(
                text.contains("over loopback"),
                via == Via::PlainLoopback,
                "{via:?}: {text}"
            );
        }
    }
}
