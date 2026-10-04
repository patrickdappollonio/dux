//! One ordered, off-thread, atomic config writer per process.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::config_write::{Durability, save_config_three_way};
use crate::worker::WorkerEvent;

const QUIET_WINDOW: Duration = Duration::from_millis(250);
const EAGER_TIMEOUT: Duration = Duration::from_secs(2);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const LAZY_INFLIGHT_CAP: usize = 128;

enum WriteMsg {
    Lazy(Config),
    Eager {
        config: Config,
        reply: SyncSender<Result<(), String>>,
    },
    Flush(SyncSender<()>),
    Pause(SyncSender<()>),
    Resume,
    /// The config as dux last read it from disk (startup, a reload), the
    /// base every following save is a three-way patch against.
    SetBase(Box<Config>),
    /// Stop the writer thread unconditionally, obeyed even while paused. Sent by
    /// `Drop` so shutdown never depends on channel disconnect (a `QuiesceGuard`
    /// holds a sender clone, so the channel can stay connected) or on guard drop
    /// order.
    Shutdown,
}

pub struct ConfigWriteQueue {
    tx: Sender<WriteMsg>,
    writer: Option<JoinHandle<()>>,
    lazy_inflight: Arc<AtomicUsize>,
}

/// Holds a reload/recover barrier open. The writer is paused (drained) while the
/// guard lives; dropping it resumes the writer. Owns a `Sender<WriteMsg>` clone
/// (not a borrow) so it can be stored on `Engine` as `reload_guard`.
pub struct QuiesceGuard {
    tx: Sender<WriteMsg>,
    /// `true` when the writer explicitly acknowledged the `Pause` message (the
    /// happy path); `false` on timeout or a dead writer. The guard ALWAYS sends
    /// `Resume` on drop regardless of this flag: callers MUST NOT suppress it
    /// (a slow-but-alive writer will eventually process the `Pause` and needs
    /// the matching `Resume` to unblock).
    acknowledged: bool,
}

impl QuiesceGuard {
    /// Returns `true` when the writer acknowledged the pause request within the
    /// timeout. When `false` (timeout or dead writer) the barrier is NOT safe: a
    /// direct config write can race a still-running writer. Callers should abort
    /// the write and surface a retry error instead.
    pub fn is_acknowledged(&self) -> bool {
        self.acknowledged
    }
}

impl Drop for QuiesceGuard {
    fn drop(&mut self) {
        // Resume is sent in ALL cases, even when `acknowledged` is false: a
        // slow-but-alive writer will eventually process the Pause and would
        // block forever waiting for the Resume if we skipped it here.
        let _ = self.tx.send(WriteMsg::Resume);
    }
}

impl ConfigWriteQueue {
    /// A queue whose failures are only logged. For tests and for callers with
    /// no engine behind them; every production site uses
    /// [`Self::with_status_lane`] instead, because a preference that silently
    /// failed to save is exactly the kind of thing nobody reads a log about.
    pub fn new(config_path: PathBuf) -> Self {
        Self::build(config_path, None)
    }

    /// A queue that reports a failed deferred write on the engine's worker
    /// lane, so whichever surface is draining says the preference was not
    /// saved.
    pub fn with_status_lane(config_path: PathBuf, status_lane: Sender<WorkerEvent>) -> Self {
        Self::build(config_path, Some(status_lane))
    }

    fn build(config_path: PathBuf, status_lane: Option<Sender<WorkerEvent>>) -> Self {
        let (tx, rx) = mpsc::channel();
        let lazy_inflight = Arc::new(AtomicUsize::new(0));
        // Read here, before the thread starts, so the base is the file as it
        // was when the queue was made. A file that cannot be loaded leaves no
        // base, and saves fall back to the full patch.
        let base = crate::config::load_config_file(&config_path).ok();
        let writer = thread::Builder::new()
            .name("config-writer".into())
            .spawn({
                let lazy_inflight = lazy_inflight.clone();
                move || writer_loop(rx, config_path, base, lazy_inflight, status_lane)
            })
            .expect("spawn config-writer thread");
        ConfigWriteQueue {
            tx,
            writer: Some(writer),
            lazy_inflight,
        }
    }

