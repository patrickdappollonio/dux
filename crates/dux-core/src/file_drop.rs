//! Saving a file dropped onto an agent pane or a terminal pane in the web UI.
//!
//! # Why this saves a file instead of sending its bytes
//!
//! The obvious reading of "drop a file onto the terminal" is to push the file's
//! bytes into the child's input stream. That cannot work. No agent CLI reads a
//! file from its input stream: they take a PATH, or they read the clipboard of
//! the machine THEY run on, which for a browser user is the wrong computer.
//! Every terminal emulator whose source was read (Alacritty, kitty, WezTerm,
//! Ghostty, GNOME Terminal, Konsole, iTerm2) answers a drop by inserting the
//! PATH as text. So dux does what a terminal does: save the file, then let the
//! browser paste its path.
//!
//! This module owns the saving half. The pasting half is the browser's, over
//! its own already-gated PTY socket, and deliberately never happens here (see
//! `dux_web::file_drop_routes` for why routing it through the upload handler
//! would walk past the server's input-ownership gate).
//!
//! # What this module guarantees
//!
//! - **A dropped name is validated, never rewritten.** An accented or non-Latin
//!   name is kept exactly as the user had it. Rewriting names to "safe"
//!   characters destroys them, can collapse two distinct names into one, and can
//!   even produce `.` or `..`.
//! - **Creation is atomic and never overwrites.** The file is created relative
//!   to a PINNED DIRECTORY HANDLE, exclusively, refusing to follow a symlink. On
//!   a name clash the next candidate is tried, so two uploads of `shot.png` in
//!   the same second produce two files.
//! - **A symlink at any candidate fails the request.** Refusing to follow it is
//!   the safety property; quietly writing next to it would hide the fact that
//!   something unexpected is sitting there. A probe that FAILS is never read as
//!   "not a symlink"; it fails the request too.
//! - **A failed or abandoned write removes what it partly wrote.**
//! - **The whole path is checked, not just the name.** The name is the last
//!   component of a path that is what actually gets sent to the terminal, so the
//!   DIRECTORY is held to the same standard: absolute, valid UTF-8, free of
//!   control characters, and verified to still name the pinned directory. A
//!   folder holding a line feed or an escape byte would arrive at the line
//!   editor as something other than a path, and quoting does not protect that
//!   layer; a folder that is not valid UTF-8 would arrive full of replacement
//!   characters. Refusing the drop is better than saving a file the user cannot
//!   reference.
//! - **An unreadable process is a refusal, not a licence to write elsewhere.**
//!   Asking a live process where it is can fail because the process has GONE, or
//!   because dux is not allowed to look (a shell running as another user) or
//!   cannot make sense of what it sees. Only the first falls through to the next
//!   candidate directory. The rest refuse, because writing to the suspended
//!   parent's directory and then confidently naming it in a toast is worse than
//!   saying no. A process that has EXITED but has not been reaped counts as
//!   gone, even though its pid still answers signal 0; so does a member that
//!   vanished mid-scan. What may never count as gone is a process dux simply
//!   could not inspect, and a group scan carries that difference rather than
//!   returning a short list as if it were the whole truth.

use std::io::Read;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Mode, OFlags};

/// How many suffixed candidates are tried after the dropped name itself before
/// the request is refused. Each candidate carries a counter, so this is only
/// reached when 64 distinct names are all taken within the same second.
pub const MAX_COLLISION_ATTEMPTS: u32 = 64;

/// Byte limit assumed for a single path component when the filesystem will not
/// answer `pathconf(_PC_NAME_MAX)`. 255 is the value on ext4, APFS, XFS, btrfs
/// and every other filesystem dux is realistically dropped onto.
pub const FALLBACK_NAME_MAX_BYTES: usize = 255;

/// Why a dropped filename is unusable.
///
/// These are all REFUSALS, not repairs: the name the user dropped is the name
/// they get, or they are told why they cannot have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropNameError {
    /// No name at all.
    Empty,
    /// `.` or `..`, which name a directory rather than a file.
    DotSegment,
    /// Contains a path separator, so it is a path and not a name. This is also
    /// what refuses a traversing name like `../../etc/passwd`.
    Separator,
    /// Contains a NUL, which no filesystem can store and which truncates the
    /// name at the syscall boundary.
    NullByte,
    /// Contains an ASCII or Unicode control character. These are legal on most
    /// filesystems and are uniformly a bad idea: they cannot be typed back and
    /// they scramble any terminal that prints the path.
    Control,
    /// Longer than this filesystem allows. The limit is in BYTES, which is what
    /// the kernel counts.
    TooLong { limit: usize },
}

impl std::fmt::Display for DropNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the file has no name"),
            Self::DotSegment => write!(f, "\".\" and \"..\" are not file names"),
            Self::Separator => write!(f, "a file name cannot contain a path separator"),
            Self::NullByte => write!(f, "the file name contains a null byte"),
            Self::Control => write!(f, "the file name contains a control character"),
            Self::TooLong { limit } => write!(
                f,
                "the file name is longer than this filesystem allows ({limit} bytes)"
            ),
        }
    }
}

/// Why a folder's own path cannot be handed to a terminal.
///
/// The path is what gets SENT, and the terminal library rewrites, re-encodes or
/// simply obeys parts of it long before any shell tokenisation happens. So a
/// path that cannot survive that trip is refused rather than saved against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreportablePath {
    /// Not absolute. A relative path can be read as an option by whatever is
    /// listening, and it names a different file from a different directory.
    NotAbsolute,
    /// Not valid UTF-8. Converting it lossily produces replacement characters,
    /// and the result names nothing at all.
    NotUtf8,
    /// Holds a control character. A line feed becomes a CARRIAGE RETURN on the
    /// way through the terminal, which submits rather than typing; an escape
    /// byte is obeyed by the line editor or by whatever full-screen program is
    /// running. Neither is something quoting can fix.
    Control,
    /// The path no longer names the directory that was pinned. The folder was
    /// removed, or it is only reachable under this name from somewhere else.
    Unverifiable,
}

impl std::fmt::Display for UnreportablePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute => write!(f, "its path is not absolute"),
            Self::NotUtf8 => write!(f, "its path is not valid UTF-8"),
            Self::Control => write!(f, "its path contains a control character"),
            Self::Unverifiable => {
                write!(f, "its path no longer names that folder")
            }
        }
    }
}

/// Why a destination folder cannot be used.
///
/// Every variant is a REFUSAL. None of them may be quietly downgraded into
/// "write somewhere else instead": the toast names a folder, and naming the
/// wrong one is the failure this type exists to prevent.
#[derive(Debug)]
pub enum DropDirError {
    /// The folder could not be opened, or a live process could not be asked
    /// where it is.
    Io(std::io::Error),
    /// The folder was pinned, but its path cannot be sent to a terminal without
    /// changing what it names. See [`UnreportablePath`].
    Unreportable(UnreportablePath),
    /// The process runs in a different MOUNT NAMESPACE, so the file would be
    /// saved in the right place under a name that means something else (or
    /// nothing) in the terminal's own view. Refused until a name that is
    /// verifiably correct in that view can be produced.
    ForeignMountNamespace,
    /// The foreground process group still owns the terminal, its leader has
    /// exited, and dux could not establish where the surviving members are:
    /// either this platform cannot enumerate a process group at all, or the
    /// enumeration could not inspect every process and so its emptiness proves
    /// nothing. Guessing the shell's directory instead would be wrong exactly
    /// when a job is running.
    ForegroundGroupUnreadable,
}

impl std::fmt::Display for DropDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not open the destination folder: {e}"),
            Self::Unreportable(why) => write!(
                f,
                "the file was not saved, because {why}, so the path could not be \
                 sent to the terminal"
            ),
            Self::ForeignMountNamespace => write!(
                f,
                "that terminal runs in a different mount namespace, so dux cannot \
                 name the folder in a way the terminal would understand"
            ),
            Self::ForegroundGroupUnreadable => write!(
                f,
                "the program running in that terminal has exited and dux could not \
                 tell where its replacement is; run the drop again"
            ),
        }
    }
}

impl std::error::Error for DropDirError {}

impl From<std::io::Error> for DropDirError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The path of a destination folder, checked for everything that would stop it
/// naming that folder once it reaches a terminal.
///
/// Pure, so the cases that cannot be constructed on a filesystem (a relative
/// answer from a lookup that should only ever produce absolute ones) are still
/// pinned by a test.
pub fn check_reportable_dir_path(path: &Path) -> Result<&str, UnreportablePath> {
    if !path.is_absolute() {
        return Err(UnreportablePath::NotAbsolute);
    }
    let text = path.to_str().ok_or(UnreportablePath::NotUtf8)?;
    if text.chars().any(char::is_control) {
        return Err(UnreportablePath::Control);
    }
    Ok(text)
}

/// Why saving a dropped file failed, in the words the user is shown.
#[derive(Debug)]
pub enum DropSaveError {
    /// The dropped name is unusable. See [`DropNameError`].
    Name(DropNameError),
    /// Something already at this name is a SYMLINK. The request fails rather
    /// than being renamed around, for ANY candidate name and not only the first.
    Symlink { name: String },
    /// Every candidate name was taken, or the name is so long that no suffixed
    /// candidate fits within the filesystem's limit. Fails deterministically
    /// rather than looping.
    NoUsableName,
    /// The destination directory could not be opened or written.
    Io(std::io::Error),
}

impl std::fmt::Display for DropSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(e) => write!(f, "{e}"),
            Self::Symlink { name } => write!(
                f,
                "a symlink named \"{name}\" is already there, and dux will not write through it"
            ),
            Self::NoUsableName => {
                write!(f, "could not find a free name for this file in that folder")
            }
            Self::Io(e) => write!(f, "could not write the file: {e}"),
        }
    }
}

impl std::error::Error for DropSaveError {}

impl From<std::io::Error> for DropSaveError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A file that was saved, and under what name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedDrop {
    /// The absolute path the file now lives at. This is what gets pasted.
    pub path: PathBuf,
    /// The name it was actually saved under.
    pub saved_name: String,
    /// True when a collision forced a different name from the dropped one. The
    /// browser reports the pair (original, saved) rather than a count, because a
    /// count says something changed without saying what the file is now called.
    pub renamed: bool,
}

/// Whether `name` is usable as a single filename on a filesystem whose
/// per-component limit is `name_max` bytes.
///
/// Validation, never repair. See [`DropNameError`] for each refusal.
pub fn validate_drop_name(name: &str, name_max: usize) -> Result<(), DropNameError> {
    if name.is_empty() {
        return Err(DropNameError::Empty);
    }
    if name == "." || name == ".." {
        return Err(DropNameError::DotSegment);
    }
    if name.contains('\0') {
        return Err(DropNameError::NullByte);
    }
    if name.contains('/') {
        return Err(DropNameError::Separator);
    }
    if name.chars().any(char::is_control) {
        return Err(DropNameError::Control);
    }
    if name.len() > name_max {
        return Err(DropNameError::TooLong { limit: name_max });
    }
    Ok(())
}

