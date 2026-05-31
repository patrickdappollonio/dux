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

/// Specification for a single one-shot background-worker spawn. Used by
/// `Engine::spawn_background_worker`, which has no caller-facing reaction
/// (background work is fire-and-forget) and no busy-status delivery
/// (background workers run silently). Panic safety still applies — see
/// `panic_event`.
pub struct BackgroundWorkerSpec {
    /// Short human-readable label. Used as a thread-name suffix and as the
    /// log prefix on any panic.
    pub label: String,
    /// Most background workers have no in-flight tracking. The option exists
    /// for the few that legitimately need single-instance semantics.
    pub in_flight_key: Option<InFlightKey>,
    /// Posted on `worker_tx` if the worker thread panics, so
    /// `process_worker_event` can clear the in-flight key through its
    /// existing failure handler. `None` means a panic is logged but no event
    /// is synthesised — appropriate for workers whose completion event has
    /// no failure variant (or no completion event at all).
    pub panic_event: Option<Box<dyn FnOnce(String) -> WorkerEvent + Send>>,
}

impl Engine {
    /// Spawn a one-shot background worker with panic safety and optional
    /// in-flight tracking. See `BackgroundWorkerSpec` for the per-site
    /// fields.
    ///
    /// Unlike `spawn_command_worker`, this primitive returns `()` because
    /// background work is fire-and-forget: there is no caller-side
    /// `EventReaction` to apply. A synchronous spawn failure is logged and
    /// the in-flight key (if any) is cleared so a future retry can proceed.
    pub fn spawn_background_worker<F>(&mut self, spec: BackgroundWorkerSpec, job: F)
    where
        F: FnOnce(Sender<WorkerEvent>) + Send + 'static,
    {
        // 1. In-flight guard. Background workers silently skip when the key
        //    is already present — they have no caller to surface a warning to.
        if let Some(ref key) = spec.in_flight_key
            && self.is_in_flight(key)
        {
            crate::logger::debug(&format!(
                "spawn_background_worker[{}] skipped: {key:?} already in flight",
                spec.label,
            ));
            return;
        }
        if let Some(ref key) = spec.in_flight_key {
            self.mark_in_flight(key.clone());
        }

        // 2. Spawn with catch_unwind. On panic, log and post the synthesised
        //    completion event (if any) so the existing handler clears the
        //    in-flight key through the same path it would for a normal
        //    failure.
        let worker_tx = self.worker_tx.clone();
        let label = spec.label.clone();
        let panic_event = spec.panic_event;
        let key_for_panic = spec.in_flight_key.clone();
        let label_for_thread = label.clone();
        let label_for_log = label.clone();
        let tx_for_job = worker_tx.clone();

        let spawn_result = thread::Builder::new()
            .name(format!("dux-bg-{label_for_thread}"))
            .spawn(move || {
                // AssertUnwindSafe: same rationale as `spawn_command_worker`.
                // The job's captured state is owned by this thread; any
                // in-flight key it left set is restored by the synthesised
                // completion event posted below.
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    job(tx_for_job);
                }));
                if let Err(payload) = result {
                    let reason = format_panic_payload(payload);
                    crate::logger::error(&format!(
                        "spawn_background_worker[{label_for_log}] panicked: {reason}",
                    ));
                    if let Some(builder) = panic_event {
                        let _ = worker_tx.send(builder(reason));
                    } else if let Some(key) = key_for_panic {
                        crate::logger::error(&format!(
                            "spawn_background_worker[{label_for_log}] has no panic_event; in-flight key {key:?} will not be cleared automatically",
                        ));
                    }
                }
            });

        if let Err(err) = spawn_result {
            if let Some(key) = &spec.in_flight_key {
                self.clear_in_flight(key);
            }
            crate::logger::error(&format!(
                "spawn_background_worker[{label}] failed to spawn thread: {err}",
            ));
        }
    }
}

/// Specification for a single long-running loop-worker spawn. Used by
/// `Engine::spawn_loop_worker`, which owns the outer loop and per-iteration
/// panic recovery. The body closure runs once per loop tick and decides
/// whether the loop continues or exits.
pub struct LoopWorkerSpec {
    /// Short human-readable label. Used as a thread-name suffix and as the
    /// log prefix on any per-iteration panic.
    pub label: String,
}

/// Per-iteration return value for a `spawn_loop_worker` body. `Continue`
/// keeps the watcher running for another iteration; `Break` exits the loop
/// (typically because the receiver was dropped or a kill switch fired).
pub enum LoopControl {
    Continue,
    Break,
}

impl Engine {
    /// Spawn a long-running loop worker that survives per-iteration panics.
    ///
    /// The primitive owns the outer `loop`: each iteration runs `body(&tx)`
    /// inside `catch_unwind`. On `Ok(Continue)` the loop runs again; on
    /// `Ok(Break)` it exits; on a caught panic the primitive logs at `error`
    /// level (so repeated panics surface in `dux.log`) and continues — one
    /// bad iteration must not kill the watcher.
    ///
    /// Takes `&self`, not `&mut self`, because loop workers do not touch
    /// in-flight state and callers commonly spawn them at bootstrap.
    ///
    /// `catch_unwind` runs once per iteration; with the in-tree loop
    /// intervals measured in seconds (2s, 10s, 45s) the overhead is
    /// negligible and not worth optimising.
    pub fn spawn_loop_worker<F>(&self, spec: LoopWorkerSpec, mut body: F)
    where
        F: FnMut(&Sender<WorkerEvent>) -> LoopControl + Send + 'static,
    {
        let worker_tx = self.worker_tx.clone();
        let label = spec.label;
        let label_for_thread = label.clone();

        let spawn_result = thread::Builder::new()
            .name(format!("dux-loop-{label_for_thread}"))
            .spawn(move || {
                loop {
                    // AssertUnwindSafe: the body's captured state is owned by
                    // this thread and is not shared with the main engine. A
                    // panic strands at most that owned state; we log and run
                    // the next iteration so a transient bad tick cannot kill
                    // the watcher.
                    let result =
                        std::panic::catch_unwind(AssertUnwindSafe(|| body(&worker_tx)));
                    match result {
                        Ok(LoopControl::Continue) => continue,
                        Ok(LoopControl::Break) => break,
                        Err(payload) => {
                            let reason = format_panic_payload(payload);
                            crate::logger::error(&format!(
                                "spawn_loop_worker[{label}] iteration panicked, continuing: {reason}",
                            ));
                        }
                    }
                }
            });

        if let Err(err) = spawn_result {
            crate::logger::error(&format!(
                "spawn_loop_worker[{label_for_thread}] failed to spawn thread: {err}",
            ));
        }
    }
}