    /// Deferred, coalesced, fire-and-forget. A dead writer is surfaced lazily via
    /// the next eager/flush; lazy itself never blocks.
    pub fn save_lazy(&self, config: Config) {
        // Bound in-flight lazy snapshots so a stalled or paused writer cannot let
        // the channel grow without limit. Lazy writes are coalesced anyway, so
        // dropping a snapshot at the cap is acceptable: the fixed deadline still
        // lands a write. The reservation is a single atomic update (not a separate
        // load-then-add) so concurrent callers cannot overshoot the cap.
        if self
            .lazy_inflight
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < LAZY_INFLIGHT_CAP).then_some(n + 1)
            })
            .is_err()
        {
            return;
        }
        if self.tx.send(WriteMsg::Lazy(config)).is_err() {
            decr_inflight(&self.lazy_inflight);
        }
    }

    /// Awaited write: blocks (a few ms) for the result, ~2 s timeout. On a dead
    /// writer it returns an error rather than hanging.
    pub fn save_eager(&self, config: Config) -> Result<(), String> {
        let (reply, rx) = mpsc::sync_channel(1);
        if self.tx.send(WriteMsg::Eager { config, reply }).is_err() {
            return Err("config writer thread is gone; config was not saved".into());
        }
        match rx.recv_timeout(EAGER_TIMEOUT) {
            Ok(res) => res,
            Err(RecvTimeoutError::Disconnected) => {
                Err("config writer thread died; config was not saved".into())
            }
            Err(RecvTimeoutError::Timeout) => {
                if self
                    .writer
                    .as_ref()
                    .map(|w| w.is_finished())
                    .unwrap_or(true)
                {
                    Err("config writer thread died; config was not saved".into())
                } else {
                    Err(
                        "config write timed out (disk may be stalled); config may not be saved"
                            .into(),
                    )
                }
            }
        }
    }

    /// Tell the writer what the file held when dux last read it (a reload just
    /// adopted it). Every later save writes only the keys memory changed
    /// since, so a key someone else set on disk is never undone. Ordered with
    /// the saves on the same channel.
    pub fn set_base(&self, config: Config) {
        let _ = self.tx.send(WriteMsg::SetBase(Box::new(config)));
    }

    /// Exit-time drain: write any pending lazy, bounded by a timeout.
    pub fn flush(&self) {
        let (ack, rx) = mpsc::sync_channel(0);
        if self.tx.send(WriteMsg::Flush(ack)).is_ok() {
            match rx.recv_timeout(FLUSH_TIMEOUT) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => crate::logger::error(
                    "config flush timed out (disk may be stalled); a pending write may be lost",
                ),
                Err(RecvTimeoutError::Disconnected) => crate::logger::error(
                    "config writer thread died before flush; a pending write may be lost",
                ),
            }
        }
    }

    /// Begin a reload/recover barrier: drain pending + pause the writer, returning
    /// a guard that resumes on drop. The caller does its own write synchronous-direct
    /// while holding the guard.
    ///
    /// Check [`QuiesceGuard::is_acknowledged`] before performing the direct write:
    /// if `false`, the writer never confirmed the pause (timeout or dead writer) and
    /// the write must be aborted to avoid racing a still-running writer.
    pub fn quiesce(&self) -> QuiesceGuard {
        let (ack, rx) = mpsc::sync_channel(0);
        let acknowledged = if self.tx.send(WriteMsg::Pause(ack)).is_ok() {
            match rx.recv_timeout(FLUSH_TIMEOUT) {
                Ok(()) => true,
                Err(RecvTimeoutError::Timeout) => {
                    crate::logger::error(
                        "config writer did not acknowledge pause within timeout; direct write aborted to avoid racing the stalled writer",
                    );
                    false
                }
                Err(RecvTimeoutError::Disconnected) => {
                    crate::logger::error(
                        "config writer thread is gone; direct write aborted (no active write barrier)",
                    );
                    false
                }
            }
        } else {
            crate::logger::error(
                "config writer thread is gone; direct write aborted (no active write barrier)",
            );
            false
        };
        QuiesceGuard {
            tx: self.tx.clone(),
            acknowledged,
        }
    }

    /// Test-only: a queue whose writer thread has already exited, so `save_eager`
    /// deterministically hits the dead-writer path.
    #[cfg(test)]
    pub fn with_dead_writer(config_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<WriteMsg>();
        drop(rx); // receiver gone → the writer is effectively dead
        let _ = config_path;
        ConfigWriteQueue {
            tx,
            writer: None,
            lazy_inflight: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Drop for ConfigWriteQueue {
    fn drop(&mut self) {
        // Flush pending lazy writes on clean process exit. This fires when the
        // engine, and so the queue, is finally dropped at real exit. It does NOT
        // fire at the in-process TUI-to-web flip, which MOVES the engine rather
        // than dropping it, so the queue keeps running across the flip.
        //
        // Step 1: drain any pending lazy write queued before the current state.
        // flush() is bounded by FLUSH_TIMEOUT so Drop cannot hang. Lazies that
        // arrived during an open reload barrier were intentionally discarded as
        // stale snapshots, so only writes from before the barrier land here.
        self.flush();

        // Step 2: tell the writer to exit.  We cannot rely on channel disconnect:
        // an outstanding `QuiesceGuard` holds a clone of the sender, so dropping
        // our own `tx` would not disconnect the channel, and a paused writer would
        // wait forever for a `Resume` that only arrives when the guard drops
        // (which, on `Engine`, happens AFTER the queue).  `Shutdown` is obeyed even
        // while paused, so shutdown is independent of guard lifetime/drop order.
        let _ = self.tx.send(WriteMsg::Shutdown);

        // Step 3: join the writer thread with a bounded timeout. An unconditional
        // join() would hang the process on exit if the writer is stuck in a
        // stalled disk write, so a helper thread joins and signals on a
        // rendezvous channel; past FLUSH_TIMEOUT both threads are abandoned,
        // which is safe because the process is exiting. A pending write may be
        // lost on a truly stalled disk.
        if let Some(handle) = self.writer.take() {
            let (done_tx, done_rx) = mpsc::sync_channel::<()>(0);
            thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
            match done_rx.recv_timeout(FLUSH_TIMEOUT) {
                Ok(()) => {} // writer exited cleanly
                Err(_) => {
                    crate::logger::error(
                        "config writer did not exit within timeout on shutdown; abandoning the thread (a pending write may be lost)",
                    );
                    // The helper thread and writer thread are leaked; safe because
                    // the process is already exiting.
                }
            }
        }
    }
}

/// Decrement the in-flight counter without ever wrapping past zero. A concurrent
/// panic-reset (`store(0)`) could otherwise race a send-failure rollback and wrap
/// the counter to `usize::MAX`, latching the cap gate shut forever.
fn decr_inflight(counter: &AtomicUsize) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_sub(1))
    });
}

