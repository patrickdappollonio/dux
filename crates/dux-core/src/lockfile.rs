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
//! The lockfile also contains the holder's PID, written after the lock is
//! acquired, so a colliding launch can tell the user which process to close.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::control_socket::{ControlSocket, Unavailable};
use crate::io_retry::retry_on_interrupt_errno;

/// Exclusive single-instance lock on the dux config directory.
///
/// The file handle is kept open for the lifetime of this value. The kernel
/// releases the advisory lock when the file descriptor is closed, including
/// on process exit via `SIGKILL` or crash, so stale lockfiles never block a
/// future launch. Only a live peer actively holding the lock will.
///
/// The lock also owns the control socket once [`Self::open_control_socket`]
/// bound it, which is what makes "only the lock holder binds the socket" a
/// fact of the types rather than of the call sites.
#[derive(Debug)]
pub struct SingleInstanceLock {
    file: File,
    control_socket: ControlSocketState,
}

/// What the lock holder knows about its control socket, and so what its lock
/// file says about it.
#[derive(Debug)]
enum ControlSocketState {
    NotOpened,
    Open(ControlSocket),
    Unavailable(String),
}

impl Drop for SingleInstanceLock {
    fn drop(&mut self) {
        // A clean exit takes the socket file and its line with it, before the
        // lock is let go, so the next holder never finds a socket of ours.
        if !matches!(self.control_socket, ControlSocketState::NotOpened) {
            self.control_socket = ControlSocketState::NotOpened;
            self.rewrite();
        }
        // The kernel also releases the advisory lock when the file
        // descriptor closes, but explicitly unlocking here makes the
        // handoff point deterministic for tests and avoids depending on
        // close-side timing alone.
        let _ = retry_on_interrupt_errno(|| flock(&self.file, FlockOperation::Unlock));
    }
}

impl SingleInstanceLock {
    /// Bind the control socket at `path` and say so in the lock file, as
    /// `control-socket=<path>`; when it cannot be bound, say why instead, as
    /// `control-socket-unavailable=<reason>`. A dead socket at the path is
    /// replaced; anything else there is left alone.
    pub fn open_control_socket(&mut self, path: &Path) -> Result<(), Unavailable> {
        let result = match ControlSocket::bind(path) {
            Ok(socket) => {
                self.control_socket = ControlSocketState::Open(socket);
                Ok(())
            }
            Err(reason) => {
                self.control_socket = ControlSocketState::Unavailable(reason.to_string());
                Err(reason)
            }
        };
        self.rewrite();
        result
    }

    /// The bound control socket, when there is one.
    pub fn control_socket(&self) -> Option<&ControlSocket> {
        match &self.control_socket {
            ControlSocketState::Open(socket) => Some(socket),
            ControlSocketState::NotOpened | ControlSocketState::Unavailable(_) => None,
        }
    }

    /// Replace the lock file's text with what this holder knows now.
    /// Best-effort like the first write: the lock itself is the `flock`.
    fn rewrite(&mut self) {
        let mut text = lock_file_text(std::process::id());
        match &self.control_socket {
            ControlSocketState::NotOpened => {}
            ControlSocketState::Open(socket) => {
                text.push_str(&format!(
                    "{CONTROL_SOCKET_PREFIX}{}\n",
                    socket.path().display()
                ));
            }
            ControlSocketState::Unavailable(reason) => {
                text.push_str(&format!("{CONTROL_SOCKET_UNAVAILABLE_PREFIX}{reason}\n"));
            }
        }
        let _ = self.file.set_len(0);
        let _ = self.file.seek(SeekFrom::Start(0));
        let _ = self.file.write_all(text.as_bytes());
        let _ = self.file.flush();
    }
}

