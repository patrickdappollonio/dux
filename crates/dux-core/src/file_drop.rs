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
//!   something unexpected is sitting there.
//! - **A failed or abandoned write removes what it partly wrote.**

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
    /// Pin `dir` by opening it as a directory.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        let fd = rustix::fs::open(
            dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let path = resolved_path_of(&fd).unwrap_or_else(|| dir.to_path_buf());
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
    fn is_symlink(&self, name: &str) -> bool {
        match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                rustix::fs::FileType::from_raw_mode(stat.st_mode as rustix::fs::RawMode)
                    == rustix::fs::FileType::Symlink
            }
            Err(_) => false,
        }
    }

    /// Remove `name` from this directory, ignoring failure. Used to clean up
    /// after a write that failed or was abandoned part-way.
    fn remove(&self, name: &str) {
        let _ = rustix::fs::unlinkat(&self.fd, name, AtFlags::empty());
    }
}

/// The path a directory handle actually points at.
///
/// Linux answers through `/proc/self/fd`. Elsewhere there is no portable way to
/// ask, and the caller falls back to the path it opened.
fn resolved_path_of(fd: &OwnedFd) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd())).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        None
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
                if dir.is_symlink(&candidate) {
                    return Err(DropSaveError::Symlink { name: candidate });
                }
                counter += 1;
                if counter > MAX_COLLISION_ATTEMPTS {
                    return Err(DropSaveError::NoUsableName);
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

impl WorkingDirectory {
    /// Walk the chain and pin the first directory that answers.
    pub fn open(&self) -> std::io::Result<DropDir> {
        for pid in self.foreground_pid.iter().chain(self.shell_pid.iter()) {
            if let Ok(dir) = open_process_cwd(*pid) {
                return Ok(dir);
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
pub fn open_process_cwd(pid: u32) -> std::io::Result<DropDir> {
    #[cfg(target_os = "linux")]
    {
        DropDir::open(Path::new(&format!("/proc/{pid}/cwd")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("lsof")
            .args(["-a", "-d", "cwd", "-Fn", "-p", &pid.to_string()])
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "lsof could not report the working directory of process {pid}"
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // `-Fn` prints one field per line, each prefixed by its field letter;
        // `n` is the name field, which for the `cwd` descriptor is the path.
        let cwd = text
            .lines()
            .find_map(|line| line.strip_prefix('n'))
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "lsof reported no working directory for process {pid}"
                ))
            })?;
        DropDir::open(Path::new(cwd))
    }
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
    fn name_max_is_asked_of_the_filesystem() {
        let dir = tmp();
        let pinned = DropDir::open(dir.path()).expect("pin");
        // Every filesystem dux runs on answers 255; the point of the assertion
        // is that a plausible value comes back rather than 0 or a huge number.
        assert!(
            (1..=4096).contains(&pinned.name_max()),
            "implausible name_max: {}",
            pinned.name_max()
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
}
