//! Single-instance enforcement for dux.
//!
//! dux keeps all per-user state (config, session database, worktree registry)
//! in a single directory resolved by [`crate::config::DuxPaths`]. Running two
//! dux processes against the same directory produces silently divergent
//! in-memory state: each process only sees sessions it created itself, while
//! the shared SQLite database and git worktrees on disk end up as the union
//! of both processes' activity.
//!
//! This module enforces a hard invariant: exactly one dux instance per config
//! directory. The guarantee comes from an OS-level advisory lock
//! ([`flock(2)`]) on a well-known lockfile. Users who want multiple concurrent
//! workspaces can point `DUX_HOME` at different directories.
//!
//! The lockfile also contains the holder's PID — written after the lock is
//! acquired — so a colliding launch can tell the user which process to close.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::io_retry::retry_on_interrupt_errno;

/// Exclusive single-instance lock on the dux config directory.
///
/// The file handle is kept open for the lifetime of this value. The kernel
/// releases the advisory lock when the file descriptor is closed — including
/// on process exit via `SIGKILL` or crash — so stale lockfiles never block a
/// future launch. Only a live peer actively holding the lock will.
#[derive(Debug)]
pub struct SingleInstanceLock {
    _file: File,
}

/// Result of attempting to acquire the single-instance lock.
#[derive(Debug)]
pub enum AcquireError {
    /// Another live dux process already holds the lock. `pid` is the holder's
    /// PID as read from the lockfile — `None` if the file was empty or its
    /// contents could not be parsed.
    AlreadyRunning { pid: Option<u32>, path: PathBuf },
    /// An unexpected filesystem error occurred (not contention).
    Io { path: PathBuf, err: std::io::Error },
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning { pid, path } => {
                // Each line is written independently so the output is
                // unambiguously left-aligned. Earlier iterations used a single
                // string literal with `\<newline>` continuations; that renders
                // correctly today, but separate writes remove any chance of a
                // stray leading space sneaking in on a reformat.
                match pid {
                    Some(pid) => {
                        writeln!(f, "Another dux instance is already running (PID {pid}).")?
                    }
                    None => writeln!(
                        f,
                        "Another dux instance is already running (PID unknown \
                         — lockfile is empty or unreadable)."
                    )?,
                }
                writeln!(
                    f,
                    "Close it before starting a new one, or set DUX_HOME to a \
                     different directory to run a separate workspace."
                )?;
                write!(f, "Lockfile: {}", path.display())
            }
            Self::Io { path, err } => {
                write!(f, "Failed to acquire lockfile at {}: {err}", path.display())
            }
        }
    }
}

impl std::error::Error for AcquireError {}

/// Bounded retry parameters for reading the holder's PID on a contention
/// loss. There's a small window between the winner's `flock()` returning
/// success and the winner finishing its PID write. A loser that reads the
/// file inside that window sees it empty. Retrying briefly makes the "PID
/// unknown" branch of [`AcquireError`] the exceptional case it should be,
/// while capping the delay well below human-perceptible latency.
const PID_READ_ATTEMPTS: usize = 5;
const PID_READ_RETRY_DELAY: Duration = Duration::from_millis(2);

impl SingleInstanceLock {
    /// Attempt to take the lock at `path`. On success the current PID is
    /// written to the file so a future colliding launch can identify the
    /// holder. The lock is held until the returned value is dropped.
    ///
    /// The parent directory of `path` must already exist.
    pub fn acquire(path: &Path) -> Result<Self, AcquireError> {
        // Opens for read+write so we can both rewrite the PID after acquiring
        // and read the PID of the holder if we lose the race.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|err| AcquireError::Io {
                path: path.to_path_buf(),
                err,
            })?;

        // `flock(2)` is normally fast when paired with `LOCK_NB`, but it can
        // still return `EINTR` if a signal is delivered during the syscall.
        // Retry transparently on `EINTR`; contention (`EWOULDBLOCK`/`EAGAIN`)
        // and real errors flow through unchanged.
        let outcome =
            retry_on_interrupt_errno(|| flock(&file, FlockOperation::NonBlockingLockExclusive));

        match outcome {
            Ok(()) => {
                // We won the race. Replace whatever PID was in the file with
                // our own. Writes here are best-effort: the lock itself comes
                // from flock(), and the PID is diagnostic for colliders.
                let _ = file.set_len(0);
                let _ = file.seek(SeekFrom::Start(0));
                let _ = writeln!(file, "{}", std::process::id());
                let _ = file.flush();
                Ok(Self { _file: file })
            }
            Err(err) if err == Errno::WOULDBLOCK || err == Errno::AGAIN => {
                // Someone else owns the lock. Read their PID so the caller
                // can show an actionable error. The holder may be mid-write,
                // so retry briefly before giving up and reporting "unknown".
                Err(AcquireError::AlreadyRunning {
                    pid: read_holder_pid_with_retry(&mut file),
                    path: path.to_path_buf(),
                })
            }
            Err(err) => Err(AcquireError::Io {
                path: path.to_path_buf(),
                err: err.into(),
            }),
        }
    }
}

/// Read the holder's PID from `file`, retrying briefly to absorb the small
/// window between the holder's `flock()` succeeding and its PID write landing
/// on disk. Returns `None` only if every attempt produced an empty or
/// unparseable file.
fn read_holder_pid_with_retry(file: &mut File) -> Option<u32> {
    for attempt in 0..PID_READ_ATTEMPTS {
        if let Some(pid) = read_holder_pid_once(file) {
            return Some(pid);
        }
        if attempt + 1 < PID_READ_ATTEMPTS {
            thread::sleep(PID_READ_RETRY_DELAY);
        }
    }
    None
}