fn writer_loop(
    rx: Receiver<WriteMsg>,
    path: PathBuf,
    base: Option<Config>,
    lazy_inflight: Arc<AtomicUsize>,
    status_lane: Option<Sender<WorkerEvent>>,
) {
    // Clone the counter before moving the original into the inner loop, so the
    // panic handler below still has a handle to reset it after the loop exits.
    let counter = lazy_inflight.clone();
    // Note: under panic = "abort" this guard is inert (the process aborts); it is active under the default unwind strategy.
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer_loop_inner(rx, path, base, lazy_inflight, status_lane)
    })) {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        crate::logger::error(&format!("config-writer thread panicked: {msg}"));
        // A panic can leave received-but-not-decremented Lazy messages counted.
        // Reset so save_lazy's cap gate can't latch shut and silently drop every
        // future lazy write: with the writer gone, save_lazy will then reach the
        // (failing) send and surface the dead-writer state like the other ops do.
        counter.store(0, Ordering::Relaxed);
    }
}

struct WriterDisconnected;

enum WriterControl {
    Continue,
    Stop,
}

fn writer_loop_inner(
    rx: Receiver<WriteMsg>,
    path: PathBuf,
    base: Option<Config>,
    lazy_inflight: Arc<AtomicUsize>,
    status_lane: Option<Sender<WorkerEvent>>,
) {
    let mut state = WriterState {
        base,
        pending: None,
        deadline: None,
    };

    loop {
        let input = receive_writer_input(&rx, state.deadline);
        if matches!(
            handle_writer_input(
                input,
                &rx,
                &path,
                &lazy_inflight,
                &mut state,
                status_lane.as_ref(),
            ),
            WriterControl::Stop
        ) {
            break;
        }
    }
}