/// Split a filename into its stem and its extension (including the dot).
///
/// A LEADING dot is not an extension separator: `.env` is a hidden file whose
/// whole name is the stem, not an empty stem with an `.env` extension. Anything
/// else splits at the LAST dot, so `archive.tar.gz` keeps `.gz`, which is what a
/// viewer uses to decide the file is a gzip.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(idx) => (&name[..idx], &name[idx..]),
    }
}

/// The longest prefix of `s` that fits in `max_bytes` and ends on a character
/// boundary.
///
/// Byte truncation is what would panic (or produce mojibake) on an accented or
/// CJK name, and those are exactly the names this module promises to preserve.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The `counter`th collision candidate for `name`, or `None` when no candidate
/// can fit within `name_max` bytes.
///
/// The suffix carries BOTH a timestamp and a counter: a timestamp alone is not
/// enough, because two uploads can land in the same second.
///
/// The limit is in bytes, and a name can pass validation and then exceed the
/// limit once a suffix is appended, so room for the suffix is reserved UP FRONT
/// and the original is truncated at a character boundary, keeping the extension
/// so the file is still recognized for what it is. When even that cannot fit,
/// the extension is shortened too, and if there is still no room for a single
/// character of stem this returns `None` and the request fails deterministically
/// rather than looping or producing something unusable.
pub fn collision_candidate(
    name: &str,
    stamp: &str,
    counter: u32,
    name_max: usize,
) -> Option<String> {
    let suffix = format!("-{stamp}-{counter}");
    // One byte of stem is the minimum that makes a candidate worth having.
    if suffix.len() + 1 > name_max {
        return None;
    }
    let (stem_full, ext_full) = split_extension(name);
    let mut ext = truncate_at_char_boundary(ext_full, name_max - suffix.len() - 1);
    loop {
        let stem = truncate_at_char_boundary(stem_full, name_max - suffix.len() - ext.len());
        if !stem.is_empty() {
            return Some(format!("{stem}{suffix}{ext}"));
        }
        if ext.is_empty() {
            return None;
        }
        // The stem's first character is wider than the room left. Give a
        // character of the extension back and try again.
        ext = truncate_at_char_boundary(ext, ext.len() - 1);
    }
}

/// A PINNED handle to the directory a dropped file will be written into.
///
/// The handle, not the path, is the thing that matters. Resolving a directory to
/// text and then reopening it by that text leaves a window in which the name can
/// be pointed somewhere else; holding the open handle and creating relative to it
/// closes that window. It also makes the live working directory of a shell
/// reachable at all: on Linux `/proc/<pid>/cwd` IS the directory, and opening it
/// as a directory is what pins it.
pub struct DropDir {
    fd: OwnedFd,
    path: PathBuf,
}

impl std::fmt::Debug for DropDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropDir").field("path", &self.path).finish()
    }
}

impl DropDir {
    /// Pin `dir` by opening it as a directory, and refuse it outright unless its
    /// own path can be sent to a terminal unchanged.
    ///
    /// The check happens HERE, before a single byte is written, because the path
    /// is the deliverable: a file saved into a folder dux cannot name is a file
    /// the user cannot reference, and it is better to refuse than to leave one
    /// behind and report a path that names something else.
    pub fn open(dir: &Path) -> Result<Self, DropDirError> {
        let fd = rustix::fs::open(
            dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| DropDirError::Io(e.into()))?;
        let path = reportable_path_of(&fd, dir)?;
        Ok(Self { fd, path })
    }

    /// The absolute path of the pinned directory.
    ///
    /// On Linux this is read back from the HANDLE (`/proc/self/fd/<n>`), so it
    /// names the directory actually pinned rather than whatever the caller
    /// passed. That is what makes `/proc/<pid>/cwd` report the shell's real
    /// directory instead of the literal `/proc` path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The filesystem's byte limit for a single name in this directory, asked of
    /// the filesystem rather than assumed. Falls back to
    /// [`FALLBACK_NAME_MAX_BYTES`] when it will not answer.
    pub fn name_max(&self) -> usize {
        // SAFETY: `fpathconf` only reads properties of the fd, which we own for
        // the duration of the call.
        let raw = unsafe { libc::fpathconf(self.fd.as_raw_fd(), libc::_PC_NAME_MAX) };
        if raw > 0 {
            raw as usize
        } else {
            FALLBACK_NAME_MAX_BYTES
        }
    }

    /// Create `name` in this directory, exclusively and without following a
    /// symlink.
    ///
    /// `O_CREAT | O_EXCL` is the whole point: testing whether a name is free and
    /// then creating it is racy against a second upload and against a symlink
    /// being swapped in between the two steps. `O_NOFOLLOW` is belt and braces
    /// (`O_EXCL` already refuses an existing symlink) and documents the intent.
    fn create_exclusive(&self, name: &str) -> rustix::io::Result<std::fs::File> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o644),
        )?;
        Ok(std::fs::File::from(fd))
    }

    /// Whether `name` in this directory is a symlink. Asked only after an
    /// `EEXIST`, to tell "something is in the way, try the next name" apart from
    /// "a symlink is in the way, refuse the request".
    ///
    /// The failure is PROPAGATED rather than folded into `false`. Reading a
    /// failed probe as "not a symlink" turns an I/O or permission problem into
    /// an ordinary collision and quietly advances to the next name, which
    /// contradicts the promise that a symlink at ANY candidate fails the whole
    /// request: the one probe that could have seen it is the one that failed.
    fn entry_is_symlink(&self, name: &str) -> rustix::io::Result<bool> {
        let stat = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        Ok(
            rustix::fs::FileType::from_raw_mode(stat.st_mode as rustix::fs::RawMode)
                == rustix::fs::FileType::Symlink,
        )
    }

    /// Remove `name` from this directory, ignoring failure. Used to clean up
    /// after a write that failed or was abandoned part-way.
    fn remove(&self, name: &str) {
        let _ = rustix::fs::unlinkat(&self.fd, name, AtFlags::empty());
    }
}

/// The path a pinned directory can be reported under, or why it has none.
///
/// Linux reads it back from the HANDLE (`/proc/self/fd`), which is what makes
/// `/proc/<pid>/cwd` report the shell's real directory rather than the literal
/// `/proc` path. There is deliberately NO fallback to `opened_as` there: that
/// value IS the opaque `/proc/<pid>/cwd` handle path for a terminal, and handing
/// it out would name a directory that stops existing the moment the process
/// does. Elsewhere there is no way to ask a handle, and `opened_as` is always a
/// real path (a worktree, a spawn directory, or `lsof`'s answer), so it is used.
///
/// Whichever path comes out is then held to [`check_reportable_dir_path`] AND
/// checked to still name the pinned directory, so a folder that was removed
/// under us (Linux answers `"/gone (deleted)"`) or that is only reachable under
/// this name from another view is refused rather than reported.
fn reportable_path_of(fd: &OwnedFd, opened_as: &Path) -> Result<PathBuf, DropDirError> {
    #[cfg(target_os = "linux")]
    let path = {
        let _ = opened_as;
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()))
            .map_err(DropDirError::Io)?
    };
    #[cfg(not(target_os = "linux"))]
    let path = opened_as.to_path_buf();

    check_reportable_dir_path(&path).map_err(DropDirError::Unreportable)?;
    if !path_names_the_pinned_directory(fd, &path) {
        return Err(DropDirError::Unreportable(UnreportablePath::Unverifiable));
    }
    Ok(path)
}

/// Whether resolving `path` from dux's own view lands on exactly the directory
/// `fd` is holding open, compared by device and inode.
///
/// This is the check that makes the reported path a FACT rather than a hope. It
/// is also what catches a directory removed while it was pinned: the handle
/// still writes, and Linux still answers a path, but that path names nothing.
fn path_names_the_pinned_directory(fd: &OwnedFd, path: &Path) -> bool {
    let (Ok(pinned), Ok(named)) = (rustix::fs::fstat(fd), rustix::fs::stat(path)) else {
        return false;
    };
    pinned.st_dev == named.st_dev && pinned.st_ino == named.st_ino
}

/// What to do after an exclusive create reported `EEXIST` and the entry sitting
/// there was probed.
///
/// Pure, and split out because the interesting branches cannot be built on a
/// real filesystem without privileges: the same directory permission that would
/// make `statat` fail also makes the `openat` that produces `EEXIST` fail, so
/// the failing probe can only be reached by handing this function the failure.
#[derive(Debug, PartialEq, Eq)]
enum CollisionStep {
    /// A symlink is sitting there. The whole request fails, at any candidate.
    RefuseSymlink,
    /// An ordinary entry is in the way. Try the next candidate name.
    NextCandidate,
    /// The entry vanished between the create and the probe, so this candidate is
    /// free again. Retry the SAME one rather than advancing past a name nothing
    /// is using.
    RetrySameCandidate,
    /// The probe itself failed. Never read as "not a symlink": that would turn
    /// an I/O or permission failure into a silent rename.
    Failed(rustix::io::Errno),
}

fn collision_step(probe: rustix::io::Result<bool>) -> CollisionStep {
    match probe {
        Ok(true) => CollisionStep::RefuseSymlink,
        Ok(false) => CollisionStep::NextCandidate,
        Err(rustix::io::Errno::NOENT) => CollisionStep::RetrySameCandidate,
        Err(e) => CollisionStep::Failed(e),
    }
}

/// Save `bytes` into `dir` under `requested_name`, or the next free candidate.
///
/// `stamp` is the timestamp half of the collision suffix, passed in rather than
/// read from the clock so the naming is testable.
pub fn save_drop(
    dir: &DropDir,
    requested_name: &str,
    bytes: &[u8],
    stamp: &str,
) -> Result<SavedDrop, DropSaveError> {
    save_drop_from(dir, requested_name, &mut &bytes[..], stamp)
}

