//! `Engine::spawn_command_worker`, the unified spawn primitive for command-side
//! workers. Owns in-flight marking, busy-status FIFO delivery through the worker
//! channel, and panic recovery via a synthesised completion event.

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
    /// primitive marks the key before spawning; the worker's own
    /// completion-event handler must clear it.
    pub in_flight_key: Option<InFlightKey>,
    /// Status to enqueue on the worker channel before the worker thread starts.
    /// It travels through the same FIFO channel as the worker's completion
    /// event, so an out-of-order arrival cannot overwrite it.
    pub busy_status: Option<StatusUpdate>,
    /// Reaction returned to the caller when `in_flight_key` is already in
    /// flight. `None` falls back to a generic warning naming the label.
    pub already_running_status: Option<StatusUpdate>,
    /// Builds the completion event posted when the worker thread panics. It must
    /// be one `process_worker_event` routes through this command's normal
    /// failure path, which is what clears the in-flight key. `None` logs the
    /// panic and synthesises nothing, so use it only where there is no
    /// in-flight key to clear.
    pub panic_event: Option<Box<dyn FnOnce(String) -> WorkerEvent + Send>>,
}

/// Format a `Box<dyn Any + Send>` panic payload as a human-readable string,
/// matching the `&str` and `String` cases the default panic hook surfaces.
pub fn format_panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
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
    /// - `EventReaction::Nothing` on the happy path, since status flows through
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

        // 2. Post the busy status before spawning so it is strictly ahead of any
        //    event the worker could send: mpsc is FIFO, so `process_worker_event`
        //    sees busy then completion however fast the worker runs.
        let worker_tx = self.worker_tx.clone();
        let mut busy_key = None;
        if let Some(mut busy) = spec.busy_status {
            // Stamp the command origin so a web operation's busy reaches only
            // the originating connection; `current_origin` is `All` elsewhere.
            busy.scope = self.current_origin.clone();
            // A keyed busy from here is an operation the engine is waiting on,
            // so recording it live makes the status controller heartbeat it
            // rather than call it timed out. Registering here covers every keyed
            // caller of the primitive, so no call site has to remember.
            if let Some(key) = busy.key.clone() {
                self.register_status_key(&key);
                busy_key = Some(key);
            }
            let _ = worker_tx.send(WorkerEvent::CommandWorkerStarted(busy));
        }

        // 3. Spawn with catch_unwind. On panic, log and post the synthesised
        //    completion event so the existing handler clears the in-flight key
        //    through the path it would take for a normal failure.
        let label = spec.label.clone();
        let panic_event = spec.panic_event;
        let key_for_panic = spec.in_flight_key.clone();
        let tx_for_job = worker_tx.clone();
        let label_for_thread = label.clone();
        let label_for_log = label.clone();

        let spawn_result = thread::Builder::new()
            .name(format!("dux-cmd-{label_for_thread}"))
            .spawn(move || {
                // AssertUnwindSafe: the job's captured state is owned by this
                // thread, not shared with the engine, so a panic strands at
                // most that state and the synthesised completion event below
                // clears any in-flight key it left set.
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
                let Some(key) = busy_key else {
                    return EventReaction::Status(StatusUpdate::error(msg));
                };
                // The busy is already on the channel and no worker will answer
                // it, so the error must land on its key and the op must be
                // abandoned; otherwise liveness heartbeats that spinner for the
                // life of the process.
                self.abandon_status_op(&key);
                EventReaction::Status(StatusUpdate::error(msg).with_key(key))
            }
        }
    }

    /// Give up on the operation behind a status key: retire its liveness
    /// registration and drop any pending op stashed under it.
    ///
    /// For the paths that abandon an operation before it can produce a final. An
    /// operation that ends normally needs none of this: its final retires
    /// liveness through the status controller and its handler consumes the op.
    ///
    /// The sweep covers only the registries keyed by the status key; a key
    /// cannot address the ones keyed by session id, and missing them costs a
    /// leaked op struct rather than anything the user sees.
    pub fn abandon_status_op(&mut self, key: &str) {
        self.retire_status_key(key);
        self.pending_create_ops.remove(key);
        self.pending_web_checkout_ops.remove(key);
        self.pending_web_add_project_ops.remove(key);
        self.pending_web_pr_lookup_ops.remove(key);
        self.pending_pr_attach_ops.remove(key);
    }

    /// Dispatch a keyed tri-state operation: emit its pending Busy, run `work`
    /// off-thread, resolve the success/failure closure where the typed result
    /// is in scope, and ship the keyed final back via `StatusOpCompleted`. The
    /// returned reaction is the pending Busy to apply now. This is the
    /// sanctioned way to show a pending status: a `StatusOp` cannot be built
    /// without both outcome closures, so a spinner always has a resolution.
    pub fn spawn_status_op<T, E, F>(
        &mut self,
        op: crate::engine::StatusOp<T, E>,
        work: F,
    ) -> EventReaction
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        // Stamp the command origin onto the pending busy and capture it for the
        // deferred final: `current_origin` is reset by the time the worker
        // completes, so the scope must travel on the `ResolvedFinal`.
        let origin = self.current_origin.clone();
        let pending = op.pending_status().with_scope(origin.clone());
        let key_for_spawn_fail = op.key().to_string();
        let key_for_panic = key_for_spawn_fail.clone();
        let tx = self.worker_tx.clone();
        // Every `spawn_status_op` is an operation the engine is waiting on, so
        // its spinner is heartbeated rather than timed out. These ops have no
        // registry of their own, so liveness cannot come from enumerating them.
        self.register_status_key(&key_for_spawn_fail);

        let spawn_result = thread::Builder::new()
            .name("dux-status-op".into())
            .spawn(move || {
                let resolved = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let result = work();
                    op.resolve(&result)
                })) {
                    Ok(r) => r,
                    Err(payload) => {
                        let reason = format_panic_payload(payload);
                        crate::logger::error(&format!("status-op worker panicked: {reason}"));
                        crate::engine::ResolvedFinal::error(
                            key_for_panic,
                            format!("Worker panicked: {reason}"),
                        )
                    }
                }
                .with_scope(origin);
                let _ = tx.send(WorkerEvent::StatusOpCompleted { resolved });
            });

        match spawn_result {
            // Apply the pending Busy now; the worker will follow with its final.
            Ok(_) => EventReaction::Status(pending),
            Err(err) => {
                // Spawn failed and the Busy rides the returned reaction being
                // replaced here, so a keyed error must go out in its place and
                // take the liveness registration back with it.
                let msg = format!("Could not start background worker: {err}");
                crate::logger::error(&msg);
                self.retire_status_key(&key_for_spawn_fail);
                EventReaction::Status(StatusUpdate::error(msg).with_key(key_for_spawn_fail))
            }
        }
    }
}