/// Result of attempting to acquire the single-instance lock.
#[derive(Debug)]
pub enum AcquireError {
    /// Another live dux process already holds the lock. `pid` is the holder's
    /// PID as read from the lockfile. `None` if the file was empty or its
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
                        "Another dux instance is already running (PID unknown: the lockfile is empty or unreadable)."
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

/// Bounded retry parameters for reading the holder's PID on contention.
///
/// Two race windows the retry loop must absorb:
///
/// 1. **Empty file** - the winner's `flock()` returned but its PID write has not
///    landed yet, so the file is empty or truncated.
/// 2. **Stale PID** - the lockfile held a PID from a now-dead process and the
///    winner is mid-overwrite, so the loser reads the old bytes.
///
/// The loop therefore waits for two consecutive reads returning the SAME PID; a
/// changing value means the winner is still writing. If the budget runs out
/// without two reads agreeing, the last observed PID is returned as a best-effort
/// fallback. The cost is one extra 2ms sleep when the winner already wrote.
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
                let _ = file.write_all(lock_file_text(std::process::id()).as_bytes());
                let _ = file.flush();
                Ok(Self {
                    file,
                    control_socket: ControlSocketState::NotOpened,
                })
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

/// Production entry point: reads the holder's PID with the default retry
/// budget ([`PID_READ_ATTEMPTS`] × [`PID_READ_RETRY_DELAY`]).
fn read_holder_pid_with_retry(file: &mut File) -> Option<u32> {
    read_holder_pid(file, PID_READ_ATTEMPTS, PID_READ_RETRY_DELAY)
}

/// Read the holder's PID from `file`, preferring a value confirmed by two
/// consecutive identical reads. This absorbs both the "empty file" window
/// (winner hasn't written yet) and the "stale PID" window (winner is
/// mid-overwrite of a leftover PID from a dead process).
///
/// If two consecutive reads agree within `attempts`, that value is returned
/// immediately. If the budget is exhausted without agreement, the last
/// successfully parsed PID is returned as a best-effort fallback:
/// reporting a potentially-stale PID is more useful than `None`. Returns
/// `None` only if no attempt produced a parseable value at all.
///
/// The retry parameters are explicit so tests can use generous budgets that stay
/// deterministic under load; production uses [`PID_READ_ATTEMPTS`] and
/// [`PID_READ_RETRY_DELAY`].
fn read_holder_pid(file: &mut File, attempts: usize, delay: Duration) -> Option<u32> {
    let mut last_pid: Option<u32> = None;
    for attempt in 0..attempts {
        if let Some(pid) = read_holder_pid_once(file) {
            if last_pid == Some(pid) {
                // Two consecutive reads agree: the PID is stable.
                return Some(pid);
            }
            last_pid = Some(pid);
        }
        if attempt + 1 < attempts {
            thread::sleep(delay);
        }
    }
    // Return the last observed PID even without a confirming second read.
    // This is the best we can do after exhausting retries.
    last_pid
}

fn read_holder_pid_once(file: &mut File) -> Option<u32> {
    let mut buf = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut buf).ok()?;
    LockFileContents::parse(&buf).pid
}

/// The line a dux adds to its lock file, after its PID, once it handles the
/// reload signal (see [`crate::reload_signal`]). `dux config set` signals
/// only a holder that wrote it: a dux from before the reload signal has no
/// handler, and the signal's default action would terminate it.
pub const RELOAD_SIGNAL_MARKER: &str = "reload-signal=usr1";

/// What this process writes into the lock file it holds: its PID on the
/// first line, as every reader expects, then the reload-signal marker when
/// the reload handler is already installed (never before: a marker the
/// process cannot yet honour would invite the signal that kills it).
fn lock_file_text(pid: u32) -> String {
    if crate::reload_signal::is_installed() {
        format!("{pid}\n{RELOAD_SIGNAL_MARKER}\n")
    } else {
        format!("{pid}\n")
    }
}

/// The line a dux adds to its lock file once its control socket is bound,
/// followed by the socket's path.
pub const CONTROL_SOCKET_PREFIX: &str = "control-socket=";

/// The line a dux adds instead when it could not bind its control socket,
/// followed by the reason.
pub const CONTROL_SOCKET_UNAVAILABLE_PREFIX: &str = "control-socket-unavailable=";

/// A lock file's contents as written by any dux: the holder's PID on the
/// first line (the whole file, for a dux from before the marker), whether a
/// later line says it handles the reload signal, and where its control socket
/// is or why it has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockFileContents {
    pub pid: Option<u32>,
    pub handles_reload_signal: bool,
    pub control_socket: Option<PathBuf>,
    pub control_socket_unavailable: Option<String>,
}

