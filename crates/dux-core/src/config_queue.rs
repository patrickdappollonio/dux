//! One ordered, off-thread, atomic config writer per process.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::config_write::{Durability, SaveBase, save_config_three_way, union_seen};
use crate::worker::WorkerEvent;

const QUIET_WINDOW: Duration = Duration::from_millis(250);
const EAGER_TIMEOUT: Duration = Duration::from_secs(2);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

enum WriteMsg {
    /// A lazy save is waiting in the [`LazySlot`].
    Lazy,
    Eager {
        /// Its place among every save, lazy ones included.
        seq: u64,
        config: Box<Config>,
        reply: SyncSender<Result<(), String>>,
    },
    Flush(SyncSender<()>),
    Pause(SyncSender<()>),
    /// End a barrier. A lazy save no newer than `discard_through` was made
    /// during a barrier that held and is discarded; 0 keeps every one.
    Resume {
        discard_through: u64,
    },
    /// The config dux just adopted (a reload), carrying the exact text it was
    /// read from: the base every following save is a three-way patch against.
    SetBase(Box<Config>),
    /// Stop the writer thread unconditionally, obeyed even while paused. Sent by
    /// `Drop` so shutdown never depends on channel disconnect (a `QuiesceGuard`
    /// holds a sender clone, so the channel can stay connected) or on guard drop
    /// order.
    Shutdown,
}

/// The process's one ordered, off-thread config writer: each save is an atomic three-way patch
/// against the text dux last read.
#[derive(Debug)]
pub struct ConfigWriteQueue {
    tx: Sender<WriteMsg>,
    writer: Option<JoinHandle<()>>,
    lazy: Arc<LazySlot>,
    last_written: LastWritten,
}

/// The order of every save, and the newest lazy save the writer has not taken
/// yet. Lazy saves are coalesced, so only the newest matters: keeping it here
/// rather than in the channel bounds what a stalled writer holds to one config,
/// and the newest is never the one let go.
///
/// A save takes its place in the order and reaches the channel under the one
/// lock, so the order of places is the order the writer receives saves in.
#[derive(Debug, Default)]
struct LazySlot {
    state: Mutex<SlotState>,
}

#[derive(Debug, Default)]
struct SlotState {
    /// The place of the newest save, eager ones included, counted from 1.
    seq: u64,
    latest: Option<(u64, Config)>,
}

impl LazySlot {
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keep `config` as the newest lazy save, telling the writer when the
    /// slot was empty (a writer that has not taken it yet already has a
    /// notice waiting, and takes this newer config with it).
    fn save_lazy(&self, tx: &Sender<WriteMsg>, config: Config) {
        let mut state = self.lock();
        state.seq += 1;
        let seq = state.seq;
        if state.latest.replace((seq, config)).is_none() {
            let _ = tx.send(WriteMsg::Lazy);
        }
    }

    /// Send an eager save in its place in the order. `false` when the
    /// writer is gone.
    fn send_eager(
        &self,
        tx: &Sender<WriteMsg>,
        config: Config,
        reply: SyncSender<Result<(), String>>,
    ) -> bool {
        let mut state = self.lock();
        state.seq += 1;
        let seq = state.seq;
        tx.send(WriteMsg::Eager {
            seq,
            config: Box::new(config),
            reply,
        })
        .is_ok()
    }

    /// The place of the newest save made so far.
    fn newest(&self) -> u64 {
        self.lock().seq
    }

    fn take(&self) -> Option<(u64, Config)> {
        self.lock().latest.take()
    }
}

/// The writer's base after its most recent SUCCESSFUL write (the config it
/// wrote and the text it has seen), with a count of writes, so a caller can
/// tell whether a write happened since it last looked.
type LastWritten = Arc<std::sync::Mutex<(u64, Option<crate::config::SourceText>)>>;

/// Holds a reload/recover barrier open. The writer is paused (drained) while the
/// guard lives; dropping it resumes the writer. Owns a `Sender<WriteMsg>` clone
/// (not a borrow) so it can be stored on `Engine` as `reload_guard`.
#[derive(Debug)]
pub struct QuiesceGuard {
    tx: Sender<WriteMsg>,
    lazy: Arc<LazySlot>,
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
    pub(crate) fn is_acknowledged(&self) -> bool {
        self.acknowledged
    }
}

