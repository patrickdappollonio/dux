//! The two first-load screens (first-run welcome, post-upgrade what's-new) on the
//! server side: the startup gate, the pending screen held in memory, dismissal,
//! and the on-demand release-notes read.
//!
//! The gate here is the FIRST-LOAD gate, a per-launch decision about which welcome
//! screen to show. It has nothing to do with access control.
//!
//! `dux_core::first_load::plan` runs once, in a task spawned at
//! [`crate::server::build_app`] time, and the result is parked in
//! [`FirstLoadState`]. Deliberately not per request:
//!
//! - The gate is a decision about this LAUNCH, so two browsers connecting to one
//!   server must see the same answer.
//! - The what's-new path needs release notes, and that fetch blocks, so it must
//!   never sit on a request path.
//! - The version is stamped as seen on DISMISSAL, never when the plan is computed.
//!   The server is long-lived, so stamping at startup would leave a browser
//!   connecting a minute later with nothing to show.
//!
//! A `Nothing` plan carrying `mark_seen` is the one case that stamps immediately:
//! there is no screen to dismiss, so there is nothing to wait for.
//!
//! Resolving the plan emits a `config.changed` so an already-connected client
//! refetches its bootstrap and finds the screen; later clients read it out of their
//! first bootstrap. Dismissal emits the same event, or a second browser's dialog
//! would stay up after the screen was settled everywhere else.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};

use dux_core::first_load::{self, FirstLoad};
use dux_core::release_notes::{self};
use dux_core::viewmodel::PendingFirstLoadView;
use dux_core::wire::WireStatus;

use crate::engine_actor::EngineHandle;
use crate::event_bus::EventBus;
use crate::rest_common::scope_from_headers;
use crate::server::AppState;

/// The correlation key for the on-demand release-notes fetch. One key per
/// operation kind, so a `Busy` and its eventual success/error replace the same
/// toast instead of stacking (the keyed-status contract in CLAUDE.md).
const RELEASE_NOTES_STATUS_KEY: &str = "release-notes-fetch";

/// The pending first-load screen for this launch, plus the release-notes API base
/// the on-demand read talks to. Held in [`crate::server::AppState`] as an `Arc` so
/// every request sees one decision. The screen is `None` until the resolver task
/// finishes, and again once a client dismisses it.
pub struct FirstLoadState {
    pending: Mutex<Option<PendingFirstLoadView>>,
    /// Where release-notes fetches point. Production passes
    /// `dux_core::urls::GITHUB_API_BASE`; tests point it at a local server so no
    /// test ever contacts the real GitHub API.
    api_base: String,
}

impl FirstLoadState {
    pub fn new(api_base: impl Into<String>) -> Self {
        Self {
            pending: Mutex::new(None),
            api_base: api_base.into(),
        }
    }

    /// The screen to show, for injection into the bootstrap document. A cheap
    /// clone under a short lock; the guard never crosses an `.await`.
    pub fn pending(&self) -> Option<PendingFirstLoadView> {
        self.pending.lock().unwrap().clone()
    }

    fn set_pending(&self, screen: Option<PendingFirstLoadView>) {
        *self.pending.lock().unwrap() = screen;
    }
}

/// The first-load routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/first-load/dismiss", post(dismiss_first_load))
        .route("/api/v1/release-notes", get(get_release_notes))
}