impl LockFileContents {
    pub fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let pid = lines
            .next()
            .and_then(|line| line.trim().parse::<u32>().ok());
        let mut contents = Self {
            pid,
            ..Self::default()
        };
        for line in lines {
            if line.trim() == RELOAD_SIGNAL_MARKER {
                contents.handles_reload_signal = true;
            } else if let Some(reason) = line.strip_prefix(CONTROL_SOCKET_UNAVAILABLE_PREFIX) {
                contents.control_socket_unavailable = Some(reason.to_string());
            } else if let Some(path) = line.strip_prefix(CONTROL_SOCKET_PREFIX) {
                contents.control_socket = Some(PathBuf::from(path));
            }
        }
        contents
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn acquire_with_retry(
        path: &Path,
        attempts: usize,
        delay: Duration,
    ) -> Result<SingleInstanceLock, AcquireError> {
        for attempt in 0..attempts {
            match SingleInstanceLock::acquire(path) {
                Ok(lock) => return Ok(lock),
                Err(AcquireError::AlreadyRunning { .. }) if attempt + 1 < attempts => {
                    thread::sleep(delay);
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("loop returns on success or final error")
    }

    #[test]
    fn acquire_writes_own_pid_and_the_control_socket_to_the_lockfile() {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");

        let mut lock = SingleInstanceLock::acquire(&path).expect("first acquire should succeed");
        let contents = fs::read_to_string(&path).unwrap();
        let written = LockFileContents::parse(&contents).pid;
        assert_eq!(
            written,
            Some(std::process::id()),
            "lockfile should contain the holder's PID"
        );
        assert!(
            contents.starts_with(&format!("{}\n", std::process::id())),
            "the PID is the whole first line: {contents:?}"
        );

        let socket = tmp.path().join("dux.sock");
        lock.open_control_socket(&socket)
            .expect("a short path in an owner-only folder binds");
        let meta = fs::symlink_metadata(&socket).unwrap();
        assert!(meta.file_type().is_socket());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.starts_with(&format!("{}\n", std::process::id())),
            "the PID stays the first line: {contents:?}"
        );
        assert!(
            contents.contains(&format!("control-socket={}\n", socket.display())),
            "{contents:?}"
        );
        std::os::unix::net::UnixStream::connect(&socket).expect("the socket accepts");
        drop(lock);

        let mut lock = SingleInstanceLock::acquire(&path).expect("acquire again");
        let long = tmp.path().join("x".repeat(120)).join("dux.sock");
        let warning = crate::control_socket::open(&mut lock, &long)
            .expect("a path past the platform limit starts dux without the socket");
        let contents = fs::read_to_string(&path).unwrap();
        let parsed = LockFileContents::parse(&contents);
        assert_eq!(parsed.control_socket, None, "{contents:?}");
        let reason = parsed
            .control_socket_unavailable
            .expect("the lock file says why");
        assert!(reason.contains("too long"), "{reason}");
        assert!(warning.contains(&reason), "{warning}");
        assert!(warning.contains("[server] control_socket"), "{warning}");
        assert!(!contents.contains("\ncontrol-socket="), "{contents:?}");
    }

    /// A clean exit takes the socket file and its line in the lock file with it.
    #[test]
    fn releasing_the_lock_removes_the_socket_and_its_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        let socket = tmp.path().join("dux.sock");
        let mut lock = SingleInstanceLock::acquire(&path).unwrap();
        lock.open_control_socket(&socket).unwrap();

        drop(lock);

        assert!(!socket.exists(), "the socket file is removed");
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(LockFileContents::parse(&contents).control_socket, None);
        assert!(!contents.contains("control-socket"), "{contents:?}");
    }

    /// The next lock holder replaces a socket a crashed dux left behind, and
    /// leaves alone anything at the path that is not a dead socket.
    #[test]
    fn a_leftover_socket_is_replaced_and_anything_else_is_left_alone() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        let socket = tmp.path().join("dux.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        let leftover = fs::symlink_metadata(&socket).unwrap().ino();

        let mut lock = SingleInstanceLock::acquire(&path).unwrap();
        lock.open_control_socket(&socket)
            .expect("a dead socket is replaced");
        assert_ne!(fs::symlink_metadata(&socket).unwrap().ino(), leftover);
        std::os::unix::net::UnixStream::connect(&socket).expect("the new socket accepts");
        drop(lock);

        let file = tmp.path().join("notes.txt");
        fs::write(&file, "keep me").unwrap();
        let mut lock = SingleInstanceLock::acquire(&path).unwrap();
        assert!(lock.open_control_socket(&file).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep me");

        let live = tmp.path().join("live.sock");
        let _serving = std::os::unix::net::UnixListener::bind(&live).unwrap();
        let serving_ino = fs::symlink_metadata(&live).unwrap().ino();
        assert!(lock.open_control_socket(&live).is_err());
        assert_eq!(fs::symlink_metadata(&live).unwrap().ino(), serving_ino);
    }

    /// A dux that loses the lock race cannot reach the socket the winner holds.
    #[test]
    fn the_loser_of_the_lock_never_touches_the_winners_socket() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        let socket = tmp.path().join("dux.sock");
        let mut winner = SingleInstanceLock::acquire(&path).unwrap();
        winner.open_control_socket(&socket).unwrap();
        let ino = fs::symlink_metadata(&socket).unwrap().ino();

        let lost = SingleInstanceLock::acquire(&path);
        assert!(matches!(lost, Err(AcquireError::AlreadyRunning { .. })));
        drop(lost);

        assert_eq!(fs::symlink_metadata(&socket).unwrap().ino(), ino);
        std::os::unix::net::UnixStream::connect(&socket).expect("still serving");
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            LockFileContents::parse(&contents).control_socket,
            Some(socket.clone())
        );
        drop(winner);
    }

