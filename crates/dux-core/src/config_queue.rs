//! One ordered, off-thread, atomic config writer per process.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::config_write::{Durability, SaveBase, save_config_three_way, union_seen};
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
    /// The config dux just adopted (a reload), carrying the exact text it was
    /// read from: the base every following save is a three-way patch against.
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
    last_written: LastWritten,
}

/// The writer's base after its most recent SUCCESSFUL write (the config it
/// wrote and the text it has seen), with a count of writes, so a caller can
/// tell whether a write happened since it last looked.
type LastWritten = Arc<std::sync::Mutex<(u64, Option<crate::config::SourceText>)>>;

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
    ///
    /// It has no base until [`Self::set_base`] gives it one, so its saves are
    /// the full patch.
    pub fn new(config_path: PathBuf) -> Self {
        Self::build(config_path, None, None)
    }

    /// A queue whose base is `loaded`, the config dux read: the exact text
    /// that config was read from ([`Config::source_text`]), never a fresh
    /// read, so a `dux config set` landing between the load and this call is
    /// not taken for part of what dux read.
    pub fn with_base(config_path: PathBuf, loaded: &Config) -> Self {
        Self::build(config_path, None, base_of_loaded(loaded))
    }

    /// A queue that reports a failed deferred write on the engine's worker
    /// lane, so whichever surface is draining says the preference was not
    /// saved. Its base is `loaded`, as for [`Self::with_base`].
    pub fn with_status_lane(
        config_path: PathBuf,
        status_lane: Sender<WorkerEvent>,
        loaded: &Config,
    ) -> Self {
        Self::build(config_path, Some(status_lane), base_of_loaded(loaded))
    }

    fn build(
        config_path: PathBuf,
        status_lane: Option<Sender<WorkerEvent>>,
        base: Option<Base>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let lazy_inflight = Arc::new(AtomicUsize::new(0));
        let last_written: LastWritten = Arc::default();
        let writer = thread::Builder::new()
            .name("config-writer".into())
            .spawn({
                let lazy_inflight = lazy_inflight.clone();
                let last_written = last_written.clone();
                move || {
                    writer_loop(
                        rx,
                        config_path,
                        base,
                        lazy_inflight,
                        status_lane,
                        last_written,
                    )
                }
            })
            .expect("spawn config-writer thread");
        ConfigWriteQueue {
            tx,
            writer: Some(writer),
            lazy_inflight,
            last_written,
        }
    }

    /// How many writes this writer has made, and its base after the latest
    /// one that succeeded, as a [`crate::config::SourceText`]: the text it
    /// has seen and the config it wrote. The engine reads it around a
    /// reload's deferred commands: when they wrote, that is what the file
    /// agrees with, not the reloaded text. A save that failed changes
    /// neither, so its change still counts as one a later save must write.
    pub fn last_written(&self) -> (u64, Option<crate::config::SourceText>) {
        self.last_written
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or((0, None))
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

    /// Tell the writer about the config dux just adopted (a reload). Its
    /// base becomes that config's own text ([`Config::source_text`]), so
    /// every later save writes only what memory changes relative to the
    /// file as it was read, and a key someone else set or deleted on disk is
    /// never undone. Ordered with the saves on the same channel.
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
            last_written: Arc::default(),
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
    base: Option<Base>,
    lazy_inflight: Arc<AtomicUsize>,
    status_lane: Option<Sender<WorkerEvent>>,
    last_written: LastWritten,
) {
    // Clone the counter before moving the original into the inner loop, so the
    // panic handler below still has a handle to reset it after the loop exits.
    let counter = lazy_inflight.clone();
    // Note: under panic = "abort" this guard is inert (the process aborts); it is active under the default unwind strategy.
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer_loop_inner(rx, path, base, lazy_inflight, status_lane, last_written)
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
    base: Option<Base>,
    lazy_inflight: Arc<AtomicUsize>,
    status_lane: Option<Sender<WorkerEvent>>,
    last_written: LastWritten,
) {
    let mut state = WriterState {
        base,
        pending: None,
        deadline: None,
        last_written,
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

/// What every save is a three-way patch against (see [`SaveBase`]): the
/// config the file last agreed with, and the file text seen since the last
/// read.
struct Base {
    config: Config,
    seen: String,
}

impl Base {
    fn as_save_base(&self) -> SaveBase<'_> {
        SaveBase {
            config: &self.config,
            seen: Some(&self.seen),
        }
    }

    /// After writing `config` as `written`.
    fn after_write(&self, config: Config, written: &str) -> Base {
        Base {
            seen: union_seen(Some(&self.seen), written),
            config,
        }
    }
}

/// The base for a file just read: its config as written, and its text.
fn base_read_from(text: &str) -> Option<Base> {
    let config = crate::config::config_from_text_as_written(text).ok()?;
    Some(Base {
        config,
        seen: text.to_string(),
    })
}

/// The base a config's own text gives: for a config read from it, the
/// config parsed from that text as written; for a config dux wrote, that
/// config itself (a re-parse could fold in another writer's change), with
/// the text as what has been seen.
fn base_from_source(config: &Config) -> Option<Base> {
    let text = config.source_text.as_str()?;
    if let Some(written) = config.source_text.written_base() {
        return Some(Base {
            config: written.clone(),
            seen: text.to_string(),
        });
    }
    base_read_from(text)
}

/// The first base: the loaded config's own text, or no base at all for a
/// config read from no file (its saves are then the full patch).
fn base_of_loaded(loaded: &Config) -> Option<Base> {
    base_from_source(loaded)
}

/// The base for a config a reload adopted: the text it was read from, or,
/// for a config read from no file, that config and the file as it is now.
fn base_adopted(path: &std::path::Path, config: Config) -> Option<Base> {
    if let Some(base) = base_from_source(&config) {
        return Some(base);
    }
    Some(Base {
        seen: std::fs::read_to_string(path).unwrap_or_default(),
        config,
    })
}

/// What the writer thread carries between messages: the base its three-way
/// saves patch against, and the coalesced lazy save waiting for its deadline.
struct WriterState {
    base: Option<Base>,
    pending: Option<Config>,
    deadline: Option<Instant>,
    last_written: LastWritten,
}

impl WriterState {
    /// A save wrote `config` as `written`: the next base, and the text the
    /// engine can ask for.
    fn record_write(&mut self, config: Config, written: &str) {
        let base = written_base(&self.base, config, written);
        if let Ok(mut last) = self.last_written.lock() {
            *last = (
                last.0 + 1,
                Some(crate::config::SourceText::written(
                    &base.seen,
                    base.config.clone(),
                )),
            );
        }
        self.base = Some(base);
    }
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
            let result = save_against_base(path, &state.base, &config, Durability::Fsync)
                .map_err(|e| format!("{e:#}"));
            match &result {
                Ok(text) => state.record_write(config, text),
                Err(error) => crate::logger::error(&format!("eager config write failed: {error}")),
            }
            let _ = reply.send(result.map(|_| ()));
        }
        Ok(Some(WriteMsg::Flush(ack))) => {
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
        }
        Ok(Some(WriteMsg::Pause(ack))) => {
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
            debug_assert!(state.pending.is_none());
            if !run_paused_writer(rx, lazy_inflight, path, &mut state.base) {
                return WriterControl::Stop;
            }
        }
        Ok(Some(WriteMsg::Resume)) => {}
        Ok(Some(WriteMsg::SetBase(config))) => state.base = base_adopted(path, *config),
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
    path: &std::path::Path,
    base: &mut Option<Base>,
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
            Ok(WriteMsg::SetBase(config)) => *base = base_adopted(path, *config),
        }
    }
}