/// Run the first-load gate for this launch, off the request path.
///
/// Spawned once from `build_app`. Requires a tokio runtime context, which every
/// `build_app` caller provides (the CLI serve paths build inside `block_on`; the
/// flip enters the runtime).
pub fn spawn_first_load_resolver(
    engine: EngineHandle,
    bus: Arc<EventBus>,
    state: Arc<FirstLoadState>,
) {
    tokio::spawn(async move {
        // No engine means no stored version and no config: show nothing and,
        // critically, stamp nothing.
        let Some(inputs) = engine.first_load_inputs().await else {
            return;
        };
        let plan = first_load::plan(
            inputs.last_seen.as_deref(),
            &inputs.running,
            inputs.disable_welcome,
            inputs.disable_release_notes,
        );

        match plan.screen {
            FirstLoad::Nothing => {
                // Nothing to dismiss, so this is the one path that stamps now.
                if plan.mark_seen {
                    mark_seen_logging_failure(&engine, &inputs.running).await;
                }
            }
            FirstLoad::Welcome => {
                // No network involved: the copy rides the bootstrap document.
                state.set_pending(Some(PendingFirstLoadView::welcome()));
                bus.emit(crate::server::config_changed_event());
            }
            FirstLoad::WhatsNew => {
                let api_base = state.api_base.clone();
                let root = inputs.state_root.clone();
                let running = inputs.running.clone();
                // The fetch BLOCKS; keep it off the async runtime's worker.
                let fetched = tokio::task::spawn_blocking(move || {
                    release_notes::load_release_notes_from(&api_base, &root, &running)
                })
                .await;
                // A panicked/cancelled blocking task is treated as a transient
                // failure: show nothing, stamp nothing, try again next launch.
                let fetched = match fetched {
                    Ok(result) => result,
                    Err(err) => {
                        dux_core::logger::warn(&format!(
                            "[server] the release-notes fetch task failed: {err}"
                        ));
                        return;
                    }
                };
                let outcome = release_notes::outcome_of(&fetched);
                let plan = first_load::after_fetch(plan, outcome);
                match (plan.screen, fetched) {
                    (FirstLoad::WhatsNew, Ok(notes)) => {
                        state.set_pending(Some(PendingFirstLoadView::whats_new(notes)));
                        bus.emit(crate::server::config_changed_event());
                    }
                    (_, result) => {
                        if let Err(err) = &result {
                            // A log line rather than a toast: nobody asked for this,
                            // it happened at startup. The severity branches on the
                            // same `is_definitive()` the response code uses, so only
                            // a transient failure reaches the warn stream.
                            let message =
                                format!("[server] no what's-new screen this launch: {err}");
                            if resolver_failure_is_actionable(err) {
                                dux_core::logger::warn(&message);
                            } else {
                                dux_core::logger::info(&message);
                            }
                        }
                        if plan.mark_seen {
                            mark_seen_logging_failure(&engine, &inputs.running).await;
                        }
                    }
                }
            }
        }
    });
}

/// Whether a startup-resolver fetch failure is worth an operator's attention as a
/// `warn` rather than an `info`. A definitive failure means GitHub has no release
/// for this tag, the normal shape of a locally built tagged binary, and nothing an
/// operator does changes it; only a transient failure is actionable. Same
/// `is_definitive()` split the on-demand handler uses to choose 404 or 502.
fn resolver_failure_is_actionable(err: &release_notes::FetchError) -> bool {
    !err.is_definitive()
}

/// Stamp the version, logging (never toasting) a failure. Called only from the
/// startup resolver, where no user is waiting on an answer.
async fn mark_seen_logging_failure(engine: &EngineHandle, version: &str) {
    if let Err(err) = engine.mark_version_seen(version.to_string()).await {
        dux_core::logger::warn(&format!(
            "[server] could not record {version} as seen; the first-load screen may \
             reappear next launch: {err}"
        ));
    }
}