impl Drop for QuiesceGuard {
    fn drop(&mut self) {
        // Resume is sent in ALL cases, even when `acknowledged` is false: a
        // slow-but-alive writer will eventually process the Pause and would
        // block forever waiting for the Resume if we skipped it here. A
        // barrier that never held had no reload behind it, so the lazy saves
        // made while it waited are kept.
        let discard_through = if self.acknowledged {
            self.lazy.newest()
        } else {
            0
        };
        let _ = self.tx.send(WriteMsg::Resume { discard_through });
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
        let lazy: Arc<LazySlot> = Arc::default();
        let last_written: LastWritten = Arc::default();
        let writer = thread::Builder::new()
            .name("config-writer".into())
            .spawn({
                let lazy = Arc::clone(&lazy);
                let last_written = std::sync::Arc::clone(&last_written);
                move || {
                    writer_loop(
                        rx,
                        &config_path,
                        base,
                        &lazy,
                        status_lane.as_ref(),
                        last_written,
                    );
                }
            })
            .expect("spawn config-writer thread");
        ConfigWriteQueue {
            tx,
            writer: Some(writer),
            lazy,
            last_written,
        }
    }

    /// How many writes this writer has made, and its base after the latest
    /// one that succeeded, as a [`crate::config::SourceText`]: the text it
    /// has seen and the config it wrote. The engine reads it around a
    /// reload's deferred commands: when they wrote, that is what the file
    /// agrees with, not the reloaded text. A save that failed changes
    /// neither, so its change still counts as one a later save must write.
    pub(crate) fn last_written(&self) -> (u64, Option<crate::config::SourceText>) {
        self.last_written
            .lock()
            .map_or((0, None), |guard| guard.clone())
    }

    /// Deferred, coalesced, fire-and-forget. A dead writer is surfaced lazily via
    /// the next eager/flush; lazy itself never blocks.
    pub fn save_lazy(&self, config: Config) {
        self.lazy.save_lazy(&self.tx, config);
    }

    /// Awaited write: blocks (a few ms) for the result, ~2 s timeout. On a dead
    /// writer it returns an error rather than hanging.
    ///
    /// # Errors
    ///
    /// Fails when the writer thread is gone, does not answer within its timeout, or the save itself
    /// fails.
    pub fn save_eager(&self, config: Config) -> Result<(), String> {
        let (reply, rx) = mpsc::sync_channel(1);
        if !self.lazy.send_eager(&self.tx, config, reply) {
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
                    .is_none_or(std::thread::JoinHandle::is_finished)
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
    pub(crate) fn set_base(&self, config: Config) {
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
            lazy: Arc::clone(&self.lazy),
            acknowledged,
        }
    }

    /// Test-only: a queue whose writer thread has already exited, so `save_eager`
    /// deterministically hits the dead-writer path.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_dead_writer(config_path: &std::path::Path) -> Self {
        let (tx, rx) = mpsc::channel::<WriteMsg>();
        drop(rx); // receiver gone → the writer is effectively dead
        let _ = config_path;
        ConfigWriteQueue {
            tx,
            writer: None,
            lazy: Arc::default(),
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

fn writer_loop(
    rx: Receiver<WriteMsg>,
    path: &std::path::Path,
    base: Option<Base>,
    lazy: &LazySlot,
    status_lane: Option<&Sender<WorkerEvent>>,
    last_written: LastWritten,
) {
    // Note: under panic = "abort" this guard is inert (the process aborts); it is active under the default unwind strategy.
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer_loop_inner(rx, path, base, lazy, status_lane, last_written);
    })) {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        crate::logger::error(&format!("config-writer thread panicked: {msg}"));
        // Emptied so the next lazy save reaches the (failing) send rather than
        // waiting on a notice nobody will read.
        let _ = lazy.take();
    }
}

struct WriterDisconnected;

enum WriterControl {
    Continue,
    Stop,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the loop owns the receiver so a panic drops it while unwinding, before writer_loop resets the in-flight counter, and every later send fails"
)]
fn writer_loop_inner(
    rx: Receiver<WriteMsg>,
    path: &std::path::Path,
    base: Option<Base>,
    lazy: &LazySlot,
    status_lane: Option<&Sender<WorkerEvent>>,
    last_written: LastWritten,
) {
    let mut state = WriterState {
        base,
        pending: None,
        written_seq: 0,
        deadline: None,
        last_written,
    };

    loop {
        let input = receive_writer_input(&rx, state.deadline);
        if matches!(
            handle_writer_input(input, &rx, path, lazy, &mut state, status_lane),
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
        // A deadline that has passed is kept before anything else is read, so
        // a steady stream of messages cannot hold a pending save back.
        Some(deadline) if Instant::now() >= deadline => Ok(None),
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
            seen: union_seen(Some(&self.seen), written, &config.projects),
            config,
        }
    }
}

