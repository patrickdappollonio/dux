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
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

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
    /// A socket at the path answers: another process is serving on it.
    InUse { path: PathBuf },
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

    /// A handle on the one bound listener, for a core to serve. Every core
    /// serves a clone, so the socket stays bound while none is serving and
    /// connections wait in its backlog.
    pub fn listener(&self) -> io::Result<UnixListener> {
        self.listener.try_clone()
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let ours = std::fs::symlink_metadata(&self.path)
            .is_ok_and(|meta| (meta.dev(), meta.ino()) == self.identity);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Remove a socket nobody serves on any more. A path that is not a socket, or
/// a socket that still answers, is refused rather than removed.
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
    if UnixStream::connect(path).is_ok() {
        return Err(Unavailable::InUse {
            path: path.to_path_buf(),
        });
    }
    std::fs::remove_file(path).map_err(|err| Unavailable::Io {
        path: path.to_path_buf(),
        step: "removing the leftover socket",
        err,
    })
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

/// Bind the control socket at `path` for the holder of `lock`, the one step
/// every way dux starts takes once it holds the lock and has read the config.
/// Answers the warning to show when dux has to run without the socket, already
/// written to `dux.log`; the lock file carries the reason either way.
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

/// The user id of this process: the only user a control socket serves.
pub fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}