fn read_holder_pid_once(file: &mut File) -> Option<u32> {
    let mut buf = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut buf).ok()?;
    buf.trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn acquire_writes_own_pid_to_lockfile() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");

        let _lock = SingleInstanceLock::acquire(&path).expect("first acquire should succeed");
        let contents = fs::read_to_string(&path).unwrap();
        let written: u32 = contents.trim().parse().expect("pid should parse");
        assert_eq!(
            written,
            std::process::id(),
            "lockfile should contain the holder's PID"
        );
    }

    #[test]
    fn second_acquire_same_path_reports_running_with_pid() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");

        let first = SingleInstanceLock::acquire(&path).expect("first acquire should succeed");
        let err = SingleInstanceLock::acquire(&path)
            .expect_err("second acquire should fail while first is held");

        match err {
            AcquireError::AlreadyRunning {
                pid: Some(pid),
                path: reported_path,
            } => {
                assert_eq!(pid, std::process::id());
                assert_eq!(reported_path, path);
            }
            other => panic!("expected AlreadyRunning with PID, got {other:?}"),
        }

        drop(first);
    }

    #[test]
    fn releasing_first_lock_allows_second_acquire() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");

        let first = SingleInstanceLock::acquire(&path).expect("first acquire");
        drop(first);

        let _second = SingleInstanceLock::acquire(&path)
            .expect("second acquire should succeed after first is dropped");
    }

    #[test]
    fn stale_pid_in_preexisting_lockfile_is_overwritten_on_acquire() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");

        // Simulate a lockfile left over from a prior crashed instance: the
        // file exists with an old PID but no one holds the flock.
        fs::write(&path, "99999\n").unwrap();

        let _lock = SingleInstanceLock::acquire(&path).expect("should take over stale lockfile");
        let contents = fs::read_to_string(&path).unwrap();
        let written: u32 = contents.trim().parse().expect("pid should parse");
        assert_eq!(written, std::process::id());
    }

    #[test]
    fn acquire_in_missing_directory_returns_io_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does_not_exist").join("dux.lock");

        let err =
            SingleInstanceLock::acquire(&path).expect_err("acquire in missing dir should fail");
        assert!(matches!(err, AcquireError::Io { .. }));
    }

    #[test]
    fn display_message_has_no_leading_whitespace_on_wrapped_lines() {
        let err = AcquireError::AlreadyRunning {
            pid: Some(12345),
            path: PathBuf::from("/tmp/dux/dux.lock"),
        };
        let msg = format!("{err}");

        // Each visible line must start at column 0. Splitting on `\n` and
        // inspecting the bytes directly catches any regression to
        // `\<newline><whitespace>` continuation style, which would leave a
        // leading space on the wrapped lines.
        let lines: Vec<&str> = msg.split('\n').collect();
        assert_eq!(
            lines.len(),
            3,
            "message should be exactly three lines, got: {msg:?}"
        );
        assert_eq!(
            lines[0],
            "Another dux instance is already running (PID 12345)."
        );
        assert!(
            lines[1].starts_with("Close it before starting a new one,"),
            "line 2 should start at column 0, got: {:?}",
            lines[1]
        );
        assert_eq!(lines[2], "Lockfile: /tmp/dux/dux.lock");

        for (i, line) in lines.iter().enumerate() {
            assert!(
                !line.starts_with(' '),
                "line {i} must not start with whitespace: {line:?}"
            );
        }
    }

    #[test]
    fn display_message_without_pid_keeps_unknown_explanation() {
        let err = AcquireError::AlreadyRunning {
            pid: None,
            path: PathBuf::from("/tmp/dux/dux.lock"),
        };
        let msg = format!("{err}");
        assert!(msg.contains("PID unknown"));
        assert!(msg.contains("/tmp/dux/dux.lock"));
        for (i, line) in msg.split('\n').enumerate() {
            assert!(
                !line.starts_with(' '),
                "line {i} must not start with whitespace: {line:?}"
            );
        }
    }

    #[test]
    fn pid_read_retries_until_holder_finishes_writing() {
        // Simulate the race window between `flock()` winning and the PID
        // write landing: the file starts empty, a concurrent writer drops a
        // valid PID after a barrier is released. The barrier makes the test
        // deterministic regardless of scheduler jitter — no wall-clock
        // sleeps are used to coordinate the two threads.
        use std::sync::{Arc, Barrier};

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "").unwrap();

        // The barrier gates the writer: it won't write until the main
        // thread has started the retry loop (first attempt sees empty,
        // then the barrier releases the writer for subsequent attempts).
        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            fs::write(&writer_path, "7777\n").unwrap();
        });

        // Open the file for reading, then kick off the retry. On the first
        // read, the file is empty. The barrier unblocks the writer and
        // subsequent retries see the PID.
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        // Release the writer just before entering the retry loop.
        barrier.wait();
        let pid = read_holder_pid_with_retry(&mut file);
        writer.join().unwrap();

        assert_eq!(
            pid,
            Some(7777),
            "retry loop should eventually observe the holder's PID"
        );
    }

    #[test]
    fn pid_read_gives_up_and_returns_none_when_file_stays_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "").unwrap();

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        assert_eq!(read_holder_pid_with_retry(&mut file), None);
    }
}