/// Save whatever `source` yields into `dir`. See [`save_drop`].
///
/// A read or write failure part-way through REMOVES the partial file: an
/// abandoned upload must leave nothing behind, or the folder fills with
/// zero-length rubble that the user did not put there and was never told about.
pub fn save_drop_from<R: Read>(
    dir: &DropDir,
    requested_name: &str,
    source: &mut R,
    stamp: &str,
) -> Result<SavedDrop, DropSaveError> {
    let name_max = dir.name_max();
    validate_drop_name(requested_name, name_max).map_err(DropSaveError::Name)?;

    let mut counter = 0u32;
    let mut retries = 0u32;
    loop {
        let (candidate, renamed) = if counter == 0 {
            (requested_name.to_string(), false)
        } else {
            match collision_candidate(requested_name, stamp, counter, name_max) {
                Some(c) => (c, true),
                None => return Err(DropSaveError::NoUsableName),
            }
        };

        match dir.create_exclusive(&candidate) {
            Ok(mut file) => {
                if let Err(e) = std::io::copy(source, &mut file).and_then(|_| file.flush()) {
                    drop(file);
                    dir.remove(&candidate);
                    return Err(DropSaveError::Io(e));
                }
                return Ok(SavedDrop {
                    path: dir.path().join(&candidate),
                    saved_name: candidate,
                    renamed,
                });
            }
            Err(rustix::io::Errno::EXIST) => {
                // A symlink in the way fails the WHOLE request, at any
                // candidate. Renaming around it would quietly write next to
                // something unexpected instead of saying so.
                match collision_step(dir.entry_is_symlink(&candidate)) {
                    CollisionStep::RefuseSymlink => {
                        return Err(DropSaveError::Symlink { name: candidate });
                    }
                    CollisionStep::NextCandidate => {
                        counter += 1;
                        if counter > MAX_COLLISION_ATTEMPTS {
                            return Err(DropSaveError::NoUsableName);
                        }
                    }
                    CollisionStep::RetrySameCandidate => {
                        // The entry went away between the create and the probe,
                        // so this name is free again. Advancing would leave a
                        // gap in the numbering for no reason. Bounded by the
                        // same budget so a name being churned by something else
                        // cannot spin here forever.
                        retries += 1;
                        if retries > MAX_COLLISION_ATTEMPTS {
                            return Err(DropSaveError::NoUsableName);
                        }
                    }
                    CollisionStep::Failed(e) => return Err(DropSaveError::Io(e.into())),
                }
            }
            // O_NOFOLLOW on an existing symlink reports ELOOP on Linux and
            // EMLINK on some BSDs. Either way it is the same refusal.
            Err(rustix::io::Errno::LOOP) | Err(rustix::io::Errno::MLINK) => {
                return Err(DropSaveError::Symlink { name: candidate });
            }
            Err(e) => return Err(DropSaveError::Io(e.into())),
        }
    }
}

/// Where a file dropped onto a pane will land, resolved from the pane's PTY.
///
/// The two cases really are different questions, which is why this is a choice
/// rather than one path: an agent's answer is a fixed directory the engine
/// already knows, while a terminal's has to be asked of a live process every
/// time.
#[derive(Debug, Clone)]
pub enum FileDropDestination {
    /// An AGENT pane: the root of that agent's worktree, so the file is visible
    /// to git and can be committed alongside whatever the agent does with it.
    Worktree(PathBuf),
    /// A TERMINAL pane: wherever that terminal actually is right now.
    Terminal(WorkingDirectory),
}

impl FileDropDestination {
    /// Pin the destination directory. Blocking: this reads `/proc` (Linux) or
    /// runs `lsof` (macOS), so it belongs on a blocking pool.
    pub fn open(&self) -> Result<DropDir, DropDirError> {
        match self {
            Self::Worktree(path) => DropDir::open(path),
            Self::Terminal(plan) => plan.open(),
        }
    }
}

/// The plan for finding the directory a terminal is ACTUALLY in.
///
/// Built by [`crate::pty::PtyClient::working_directory`], which owns the policy;
/// this carries it off the engine thread so the probing (a `/proc` read on
/// Linux, an `lsof` process on macOS) happens on a blocking pool like every
/// other filesystem call in the server.
///
/// The order is deliberate: the FOREGROUND process group first, then the shell,
/// then the directory the terminal was opened in. If a file is being handed to
/// whatever is reading the terminal right now, the right directory is that
/// program's, not its suspended parent's.
///
/// This is the best available representation and not a perfect answer: a
/// pipeline's leader can exit while its group lives on, and members of one group
/// can technically hold different directories.
#[derive(Debug, Clone)]
pub struct WorkingDirectory {
    /// The foreground process group leader, when one is running that is not the
    /// shell itself.
    pub foreground_pid: Option<u32>,
    /// The shell's own pid.
    pub shell_pid: Option<u32>,
    /// Where the terminal was opened. A shell's directory changes the moment
    /// someone types `cd`, so this is the last fallback and never the answer
    /// while a live process can be asked.
    pub spawn_dir: PathBuf,
}

/// What one process could tell us about its working directory.
///
/// The distinction between the last two is the whole point. Discarding it, and
/// then opening the spawn directory unconditionally, is what turned "dux is not
/// allowed to read that process" into "dux writes somewhere else and the toast
/// confidently names it".
#[derive(Debug)]
enum CwdProbe {
    /// The directory, pinned.
    Pinned(DropDir),
    /// The process is not there any more. The next candidate in the chain is
    /// the honest answer, so this is the ONLY case that falls through.
    Gone,
    /// The process is alive and could not be read. A refusal.
    Failed(DropDirError),
}

/// Ask one live process where it is, and classify the failure.
///
/// A permission failure is taken at its word without a second syscall. Anything
/// else is disambiguated by asking whether the process still exists at all
/// (signal 0, which reports `ESRCH` for a process that is gone and `EPERM` for
/// one that is alive but not ours). That is what tells a vanished process apart
/// from a namespace or I/O problem on both platforms, including macOS, where
/// `lsof` exits non-zero for both reasons and says nothing useful about which.
fn probe_process_cwd(pid: u32) -> CwdProbe {
    match open_process_cwd(pid) {
        Ok(dir) => CwdProbe::Pinned(dir),
        Err(DropDirError::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            CwdProbe::Failed(DropDirError::Io(e))
        }
        Err(e) => {
            if process_can_answer(pid) {
                CwdProbe::Failed(e)
            } else {
                CwdProbe::Gone
            }
        }
    }
}

/// Whether `pid` names a process that can still answer for itself.
///
/// Signal 0 alone is not enough, and reading it as the whole answer was a bug.
/// It reports `ESRCH` for a pid nobody is using and `EPERM` for one that is
/// there and belongs to someone else, which is still THERE. But a process that
/// has EXITED and has not yet been reaped, a ZOMBIE, answers signal 0 too: its
/// pid stays allocated while it holds an exit status for its parent, and a
/// shell does not reap a finished job the instant it ends.
///
/// A zombie has nothing left to ask. Verified against the kernel rather than
/// reasoned about: for a real unreaped child, `/proc/<pid>/cwd` and
/// `/proc/<pid>/ns/mnt` both answer `ENOENT` while `kill(pid, 0)` succeeds. So
/// treating it as alive turned "this job finished" into a REFUSAL, exactly
/// where a surviving group member or the shell should have been asked instead.
fn process_can_answer(pid: u32) -> bool {
    signal_zero_finds(pid) && !process_is_zombie(pid)
}

/// Whether signal 0 finds anything at `pid`. `EPERM` counts as found.
fn signal_zero_finds(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

/// Whether `pid` has exited without being reaped.
///
/// Linux keeps `/proc/<pid>/stat` readable for a zombie (it is one of the few
/// things that survives), and its state field is `Z`. An unreadable stat is NOT
/// read as a zombie: that is a different failure, and the signal-0 answer
/// stands.
///
/// Read as BYTES, because a process name is bytes. See [`stat_process_state`].
#[cfg(target_os = "linux")]
fn process_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat_process_state(&stat) == Some(b'Z')
}

/// The same question where there is no `/proc`.
///
/// `state` is a POSIX `ps -o` keyword and macOS prints `Z` for a zombie. This
/// adds no new kind of dependency: the non-Linux directory probe itself already
/// runs `lsof`. Anything other than a leading `Z`, including `ps` failing to run
/// at all, leaves the signal-0 answer standing.
///
/// Not exercised by dux's test suite, which runs on Linux.
#[cfg(not(target_os = "linux"))]
fn process_is_zombie(pid: u32) -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .trim_start()
        .starts_with('Z')
}

/// The members of a process group, and whether every process was inspected.
#[derive(Debug)]
enum ProcessGroup {
    /// The members, from a scan that saw everything there was to see. An empty
    /// one is a FACT: the group has gone.
    Members(Vec<u32>),
    /// The members that could be identified, from a scan that could not inspect
    /// at least one process. Deliberately not the same case as `Members`: an
    /// empty one says only that dux does not know, and reading that as "the
    /// group has gone" is what sent a file to the shell's folder instead.
    Incomplete(Vec<u32>),
    /// This platform cannot answer at all. Not the same as "no members".
    Unknown,
}

/// What one process contributed to a group scan.
#[derive(Debug, PartialEq, Eq)]
enum ScanEntry {
    Member,
    NotAMember,
    /// It could not be inspected. An inability to LOOK, never a fact about the
    /// world, so it makes the whole scan incomplete.
    Unreadable,
}

/// Classify one process from its `/proc/<pid>/stat`, read as BYTES.
///
/// Pure, because a genuinely unreadable stat cannot be built on this machine:
/// the file is world-readable, and the kernel's own way of hiding a process
/// (the `hidepid` mount option) removes the whole directory from the listing
/// instead, which is a different thing entirely.
#[cfg(target_os = "linux")]
fn group_scan_step(stat: std::io::Result<Vec<u8>>, pgid: u32) -> ScanEntry {
    match stat {
        Ok(stat) => match stat_process_group(&stat) {
            Some(found) if found == pgid => ScanEntry::Member,
            Some(_) => ScanEntry::NotAMember,
            // Present, readable, and not something this code can make sense of.
            None => ScanEntry::Unreadable,
        },
        // It exited between the listing and the read. Ordinary churn, and a
        // fact: it is not in the group any more.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ScanEntry::NotAMember,
        Err(_) => ScanEntry::Unreadable,
    }
}

/// Every live process in group `pgid`, leader excluded.
///
/// Linux answers from `/proc`, which is a directory read and no subprocess. On
/// every other platform this is [`ProcessGroup::Unknown`], and the caller
/// refuses rather than guessing; see [`WorkingDirectory::open`].
///
/// A scan that could not inspect some process comes back as
/// [`ProcessGroup::Incomplete`] rather than as whatever it happened to collect.
/// The two are read differently by the caller, because "the group has gone" and
/// "dux could not look properly" are different facts and only the first may
/// fall through to the shell.
fn process_group_members(pgid: u32) -> ProcessGroup {
    // Group 0 is not a group. Every kernel thread reports a process group of 0,
    // so scanning for it would sweep them all in and then refuse the drop the
    // moment one of them could not be read.
    if pgid == 0 {
        return ProcessGroup::Members(Vec::new());
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return ProcessGroup::Unknown;
        };
        let mut members = Vec::new();
        let mut complete = true;
        for entry in entries {
            // The listing itself failed part-way. Nothing was seen there, so
            // nothing may be concluded from what is missing.
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                // Not a pid directory at all (`/proc/self`, `/proc/meminfo`).
                continue;
            };
            if pid == pgid {
                continue;
            }
            match group_scan_step(std::fs::read(format!("/proc/{pid}/stat")), pgid) {
                ScanEntry::Member => members.push(pid),
                ScanEntry::NotAMember => {}
                ScanEntry::Unreadable => complete = false,
            }
        }
        if complete {
            ProcessGroup::Members(members)
        } else {
            ProcessGroup::Incomplete(members)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS answers UNKNOWN, deliberately, and it is a real gap rather than
        // a placeholder waiting to be filled in casually.
        //
        // Linux enumerates a process group by reading `/proc`. macOS has no
        // `/proc`, so the equivalent is `proc_listpids` through libproc or a
        // parse of `ps -eo pid,pgid`, and either one is a guess until it has
        // been RUN on a Mac: what `ps` prints for a zombie, whether a
        // partially-readable listing can be told from a complete one, and what
        // libproc returns for a group whose leader has just gone are all
        // measurable facts and none of them is memory. Writing an unverified
        // enumeration here would not fail loudly; it would report a confident
        // empty group, which falls through to the SHELL's directory, which is
        // exactly the "guessed the wrong destination" defect this whole chain
        // exists to prevent.
        //
        // So the honest answer is Unknown, which the caller turns into a
        // refusal. The window is narrow: it opens only when the foreground
        // leader has already exited while its group still owns the terminal
        // (`open_with` only asks for members in the `Gone` arm), and the user
        // is told to run the drop again. `website/docs/dropping-files.md` says
        // so rather than promising the Linux behaviour everywhere.
        let _ = pgid;
        ProcessGroup::Unknown
    }
}

