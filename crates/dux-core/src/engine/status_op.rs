//! The tri-state status object. A pending status cannot be constructed without
//! its success and failure outcomes (enforced by the typestate builder), so a
//! "loading" status that never resolves is inexpressible.
//!
//! Construct one at the dispatch site with [`status_op`], declaring the pending
//! message and then both outcome closures. Hand it to
//! [`Engine::spawn_status_op`](crate::engine::Engine::spawn_status_op): the
//! pending [`StatusTone::Busy`] shows immediately, the work runs off-thread, and
//! the matching closure resolves the [`Final`] *where the typed result is in
//! scope*, shipping back only the plain [`ResolvedFinal`] data. The engine turns
//! that into the keyed final (or a clear) so the pending status is always
//! replaced.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;

/// Process-global source of opaque status ids. Monotonic only so each op gets a
/// distinct correlation handle; the value carries no meaning and consumers never
/// read or construct it.
static NEXT_STATUS_ID: AtomicU64 = AtomicU64::new(1);

/// Mint a fresh opaque correlation id for one status operation. Used internally
/// by [`status_op`]; not part of the consumer-facing API.
fn next_status_id() -> String {
    format!("op-{}", NEXT_STATUS_ID.fetch_add(1, Ordering::Relaxed))
}

/// What replaces a pending status when its operation finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Final {
    /// Replace the spinner with a transient success / persistent failure line.
    Message { tone: StatusTone, text: String },
    /// Deliberately dismiss the pending status with NO replacement message.
    /// Reads in review as "no final message needed here, empty is fine".
    Clear,
}

impl Final {
    pub fn info(text: impl Into<String>) -> Self {
        Final::Message {
            tone: StatusTone::Info,
            text: text.into(),
        }
    }
    pub fn warning(text: impl Into<String>) -> Self {
        Final::Message {
            tone: StatusTone::Warning,
            text: text.into(),
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Final::Message {
            tone: StatusTone::Error,
            text: text.into(),
        }
    }
    pub fn clear() -> Self {
        Final::Clear
    }
}

/// The resolved outcome shipped back from a worker thread: the operation's key
/// plus the [`Final`] produced by running the matching success/failure closure
/// where the typed result was in scope. Plain data so it crosses the worker
/// channel without closures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFinal {
    pub key: String,
    pub outcome: Final,
}

impl ResolvedFinal {
    pub fn new(key: impl Into<String>, outcome: Final) -> Self {
        Self {
            key: key.into(),
            outcome,
        }
    }

    /// Panic fallback used by `spawn_status_op` when the work closure unwinds:
    /// the success/failure closures never ran, so synthesise a keyed error.
    pub fn error(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            outcome: Final::error(text),
        }
    }

    /// Translate the carried outcome into the engine reaction that applies it.
    pub fn into_reaction(self) -> crate::engine::EventReaction {
        use crate::engine::EventReaction;
        match self.outcome {
            Final::Message { tone, text } => EventReaction::Status(StatusUpdate {
                tone,
                message: text,
                key: Some(self.key),
            }),
            Final::Clear => EventReaction::ClearStatus(self.key),
        }
    }
}

/// Entry point: a pending message, awaiting its success closure. An opaque
/// correlation id is minted internally, so the caller never authors or sees a
/// key — the busy and its final correlate purely by sharing this object. You
/// cannot obtain a [`StatusOp`] without passing through `on_success` then
/// `on_failure`, so both outcomes are always declared.
pub fn status_op(pending: impl Into<String>) -> NeedsSuccess {
    NeedsSuccess {
        key: next_status_id(),
        pending: pending.into(),
    }
}

pub struct NeedsSuccess {
    key: String,
    pending: String,
}

impl NeedsSuccess {
    pub fn on_success<T, F>(self, f: F) -> NeedsFailure<T>
    where
        F: FnOnce(&T) -> Final + Send + 'static,
    {
        NeedsFailure {
            key: self.key,
            pending: self.pending,
            on_success: Box::new(f),
            _t: PhantomData,
        }
    }

    /// Alternative to `on_success`/`on_failure` for operations whose final is
    /// decided LATER, in the completion handler, from an outcome the worker
    /// can't see (post-worker fallible state, a 3-way result, render context).
    /// The single closure is declared HERE (so the outcome is still mandatory at
    /// dispatch) but receives a handler-computed `Outcome` and runs where that
    /// outcome exists. The op is correlated to its pending by the opaque id.
    pub fn resolve_in_handler<O, F>(self, f: F) -> HandlerStatusOp<O>
    where
        F: FnOnce(&O) -> Final + Send + 'static,
    {
        HandlerStatusOp {
            key: self.key,
            pending: self.pending,
            resolver: Box::new(f),
        }
    }
}