fn receive_writer_input(
    rx: &Receiver<WriteMsg>,
    deadline: Option<Instant>,
) -> Result<Option<WriteMsg>, WriterDisconnected> {
    match deadline {
        Some(deadline) => {
            let wait = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(message) => Ok(Some(message)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(WriterDisconnected),
            }
        }
        None => match rx.recv() {
            Ok(message) => Ok(Some(message)),
            Err(_) => Err(WriterDisconnected),
        },
    }
}

/// What the writer thread carries between messages: the base its three-way
/// saves patch against, and the coalesced lazy save waiting for its deadline.
struct WriterState {
    base: Option<Config>,
    pending: Option<Config>,
    deadline: Option<Instant>,
}

fn handle_writer_input(
    input: Result<Option<WriteMsg>, WriterDisconnected>,
    rx: &Receiver<WriteMsg>,
    path: &std::path::Path,
    lazy_inflight: &AtomicUsize,
    state: &mut WriterState,
    status_lane: Option<&Sender<WorkerEvent>>,
) -> WriterControl {
    match input {
        Ok(None) => flush_pending(path, state, status_lane),
        Err(WriterDisconnected) => return WriterControl::Stop,
        Ok(Some(WriteMsg::Lazy(config))) => {
            decr_inflight(lazy_inflight);
            state.pending = Some(config);
            if state.deadline.is_none() {
                state.deadline = Some(Instant::now() + QUIET_WINDOW);
            }
        }
        Ok(Some(WriteMsg::Eager { config, reply })) => {
            state.pending = None;
            state.deadline = None;
            let result =
                save_config_three_way(path, state.base.as_ref(), &config, Durability::Fsync)
                    .map_err(|e| format!("{e:#}"));
            match &result {
                Ok(()) => state.base = Some(config),
                Err(error) => crate::logger::error(&format!("eager config write failed: {error}")),
            }
            let _ = reply.send(result);
        }
        Ok(Some(WriteMsg::Flush(ack))) => {
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
        }
        Ok(Some(WriteMsg::Pause(ack))) => {
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
            debug_assert!(state.pending.is_none());
            if !run_paused_writer(rx, lazy_inflight, &mut state.base) {
                return WriterControl::Stop;
            }
        }
        Ok(Some(WriteMsg::Resume)) => {}
        Ok(Some(WriteMsg::SetBase(config))) => state.base = Some(*config),
        Ok(Some(WriteMsg::Shutdown)) => {
            flush_pending(path, state, status_lane);
            return WriterControl::Stop;
        }
    }
    WriterControl::Continue
}

