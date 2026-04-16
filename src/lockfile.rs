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

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

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
            Self::AlreadyRunning {
                pid: Some(pid),
                path,
            } => write!(
                f,
                "Another dux instance is already running (PID {pid}).\n\
                 Close it before starting a new one, or set DUX_HOME to a \
                 different directory to run a separate workspace.\n\
                 Lockfile: {}",
                path.display()
            ),
            Self::AlreadyRunning { pid: None, path } => write!(
                f,
                "Another dux instance is already running (PID unknown — lockfile is empty or unreadable).\n\
                 Close it before starting a new one, or set DUX_HOME to a \
                 different directory to run a separate workspace.\n\
                 Lockfile: {}",
                path.display()
            ),
            Self::Io { path, err } => {
                write!(f, "Failed to acquire lockfile at {}: {err}", path.display())
            }
        }
    }
}

impl std::error::Error for AcquireError {}

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

        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
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
                // can show an actionable error.
                let mut buf = String::new();
                let _ = file.seek(SeekFrom::Start(0));
                let pid = file
                    .read_to_string(&mut buf)
                    .ok()
                    .and_then(|_| buf.trim().parse::<u32>().ok());
                Err(AcquireError::AlreadyRunning {
                    pid,
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
    fn display_message_mentions_pid_and_path() {
        let err = AcquireError::AlreadyRunning {
            pid: Some(12345),
            path: PathBuf::from("/tmp/dux/dux.lock"),
        };
        let msg = format!("{err}");
        assert!(msg.contains("12345"), "message should include the PID");
        assert!(
            msg.contains("/tmp/dux/dux.lock"),
            "message should include the lockfile path"
        );
        assert!(
            msg.to_lowercase().contains("already running"),
            "message should explain the situation"
        );
    }
}