/// Outcome of a `spawn_background_worker` call. Background work is otherwise
/// fire-and-forget, but a synchronous spawn failure fires no completion event,
/// so the caller needs this signal to unwind the optimistic state it set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundSpawn {
    /// The worker thread started; its completion or synthesised panic event
    /// will follow through `worker_tx`.
    Spawned,
    /// Skipped because the in-flight key was already present. This is the
    /// primitive's defensive backstop, unreachable on a path that guards
    /// re-entry itself before calling in. No event fires.
    AlreadyInFlight,
    /// `thread::Builder::spawn` failed synchronously (PID or RLIMIT
    /// exhaustion). The in-flight key was cleared so a retry can proceed, but
    /// no completion event fires: the caller must unwind its optimistic state.
    SpawnFailed,
}

/// Specification for a single one-shot background-worker spawn. Used by
/// `Engine::spawn_background_worker`, which returns a coarse
/// [`BackgroundSpawn`] outcome and has no busy-status delivery, because
/// background workers run silently. Panic safety still applies; see
/// `panic_event`.
pub struct BackgroundWorkerSpec {
    /// Short human-readable label. Used as a thread-name suffix and as the
    /// log prefix on any panic.
    pub label: String,
    /// Most background workers have no in-flight tracking. The option exists
    /// for the few that legitimately need single-instance semantics.
    pub in_flight_key: Option<InFlightKey>,
    /// Posted on `worker_tx` if the worker thread panics, so
    /// `process_worker_event` can clear the in-flight key through its existing
    /// failure handler. `None` logs the panic and synthesises nothing, for
    /// workers whose completion event has no failure variant, or none at all.
    pub panic_event: Option<Box<dyn FnOnce(String) -> WorkerEvent + Send>>,
}

