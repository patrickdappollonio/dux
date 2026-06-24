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

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;

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

/// Entry point: a key + pending message, awaiting its success closure. You
/// cannot obtain a [`StatusOp`] without passing through `on_success` then
/// `on_failure`, so both outcomes are always declared.
pub fn status_op(key: impl Into<String>, pending: impl Into<String>) -> NeedsSuccess {
    NeedsSuccess {
        key: key.into(),
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
    fn status_op_resolves_success_and_failure_with_its_key() {
        let op = status_op("push:/a", "Pushing\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
            .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
        assert_eq!(op.key(), "push:/a");
        let pending = op.pending_status();
        assert_eq!(pending.tone, StatusTone::Busy);
        assert_eq!(pending.key.as_deref(), Some("push:/a"));
        let resolved = op.resolve(&Ok::<u32, String>(3));
        assert_eq!(
            resolved,
            ResolvedFinal::new("push:/a", Final::info("Pushed 3 commits."))
        );

        // Fresh op (resolve consumes self) for the failure branch.
        let op = status_op("push:/a", "Pushing\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Pushed {n} commits.")))
            .on_failure(|e: &String| Final::error(format!("Push failed: {e}")));
        let resolved = op.resolve(&Err::<u32, String>("nope".into()));
        assert_eq!(
            resolved,
            ResolvedFinal::new("push:/a", Final::error("Push failed: nope"))
        );
    }

    #[test]
    fn spawn_status_op_emits_pending_then_resolves_via_worker() {
        use crate::engine::EventReaction;
        let (mut engine, _tmp) = crate::engine::test_support::test_engine();
        let op = status_op("op:1", "Working\u{2026}")
            .on_success(|n: &u32| Final::info(format!("Did {n}.")))
            .on_failure(|e: &String| Final::error(e.clone()));
        let pending = engine.spawn_status_op(op, || Ok::<u32, String>(2));
        match pending {
            EventReaction::Status(s) => {
                assert_eq!(s.tone, StatusTone::Busy);
                assert_eq!(s.key.as_deref(), Some("op:1"));
            }
            _ => panic!("expected a pending Busy Status"),
        }
        // The worker runs on a thread; block briefly for its completion event.
        let ev = engine.worker_rx.recv().expect("completion event");
        match engine.process_worker_event(ev) {
            EventReaction::Status(s) => {
                assert_eq!(s.key.as_deref(), Some("op:1"));
                assert_eq!(s.message, "Did 2.");
            }
            _ => panic!("expected a resolved keyed Status"),
        }
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