/// One three-way save against the writer's base; the text written comes back
/// to become the next base.
fn save_against_base(
    path: &std::path::Path,
    base: &Option<Base>,
    config: &Config,
    durability: Durability,
) -> anyhow::Result<String> {
    save_config_three_way(
        path,
        base.as_ref().map(Base::as_save_base),
        config,
        durability,
    )
}

/// The base after a save wrote `config` as `written`.
fn written_base(base: &Option<Base>, config: Config, written: &str) -> Base {
    match base {
        Some(base) => base.after_write(config, written),
        None => Base {
            config,
            seen: written.to_string(),
        },
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
    let result = save_against_base(path, &state.base, &cfg, Durability::NoFsync);
    let result = match result {
        Ok(text) => {
            state.record_write(cfg, &text);
            Ok(())
        }
        Err(error) => Err(error),
    };
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
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);

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

    /// A value dux corrected at load (an out-of-range font size) is written
    /// back by the next save: the writer's first base is the file as written,
    /// so the correction counts as a change.
    #[test]
    fn a_load_time_correction_reaches_the_file_on_the_next_save() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nterminal_font_size = 500\n").unwrap();
        let memory = crate::config::load_config_file(&path).expect("load");
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        assert_ne!(memory.ui.terminal_font_size, 500, "corrected in memory");
        q.save_eager(memory.clone()).unwrap();
        let after: Config = toml::from_str(&read(&path)).unwrap();
        assert_eq!(after.ui.terminal_font_size, memory.ui.terminal_font_size);
    }

    fn loaded(path: &std::path::Path) -> Config {
        crate::config::load_config_file(path).expect("load")
    }

    /// A hand deletion stays deleted across any number of saves, not only
    /// the first one after it.
    #[test]
    fn a_hand_deletion_survives_two_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\ncopy_on_select = true\n").unwrap();
        let mut memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
        memory.ui.copy_on_select = false;
        q.save_eager(memory.clone()).unwrap();
        memory.ui.right_width_pct = 30;
        q.save_eager(memory).unwrap();
        let after = read(&path);
        assert!(!after.contains("left_width_pct"), "{after}");
        assert!(after.contains("right_width_pct = 30"), "{after}");
    }

    /// Changing in dux a setting that was deleted from the file by hand
    /// writes it again: memory's own change wins over the deletion.
    #[test]
    fn a_change_in_memory_beats_a_hand_deletion_of_the_same_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let mut memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, "[ui]\n").unwrap();
        memory.ui.left_width_pct = 25;
        q.save_eager(memory.clone()).unwrap();
        memory.ui.copy_on_select = !memory.ui.copy_on_select;
        q.save_eager(memory).unwrap();
        assert!(
            read(&path).contains("left_width_pct = 25"),
            "{}",
            read(&path)
        );
    }

    /// Inside a project entry too: a field deleted by hand stays deleted
    /// across saves.
    #[test]
    fn a_field_deleted_from_a_project_by_hand_survives_two_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\nstartup_command = \"make\"\n",
        )
        .unwrap();
        let mut memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\n").unwrap();
        memory.ui.copy_on_select = !memory.ui.copy_on_select;
        q.save_eager(memory.clone()).unwrap();
        memory.ui.right_width_pct = 31;
        q.save_eager(memory).unwrap();
        let after = read(&path);
        assert!(!after.contains("startup_command = \"make\""), "{after}");
        assert_eq!(after.matches("[[projects]]").count(), 1, "{after}");
    }

    /// A project without an id deleted by hand stays deleted, even though
    /// each parse mints it a different id.
    #[test]
    fn an_id_less_project_deleted_by_hand_stays_deleted() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\npath = \"/tmp/p\"\n\n[ui]\nleft_width_pct = 20\n",
        )
        .unwrap();
        let mut memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        memory.ui.copy_on_select = !memory.ui.copy_on_select;
        q.save_eager(memory.clone()).unwrap();
        memory.ui.right_width_pct = 32;
        q.save_eager(memory).unwrap();
        assert!(!read(&path).contains("/tmp/p"), "{}", read(&path));
    }

    /// A setting deleted from the file by hand while dux runs is not put
    /// back by the next save from memory.
    #[test]
    fn the_writer_leaves_a_hand_deleted_key_deleted() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\ncopy_on_select = true\n").unwrap();
        let mut memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
        memory.ui.copy_on_select = false;
        q.save_eager(memory).unwrap();
        let after = read(&path);
        assert!(!after.contains("left_width_pct"), "{after}");
        assert!(after.contains("copy_on_select = false"), "{after}");
    }

    /// The writer's first base is the text the loaded config was read from.
    #[test]
    fn the_writer_takes_its_first_base_from_the_loaded_text() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let memory = loaded(&path);
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
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
        let q = ConfigWriteQueue::with_status_lane(
            "/nonexistent/dir/config.toml".into(),
            tx,
            &Config::default(),
        );

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