/// A status op whose [`Final`] is produced in the completion handler from a
/// handler-computed `Outcome` (see [`NeedsSuccess::resolve_in_handler`]). The
/// dispatch site emits [`Self::pending_status`] and stashes the op keyed by
/// [`Self::id`]; the handler retrieves it and calls [`Self::resolve`].
pub struct HandlerStatusOp<O> {
    key: String,
    pending: String,
    resolver: Box<dyn FnOnce(&O) -> Final + Send>,
}

impl<O> HandlerStatusOp<O> {
    pub fn id(&self) -> &str {
        &self.key
    }

    /// The keyed [`StatusTone::Busy`] to show while the operation runs.
    pub fn pending_status(&self) -> StatusUpdate {
        StatusUpdate::busy(self.pending.clone()).with_key(self.key.clone())
    }

    /// An UPDATED keyed busy on the same id, for operations that report progress
    /// mid-flight (e.g. agent creation streaming "Creating worktree…", "Launching
    /// session…"). Does not consume the op — the eventual [`Self::resolve`] still
    /// replaces it. Replaces the old hand-keyed progress re-emit.
    pub fn progress(&self, message: impl Into<String>) -> StatusUpdate {
        StatusUpdate::busy(message).with_key(self.key.clone())
    }

    /// Run the resolver against the handler-computed outcome, returning the
    /// keyed final.
    pub fn resolve(self, outcome: &O) -> ResolvedFinal {
        ResolvedFinal::new(self.key, (self.resolver)(outcome))
    }
}

pub struct NeedsFailure<T> {
    key: String,
    pending: String,
    on_success: Box<dyn FnOnce(&T) -> Final + Send>,
    _t: PhantomData<fn(&T)>,
}

impl<T> NeedsFailure<T> {
    pub fn on_failure<E, F>(self, f: F) -> StatusOp<T, E>
    where
        F: FnOnce(&E) -> Final + Send + 'static,
    {
        StatusOp {
            key: self.key,
            pending: self.pending,
            on_success: self.on_success,
            on_failure: Box::new(f),
        }
    }
}

/// A fully-specified tri-state status. Carries its key, pending text, and the
/// two outcome closures. Resolve it where the typed `Result` is in scope.
pub struct StatusOp<T, E> {
    key: String,
    pending: String,
    on_success: Box<dyn FnOnce(&T) -> Final + Send>,
    on_failure: Box<dyn FnOnce(&E) -> Final + Send>,
}

impl<T, E> StatusOp<T, E> {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The keyed [`StatusTone::Busy`] to show while the operation runs.
    pub fn pending_status(&self) -> StatusUpdate {
        StatusUpdate::busy(self.pending.clone()).with_key(self.key.clone())
    }

