//! What an open WebSocket holds so the login can reach it after the upgrade.
//!
//! A socket passes the auth layer once, at its upgrade, and never again, so
//! every events and PTY loop holds a [`SocketAuth`] and closes the moment it
//! resolves: on a sign-out, a password change or removal, a tighter `require`,
//! the client's address being blocked, or the exposure changing what its client
//! counts as. It also holds the session's lease, which keeps a signed-in tab
//! from going idle for as long as the socket lives.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use dux_core::web_sessions::TokenDigest;

use super::provenance::RequestFacts;
use super::sessions::SessionLease;
use super::{AuthState, RequestAuth};

/// The close code for a socket whose session ended or never was: signed out,
/// revoked, expired, or a password change.
pub const CLOSE_SIGNED_OUT: u16 = 4401;
/// The close code for a socket whose client address is blocked.
pub const CLOSE_BLOCKED: u16 = 4403;

/// The close reason sent with a code, for logs and developer tools; the
/// browser decides on the code alone.
pub(crate) fn close_reason(code: u16) -> &'static str {
    match code {
        CLOSE_BLOCKED => "blocked",
        _ => "signed out",
    }
}

/// An open socket's hold on the login. See the module doc.
pub(crate) struct SocketAuth {
    watch: Option<Watch>,
}

struct Watch {
    state: std::sync::Arc<AuthState>,
    facts: RequestFacts,
    session: Option<TokenDigest>,
    _lease: Option<SessionLease>,
    revision: tokio::sync::watch::Receiver<u64>,
    exposure: Option<tokio::sync::watch::Receiver<crate::exposure::Exposure>>,
}

impl SocketAuth {
    /// A socket with no login behind it (a router built without the auth
    /// layer). It never closes for auth.
    pub(crate) fn none() -> Self {
        Self { watch: None }
    }

    pub(crate) fn new(state: std::sync::Arc<AuthState>, auth: &RequestAuth) -> Self {
        let session = auth.0.session;
        let lease = session.and_then(|digest| state.sessions.lease(digest));
        let revision = state.subscribe_revision();
        let exposure = state.subscribe_exposure();
        Self {
            watch: Some(Watch {
                facts: auth.0.facts.clone(),
                session,
                _lease: lease,
                revision,
                exposure,
                state,
            }),
        }
    }

    /// The close code this socket must close with right now, if any.
    pub(crate) fn verdict_now(&self) -> Option<u16> {
        let watch = self.watch.as_ref()?;
        watch
            .state
            .socket_verdict(&watch.facts, watch.session.as_ref())
    }

    /// [`SocketAuth::verdict_now`] for a socket about to send its opening
    /// frames, after the test seam (when one is set) has had its turn.
    pub(crate) async fn opening_verdict(&self) -> Option<u16> {
        let watch = self.watch.as_ref()?;
        if let Some(hook) = &watch.state.opening_hook {
            hook().await;
        }
        self.verdict_now()
    }

    /// Resolves with the close code once this socket must close; at once when
    /// it already must. Never resolves for [`SocketAuth::none`].
    pub(crate) async fn revoked(&mut self) -> u16 {
        let Some(watch) = self.watch.as_mut() else {
            return std::future::pending().await;
        };
        loop {
            watch.revision.borrow_and_update();
            if let Some(exposure) = watch.exposure.as_mut() {
                exposure.borrow_and_update();
            }
            if let Some(code) = watch
                .state
                .socket_verdict(&watch.facts, watch.session.as_ref())
            {
                return code;
            }
            let exposure_changed = async {
                match watch.exposure.as_mut() {
                    Some(exposure) => exposure.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                changed = watch.revision.changed() => {
                    if changed.is_err() {
                        return std::future::pending().await;
                    }
                }
                still = exposure_changed => {
                    if !still {
                        watch.exposure = None;
                    }
                }
            }
        }
    }
}

impl FromRequestParts<crate::server::AppState> for SocketAuth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::server::AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(match parts.extensions.get::<RequestAuth>() {
            Some(auth) => Self::new(std::sync::Arc::clone(&state.auth), auth),
            None => Self::none(),
        })
    }
}
