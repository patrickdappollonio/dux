//! One ordered, off-thread, atomic config writer per process.

use std::mem;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::config_write::{Durability, save_config_with};

const QUIET_WINDOW: Duration = Duration::from_millis(250);
const EAGER_TIMEOUT: Duration = Duration::from_secs(2);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

enum WriteMsg {
    Lazy(Config),
    Eager {
        config: Config,
        reply: SyncSender<Result<(), String>>,
    },
    Flush(SyncSender<()>),
    Pause(SyncSender<()>),
    Resume,
}

pub struct ConfigWriteQueue {
    tx: Sender<WriteMsg>,
    writer: Option<JoinHandle<()>>,
}

/// Holds a reload/recover barrier open. The writer is paused (drained) while the
/// guard lives; dropping it resumes the writer. Owns a `Sender<WriteMsg>` clone
/// (not a borrow) so it can be stored on `Engine` as `reload_guard`.
pub struct QuiesceGuard {
    tx: Sender<WriteMsg>,
}

impl Drop for QuiesceGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(WriteMsg::Resume);
    }
}

impl ConfigWriteQueue {
    pub fn new(config_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let writer = thread::Builder::new()
            .name("config-writer".into())
            .spawn(move || writer_loop(rx, config_path))
            .expect("spawn config-writer thread");
        ConfigWriteQueue {
            tx,
            writer: Some(writer),
        }
    }

    /// Deferred, coalesced, fire-and-forget. A dead writer is surfaced lazily via
    /// the next eager/flush; lazy itself never blocks.
    pub fn save_lazy(&self, config: Config) {
        let _ = self.tx.send(WriteMsg::Lazy(config));
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

    /// Exit-time drain: write any pending lazy, bounded by a timeout.
    pub fn flush(&self) {
        let (ack, rx) = mpsc::sync_channel(0);
        if self.tx.send(WriteMsg::Flush(ack)).is_ok() {
            let _ = rx.recv_timeout(FLUSH_TIMEOUT);
        }
    }

    /// Begin a reload/recover barrier: drain pending + pause the writer, returning
    /// a guard that resumes on drop. The caller does its own write synchronous-direct
    /// while holding the guard.
    pub fn quiesce(&self) -> QuiesceGuard {
        let (ack, rx) = mpsc::sync_channel(0);
        if self.tx.send(WriteMsg::Pause(ack)).is_ok() {
            let _ = rx.recv_timeout(FLUSH_TIMEOUT);
        }
        QuiesceGuard {
            tx: self.tx.clone(),
        }
    }

    /// Test-only: a queue whose writer thread has already exited, so `save_eager`
    /// deterministically hits the dead-writer path.
    #[cfg(test)]
    pub fn with_dead_writer(config_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<WriteMsg>();
        drop(rx); // receiver gone → the writer is effectively dead
        let _ = config_path;
        ConfigWriteQueue { tx, writer: None }
    }
}

impl Drop for ConfigWriteQueue {
    fn drop(&mut self) {
        // Spec: flush pending lazy writes on clean process exit.  This fires when
        // the engine (and thus the queue) is finally dropped at real exit.  It does
        // NOT fire at the in-process TUI→web flip, which MOVES the engine rather
        // than dropping it, so the queue keeps running across the flip.
        //
        // Step 1 — send a Flush and wait for the writer to drain any pending lazy
        // write.  flush() is bounded by FLUSH_TIMEOUT so Drop cannot hang the
        // process indefinitely.
        self.flush();

        // Step 2 — disconnect the sender so the writer loop exits after flush.
        // We replace `self.tx` with a fresh disconnected channel; the old tx is
        // then dropped, which disconnects the channel and causes the writer's
        // blocking `recv()` to return `Err(Disconnected)` → `break`.
        let (dead_tx, _dead_rx) = mpsc::channel::<WriteMsg>();
        let _ = mem::replace(&mut self.tx, dead_tx);
        // _dead_rx is dropped here, disconnecting dead_tx immediately.

        // Step 3 — join the writer thread for a clean shutdown.  The writer exited
        // (or will exit within one recv round-trip) because the original tx is now
        // gone.  Joining is best-effort: if the thread already finished or panicked
        // we just move on.
        if let Some(handle) = self.writer.take() {
            let _ = handle.join();
        }
    }
}

fn writer_loop(rx: Receiver<WriteMsg>, path: PathBuf) {
    let mut pending: Option<Config> = None;
    let mut deadline: Option<Instant> = None;

    loop {
        let msg = match deadline {
            Some(d) => {
                let wait = d.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None, // deadline elapsed
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };

        match msg {
            None => flush_pending(&path, &mut pending, &mut deadline),
            Some(WriteMsg::Lazy(cfg)) => {
                pending = Some(cfg);
                if deadline.is_none() {
                    deadline = Some(Instant::now() + QUIET_WINDOW);
                }
            }
            Some(WriteMsg::Eager { config, reply }) => {
                pending = None;
                deadline = None;
                let res = save_config_with(&path, &config, Durability::Fsync)
                    .map_err(|e| format!("{e:#}"));
                if let Err(ref e) = res {
                    crate::logger::error(&format!("eager config write failed: {e}"));
                }
                let _ = reply.send(res); // dropped reply = caller moved on; no-op
            }
            Some(WriteMsg::Flush(ack)) => {
                flush_pending(&path, &mut pending, &mut deadline);
                let _ = ack.send(());
            }
            Some(WriteMsg::Pause(ack)) => {
                flush_pending(&path, &mut pending, &mut deadline);
                let _ = ack.send(());
                // Hold paused until Resume; drop stray lazies, reject stray eagers.
                loop {
                    match rx.recv() {
                        Ok(WriteMsg::Resume) => break,
                        Ok(WriteMsg::Lazy(_)) => {} // stale snapshot — drop
                        Ok(WriteMsg::Eager { reply, .. }) => {
                            let _ =
                                reply.send(Err("config busy (reload in progress); retry".into()));
                        }
                        Ok(WriteMsg::Flush(ack)) => {
                            let _ = ack.send(());
                        }
                        Ok(WriteMsg::Pause(ack)) => {
                            let _ = ack.send(());
                        }
                        Err(_) => return,
                    }
                }
            }
            Some(WriteMsg::Resume) => {} // no-op when not paused
        }
    }
}

fn flush_pending(
    path: &std::path::Path,
    pending: &mut Option<Config>,
    deadline: &mut Option<Instant>,
) {
    *deadline = None;
    if let Some(cfg) = pending.take()
        && let Err(e) = save_config_with(path, &cfg, Durability::NoFsync)
    {
        crate::logger::error(&format!("lazy config write failed: {e:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn read(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
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
            // Drop q here — the Drop impl must flush the pending lazy write.
        }

        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            saved.contains("DROP_MARKER = \"flushed\""),
            "pending lazy write was NOT flushed on drop: {saved}"
        );
    }
}