/// `POST /api/v1/first-load/dismiss`. Records the running version as seen and drops
/// the pending screen. The write is what makes dismissal SHARED: `last_seen_version`
/// is one SQLite row the TUI reads too.
///
/// `500` with a message when the store write fails, and the screen then stays
/// pending, so the next bootstrap still carries it and disk and memory cannot
/// disagree.
async fn dismiss_first_load(State(state): State<AppState>) -> Response {
    let version = dux_core::display_version().to_string();
    match state.engine.mark_version_seen(version).await {
        Ok(()) => {
            state.first_load.set_pending(None);
            // `config.changed` is the only event that drives a bootstrap refetch,
            // so without this a second tab keeps its dialog open after the first
            // one dismissed.
            state.event_bus.emit(crate::server::config_changed_event());
            StatusCode::OK.into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
    }
}

/// `GET /api/v1/release-notes`. Independent of the gate: it works even under
/// `ui.disable_release_notes`, which suppresses only the automatic screen, and it
/// stamps no version, because looking at the notes is not a dismissal.
///
/// May fetch, cache first, so it runs on a blocking task and reports a `Busy` and
/// then its final on the same key, so the toast is replaced rather than stranded.
/// `404` when GitHub has no release for this tag, `502` for anything retryable.
async fn get_release_notes(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let scope = scope_from_headers(&headers, &state.connections);
    let Some(inputs) = state.engine.first_load_inputs().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response();
    };

    state.engine.emit_status(
        WireStatus::new("busy", "Fetching the release notes from GitHub...")
            .with_key(RELEASE_NOTES_STATUS_KEY)
            .with_scope(scope.clone()),
    );

    // EVERY path below must post a final on the same key, or the client strands a
    // Busy toast at Infinity. Declared before the permit wait so the bail-out
    // path below can resolve the Busy too.
    let final_status = |tone: &str, message: String| {
        state.engine.emit_status(
            WireStatus::new(tone, message)
                .with_key(RELEASE_NOTES_STATUS_KEY)
                .with_scope(scope.clone()),
        );
    };

    // The fetch below runs on a blocking thread and every tab can trigger it, so a
    // burst must not exhaust the pool. A request beyond the limit waits rather than
    // being rejected, and usually answers from cache once it gets in. `None` means
    // the config value is 0, unlimited.
    let _permit = match &state.release_notes_semaphore {
        Some(sem) => match Arc::clone(sem).acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => {
                let message =
                    "the release-notes concurrency semaphore closed unexpectedly".to_string();
                final_status("error", message.clone());
                return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response();
            }
        },
        None => None,
    };

    let api_base = state.first_load.api_base.clone();
    let root = inputs.state_root.clone();
    let running = inputs.running.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        release_notes::load_release_notes_from(&api_base, &root, &running)
    })
    .await;

    match fetched {
        Ok(Ok(notes)) => {
            // The final for the success path is a CLEAR, not an info: the
            // rendered notes ARE the success signal, and a toast narrating
            // what is already on screen is noise. The keyed contract is
            // still honored (the Busy never strands); only the error paths
            // below post a message-carrying final.
            state.engine.clear_status(RELEASE_NOTES_STATUS_KEY);
            Json(notes).into_response()
        }
        Ok(Err(err)) => {
            let message = err.to_string();
            final_status("error", message.clone());
            let status = if err.is_definitive() {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, message).into_response()
        }
        Err(err) => {
            let message = format!("The release-notes fetch did not finish: {err}");
            final_status("error", message.clone());
            (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_state_has_no_pending_screen() {
        let state = FirstLoadState::new("http://127.0.0.1:1");
        assert!(state.pending().is_none());
    }

    #[test]
    fn the_pending_screen_round_trips_and_clears() {
        let state = FirstLoadState::new("http://127.0.0.1:1");
        state.set_pending(Some(PendingFirstLoadView::welcome()));
        assert_eq!(
            state.pending().map(|p| p.screen),
            Some(PendingFirstLoadView::WELCOME.to_string())
        );
        // Dismissal drops it, so a later bootstrap in the same launch is clean.
        state.set_pending(None);
        assert!(state.pending().is_none());
    }

    /// The resolver's failure log must not cry wolf: a definitive "GitHub has no
    /// release for this tag" is the routine shape of a locally built tagged
    /// binary and is unactionable, so it is an info. A transient failure is the
    /// only one an operator might act on, so it stays a warning — which is what
    /// keeps the warn stream meaning "look at this".
    #[test]
    fn only_a_transient_resolver_failure_is_worth_a_warning() {
        let definitive = release_notes::FetchError::NoSuchRelease {
            tag: "v9.9.9".to_string(),
        };
        assert!(
            !resolver_failure_is_actionable(&definitive),
            "an unpublished tag is expected and unactionable: it must log as info"
        );

        let transient = release_notes::FetchError::Transient(anyhow::anyhow!("connection refused"));
        assert!(
            resolver_failure_is_actionable(&transient),
            "offline/timeout/rate-limit is retryable and worth an operator's eye"
        );
    }

    /// The dev-build fallback is the one behavioural branch in `load_notes`. It
    /// must ask for the NEWEST release (there is no tag to ask for), while a real
    /// version asks for its own tag. Proven without a network by pointing at a
    /// closed port and reading which URL the failure names.
    #[test]
    fn a_development_build_asks_for_the_newest_release_and_a_real_version_for_its_tag() {
        let tmp = tempfile::tempdir().unwrap();
        // Port 1 refuses immediately, so this is a fast, offline transient error.
        let base = "http://127.0.0.1:1";

        let err = release_notes::load_release_notes_from(
            base,
            tmp.path(),
            first_load::DEVELOPMENT_VERSION,
        )
        .expect_err("a closed port cannot serve notes");
        assert!(
            err.to_string().contains("/releases/latest"),
            "a dev build has no tag, so it must ask for the newest release: {err}"
        );
        assert!(!err.is_definitive(), "a refused connection is retryable");

        let err = release_notes::load_release_notes_from(base, tmp.path(), "v0.6.0")
            .expect_err("a closed port cannot serve notes");
        assert!(
            err.to_string().contains("/releases/tags/v0.6.0"),
            "a real build must ask for ITS OWN tag: {err}"
        );
    }
}