impl Engine {
    /// Spawn a one-shot background worker with panic safety and optional
    /// in-flight tracking. See `BackgroundWorkerSpec` for the per-site
    /// fields.
    ///
    /// There is no caller-side `EventReaction` to apply, but the coarse
    /// [`BackgroundSpawn`] outcome lets a caller unwind optimistic state on a
    /// synchronous spawn failure, which is logged and clears the in-flight key
    /// so a retry can proceed.
    pub fn spawn_background_worker<F>(
        &mut self,
        spec: BackgroundWorkerSpec,
        job: F,
    ) -> BackgroundSpawn
    where
        F: FnOnce(Sender<WorkerEvent>) + Send + 'static,
    {
        // 1. In-flight guard, a defensive backstop only: the user-facing
        //    re-entry guard belongs at the call site, which surfaces an error
        //    before calling in. A background worker has no caller to warn, so
        //    this only logs.
        if let Some(ref key) = spec.in_flight_key
            && self.is_in_flight(key)
        {
            crate::logger::debug(&format!(
                "spawn_background_worker[{}] skipped: {key:?} already in flight",
                spec.label,
            ));
            return BackgroundSpawn::AlreadyInFlight;
        }
        if let Some(ref key) = spec.in_flight_key {
            self.mark_in_flight(key.clone());
        }

        // Test-only: take the same exit a synchronous spawn failure takes,
        // without exhausting the machine's process table to provoke a real one.
        // No completion event fires, so callers recover from this path
        // themselves and it has to be reachable from a test.
        #[cfg(test)]
        if std::mem::take(&mut self.force_worker_spawn_failure) {
            if let Some(key) = &spec.in_flight_key {
                self.clear_in_flight(key);
            }
            crate::logger::error(&format!(
                "spawn_background_worker[{}] failed to spawn thread: injected test failure",
                spec.label,
            ));
            return BackgroundSpawn::SpawnFailed;
        }

        // 2. Spawn with catch_unwind. On panic, log and post the synthesised
        //    completion event so the existing handler clears the in-flight key
        //    through the path it would take for a normal failure.
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
                // AssertUnwindSafe: the job's captured state is owned by this
                // thread, and the synthesised completion event below clears any
                // in-flight key a panic left set.
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
            return BackgroundSpawn::SpawnFailed;
        }
        BackgroundSpawn::Spawned
    }
}

/// Specification for a single long-running loop-worker spawn. Used by
/// `Engine::spawn_loop_worker`, which owns the outer loop and per-iteration
/// panic recovery; the body runs once per tick and decides whether to continue.
pub struct LoopWorkerSpec {
    /// Short human-readable label. Used as a thread-name suffix and as the
    /// log prefix on any per-iteration panic.
    pub label: String,
}

/// Per-iteration return value for a `spawn_loop_worker` body. `Continue` runs
/// another iteration; `Break` exits the loop.
pub enum LoopControl {
    Continue,
    Break,
}

impl Engine {
    /// Spawn a long-running loop worker that survives per-iteration panics.
    ///
    /// The primitive owns the outer `loop`: each iteration runs `body(&tx)`
    /// inside `catch_unwind`. `Ok(Continue)` runs again, `Ok(Break)` exits, and
    /// a caught panic is logged at `error` level and then continues, because one
    /// bad iteration must not kill the watcher.
    ///
    /// Takes `&self`, not `&mut self`, because loop workers do not touch
    /// in-flight state and callers commonly spawn them at bootstrap.
    ///
    /// Returns whether the thread actually started. Most callers ignore it,
    /// since a watcher that cannot start is logged and nothing more, but a
    /// caller holding a single-instance slot for the loop must release it, or
    /// the loop can never be started again.
    pub fn spawn_loop_worker<F>(&self, spec: LoopWorkerSpec, mut body: F) -> bool
    where
        F: FnMut(&Sender<WorkerEvent>) -> LoopControl + Send + 'static,
    {
        let worker_tx = self.worker_tx.clone();
        let label = spec.label;
        let label_for_thread = label.clone();

        // Test-only: take the same exit a synchronous spawn failure takes,
        // without exhausting the machine's process table to provoke a real one.
        // A caller holding a single-instance slot must release it on `false`,
        // so that recovery has to be reachable from a test.
        #[cfg(test)]
        if self
            .force_loop_worker_spawn_failure
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            crate::logger::error(&format!(
                "spawn_loop_worker[{label}] failed to spawn thread: injected test failure",
            ));
            return false;
        }

        let spawn_result = thread::Builder::new()
            .name(format!("dux-loop-{label_for_thread}"))
            .spawn(move || {
                loop {
                    // AssertUnwindSafe: the body's captured state is owned by
                    // this thread, not shared with the engine, so a panic
                    // strands at most that state and the next iteration runs.
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
            return false;
        }
        true
    }
}
