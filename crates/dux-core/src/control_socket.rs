//! The control socket: a Unix socket in the config folder (by default) on
//! which every running dux serves its API to command-line clients on this
//! machine.
//!
//! Only the process holding `dux.lock` binds it, through
//! [`crate::lockfile::SingleInstanceLock::open_control_socket`], so a dux that
//! loses the lock race never touches the socket the winner serves. It is bound
//! once and kept for the whole process: every servicing core (the plain
//! terminal UI's, background serving's, the flip's and `dux server`'s) serves a
//! clone of the one listener, and a connection that arrives while the engine
//! moves between them waits in the listener's backlog for the next one.
//!
//! The socket file is owner-only and every accepted connection's peer user is
//! checked by whoever serves it, so the socket needs no password.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The longest socket path the platform accepts, in bytes. `sun_path` holds
/// 104 bytes on macOS and 108 on Linux, and the terminating NUL takes one.
pub const MAX_PATH_BYTES: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

/// Why dux runs without its control socket. Each reads as one line, because
/// it is written into the lock file as `control-socket-unavailable=<reason>`.
#[derive(Debug)]
pub enum Unavailable {
    /// The resolved path is longer than [`MAX_PATH_BYTES`].
    PathTooLong { path: PathBuf, bytes: usize },
    /// The path has a line break in it, which the lock file cannot carry.
    LineBreak,
    /// Something that is not a socket is at the path. dux never removes it.
    NotASocket { path: PathBuf },
    /// A socket at the path answers, hangs up on a connection, or was listened
    /// on by a process that is still running: a live process holds it.
    InUse { path: PathBuf },
    /// The socket's folder is not this user's alone, so somebody else could swap
    /// the file under dux between its steps.
    SharedFolder { folder: PathBuf },
    /// Connecting to a socket left at the path failed for a reason other than
    /// a refusal, so nothing proves it dead and dux leaves it alone.
    Unproven { path: PathBuf, err: io::Error },
    /// The bind, the removal of a dead socket or the mode change failed.
    Io {
        path: PathBuf,
        step: &'static str,
        err: io::Error,
    },
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathTooLong { path, bytes } => write!(
                f,
                "the path {} is too long: {bytes} bytes, and this system allows at most \
                 {MAX_PATH_BYTES}",
                path.display()
            ),
            Self::LineBreak => write!(f, "the path has a line break in it"),
            Self::NotASocket { path } => write!(
                f,
                "{} already exists and is not a socket, so dux leaves it alone",
                path.display()
            ),
            Self::InUse { path } => write!(
                f,
                "another process is already serving on {}",
                path.display()
            ),
            Self::SharedFolder { folder } => write!(
                f,
                "the folder {} is not yours alone (another user owns it or can write to it), \
                 so dux puts no socket there; point [server] control_socket at a folder only \
                 you can write to",
                folder.display()
            ),
            Self::Unproven { path, err } => {
                let err = err.to_string().replace(['\n', '\r'], " ");
                write!(
                    f,
                    "a socket is already at {} and dux could not tell whether anything still \
                     serves on it ({err}), so it leaves it alone; remove it or point \
                     [server] control_socket elsewhere",
                    path.display()
                )
            }
            Self::Io { path, step, err } => {
                // One line whatever the OS put in its message.
                let err = err.to_string().replace(['\n', '\r'], " ");
                write!(f, "{step} {} failed: {err}", path.display())
            }
        }
    }
}

impl std::error::Error for Unavailable {}

/// The bound control socket. Dropping it removes the socket file, when the
/// file at the path is still the one this bound.
#[derive(Debug)]
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
    /// The bound file's device and inode, so a drop never removes a socket
    /// somebody else bound at the same path since.
    identity: (u64, u64),
}