/// Adversarial review cases (fifth review), kept as regression tests.
#[cfg(test)]
mod adv_tests {
    use super::*;
    fn read(p: &std::path::Path) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }
    fn loaded(p: &std::path::Path) -> Config {
        crate::config::load_config_file(p).unwrap()
    }
    /// A writer whose base is the file as dux loaded it, as dux builds it.
    fn queue(p: &std::path::Path) -> ConfigWriteQueue {
        ConfigWriteQueue::with_base(p.to_path_buf(), &loaded(p))
    }

    #[test]
    fn adv_project_env_deletion_two_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\n\n[projects.env]\nTOK = \"secret\"\nKEEP = \"1\"\n").unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        std::fs::write(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\n\n[projects.env]\nKEEP = \"1\"\n",
        )
        .unwrap();
        for i in 0..3 {
            m.ui.right_width_pct = 30 + i;
            q.save_eager(m.clone()).unwrap();
        }
        let a = read(&path);
        println!("ENV:\n{a}");
        assert!(!a.contains("TOK"), "{a}");
    }

    #[test]
    fn adv_moved_project_and_memory_rename() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\nname = \"a\"\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        std::fs::write(
            &path,
            "[[projects]]\nid = \"p\"\npath = \"/tmp/q\"\nname = \"a\"\n",
        )
        .unwrap();
        m.projects[0].name = Some("b".into());
        q.save_eager(m.clone()).unwrap();
        m.ui.right_width_pct = 33;
        q.save_eager(m.clone()).unwrap();
        let a = read(&path);
        println!("MOVE:\n{a}");
        assert_eq!(a.matches("[[projects]]").count(), 1, "{a}");
        assert!(a.contains("/tmp/q") && a.contains("\"b\""), "{a}");
    }

    #[test]
    fn adv_idless_projects_with_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\npath = \"/tmp/a\"\n\n[[projects]]\npath = \"/tmp/b\"\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        m.ui.right_width_pct = 31;
        q.save_eager(m.clone()).unwrap();
        let s1 = read(&path);
        println!("S1:\n{s1}");
        // reload
        let mut m2 = loaded(&path);
        q.set_base(m2.clone());
        m2.ui.right_width_pct = 32;
        q.save_eager(m2.clone()).unwrap();
        // hand-add id-less project c; memory (stale m2) saves again
        let t = read(&path) + "\n[[projects]]\npath = \"/tmp/c\"\n";
        std::fs::write(&path, &t).unwrap();
        println!("BEFORE34 has c: {}", read(&path).contains("/tmp/c"));
        m2.ui.right_width_pct = 34;
        q.save_eager(m2.clone()).unwrap();
        println!("AFTER34 has c: {}", read(&path).contains("/tmp/c"));
        m2.ui.right_width_pct = 35;
        q.save_eager(m2.clone()).unwrap();
        let a = read(&path);
        println!("IDLESS:\n{a}");
        assert_eq!(a.matches("[[projects]]").count(), 3, "{a}");
        assert_eq!(a.matches("id = ").count(), 2, "{a}");
    }

    #[test]
    fn adv_memory_added_project_hand_deleted_then_memory_removes_other() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        let mut p = m.projects[0].clone();
        p.id = "n".into();
        p.path = "/tmp/n".into();
        m.projects.push(p);
        q.save_eager(m.clone()).unwrap();
        // hand delete n
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
        m.ui.right_width_pct = 31;
        q.save_eager(m.clone()).unwrap();
        m.ui.right_width_pct = 32;
        q.save_eager(m.clone()).unwrap();
        let a = read(&path);
        println!("ADDDEL:\n{a}");
        assert!(!a.contains("/tmp/n"), "{a}");
    }

    #[test]
    fn adv_set_between_two_lazy_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# top\n[ui]\n# keep me\nleft_width_pct = 20\ncopy_on_select = true\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        m.ui.copy_on_select = false;
        q.save_eager(m.clone()).unwrap();
        let key = crate::config_keys::lookup("ui.left_width_pct").unwrap();
        crate::config_keys::set_plain(&path, &key, "40").unwrap();
        m.ui.right_width_pct = 31;
        q.save_eager(m.clone()).unwrap();
        m.ui.copy_on_select = true;
        q.save_eager(m.clone()).unwrap();
        let a = read(&path);
        println!("SET:\n{a}");
        assert!(
            a.contains("left_width_pct = 40") && a.contains("# keep me"),
            "{a}"
        );
    }

    #[test]
    fn adv_whole_env_table_deleted_then_memory_adds_env() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[env]\nA = \"1\"\nB = \"2\"\n\n[ui]\nleft_width_pct = 20\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        m.env.insert("C".into(), "3".into());
        q.save_eager(m.clone()).unwrap();
        m.ui.right_width_pct = 31;
        q.save_eager(m.clone()).unwrap();
        let a = read(&path);
        println!("ENVT:\n{a}");
        assert!(a.contains("C = \"3\"") && !a.contains("A = "), "{a}");
    }

    #[test]
    fn adv_hand_added_things_survive_two_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n\n[env]\nA = \"1\"\n\n[macros]\nm1 = \"x\"\n").unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        let t = read(&path)
            .replace("[env]\nA = \"1\"\n", "[env]\nA = \"1\"\nHAND_ENV = \"2\"\n")
            .replace("m1 = \"x\"\n", "m1 = \"x\"\nhand_macro = \"y\"\n")
            + "\n[[projects]]\nid = \"c\"\npath = \"/tmp/c\"\n\n[providers.handprov]\ncommand = \"foo\"\n";
        std::fs::write(&path, &t).unwrap();
        for i in 0..3 {
            m.ui.right_width_pct = 30 + i;
            q.save_eager(m.clone()).unwrap();
            let a = read(&path);
            assert!(a.contains("/tmp/c"), "save {i}: hand project kept:\n{a}");
            assert!(a.contains("HAND_ENV"), "save {i}: hand env kept:\n{a}");
            assert!(a.contains("hand_macro"), "save {i}: hand macro kept:\n{a}");
            assert!(a.contains("handprov"), "save {i}: hand provider kept:\n{a}");
        }
    }

    #[test]
    fn adv_env_path_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\npath = \"$HOME/p\"\n\n[[projects]]\nid = \"q\"\npath = \"$HOME/q\"\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        println!(
            "mem paths: {:?}",
            m.projects
                .iter()
                .map(|p| p.path.clone())
                .collect::<Vec<_>>()
        );
        for i in 0..2 {
            m.ui.right_width_pct = 30 + i;
            q.save_eager(m.clone()).unwrap();
        }
        let a = read(&path);
        assert_eq!(a.matches("[[projects]]").count(), 2, "{a}");
    }

    /// The same with an id-less hand-added project, and after a reload.
    #[test]
    fn adv_hand_added_idless_project_survives_saves_and_a_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        let t = read(&path) + "\n[[projects]]\npath = \"/tmp/hand\"\n";
        std::fs::write(&path, &t).unwrap();
        for i in 0..3 {
            m.ui.right_width_pct = 30 + i;
            q.save_eager(m.clone()).unwrap();
            assert!(
                read(&path).contains("/tmp/hand"),
                "save {i}:\n{}",
                read(&path)
            );
        }
        // A reload: memory and base become the file as it is now (hand
        // project and all, its id minted by this read), then two saves.
        m = loaded(&path);
        q.set_base(m.clone());
        for i in 0..2 {
            m.ui.right_width_pct = 40 + i;
            q.save_eager(m.clone()).unwrap();
            assert!(
                read(&path).contains("/tmp/hand"),
                "after reload {i}:\n{}",
                read(&path)
            );
        }
    }

    /// A project dux removed, then added back to the file by hand, stays.
    #[test]
    fn adv_a_project_removed_in_dux_then_re_added_by_hand_survives() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n\n[[projects]]\nid = \"b\"\npath = \"/tmp/b\"\n",
        )
        .unwrap();
        let q = queue(&path);
        let mut m = loaded(&path);
        m.projects.retain(|p| p.id != "b");
        q.save_eager(m.clone()).unwrap();
        assert!(!read(&path).contains("/tmp/b"), "removed by dux");
        let t = read(&path) + "\n[[projects]]\nid = \"b\"\npath = \"/tmp/b\"\n";
        std::fs::write(&path, &t).unwrap();
        for i in 0..3 {
            m.ui.right_width_pct = 30 + i;
            q.save_eager(m.clone()).unwrap();
            assert!(read(&path).contains("/tmp/b"), "save {i}:\n{}", read(&path));
        }
    }

    /// A `set` that lands between dux loading the file and building its
    /// writer is not reverted: the writer's base is the loaded text, not a
    /// fresh read.
    #[test]
    fn adv_a_set_between_load_and_writer_build_survives() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let mut m = loaded(&path);
        let key = crate::config_keys::lookup("ui.left_width_pct").unwrap();
        crate::config_keys::set_plain(&path, &key, "40").unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &m);
        m.ui.copy_on_select = !m.ui.copy_on_select;
        q.save_eager(m.clone()).unwrap();
        m.ui.right_width_pct = 31;
        q.save_eager(m).unwrap();
        assert!(
            read(&path).contains("left_width_pct = 40"),
            "{}",
            read(&path)
        );
    }

    // Sixth review cases.

    fn pc(id: &str, path: &str, name: &str) -> crate::config::ProjectConfig {
        crate::config::ProjectConfig {
            id: id.into(),
            path: path.into(),
            name: Some(name.into()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        }
    }

    /// dux changes B: the file must end with one B.
    #[test]
    fn rv_same_name_hand_delete_and_memory_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/x/api\"\nname = \"api\"\n\n[[projects]]\nid = \"b\"\npath = \"/y/api\"\nname = \"api\"\n").unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        // hand delete A
        std::fs::write(
            &path,
            "[[projects]]\nid = \"b\"\npath = \"/y/api\"\nname = \"api\"\n",
        )
        .unwrap();
        let mut memory = loaded.clone();
        memory.projects[1].default_provider = Some("codex".into());
        q.save_eager(memory).unwrap();
        let text = read(&path);
        eprintln!("RESULT1:\n{text}");
        assert_eq!(text.matches("id = \"b\"").count(), 1, "{text}");
    }

    /// The user moves a project by hand: deletes the old entry, writes a new
    /// one without an id but with the same name. The new entry must not
    /// inherit the old id.
    #[test]
    fn rv_hand_move_without_id_does_not_steal_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/old/dux\"\nname = \"dux\"\n",
        )
        .unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        std::fs::write(&path, "[[projects]]\npath = \"/new/dux\"\nname = \"dux\"\n").unwrap();
        let mut memory = loaded.clone();
        memory.ui.copy_on_select = !memory.ui.copy_on_select;
        q.save_eager(memory).unwrap();
        let text = read(&path);
        eprintln!("RESULT2:\n{text}");
        assert!(!text.contains("id = \"a\""), "{text}");
    }

    /// Env var deleted by hand, then 3 saves, then memory changes another env var.
    #[test]
    fn rv_env_hand_delete_survives_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\nA = \"1\"\nB = \"2\"\n").unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        std::fs::write(&path, "[env]\nB = \"2\"\nC = \"3\"\n").unwrap();
        let mut memory = loaded.clone();
        for i in 0..3 {
            memory.ui.copy_on_select = i % 2 == 0;
            q.save_eager(memory.clone()).unwrap();
        }
        memory.env.insert("D".into(), "4".into());
        q.save_eager(memory.clone()).unwrap();
        memory.env.remove("B");
        q.save_eager(memory.clone()).unwrap();
        let text = read(&path);
        eprintln!("RESULT3:\n{text}");
        assert!(!text.contains("A ="), "{text}");
        assert!(text.contains("C = \"3\""), "{text}");
        assert!(text.contains("D = \"4\""), "{text}");
        assert!(!text.contains("B ="), "{text}");
    }

    /// Macros reorder in memory while hand adds a macro.
    #[test]
    fn rv_hand_added_project_then_memory_removes_other_then_readds() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"a\"\n",
        )
        .unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        // hand adds c
        let mut t = read(&path);
        t.push_str("\n[[projects]]\npath = \"/c\"\nname = \"c\"\n");
        std::fs::write(&path, t).unwrap();
        // dux adds b
        let mut memory = loaded.clone();
        memory.projects.push(pc("b", "/b", "b"));
        q.save_eager(memory.clone()).unwrap();
        // dux removes a
        memory.projects.remove(0);
        q.save_eager(memory.clone()).unwrap();
        // dux removes b
        memory.projects.remove(0);
        q.save_eager(memory.clone()).unwrap();
        let text = read(&path);
        eprintln!("RESULT4:\n{text}");
        assert!(text.contains("/c"), "{text}");
        assert!(!text.contains("\"/a\""), "{text}");
        assert!(!text.contains("\"/b\""), "{text}");
    }

    #[test]
    fn rv_hand_add_same_name_then_remove_old_in_dux() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/old/api\"\nname = \"api\"\n",
        )
        .unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        let mut t = read(&path);
        t.push_str("\n[[projects]]\npath = \"/new/api\"\nname = \"api\"\n");
        std::fs::write(&path, t).unwrap();
        let mut memory = loaded.clone();
        memory.projects.clear();
        q.save_eager(memory).unwrap();
        let text = read(&path);
        assert!(
            text.contains("/new/api"),
            "hand-added project lost:\n{}",
            text.lines().take(8).collect::<Vec<_>>().join("\n")
        );
    }
}