/// The base for a file just read: its config as loaded (the load's
/// corrections included, which is what memory starts as, so a correction is
/// never a change dux made), and what it expressed, the keys its deprecated
/// ones carry over to included (see [`crate::config_write::seen_of_read`]).
fn base_read_from(text: &str) -> Option<Base> {
    let config = crate::config::config_from_text_as_loaded(text).ok()?;
    Some(Base {
        seen: crate::config_write::seen_of_read(text, &config.projects),
        config,
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
    let mut base = base_read_from(text)?;
    adopt_minted_ids(&mut base, config);
    Some(base)
}

/// A project written without an `id` gets one minted on every read, so the
/// base's re-read of the text and `config`, read from the same text, gave
/// the same entry different ids. The base takes `config`'s, so memory's
/// projects find their own base entries by id: an id-less entry is then
/// never told apart from another at the same path by its name alone, which
/// a rename can make the other one's. Only when both list the same paths in
/// the same order (they were read from one text), and only for an entry the
/// text gives no id; an id written in the file is the file's.
fn adopt_minted_ids(base: &mut Base, config: &Config) {
    let same_entries = base.config.projects.len() == config.projects.len()
        && base
            .config
            .projects
            .iter()
            .zip(&config.projects)
            .all(|(a, b)| a.path == b.path);
    if !same_entries {
        return;
    }
    let Ok(seen) = base.seen.parse::<toml_edit::DocumentMut>() else {
        return;
    };
    let Some(entries) = seen
        .get("projects")
        .and_then(|item| item.as_array_of_tables())
    else {
        return;
    };
    if entries.len() != base.config.projects.len() {
        return;
    }
    for ((project, theirs), entry) in base
        .config
        .projects
        .iter_mut()
        .zip(&config.projects)
        .zip(entries.iter())
    {
        if !entry.contains_key("id") {
            project.id.clone_from(&theirs.id);
        }
    }
}

/// The first base: the loaded config's own text, or no base at all for a
/// config read from no file (its saves are then the full patch).
fn base_of_loaded(loaded: &Config) -> Option<Base> {
    // A config read from no file (dux started without one) is still what
    // memory started with: only what memory changes since is dux's change,
    // so a file the user writes afterwards keeps everything they wrote.
    // Nothing has been seen, so a setting the file lacks is filled in.
    base_from_source(loaded).or_else(|| {
        Some(Base {
            config: loaded.clone(),
            seen: String::new(),
        })
    })
}

/// The base for a config a reload adopted: the text it was read from, or,
/// for a config read from no file, that config and the file as it is now.
fn base_adopted(path: &std::path::Path, config: Config) -> Base {
    if let Some(base) = base_from_source(&config) {
        return base;
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    Base {
        seen: crate::config_write::seen_of_read(&text, &config.projects),
        config,
    }
}

/// What the writer thread carries between messages: the base its three-way
/// saves patch against, and the coalesced lazy save waiting for its deadline.
struct WriterState {
    base: Option<Base>,
    /// The coalesced lazy save, with its place among every save.
    pending: Option<(u64, Config)>,
    /// The place of the newest save written, lazy or eager: nothing older
    /// is ever written over it.
    written_seq: u64,
    deadline: Option<Instant>,
    last_written: LastWritten,
}

impl WriterState {
    /// Take the slot's lazy save as the pending one, unless something newer
    /// is pending or already written.
    fn take_lazy(&mut self, lazy: &LazySlot) {
        let Some((seq, config)) = lazy.take() else {
            return;
        };
        let newer = seq > self.written_seq
            && self
                .pending
                .as_ref()
                .is_none_or(|(pending, _)| seq > *pending);
        if newer {
            self.pending = Some((seq, config));
            if self.deadline.is_none() {
                self.deadline = Some(Instant::now() + QUIET_WINDOW);
            }
        }
    }

    /// A save wrote `config` as `written`: the next base, and the text the
    /// engine can ask for.
    fn record_write(&mut self, config: Config, written: &str) {
        let base = written_base(self.base.as_ref(), config, written);
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
    lazy: &LazySlot,
    state: &mut WriterState,
    status_lane: Option<&Sender<WorkerEvent>>,
) -> WriterControl {
    match input {
        Ok(None) => flush_pending(path, state, status_lane),
        Err(WriterDisconnected) => return WriterControl::Stop,
        Ok(Some(WriteMsg::Lazy)) => state.take_lazy(lazy),
        Ok(Some(WriteMsg::Eager { seq, config, reply })) => {
            // A lazy save newer than this one stays pending: it was taken
            // from the slot ahead of this eager message, after being made.
            if state
                .pending
                .as_ref()
                .is_some_and(|(pending, _)| *pending < seq)
            {
                state.pending = None;
                state.deadline = None;
            }
            // A newer save is already on disk; this older snapshot would
            // undo it.
            if seq < state.written_seq {
                let _ = reply.send(Ok(()));
                return WriterControl::Continue;
            }
            let result = save_against_base(path, state.base.as_ref(), &config, Durability::Fsync)
                .map_err(|e| format!("{e:#}"));
            match &result {
                Ok(text) => {
                    state.record_write(*config, text);
                    state.written_seq = seq;
                }
                Err(error) => crate::logger::error(&format!("eager config write failed: {error}")),
            }
            let _ = reply.send(result.map(|_| ()));
        }
        // Each of these writes every lazy save made before it, so it takes
        // the slot itself: the notice of the save in it may still be on its
        // way, or may have come for an older save.
        Ok(Some(WriteMsg::Flush(ack))) => {
            state.take_lazy(lazy);
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
        }
        Ok(Some(WriteMsg::Pause(ack))) => {
            state.take_lazy(lazy);
            flush_pending(path, state, status_lane);
            let _ = ack.send(());
            debug_assert!(state.pending.is_none());
            let Some(discard_through) = run_paused_writer(rx, path, &mut state.base) else {
                return WriterControl::Stop;
            };
            // The slot holds the newest lazy save; one made during a barrier
            // that held is a stale snapshot, and any other waits as usual.
            if lazy
                .lock()
                .latest
                .as_ref()
                .is_some_and(|(seq, _)| *seq <= discard_through)
            {
                let _ = lazy.take();
            }
            state.take_lazy(lazy);
        }
        Ok(Some(WriteMsg::Resume { .. })) => {}
        Ok(Some(WriteMsg::SetBase(config))) => adopt_base(path, &mut state.base, *config),
        Ok(Some(WriteMsg::Shutdown)) => {
            state.take_lazy(lazy);
            flush_pending(path, state, status_lane);
            return WriterControl::Stop;
        }
    }
    WriterControl::Continue
}

/// Move the writer's base to `config`, which a reload adopted. What the
/// reload read is ADDED to what has been seen, never replacing it, so a
/// setting the user deleted by hand stays seen and stays deleted; projects
/// gone from both memory and the file are still forgotten.
fn adopt_base(path: &std::path::Path, base: &mut Option<Base>, config: Config) {
    let mut adopted = base_adopted(path, config);
    if let Some(old) = base.take() {
        adopted.seen = crate::config_write::union_seen(
            Some(&old.seen),
            &adopted.seen,
            &adopted.config.projects,
        );
    }
    *base = Some(adopted);
}

/// Wait out a barrier, nested ones included. Returns the newest place any
/// `Resume` asked to discard through, or `None` when the writer must stop.
/// Lazy saves stay in their slot meanwhile, so the one made right after the
/// barrier is not taken for one made during it.
fn run_paused_writer(
    rx: &Receiver<WriteMsg>,
    path: &std::path::Path,
    base: &mut Option<Base>,
) -> Option<u64> {
    let mut depth = 1usize;
    let mut discard = 0;
    loop {
        match rx.recv() {
            Ok(WriteMsg::Resume { discard_through }) => {
                discard = discard.max(discard_through);
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(discard);
                }
            }
            Ok(WriteMsg::Shutdown) | Err(_) => return None,
            Ok(WriteMsg::Lazy) => {}
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
            Ok(WriteMsg::SetBase(config)) => adopt_base(path, base, *config),
        }
    }
}

/// One three-way save against the writer's base; the text written comes back
/// to become the next base.
fn save_against_base(
    path: &std::path::Path,
    base: Option<&Base>,
    config: &Config,
    durability: Durability,
) -> anyhow::Result<String> {
    save_config_three_way(path, base.map(Base::as_save_base), config, durability)
}

/// The base after a save wrote `config` as `written`.
fn written_base(base: Option<&Base>, mut config: Config, written: &str) -> Base {
    // The base keeps the config, not the text it came from.
    config.source_text = crate::config::SourceText::default();
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
    let Some((seq, cfg)) = state.pending.take() else {
        return;
    };
    if seq < state.written_seq {
        return;
    }
    let result = save_against_base(path, state.base.as_ref(), &cfg, Durability::NoFsync);
    let result = match result {
        Ok(text) => {
            state.record_write(cfg, &text);
            state.written_seq = seq;
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
mod three_way_save_regressions;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn read(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// `dux config set` changes a key on disk, an
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

    /// A value dux corrected at load (an out-of-range font size) is never
    /// written: the writer's base is the config as loaded, which is what
    /// memory started as, so the correction is not a change dux made. The
    /// file keeps what the user wrote, and the load keeps warning about it.
    #[test]
    fn a_load_time_correction_is_never_written() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[ui]\nterminal_font_size = 500\ncopy_on_select = true\n",
        )
        .unwrap();
        let mut memory = crate::config::load_config_file(&path).expect("load");
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        assert_ne!(memory.ui.terminal_font_size, 500, "corrected in memory");
        memory.ui.copy_on_select = false;
        q.save_eager(memory).unwrap();
        let after = read(&path);
        assert!(after.contains("terminal_font_size = 500"), "{after}");
        assert!(after.contains("copy_on_select = false"), "{after}");
    }

    /// A reload's new base is the corrected config of the text it read: a
    /// correction the reloaded file needs is not written by the next save
    /// either, and a hand fix after the reload survives it.
    #[test]
    fn a_reloads_base_is_the_corrected_config_of_what_it_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
        let memory = crate::config::load_config_file(&path).expect("load");
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(
            &path,
            "[server]\ntailscale = \"maybe\"\n\n[ui]\ncopy_on_select = true\n",
        )
        .unwrap();
        let mut reloaded = crate::config::load_config_file(&path).expect("reload");
        assert_eq!(reloaded.server.tailscale, "auto");
        q.set_base(reloaded.clone());
        std::fs::write(
            &path,
            "[server]\ntailscale = \"no\"\n\n[ui]\ncopy_on_select = true\n",
        )
        .unwrap();
        reloaded.ui.copy_on_select = false;
        q.save_eager(reloaded).unwrap();
        let after: Config = toml::from_str(&read(&path)).unwrap();
        assert_eq!(after.server.tailscale, "no", "{}", read(&path));
        assert!(!after.ui.copy_on_select);
    }

    /// The lazy save a reload signal flushes carries memory's corrected
    /// value, which is never written over a value `dux config set` has just
    /// stored in the file.
    #[test]
    fn a_flushed_lazy_save_never_writes_a_correction_over_a_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[ui]\nterminal_font_size = 500\ncopy_on_select = true\n",
        )
        .unwrap();
        let mut memory = crate::config::load_config_file(&path).expect("load");
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        crate::config_keys::set_plain(
            &path,
            &crate::config_keys::lookup("ui.terminal_font_size").unwrap(),
            "20",
        )
        .expect("set");
        memory.ui.copy_on_select = false;
        q.save_lazy(memory);
        q.flush();
        let after: Config = toml::from_str(&read(&path)).unwrap();
        assert_eq!(after.ui.terminal_font_size, 20, "{}", read(&path));
        assert!(!after.ui.copy_on_select);
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
        let q = ConfigWriteQueue::with_dead_writer(&path);
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
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
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
        let q = ConfigWriteQueue::with_dead_writer(&path);
        let guard = q.quiesce();
        assert!(
            !guard.is_acknowledged(),
            "dead writer must not report acknowledgement"
        );
    }

    // ── Fix 2: bounded join in Drop ────────────────────────────────────────

    /// Dropping a live queue completes in well under `FLUSH_TIMEOUT`, proving the
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

    fn with_env(name: &str, value: &str) -> Config {
        let mut cfg = Config::default();
        cfg.env.insert(name.into(), value.into());
        cfg
    }

    /// A lazy save stored in the slot whose notice has not reached the writer:
    /// another caller between storing and sending, or one whose save replaced
    /// a slot that already had a notice out.
    fn store_without_notice(q: &ConfigWriteQueue, config: Config) {
        let mut slot = q.lazy.lock();
        slot.seq += 1;
        let seq = slot.seq;
        slot.latest = Some((seq, config));
    }

    /// A flush writes every lazy save made before it, its notice still on the
    /// way or not.
    #[test]
    fn a_flush_writes_a_lazy_whose_notice_has_not_arrived() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        store_without_notice(&q, with_env("C", "2"));
        q.flush();
        assert!(read(&path).contains("C = \"2\""), "{}", read(&path));
    }

    /// A barrier writes every lazy save made before it, its notice still on
    /// the way or not, rather than discarding it at the resume.
    #[test]
    fn a_barrier_writes_a_lazy_made_before_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        store_without_notice(&q, with_env("C", "2"));
        drop(q.quiesce());
        q.flush();
        assert!(read(&path).contains("C = \"2\""), "{}", read(&path));
    }

    /// An eager save that reaches the writer after a newer one never writes
    /// its older snapshot over it.
    #[test]
    fn an_older_eager_save_never_overwrites_a_newer_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        for (seq, value) in [(5, "newer"), (3, "older")] {
            let (reply, answer) = mpsc::sync_channel(1);
            q.tx.send(WriteMsg::Eager {
                seq,
                config: Box::new(with_env("V", value)),
                reply,
            })
            .unwrap();
            answer.recv().unwrap().unwrap();
        }
        assert!(read(&path).contains("V = \"newer\""), "{}", read(&path));
    }

    /// A passed deadline is answered before a message waiting in the channel,
    /// so a steady stream cannot hold a pending save back.
    #[test]
    fn a_passed_deadline_comes_before_a_waiting_message() {
        let (tx, rx) = mpsc::channel();
        tx.send(WriteMsg::Lazy).unwrap();
        let passed = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        assert!(matches!(receive_writer_input(&rx, Some(passed)), Ok(None)));
    }

    /// A barrier that never held (the writer was stalled, so no reload ran)
    /// keeps the lazy saves made while it waited.
    #[test]
    fn a_barrier_that_never_held_keeps_its_lazy_saves() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        let held = crate::config_write::ConfigFileLock::acquire(&path).unwrap();
        let (reply, answer) = mpsc::sync_channel(1);
        assert!(q.lazy.send_eager(&q.tx, Config::default(), reply));
        let guard = q.quiesce();
        assert!(!guard.is_acknowledged());
        drop(held);
        answer.recv().unwrap().unwrap();
        // Answered by the paused writer, so the save below is made during
        // the pause.
        q.flush();
        q.save_lazy(with_env("C", "kept"));
        drop(guard);
        q.flush();
        assert!(read(&path).contains("C = \"kept\""), "{}", read(&path));
    }

    /// A burst of lazy saves while the writer is stuck on the disk still ends
    /// with the newest one written.
    #[test]
    fn the_newest_lazy_lands_after_a_stalled_burst() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());
        let held = crate::config_write::ConfigFileLock::acquire(&path).unwrap();
        // An eager save the writer takes first and then waits on the lock in.
        let (reply, answer) = mpsc::sync_channel(1);
        assert!(q.lazy.send_eager(&q.tx, Config::default(), reply));
        for i in 0..300 {
            let mut cfg = Config::default();
            cfg.env.insert("N".into(), i.to_string());
            q.save_lazy(cfg);
        }
        drop(held);
        answer.recv().unwrap().unwrap();
        q.flush();
        assert!(read(&path).contains("N = \"299\""), "{}", read(&path));
    }

    /// Lazies sent during an open barrier are discarded by the paused writer,
    /// and a lazy sent after it still lands.
    #[test]
    fn a_lazy_after_a_barrier_still_lands() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[env]\n").unwrap();
        let q = ConfigWriteQueue::new(path.clone());

        {
            let _guard = q.quiesce(); // writer drained + paused
            // Send a burst during the barrier; all are discarded by the paused writer.
            for i in 0..300 {
                let mut cfg = Config::default();
                cfg.env.insert("N".into(), i.to_string());
                q.save_lazy(cfg);
            }
        } // guard drops → resume

        let mut cfg = Config::default();
        cfg.env.insert("AFTER".into(), "barrier".into());
        q.save_lazy(cfg);
        q.flush();
        let saved = read(&path);
        assert!(saved.contains("AFTER = \"barrier\""), "{saved}");
        assert!(!saved.contains("N = "), "{saved}");
    }
}
