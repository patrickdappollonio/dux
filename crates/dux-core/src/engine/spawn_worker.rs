//! `Engine::spawn_command_worker` — the unified spawn primitive for
//! command-side workers. Owns in-flight marking, busy-status FIFO delivery
//! through the worker channel, and panic recovery via a synthesised
//! completion event.
//!
//! Background and rationale live in
//! `docs/superpowers/specs/2026-05-31-engine-spawn-worker-primitive.md`.

use std::panic::AssertUnwindSafe;
use std::sync::mpsc::Sender;
use std::thread;

use crate::engine::events::EventReaction;
use crate::engine::{Engine, InFlightKey, StatusUpdate};
use crate::worker::WorkerEvent;

/// Specification for a single command-worker spawn. Constructed by the
/// `Command::*` arm that wants the worker, consumed by
/// `Engine::spawn_command_worker`.
pub struct CommandWorkerSpec {
    /// Short human-readable label. Used as a thread-name suffix and as the
    /// log prefix on any panic.
    pub label: String,
    /// `Some` when the command guards re-entry through `in_flight`. The
    /// primitive marks the key before spawning and the worker's normal
    /// completion-event handler is responsible for clearing it.
    pub in_flight_key: Option<InFlightKey>,
    /// Status to enqueue on the worker channel before the worker thread
    /// starts. Travels through the same FIFO channel as the worker's
    /// completion event so the busy status cannot be overwritten by an
    /// out-of-order arrival.
    pub busy_status: Option<StatusUpdate>,
    /// Reaction returned to the caller when `in_flight_key` is already in
    /// flight. `None` falls back to a generic "<label> is already running."
    /// warning.
    pub already_running_status: Option<StatusUpdate>,
    /// Builds the completion event posted when the worker thread panics. The
    /// event must be one that `process_worker_event` already knows how to
    /// route through the same path it would route a normal failure for this
    /// command — that path is what clears the in-flight key. `None` means
    /// the panic is logged but no event is synthesised, in which case the
    /// in-flight key (if any) will not be cleared automatically; only use
    /// `None` when the site has no in-flight key.
    pub panic_event: Option<Box<dyn FnOnce(String) -> WorkerEvent + Send>>,
}

// TODO: synchronous spawn-failure test pending #[cfg(test)] hook — the
// failure path is exercised in production but not yet covered by a unit
// test because exercising it cleanly would require a test-only hook into
// `thread::Builder::spawn`.

/// Format a `Box<dyn Any + Send>` panic payload as a human-readable
/// string, matching the `&str` / `String` cases stdlib normally surfaces
/// through the default panic hook.
pub(crate) fn format_panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

impl Engine {
    /// Spawn a command-side worker with unified in-flight, busy-status, and
    /// panic safety. See `CommandWorkerSpec` for the per-site fields.
    ///
    /// Returns:
    /// - `EventReaction::Status(already_running)` when the in-flight key was
    ///   already present and the worker was not spawned.
    /// - `EventReaction::Nothing` on the happy path — status flows through
    ///   `worker_tx`.
    /// - `EventReaction::Status(error)` when `thread::Builder::spawn`
    ///   returns `Err` (rare; PID / RLIMIT exhaustion). The in-flight key is
    ///   cleared in this case so a retry can proceed.
    pub fn spawn_command_worker<F>(&mut self, spec: CommandWorkerSpec, job: F) -> EventReaction
    where
        F: FnOnce(Sender<WorkerEvent>) + Send + 'static,
    {
        // 1. In-flight guard.
        if let Some(ref key) = spec.in_flight_key
            && self.is_in_flight(key)
        {
            let status = spec.already_running_status.unwrap_or_else(|| {
                StatusUpdate::warning(format!("{} is already running.", spec.label))
            });
            return EventReaction::Status(status);
        }
        if let Some(ref key) = spec.in_flight_key {
            self.mark_in_flight(key.clone());
        }

        // 2. Post the busy status BEFORE spawning so it is strictly ahead of
        //    any event the worker could send. mpsc preserves FIFO order, so
        //    `process_worker_event` sees busy → completion regardless of how
        //    fast the worker runs.
        let worker_tx = self.worker_tx.clone();
        if let Some(busy) = spec.busy_status {
            let _ = worker_tx.send(WorkerEvent::CommandWorkerStarted(busy));
        }

        // 3. Spawn with catch_unwind. On panic, log and post the synthesised
        //    completion event so the existing handler clears the in-flight
        //    key through the same path it would for a normal failure.
        let label = spec.label.clone();
        let panic_event = spec.panic_event;
        let key_for_panic = spec.in_flight_key.clone();
        let tx_for_job = worker_tx.clone();
        let label_for_thread = label.clone();
        let label_for_log = label.clone();

        let spawn_result = thread::Builder::new()
            .name(format!("dux-cmd-{label_for_thread}"))
            .spawn(move || {
                // AssertUnwindSafe: the job's captured state is owned by
                // this thread and is not shared with the main engine. A
                // panic strands at most that owned state; the in-flight
                // key it left set is restored by the synthesised
                // completion event posted below, which `drain_events`
                // routes through the existing failure handler.
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    job(tx_for_job);
                }));
                if let Err(payload) = result {
                    let reason = format_panic_payload(payload);
                    crate::logger::error(&format!(
                        "spawn_command_worker[{label_for_log}] panicked: {reason}",
                    ));
                    if let Some(builder) = panic_event {
                        let _ = worker_tx.send(builder(reason));
                    } else if let Some(key) = key_for_panic {
                        crate::logger::error(&format!(
                            "spawn_command_worker[{label_for_log}] has no panic_event; in-flight key {key:?} will not be cleared automatically",
                        ));
                    }
                }
            });

        match spawn_result {
            Ok(_) => EventReaction::Nothing,
            Err(err) => {
                if let Some(key) = &spec.in_flight_key {
                    self.clear_in_flight(key);
                }
                let msg = format!("Could not start background worker '{label}': {err}");
                crate::logger::error(&msg);
                EventReaction::Status(StatusUpdate::error(msg))
            }
        }
    }
}