    /// Run the matching closure for the outcome and return the keyed result.
    pub fn resolve(self, result: &Result<T, E>) -> ResolvedFinal {
        let outcome = match result {
            Ok(t) => (self.on_success)(t),
            Err(e) => (self.on_failure)(e),
        };
        ResolvedFinal::new(self.key, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_constructors_carry_tone_and_text() {
        assert_eq!(
            Final::info("ok"),
            Final::Message {
                tone: StatusTone::Info,
                text: "ok".into()
            }
        );
        assert_eq!(
            Final::error("bad"),
            Final::Message {
                tone: StatusTone::Error,
                text: "bad".into()
            }
        );
        assert_eq!(
            Final::warning("hmm"),
            Final::Message {
                tone: StatusTone::Warning,
                text: "hmm".into()
            }
        );
        assert_eq!(Final::clear(), Final::Clear);
    }

    #[test]
    fn resolved_final_error_builds_a_keyed_error_message() {
        let r = ResolvedFinal::error("push:/a", "boom");
        assert_eq!(r.key, "push:/a");
        assert_eq!(r.outcome, Final::error("boom"));
    }

    #[test]
    fn status_op_resolves_success_and_failure_with_its_own_id() {
        let op = status_op("Pushing\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
            .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
        // The id is opaque and minted internally; the pending and the resolved
        // final must share it without the caller ever naming it.
        let id = op.key().to_string();
        let pending = op.pending_status();
        assert_eq!(pending.tone, StatusTone::Busy);
        assert_eq!(pending.key.as_deref(), Some(id.as_str()));
        let resolved = op.resolve(&Ok::<u32, String>(3));
        assert_eq!(
            resolved,
            ResolvedFinal::new(&id, Final::info("Pushed 3 commits."))
        );

        // Distinct op gets a distinct id; failure branch resolves on its own id.
        let op = status_op("Pushing\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
            .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
        let id2 = op.key().to_string();
        assert_ne!(id, id2, "each op mints a fresh id");
        let resolved = op.resolve(&Err::<u32, String>("nope".into()));
        assert_eq!(
            resolved,
            ResolvedFinal::new(&id2, Final::error("Push failed: nope"))
        );
    }

    #[test]
    fn spawn_status_op_emits_pending_then_resolves_via_worker() {
        use crate::engine::EventReaction;
        let (mut engine, _tmp) = crate::engine::test_support::test_engine();
        let op = status_op("Working\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Did {n}.")))
            .on_failure(|e: &String| Final::error(e.clone()));
        let id = op.key().to_string();
        let pending = engine.spawn_status_op(op, || Ok::<u32, String>(2));
        match pending {
            EventReaction::Status(s) => {
                assert_eq!(s.tone, StatusTone::Busy);
                assert_eq!(s.key.as_deref(), Some(id.as_str()));
            }
            _ => panic!("expected a pending Busy Status"),
        }
        // The worker runs on a thread; block briefly for its completion event.
        let ev = engine.worker_rx.recv().expect("completion event");
        match engine.process_worker_event(ev) {
            EventReaction::Status(s) => {
                assert_eq!(s.key.as_deref(), Some(id.as_str()));
                assert_eq!(s.message, "Did 2.");
            }
            _ => panic!("expected a resolved keyed Status"),
        }
    }

    #[test]
    fn handler_status_op_resolves_an_n_way_outcome_in_the_handler() {
        // The outcome (3-way here) is decided in the handler, not the worker.
        enum Outcome {
            Ok,
            DbFail(String),
            ConfigFail(String),
        }
        let build = || {
            status_op("Saving\u{2026}").resolve_in_handler(|o: &Outcome| match o {
                Outcome::Ok => Final::info("Saved."),
                Outcome::DbFail(e) => Final::error(format!("DB failed: {e}")),
                Outcome::ConfigFail(e) => {
                    Final::warning(format!("Saved to DB, config failed: {e}"))
                }
            })
        };
        // The pending carries the op's own opaque id.
        let op = build();
        let id = op.id().to_string();
        let pending = op.pending_status();
        assert_eq!(pending.tone, StatusTone::Busy);
        assert_eq!(pending.key.as_deref(), Some(id.as_str()));

        // Each handler-decided outcome resolves through the single closure
        // (resolve consumes the op, so build a fresh one per branch).
        assert_eq!(build().resolve(&Outcome::Ok).outcome, Final::info("Saved."));
        assert_eq!(
            build().resolve(&Outcome::DbFail("locked".into())).outcome,
            Final::error("DB failed: locked")
        );
        assert_eq!(
            build().resolve(&Outcome::ConfigFail("disk".into())).outcome,
            Final::warning("Saved to DB, config failed: disk")
        );
    }

    #[test]
    fn handler_status_op_progress_reuses_the_id_without_consuming() {
        let op = status_op("Creating worktree\u{2026}").resolve_in_handler(|_: &()| Final::clear());
        let id = op.id().to_string();
        // Progress updates re-emit a busy on the SAME id and don't consume the op.
        let p1 = op.progress("Launching session\u{2026}");
        assert_eq!(p1.tone, StatusTone::Busy);
        assert_eq!(p1.key.as_deref(), Some(id.as_str()));
        assert_eq!(p1.message, "Launching session\u{2026}");
        let p2 = op.progress("Almost there\u{2026}");
        assert_eq!(p2.key.as_deref(), Some(id.as_str()));
        // Still resolvable afterward (op was not consumed by progress).
        let resolved = op.resolve(&());
        assert_eq!(resolved.key, id);
        assert_eq!(resolved.outcome, Final::Clear);
    }

    #[test]
    fn resolved_final_into_reaction_maps_message_and_clear() {
        use crate::engine::EventReaction;
        match ResolvedFinal::new("k", Final::info("done")).into_reaction() {
            EventReaction::Status(s) => {
                assert_eq!(s.key.as_deref(), Some("k"));
                assert_eq!(s.tone, StatusTone::Info);
                assert_eq!(s.message, "done");
            }
            _ => panic!("expected a Status reaction"),
        }
        match ResolvedFinal::new("k", Final::clear()).into_reaction() {
            EventReaction::ClearStatus(k) => assert_eq!(k, "k"),
            _ => panic!("expected a ClearStatus reaction"),
        }
    }
}