/// The whitespace-separated fields of a `/proc/<pid>/stat` line that follow the
/// executable name, as BYTES.
///
/// Split at the LAST `)` rather than by splitting on whitespace from the start,
/// because field two is the executable name in parentheses and it can contain
/// both spaces and parentheses. After it come state, ppid and then the process
/// group.
///
/// Bytes and not text, because a process name is a filename and a filename is a
/// byte string: nothing stops it holding bytes that are not valid UTF-8, and
/// that is an ordinary legal name rather than an attack. Decoding the line as
/// text made the whole read fail for such a process, which read as "not a
/// zombie" (so a finished process looked alive and REFUSED the drop) and as
/// "could not inspect" during a group scan (so one unrelated oddly named
/// process anywhere on the machine refused an ordinary drop). Everything this
/// parser actually reads is ASCII; only the name it steps over is not, so the
/// last `)` is exactly the right place to start.
#[cfg(target_os = "linux")]
fn stat_fields_after_comm(stat: &[u8]) -> Option<impl Iterator<Item = &[u8]>> {
    let after_comm = &stat[stat.iter().rposition(|&b| b == b')')? + 1..];
    Some(
        after_comm
            .split(|b| b.is_ascii_whitespace())
            .filter(|field| !field.is_empty()),
    )
}

/// The process group id out of a `/proc/<pid>/stat` line.
#[cfg(target_os = "linux")]
fn stat_process_group(stat: &[u8]) -> Option<u32> {
    let field = stat_fields_after_comm(stat)?.nth(2)?;
    std::str::from_utf8(field).ok()?.parse().ok()
}

/// The state letter out of a `/proc/<pid>/stat` line: the first field after the
/// executable name, so `Z` for an exited-but-unreaped process.
#[cfg(target_os = "linux")]
fn stat_process_state(stat: &[u8]) -> Option<u8> {
    stat_fields_after_comm(stat)?.next()?.first().copied()
}

impl WorkingDirectory {
    /// Walk the chain and pin the first directory that answers.
    ///
    /// Falling through is not free: it happens ONLY when a process is genuinely
    /// gone. Any other failure refuses the drop, because the alternative is
    /// writing into the suspended parent's directory (or the spawn directory)
    /// and telling the user that is where their file is.
    ///
    /// When the foreground group's LEADER has exited, a surviving member of that
    /// group is asked before the shell is: the group still owns the terminal, so
    /// the shell's directory is not the answer, and a pipeline whose first stage
    /// finished is an ordinary thing rather than an error. Only when every
    /// member is gone too does the shell get its turn. A platform that cannot
    /// enumerate the group refuses instead of guessing.
    pub fn open(&self) -> Result<DropDir, DropDirError> {
        self.open_with(probe_process_cwd, process_group_members)
    }

    fn open_with<P, G>(&self, probe: P, members: G) -> Result<DropDir, DropDirError>
    where
        P: Fn(u32) -> CwdProbe,
        G: Fn(u32) -> ProcessGroup,
    {
        if let Some(foreground) = self.foreground_pid {
            match probe(foreground) {
                CwdProbe::Pinned(dir) => return Ok(dir),
                CwdProbe::Failed(e) => return Err(e),
                CwdProbe::Gone => {
                    // An INCOMPLETE scan is not an empty one. Whatever it found
                    // is still worth asking (any member of the group is an
                    // acceptable answer), but if none of them answers, the
                    // emptiness proves nothing and the drop is refused rather
                    // than falling through to the shell.
                    let (pids, complete) = match members(foreground) {
                        ProcessGroup::Members(pids) => (pids, true),
                        ProcessGroup::Incomplete(pids) => (pids, false),
                        ProcessGroup::Unknown => {
                            return Err(DropDirError::ForegroundGroupUnreadable);
                        }
                    };
                    for pid in pids {
                        match probe(pid) {
                            CwdProbe::Pinned(dir) => return Ok(dir),
                            CwdProbe::Failed(e) => return Err(e),
                            CwdProbe::Gone => continue,
                        }
                    }
                    if !complete {
                        return Err(DropDirError::ForegroundGroupUnreadable);
                    }
                    // Every member was seen and every member is gone, so the
                    // foreground job really did end and the shell is the honest
                    // next answer.
                }
            }
        }
        if let Some(shell) = self.shell_pid {
            match probe(shell) {
                CwdProbe::Pinned(dir) => return Ok(dir),
                CwdProbe::Failed(e) => return Err(e),
                CwdProbe::Gone => {}
            }
        }
        DropDir::open(&self.spawn_dir)
    }
}

/// Pin the current working directory of a live process.
///
/// On Linux `/proc/<pid>/cwd` is the directory, so opening it with `O_DIRECTORY`
/// yields the handle in one step, with nothing to swap in between.
///
/// On macOS there is no such entry. `lsof` ships with the system and reports it,
/// so it is asked, and the answer is then opened by path. **That reopen-by-path
/// gap cannot be closed by this mechanism**, and is stated rather than glossed:
/// on macOS the directory named by `lsof` could in principle be replaced before
/// the open. Linux has no such gap.
/// Whether `pid` sees the same filesystem tree dux does.
///
/// `/proc/<pid>/ns/mnt` is a magic link whose text (`mnt:[4026531840]`) is the
/// namespace's identity, so comparing the two links compares the namespaces
/// without needing to enter either. A kernel built without namespace support has
/// no such entry at all, and then nobody has a namespace to differ in, so an
/// unreadable link on OUR side reads as "the same".
#[cfg(target_os = "linux")]
fn shares_our_mount_namespace(pid: u32) -> Result<bool, DropDirError> {
    let Ok(ours) = std::fs::read_link("/proc/self/ns/mnt") else {
        return Ok(true);
    };
    // Theirs is NOT forgiven: an error here is the process being gone or dux not
    // being allowed to look, and both must reach the caller's classifier rather
    // than being read as agreement.
    let theirs = std::fs::read_link(format!("/proc/{pid}/ns/mnt")).map_err(DropDirError::Io)?;
    Ok(ours == theirs)
}