    /// A lock file from before the marker is the PID alone; one from now
    /// may add the marker on a later line. Both read the same PID.
    #[test]
    fn a_lock_file_reads_with_and_without_the_reload_marker() {
        assert_eq!(
            LockFileContents::parse("1234"),
            LockFileContents {
                pid: Some(1234),
                handles_reload_signal: false,
                ..LockFileContents::default()
            }
        );
        assert_eq!(
            LockFileContents::parse(&format!("1234\n{RELOAD_SIGNAL_MARKER}\n")),
            LockFileContents {
                pid: Some(1234),
                handles_reload_signal: true,
                ..LockFileContents::default()
            }
        );
        assert_eq!(LockFileContents::parse("").pid, None);
        let with_socket = LockFileContents::parse(&format!(
            "1234\n{RELOAD_SIGNAL_MARKER}\ncontrol-socket=/home/me/.config/dux/dux.sock\n"
        ));
        assert_eq!(with_socket.pid, Some(1234));
        assert!(with_socket.handles_reload_signal);
        assert_eq!(
            with_socket.control_socket,
            Some(PathBuf::from("/home/me/.config/dux/dux.sock"))
        );
        assert_eq!(with_socket.control_socket_unavailable, None);
        let without = LockFileContents::parse(
            "1234\ncontrol-socket-unavailable=binding /x/dux.sock failed: Permission denied\n",
        );
        assert_eq!(without.control_socket, None);
        assert_eq!(
            without.control_socket_unavailable.as_deref(),
            Some("binding /x/dux.sock failed: Permission denied")
        );
    }

    /// The marker is written only by a process whose reload handler is in
    /// place.
    #[test]
    fn the_marker_follows_the_installed_handler() {
        crate::reload_signal::install().expect("install");
        assert_eq!(lock_file_text(7), format!("7\n{RELOAD_SIGNAL_MARKER}\n"));
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

        let _second = acquire_with_retry(&path, 20, Duration::from_millis(2))
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

    /// Generous retry budget for tests. The writer thread needs only
    /// microseconds; 50 × 20ms = 1s gives even the slowest CI scheduler
    /// ample room, eliminating timing-dependent flakiness.
    const TEST_ATTEMPTS: usize = 50;
    const TEST_DELAY: Duration = Duration::from_millis(20);

    #[test]
    fn pid_read_retries_until_holder_finishes_writing() {
        // Simulate the empty-file race: the file starts empty and a
        // concurrent writer drops a valid PID after a barrier is released.
        // The barrier coordinates ordering (writer won't run before the
        // retry loop starts). The generous test budget ensures the writer
        // is always scheduled within the retry window.
        use std::sync::{Arc, Barrier};

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "").unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            fs::write(&writer_path, "7777\n").unwrap();
        });

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        barrier.wait();
        let pid = read_holder_pid(&mut file, TEST_ATTEMPTS, TEST_DELAY);
        writer.join().unwrap();

        assert_eq!(
            pid,
            Some(7777),
            "retry loop should eventually observe the holder's PID"
        );
    }

    #[test]
    fn pid_read_requires_two_consecutive_agreeing_reads() {
        // When a stale PID sits in the lockfile, the first read parses
        // successfully. The retry loop must NOT accept it immediately:
        // it needs a second read to agree. Here the file changes between
        // reads: attempt 0 sees 9999, the writer overwrites to 7777, and
        // the loop eventually stabilises on 7777.
        use std::sync::{Arc, Barrier};

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "9999\n").unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            fs::write(&writer_path, "7777\n").unwrap();
        });

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        barrier.wait();
        let pid = read_holder_pid(&mut file, TEST_ATTEMPTS, TEST_DELAY);
        writer.join().unwrap();

        assert_eq!(
            pid,
            Some(7777),
            "retry loop should settle on the new PID, not the stale one"
        );
    }

    #[test]
    fn pid_read_accepts_stable_value_without_change() {
        // When the PID is already written and stable, two consecutive
        // reads agree and the loop returns on the second attempt.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "4242\n").unwrap();

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        assert_eq!(
            read_holder_pid(&mut file, PID_READ_ATTEMPTS, PID_READ_RETRY_DELAY),
            Some(4242)
        );
    }

    #[test]
    fn pid_read_gives_up_and_returns_none_when_file_stays_empty() {
        // Use minimal budget here: no writer thread, so every attempt
        // fails immediately and there's nothing to wait for.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("dux.lock");
        fs::write(&path, "").unwrap();

        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        assert_eq!(
            read_holder_pid(&mut file, PID_READ_ATTEMPTS, PID_READ_RETRY_DELAY),
            None
        );
    }
}