/// Seventh review cases, kept as regression tests.
#[cfg(test)]
mod zz_attack {
    use super::*;
    fn read(p: &std::path::Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }
    fn setup(
        text: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Config,
        ConfigWriteQueue,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        (dir, path, loaded, q)
    }

    /// Hand-added project env var, then dux changes another var of that project's env.
    #[test]
    fn za_project_env_hand_add_lost() {
        let (_d, path, loaded, q) =
            setup("[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\" }\n");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\", B = \"2\" }\n",
        )
        .unwrap();
        let mut m = loaded.clone();
        m.projects[0].env.insert("C".into(), "3".into());
        q.save_eager(m).unwrap();
        let t = read(&path);
        eprintln!("ZA:\n{t}");
        assert!(t.contains("B = \"2\""), "{t}");
    }

    /// Project env written as a subtable by hand.
    #[test]
    fn zb_project_env_subtable_hand_add_lost() {
        let (_d, path, loaded, q) =
            setup("[[projects]]\nid = \"a\"\npath = \"/a\"\n[projects.env]\nA = \"1\"\n");
        std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/a\"\n# my env\n[projects.env]\nA = \"1\"\nB = \"2\"\n").unwrap();
        let mut m = loaded.clone();
        m.projects[0].env.insert("C".into(), "3".into());
        q.save_eager(m).unwrap();
        let t = read(&path);
        eprintln!("ZB:\n{t}");
        assert!(t.contains("B = \"2\""), "{t}");
        assert!(t.contains("C = \"3\""), "{t}");
        assert!(t.contains("# my env"), "the comment stays: {t}");
        assert!(t.contains("[projects.env]"), "the subtable form stays: {t}");
    }

    /// dux removes project a; disk meanwhile has a hand-added id-less project at another path. 3 saves.
    #[test]
    fn zc_multi_save_mix() {
        let (_d, path, loaded, q) = setup(
            "[env]\nX = \"1\"\n\n[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"api\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\nname = \"api\"\n",
        );
        let mut t = read(&path);
        t = t.replace("[env]\nX = \"1\"\n", "[env]\nX = \"1\"\nH = \"hand\"\n");
        t.push_str("\n[[projects]]\npath = \"/c\"\nname = \"api\"\n");
        std::fs::write(&path, &t).unwrap();
        let mut m = loaded.clone();
        m.projects.retain(|p| p.id != "a");
        q.save_eager(m.clone()).unwrap();
        eprintln!("ZC1:\n{}", read(&path));
        // hand deletes b's name, adds key
        let t = read(&path).replace("name = \"api\"\n", "");
        std::fs::write(&path, &t).unwrap();
        m.ui.copy_on_select = !m.ui.copy_on_select;
        q.save_eager(m.clone()).unwrap();
        eprintln!("ZC2:\n{}", read(&path));
        m.env.insert("Y".into(), "2".into());
        q.save_eager(m.clone()).unwrap();
        m.projects.push(crate::config::ProjectConfig {
            id: "d".into(),
            path: "/c".into(),
            name: Some("c".into()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        });
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        eprintln!("ZC3:\n{t}");
        assert!(t.contains("H = \"hand\""));
        assert!(!t.contains("\"/a\""));
        assert_eq!(t.matches("\"/c\"").count(), 1, "{t}");
        assert!(!t.contains("name = \"api\""), "{t}");
    }

    /// Hand-added provider and macro, dux edits macros.
    #[test]
    fn zd_macros_providers() {
        let (_d, path, loaded, q) = setup("[macros]\none = \"1\"\ntwo = \"2\"\n");
        let t = read(&path) + "three = \"3\"\n\n[providers.mine]\ncommand = \"mine\"\n";
        std::fs::write(&path, &t).unwrap();
        let mut m = loaded.clone();
        eprintln!("macros type: {:?}", m.macros);
        q.save_eager(m.clone()).unwrap();
        m.ui.copy_on_select = !m.ui.copy_on_select;
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        eprintln!("ZD:\n{t}");
        assert!(t.contains("three"), "{t}");
        assert!(t.contains("providers.mine"), "{t}");
    }

    /// Written base: a config dux wrote whose source is `written`; hand deletes a key
    /// that only memory/defaults had; and the queue's seen is only W.
    #[test]
    fn ze_written_base_then_hand_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let loaded = crate::config::load_config_file(&path).unwrap();
        // simulate startup sync write
        let mut cfg = loaded.clone();
        cfg.env.insert("S".into(), "1".into());
        let w = crate::config_write::save_config_three_way(
            &path,
            Some(crate::config_write::SaveBase::read(&loaded)),
            &cfg,
            crate::config_write::Durability::Fsync,
        )
        .unwrap();
        cfg.source_text = crate::config::SourceText::written(&w, cfg.clone());
        eprintln!("W:\n{w}");
        let q = ConfigWriteQueue::with_base(path.clone(), &cfg);
        // hand delete left_width_pct and copy_on_select
        let t = read(&path).replace("left_width_pct = 20\n", "");
        std::fs::write(&path, &t).unwrap();
        let mut m = cfg.clone();
        m.env.insert("T".into(), "2".into());
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        eprintln!("ZE:\n{t}");
        assert!(!t.contains("left_width_pct"), "{t}");
    }

    /// A project env variable deleted by hand stays deleted when dux changes
    /// another variable of the same project, across saves.
    #[test]
    fn zf_project_env_hand_delete_survives() {
        let (_d, path, loaded, q) =
            setup("[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\", B = \"2\" }\n");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\" }\n",
        )
        .unwrap();
        let mut m = loaded.clone();
        m.projects[0].env.insert("C".into(), "3".into());
        q.save_eager(m.clone()).unwrap();
        m.ui.copy_on_select = !m.ui.copy_on_select;
        q.save_eager(m).unwrap();
        let t = read(&path);
        assert!(!t.contains("B = "), "{t}");
        assert!(t.contains("A = \"1\"") && t.contains("C = \"3\""), "{t}");
    }

    /// A change whose save failed is still a change: the base is the config
    /// of the last save that succeeded, so a later save writes it.
    #[test]
    fn zg_a_change_whose_save_failed_is_written_by_a_later_save() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path, loaded, q) = setup("[env]\nA = \"1\"\n");
        let mut m = loaded.clone();
        m.env.insert("B".into(), "2".into());
        q.save_eager(m.clone()).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let mut failed = m.clone();
        failed.ui.left_width_pct = 41;
        let result = q.save_eager(failed.clone());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err(), "the write failed");
        // What the engine hands the writer after a reload whose deferred
        // save failed: the surfaced config (with the failed change), its base
        // the last successful write.
        let (_, last) = q.last_written();
        let mut surfaced = failed.clone();
        surfaced.source_text = last.expect("a write succeeded");
        q.set_base(surfaced.clone());
        q.save_eager(surfaced).unwrap();
        assert!(
            read(&path).contains("left_width_pct = 41"),
            "{}",
            read(&path)
        );
    }

    /// A project moved by hand and a new one added at its old path, while dux
    /// removes the project: the new one is not taken for the removed one.
    #[test]
    fn zh_one_base_entry_never_absorbs_two_file_entries() {
        let (_d, path, loaded, q) = setup("[[projects]]\nid = \"a\"\npath = \"/old\"\n");
        std::fs::write(
            &path,
            "[[projects]]\nid = \"a\"\npath = \"/new\"\n\n[[projects]]\npath = \"/old\"\n",
        )
        .unwrap();
        let mut m = loaded.clone();
        m.projects.clear();
        q.save_eager(m).unwrap();
        let t = read(&path);
        assert!(
            t.contains("\"/old\""),
            "the new project at the old path stays: {t}"
        );
    }
}