pub fn open_process_cwd(pid: u32) -> Result<DropDir, DropDirError> {
    #[cfg(target_os = "linux")]
    {
        // Before anything is saved. The handle would be opened correctly through
        // the process's own view, but the path is read back in DUX's view, so a
        // process in another mount namespace can be given a file that really did
        // land in the right place under a name that names something else there.
        // There is no way to render a correct name for it from here, so refuse.
        if !shares_our_mount_namespace(pid)? {
            return Err(DropDirError::ForeignMountNamespace);
        }
        DropDir::open(Path::new(&format!("/proc/{pid}/cwd")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("lsof")
            .args(["-a", "-d", "cwd", "-Fn", "-p", &pid.to_string()])
            .output()
            .map_err(DropDirError::Io)?;
        if !output.status.success() {
            return Err(DropDirError::Io(std::io::Error::other(format!(
                "lsof could not report the working directory of process {pid}"
            ))));
        }
        let cwd = lsof_cwd_path(&output.stdout).ok_or_else(|| {
            DropDirError::Io(std::io::Error::other(format!(
                "lsof reported no working directory for process {pid}"
            )))
        })?;
        DropDir::open(&cwd)
    }
}

/// The working directory out of `lsof -Fn` output, as BYTES.
///
/// A Unix path is a byte string and is not required to be valid UTF-8, so this
/// never decodes. Decoding lsof's answer lossily, which is what this used to do,
/// substitutes a replacement character for every byte it cannot read, and the
/// result is a DIFFERENT path: usually one that names nothing, occasionally one
/// that names something else. Either way dux would then open it, save into it,
/// and report it as the folder the file landed in. The rule everywhere else in
/// this module is that a path dux cannot name exactly is REFUSED and never
/// substituted, and a lossy decode is a substitution wearing a success. So the
/// bytes are carried through unchanged, the directory that is opened is the real
/// one, and a path that genuinely cannot be reported as text is refused where
/// every other unreportable path is, as [`UnreportablePath::NotUtf8`], at the
/// moment it would be sent to a terminal.
///
/// `-Fn` prints one field per line, each prefixed by its field letter; `n` is
/// the name field, which for the `cwd` descriptor is the path. One limit of that
/// format is left open and is stated rather than glossed: a field is terminated
/// by a newline, so a directory whose own name contains a newline is
/// indistinguishable from two fields and comes back truncated. lsof offers a
/// NUL-terminated mode for exactly this, and it is deliberately not used here,
/// because this branch only ever runs on macOS and nobody has been able to
/// measure that flag against the lsof macOS ships. Guessing at an unverified
/// flag on a path that cannot be exercised is how the rest of this file went
/// wrong; a documented limit is the honest alternative.
#[cfg(any(not(target_os = "linux"), test))]
fn lsof_cwd_path(stdout: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    stdout
        .split(|&b| b == b'\n')
        .find_map(|line| line.strip_prefix(b"n"))
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    // ── Name validation: kept as given, or refused with a reason ─────────────

    #[test]
    fn keeps_ordinary_awkward_names_exactly_as_dropped() {
        // Spaces, parentheses and quotes are all legal in a filename and are
        // what a screenshot is actually called. They are a QUOTING problem at
        // paste time, not a naming problem here.
        for name in [
            "Screen Shot 2026-08-01 at 12.00.00.png",
            "report (final).pdf",
            "it's a \"quote\".txt",
            "café ☕.png",
            "スクリーンショット.png",
            "Снимок экрана.png",
            ".env",
        ] {
            assert_eq!(
                validate_drop_name(name, 255),
                Ok(()),
                "{name} should be accepted unchanged"
            );
        }
    }

    #[test]
    fn refuses_names_that_are_not_names() {
        assert_eq!(validate_drop_name("", 255), Err(DropNameError::Empty));
        assert_eq!(validate_drop_name(".", 255), Err(DropNameError::DotSegment));
        assert_eq!(
            validate_drop_name("..", 255),
            Err(DropNameError::DotSegment)
        );
        assert_eq!(
            validate_drop_name("a\0b.png", 255),
            Err(DropNameError::NullByte)
        );
        assert_eq!(
            validate_drop_name("a\nb.png", 255),
            Err(DropNameError::Control)
        );
        assert_eq!(
            validate_drop_name("a\x7fb.png", 255),
            Err(DropNameError::Control)
        );
    }

    #[test]
    fn refuses_a_traversing_name() {
        // The one that matters: a name is a NAME, so anything carrying a
        // separator is refused outright rather than being resolved.
        assert_eq!(
            validate_drop_name("../../etc/passwd", 255),
            Err(DropNameError::Separator)
        );
        assert_eq!(
            validate_drop_name("sub/shot.png", 255),
            Err(DropNameError::Separator)
        );
        assert_eq!(
            validate_drop_name("/etc/passwd", 255),
            Err(DropNameError::Separator)
        );
    }

    #[test]
    fn refuses_a_name_over_the_byte_limit() {
        // The limit is BYTES, not characters: 100 three-byte glyphs are 300
        // bytes and do not fit in 255.
        let wide = "日".repeat(100);
        assert_eq!(wide.chars().count(), 100);
        assert_eq!(
            validate_drop_name(&wide, 255),
            Err(DropNameError::TooLong { limit: 255 })
        );
        assert_eq!(validate_drop_name(&"a".repeat(255), 255), Ok(()));
        assert_eq!(
            validate_drop_name(&"a".repeat(256), 255),
            Err(DropNameError::TooLong { limit: 255 })
        );
    }

    // ── Collision candidates ────────────────────────────────────────────────

    #[test]
    fn a_candidate_keeps_the_extension_and_carries_a_counter() {
        assert_eq!(
            collision_candidate("shot.png", "20260801-120000", 1, 255).as_deref(),
            Some("shot-20260801-120000-1.png")
        );
        assert_eq!(
            collision_candidate("shot.png", "20260801-120000", 2, 255).as_deref(),
            Some("shot-20260801-120000-2.png")
        );
    }

    #[test]
    fn a_hidden_file_has_no_extension_to_keep() {
        // A LEADING dot is not an extension separator; `.env` is a whole name.
        assert_eq!(
            collision_candidate(".env", "S", 1, 255).as_deref(),
            Some(".env-S-1")
        );
    }

    #[test]
    fn a_multi_dot_name_keeps_only_its_last_extension() {
        assert_eq!(
            collision_candidate("archive.tar.gz", "S", 1, 255).as_deref(),
            Some("archive.tar-S-1.gz")
        );
    }

    #[test]
    fn a_name_at_the_limit_still_gets_a_usable_collision_name() {
        // The case the design calls out by name: a name that PASSES validation
        // and would exceed the limit once a suffix is appended. Room is
        // reserved up front, the stem is truncated, the extension survives.
        let stem = "a".repeat(251);
        let name = format!("{stem}.png"); // exactly 255 bytes
        assert_eq!(name.len(), 255);
        let candidate = collision_candidate(&name, "20260801-120000", 1, 255)
            .expect("a name at the limit must still get a candidate");
        assert!(
            candidate.len() <= 255,
            "candidate is {} bytes, over the limit: {candidate}",
            candidate.len()
        );
        assert!(
            candidate.ends_with(".png"),
            "the extension must survive so the file is still recognized: {candidate}"
        );
        assert!(candidate.contains("-20260801-120000-1"));
    }

    #[test]
    fn truncation_never_splits_a_character() {
        // A name of wide glyphs truncated to fit must stay valid UTF-8 and must
        // not end mid-character. Byte slicing here is the bug this pins.
        let name = format!("{}.png", "日".repeat(84)); // 252 + 4 = 256 bytes
        let candidate =
            collision_candidate(&name, "S", 1, 255).expect("wide name must get a candidate");
        assert!(candidate.len() <= 255);
        assert!(candidate.ends_with("-S-1.png"));
        let stem = candidate.strip_suffix("-S-1.png").expect("stem");
        assert!(
            stem.chars().all(|c| c == '日'),
            "truncation split a character: {stem:?}"
        );
    }

    #[test]
    fn an_impossible_limit_fails_deterministically() {
        // The suffix alone leaves no room for a single byte of stem. It must
        // return None rather than loop or produce something unusable.
        assert_eq!(
            collision_candidate("shot.png", "20260801-120000", 1, 8),
            None
        );
    }

    // ── Saving ──────────────────────────────────────────────────────────────

    #[test]
    fn dropping_the_same_name_twice_produces_two_files() {
        // The journey: a user drops shot.png, then drops shot.png again in the
        // same second. Nothing is overwritten and the second is renamed, so the
        // browser can tell them what it is now called.
        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");

        let first = save_drop(&pinned, "shot.png", b"one", "20260801-120000").expect("first save");
        assert_eq!(first.saved_name, "shot.png");
        assert!(!first.renamed);

        let second =
            save_drop(&pinned, "shot.png", b"two", "20260801-120000").expect("second save");
        assert!(
            second.renamed,
            "the second drop must be reported as renamed"
        );
        assert_ne!(second.saved_name, "shot.png");
        assert_eq!(second.saved_name, "shot-20260801-120000-1.png");

        assert_eq!(std::fs::read(&first.path).unwrap(), b"one");
        assert_eq!(std::fs::read(&second.path).unwrap(), b"two");
    }

    #[test]
    fn a_non_latin_name_is_saved_exactly_as_dropped() {
        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");
        let saved = save_drop(&pinned, "スクリーンショット.png", b"x", "S").expect("save");
        assert_eq!(saved.saved_name, "スクリーンショット.png");
        assert!(saved.path.ends_with("スクリーンショット.png"));
        assert!(saved.path.exists());
    }

    #[test]
    fn a_symlink_at_the_destination_fails_the_request() {
        // Refusing to follow it is the safety property. Writing next to it
        // would hide the fact that something unexpected is sitting there.
        let dir = tmp();
        let outside = tmp();
        let victim = outside.path().join("victim.txt");
        std::fs::write(&victim, b"original").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("shot.png")).unwrap();

        let pinned = DropDir::open(dir.path()).expect("pin");
        let err = save_drop(&pinned, "shot.png", b"attacker", "S").expect_err("must refuse");
        assert!(
            matches!(&err, DropSaveError::Symlink { name } if name == "shot.png"),
            "expected a symlink refusal, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"original",
            "the symlink target must be untouched"
        );
    }

    #[test]
    fn a_symlink_at_a_later_candidate_also_fails_the_request() {
        // "Any candidate name tried, not only the first" is the part that is
        // easy to get wrong: the loop must not skip past a symlink.
        let dir = tmp();
        let outside = tmp();
        let victim = outside.path().join("victim.txt");
        std::fs::write(&victim, b"original").unwrap();
        std::fs::write(dir.path().join("shot.png"), b"real file").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("shot-S-1.png")).unwrap();

        let pinned = DropDir::open(dir.path()).expect("pin");
        let err = save_drop(&pinned, "shot.png", b"attacker", "S").expect_err("must refuse");
        assert!(
            matches!(&err, DropSaveError::Symlink { name } if name == "shot-S-1.png"),
            "expected a symlink refusal at the second candidate, got {err:?}"
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"original");
    }

    #[test]
    fn a_failed_symlink_probe_fails_the_request_instead_of_renaming_around_it() {
        // The probe used to swallow every error into `false`, so an I/O or
        // permission failure became an ordinary collision and the loop advanced
        // to the next name. That contradicts the promised rule outright: the one
        // look that could have seen a symlink is exactly the one that failed.
        //
        // These branches cannot be built on a real filesystem unprivileged,
        // because the same directory permission that makes `statat` fail also
        // makes the `openat` that reports EEXIST fail, so there is no EEXIST to
        // probe after. The decision is therefore a pure function and is pinned
        // here; the two reachable outcomes keep their end-to-end tests above.
        assert_eq!(collision_step(Ok(true)), CollisionStep::RefuseSymlink);
        assert_eq!(collision_step(Ok(false)), CollisionStep::NextCandidate);
        assert_eq!(
            collision_step(Err(rustix::io::Errno::ACCESS)),
            CollisionStep::Failed(rustix::io::Errno::ACCESS),
            "a probe dux was not allowed to make must not read as \"not a symlink\""
        );
        assert_eq!(
            collision_step(Err(rustix::io::Errno::IO)),
            CollisionStep::Failed(rustix::io::Errno::IO)
        );
        // "No longer there" is the one error that is not a failure: the name is
        // free again, so the SAME candidate is retried rather than skipped,
        // which would leave a gap in the numbering for nothing.
        assert_eq!(
            collision_step(Err(rustix::io::Errno::NOENT)),
            CollisionStep::RetrySameCandidate
        );
    }

    #[test]
    fn an_abandoned_upload_leaves_nothing_behind() {
        struct FailsHalfway {
            sent: bool,
        }
        impl Read for FailsHalfway {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.sent {
                    return Err(std::io::Error::other("connection dropped"));
                }
                self.sent = true;
                buf[..4].copy_from_slice(b"part");
                Ok(4)
            }
        }

        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");
        let mut source = FailsHalfway { sent: false };
        let err = save_drop_from(&pinned, "shot.png", &mut source, "S").expect_err("must fail");
        assert!(matches!(err, DropSaveError::Io(_)), "got {err:?}");

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "a failed upload must remove its partial file, found {leftovers:?}"
        );
    }

    #[test]
    fn a_traversing_name_never_reaches_the_filesystem() {
        let parent = tmp();
        let dir = parent.path().join("inner");
        std::fs::create_dir(&dir).unwrap();
        let pinned = DropDir::open(&dir).expect("pin");
        let err = save_drop(&pinned, "../escaped.png", b"x", "S").expect_err("must refuse");
        assert!(
            matches!(err, DropSaveError::Name(DropNameError::Separator)),
            "got {err:?}"
        );
        assert!(!parent.path().join("escaped.png").exists());
    }

    #[test]
    fn a_pinned_directory_reports_its_own_absolute_path() {
        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");
        assert!(pinned.path().is_absolute());
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn name_max_is_the_filesystem_s_real_limit_and_not_an_assumption() {
        // Asserting a plausible RANGE proves nothing: a hard-coded 255 sits
        // inside it, and so does the fallback. The only honest proof is to use
        // the number the filesystem is claimed to have given, so the limit is
        // EXERCISED: a name of exactly that many bytes must be creatable, and one
        // byte more must be refused by the kernel for being too long.
        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");
        let limit = pinned.name_max();

        let at_limit = "a".repeat(limit);
        pinned
            .create_exclusive(&at_limit)
            .unwrap_or_else(|e| panic!("a {limit}-byte name must fit, but the kernel said {e:?}"));

        let over_limit = "a".repeat(limit + 1);
        let err = pinned
            .create_exclusive(&over_limit)
            .expect_err("a name one byte over the reported limit must not fit");
        assert_eq!(
            err,
            rustix::io::Errno::NAMETOOLONG,
            "the reported limit is not the filesystem's: {} bytes was refused with {err:?} \
             rather than ENAMETOOLONG",
            limit + 1
        );
    }

    // ── The destination folder's own path ───────────────────────────────────

    /// A directory whose NAME is exactly `raw`, which is how a folder holding a
    /// line feed, an escape byte or invalid UTF-8 gets built.
    fn dir_named(raw: &[u8]) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::ffi::OsStrExt;
        let parent = tmp();
        let child = parent.path().join(std::ffi::OsStr::from_bytes(raw));
        std::fs::create_dir(&child).expect("create the awkwardly named directory");
        (parent, child)
    }

    #[test]
    fn a_folder_whose_path_cannot_be_sent_refuses_the_drop_before_writing() {
        // BEFORE this check every one of these was ACCEPTED and the file was
        // saved, with a path that does not name it once it reaches a terminal:
        // the line feed is rewritten to a carriage return on the way through, so
        // the token submits instead of naming a file; the escape byte is obeyed
        // by the line editor or by whatever full-screen program is reading; and
        // the invalid UTF-8 came back as "dir\u{fffd}\u{fffd}name", which names
        // nothing at all. Quoting protects none of that, because none of it is a
        // shell problem.
        for (label, raw, want) in [
            (
                "a line feed",
                b"dir\nname".as_slice(),
                UnreportablePath::Control,
            ),
            (
                "an escape byte",
                b"dir\x1bname".as_slice(),
                UnreportablePath::Control,
            ),
            (
                "invalid UTF-8",
                b"dir\xff\xfename".as_slice(),
                UnreportablePath::NotUtf8,
            ),
        ] {
            let (_parent, child) = dir_named(raw);
            let err = DropDir::open(&child)
                .err()
                .unwrap_or_else(|| panic!("a folder with {label} in its path must be refused"));
            assert!(
                matches!(&err, DropDirError::Unreportable(w) if *w == want),
                "{label}: expected {want:?}, got {err:?}"
            );
            assert!(
                std::fs::read_dir(&child).unwrap().next().is_none(),
                "{label}: the refusal must happen before anything is written"
            );
        }
    }

    #[test]
    fn the_folders_that_are_only_a_quoting_problem_still_work() {
        // The other half of the rule, and the reason the check is about control
        // characters and encoding rather than "unusual characters". A dollar, a
        // backtick, a space and a single quote all survive as ONE shell token
        // once the path is quoted, so refusing them would break ordinary folders
        // for no safety gain: a worktree path is built from the project's name
        // and routinely contains spaces.
        for name in [
            "with $dollar",
            "with `backtick`",
            "with space",
            "with'quote",
            "with\"double\"",
            "with;semicolon",
        ] {
            let parent = tmp();
            let child = parent.path().join(name);
            std::fs::create_dir(&child).unwrap();
            let pinned = DropDir::open(&child)
                .unwrap_or_else(|e| panic!("{name} must still be usable, got {e}"));
            let saved = save_drop(&pinned, "shot.png", b"x", "S")
                .unwrap_or_else(|e| panic!("{name} must still save, got {e}"));
            assert!(saved.path.exists(), "{name}: the file must be there");
            assert!(saved.path.to_str().unwrap().contains(name));
        }
    }

    #[test]
    fn a_relative_answer_is_refused_rather_than_read_as_an_option() {
        // Absoluteness was ASSUMED rather than checked. It cannot be constructed
        // through `DropDir::open` on Linux (the path is read back from the
        // handle and is therefore always absolute), so the rule is pinned where
        // it lives instead of being left unstated.
        assert_eq!(
            check_reportable_dir_path(Path::new("relative/dir")),
            Err(UnreportablePath::NotAbsolute)
        );
        assert_eq!(
            check_reportable_dir_path(Path::new("-rf")),
            Err(UnreportablePath::NotAbsolute)
        );
        assert_eq!(
            check_reportable_dir_path(Path::new("/tmp/fine")),
            Ok("/tmp/fine")
        );
    }

    #[test]
    fn a_folder_removed_while_pinned_is_refused_rather_than_reported() {
        // The handle still writes, and Linux still answers a path for it (with a
        // "(deleted)" marker), but that path names nothing. Verifying the path
        // against the pinned directory is what turns the reported path from a
        // hope into a fact.
        let parent = tmp();
        let child = parent.path().join("gone");
        std::fs::create_dir(&child).unwrap();
        let fd = rustix::fs::open(
            &child,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("pin the directory");
        std::fs::remove_dir(&child).unwrap();

        let err = reportable_path_of(&fd, &child)
            .expect_err("a removed folder has no path that names it");
        assert!(
            matches!(
                err,
                DropDirError::Unreportable(UnreportablePath::Unverifiable)
            ),
            "got {err:?}"
        );
    }

    // ── Live working directory ──────────────────────────────────────────────

    #[test]
    fn a_live_process_reports_the_directory_it_is_actually_in() {
        // Proves the discovery is LIVE rather than the spawn directory: the
        // child is started somewhere and then changes directory itself.
        let start = tmp();
        let elsewhere = tmp();
        let target = std::fs::canonicalize(elsewhere.path()).unwrap();

        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "cd {} && echo ready && read _line",
                shell_single_quote(&target.to_string_lossy())
            ))
            .current_dir(start.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");

        // Wait for the `cd` to have happened, using the child's own output
        // rather than a sleep, so the state is constructed and not raced for.
        let mut stdout = child.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        stdout.read_exact(&mut buf).expect("read ready marker");
        assert_eq!(&buf, b"ready\n");

        let pinned = open_process_cwd(child.id()).expect("read the child's cwd");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            target,
            "discovery returned the spawn directory instead of where the process actually is"
        );

        drop(stdout);
        drop(child.stdin.take());
        let _ = child.wait();
    }

    #[test]
    fn the_chain_falls_back_to_the_spawn_directory_when_nothing_answers() {
        let dir = tmp();
        let plan = WorkingDirectory {
            // Both pids are impossible, so neither can answer.
            foreground_pid: Some(0),
            shell_pid: Some(0),
            spawn_dir: dir.path().to_path_buf(),
        };
        let pinned = plan.open().expect("fall back to the spawn dir");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn the_chain_prefers_the_foreground_process_over_the_shell() {
        // A file handed to a terminal belongs to whatever is reading it right
        // now, not to its suspended parent. Two live processes in two different
        // directories make the preference observable.
        let foreground_dir = tmp();
        let shell_dir = tmp();
        let spawn_dir = tmp();

        let mut foreground = park_in(foreground_dir.path());
        let mut shell = park_in(shell_dir.path());

        let plan = WorkingDirectory {
            foreground_pid: Some(foreground.id()),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn_dir.path().to_path_buf(),
        };
        let pinned = plan.open().expect("open");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(foreground_dir.path()).unwrap(),
            "the shell's directory won over the foreground process's"
        );

        let _ = foreground.kill();
        let _ = foreground.wait();
        let _ = shell.kill();
        let _ = shell.wait();
    }

    #[test]
    fn a_process_dux_cannot_read_refuses_the_drop_instead_of_writing_elsewhere() {
        // The bug, exactly as it behaved: every lookup error was discarded and
        // the spawn directory was then opened unconditionally, so a terminal
        // whose shell dux is not allowed to read wrote into the spawn directory
        // and the toast confidently named it. Recorded before the fix: the chain
        // "landed on" the spawn dir with no complaint.
        //
        // The seam is the probe, so the two failures are told apart by their
        // MEANING rather than by finding a real process of another user.
        let spawn = tmp();
        let plan = WorkingDirectory {
            foreground_pid: Some(4242),
            shell_pid: Some(4243),
            spawn_dir: spawn.path().to_path_buf(),
        };

        let err = plan
            .open_with(
                |_| {
                    CwdProbe::Failed(DropDirError::Io(std::io::Error::from(
                        std::io::ErrorKind::PermissionDenied,
                    )))
                },
                |_| ProcessGroup::Members(Vec::new()),
            )
            .expect_err("a process dux is not allowed to read must refuse the drop");
        assert!(
            matches!(&err, DropDirError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
            "got {err:?}"
        );
        assert!(
            std::fs::read_dir(spawn.path()).unwrap().next().is_none(),
            "nothing may be written to the spawn directory on a refusal"
        );

        // And the ONE case that legitimately falls through: the process is
        // genuinely gone, so the next candidate really is the honest answer.
        let pinned = plan
            .open_with(|_| CwdProbe::Gone, |_| ProcessGroup::Members(Vec::new()))
            .expect("a process that is gone falls through to the spawn directory");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(spawn.path()).unwrap()
        );
    }

    #[test]
    fn permission_denied_and_not_found_are_told_apart_on_a_real_process() {
        // The seam above is only honest if the REAL probe classifies the same
        // way, so both halves are exercised against real pids. Pid 1 is running
        // and owned by root on any machine dux runs on, so a non-root test
        // process is refused when it looks; a pid that does not exist is Gone.
        //
        // Running as root would make the first half meaningless rather than
        // failing, so it is stated instead of silently passing.
        if rustix::process::geteuid().is_root() {
            eprintln!("SKIPPED: running as root, so /proc/1 is readable and proves nothing");
        } else {
            let denied = open_process_cwd(1).expect_err("root's pid 1 must not be readable");
            assert!(
                matches!(&denied, DropDirError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
                "expected a permission failure for pid 1, got {denied:?}"
            );
            assert!(
                matches!(probe_process_cwd(1), CwdProbe::Failed(_)),
                "a live process dux cannot read must classify as a refusal, not as gone"
            );
        }

        // A pid the kernel will never hand out: nothing to read, nothing alive.
        assert!(!process_can_answer(0));
        assert!(
            matches!(probe_process_cwd(0), CwdProbe::Gone),
            "a process that does not exist must classify as gone so the chain continues"
        );
    }

    #[test]
    fn a_surviving_group_member_answers_when_the_foreground_leader_has_exited() {
        // A pipeline whose first stage finished is ordinary: the group still owns
        // the terminal, so the shell's directory is NOT where the user is.
        // Recorded before the fix: the chain landed on the shell's directory and
        // said nothing.
        //
        // Constructed rather than raced for. The leader exits immediately; a
        // grandchild in the SAME process group stays alive in another directory,
        // and its own "ready" line is the marker that it got there.
        use std::os::unix::process::CommandExt;

        let member_dir = tmp();
        let shell_dir = tmp();
        let spawn_dir = tmp();
        let member_target = std::fs::canonicalize(member_dir.path()).unwrap();

        let mut leader = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "(cd {} && echo ready && exec sleep 300) & exit 0",
                shell_single_quote(&member_target.to_string_lossy())
            ))
            .stdout(std::process::Stdio::piped())
            // A new process group whose id is this child's pid, so killing the
            // leader leaves a group with a dead leader and a live member.
            .process_group(0)
            .spawn()
            .expect("spawn the group leader");
        let pgid = leader.id();
        let mut stdout = leader.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        stdout.read_exact(&mut buf).expect("read ready marker");
        assert_eq!(&buf, b"ready\n");
        leader.wait().expect("the leader must exit");

        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(pgid),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn_dir.path().to_path_buf(),
        };

        let pinned = plan.open().expect("a surviving member must answer");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            member_target,
            "the dead leader's group still owns the terminal, so a surviving \
             member's directory is the answer, not the shell's"
        );

        if let Ok(pgid) = i32::try_from(pgid)
            && let Some(pgid) = rustix::process::Pid::from_raw(pgid)
        {
            let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
        }
        let _ = shell.kill();
        let _ = shell.wait();
    }

    #[test]
    fn an_exited_but_unreaped_process_is_gone_rather_than_alive() {
        // A zombie: the process has exited and nobody has reaped it, so the pid
        // is still allocated (it is holding an exit status for its parent) but
        // there is nothing behind it. Verified against this kernel before the
        // fix: /proc/<pid>/cwd and /proc/<pid>/ns/mnt both answer ENOENT while
        // kill(pid, 0) SUCCEEDS.
        //
        // So asking signal 0 to tell "gone" from "alive but not ours" gets the
        // wrong answer here, and the wrong answer was CwdProbe::Failed, which
        // refuses the whole drop. Recorded before the fix: `process_exists`
        // said true and `probe_process_cwd` classified a finished process as a
        // failure.
        //
        // The state is CONSTRUCTED, not raced for: the child is never waited
        // on, and the test does not continue until /proc says its state is Z.
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn a child that exits at once");
        let pid = child.id();
        wait_until_zombie(pid);

        assert!(
            signal_zero_finds(pid),
            "the premise of the bug: a zombie still answers signal 0"
        );
        assert!(
            process_is_zombie(pid),
            "the state was built wrong: this pid is not a zombie"
        );
        assert!(
            !process_can_answer(pid),
            "an exited-but-unreaped process cannot answer for itself and must \
             read as gone, or the chain refuses instead of continuing"
        );
        assert!(
            matches!(probe_process_cwd(pid), CwdProbe::Gone),
            "a zombie must classify as gone so a surviving group member or the \
             shell gets its turn"
        );

        child.wait().expect("reap the child");
    }

    #[test]
    fn a_surviving_member_answers_for_a_leader_that_was_never_reaped() {
        // The end-to-end shape of the same bug, and the reason the existing
        // surviving-group test does not catch it: that one calls `wait` on the
        // leader, which is exactly what a terminal's shell does NOT do
        // instantly. Recorded before the fix: the drop was REFUSED with an I/O
        // error for the leader instead of landing in the live member's folder.
        use std::os::unix::process::CommandExt;

        let member_dir = tmp();
        let shell_dir = tmp();
        let spawn_dir = tmp();
        let member_target = std::fs::canonicalize(member_dir.path()).unwrap();

        let mut leader = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "(cd {} && echo ready && exec sleep 300) & exit 0",
                shell_single_quote(&member_target.to_string_lossy())
            ))
            .stdout(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("spawn the group leader");
        let pgid = leader.id();
        let mut stdout = leader.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        stdout.read_exact(&mut buf).expect("read ready marker");
        assert_eq!(&buf, b"ready\n");
        // Deliberately NOT reaped, so the leader sits there as a zombie.
        wait_until_zombie(pgid);

        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(pgid),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn_dir.path().to_path_buf(),
        };

        let pinned = plan
            .open()
            .expect("an unreaped leader must not refuse the drop");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            member_target,
            "the leader is finished, so the live member of its group is the answer"
        );

        if let Ok(raw) = i32::try_from(pgid)
            && let Some(raw) = rustix::process::Pid::from_raw(raw)
        {
            let _ = rustix::process::kill_process_group(raw, rustix::process::Signal::KILL);
        }
        let _ = leader.wait();
        let _ = shell.kill();
        let _ = shell.wait();
    }

    /// Block until `pid` is an exited-but-unreaped process, reading the kernel's
    /// own answer rather than sleeping for a guessed interval.
    fn wait_until_zombie(pid: u32) {
        for _ in 0..2000 {
            if process_is_zombie(pid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("process {pid} never became a zombie");
    }

    #[test]
    fn a_group_scan_that_could_not_read_a_process_is_not_an_empty_group() {
        // The scan used to discard every per-entry failure and return whatever
        // it managed to collect, so a group it could not fully inspect came back
        // as an EMPTY group, which the chain reads as "the whole job ended" and
        // falls through to the shell. Recorded before the fix: the drop landed
        // in the shell's directory and the toast named it.
        //
        // The classification is a pure function because a real unreadable
        // /proc/<pid>/stat cannot be built here: the file is world-readable, and
        // the kernel's own way of hiding a process (hidepid) removes the whole
        // directory from the listing instead, which is a different thing.
        assert_eq!(
            group_scan_step(Ok(b"1 (sh) S 0 4242 0".to_vec()), 4242),
            ScanEntry::Member
        );
        assert_eq!(
            group_scan_step(Ok(b"1 (sh) S 0 7 0".to_vec()), 4242),
            ScanEntry::NotAMember
        );
        // A process name is a filename, and a filename is BYTES. An ordinary,
        // legal name that is not valid UTF-8 must be stepped over like any
        // other, not read as "could not inspect this process": doing that
        // marked the whole scan incomplete and refused an ordinary drop.
        assert_eq!(
            group_scan_step(Ok(b"1 (sh\xffell) S 0 4242 0".to_vec()), 4242),
            ScanEntry::Member
        );
        assert_eq!(
            group_scan_step(Ok(b"1 (sh\xffell) S 0 7 0".to_vec()), 4242),
            ScanEntry::NotAMember
        );
        // And a name holding the delimiters themselves still parses, which is
        // why the LAST `)` is the split point.
        assert_eq!(
            group_scan_step(Ok(b"1 (od(d )na\xffme) S 0 4242 0".to_vec()), 4242),
            ScanEntry::Member
        );
        // Gone between listing /proc and reading its stat: ordinary churn, and a
        // FACT about the world (it is not in the group any more), so it must not
        // spoil the scan.
        assert_eq!(
            group_scan_step(
                Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                4242
            ),
            ScanEntry::NotAMember
        );
        // Anything else is an inability to LOOK, never a fact.
        assert_eq!(
            group_scan_step(
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                4242
            ),
            ScanEntry::Unreadable
        );
        // Present, readable, and not something this code can parse. Also an
        // inability to look.
        assert_eq!(
            group_scan_step(Ok(b"nonsense with no paren".to_vec()), 4242),
            ScanEntry::Unreadable
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stat_line_is_parsed_out_of_its_bytes() {
        // The state letter and the group id are both ASCII; only the name
        // between them is arbitrary bytes. Pinning the parser directly, because
        // this is where reading the file as text lost both answers at once.
        assert_eq!(stat_process_state(b"1 (sh) Z 0 4242 0"), Some(b'Z'));
        assert_eq!(stat_process_state(b"1 (sh\xffell) Z 0 4242 0"), Some(b'Z'));
        assert_eq!(stat_process_state(b"1 (sh\xffell) S 0 4242 0"), Some(b'S'));
        assert_eq!(stat_process_group(b"1 (sh\xffell) Z 0 4242 0"), Some(4242));
        // A name carrying the delimiter, with and without valid text in it.
        assert_eq!(stat_process_state(b"1 (od)d) Z 0 7 0"), Some(b'Z'));
        assert_eq!(stat_process_group(b"1 (od)d\xff) Z 0 7 0"), Some(7));
        // Nothing to parse is None, never a guess.
        assert_eq!(stat_process_state(b"no paren here"), None);
        assert_eq!(stat_process_group(b"no paren here"), None);
        assert_eq!(stat_process_state(b"1 (sh)"), None);
        assert_eq!(stat_process_group(b"1 (sh) Z 0"), None);
    }

    #[test]
    fn an_incomplete_group_scan_refuses_instead_of_falling_through_to_the_shell() {
        // The whole point of carrying the difference: an empty INCOMPLETE scan
        // must refuse exactly as an unreadable directory does, while an empty
        // COMPLETE scan still lets the shell answer.
        let spawn = tmp();
        let shell_dir = tmp();
        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(4242),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn.path().to_path_buf(),
        };
        let probe = |pid: u32| {
            if pid == 4242 {
                CwdProbe::Gone
            } else {
                probe_process_cwd(pid)
            }
        };

        let err = plan
            .open_with(probe, |_| ProcessGroup::Incomplete(Vec::new()))
            .expect_err("a group dux could not fully inspect must refuse");
        assert!(
            matches!(err, DropDirError::ForegroundGroupUnreadable),
            "got {err:?}"
        );
        assert!(
            std::fs::read_dir(spawn.path()).unwrap().next().is_none(),
            "nothing may be written on a refusal"
        );

        // And a member that DID answer still answers: an incomplete scan is only
        // a refusal when nothing in it could tell us where the job is.
        let member_dir = tmp();
        let mut member = park_in(member_dir.path());
        let member_pid = member.id();
        let pinned = plan
            .open_with(probe, move |_| ProcessGroup::Incomplete(vec![member_pid]))
            .expect("a member that answered is an answer");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(member_dir.path()).unwrap()
        );

        let _ = member.kill();
        let _ = member.wait();
        let _ = shell.kill();
        let _ = shell.wait();
    }

    #[test]
    fn a_real_group_scan_finds_a_live_member_and_calls_itself_complete() {
        // The pure classifier above is only honest if the real scan agrees, so
        // the ordinary case is exercised against real processes: a member in a
        // group of its own must be found, and a scan of a machine dux can read
        // must call itself COMPLETE rather than incomplete.
        use std::os::unix::process::CommandExt;

        let dir = tmp();
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("echo ready && read _line")
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("spawn sh in its own group");
        let mut stdout = child.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        stdout.read_exact(&mut buf).expect("read ready marker");

        // The child IS the leader of its own group, and the leader is excluded,
        // so the group reads as complete and empty. A grandchild joins it.
        let mut inner = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("echo ready && read _line")
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .process_group(child.id() as i32)
            .spawn()
            .expect("spawn a second member of the same group");
        let mut inner_out = inner.stdout.take().expect("stdout");
        inner_out.read_exact(&mut buf).expect("read ready marker");

        match process_group_members(child.id()) {
            ProcessGroup::Members(pids) => assert!(
                pids.contains(&inner.id()),
                "the live member {} was not found in {pids:?}",
                inner.id()
            ),
            other => panic!("a readable /proc must produce a complete scan, got {other:?}"),
        }

        let _ = inner.kill();
        let _ = inner.wait();
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_platform_that_cannot_enumerate_the_group_refuses_rather_than_guessing() {
        // "Prefer a surviving member, or refuse." A platform with no way to list
        // the group takes the second branch: falling through to the shell would
        // be the same guess this whole rule exists to stop, and it is the branch
        // every non-Linux build takes.
        let spawn = tmp();
        let shell_dir = tmp();
        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(4242),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn.path().to_path_buf(),
        };
        let err = plan
            .open_with(
                |pid| {
                    if pid == 4242 {
                        CwdProbe::Gone
                    } else {
                        probe_process_cwd(pid)
                    }
                },
                |_| ProcessGroup::Unknown,
            )
            .expect_err("an unreadable foreground group must refuse");
        assert!(
            matches!(err, DropDirError::ForegroundGroupUnreadable),
            "got {err:?}"
        );
        let _ = shell.kill();
        let _ = shell.wait();
    }

    #[test]
    fn a_group_that_is_entirely_gone_lets_the_shell_answer() {
        // The other side of the same rule: when every member really has exited,
        // the foreground job ended and the shell IS the honest next answer. Only
        // "cannot tell" refuses.
        let spawn = tmp();
        let shell_dir = tmp();
        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(4242),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn.path().to_path_buf(),
        };
        let pinned = plan
            .open_with(
                |pid| {
                    if pid == 4242 {
                        CwdProbe::Gone
                    } else {
                        probe_process_cwd(pid)
                    }
                },
                |_| ProcessGroup::Members(Vec::new()),
            )
            .expect("the shell answers once the whole group is gone");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(shell_dir.path()).unwrap()
        );
        let _ = shell.kill();
        let _ = shell.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_in_our_own_mount_namespace_is_not_refused() {
        // The namespace check must not fire on the ordinary case, which is every
        // terminal dux spawns. A child of this process shares our namespace, so
        // it has to be readable; a pid that is gone must report GONE rather than
        // a namespace complaint, or the chain would refuse instead of continuing.
        let dir = tmp();
        let mut child = park_in(dir.path());
        assert!(
            shares_our_mount_namespace(child.id()).expect("read the child's namespace"),
            "a child of this process shares its mount namespace"
        );
        assert!(open_process_cwd(child.id()).is_ok());
        let _ = child.kill();
        let _ = child.wait();

        let err = shares_our_mount_namespace(0).expect_err("pid 0 has no namespace link");
        assert!(
            matches!(&err, DropDirError::Io(e) if e.kind() == std::io::ErrorKind::NotFound),
            "a missing process must surface as NotFound so the chain can continue, got {err:?}"
        );
    }

    /// A path that runs `/bin/sh` under a name whose bytes are NOT valid UTF-8.
    ///
    /// A filename on Linux is a byte string, so this is an ordinary, legal name
    /// rather than a hostile one. The kernel copies the basename of the path
    /// handed to `execve` into the process's `comm`, which is what
    /// `/proc/<pid>/stat` prints between parentheses, so a process started this
    /// way has a stat line that no text decoder can read. Six bytes, so it
    /// survives `comm`'s 15-byte limit with the invalid byte still inside.
    ///
    /// A SYMLINK rather than a copy: `comm` comes from the path as given and not
    /// from what it resolves to, and copying an executable makes the exec race
    /// with any other thread that forked while the copy's write handle was open
    /// (the kernel reports `ETXTBSY`). Nothing here writes to an executable.
    #[cfg(target_os = "linux")]
    fn odd_named_shell(dir: &Path) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        let path = dir.join(std::ffi::OsStr::from_bytes(b"sh\xffell"));
        std::os::unix::fs::symlink("/bin/sh", &path).expect("link /bin/sh to an odd name");
        path
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_finished_process_whose_name_is_not_text_still_reads_as_gone() {
        // The zombie check read /proc/<pid>/stat as TEXT. A process name is
        // bytes, so a perfectly legal executable name can make that read fail
        // outright, and a failed read left the signal-0 answer standing: a
        // process that had FINISHED classified as alive-but-unreadable, which
        // refuses the drop.
        let dir = tmp();
        let program = odd_named_shell(dir.path());
        let mut child = std::process::Command::new(&program)
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn a child whose name is not valid UTF-8");
        let pid = child.id();

        // The premise, asserted rather than assumed: this stat line really is
        // unreadable as text, and really does say Z.
        for _ in 0..2000 {
            let raw = std::fs::read(format!("/proc/{pid}/stat")).expect("read stat as bytes");
            if raw[raw.iter().rposition(|&b| b == b')').expect("comm") + 1..].contains(&b'Z') {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_err(),
            "the premise of this test: the process name is not valid text"
        );

        assert!(
            process_is_zombie(pid),
            "an unreadable-as-text stat line still says Z, and the state must be \
             read out of the bytes"
        );
        assert!(
            !process_can_answer(pid),
            "a finished process cannot answer for itself, whatever it is called"
        );
        assert!(
            matches!(probe_process_cwd(pid), CwdProbe::Gone),
            "it must classify as gone so the chain continues instead of refusing"
        );

        child.wait().expect("reap the child");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unrelated_process_with_an_odd_name_does_not_refuse_an_ordinary_drop() {
        // The group scan reads EVERY process on the machine. Reading each stat
        // as text meant one unrelated process anywhere whose name is not valid
        // UTF-8 made a read fail, which counted as "could not inspect", which
        // marked the whole scan INCOMPLETE, which refuses the drop. Somebody
        // else's oddly named program must not be able to do that.
        let dir = tmp();
        let program = odd_named_shell(dir.path());
        let mut odd = std::process::Command::new(&program)
            .arg("-c")
            .arg("echo ready && read _line")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn a live process whose name is not valid UTF-8");
        let mut odd_out = odd.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        odd_out.read_exact(&mut buf).expect("read ready marker");

        match process_group_members(4242) {
            ProcessGroup::Members(_) => {}
            other => panic!(
                "a process dux CAN read must not spoil the scan, got {other:?} \
                 while pid {} is alive",
                odd.id()
            ),
        }

        // And end to end: the foreground job has genuinely ended, so the shell
        // is the honest answer and the drop must land there.
        let spawn = tmp();
        let shell_dir = tmp();
        let mut shell = park_in(shell_dir.path());
        let plan = WorkingDirectory {
            foreground_pid: Some(4242),
            shell_pid: Some(shell.id()),
            spawn_dir: spawn.path().to_path_buf(),
        };
        let pinned = plan
            .open_with(
                |pid| {
                    if pid == 4242 {
                        CwdProbe::Gone
                    } else {
                        probe_process_cwd(pid)
                    }
                },
                process_group_members,
            )
            .expect("an unrelated odd name must not refuse the drop");
        assert_eq!(
            std::fs::canonicalize(pinned.path()).unwrap(),
            std::fs::canonicalize(shell_dir.path()).unwrap()
        );

        let _ = shell.kill();
        let _ = shell.wait();
        let _ = odd.kill();
        let _ = odd.wait();
    }

    /// Start a process that sits in `dir` until it is killed, and wait until it
    /// has actually got there. Deterministic: the marker is the child's own
    /// output, not a sleep.
    fn park_in(dir: &Path) -> std::process::Child {
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("echo ready && read _line")
            .current_dir(dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let mut stdout = child.stdout.take().expect("stdout");
        let mut buf = [0u8; 6];
        stdout.read_exact(&mut buf).expect("read ready marker");
        child.stdout = Some(stdout);
        child
    }

    fn shell_single_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    // ── lsof's answer is bytes, and bytes are what a path is ─────────────────
    //
    // These run everywhere, including on Linux where the parser they cover is
    // never called, because the branch that calls it is macOS-only and the only
    // machines available here are Linux. A parser nobody can exercise is a
    // parser nobody can check.

    #[test]
    fn reads_the_working_directory_out_of_lsof_field_output() {
        assert_eq!(
            lsof_cwd_path(b"p4242\nn/home/patrick/notes\n"),
            Some(PathBuf::from("/home/patrick/notes")),
            "the `n` field of the `cwd` descriptor is the directory"
        );
    }

    #[test]
    fn a_directory_that_is_not_valid_utf8_survives_lsof_exactly() {
        use std::os::unix::ffi::OsStrExt;

        // The bug this pins: decoding lsof's answer lossily replaced the 0xFF
        // with U+FFFD, so dux opened, wrote into and then NAMED a directory that
        // is not the one the terminal is in. A path is a byte string; the bytes
        // come through untouched, and a path that cannot be rendered as text is
        // refused later (`UnreportablePath::NotUtf8`) rather than substituted
        // here.
        let raw: &[u8] = b"p9\nn/tmp/od\xffd\n";
        let parsed = lsof_cwd_path(raw).expect("a non-UTF-8 directory still parses");
        assert_eq!(
            parsed.as_os_str().as_bytes(),
            b"/tmp/od\xffd",
            "the bytes must survive; got {parsed:?}"
        );
        assert!(
            !parsed.to_string_lossy().contains('\u{fffd}') || parsed.to_str().is_none(),
            "a replacement character must come from RENDERING it, never from parsing it"
        );
        assert!(
            parsed.to_str().is_none(),
            "and this path really is one that cannot be rendered, so the \
             assertion above is not vacuous"
        );
    }

    #[test]
    fn lsof_saying_nothing_useful_is_not_a_directory() {
        for empty in [
            &b""[..],
            // The process was gone by the time lsof looked: a process record and
            // no name field at all.
            &b"p4242\n"[..],
            // A name field that is empty names nothing, and `Path::new("")` is a
            // relative path that would resolve against dux's own directory.
            &b"p4242\nn\n"[..],
        ] {
            assert_eq!(
                lsof_cwd_path(empty),
                None,
                "must refuse rather than invent a directory from {empty:?}"
            );
        }
    }
}

#[cfg(test)]
mod independent_path_safety_check {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    /// A path dux cannot paste losslessly must be refused BEFORE anything is
    /// written. Saving a file and handing back a reference that names something
    /// else is worse than refusing the drop.
    #[test]
    fn a_directory_dux_cannot_name_is_refused_before_writing() {
        let root = tempfile::tempdir().expect("tempdir");
        for raw in [
            b"dir\nname".as_slice(),
            b"dir\x1bname".as_slice(),
            b"dir\xffname".as_slice(),
        ] {
            let name = std::ffi::OsStr::from_bytes(raw);
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).expect("create dir");
            let opened = DropDir::open(&dir);
            assert!(
                opened.is_err(),
                "must refuse a directory it cannot name: {:?}",
                dir
            );
            assert!(
                std::fs::read_dir(&dir).expect("read").next().is_none(),
                "and must not have written anything into it"
            );
        }
    }

    /// The shapes the review confirmed are SAFE must keep working, or the fix
    /// has over-corrected into refusing ordinary directories.
    #[test]
    fn ordinary_awkward_directories_still_work() {
        let root = tempfile::tempdir().expect("tempdir");
        for name in [
            "has space",
            "has$dollar",
            "has`backtick",
            "has'quote",
            "has(parens)",
        ] {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).expect("create dir");
            assert!(DropDir::open(&dir).is_ok(), "must still accept {name:?}");
        }
    }
}