impl ControlSocket {
    /// Bind `path` owner-only, replacing a dead socket left there by a dux
    /// that did not exit cleanly.
    pub(crate) fn bind(path: &Path) -> Result<Self, Unavailable> {
        let bytes = path.as_os_str().len();
        if bytes > MAX_PATH_BYTES {
            return Err(Unavailable::PathTooLong {
                path: path.to_path_buf(),
                bytes,
            });
        }
        if path.as_os_str().as_encoded_bytes().contains(&b'\n') {
            return Err(Unavailable::LineBreak);
        }
        // Every step below names the path, which follows symlinks: only a private
        // folder keeps it from being swapped between them.
        if !in_private_folder(path) {
            return Err(Unavailable::SharedFolder {
                folder: folder_of(path),
            });
        }
        remove_dead_socket(path)?;
        let io = |step: &'static str| {
            move |err: io::Error| Unavailable::Io {
                path: path.to_path_buf(),
                step,
                err,
            }
        };
        let listener = UnixListener::bind(path).map_err(io("binding"))?;
        let socket = Self {
            listener,
            path: path.to_path_buf(),
            identity: std::fs::symlink_metadata(path)
                .map(|meta| (meta.dev(), meta.ino()))
                .map_err(io("reading"))?,
        };
        // On failure the drop removes the file again: a socket other users
        // might open is worse than no socket.
        crate::file_modes::make_socket_private(path).map_err(io("restricting"))?;
        Ok(socket)
    }

    /// Where the socket is bound.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A clone of the bound listener for a core to serve; the socket stays bound
    /// while no core serves, and connections wait in its backlog.
    pub fn listener(&self) -> io::Result<UnixListener> {
        self.listener.try_clone()
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        // The same rule as the bind: nothing is removed from a folder somebody
        // else could swap the file in, and only the socket this bound is.
        let ours = in_private_folder(&self.path)
            && std::fs::symlink_metadata(&self.path)
                .is_ok_and(|meta| (meta.dev(), meta.ino()) == self.identity);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The folder `path` is in.
fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Whether the folder `path` is in belongs to this user and neither its group
/// nor anyone else can write to it.
fn in_private_folder(path: &Path) -> bool {
    std::fs::metadata(folder_of(path))
        .is_ok_and(|meta| meta.uid() == current_uid() && meta.mode() & 0o022 == 0)
}

/// Remove a socket nobody serves on any more. A path that is not a socket, or
/// a socket a live process still holds, is refused rather than removed.
fn remove_dead_socket(path: &Path) -> Result<(), Unavailable> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(Unavailable::Io {
                path: path.to_path_buf(),
                step: "reading",
                err,
            });
        }
    };
    if !meta.file_type().is_socket() {
        return Err(Unavailable::NotASocket {
            path: path.to_path_buf(),
        });
    }
    match UnixStream::connect(path) {
        Ok(mut stream) => {
            // A live dux that has bound its socket but is not serving it yet
            // (starting up, or handing it between cores) is silent too; the
            // process that listened on it being alive is what tells it apart.
            // Its pid is read before the probe because macOS stops answering
            // once the other end hangs up; a hang-up before this read is one
            // the probe then sees.
            let listener = listener_pid(&stream);
            if probe(&mut stream) != Probe::Silent
                || listener.is_some_and(crate::file_drop::process_can_answer)
            {
                return Err(Unavailable::InUse {
                    path: path.to_path_buf(),
                });
            }
            crate::logger::warn(&format!(
                "replacing the leftover control socket {}: it accepted a connection but no \
                 dux answered on it within {} ms and no live process listened on it, so it is a \
                 dead dux's listener that another process still holds open",
                path.display(),
                ANSWER_WAIT.as_millis()
            ));
        }
        // A refusal proves nothing listens there any more, as does the silence
        // handled above.
        Err(err) if err.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(err) => {
            return Err(Unavailable::Unproven {
                path: path.to_path_buf(),
                err,
            });
        }
    }
    std::fs::remove_file(path).map_err(|err| Unavailable::Io {
        path: path.to_path_buf(),
        step: "removing the leftover socket",
        err,
    })
}

/// How long a leftover socket that accepts a connection has to answer before
/// it counts as dead.
const ANSWER_WAIT: Duration = Duration::from_millis(500);

/// A request every serving dux answers on its control socket from
/// process-local data, with no engine round-trip.
const ANSWER_PROBE: &[u8] =
    b"GET /api/v1/build HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