fn run_paused_writer(
    rx: &Receiver<WriteMsg>,
    lazy_inflight: &AtomicUsize,
    base: &mut Option<Config>,
) -> bool {
    let mut depth = 1usize;
    loop {
        match rx.recv() {
            Ok(WriteMsg::Resume) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return true;
                }
            }
            Ok(WriteMsg::Shutdown) | Err(_) => return false,
            Ok(WriteMsg::Lazy(_)) => decr_inflight(lazy_inflight),
            Ok(WriteMsg::Eager { reply, .. }) => {
                let _ = reply.send(Err("config busy (reload in progress); retry".into()));
            }
            Ok(WriteMsg::Flush(ack)) => {
                let _ = ack.send(());
            }
            Ok(WriteMsg::Pause(ack)) => {
                depth = depth.saturating_add(1);
                let _ = ack.send(());
            }
            Ok(WriteMsg::SetBase(config)) => *base = Some(*config),
        }
    }
}

fn flush_pending(
    path: &std::path::Path,
    state: &mut WriterState,
    status_lane: Option<&Sender<WorkerEvent>>,
) {
    state.deadline = None;
    let Some(cfg) = state.pending.take() else {
        return;
    };
    let result = save_config_three_way(path, state.base.as_ref(), &cfg, Durability::NoFsync);
    if result.is_ok() {
        state.base = Some(cfg);
    }
    if let Err(e) = result {
        crate::logger::error(&format!("lazy config write failed: {e:#}"));
        // Nothing asked for this write and nothing is waiting on its answer, so
        // the only sign of the failure is the preference reverting the next time
        // dux starts. Say so while the user is still here to fix it.
        if let Some(lane) = status_lane {
            let _ = lane.send(WorkerEvent::PollerStatus(
                crate::config_reload_status::lazy_write_failed(&format!("{e:#}")),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn read(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The reviewer's repro: `dux config set` changes a key on disk, an
    /// unrelated lazy save is still pending in the running dux, the reload's
    /// quiesce flushes it, and the set value must survive into the reload.
    #[test]
    fn a_pending_lazy_save_never_undoes_a_key_set_on_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\ncopy_on_select = true\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        let loaded: Config = toml::from_str(&read(&path)).unwrap();
        q.set_base(loaded.clone());

        // `dux config set ui.left_width_pct 33`, from another process.
        let key = crate::config_keys::lookup("ui.left_width_pct").unwrap();
        crate::config_keys::set_plain(&path, &key, "33").unwrap();

        let mut memory = loaded;
        memory.ui.copy_on_select = false;
        q.save_lazy(memory);
        drop(q.quiesce());
        q.flush();

        let after: Config = toml::from_str(&read(&path)).unwrap();
        assert_eq!(after.ui.left_width_pct, 33, "{}", read(&path));
        assert!(!after.ui.copy_on_select);
    }

    /// With no base set yet, the writer reads one from the file when it starts.
    #[test]
    fn the_writer_takes_its_first_base_from_the_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        let memory: Config = toml::from_str(&read(&path)).unwrap();
        std::fs::write(&path, "[ui]\nleft_width_pct = 44\n").unwrap();
        q.save_eager(memory).unwrap();
        assert!(
            read(&path).contains("left_width_pct = 44"),
            "{}",
            read(&path)
        );
    }

    #[test]
    fn eager_save_persists_and_returns_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // Seed an existing file so the patch path is exercised.
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());

        let mut cfg = Config::default();
        cfg.env.insert("FOO".into(), "bar".into());
        q.save_eager(cfg).expect("eager ok");

        assert!(read(&path).contains("FOO = \"bar\""));
    }

    /// Nobody waits on a lazy write, so a failure has no caller to report to:
    /// without this the only sign is the preference reverting at the next start.
    #[test]
    fn a_failed_lazy_write_reports_on_the_status_lane() {
        let (tx, rx) = mpsc::channel();
        let q = ConfigWriteQueue::with_status_lane("/nonexistent/dir/config.toml".into(), tx);

        q.save_lazy(Config::default());
        q.flush();

        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a status on the lane");
        let WorkerEvent::PollerStatus(status) = event else {
            panic!("a failed deferred write rides the poller-status lane");
        };
        assert_eq!(status.tone, crate::statusline::StatusTone::Warning);
        assert!(status.message.contains("gone after a restart"));
    }

    #[test]
    fn lazy_burst_coalesces_to_latest_within_deadline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());

        for i in 0..50 {
            let mut cfg = Config::default();
            cfg.env.insert("N".into(), i.to_string());
            q.save_lazy(cfg);
        }
        // Flush forces the pending write to land deterministically.
        q.flush();
        assert!(
            read(&path).contains("N = \"49\""),
            "latest lazy must win: {}",
            read(&path)
        );
    }

    #[test]
    fn save_eager_after_writer_gone_errors_not_hangs() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let q = ConfigWriteQueue::with_dead_writer(path);
        let err = q.save_eager(Config::default()).unwrap_err();
        assert!(err.to_lowercase().contains("writer"), "got: {err}");
    }

    #[test]
    fn sustained_lazy_burst_still_lands_via_fixed_deadline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        // Arrivals closer than the window for ~1s; the fixed deadline must fire a write.
        let start = std::time::Instant::now();
        let mut i = 0;
        while start.elapsed() < Duration::from_millis(900) {
            let mut cfg = Config::default();
            cfg.env.insert("N".into(), i.to_string());
            q.save_lazy(cfg);
            i += 1;
            std::thread::sleep(Duration::from_millis(50));
        }
        // Within ~one window after the first arrival a write should already exist.
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("N = "),
            "deadline never fired"
        );
        q.flush();
    }

    #[test]
    fn lazy_during_barrier_is_dropped() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\nKEEP = \"recovered\"\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());

        {
            let _guard = q.quiesce(); // writer drained + paused
            // Operation's own write, synchronous-direct, while paused:
            let mut recovered = Config::default();
            recovered.env.insert("KEEP".into(), "recovered".into());
            crate::config_write::save_config_with(
                &path,
                &recovered,
                crate::config_write::Durability::Fsync,
            )
            .unwrap();
            // A concurrent stale lazy arrives during the barrier:
            let mut stale = Config::default();
            stale.env.insert("KEEP".into(), "stale".into());
            q.save_lazy(stale);
        } // guard drops → resume

        q.flush();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("KEEP = \"recovered\""),
            "stale lazy clobbered the barrier write: {saved}"
        );
    }

    #[test]
    fn eager_during_barrier_gets_retry_not_stale_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        let guard = q.quiesce();
        let err = q.save_eager(Config::default()).unwrap_err();
        drop(guard);
        assert!(
            err.to_lowercase().contains("retry") || err.to_lowercase().contains("busy"),
            "got: {err}"
        );
    }

    /// Dropping the queue while a `QuiesceGuard` is still alive (a reload barrier
    /// left open, e.g. an `Engine` torn down mid-reload) must NOT deadlock. The
    /// guard holds a clone of the writer's channel sender, so the channel never
    /// disconnects; the paused writer must be stopped by an explicit shutdown
    /// signal, independent of guard drop order. The drop runs on a worker thread
    /// and signals completion so a regression times out here instead of hanging
    /// the whole suite.
    #[test]
    fn drop_with_open_barrier_does_not_deadlock() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();

        let q = ConfigWriteQueue::new(path);
        // Open the barrier and keep the guard alive PAST the queue's drop.
        let guard = q.quiesce();

        let (done_tx, done_rx) = mpsc::sync_channel::<()>(1);
        let h = thread::spawn(move || {
            drop(q);
            let _ = done_tx.send(());
        });

        match done_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(()) => h.join().unwrap(),
            Err(_) => {
                // On a real regression drop(q) is genuinely deadlocked, so the spawned
                // thread cannot be joined (that would re-hang); we abandon it and fail.
                panic!("ConfigWriteQueue::drop deadlocked while a QuiesceGuard was still alive")
            }
        }
        // The guard outlives the queue; dropping it now sends Resume to a writer
        // that is already gone, a harmless no-op (mirrors Engine field order).
        drop(guard);
    }

    /// Proves that a pending lazy write is flushed when the queue is dropped
    /// (i.e. on clean process exit), not lost.  Without the `Drop` impl this
    /// test is RED: the pending lazy sits in the channel and the writer loop
    /// exits without writing it.  With the `Drop` impl it is GREEN.
    #[test]
    fn lazy_pending_is_flushed_on_drop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();

        {
            let q = ConfigWriteQueue::new(path.clone());
            let mut cfg = Config::default();
            cfg.env.insert("DROP_MARKER".into(), "flushed".into());
            q.save_lazy(cfg);
            // Drop q here: the Drop impl must flush the pending lazy write.
        }

        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            saved.contains("DROP_MARKER = \"flushed\""),
            "pending lazy write was NOT flushed on drop: {saved}"
        );
    }

    /// A lazy queued before a barrier opens must survive the quiesce-then-drop
    /// sequence (quiesce flushes it on pause entry; the drop must not lose it).
    #[test]
    fn lazy_before_barrier_is_flushed_on_drop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();

        let saved = {
            let q = ConfigWriteQueue::new(path.clone());
            let mut cfg = Config::default();
            cfg.env.insert("PRE_BARRIER".into(), "kept".into());
            q.save_lazy(cfg); // queued before the barrier
            let _guard = q.quiesce(); // drains the pre-barrier lazy, then pauses
            drop(q); // guard still alive → paused-writer drop path
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        assert!(
            saved.contains("PRE_BARRIER = \"kept\""),
            "pre-barrier lazy was not flushed on drop: {saved}"
        );
    }

    // ── Fix 1: QuiesceGuard::is_acknowledged ───────────────────────────────

    /// A normal `quiesce()` call returns a guard with `is_acknowledged() == true`.
    #[test]
    fn quiesce_acknowledged_on_live_writer() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path);
        let guard = q.quiesce();
        assert!(
            guard.is_acknowledged(),
            "live writer must acknowledge the pause"
        );
    }

    /// A `quiesce()` on a queue whose writer has already exited returns a guard
    /// with `is_acknowledged() == false`: the barrier is not effective.
    #[test]
    fn quiesce_not_acknowledged_on_dead_writer() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let q = ConfigWriteQueue::with_dead_writer(path);
        let guard = q.quiesce();
        assert!(
            !guard.is_acknowledged(),
            "dead writer must not report acknowledgement"
        );
    }

    // ── Fix 2: bounded join in Drop ────────────────────────────────────────

    /// Dropping a live queue completes in well under FLUSH_TIMEOUT, proving the
    /// bounded join does not break the happy path.
    #[test]
    fn drop_completes_quickly_on_clean_writer() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path);
        let start = std::time::Instant::now();
        drop(q);
        // The bounded join should complete well within the FLUSH_TIMEOUT (2 s).
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "drop took too long: {:?}",
            start.elapsed()
        );
    }

    /// Lazies sent during an open barrier are discarded by the paused writer, and
    /// their in-flight count must be decremented so the cap gate re-opens: a
    /// post-barrier lazy must still land.
    #[test]
    fn inflight_counter_drains_during_barrier_and_gate_reopens() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());

        {
            let _guard = q.quiesce(); // writer drained + paused
            // Send a burst during the barrier; all are discarded by the paused writer.
            for i in 0..(LAZY_INFLIGHT_CAP * 2) {
                let mut cfg = Config::default();
                cfg.env.insert("N".into(), i.to_string());
                q.save_lazy(cfg);
            }
        } // guard drops → resume

        // The writer drains the discarded burst asynchronously after resuming, so
        // the counter returns below the cap without a fixed timing guarantee. Poll
        // (bounded) until a fresh lazy lands, proving the gate re-opens once the
        // backlog drains, without depending on drain timing.
        let mut landed = false;
        for _ in 0..200 {
            let mut cfg = Config::default();
            cfg.env.insert("AFTER".into(), "barrier".into());
            q.save_lazy(cfg);
            q.flush();
            if std::fs::read_to_string(&path)
                .unwrap_or_default()
                .contains("AFTER = \"barrier\"")
            {
                landed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(landed, "cap gate did not re-open after the barrier drained");
    }
}