/// What a leftover socket that accepted a connection did with it.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    /// It sent something back.
    Answered,
    /// It hung up or reset the connection, which takes a live process.
    Closed,
    /// Nothing came back within [`ANSWER_WAIT`].
    Silent,
}

/// Send `stream` a request and wait up to [`ANSWER_WAIT`] for what comes back.
///
/// A dux that died can leave its listener open in a child that inherited it
/// (on macOS a socket's close-on-exec flag is set after the socket is made, so
/// a process spawned in between keeps it), and the kernel then accepts
/// connections nobody will ever read or close. Silence is the only sign of
/// that: anything that answers or hangs up is a process looking at the socket.
fn probe(stream: &mut UnixStream) -> Probe {
    let deadline = Instant::now() + ANSWER_WAIT;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    if stream.set_write_timeout(Some(ANSWER_WAIT)).is_err() {
        return Probe::Closed;
    }
    // A write that fails means the other end is gone; the read below says so.
    let _ = stream.write_all(ANSWER_PROBE);
    let mut byte = [0u8; 1];
    loop {
        let wait = remaining();
        if wait.is_zero() || stream.set_read_timeout(Some(wait)).is_err() {
            return Probe::Silent;
        }
        match stream.read(&mut byte) {
            Ok(0) => return Probe::Closed,
            Ok(_) => return Probe::Answered,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Probe::Silent;
            }
            Err(_) => return Probe::Closed,
        }
    }
}

/// The process that called listen() on the socket `stream` is connected to,
/// even while nothing has accepted the connection: Linux copies the listener's
/// credentials, taken at listen(), into the connecting socket, and macOS hands
/// the embryonic connection the listener's last owner, the process that last
/// listened, accepted or polled on it. A process that merely inherited the
/// listener and never touched it is neither. A dead listener's pid that has
/// since been reused reads as alive, which leaves the socket alone. `None`
/// when it cannot be read.
#[cfg(target_os = "linux")]
fn listener_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes of the sizes passed, and
    // the descriptor is open for as long as `stream` is borrowed.
    let read = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(cred).cast::<libc::c_void>(),
            std::ptr::addr_of_mut!(len),
        )
    };
    (read == 0 && cred.pid > 0).then_some(cred.pid as u32)
}

/// The process that called listen() on the socket `stream` is connected to;
/// see the Linux version.
#[cfg(target_os = "macos")]
fn listener_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` are valid for writes of the sizes passed, and
    // the descriptor is open for as long as `stream` is borrowed.
    let read = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            std::ptr::addr_of_mut!(pid).cast::<libc::c_void>(),
            std::ptr::addr_of_mut!(len),
        )
    };
    (read == 0 && pid > 0).then_some(pid as u32)
}

/// The sentence a start without the socket shows on its status line and
/// writes to `dux.log`: what is missing, why, and the setting that fixes it.
pub fn unavailable_warning(reason: &Unavailable) -> String {
    format!(
        "dux is running without its control socket, so command-line clients cannot reach \
         it: {reason}. Set [server] control_socket in config.toml to a path dux can use, \
         then restart dux."
    )
}

/// Bind the control socket at `path` for the holder of `lock`. Answers the warning
/// to show when dux runs without it, already written to `dux.log`.
pub fn open(lock: &mut crate::lockfile::SingleInstanceLock, path: &Path) -> Option<String> {
    match lock.open_control_socket(path) {
        Ok(()) => {
            crate::logger::info(&format!(
                "serving command-line clients on the control socket {}",
                path.display()
            ));
            None
        }
        Err(reason) => {
            let warning = unavailable_warning(&reason);
            crate::logger::warn(&warning);
            Some(warning)
        }
    }
}

/// What a config reload says when it moved `[server] control_socket`, bound once
/// per process; `None` when the setting did not change.
pub fn moved_warning(
    prev: &crate::config::ServerConfig,
    next: &crate::config::ServerConfig,
) -> Option<String> {
    (prev.control_socket != next.control_socket).then(|| {
        "The [server] control_socket setting changed; dux binds its control socket when it \
         starts, so command-line clients keep using the old one until the next time dux starts."
            .to_string()
    })
}

/// The user id of this process: the only user a control socket serves.
pub fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}
