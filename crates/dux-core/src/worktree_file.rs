//! Safe working-copy file read/write for client-supplied worktree paths, used by
//! the web editor. The editor works against the WORKTREE itself: any file inside
//! it can be read, written, or CREATED: the only constraint is containment.
//! Reads use a read-permissive resolver that allows `.git/` paths (returning them
//! as `read_only`); writes keep the full guards and refuse `.git/` and outside-
//! resolving symlinks. There is no git-tracked/changed-file gate here: that is
//! the changes pane's concern.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::git::{is_under, resolve_worktree_path, resolves_into_git_dir};

/// Largest working copy the editor will load. Beyond this, Monaco bogs down and
/// the read would buffer the whole file into memory and a JSON response, so the
/// reader refuses instead. Source files are far smaller; this only excludes
/// generated blobs that happen to appear in `git status`.
pub const MAX_EDITABLE_BYTES: u64 = 5 * 1024 * 1024;

/// Largest file the editor will write. Twice the read cap, so a file that
/// opened can grow while it is edited; measured on the bytes that would land on
/// disk, after encoding, never on the text sent, which for a single-byte code
/// page can be three times larger (every € is one byte on disk and three in
/// UTF-8).
pub const MAX_SAVED_BYTES: u64 = 2 * MAX_EDITABLE_BYTES;

/// A save refused because the file it would write is past [`MAX_SAVED_BYTES`].
/// Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveTooLarge {
    /// The size the file would have been, in bytes.
    pub size: u64,
}

impl std::fmt::Display for SaveTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "This file is too large for the editor to save: it would be {} bytes on disk, \
             over the {} MB limit. Nothing was written. Edit it outside dux instead.",
            self.size,
            MAX_SAVED_BYTES / (1024 * 1024)
        )
    }
}

impl std::error::Error for SaveTooLarge {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeFile {
    pub path: String,
    /// True when the working copy is binary (see [`crate::text_encoding::decode`]);
    /// `content` is then empty and the editor refuses to open it.
    pub binary: bool,
    pub content: String,
    /// The WHATWG name of the encoding `content` was decoded from, which a save
    /// sends back so the file is written in it again. `None` for binary.
    pub encoding: Option<String>,
    /// The file began with a byte-order mark, which `content` does not hold and
    /// a save has to write back.
    pub bom: bool,
    /// The encoding is the fallback for bytes that are not UTF-8 (windows-1252,
    /// which reads every byte), not something the file said about itself; the
    /// editor tells the person so, since only they can know the real one.
    pub encoding_fallback: bool,
    /// Why the text cannot be saved in `encoding` without changing bytes nobody
    /// edited (`round_trip` or `doubled_mark`, see
    /// [`crate::text_encoding::Unwritable`]). The file is then `read_only`, and
    /// the way out is another encoding.
    pub unwritable: Option<&'static str>,
    /// The line endings are mixed or include a lone CR. The editor holds one
    /// style per file, so a save writes every break as `eol`.
    pub mixed_line_endings: bool,
    /// The line ending style the editor holds and a save writes, `LF` or
    /// `CRLF`. `None` for binary.
    pub eol: Option<&'static str>,
    /// The file was read in an encoding the person chose, other than UTF-8 or
    /// UTF-16, yet its bytes are valid UTF-8 with non-ASCII in them: most
    /// likely it was rewritten as UTF-8 since that choice was made.
    pub looks_like_utf8: bool,
    /// Read-only for where the file is (an outside or symlinked path, a `.git`
    /// path), which no encoding changes: there is no save of any kind. Apart
    /// from `unwritable`, which only rules out saving in this one encoding.
    pub path_read_only: bool,
    /// True when the file was opened read-only: an outside-resolving symlink
    /// or a `.git/` path (`path_read_only`), or text its encoding cannot write
    /// back unchanged (`unwritable`). The UI must grey out Save and ignore the
    /// dirty guard.
    #[serde(default)]
    pub read_only: bool,
    /// The mtime of the bytes in `content`, [`format_mtime`]'s string, taken
    /// from an `fstat` of the very descriptor the content was read from (see
    /// [`read_nofollow_stat`]). `None` when the filesystem reports no mtime.
    ///
    /// It is the mtime as of BEFORE the read, never after: a post-read stat
    /// would fold in a write that landed mid-read and hand the client a token
    /// claiming freshness for content it does not have. Stamping early can only
    /// err the safe way, by calling a fresh buffer stale.
    pub modified: Option<String>,
    /// The size of the bytes read, from the same `fstat`. Half of the freshness
    /// token; see [`FileStamp`] for why mtime alone will not do.
    pub size: Option<u64>,
}

/// What "the file I opened" means on the wire: an mtime plus a size.
///
/// The editor sends this back with a save so the server can refuse to overwrite
/// a file that moved underneath the buffer. Size is not decoration: mtime
/// granularity is coarse on some filesystems (and deliberately coarsened by
/// some kernels), so two writes inside one tick share a timestamp, which is
/// precisely the racing-writer case the guard exists for. Both come from the
/// same metadata call, so the second half is free.
///
/// Comparison is exact equality of both halves. When a filesystem reports no
/// mtime at all, both sides are `None` and the guard degrades to size-only;
/// that is stated rather than hidden, and it is still strictly better than the
/// unconditional write it replaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct FileStamp {
    pub modified: Option<String>,
    pub size: u64,
}

impl FileStamp {
    /// True when `self` still describes `other`, i.e. nothing observable about
    /// the file has moved.
    pub fn matches(&self, other: &FileStamp) -> bool {
        self.size == other.size && self.modified == other.modified
    }
}

/// A save refused because the file is not what the client last read.
///
/// A dedicated error type (like [`EntryMissing`]) so the HTTP layer can answer
/// 409 for it and carry the CURRENT stamp back: the browser needs it to offer
/// "overwrite anyway" without a second round trip, and to re-baseline if the
/// user reloads.
#[derive(Debug)]
pub struct WriteConflict {
    /// The file's stamp as it is NOW, or `None` when it is gone.
    pub current: Option<FileStamp>,
    /// True when the file was deleted underneath the editor. A separate rung
    /// from "changed": the answer is close-or-keep, not reload, and a guarded
    /// save must never silently resurrect a file someone removed.
    pub deleted: bool,
}

impl std::fmt::Display for WriteConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.deleted {
            write!(
                f,
                "the file was deleted on disk after you opened it; nothing was written"
            )
        } else {
            write!(
                f,
                "the file changed on disk after you opened it; nothing was written"
            )
        }
    }
}

impl std::error::Error for WriteConflict {}

/// The ONE mtime formatter. Every surface that puts an mtime in a payload goes
/// through it: the read route, the info panel, a save's success body, and a
/// conflict's body. chrono prints a variable number of fractional digits, so
/// two independent `to_rfc3339()` call sites can render the same instant as two
/// different strings and a token comparison would then never match. There is a
/// test asserting read, info and write all speak this one string.
pub fn format_mtime(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
}

/// The freshness stamp of an already-stat'd entry.
pub fn stamp_from_meta(meta: &std::fs::Metadata) -> FileStamp {
    FileStamp {
        modified: meta.modified().ok().map(format_mtime),
        size: meta.len(),
    }
}

/// Resolve `worktree/rel_path` for READ-only access, bypassing the literal
/// `.git`-component rejection so `.git/*` files can be opened. Returns
/// `(abs_path, is_git_dir, is_outside)`.
///
/// - `is_git_dir`: true when the canonical real path is inside a `.git` directory
///   (the caller must set `read_only = true`).
/// - `is_outside`: true when the canonical real path escapes the worktree via ANY
///   symlink, intermediate or leaf. The caller must set `read_only = true` and
///   skip the normal `O_NOFOLLOW` branch, reading from the resolved target instead.
///   A dangling symlink whose parent cannot be canonicalized is an error.
///
/// Traversal attacks (absolute paths, `..`) are still rejected with an error.
///
/// `pub` rather than `pub(crate)` because `dux-web`'s `file_routes` calls it for
/// the read-permissive resolver in `read_raw`.
pub fn resolve_worktree_path_for_read(
    worktree: &Path,
    rel_path: &str,
) -> anyhow::Result<(PathBuf, bool, bool)> {
    use std::path::Component;
    let rp = Path::new(rel_path);
    if rp.as_os_str().is_empty()
        || rp.is_absolute()
        || rp.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        anyhow::bail!("invalid worktree path: {rel_path}");
    }
    let joined = worktree.join(rel_path);

    // Canonicalize the FULL path so intermediate-directory symlinks (e.g.
    // `evil/ -> /etc`) are followed and containment is checked against the
    // real on-disk location, not the un-resolved textual path.
    //
    // For a leaf that does not yet exist (new file), canonicalize its parent
    // and re-attach the leaf component so we get a real path we can check.
    let real = match std::fs::canonicalize(&joined) {
        Ok(r) => r,
        Err(_) => {
            // Leaf may not exist. Try canonicalizing the parent.
            let parent = joined.parent().unwrap_or(worktree);
            let real_parent = std::fs::canonicalize(parent)
                .map_err(|e| anyhow::anyhow!("cannot resolve path {rel_path}: {e}"))?;
            let leaf = joined
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid path: {rel_path}"))?;
            real_parent.join(leaf)
        }
    };

    // Containment check against the canonical worktree.
    let real_worktree = std::fs::canonicalize(worktree)
        .map_err(|e| anyhow::anyhow!("cannot canonicalize worktree: {e}"))?;
    let is_outside = !real.starts_with(&real_worktree);

    // Check whether the literal path contains a `.git` component. We check this
    // first because the canonical path check below uses `real`, which for a
    // not-yet-existing leaf is the parent-canonicalized path: a `.git/new-file`
    // that doesn't exist would return false from the canonical check but true
    // from the literal check.
    let is_git_literal = rp
        .iter()
        .any(|c| c.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")));

    // Also check whether the canonical real path passes through a `.git`
    // directory, covering symlinks that redirect INTO a `.git/` dir even when
    // the literal path has no `.git` component.
    let is_git_canonical = real.strip_prefix(&real_worktree).is_ok_and(|rel| {
        rel.components().any(|comp| {
            comp.as_os_str()
                .to_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(".git"))
        })
    });

    let is_git = is_git_literal || is_git_canonical;
    Ok((joined, is_git, is_outside))
}

/// Read bytes from `abs_path` using `O_NOFOLLOW | O_RDONLY`, closing the
/// TOCTOU window between the caller's stat and the actual read. On Linux,
/// opening a symlink with `O_NOFOLLOW` fails with ELOOP. The caller must have
/// already confirmed (via stat) that the target is the intended file; if a race
/// causes the path to change to a symlink between stat and here, this open will
/// fail rather than silently following the new link.
///
/// This is `pub` (not `pub(crate)`) because `dux-web`'s `file_routes` uses it
/// for the TOCTOU-safe read in `read_raw` after `canonicalize()` (cross-crate call).
pub fn read_nofollow(abs_path: &Path) -> anyhow::Result<Vec<u8>> {
    read_nofollow_stat(abs_path).map(|(bytes, _)| bytes)
}

/// Open `abs_path` with `flags | O_NOFOLLOW` for the editor, refusing anything
/// that is not a regular file before a single byte moves.
///
/// The path can be swapped for a FIFO, a socket or a device between any check
/// and the open, and opening a FIFO with no writer blocks until one appears,
/// which for the editor is forever. So the open is non-blocking, the
/// descriptor's own `fstat` decides (a stat of the path would be about
/// whatever is there by then), and only a regular file goes on, with blocking
/// I/O restored. `on_open_error` words a failed open for its caller.
pub(crate) fn open_regular_nofollow(
    abs_path: &Path,
    flags: rustix::fs::OFlags,
    mode: rustix::fs::Mode,
    on_open_error: impl FnOnce(rustix::io::Errno) -> anyhow::Error,
) -> anyhow::Result<std::fs::File> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl, open as rustix_open};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    let fd = rustix_open(abs_path, flags | OFlags::NOFOLLOW | OFlags::NONBLOCK, mode)
        .map_err(on_open_error)?;
    // SAFETY: we own the fd returned by rustix_open; it is valid and open.
    let f = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    let kind = f.metadata()?.file_type();
    if !kind.is_file() {
        let what = if kind.is_fifo() {
            "a named pipe (FIFO)"
        } else if kind.is_socket() {
            "a socket"
        } else if kind.is_char_device() || kind.is_block_device() {
            "a device"
        } else if kind.is_dir() {
            "a directory"
        } else {
            "a special file"
        };
        anyhow::bail!(
            "not a regular file: {} is {what}, which the editor never reads or writes; \
             nothing was read or written",
            abs_path.display()
        );
    }
    let now = fcntl_getfl(&f)?;
    fcntl_setfl(&f, now - OFlags::NONBLOCK)?;
    Ok(f)
}

/// [`read_nofollow`] plus the `fstat` of the descriptor the bytes came from,
/// taken BEFORE the read.
///
/// The descriptor is what makes this worth having: a stat of the PATH names
/// whatever is at that path now, which after a rename or a replace is a
/// different file than the one that was read. `fstat` describes the open file
/// itself, so the stamp and the content are guaranteed to be about the same
/// inode. Taking it before the read rather than after is deliberate: a write
/// that lands mid-read leaves us reporting the older mtime for newer content,
/// which makes a fresh buffer look stale (a wasted re-check) instead of making
/// a stale buffer look fresh (a lost edit).
pub fn read_nofollow_stat(abs_path: &Path) -> anyhow::Result<(Vec<u8>, std::fs::Metadata)> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read;

    // O_NOFOLLOW | O_RDONLY, and only ever a regular file.
    let mut f = open_regular_nofollow(abs_path, OFlags::RDONLY, Mode::empty(), |e| {
        anyhow::anyhow!("open {}: {e}", abs_path.display())
    })?;
    let meta = f.metadata()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok((bytes, meta))
}

/// Read a worktree file's working copy as text, in the encoding its bytes say
/// (see [`crate::text_encoding::decode`]). A missing file is an error. Binary
/// content yields `binary: true` with empty `content`.
///
/// ANY symlink leaf is read with `read_only: true` regardless of where its
/// target lives: `write_file` refuses all symlinks so the Save button would
/// always fail for them; marking them read-only surfaces this immediately.
/// `.git/` paths are always `read_only: true`.
/// Dangling symlinks (target does not exist) return an error.
pub fn read_file(worktree: &Path, rel_path: &str) -> anyhow::Result<WorktreeFile> {
    read_file_as(worktree, rel_path, None)
}

/// [`read_file`] in exactly the `requested` encoding when one is given: the
/// person's answer to a fallback that read the file wrong. Bytes that are not
/// valid in it are refused (a [`crate::text_encoding::DecodeRefused`]) rather
/// than shown with replacement characters a save would write back.
pub fn read_file_as(
    worktree: &Path,
    rel_path: &str,
    requested: Option<&'static encoding_rs::Encoding>,
) -> anyhow::Result<WorktreeFile> {
    // Use the read-permissive resolver (allows .git/ paths).
    // `is_outside` is true when the canonical real path escapes the worktree
    // via ANY symlink (intermediate or leaf), so an intermediate-dir symlink
    // (e.g. `evil/ -> /etc`) is caught here before we ever reach the leaf check.
    let (path, is_git_dir, is_outside) = resolve_worktree_path_for_read(worktree, rel_path)?;

    // No-follow stat: tells us (a) the entry kind, (b) whether it is a symlink,
    // and (c) the size. For regular files the no-follow open below is identical
    // to a regular open. For symlinks we need to additionally resolve the target
    // to check containment.
    let meta = std::fs::symlink_metadata(&path)?;

    let (bytes, read_meta, read_only) = if meta.file_type().is_symlink() || is_outside {
        // Either the leaf itself is a symlink, OR an intermediate path component
        // is a symlink that escapes the worktree. In both cases, resolve to the
        // canonical real path and read from there.
        //
        // For an intermediate-symlink escape, `path` is not itself a symlink at
        // the leaf but the canonical resolution already confirmed the real path
        // is outside. We still read via the resolved path so O_NOFOLLOW on
        // `path` would succeed (the leaf is a plain file), but reading via
        // `canonicalize` is the consistent, safe choice in both cases.
        let target = std::fs::canonicalize(&path)?; // follows all symlinks

        // Stat the TARGET for the size check.
        let target_meta = std::fs::metadata(&target)?;
        if target_meta.len() > MAX_EDITABLE_BYTES {
            anyhow::bail!(
                "file too large to edit: {} bytes (limit {MAX_EDITABLE_BYTES})",
                target_meta.len()
            );
        }

        // Open the TARGET file with O_NOFOLLOW pointing at the already-resolved
        // target path. If the target changed to yet another symlink between
        // canonicalize() and here, O_NOFOLLOW refuses it (the race window is
        // millisecond-scale and the failure is safe).
        let (bytes, read_meta) = read_nofollow_stat(&target)?;

        // `read_only` is true when: the leaf is ANY symlink (write_file refuses
        // all symlinks, so Save would always fail; mark it read-only up front),
        // OR the real path is outside the worktree (is_outside), OR the path is
        // inside .git/.
        (
            bytes,
            read_meta,
            meta.file_type().is_symlink() || is_outside || is_git_dir,
        )
    } else {
        if meta.len() > MAX_EDITABLE_BYTES {
            anyhow::bail!(
                "file too large to edit: {} bytes (limit {MAX_EDITABLE_BYTES})",
                meta.len()
            );
        }
        // Regular file (or other non-symlink kind) confirmed to be inside the
        // worktree by the resolver. Use O_NOFOLLOW so a time-of-check /
        // time-of-use race that replaces the file with a symlink between our
        // stat and open fails safely.
        let (bytes, read_meta) = read_nofollow_stat(&path)?;
        (bytes, read_meta, is_git_dir)
    };

    // The stamp of the bytes just read, from the fstat of the descriptor they
    // came from. This is the token a later save sends back.
    let stamp = stamp_from_meta(&read_meta);
    // Binary content goes out as binary, which the editor refuses to open.
    // Never fall back to an empty string here: the editor would show an empty,
    // editable buffer, and the stamp it carries is a valid one, so a save would
    // replace the file. Text that cannot be written back unchanged in its
    // encoding is still text, and goes out read-only instead.
    let decoded = crate::text_encoding::decode(&bytes, requested).map_err(anyhow::Error::new)?;
    let looks_like_utf8 = requested.is_some_and(|e| {
        e != encoding_rs::UTF_8 && e != encoding_rs::UTF_16LE && e != encoding_rs::UTF_16BE
    }) && crate::text_encoding::looks_like_utf8(&bytes);
    let text = match decoded {
        crate::text_encoding::Decoded::Text(t) => Some(t),
        crate::text_encoding::Decoded::Binary => None,
    };
    let endings = text
        .as_ref()
        .map(|t| crate::text_encoding::line_endings(&t.text));
    let unwritable = text
        .as_ref()
        .and_then(|t| t.unwritable)
        .map(super::text_encoding::Unwritable::as_str);
    Ok(WorktreeFile {
        path: rel_path.to_string(),
        binary: text.is_none(),
        encoding: text.as_ref().map(|t| t.encoding.name().to_string()),
        bom: text.as_ref().is_some_and(|t| t.bom),
        encoding_fallback: text.as_ref().is_some_and(|t| t.fallback),
        unwritable,
        mixed_line_endings: endings.is_some_and(|e| e.mixed),
        eol: endings.map(|e| e.saves_as.label()),
        looks_like_utf8,
        content: text.map(|t| t.text).unwrap_or_default(),
        // A text the encoding cannot write back unchanged is shown, never
        // saved in it: the same read-only the editor gives a symlink.
        read_only: read_only || unwritable.is_some(),
        path_read_only: read_only,
        modified: stamp.modified,
        size: Some(stamp.size),
    })
}

/// Write `content` to `abs_path` with `O_WRONLY | O_CREAT | O_NOFOLLOW`, truncating once
/// the descriptor is known to be a regular file (see [`open_regular_nofollow`]),
/// the write-side counterpart of [`read_nofollow`]. The single open is what
/// enforces the no-symlink rule: an ordinary truncating write follows a symlink
/// at the leaf, so a stat that says "regular file" is worthless by the time the
/// write runs if the entry can be replaced in between. With `O_NOFOLLOW` a
/// symlink leaf fails the open with `ELOOP` and nothing is written, whether the
/// link was there all along or appeared a microsecond ago.
///
/// The permission bits match `std::fs::write` (0o666, masked by the process
/// umask); they apply only when the file is created.
pub fn write_nofollow(abs_path: &Path, content: &str) -> anyhow::Result<()> {
    write_nofollow_stamped(abs_path, content).map(|_| ())
}

/// [`write_nofollow`] plus the stamp of the file it just wrote, taken by
/// `fstat` on the same descriptor. That stamp is what a client re-baselines on
/// so its OWN save never looks like somebody else's edit on the next check.
pub fn write_nofollow_stamped(abs_path: &Path, content: &str) -> anyhow::Result<FileStamp> {
    write_nofollow_bytes_stamped(abs_path, content.as_bytes())
}

/// [`write_nofollow_stamped`] for text already encoded into bytes.
fn write_nofollow_bytes_stamped(abs_path: &Path, content: &[u8]) -> anyhow::Result<FileStamp> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Write;

    // No O_TRUNC in the open: truncation waits until the descriptor is known
    // to be a regular file.
    let mut f = open_regular_nofollow(
        abs_path,
        OFlags::WRONLY | OFlags::CREATE,
        Mode::from_raw_mode(0o666),
        |e| open_for_writing_error(e, abs_path),
    )?;
    f.set_len(0)?;
    f.write_all(content)?;
    Ok(stamp_from_meta(&f.metadata()?))
}

/// Shared phrasing for a failed write-side open, so the guarded and unguarded
/// paths cannot drift on the one message a user actually sees.
fn open_for_writing_error(e: rustix::io::Errno, abs_path: &Path) -> anyhow::Error {
    if e == rustix::io::Errno::LOOP {
        anyhow::anyhow!(
            "refusing to write through a symlink: {}",
            abs_path.display()
        )
    } else {
        anyhow::anyhow!("open {} for writing: {e}", abs_path.display())
    }
}

/// Write `content` over an EXISTING file only if it still matches `expected`.
///
/// Everything hangs off one descriptor. Stat-then-[`write_nofollow`] cannot work:
/// that open truncates, so the other writer's work is gone before anything can be
/// compared, and a stat is a statement about the path rather than about the file
/// the write lands on. So: open `O_RDWR | O_NOFOLLOW` with NEITHER `O_CREAT` nor
/// `O_TRUNC`, `fstat` that descriptor, compare, check that the bytes it holds are
/// valid UTF-8 the editor could have shown, and only then `ftruncate` and write through
/// it.
///
/// `ENOENT` is a conflict, not a create: with a token in hand the client is saying
/// "update the file I read", and that file is gone. Resurrecting it would undo a
/// deletion the user never saw.
///
/// The window this does NOT close is the human one, between the read that produced
/// `expected` and the save; that is the window the guard REPORTS on rather than
/// prevents. What remains after the guard is the microseconds between the `fstat`
/// and the `ftruncate` on the same descriptor.
fn write_checked_existing(
    abs_path: &Path,
    content: &[u8],
    expected: &FileStamp,
    encoding: Option<SaveEncoding>,
) -> anyhow::Result<FileStamp> {
    use rustix::fs::{Mode, OFlags};
    use std::io::{Read, Seek, Write};

    // Read-write rather than write-only: the bytes about to be replaced are
    // checked through this same descriptor below.
    let mut f = open_regular_nofollow(abs_path, OFlags::RDWR, Mode::empty(), |e| {
        if e == rustix::io::Errno::NOENT {
            anyhow::Error::new(WriteConflict {
                current: None,
                deleted: true,
            })
        } else {
            open_for_writing_error(e, abs_path)
        }
    })?;
    let current = stamp_from_meta(&f.metadata()?);
    if !expected.matches(&current) {
        return Err(anyhow::Error::new(WriteConflict {
            current: Some(current),
            deleted: false,
        }));
    }
    // A stamp proves the client holds this version of the file, not that it
    // was ever shown the content. A file that is not valid UTF-8 is served to
    // the editor with no content, so a buffer holding a stamp for one (a draft
    // the browser kept from before such files were refused, or a client that
    // ignores the flag) was typed over nothing, and saving it would replace
    // bytes nobody saw.
    //
    // The check is deliberately narrow. Only that case is refused: a file
    // with a NUL was always served without content, so it never had the
    // empty-buffer problem, and content the editor wrote itself must stay
    // saveable. Past the read cap the file could not have been read at all, so
    // a matching stamp there can only be one this client got back from its own
    // save, which the route allows to grow past the cap.
    //
    // A save that states its encoding comes from a client that reads files in
    // their own encodings, so it was shown the text it is saving, and that
    // check does not apply: that is what lets a windows-1252 file be saved
    // back as windows-1252, or converted to UTF-8 on purpose. A save with no
    // encoding (an older page, or a draft one kept) is still held to it.
    //
    // A stated encoding is held to its own check instead, rather than trusting
    // the client to have honoured the read's read-only: when the bytes on disk
    // read in the save's encoding but that encoding cannot write them back
    // unchanged, the save would quietly rewrite characters nobody edited, so it
    // is refused. Bytes that do not read in it are a conversion from another
    // encoding, a whole-file rewrite the person asked for.
    if current.size <= MAX_EDITABLE_BYTES {
        let mut existing = Vec::new();
        (&f).take(MAX_EDITABLE_BYTES).read_to_end(&mut existing)?;
        match encoding {
            None if std::str::from_utf8(&existing).is_err() => anyhow::bail!(
                "refusing to save: the file on disk is not UTF-8 text, so the editor never \
                 showed its contents and saving would replace them; nothing was written"
            ),
            Some(save) => {
                // Whose round trip matters depends on what the save is. In the
                // encoding the file was read in, it rewrites those same bytes,
                // which must come back unchanged. In another one it is a
                // conversion: the old bytes are replaced whole, so only their
                // reading has to be honest, and the new text answers for
                // itself (the encoder's own exactness check).
                let (judge_in, conversion) = match save.read_as {
                    Some((read, read_bom)) => (read, (read, read_bom) != (save.encoding, save.bom)),
                    // An older client says nothing about its read: judged in
                    // the target encoding, the conservative way.
                    None => (save.encoding, false),
                };
                // Whatever the save is, a read encoding the client states must
                // decode the bytes on disk: if it cannot, the text being saved
                // was not read from these bytes in it. (An older client states
                // none; its target-encoding check below treats bytes that do
                // not decode as a conversion, the conservative old behaviour.)
                if (conversion || save.read_as.is_some())
                    && crate::text_encoding::decode(&existing, Some(judge_in)).is_err()
                {
                    anyhow::bail!(
                        "refusing to save: the file on disk is not valid {}, the encoding \
                         this save says it was read in, so it was not shown as it is; \
                         nothing was written. Reopen it and try again",
                        judge_in.name()
                    );
                }
                if conversion {
                    // The old bytes read honestly and are replaced whole.
                } else if let Some(reason) =
                    crate::text_encoding::would_rewrite_untouched(&existing, judge_in)
                {
                    return Err(anyhow::Error::new(
                        crate::text_encoding::WouldRewriteUntouched {
                            encoding: judge_in,
                            reason,
                        },
                    ));
                }
            }
            None => {}
        }
        f.rewind()?;
    }
    // Truncate through the same descriptor rather than reopening: the check
    // above is only worth something if nothing between it and the write can
    // point at a different file.
    f.set_len(0)?;
    f.write_all(content)?;
    Ok(stamp_from_meta(&f.metadata()?))
}

/// Write text to a worktree file, creating it if it does not exist (the editor
/// can save brand-new, uncommitted files). The only constraint is containment:
/// the target (and, when creating, its parent directory) must stay inside the
/// worktree. Refuses to write THROUGH a symlink (an existing one, or a dangling
/// one whose target could appear between the boundary's existence check and the
/// write) and refuses to write to a directory/fifo/device.
///
/// The stat below is what produces the readable per-case error messages; it is
/// NOT what enforces the symlink rule, because an entry can be replaced between
/// the stat and the write. The [`write_nofollow`] open is the enforcement, and
/// it refuses a symlink leaf no matter when the link appeared.
pub fn write_file(worktree: &Path, rel_path: &str, content: &str) -> anyhow::Result<FileStamp> {
    write_file_checked(worktree, rel_path, content, None)
}

/// [`write_file`] with an optional freshness guard.
///
/// The guard is opt-in BY PRESENCE: with `expected: None` this is byte for byte
/// the old unconditional write, so the upload/drop route, an older browser tab,
/// and any future writer keep working exactly as they did. With a stamp, the
/// write is refused (a [`WriteConflict`]) unless the file still matches it; see
/// [`write_checked_existing`] for why the comparison and the truncation have to
/// share one descriptor.
///
/// Returns the file's stamp AFTER the write either way, so the caller can hand
/// it back to the client as its new baseline.
pub fn write_file_checked(
    worktree: &Path,
    rel_path: &str,
    content: &str,
    expected: Option<&FileStamp>,
) -> anyhow::Result<FileStamp> {
    write_file_encoded(worktree, rel_path, content, None, expected)
}

/// The encoding a save writes its text in, and whether the bytes start with a
/// byte-order mark: the pair a read handed out, or a conversion the person
/// chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveEncoding {
    pub encoding: &'static encoding_rs::Encoding,
    pub bom: bool,
    /// The encoding (and mark) the client read the file in, which tells an
    /// ordinary save (the same pair) from a conversion (any other). `None`
    /// from an older client, which is held to the conservative check.
    pub read_as: Option<(&'static encoding_rs::Encoding, bool)>,
}

/// Mark a refused leading U+FEFF as the file's own when the file on disk,
/// read in the encoding the save says it was read in, starts with that
/// character: the editor opened such a file read-only, so the refusal must not
/// tell the person to delete a character they cannot reach. Any other error,
/// or a file that cannot be read for this, is returned as it was; the answer
/// only chooses the refusal's wording, never whether anything is written.
fn leading_mark_from_file(
    err: anyhow::Error,
    worktree: &Path,
    rel_path: &str,
    save: SaveEncoding,
) -> anyhow::Error {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read;

    let Some(refused) = err.downcast_ref::<crate::text_encoding::LeadingMarkInText>() else {
        return err;
    };
    let Some((read, _)) = save.read_as else {
        return err;
    };
    let Ok(path) = resolve_worktree_path(worktree, rel_path) else {
        return err;
    };
    let Ok(f) = open_regular_nofollow(&path, OFlags::RDONLY, Mode::empty(), |e| {
        anyhow::anyhow!("open {}: {e}", path.display())
    }) else {
        return err;
    };
    let mut existing = Vec::new();
    if f.take(MAX_EDITABLE_BYTES)
        .read_to_end(&mut existing)
        .is_err()
    {
        return err;
    }
    let from_file = matches!(
        crate::text_encoding::decode(&existing, Some(read)),
        Ok(crate::text_encoding::Decoded::Text(t)) if t.text.starts_with('\u{feff}')
    );
    if !from_file {
        return err;
    }
    anyhow::Error::new(crate::text_encoding::LeadingMarkInText {
        encoding: refused.encoding,
        from_file: true,
    })
}

/// [`write_file_checked`] with the text written in `encoding` rather than as
/// its own UTF-8.
///
/// The text is encoded before anything else happens, so a character the
/// encoding cannot hold is refused (a [`crate::text_encoding::Unmappable`],
/// returned unwrapped so the HTTP layer can answer with its details) before the
/// file is so much as opened. `None` is the old behaviour exactly: the text's
/// own UTF-8 bytes, and the guard against saving over bytes that are not UTF-8.
pub fn write_file_encoded(
    worktree: &Path,
    rel_path: &str,
    content: &str,
    encoding: Option<SaveEncoding>,
    expected: Option<&FileStamp>,
) -> anyhow::Result<FileStamp> {
    let bytes: std::borrow::Cow<'_, [u8]> = match encoding {
        Some(save) => crate::text_encoding::encode(content, save.encoding, save.bom)
            .map_err(|e| leading_mark_from_file(e, worktree, rel_path, save))?
            .into(),
        None => content.as_bytes().into(),
    };
    if bytes.len() as u64 > MAX_SAVED_BYTES {
        return Err(anyhow::Error::new(SaveTooLarge {
            size: bytes.len() as u64,
        }));
    }
    let path = resolve_worktree_path(worktree, rel_path)?;
    // No-follow stat tells existing-file kind apart from "does not exist".
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("refusing to write through a symlink: {rel_path}");
        }
        Ok(meta) if meta.is_file() => {
            // Overwrite an existing regular file.
        }
        Ok(_) => anyhow::bail!("not a regular file: {rel_path}"),
        // With a token in hand this is not a create at all, whatever the
        // filesystem looks like: the client is saying "update the file I
        // read", and that file is gone. Validating the parent first answered a
        // question nobody asked, and when the parent had gone too (an agent
        // removing a whole directory, the common way this happens) the browser
        // got a create-validation refusal instead of the deleted conflict the
        // save-conflict dialog knows how to offer. `write_checked_existing`
        // opens without O_CREAT, so it reports the conflict rather than
        // resurrecting anything.
        Err(_) if expected.is_some() => {}
        Err(_) => {
            // Creating a new file: the parent directory must already exist and
            // resolve INSIDE the worktree. `is_under` canonicalizes it, so a
            // symlinked/escaping parent (which the boundary's existence check
            // skips for a not-yet-existing target) is rejected here, and a
            // missing parent fails too (no implicit `mkdir -p`).
            let parent = path.parent().unwrap_or(worktree);
            if !is_under(worktree, parent) {
                anyhow::bail!(
                    "cannot create file: parent directory is missing or outside the worktree: {rel_path}"
                );
            }
            // ...and must not resolve into a `.git` dir via a symlinked parent
            // (the literal `.git` check in resolve_worktree_path can't see that).
            if resolves_into_git_dir(worktree, parent) {
                anyhow::bail!("refusing to create a file inside the git directory: {rel_path}");
            }
        }
    }
    match expected {
        // A conflict must reach the caller AS a `WriteConflict` (the HTTP layer
        // downcasts it for the 409 and its body), so it is the one error here
        // that is not re-wrapped with the path suffix; its own message already
        // says what happened.
        Some(stamp) => write_checked_existing(&path, &bytes, stamp, encoding).map_err(|e| {
            if e.downcast_ref::<WriteConflict>().is_some() {
                e
            } else {
                anyhow::anyhow!("{e} (writing {rel_path})")
            }
        }),
        None => write_nofollow_bytes_stamped(&path, &bytes)
            .map_err(|e| anyhow::anyhow!("{e} (writing {rel_path})")),
    }
}

/// Create a new EMPTY file at `rel_path`. Refuses to overwrite an existing entry
/// (file, dir, or symlink), refuses `.git`/traversal/escape, and, like
/// `write_file`, requires the parent to already exist inside the worktree (no
/// implicit `mkdir -p`). Uses `File::create_new` so the create itself is
/// TOCTOU-safe: if another process creates the file in the race window between
/// our existence check and the actual create, this call fails instead of
/// silently overwriting.
pub fn create_file(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
    let path = resolve_worktree_path(worktree, rel_path)?;
    if path.symlink_metadata().is_ok() {
        anyhow::bail!("refusing to create file, entry already exists: {rel_path}");
    }
    let parent = path.parent().unwrap_or(worktree);
    if !is_under(worktree, parent) {
        anyhow::bail!(
            "cannot create file: parent directory is missing or outside the worktree: {rel_path}"
        );
    }
    if resolves_into_git_dir(worktree, parent) {
        anyhow::bail!("refusing to create a file inside the git directory: {rel_path}");
    }
    std::fs::File::create_new(&path)
        .map_err(|e| anyhow::anyhow!("cannot create file {rel_path}: {e}"))?;
    Ok(())
}

/// Create a new directory at `rel_path`, creating missing intermediate
/// components (VS Code "New Folder: a/b/c"). Refuses to overwrite an existing
/// entry, refuses `.git`/traversal, and refuses if the nearest existing
/// ancestor escapes the worktree or resolves into `.git`.
pub fn create_dir(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
    let path = resolve_worktree_path(worktree, rel_path)?;
    if path.symlink_metadata().is_ok() {
        anyhow::bail!("refusing to create directory, entry already exists: {rel_path}");
    }
    // The target does not exist yet, so `resolve_worktree_path`'s existence-gated
    // containment check did not run for it. Walk up to the nearest ancestor that
    // DOES exist and check containment there instead, since intermediate
    // components may be created and any of them could be a symlink.
    let mut ancestor = path.as_path();
    loop {
        match ancestor.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                if parent.exists() {
                    ancestor = parent;
                    break;
                }
                ancestor = parent;
            }
            _ => {
                ancestor = worktree;
                break;
            }
        }
    }
    if !is_under(worktree, ancestor) {
        anyhow::bail!("cannot create directory: outside the worktree: {rel_path}");
    }
    if resolves_into_git_dir(worktree, ancestor) {
        anyhow::bail!("refusing to create a directory inside the git directory: {rel_path}");
    }
    create_dir_from_root(worktree, rel_path)
}

/// Create `rel_path`'s missing components one at a time, each relative to the
/// directory handle of the one before it, starting from a handle on the ROOT
/// opened without creating it.
///
/// `create_dir_all` on the joined path would happily recreate the root itself:
/// a worktree removed while the request was in flight came back as an empty
/// directory with no git registration behind it. Pinned this way, a root that
/// is gone fails the open, and one removed after the open fails the first
/// `mkdirat` (an unlinked directory takes no new entries), so the root is never
/// recreated. Symlinked intermediate directories are still followed, exactly
/// as before; containment was checked by the caller.
fn create_dir_from_root(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags};
    let mut fd = rustix::fs::open(
        worktree,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "cannot create directory {rel_path}: the folder it belongs in is gone ({e}). It may \
             have been removed."
        )
    })?;
    #[cfg(test)]
    tests::run_before_create_hook();
    let components: Vec<_> = Path::new(rel_path).components().collect();
    for (index, component) in components.iter().enumerate() {
        let name = component.as_os_str();
        let last = index + 1 == components.len();
        // `mkdirat` applies the umask, as `create_dir_all` did.
        match rustix::fs::mkdirat(&fd, name, Mode::from_bits_truncate(0o777)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) if !last => {}
            Err(rustix::io::Errno::EXIST) => {
                anyhow::bail!("refusing to create directory, entry already exists: {rel_path}")
            }
            Err(e) => anyhow::bail!("cannot create directory {rel_path}: {e}"),
        }
        if !last {
            fd = rustix::fs::openat(
                &fd,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| anyhow::anyhow!("cannot create directory {rel_path}: {e}"))?;
        }
    }
    Ok(())
}

/// Rename/move `from_rel` to `to_rel` (file or directory). Refuses: a missing
/// source, an existing destination (no overwrite), `.git`/traversal/escape on
/// either side, and a destination whose parent is missing or escapes the
/// worktree or resolves into `.git`.
///
/// The SOURCE is resolved WITHOUT following it, exactly as [`delete_entry`]
/// does and for the same reason: `rename(2)` moves the directory ENTRY, so
/// moving a symlink moves the link and never touches whatever it points at.
/// Resolving through it would refuse to move a link whose target escapes the
/// worktree, an entry the tree already shows and the user can already delete,
/// and that refusal would have named a file the user never touched. Only the
/// literal path and its PARENT's containment matter for the source.
///
/// The DESTINATION keeps the following resolver, because a destination reached
/// through a symlinked directory really would write outside the tree.
///
/// It moves only with a clearance for both ends (see [`crate::destructive`]):
/// `clear` is asked once the move is otherwise valid, right before anything
/// moves. A symbolic link is judged at its own path, never followed.
pub fn rename_entry<'c>(
    worktree: &Path,
    from_rel: &str,
    to_rel: &str,
    clear: impl FnOnce() -> Result<crate::destructive::Cleared<'c>, crate::destructive::Refused>,
) -> anyhow::Result<()> {
    let src = entry_literal_path(worktree, from_rel)?;
    let dst = resolve_worktree_path(worktree, to_rel)?;
    src.symlink_metadata()
        .map_err(|e| anyhow::anyhow!("rename source does not exist: {from_rel}: {e}"))?;
    check_entry_parent_contained(worktree, &src, from_rel)?;
    let dst_parent = dst.parent().unwrap_or(worktree);
    if !is_under(worktree, dst_parent) {
        anyhow::bail!(
            "cannot rename: destination's parent directory is missing or outside the worktree: {to_rel}"
        );
    }
    if resolves_into_git_dir(worktree, dst_parent) {
        anyhow::bail!("refusing to rename into the git directory: {to_rel}");
    }
    // A link too: moving one leaves its target where it is, but an agent's
    // folder or a project's repository can be the link itself.
    let cleared = clear().map_err(anyhow::Error::new)?;
    cleared.require(&src)?;
    cleared.require(&dst)?;
    rename_no_replace(&src, &dst).map_err(|e| match e {
        RenameNoReplaceError::DestinationExists => {
            anyhow::anyhow!("refusing to rename, destination already exists: {to_rel}")
        }
        RenameNoReplaceError::Failed(err) => {
            anyhow::anyhow!("cannot rename {from_rel} to {to_rel}: {err}")
        }
    })
}

/// Why [`rename_no_replace`] did not rename. The occupied-destination case is
/// its own variant because the caller phrases it differently: it is a refusal
/// dux promises, not an I/O failure.
#[derive(Debug)]
enum RenameNoReplaceError {
    DestinationExists,
    Failed(std::io::Error),
}

/// Which of the two code paths inside [`rename_no_replace_reporting`] actually
/// answered. It exists to make the choice OBSERVABLE, because the result alone
/// cannot tell the two apart: for every input a test can construct, a stat
/// followed by [`std::fs::rename`] returns exactly the same
/// [`RenameNoReplaceError::DestinationExists`] that the kernel flag returns.
/// The difference is only WHICH code ran, so that is what gets reported and
/// asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameMechanism {
    /// `renameat_with(RenameFlags::NOREPLACE)` answered, by renaming or by
    /// refusing. Both supported platforms take this path.
    NoReplaceSyscall,
    /// The flag is unimplemented here, so the older stat-then-rename pair ran.
    StatThenRename,
}

/// Rename `src` onto `dst`, refusing an occupied destination in one syscall.
///
/// A stat followed by [`std::fs::rename`] is not the same promise: `rename(2)`
/// silently overwrites (measured: file-onto-file and directory-onto-empty-
/// directory both succeed), so another client can create the destination in the
/// window between the two calls and lose it, and dux is explicitly multi-client
/// with no trash to recover from. `renameat_with(RenameFlags::NOREPLACE)` puts the
/// refusal in the rename call itself. rustix maps that flag to `RENAME_NOREPLACE`
/// on Linux and to `renameatx_np`'s `RENAME_EXCL` on macOS, which are dux's two
/// supported targets.
///
/// The fallback is narrow: a filesystem with no `renameat2` (and macOS before
/// 10.12, where rustix finds no `renameatx_np` and answers `ENOSYS`) reports
/// `ENOSYS`/`EINVAL`/`ENOTSUP`, and only there does this stat first and rename
/// after, which is the older racy pair. There and only there does the TOCTOU
/// window still exist.
fn rename_no_replace(src: &Path, dst: &Path) -> Result<(), RenameNoReplaceError> {
    rename_no_replace_reporting(src, dst).1
}

/// [`rename_no_replace`] plus the branch it took. The wrapper above is what
/// production calls, and it discards the mechanism; only the tests read it.
fn rename_no_replace_reporting(
    src: &Path,
    dst: &Path,
) -> (RenameMechanism, Result<(), RenameNoReplaceError>) {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => (RenameMechanism::NoReplaceSyscall, Ok(())),
        Err(err) if err == Errno::EXIST || err == Errno::NOTEMPTY => (
            RenameMechanism::NoReplaceSyscall,
            Err(RenameNoReplaceError::DestinationExists),
        ),
        Err(err)
            if err == Errno::NOSYS
                || err == Errno::INVAL
                || err == Errno::NOTSUP
                || err == Errno::OPNOTSUPP =>
        {
            if dst.symlink_metadata().is_ok() {
                return (
                    RenameMechanism::StatThenRename,
                    Err(RenameNoReplaceError::DestinationExists),
                );
            }
            (
                RenameMechanism::StatThenRename,
                std::fs::rename(src, dst).map_err(RenameNoReplaceError::Failed),
            )
        }
        Err(err) => (
            RenameMechanism::NoReplaceSyscall,
            Err(RenameNoReplaceError::Failed(err.into())),
        ),
    }
}

/// Join `rel_path` onto the worktree for an operation that acts on the
/// directory ENTRY rather than on what it points at (delete, and a rename's
/// source). Applies the literal guards only, and deliberately does NOT follow
/// the leaf: see [`delete_entry`] and [`rename_entry`] for why the leaf may
/// legitimately be a symlink pointing anywhere at all.
fn entry_literal_path(worktree: &Path, rel_path: &str) -> anyhow::Result<PathBuf> {
    use std::path::Component;
    let rp = Path::new(rel_path);
    if rp.as_os_str().is_empty()
        || rp.is_absolute()
        || rp.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
        // `Path::components()` normalizes away a non-leading `.` segment, so
        // `sub/.` does NOT surface a `Component::CurDir` above even though the
        // raw string contains one. Check the raw, unnormalized string instead.
        // The hole this closes: `delete_entry(worktree,
        // "<symlink>/.")` let a trailing `.` make `symlink_metadata` dereference
        // the preceding symlink (POSIX resolves `.` against the target
        // directory, so it reports is_dir=true, is_symlink=false) AND made
        // `Path::parent()` on the joined path strip the symlink component too,
        // so the parent-containment check below ran against the always-safe
        // worktree root while `remove_dir_all` acted on the symlink's external
        // target's contents. See `delete_refuses_curdir_component` for the
        // regression test.
        || rel_path.split('/').any(|seg| seg == ".")
    {
        anyhow::bail!("invalid worktree path: {rel_path}");
    }
    if rp
        .iter()
        .any(|c| c.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")))
    {
        anyhow::bail!("refusing to access the git directory: {rel_path}");
    }
    Ok(worktree.join(rel_path))
}

/// Containment for an entry resolved by [`entry_literal_path`]: checked on the
/// PARENT directory (canonicalized, so an intermediate symlink that escapes
/// the worktree is still caught), not on the leaf, since the leaf may
/// legitimately be an escaping symlink.
fn check_entry_parent_contained(
    worktree: &Path,
    path: &Path,
    rel_path: &str,
) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or(worktree);
    if !is_under(worktree, parent) {
        anyhow::bail!("path escapes worktree: {rel_path}");
    }
    if resolves_into_git_dir(worktree, parent) {
        anyhow::bail!("refusing to access the git directory: {rel_path}");
    }
    Ok(())
}

/// Delete `rel_path`: a file or symlink is removed with `remove_file` (a
/// symlink removes the LINK, never its target); a directory is removed
/// recursively with `remove_dir_all`. Refuses `.git`/traversal/escape and
/// refuses to delete the worktree root itself. Permanent: there is no trash on
/// the server (CLAUDE.md: worktrees are user data, but deletion here is the
/// explicit, confirmed action the caller asked for).
///
/// Deliberately does NOT call `resolve_worktree_path`: that resolver checks
/// containment of the FOLLOWED realpath, which is the wrong check for delete.
/// Deleting a symlink removes the directory entry (the link), never its
/// target, so an escaping-target symlink is a legitimate delete target: only
/// the literal path and its PARENT's containment matter here.
///
/// It goes only with a clearance for exactly that path (see
/// [`crate::destructive`]): `clear` is asked once the delete is otherwise
/// valid, right before anything is removed. A symbolic link is judged at its
/// own path, never followed.
pub fn delete_entry<'c>(
    worktree: &Path,
    rel_path: &str,
    clear: impl FnOnce() -> Result<crate::destructive::Cleared<'c>, crate::destructive::Refused>,
) -> anyhow::Result<()> {
    let path = entry_literal_path(worktree, rel_path)?;
    // No-follow stat on the literal path: existence and kind of the entry
    // ITSELF, never its symlink target.
    let meta = path
        .symlink_metadata()
        .map_err(|e| anyhow::anyhow!("delete target does not exist: {rel_path}: {e}"))?;
    check_entry_parent_contained(worktree, &path, rel_path)?;
    let canon_worktree = worktree
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot canonicalize worktree: {e}"))?;
    if let Ok(canon_path) = path.canonicalize()
        && canon_path == canon_worktree
    {
        anyhow::bail!("refusing to delete the worktree root");
    }
    // A link too: deleting one leaves its target where it is, but an agent's
    // folder or a project's repository can be the link itself.
    clear().map_err(anyhow::Error::new)?.require(&path)?;
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("cannot delete {rel_path}: {e}"))?;
    } else {
        std::fs::remove_dir_all(&path)
            .map_err(|e| anyhow::anyhow!("cannot delete {rel_path}: {e}"))?;
    }
    Ok(())
}

/// What kind of thing the info panel is describing. A no-follow stat decides
/// this, so a symlink is a `Symlink` and never silently the file it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// A fifo, socket, or device node. Rare inside a worktree, but real, and
    /// calling one a file would be wrong.
    Other,
}

/// What git has to say about the entry. An enum rather than an `Option` of
/// codes because these are genuinely different answers and collapsing any two
/// of them into `null` makes the panel lie.
///
/// Two of the variants exist because of the SAME failure: `git status` lists
/// nothing at all for an ignored path or for anything inside a nested
/// repository, so with only `Clean` to fall back on the panel called every
/// file under `node_modules`, `target` and every vendored subrepo "tracked and
/// unmodified". The editor's tree is a plain filesystem browser with no ignore
/// filter, so those paths are one right-click away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GitStatusView {
    /// There is no repository here at all.
    NotARepository,
    /// The path belongs to a DIFFERENT repository than this worktree: a nested
    /// clone or a submodule. This worktree's git has nothing to say about it.
    OtherRepository,
    /// Git tracks files, not directories.
    NotApplicable,
    /// Matched by an ignore rule, so it is untracked ON PURPOSE and appears in
    /// no status listing.
    Ignored,
    /// Tracked, with nothing pending.
    Clean,
    Changed {
        staged: Option<String>,
        unstaged: Option<String>,
    },
}

/// The read-only facts the web editor's file-info panel shows. Everything here
/// comes from one no-follow stat plus one git status lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeEntryInfo {
    /// Echoed back exactly as the caller wrote it, never re-encoded or
    /// normalized, so a non-Latin name survives the round trip.
    pub path: String,
    pub kind: EntryKind,
    /// `None` for a directory: the on-disk size of a directory entry is an
    /// implementation detail of the filesystem, not something a user wants.
    pub size: Option<u64>,
    /// RFC 3339, UTC. `None` when the filesystem does not report an mtime.
    pub modified: Option<String>,
    /// The permission bits in octal, without a leading zero (`"644"`).
    pub mode: String,
    /// The same bits as `ls -l` prints them (`"rw-r--r--"`).
    pub permissions: String,
    /// The symlink's target as stored on disk (not resolved), for a symlink.
    pub symlink_target: Option<String>,
    /// The TARGET's mtime and size, present only when this entry is a symlink
    /// whose target could be stat'd. `None` for everything else, so presence is
    /// the client's test rather than a sentinel value.
    ///
    /// The route carries two stamps because its consumers ask different
    /// questions. The info panel describes the LINK, permanently: following it
    /// would print a mode, a size and an mtime belonging to a file the user did
    /// not ask about. [`read_file`] reads THROUGH the link and stamps the
    /// descriptor it actually read from, so the editor's freshness check must
    /// compare against this target stamp or a symlinked open file reads as stale
    /// forever and its banner can never be retired.
    ///
    /// The mtime goes through the same [`format_mtime`] as every other timestamp
    /// on the wire, because two formatters would silently disagree on the same
    /// instant.
    pub target_modified: Option<String>,
    pub target_size: Option<u64>,
    pub git: GitStatusView,
}

/// Render permission bits the way `ls -l` does: three rwx triplets, with
/// setuid/setgid/sticky folded onto the matching execute position (lower-case
/// when that execute bit is also set, upper-case when it is not).
pub fn symbolic_permissions(mode: u32) -> String {
    let mut out = String::with_capacity(9);
    let triplet = |shift: u32, special: bool, special_lower: char, special_upper: char| {
        let bits = (mode >> shift) & 0o7;
        let exec = bits & 0o1 != 0;
        let last = match (special, exec) {
            (true, true) => special_lower,
            (true, false) => special_upper,
            (false, true) => 'x',
            (false, false) => '-',
        };
        [
            if bits & 0o4 != 0 { 'r' } else { '-' },
            if bits & 0o2 != 0 { 'w' } else { '-' },
            last,
        ]
    };
    out.extend(triplet(6, mode & 0o4000 != 0, 's', 'S'));
    out.extend(triplet(3, mode & 0o2000 != 0, 's', 'S'));
    out.extend(triplet(0, mode & 0o1000 != 0, 't', 'T'));
    out
}

/// Returned by [`entry_info`] when the path resolved cleanly but nothing is
/// there. A distinct type, not just a message, so the HTTP layer can answer
/// 404 for it and 400 for a REFUSED path: "it is gone" and "you may not look
/// at that" are different answers, and the browser's info panel only
/// self-dismisses on the first.
#[derive(Debug)]
pub struct EntryMissing(pub String);

impl std::fmt::Display for EntryMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no such entry in the worktree: {}", self.0)
    }
}

impl std::error::Error for EntryMissing {}

/// Describe one worktree entry for the editor's read-only info panel.
///
/// Containment is the SAME boundary every other editor operation uses:
/// `resolve_worktree_path` refuses an absolute path, any `..` or `.` segment, a
/// `.git` component anywhere, and (for a path that exists, or a dangling symlink)
/// anything whose realpath escapes the worktree or lands inside a `.git` directory.
///
/// That last check refuses a symlink escaping the tree, and the reason is the
/// TARGET PATH STRING rather than the target's contents: the stat is
/// `symlink_metadata`, so nothing is followed, a symlink is described as itself,
/// and no size, mode or mtime of the host file leaks. What would leak is
/// `symlink_target`, which the panel prints verbatim, and `/root/.ssh/id_ed25519`
/// is a disclosure on its own.
pub fn entry_info(worktree: &Path, rel_path: &str) -> anyhow::Result<WorktreeEntryInfo> {
    use std::os::unix::fs::PermissionsExt;

    let path = resolve_worktree_path(worktree, rel_path)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::Error::new(EntryMissing(rel_path.to_string()))
        } else {
            anyhow::anyhow!("cannot stat {rel_path}: {e}")
        }
    })?;
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let size = match kind {
        EntryKind::Dir => None,
        _ => Some(meta.len()),
    };
    // The shared formatter, not a second `to_rfc3339()` call site: the editor
    // compares this string against the one the read route sent, and chrono's
    // variable fractional digits would make two formatters silently disagree.
    let modified = meta.modified().ok().map(format_mtime);
    let bits = meta.permissions().mode() & 0o7777;
    let symlink_target = if ft.is_symlink() {
        std::fs::read_link(&path)
            .ok()
            .map(|t| t.to_string_lossy().into_owned())
    } else {
        None
    };
    // The FOLLOWING stat, for a symlink only, and only for the freshness
    // fields. A dangling or unreadable target simply leaves them absent; it is
    // not an error, because the panel's own answer does not depend on it. See
    // the field docs on `target_modified` for why both stamps exist.
    let (target_modified, target_size) = if ft.is_symlink() {
        match std::fs::metadata(&path) {
            Ok(target) => (target.modified().ok().map(format_mtime), Some(target.len())),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    // Git tracks files, not directories, so asking about a directory would
    // report on its children instead of on the thing the panel is describing.
    let git = if kind == EntryKind::Dir {
        GitStatusView::NotApplicable
    } else {
        entry_git_status(worktree, rel_path, &path)?
    };
    Ok(WorktreeEntryInfo {
        path: rel_path.to_string(),
        kind,
        size,
        modified,
        mode: format!("{bits:o}"),
        permissions: symbolic_permissions(bits),
        symlink_target,
        target_modified,
        target_size,
        git,
    })
}

/// What git says about ONE non-directory entry, asked in the order that keeps
/// each answer honest.
///
/// The order matters, and each step exists because the step after it cannot
/// tell the difference on its own:
///
/// 1. Who OWNS this path? A nested clone or a submodule is opaque to the
///    worktree's repository, which lists nothing for anything inside one.
/// 2. What does `git status` say? A code here is the definitive answer.
/// 3. Nothing from status means one of two things, and only one of them is
///    "clean": an ignored path is listed nowhere either.
fn entry_git_status(
    worktree: &Path,
    rel_path: &str,
    abs_path: &Path,
) -> anyhow::Result<GitStatusView> {
    let dir = abs_path.parent().unwrap_or(worktree);
    let Some(owner) = crate::git::repository_root(dir)? else {
        return Ok(GitStatusView::NotARepository);
    };
    let same_repository = match (owner.canonicalize(), worktree.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => owner == worktree,
    };
    if !same_repository {
        return Ok(GitStatusView::OtherRepository);
    }
    match crate::git::file_status(worktree, rel_path)? {
        None => Ok(GitStatusView::NotARepository),
        Some(codes) if codes.staged.is_none() && codes.unstaged.is_none() => {
            if crate::git::path_is_ignored(worktree, rel_path)? {
                Ok(GitStatusView::Ignored)
            } else {
                Ok(GitStatusView::Clean)
            }
        }
        Some(codes) => Ok(GitStatusView::Changed {
            staged: codes.staged,
            unstaged: codes.unstaged,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operations under test, cleared for any path: these tests are about
    /// what the operations themselves decide.
    fn rename_entry(worktree: &Path, from_rel: &str, to_rel: &str) -> anyhow::Result<()> {
        super::rename_entry(worktree, from_rel, to_rel, || {
            Ok(crate::destructive::Cleared::any_for_tests())
        })
    }

    fn delete_entry(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
        super::delete_entry(worktree, rel_path, || {
            Ok(crate::destructive::Cleared::any_for_tests())
        })
    }

    fn worktree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi\nthere\n").unwrap();
        dir
    }

    #[test]
    fn reads_existing_text_file() {
        let dir = worktree();
        let f = read_file(dir.path(), "hello.txt").unwrap();
        assert!(!f.binary);
        assert_eq!(f.content, "hi\nthere\n");
        assert_eq!(f.path, "hello.txt");
        assert!(!f.read_only, "normal file must not be read_only");
    }

    /// A Latin-1 `café` has no NUL and no magic number, so a sniff of the
    /// first bytes calls it text; it is not UTF-8, so serving it as text would
    /// mean serving an EMPTY buffer that a save then writes over the original.
    #[test]
    fn a_non_utf8_file_opens_as_windows_1252_and_says_it_was_a_fallback() {
        let dir = worktree();
        let latin1 = b"caf\xe9 au lait\n";
        std::fs::write(dir.path().join("latin1.txt"), latin1).unwrap();
        let f = read_file(dir.path(), "latin1.txt").unwrap();
        assert!(!f.binary);
        assert_eq!(
            f.content, "caf\u{e9} au lait\n",
            "never an empty buffer a save would write over the real bytes"
        );
        assert_eq!(f.encoding.as_deref(), Some("windows-1252"));
        assert!(f.encoding_fallback);
        assert!(!f.bom);
        assert_eq!(f.size, Some(latin1.len() as u64));
    }

    #[test]
    fn a_utf8_file_reports_its_encoding_and_no_fallback() {
        let dir = worktree();
        let f = read_file(dir.path(), "hello.txt").unwrap();
        assert_eq!(f.encoding.as_deref(), Some("UTF-8"));
        assert!(!f.encoding_fallback && !f.bom);
    }

    #[test]
    fn a_utf16_file_with_a_mark_opens_despite_its_zero_bytes() {
        let dir = worktree();
        std::fs::write(
            dir.path().join("wide.txt"),
            [0xFF, 0xFE, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0],
        )
        .unwrap();
        let f = read_file(dir.path(), "wide.txt").unwrap();
        assert!(!f.binary);
        assert_eq!(f.content, "hi\r\n");
        assert_eq!(f.encoding.as_deref(), Some("UTF-16LE"));
        assert!(f.bom && !f.encoding_fallback);
    }

    /// A file whose bytes this encoding cannot write back unchanged is text,
    /// shown read-only with the reason, never binary.
    #[test]
    fn a_file_the_encoding_cannot_write_back_opens_read_only_with_the_reason() {
        let dir = worktree();
        std::fs::write(dir.path().join("nec.txt"), [b'a', 0x87, 0x90, b'\n']).unwrap();
        let f = read_file_as(dir.path(), "nec.txt", Some(encoding_rs::SHIFT_JIS)).unwrap();
        assert!(!f.binary);
        assert!(f.read_only);
        assert_eq!(f.unwritable, Some("round_trip"));
        assert_eq!(f.content, "a\u{2252}\n");

        std::fs::write(dir.path().join("two.txt"), b"\xEF\xBB\xBF\xEF\xBB\xBFx").unwrap();
        let f = read_file(dir.path(), "two.txt").unwrap();
        assert!(f.read_only);
        assert_eq!(f.unwritable, Some("doubled_mark"));

        let f = read_file(dir.path(), "hello.txt").unwrap();
        assert!(!f.read_only);
        assert_eq!(f.unwritable, None);
    }

    #[test]
    fn mixed_line_endings_are_reported_with_what_a_save_writes() {
        let dir = worktree();
        std::fs::write(dir.path().join("mixed.txt"), "a\r\nb\r\nc\n").unwrap();
        let f = read_file(dir.path(), "mixed.txt").unwrap();
        assert!(f.mixed_line_endings);
        assert_eq!(f.eol, Some("CRLF"));
        std::fs::write(dir.path().join("mac.txt"), b"a\rb\r").unwrap();
        let f = read_file(dir.path(), "mac.txt").unwrap();
        assert!(f.mixed_line_endings);
        let f = read_file(dir.path(), "hello.txt").unwrap();
        assert!(!f.mixed_line_endings);
        assert_eq!(f.eol, Some("LF"));
    }

    /// A file the person reopened in a code page, since rewritten as UTF-8, is
    /// flagged rather than silently read as mojibake in the old choice.
    #[test]
    fn utf8_read_in_a_chosen_code_page_says_it_looks_like_utf8() {
        let dir = worktree();
        std::fs::write(dir.path().join("u.txt"), "caf\u{e9}\n").unwrap();
        let f = read_file_as(dir.path(), "u.txt", Some(encoding_rs::WINDOWS_1252)).unwrap();
        assert!(f.looks_like_utf8);
        assert!(
            !read_file_as(dir.path(), "u.txt", Some(encoding_rs::UTF_8))
                .unwrap()
                .looks_like_utf8
        );
        assert!(!read_file(dir.path(), "u.txt").unwrap().looks_like_utf8);
        let f = read_file_as(dir.path(), "hello.txt", Some(encoding_rs::WINDOWS_1252)).unwrap();
        assert!(
            !f.looks_like_utf8,
            "plain ASCII reads the same in every one"
        );
    }

    #[test]
    fn a_binary_file_reports_no_encoding() {
        let dir = worktree();
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let f = read_file(dir.path(), "blob.bin").unwrap();
        assert!(f.binary);
        assert_eq!(f.encoding, None);
    }

    #[test]
    fn a_requested_encoding_reads_exactly_that_or_is_refused() {
        let dir = worktree();
        let sjis = [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea];
        std::fs::write(dir.path().join("jp.txt"), sjis).unwrap();
        let f = read_file_as(dir.path(), "jp.txt", Some(encoding_rs::SHIFT_JIS)).unwrap();
        assert_eq!(f.content, "\u{65e5}\u{672c}\u{8a9e}");
        assert_eq!(f.encoding.as_deref(), Some("Shift_JIS"));
        assert!(!f.encoding_fallback);

        let err = read_file_as(dir.path(), "jp.txt", Some(encoding_rs::UTF_8)).unwrap_err();
        assert!(
            err.downcast_ref::<crate::text_encoding::DecodeRefused>()
                .is_some(),
            "{err}"
        );
    }

    #[test]
    fn missing_file_is_error() {
        let dir = worktree();
        assert!(read_file(dir.path(), "nope.txt").is_err());
    }

    #[test]
    fn binary_file_is_flagged_not_returned_as_text() {
        let dir = worktree();
        std::fs::write(dir.path().join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        let f = read_file(dir.path(), "blob.bin").unwrap();
        assert!(f.binary);
        assert!(f.content.is_empty());
    }

    #[test]
    fn utf8_with_nul_is_binary() {
        let dir = worktree();
        std::fs::write(dir.path().join("nul.txt"), b"valid\0utf8").unwrap();
        assert!(read_file(dir.path(), "nul.txt").unwrap().binary);
    }

    #[test]
    fn oversized_file_is_refused() {
        let dir = worktree();
        let big = vec![b'a'; (MAX_EDITABLE_BYTES + 1) as usize];
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let err = read_file(dir.path(), "big.txt").unwrap_err().to_string();
        assert!(err.contains("too large"), "unexpected error: {err}");
    }

    #[test]
    fn write_overwrites_existing_file() {
        let dir = worktree();
        write_file(dir.path(), "hello.txt", "new body\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "new body\n"
        );
    }

    #[test]
    fn write_creates_a_new_file_at_the_worktree_root() {
        let dir = worktree();
        write_file(dir.path(), "brand-new.txt", "hello\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("brand-new.txt")).unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn write_creates_a_new_file_in_an_existing_subdir() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        write_file(dir.path(), "src/new.rs", "fn main() {}\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/new.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn write_refuses_to_create_in_a_missing_directory() {
        let dir = worktree();
        // No implicit mkdir -p: the parent must already exist.
        assert!(write_file(dir.path(), "nope/new.txt", "x").is_err());
        assert!(!dir.path().join("nope").exists());
    }

    #[test]
    fn write_refuses_to_create_through_a_symlinked_parent_that_escapes() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        // A dir symlink inside the worktree pointing outside it.
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        // Creating "escape/evil.txt" would land at <outside>/evil.txt.
        assert!(write_file(dir.path(), "escape/evil.txt", "pwned").is_err());
        assert!(!outside.path().join("evil.txt").exists());
    }

    #[test]
    fn write_refuses_a_directory_path() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(write_file(dir.path(), "sub", "x").is_err());
    }

    #[test]
    fn path_traversal_is_rejected_on_read_and_write() {
        let dir = worktree();
        assert!(read_file(dir.path(), "../secret").is_err());
        assert!(write_file(dir.path(), "../secret", "x").is_err());
        assert!(read_file(dir.path(), "/etc/passwd").is_err());
    }

    // --- Symlink tests (updated for new read-permissive behavior) ---

    #[test]
    fn read_file_follows_symlink_inside_worktree_as_read_only() {
        let dir = worktree();
        // A symlink INSIDE the worktree pointing to another file inside it.
        // Even in-tree symlinks are read_only because write_file refuses ALL
        // symlinks: the Save button must be disabled rather than lie to the user.
        std::os::unix::fs::symlink(dir.path().join("hello.txt"), dir.path().join("link.txt"))
            .unwrap();
        let f = read_file(dir.path(), "link.txt").unwrap();
        assert_eq!(f.content, "hi\nthere\n");
        assert!(f.read_only, "any symlink leaf must be read_only");
    }

    #[test]
    fn read_file_follows_symlink_outside_worktree_as_read_only() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("external.txt"), "external content\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("external.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        let f = read_file(dir.path(), "link.txt").unwrap();
        assert_eq!(f.content, "external content\n");
        assert!(f.read_only, "out-of-tree symlink must be read_only");
    }

    #[test]
    fn read_file_can_open_git_config_as_read_only() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        let f = read_file(dir.path(), ".git/config").unwrap();
        assert!(
            f.content.contains("repositoryformatversion"),
            "content: {}",
            f.content
        );
        assert!(f.read_only, ".git/config must be read_only");
    }

    #[test]
    fn read_file_can_open_git_head_as_read_only() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let f = read_file(dir.path(), ".git/HEAD").unwrap();
        assert!(
            f.content.contains("refs/heads/main"),
            "content: {}",
            f.content
        );
        assert!(f.read_only, ".git/HEAD must be read_only");
    }

    #[test]
    fn read_file_git_objects_is_not_readable_via_read_file() {
        // .git/objects is excluded from the listing but the read endpoint
        // could still be called directly. It is binary content; the binary flag
        // catches it (or size-cap for pack files). This test verifies the path
        // IS reachable (so the guard is on `read_only`, not an error) but the content
        // is marked binary since loose objects are compressed binary.
        let dir = worktree();
        std::fs::create_dir_all(dir.path().join(".git/objects/ab")).unwrap();
        // Simulate a loose object file with binary content.
        std::fs::write(
            dir.path().join(".git/objects/ab/cdef"),
            [0x78_u8, 0x9c, 0x00],
        )
        .unwrap();
        let f = read_file(dir.path(), ".git/objects/ab/cdef").unwrap();
        assert!(f.read_only, ".git path must be read_only");
        assert!(f.binary, "compressed git object must be detected as binary");
    }

    #[test]
    fn write_file_still_refuses_git_path_even_after_read_loosening() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
        // The write path keeps both .git guards.
        assert!(write_file(dir.path(), ".git/config", "corrupted").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".git/config")).unwrap(),
            "[core]\n"
        );
    }

    #[test]
    fn write_file_still_refuses_out_of_tree_symlink() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        // read is now permissive but write must still be refused.
        assert!(write_file(dir.path(), "link.txt", "pwned").is_err());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "top secret\n"
        );
    }

    /// The stat in `write_file` cannot be trusted by the time the write runs:
    /// a symlink swapped in behind it would be followed by an ordinary
    /// truncating write, clobbering whatever it points at. The writer itself
    /// must refuse, so this drives it directly with the state the race would
    /// leave behind, with no stat in front of it.
    #[test]
    fn write_nofollow_refuses_a_symlink_and_leaves_its_target_untouched() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n").unwrap();
        let target = dir.path().join("hello.txt");
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let err = write_nofollow(&target, "pwned").unwrap_err().to_string();
        assert!(
            err.contains("symlink"),
            "error should name the symlink: {err}"
        );
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "top secret\n");
    }

    #[test]
    fn write_nofollow_writes_and_truncates_a_regular_file() {
        let dir = worktree();
        let target = dir.path().join("hello.txt");
        write_nofollow(&target, "short\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "short\n");
    }

    #[test]
    fn write_nofollow_creates_a_missing_file() {
        let dir = worktree();
        let target = dir.path().join("brand-new.txt");
        write_nofollow(&target, "hello\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello\n");
    }

    /// Previously named `symlink_escaping_worktree_is_rejected`. Under the new
    /// read-permissive behavior, an outside-resolving symlink is readable
    /// (`read_only: true`). The write path remains strict.
    #[test]
    fn symlink_escaping_worktree_is_returned_read_only() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        // Read is now permissive: returns content but read_only = true.
        let f = read_file(dir.path(), "link.txt").unwrap();
        assert_eq!(f.content, "top secret\n");
        assert!(f.read_only, "out-of-tree symlink must be read_only");
        // Write is still refused: outside file untouched.
        assert!(write_file(dir.path(), "link.txt", "x").is_err());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "top secret\n"
        );
    }

    /// Previously named `dangling_symlink_is_rejected_before_its_target_can_appear`.
    /// Dangling symlinks (target does not exist) cannot be canonicalized, so
    /// `read_file` returns an error.
    #[test]
    fn dangling_symlink_is_error_because_target_is_missing() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("not-yet.txt");
        std::os::unix::fs::symlink(&target, dir.path().join("dangling.txt")).unwrap();
        assert!(!target.exists());
        // Dangling symlink: canonicalize fails → read_file returns Err.
        assert!(read_file(dir.path(), "dangling.txt").is_err());
        // Write is refused by the existing symlink guard.
        assert!(write_file(dir.path(), "dangling.txt", "pwned").is_err());
        assert!(!target.exists());
    }

    /// Previously named `git_directory_is_refused`. Under the new behavior, reads
    /// of `.git/` paths are allowed but return `read_only: true`. Writes remain
    /// refused.
    #[test]
    fn git_directory_write_is_refused_read_is_allowed() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
        // Read is now allowed but read_only.
        let f = read_file(dir.path(), ".git/config").unwrap();
        assert!(f.read_only);
        assert!(f.content.contains("[core]"));
        // Write is still refused.
        assert!(write_file(dir.path(), ".git/config", "x").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".git/config")).unwrap(),
            "[core]\n"
        );
    }

    #[test]
    fn nested_git_directory_is_refused() {
        // A NESTED repo's .git (vendored dep / submodule) must be unreachable:
        // a hook written here would run as code on the next git op in that repo.
        let dir = worktree();
        std::fs::create_dir_all(dir.path().join("vendor/repo/.git/hooks")).unwrap();
        std::fs::write(dir.path().join("vendor/repo/.git/config"), "[core]\n").unwrap();
        // Read is allowed but must be read_only.
        let f = read_file(dir.path(), "vendor/repo/.git/config").unwrap();
        assert!(f.read_only, "nested .git path must be read_only");
        // Write is still refused.
        assert!(
            write_file(
                dir.path(),
                "vendor/repo/.git/hooks/pre-commit",
                "#!/bin/sh\necho pwned",
            )
            .is_err()
        );
        assert!(
            !dir.path()
                .join("vendor/repo/.git/hooks/pre-commit")
                .exists()
        );
    }

    /// Previously named `symlink_into_git_directory_is_refused`. Under the new
    /// behavior, reads through a symlink into `.git/` are allowed but
    /// `read_only: true`. Writes remain refused.
    #[test]
    fn symlink_into_git_directory_is_read_only_write_still_refused() {
        // A symlinked dir resolving into .git sidesteps the literal name check;
        // the canonical realpath check must still flag it read_only on reads and
        // refuse all writes.
        let dir = worktree();
        std::fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\nsecret=1\n").unwrap();
        std::os::unix::fs::symlink(dir.path().join(".git"), dir.path().join("gitlink")).unwrap();

        // Read is now allowed but must be read_only because the canonical target
        // is inside .git/.
        let f = read_file(dir.path(), "gitlink/config").unwrap();
        assert!(f.read_only, "symlink-into-.git must be read_only");
        assert!(f.content.contains("secret=1"), "content must be readable");

        // Write is still refused.
        assert!(write_file(dir.path(), "gitlink/hooks/post-checkout", "#!/bin/sh").is_err());
        assert!(!dir.path().join(".git/hooks/post-checkout").exists());
    }

    /// Regression test for the intermediate-symlink containment bypass.
    ///
    /// Before the fix, `read_file` only checked whether the LEAF path component
    /// was a symlink. An intermediate directory symlink (e.g. `evil/ -> /tmp/…`)
    /// was not caught: the leaf `secret.txt` is a plain file, so the code took
    /// the non-symlink branch and returned `read_only: false`, serving an
    /// out-of-tree file as if it were an editable in-tree file.
    ///
    /// After the fix, `resolve_worktree_path_for_read` canonicalizes the FULL
    /// joined path and checks containment against the canonical worktree. Any
    /// path that resolves outside the worktree via ANY symlink (intermediate or
    /// leaf) is returned with `read_only: true`.
    #[test]
    fn intermediate_dir_symlink_escaping_worktree_is_read_only() {
        let worktree_dir = worktree();
        // A separate directory OUTSIDE the worktree that we want to prevent access to.
        let outside_dir = tempfile::tempdir().unwrap();
        std::fs::write(outside_dir.path().join("secret.txt"), "classified\n").unwrap();

        // Create a directory symlink INSIDE the worktree that points OUTSIDE it.
        // `evil` is not itself a file: it is an intermediate directory component.
        std::os::unix::fs::symlink(outside_dir.path(), worktree_dir.path().join("evil")).unwrap();

        // rel_path = "evil/secret.txt"
        //   - The leaf "secret.txt" is a plain file (not a symlink).
        //   - The intermediate "evil/" is a symlink pointing outside the worktree.
        //   - Before the fix: symlink_metadata on the leaf says "regular file",
        //     so the code returned read_only: false, the bypass.
        //   - After the fix: the full canonical path resolves outside the
        //     worktree, so read_only must be true.
        let f = read_file(worktree_dir.path(), "evil/secret.txt").unwrap();
        assert_eq!(
            f.content, "classified\n",
            "content should still be readable"
        );
        assert!(
            f.read_only,
            "intermediate-dir symlink escaping the worktree must force read_only: true"
        );

        // Write must remain refused.
        assert!(
            write_file(worktree_dir.path(), "evil/secret.txt", "pwned").is_err(),
            "write through an escaping intermediate symlink must be refused"
        );
        assert_eq!(
            std::fs::read_to_string(outside_dir.path().join("secret.txt")).unwrap(),
            "classified\n",
            "outside file must be untouched"
        );
    }

    // --- create_file ---

    #[test]
    fn create_file_at_root() {
        let dir = worktree();
        create_file(dir.path(), "new.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            ""
        );
    }

    #[test]
    fn create_file_in_existing_subdir() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        create_file(dir.path(), "src/new.rs").unwrap();
        assert!(dir.path().join("src/new.rs").exists());
    }

    #[test]
    fn create_file_refuses_overwrite_existing_file() {
        let dir = worktree();
        assert!(create_file(dir.path(), "hello.txt").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hi\nthere\n",
            "existing file must be untouched"
        );
    }

    #[test]
    fn create_file_refuses_overwrite_dir() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(create_file(dir.path(), "sub").is_err());
        assert!(dir.path().join("sub").is_dir());
    }

    #[test]
    fn create_file_refuses_git_path() {
        let dir = worktree();
        assert!(create_file(dir.path(), ".git/evil").is_err());
    }

    #[test]
    fn create_file_refuses_traversal() {
        let dir = worktree();
        assert!(create_file(dir.path(), "../evil.txt").is_err());
    }

    #[test]
    fn create_file_refuses_missing_parent() {
        let dir = worktree();
        assert!(create_file(dir.path(), "nope/new.txt").is_err());
        assert!(!dir.path().join("nope").exists());
    }

    #[test]
    fn create_file_refuses_escaping_symlink_parent() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        assert!(create_file(dir.path(), "escape/evil.txt").is_err());
        assert!(!outside.path().join("evil.txt").exists());
    }

    // --- create_dir ---

    #[test]
    fn create_dir_single() {
        let dir = worktree();
        create_dir(dir.path(), "newdir").unwrap();
        assert!(dir.path().join("newdir").is_dir());
    }

    #[test]
    fn create_dir_nested_creates_intermediates() {
        let dir = worktree();
        create_dir(dir.path(), "a/b/c").unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[test]
    fn create_dir_refuses_existing() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(create_dir(dir.path(), "sub").is_err());
    }

    #[test]
    fn create_dir_refuses_git_path() {
        let dir = worktree();
        assert!(create_dir(dir.path(), ".git/evil").is_err());
    }

    #[test]
    fn create_dir_refuses_traversal() {
        let dir = worktree();
        assert!(create_dir(dir.path(), "../evil").is_err());
    }

    #[test]
    fn create_dir_refuses_escaping_ancestor() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        assert!(create_dir(dir.path(), "escape/nested/dir").is_err());
        assert!(!outside.path().join("nested").exists());
    }

    thread_local! {
        static BEFORE_CREATE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Runs, once, the hook a test armed for the moment between `create_dir`'s
    /// checks and its creation: the window a worktree removal can land in.
    pub(super) fn run_before_create_hook() {
        if let Some(hook) = BEFORE_CREATE.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }

    /// The root passed every check and was then removed (the agent's worktree
    /// went while the request was in flight). Creating `a/b` must fail rather
    /// than bring the removed worktree back as an empty directory.
    #[test]
    fn create_dir_never_recreates_a_root_removed_after_its_checks() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("wt");
        std::fs::create_dir(&root).unwrap();
        let doomed = root.clone();
        BEFORE_CREATE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || std::fs::remove_dir_all(&doomed).unwrap()));
        });
        assert!(create_dir(&root, "a/b").is_err());
        assert!(!root.exists(), "the removed worktree must stay removed");
    }

    // --- rename_entry ---

    #[test]
    fn rename_file_happy() {
        let dir = worktree();
        rename_entry(dir.path(), "hello.txt", "renamed.txt").unwrap();
        assert!(!dir.path().join("hello.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("renamed.txt")).unwrap(),
            "hi\nthere\n"
        );
    }

    #[test]
    fn rename_dir_with_contents_happy() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/inner.txt"), "inner\n").unwrap();
        rename_entry(dir.path(), "sub", "moved").unwrap();
        assert!(!dir.path().join("sub").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("moved/inner.txt")).unwrap(),
            "inner\n"
        );
    }

    #[test]
    fn rename_refuses_missing_source() {
        let dir = worktree();
        assert!(rename_entry(dir.path(), "nope.txt", "dst.txt").is_err());
    }

    #[test]
    fn rename_refuses_existing_destination() {
        let dir = worktree();
        std::fs::write(dir.path().join("dst.txt"), "already here\n").unwrap();
        assert!(rename_entry(dir.path(), "hello.txt", "dst.txt").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dst.txt")).unwrap(),
            "already here\n"
        );
    }

    #[test]
    fn rename_refuses_git_source() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
        assert!(rename_entry(dir.path(), ".git/config", "leaked.txt").is_err());
    }

    #[test]
    fn rename_refuses_git_destination() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(rename_entry(dir.path(), "hello.txt", ".git/evil").is_err());
    }

    #[test]
    fn rename_refuses_traversal_either_side() {
        let dir = worktree();
        assert!(rename_entry(dir.path(), "../evil.txt", "dst.txt").is_err());
        assert!(rename_entry(dir.path(), "hello.txt", "../evil.txt").is_err());
    }

    #[test]
    fn rename_refuses_escaping_dest_parent() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        assert!(rename_entry(dir.path(), "hello.txt", "escape/evil.txt").is_err());
        assert!(!outside.path().join("evil.txt").exists());
    }

    // --- delete_entry ---

    #[test]
    fn delete_file_happy() {
        let dir = worktree();
        delete_entry(dir.path(), "hello.txt").unwrap();
        assert!(!dir.path().join("hello.txt").exists());
    }

    #[test]
    fn delete_empty_dir_happy() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        delete_entry(dir.path(), "empty").unwrap();
        assert!(!dir.path().join("empty").exists());
    }

    #[test]
    fn delete_dir_recursive_happy() {
        let dir = worktree();
        std::fs::create_dir_all(dir.path().join("sub/nested")).unwrap();
        std::fs::write(dir.path().join("sub/nested/f.txt"), "x\n").unwrap();
        delete_entry(dir.path(), "sub").unwrap();
        assert!(!dir.path().join("sub").exists());
    }

    #[test]
    fn delete_symlink_removes_link_not_target() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("target.txt"), "keep me\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        delete_entry(dir.path(), "link.txt").unwrap();
        assert!(!dir.path().join("link.txt").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("target.txt")).unwrap(),
            "keep me\n",
            "symlink target must survive deleting the link"
        );
    }

    #[test]
    fn delete_refuses_worktree_root_dot() {
        let dir = worktree();
        assert!(delete_entry(dir.path(), ".").is_err());
        assert!(dir.path().exists());
    }

    #[test]
    fn delete_refuses_worktree_root_empty() {
        let dir = worktree();
        assert!(delete_entry(dir.path(), "").is_err());
        assert!(dir.path().exists());
    }

    /// Pins the trailing-dot hole: `delete_entry(worktree, "<symlink>/.")`
    /// where the symlink points OUTSIDE the worktree must NOT recursively delete
    /// the target directory's contents.
    ///
    /// Root cause: a trailing `.` component makes `symlink_metadata` dereference
    /// the preceding symlink (POSIX resolves `.` against the directory, so the
    /// stat reports `is_dir = true`, `is_symlink = false`), and it also makes
    /// `Path::parent()` on the joined path strip the symlink component, so the
    /// parent-containment check runs against the always-safe worktree root
    /// instead of the symlink's escaping target. `delete_entry` then took the
    /// `remove_dir_all` branch on what it believed was an in-tree directory, but
    /// was actually the external target reached through the symlink.
    #[test]
    fn delete_refuses_curdir_component() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep-me.txt"), "do not delete\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();

        assert!(
            delete_entry(dir.path(), "sub/.").is_err(),
            "a trailing `.` component after a symlink must be rejected, not resolved"
        );
        assert!(
            outside.path().join("keep-me.txt").exists(),
            "the external target's contents must survive"
        );
        assert!(
            outside.path().exists(),
            "the external target directory itself must survive"
        );
    }

    #[test]
    fn delete_refuses_git_path() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
        assert!(delete_entry(dir.path(), ".git/config").is_err());
        assert!(dir.path().join(".git/config").exists());
    }

    #[test]
    fn delete_refuses_traversal() {
        let dir = worktree();
        assert!(delete_entry(dir.path(), "../evil").is_err());
    }

    #[test]
    fn delete_escaping_symlink_removes_only_link() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("escape-link"),
        )
        .unwrap();
        delete_entry(dir.path(), "escape-link").unwrap();
        assert!(!dir.path().join("escape-link").exists());
        assert!(
            outside.path().join("secret.txt").exists(),
            "the outside target must survive"
        );
    }

    /// The read-only entry-info panel: one no-follow stat plus a git status
    /// lookup, behind the same containment boundary as every other editor
    /// operation.
    mod entry_info_tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn symbolic_permissions_renders_the_usual_triplets() {
            assert_eq!(symbolic_permissions(0o644), "rw-r--r--");
            assert_eq!(symbolic_permissions(0o755), "rwxr-xr-x");
            assert_eq!(symbolic_permissions(0o000), "---------");
            assert_eq!(symbolic_permissions(0o777), "rwxrwxrwx");
        }

        /// setuid/setgid/sticky replace the matching execute bit, upper-case
        /// when that execute bit is NOT also set. This is what `ls -l` prints,
        /// and getting it wrong makes a setuid binary look ordinary.
        #[test]
        fn symbolic_permissions_folds_in_setuid_setgid_and_sticky() {
            assert_eq!(symbolic_permissions(0o4755), "rwsr-xr-x");
            assert_eq!(symbolic_permissions(0o4644), "rwSr--r--");
            assert_eq!(symbolic_permissions(0o2755), "rwxr-sr-x");
            assert_eq!(symbolic_permissions(0o2745), "rwxr-Sr-x");
            assert_eq!(symbolic_permissions(0o1777), "rwxrwxrwt");
            assert_eq!(symbolic_permissions(0o1666), "rw-rw-rwT");
        }

        #[test]
        fn reports_size_mode_and_kind_for_a_plain_file() {
            let dir = worktree();
            std::fs::set_permissions(
                dir.path().join("hello.txt"),
                std::fs::Permissions::from_mode(0o640),
            )
            .unwrap();
            let info = entry_info(dir.path(), "hello.txt").unwrap();
            assert_eq!(info.path, "hello.txt");
            assert_eq!(info.kind, EntryKind::File);
            assert_eq!(info.size, Some(9));
            assert_eq!(info.mode, "640");
            assert_eq!(info.permissions, "rw-r-----");
            assert!(info.modified.is_some(), "a real file has an mtime");
            assert_eq!(info.symlink_target, None);
        }

        /// A name that is not ASCII must come back byte-for-byte: the info
        /// panel echoes the path the caller asked about and never re-encodes,
        /// transliterates or normalizes it.
        #[test]
        fn a_non_latin_name_survives_unchanged() {
            let dir = worktree();
            let name = "документы/日本語 файл.txt";
            std::fs::create_dir(dir.path().join("документы")).unwrap();
            std::fs::write(dir.path().join(name), "содержимое\n").unwrap();
            let info = entry_info(dir.path(), name).unwrap();
            assert_eq!(info.path, name);
            assert_eq!(info.kind, EntryKind::File);
        }

        /// A directory has no meaningful byte size and git tracks files, not
        /// directories, so both are reported as inapplicable rather than as a
        /// number and a state that would both be lies.
        #[test]
        fn a_directory_reports_no_size_and_no_git_state() {
            let dir = worktree();
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            let info = entry_info(dir.path(), "sub").unwrap();
            assert_eq!(info.kind, EntryKind::Dir);
            assert_eq!(info.size, None);
            assert_eq!(info.git, GitStatusView::NotApplicable);
        }

        /// The stat is NO-FOLLOW: a symlink is reported as a symlink with its
        /// own target, never silently as the file it points at.
        #[test]
        fn a_symlink_is_reported_as_a_symlink_not_its_target() {
            let dir = worktree();
            std::os::unix::fs::symlink("hello.txt", dir.path().join("link.txt")).unwrap();
            let info = entry_info(dir.path(), "link.txt").unwrap();
            assert_eq!(info.kind, EntryKind::Symlink);
            assert_eq!(info.symlink_target.as_deref(), Some("hello.txt"));
        }

        #[test]
        fn refuses_traversal_and_the_git_directory() {
            let dir = worktree();
            assert!(entry_info(dir.path(), "../evil").is_err());
            assert!(entry_info(dir.path(), ".git/config").is_err());
        }

        /// A symlink whose target escapes the worktree is refused outright,
        /// exactly as the write path refuses it: reporting a size and mode for
        /// a host file outside the tree would leak what is there.
        #[test]
        fn refuses_a_symlink_that_escapes_the_worktree() {
            let dir = worktree();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("secret.txt"),
                dir.path().join("escape-link"),
            )
            .unwrap();
            assert!(entry_info(dir.path(), "escape-link").is_err());
        }

        #[test]
        fn a_missing_entry_is_an_error() {
            let dir = worktree();
            assert!(entry_info(dir.path(), "nope.txt").is_err());
        }

        /// Outside a repository there is nothing for git to say, and the panel
        /// must say THAT rather than pretending the file is clean.
        #[test]
        fn outside_a_repository_the_git_state_says_so() {
            let dir = worktree();
            let info = entry_info(dir.path(), "hello.txt").unwrap();
            assert_eq!(info.git, GitStatusView::NotARepository);
        }

        /// A DANGLING symlink escaping the worktree must be refused exactly as
        /// a live one is. `exists()` follows the link, so a link whose target
        /// was removed skipped every containment check and the panel answered
        /// 200 with `symlink_target: "/root/.ssh/id_ed25519"` printed in full.
        #[test]
        fn refuses_a_dangling_symlink_that_escapes_the_worktree() {
            let dir = worktree();
            std::os::unix::fs::symlink("/root/.ssh/id_ed25519", dir.path().join("stolen")).unwrap();
            let err = entry_info(dir.path(), "stolen").unwrap_err();
            assert!(
                err.to_string().contains("escapes worktree"),
                "unexpected error: {err}"
            );
        }

        /// A dangling link that stays inside the worktree is ordinary and is
        /// still described.
        #[test]
        fn describes_a_dangling_symlink_that_stays_inside_the_worktree() {
            let dir = worktree();
            std::os::unix::fs::symlink("not-yet.txt", dir.path().join("pending")).unwrap();
            let info = entry_info(dir.path(), "pending").unwrap();
            assert_eq!(info.kind, EntryKind::Symlink);
            assert_eq!(info.symlink_target.as_deref(), Some("not-yet.txt"));
        }

        /// A repository with one committed file, for the git-state cases.
        fn repo_worktree() -> tempfile::TempDir {
            let dir = tempfile::tempdir().unwrap();
            let run = |args: &[&str], cwd: &Path| {
                let out = crate::git::test_support::git_command()
                    .args(args)
                    .current_dir(cwd)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "git {:?} failed: {}",
                    args,
                    String::from_utf8_lossy(&out.stderr)
                );
            };
            run(&["init", "-b", "main"], dir.path());
            run(&["config", "user.name", "test"], dir.path());
            run(&["config", "user.email", "t@t"], dir.path());
            std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
            run(&["add", "tracked.txt"], dir.path());
            run(&["commit", "-m", "init"], dir.path());
            dir
        }

        #[test]
        fn a_clean_tracked_file_reports_clean() {
            let dir = repo_worktree();
            let info = entry_info(dir.path(), "tracked.txt").unwrap();
            assert_eq!(info.git, GitStatusView::Clean);
        }

        /// The lie this state exists to stop: an IGNORED file is listed by no
        /// `git status`, so it used to report as tracked and unmodified. The
        /// editor's tree is a plain filesystem browser with no ignore filter,
        /// so `node_modules` is one right-click away from that answer.
        #[test]
        fn an_ignored_file_is_reported_as_ignored_not_unmodified() {
            let dir = repo_worktree();
            std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
            std::fs::create_dir(dir.path().join("node_modules")).unwrap();
            std::fs::write(dir.path().join("node_modules/a.js"), "x\n").unwrap();
            let info = entry_info(dir.path(), "node_modules/a.js").unwrap();
            assert_eq!(info.git, GitStatusView::Ignored);
        }

        /// A file inside a NESTED repository is invisible to the outer one's
        /// status for the same reason, and answering "unmodified" about a
        /// vendored subrepo is the same lie.
        #[test]
        fn a_file_in_a_nested_repository_says_it_belongs_to_another_one() {
            let dir = repo_worktree();
            let nested = dir.path().join("vendor");
            std::fs::create_dir(&nested).unwrap();
            let out = crate::git::test_support::git_command()
                .args(["init", "-b", "main"])
                .current_dir(&nested)
                .output()
                .unwrap();
            assert!(out.status.success());
            std::fs::write(nested.join("inner.txt"), "x\n").unwrap();
            let info = entry_info(dir.path(), "vendor/inner.txt").unwrap();
            assert_eq!(info.git, GitStatusView::OtherRepository);
        }

        /// A tracked file that a rule would otherwise ignore is NOT ignored,
        /// so the ignore probe must not override a real status answer either.
        #[test]
        fn a_modified_tracked_file_keeps_its_status_codes() {
            let dir = repo_worktree();
            std::fs::write(dir.path().join(".gitignore"), "tracked.txt\n").unwrap();
            std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
            let info = entry_info(dir.path(), "tracked.txt").unwrap();
            assert_eq!(
                info.git,
                GitStatusView::Changed {
                    staged: None,
                    unstaged: Some("M".to_string()),
                }
            );
        }
    }

    /// The rename/move primitive: the occupied-destination refusal, and the
    /// symlink source that delete already allows.
    mod rename_entry_tests {
        use super::*;

        /// An occupied destination is refused and nothing is lost: `rename(2)`
        /// on its own silently overwrites. This calls the primitive directly,
        /// so no caller-side stat can be what produced the refusal. It does
        /// NOT establish that the KERNEL refused rather than a stat inside the
        /// primitive; that is what
        /// `a_refused_rename_goes_through_the_no_replace_syscall` is for.
        #[test]
        fn the_rename_primitive_itself_refuses_an_occupied_destination() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("a.txt");
            let dst = dir.path().join("b.txt");
            std::fs::write(&src, "source\n").unwrap();
            std::fs::write(&dst, "do not lose me\n").unwrap();
            let err = rename_no_replace(&src, &dst).unwrap_err();
            assert!(
                matches!(err, RenameNoReplaceError::DestinationExists),
                "unexpected error: {err:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&dst).unwrap(),
                "do not lose me\n",
                "not a single byte of the destination may change"
            );
            assert!(src.exists(), "a refused rename leaves the source alone");
        }

        /// A directory onto an EMPTY directory is the other shape `rename(2)`
        /// overwrites silently.
        #[test]
        fn the_rename_primitive_refuses_a_directory_onto_an_empty_directory() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("from");
            let dst = dir.path().join("onto");
            std::fs::create_dir(&src).unwrap();
            std::fs::write(src.join("inside.txt"), "x\n").unwrap();
            std::fs::create_dir(&dst).unwrap();
            let err = rename_no_replace(&src, &dst).unwrap_err();
            assert!(
                matches!(err, RenameNoReplaceError::DestinationExists),
                "unexpected error: {err:?}"
            );
            assert!(src.join("inside.txt").exists());
        }

        #[test]
        fn the_rename_primitive_moves_an_entry_when_the_destination_is_free() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("a.txt");
            let dst = dir.path().join("sub/b.txt");
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(&src, "moved\n").unwrap();
            rename_no_replace(&src, &dst).unwrap();
            assert!(!src.exists());
            assert_eq!(std::fs::read_to_string(&dst).unwrap(), "moved\n");
        }

        /// Both assertions below exist because the RESULT of a no-replace
        /// rename is not evidence of how it was reached: reverting the
        /// `renameat_with` call to a stat followed by `std::fs::rename` leaves
        /// every other rename test in this file green, since a stat in front
        /// of `rename(2)` returns `DestinationExists` for exactly the inputs a
        /// test can set up. So these assert the BRANCH instead. Both supported
        /// targets implement the flag (Linux `RENAME_NOREPLACE`, macOS
        /// `renameatx_np`/`RENAME_EXCL`), so the fallback must not run here,
        /// and it running is the regression that matters: it is the shape the
        /// TOCTOU window comes back in.
        #[test]
        fn a_successful_rename_goes_through_the_no_replace_syscall() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("a.txt");
            let dst = dir.path().join("b.txt");
            std::fs::write(&src, "moved\n").unwrap();
            let (mechanism, result) = rename_no_replace_reporting(&src, &dst);
            result.unwrap();
            assert_eq!(
                mechanism,
                RenameMechanism::NoReplaceSyscall,
                "the rename must be the kernel's, not a stat followed by rename(2)"
            );
        }

        #[test]
        fn a_refused_rename_goes_through_the_no_replace_syscall() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("a.txt");
            let dst = dir.path().join("b.txt");
            std::fs::write(&src, "source\n").unwrap();
            std::fs::write(&dst, "do not lose me\n").unwrap();
            let (mechanism, result) = rename_no_replace_reporting(&src, &dst);
            assert!(
                matches!(result, Err(RenameNoReplaceError::DestinationExists)),
                "unexpected result: {result:?}"
            );
            assert_eq!(
                mechanism,
                RenameMechanism::NoReplaceSyscall,
                "the refusal must be the kernel's, not a stat in front of rename(2)"
            );
        }

        /// A symlink whose target escapes the worktree can be DELETED, so it
        /// can be moved too: both act on the directory entry and neither
        /// touches the target. Refusing it named a file the user never
        /// touched.
        #[test]
        fn moving_a_symlink_whose_target_escapes_the_worktree_moves_the_link() {
            let dir = worktree();
            let outside = tempfile::tempdir().unwrap();
            let secret = outside.path().join("secret.txt");
            std::fs::write(&secret, "top secret\n").unwrap();
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::os::unix::fs::symlink(&secret, dir.path().join("escape-link")).unwrap();

            rename_entry(dir.path(), "escape-link", "sub/escape-link").unwrap();

            let moved = dir.path().join("sub/escape-link");
            assert!(
                moved.symlink_metadata().unwrap().file_type().is_symlink(),
                "the LINK moves, as itself"
            );
            assert_eq!(std::fs::read_link(&moved).unwrap(), secret);
            assert!(
                dir.path().join("escape-link").symlink_metadata().is_err(),
                "the old entry is gone"
            );
            assert_eq!(
                std::fs::read_to_string(&secret).unwrap(),
                "top secret\n",
                "the target outside the worktree is untouched"
            );
        }

        /// The source's PARENT is still contained: a source reached THROUGH an
        /// escaping symlinked directory is a different thing entirely and
        /// stays refused.
        #[test]
        fn a_source_reached_through_an_escaping_directory_symlink_is_refused() {
            let dir = worktree();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("prize.txt"), "outside\n").unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
            assert!(rename_entry(dir.path(), "escape/prize.txt", "prize.txt").is_err());
            assert!(!dir.path().join("prize.txt").exists());
        }

        #[test]
        fn a_rename_source_in_the_git_directory_is_still_refused() {
            let dir = worktree();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            std::fs::write(dir.path().join(".git/config"), "x\n").unwrap();
            assert!(rename_entry(dir.path(), ".git/config", "config").is_err());
            assert!(!dir.path().join("config").exists());
        }
    }

    /// The freshness stamp and the guarded save.
    ///
    /// The bug these exist for: the editor's save was unconditional
    /// last-writer-wins, so a buffer opened before an agent edited the file
    /// silently destroyed that edit. The guard is opt-in by the PRESENCE of an
    /// expected stamp, so every other writer keeps today's behavior exactly.
    mod freshness {
        use super::*;

        /// Force `path`'s mtime to `to`, so a test can construct the one case
        /// an mtime-only token cannot see: two different contents sharing a
        /// timestamp. Real filesystems produce this on their own whenever two
        /// writes land inside one clock tick; doing it deliberately is the
        /// deterministic stand-in (a load-driven attempt to race the clock
        /// would prove nothing when it passed).
        fn set_mtime(path: &Path, to: std::time::SystemTime) {
            use rustix::fs::{AtFlags, CWD, Timestamps, utimensat};
            let dur = to.duration_since(std::time::UNIX_EPOCH).unwrap();
            let ts = rustix::fs::Timespec {
                tv_sec: dur.as_secs() as _,
                tv_nsec: dur.subsec_nanos() as _,
            };
            utimensat(
                CWD,
                path,
                &Timestamps {
                    last_access: ts,
                    last_modification: ts,
                },
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .unwrap();
        }

        #[test]
        fn read_reports_the_stamp_of_what_it_read() {
            let dir = worktree();
            let f = read_file(dir.path(), "hello.txt").unwrap();
            assert_eq!(f.size, Some("hi\nthere\n".len() as u64));
            let modified = f.modified.expect("a real filesystem reports an mtime");
            let on_disk = std::fs::metadata(dir.path().join("hello.txt")).unwrap();
            assert_eq!(modified, format_mtime(on_disk.modified().unwrap()));
        }

        /// One formatter, or the token silently never matches. chrono prints a
        /// variable number of fractional digits, so two independently written
        /// `to_rfc3339()` call sites can disagree on the SAME instant.
        #[test]
        fn read_info_and_write_all_speak_the_same_mtime_string() {
            let dir = worktree();
            let read = read_file(dir.path(), "hello.txt").unwrap();
            let info = entry_info(dir.path(), "hello.txt").unwrap();
            assert_eq!(read.modified, info.modified);
            let after = write_file_checked(dir.path(), "hello.txt", "new\n", None).unwrap();
            let info_after = entry_info(dir.path(), "hello.txt").unwrap();
            assert_eq!(after.modified, info_after.modified);
        }

        #[test]
        fn a_write_with_a_matching_stamp_succeeds_and_returns_the_fresh_one() {
            let dir = worktree();
            let before = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            let fresh = write_file_checked(dir.path(), "hello.txt", "rewritten\n", Some(&expected))
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "rewritten\n"
            );
            assert_eq!(fresh.size, "rewritten\n".len() as u64);
            // The returned stamp is the one the client must re-baseline on, so
            // an immediate second save with it must be accepted.
            write_file_checked(dir.path(), "hello.txt", "again\n", Some(&fresh)).unwrap();
        }

        #[test]
        fn a_stale_stamp_is_refused_and_the_file_is_untouched() {
            let dir = worktree();
            let before = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            // The agent edits the file underneath the open editor.
            std::fs::write(dir.path().join("hello.txt"), "the agent's work\n").unwrap();

            let err = write_file_checked(dir.path(), "hello.txt", "clobber\n", Some(&expected))
                .unwrap_err();
            let conflict = err
                .downcast_ref::<WriteConflict>()
                .expect("a stale stamp must surface as a WriteConflict, not a generic error");
            assert!(!conflict.deleted);
            let current = conflict.current.as_ref().unwrap();
            assert_eq!(current.size, "the agent's work\n".len() as u64);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "the agent's work\n",
                "a refused save must not truncate or write a single byte"
            );
        }

        /// The save route accepts bodies past the read cap, so the editor can
        /// grow a file beyond it. The stamp it got back proves it wrote those
        /// bytes, so the next save in the same tab must go through.
        #[test]
        fn a_file_the_editor_grew_past_the_read_cap_can_be_saved_again() {
            let dir = worktree();
            let read = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: read.modified.clone(),
                size: read.size.unwrap(),
            };
            let big = "a".repeat(MAX_EDITABLE_BYTES as usize + 1);
            let grown = write_file_checked(dir.path(), "hello.txt", &big, Some(&expected)).unwrap();
            assert!(grown.size > MAX_EDITABLE_BYTES);
            write_file_checked(dir.path(), "hello.txt", "smaller again\n", Some(&grown)).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "smaller again\n"
            );
        }

        /// A NUL is valid UTF-8. A file holding one reads as binary, but if the
        /// editor itself wrote it, the stamp it got back must still save.
        #[test]
        fn content_the_editor_wrote_with_a_nul_can_be_saved_again() {
            let dir = worktree();
            let read = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: read.modified.clone(),
                size: read.size.unwrap(),
            };
            let with_nul =
                write_file_checked(dir.path(), "hello.txt", "a\0b\n", Some(&expected)).unwrap();
            write_file_checked(dir.path(), "hello.txt", "clean\n", Some(&with_nul)).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "clean\n"
            );
        }

        fn stamp_of(read: &WorktreeFile) -> FileStamp {
            FileStamp {
                modified: read.modified.clone(),
                size: read.size.unwrap(),
            }
        }

        fn save_as(encoding: &'static encoding_rs::Encoding, bom: bool) -> Option<SaveEncoding> {
            Some(SaveEncoding {
                encoding,
                bom,
                read_as: None,
            })
        }

        fn save_read_as(
            encoding: &'static encoding_rs::Encoding,
            read: &'static encoding_rs::Encoding,
        ) -> Option<SaveEncoding> {
            Some(SaveEncoding {
                encoding,
                bom: false,
                read_as: Some((read, false)),
            })
        }

        /// A conversion is judged by the encoding the file was READ in: UTF-8
        /// `あ燐` has an exact `Shift_JIS` form, though its UTF-8 bytes, misread
        /// as `Shift_JIS`, hold the `87 90` that does not round-trip.
        #[test]
        fn a_conversion_is_judged_by_the_encoding_the_file_was_read_in() {
            let dir = worktree();
            std::fs::write(dir.path().join("j.txt"), "\u{3042}\u{71d0}").unwrap();
            let read = read_file(dir.path(), "j.txt").unwrap();
            write_file_encoded(
                dir.path(),
                "j.txt",
                &read.content,
                save_read_as(encoding_rs::SHIFT_JIS, encoding_rs::UTF_8),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            let written = std::fs::read(dir.path().join("j.txt")).unwrap();
            let (back, _, _) = encoding_rs::SHIFT_JIS.decode(&written);
            assert_eq!(back, "\u{3042}\u{71d0}");
        }

        #[test]
        fn a_save_in_the_encoding_it_was_read_in_is_still_held_to_writing_back() {
            let dir = worktree();
            std::fs::write(dir.path().join("nec.txt"), [0x87, 0x90]).unwrap();
            let read = read_file_as(dir.path(), "nec.txt", Some(encoding_rs::SHIFT_JIS)).unwrap();
            assert!(
                write_file_encoded(
                    dir.path(),
                    "nec.txt",
                    &read.content,
                    save_read_as(encoding_rs::SHIFT_JIS, encoding_rs::SHIFT_JIS),
                    Some(&stamp_of(&read)),
                )
                .is_err()
            );
            assert_eq!(
                std::fs::read(dir.path().join("nec.txt")).unwrap(),
                [0x87, 0x90]
            );
        }

        /// An ordinary save (the same encoding read and written) over bytes
        /// that are not valid in that encoding was never shown those bytes:
        /// refused, nothing written.
        #[test]
        fn an_ordinary_save_over_bytes_its_read_encoding_cannot_decode_is_refused() {
            let dir = worktree();
            std::fs::write(dir.path().join("l.txt"), b"caf\xe9").unwrap();
            let read = read_file(dir.path(), "l.txt").unwrap();
            let err = write_file_encoded(
                dir.path(),
                "l.txt",
                "x",
                save_read_as(encoding_rs::UTF_8, encoding_rs::UTF_8),
                Some(&stamp_of(&read)),
            )
            .unwrap_err();
            assert!(err.to_string().contains("nothing was written"), "{err}");
            assert_eq!(std::fs::read(dir.path().join("l.txt")).unwrap(), b"caf\xe9");
        }

        /// A read encoding the bytes on disk are not valid in is not what the
        /// file was read in; the save is refused.
        #[test]
        fn a_read_encoding_the_bytes_do_not_decode_in_is_refused() {
            let dir = worktree();
            std::fs::write(dir.path().join("l.txt"), b"caf\xe9").unwrap();
            let read = read_file(dir.path(), "l.txt").unwrap();
            let err = write_file_encoded(
                dir.path(),
                "l.txt",
                "x",
                save_read_as(encoding_rs::WINDOWS_1252, encoding_rs::UTF_8),
                Some(&stamp_of(&read)),
            )
            .unwrap_err();
            assert!(err.to_string().contains("nothing was written"), "{err}");
            assert_eq!(std::fs::read(dir.path().join("l.txt")).unwrap(), b"caf\xe9");
        }

        /// A GB18030 file whose text starts with U+FEFF opens read-only, so
        /// there is no character the person could delete: a refused
        /// conversion of it says the file itself starts with one and points
        /// outside dux, never "delete that character". A U+FEFF the person
        /// typed into a file that has none is still told to delete it.
        #[test]
        fn a_refused_conversion_says_whether_the_mark_character_came_from_the_file() {
            let dir = worktree();
            let on_disk = [0x84, 0x31, 0x95, 0x33, b'x'];
            std::fs::write(dir.path().join("gb.txt"), on_disk).unwrap();
            let read = read_file_as(dir.path(), "gb.txt", Some(encoding_rs::GB18030)).unwrap();
            assert_eq!(read.content, "\u{feff}x");
            for (encoding, bom) in [
                (encoding_rs::UTF_8, false),
                (encoding_rs::UTF_8, true),
                (encoding_rs::UTF_16LE, true),
            ] {
                let err = write_file_encoded(
                    dir.path(),
                    "gb.txt",
                    &read.content,
                    Some(SaveEncoding {
                        encoding,
                        bom,
                        read_as: Some((encoding_rs::GB18030, false)),
                    }),
                    Some(&stamp_of(&read)),
                )
                .unwrap_err();
                let refused = err
                    .downcast_ref::<crate::text_encoding::LeadingMarkInText>()
                    .expect("typed refusal");
                assert!(refused.from_file, "{encoding:?}");
                let message = err.to_string();
                assert!(!message.contains("Delete that character"), "{message}");
                assert!(message.contains("edit it outside dux"), "{message}");
                assert!(message.contains("nothing was written"), "{message}");
            }
            assert_eq!(std::fs::read(dir.path().join("gb.txt")).unwrap(), on_disk);

            std::fs::write(dir.path().join("m.txt"), "m\n").unwrap();
            let read = read_file(dir.path(), "m.txt").unwrap();
            let err = write_file_encoded(
                dir.path(),
                "m.txt",
                "\u{feff}m\n",
                save_read_as(encoding_rs::UTF_8, encoding_rs::UTF_8),
                Some(&stamp_of(&read)),
            )
            .unwrap_err();
            let refused = err
                .downcast_ref::<crate::text_encoding::LeadingMarkInText>()
                .expect("typed refusal");
            assert!(!refused.from_file);
            assert!(err.to_string().contains("Delete that character"), "{err}");
        }

        /// A path the editor opens can be swapped for a FIFO (or another special
        /// file) at any moment, and opening a FIFO with no writer blocks until
        /// one appears: forever, holding the file's operation in the browser.
        /// Every editor-facing open refuses anything but a regular file
        /// without waiting for it.
        #[test]
        fn a_fifo_is_refused_at_once_by_every_editor_read_and_write() {
            let dir = worktree();
            std::fs::write(dir.path().join("p.txt"), "x\n").unwrap();
            let read = read_file(dir.path(), "p.txt").unwrap();
            std::fs::remove_file(dir.path().join("p.txt")).unwrap();
            // mkfifo(1) rather than rustix's mknodat, which macOS lacks.
            let made = std::process::Command::new("mkfifo")
                .arg(dir.path().join("p.txt"))
                .status()
                .unwrap();
            assert!(made.success(), "mkfifo failed");
            let root = dir.path().to_path_buf();
            let stamp = stamp_of(&read);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let read = read_file(&root, "p.txt")
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"));
                let as_gb = read_file_as(&root, "p.txt", Some(encoding_rs::GB18030))
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"));
                let diff = crate::diff::file_diff_contents(&root, "p.txt")
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"));
                let save = write_file_encoded(
                    &root,
                    "p.txt",
                    "\u{feff}y",
                    save_read_as(encoding_rs::UTF_8, encoding_rs::GB18030),
                    Some(&stamp),
                )
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
                let raw = read_nofollow(&root.join("p.txt"))
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"));
                tx.send((read, as_gb, diff, save, raw)).unwrap();
            });
            let (read, as_gb, diff, save, raw) = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("an editor open blocked on a FIFO");
            for (what, result) in [
                ("read", read),
                ("read as", as_gb),
                ("diff", diff),
                ("raw", raw),
            ] {
                let err = result.expect_err(what);
                assert!(err.contains("not a regular file"), "{what}: {err}");
            }
            // The save is refused too (by its encoding, then its kind); it
            // only has to come back.
            assert!(save.is_err());
        }

        /// The editor's whole loop for a windows-1252 file: read, change one
        /// character, save in the encoding the read reported. Every other byte
        /// comes back exactly as it was, CRLF included.
        #[test]
        fn a_windows_1252_file_saves_back_in_windows_1252_byte_for_byte() {
            let dir = worktree();
            let latin1 = b"caf\xe9\r\nna\xefve \x80\r\n";
            std::fs::write(dir.path().join("latin1.txt"), latin1).unwrap();
            let read = read_file(dir.path(), "latin1.txt").unwrap();
            let edited = read.content.replace("caf\u{e9}", "th\u{e9}");
            let encoding =
                crate::text_encoding::encoding_for_label(read.encoding.as_deref().unwrap())
                    .unwrap();
            write_file_encoded(
                dir.path(),
                "latin1.txt",
                &edited,
                save_as(encoding, read.bom),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(dir.path().join("latin1.txt")).unwrap(),
                b"th\xe9\r\nna\xefve \x80\r\n"
            );
        }

        #[test]
        fn a_utf16_file_saves_back_with_its_mark() {
            let dir = worktree();
            let wide = [0xFE, 0xFF, 0, b'a', 0, b'\n'];
            std::fs::write(dir.path().join("wide.txt"), wide).unwrap();
            let read = read_file(dir.path(), "wide.txt").unwrap();
            write_file_encoded(
                dir.path(),
                "wide.txt",
                "ab\n",
                save_as(encoding_rs::UTF_16BE, read.bom),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(dir.path().join("wide.txt")).unwrap(),
                [0xFE, 0xFF, 0, b'a', 0, b'b', 0, b'\n']
            );
        }

        /// Converting is a save with a different encoding stated, and it is
        /// allowed over bytes that are not UTF-8 because the client says it
        /// read them as something.
        #[test]
        fn a_windows_1252_file_can_be_converted_to_utf8_on_purpose() {
            let dir = worktree();
            std::fs::write(dir.path().join("latin1.txt"), b"caf\xe9\n").unwrap();
            let read = read_file(dir.path(), "latin1.txt").unwrap();
            write_file_encoded(
                dir.path(),
                "latin1.txt",
                &read.content,
                save_as(encoding_rs::UTF_8, false),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("latin1.txt")).unwrap(),
                "caf\u{e9}\n"
            );
        }

        /// The read says unwritable; the server must not take the client's word
        /// for it. A guarded save in `Shift_JIS` over `87 90 0A` would write
        /// `81 E0 0A`, so it is refused and the bytes survive.
        #[test]
        fn a_save_in_an_encoding_that_cannot_write_the_file_back_is_refused() {
            let dir = worktree();
            let nec = [0x87, 0x90, b'\n'];
            std::fs::write(dir.path().join("nec.txt"), nec).unwrap();
            let read = read_file_as(dir.path(), "nec.txt", Some(encoding_rs::SHIFT_JIS)).unwrap();
            assert_eq!(read.unwritable, Some("round_trip"));
            let err = write_file_encoded(
                dir.path(),
                "nec.txt",
                &read.content,
                save_as(encoding_rs::SHIFT_JIS, false),
                Some(&stamp_of(&read)),
            )
            .unwrap_err();
            assert!(err.to_string().contains("nothing was written"), "{err}");
            assert_eq!(std::fs::read(dir.path().join("nec.txt")).unwrap(), nec);

            // Converting it is still allowed: its bytes are not UTF-8, so a
            // UTF-8 save is a conversion, not a rewrite in place.
            write_file_encoded(
                dir.path(),
                "nec.txt",
                &read.content,
                save_as(encoding_rs::UTF_8, false),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("nec.txt")).unwrap(),
                "\u{2252}\n"
            );
        }

        #[test]
        fn a_save_over_a_doubled_mark_in_its_own_encoding_is_refused() {
            let dir = worktree();
            let two = b"\xEF\xBB\xBF\xEF\xBB\xBFx";
            std::fs::write(dir.path().join("two.txt"), two).unwrap();
            let read = read_file(dir.path(), "two.txt").unwrap();
            assert!(
                write_file_encoded(
                    dir.path(),
                    "two.txt",
                    "x",
                    save_as(encoding_rs::UTF_8, true),
                    Some(&stamp_of(&read)),
                )
                .is_err()
            );
            assert_eq!(std::fs::read(dir.path().join("two.txt")).unwrap(), two);
        }

        /// Every ordinary save, and a conversion to windows-1252 (which reads
        /// and writes back every byte), goes through.
        #[test]
        fn the_save_limit_is_on_the_bytes_written_not_the_text_sent() {
            let dir = worktree();
            // 4 MiB of € in windows-1252 is 4 MiB on disk, though its text is
            // 12 MiB of UTF-8: it saves.
            let euros = "\u{20ac}".repeat(4 * 1024 * 1024);
            write_file_encoded(
                dir.path(),
                "euro.txt",
                &euros,
                save_as(encoding_rs::WINDOWS_1252, false),
                None,
            )
            .unwrap();
            assert_eq!(
                std::fs::metadata(dir.path().join("euro.txt"))
                    .unwrap()
                    .len(),
                4 * 1024 * 1024
            );
            // Past the limit on disk it is refused, typed, and writes nothing.
            let over = "a".repeat(MAX_SAVED_BYTES as usize + 1);
            let err = write_file_encoded(
                dir.path(),
                "big.txt",
                &over,
                save_as(encoding_rs::UTF_8, false),
                None,
            )
            .unwrap_err();
            assert!(err.downcast_ref::<SaveTooLarge>().is_some(), "{err}");
            assert!(!dir.path().join("big.txt").exists());
        }

        #[test]
        fn ordinary_saves_and_conversions_are_not_mistaken_for_rewrites() {
            let dir = worktree();
            std::fs::write(dir.path().join("l.txt"), b"caf\xe9\n").unwrap();
            let read = read_file(dir.path(), "l.txt").unwrap();
            let stamp = write_file_encoded(
                dir.path(),
                "l.txt",
                "caf\u{e9}!\n",
                save_as(encoding_rs::WINDOWS_1252, false),
                Some(&stamp_of(&read)),
            )
            .unwrap();
            write_file_encoded(
                dir.path(),
                "l.txt",
                "caf\u{e9}!\n",
                save_as(encoding_rs::WINDOWS_1252, false),
                Some(&stamp),
            )
            .unwrap();
            let read = read_file(dir.path(), "hello.txt").unwrap();
            write_file_encoded(
                dir.path(),
                "hello.txt",
                "x\n",
                save_as(encoding_rs::WINDOWS_1252, false),
                Some(&stamp_of(&read)),
            )
            .unwrap();
        }

        /// A character the encoding cannot hold stops the save before the file
        /// is touched, as a typed refusal the route can describe.
        #[test]
        fn an_unmappable_character_is_refused_and_nothing_is_written() {
            let dir = worktree();
            let latin1 = b"caf\xe9\n";
            std::fs::write(dir.path().join("latin1.txt"), latin1).unwrap();
            let read = read_file(dir.path(), "latin1.txt").unwrap();
            let err = write_file_encoded(
                dir.path(),
                "latin1.txt",
                "caf\u{e9} \u{2603}\n",
                save_as(encoding_rs::WINDOWS_1252, false),
                Some(&stamp_of(&read)),
            )
            .unwrap_err();
            let unmappable = err
                .downcast_ref::<crate::text_encoding::Unmappable>()
                .expect("typed refusal");
            assert_eq!(unmappable.character, '\u{2603}');
            assert_eq!((unmappable.line, unmappable.column), (1, 6));
            assert_eq!(
                std::fs::read(dir.path().join("latin1.txt")).unwrap(),
                latin1
            );
            // Not even a create: an unguarded save to a new path is refused too.
            assert!(
                write_file_encoded(
                    dir.path(),
                    "new.txt",
                    "\u{2603}",
                    save_as(encoding_rs::WINDOWS_1252, false),
                    None,
                )
                .is_err()
            );
            assert!(!dir.path().join("new.txt").exists());
        }

        /// A buffer that was ever opened over a non-UTF-8 file by a page that
        /// does not know about encodings (a draft kept from before the read
        /// refused such files, or any client that ignores the binary flag)
        /// holds a valid stamp for bytes it never saw. Its save states no
        /// encoding, and the guarded save refuses it, so the bytes survive.
        #[test]
        fn a_guarded_save_over_a_non_utf8_file_is_refused_and_the_bytes_survive() {
            let dir = worktree();
            let latin1 = b"caf\xe9 au lait\n";
            std::fs::write(dir.path().join("latin1.txt"), latin1).unwrap();
            let read = read_file(dir.path(), "latin1.txt").unwrap();
            let expected = FileStamp {
                modified: read.modified.clone(),
                size: read.size.unwrap(),
            };
            let err = write_file_checked(
                dir.path(),
                "latin1.txt",
                "typed into an empty buffer",
                Some(&expected),
            )
            .unwrap_err();
            assert!(
                err.downcast_ref::<WriteConflict>().is_none(),
                "this is a refusal, not a freshness conflict a reload could resolve"
            );
            assert!(err.to_string().contains("UTF-8"), "{err}");
            assert_eq!(
                std::fs::read(dir.path().join("latin1.txt")).unwrap(),
                latin1
            );
        }

        /// The whole reason the token is mtime PLUS size: two writes inside one
        /// coarse-clock tick share an mtime, and that is exactly the racing
        /// writer the guard exists for.
        #[test]
        fn a_same_mtime_change_of_size_is_still_refused() {
            let dir = worktree();
            let before = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            let stamped = std::fs::metadata(dir.path().join("hello.txt"))
                .unwrap()
                .modified()
                .unwrap();
            std::fs::write(
                dir.path().join("hello.txt"),
                "a longer line from the agent\n",
            )
            .unwrap();
            set_mtime(&dir.path().join("hello.txt"), stamped);

            let err = write_file_checked(dir.path(), "hello.txt", "clobber\n", Some(&expected))
                .unwrap_err();
            assert!(
                err.downcast_ref::<WriteConflict>().is_some(),
                "an mtime-only token would have accepted this write: {err}"
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "a longer line from the agent\n"
            );
        }

        /// Deleted underneath is its own rung: with a token, a missing file is
        /// a conflict, never a silent resurrection of a file someone removed.
        #[test]
        fn a_file_deleted_underneath_is_a_conflict_and_is_not_resurrected() {
            let dir = worktree();
            let before = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            std::fs::remove_file(dir.path().join("hello.txt")).unwrap();

            let err = write_file_checked(
                dir.path(),
                "hello.txt",
                "back from the dead\n",
                Some(&expected),
            )
            .unwrap_err();
            let conflict = err.downcast_ref::<WriteConflict>().unwrap();
            assert!(conflict.deleted);
            assert!(conflict.current.is_none());
            assert!(
                !dir.path().join("hello.txt").exists(),
                "a conflicted save must not recreate the deleted file"
            );
        }

        /// Absent token = today's behavior, exactly: overwrite whatever is
        /// there, and create what is not.
        #[test]
        fn no_stamp_keeps_the_old_unconditional_behavior() {
            let dir = worktree();
            std::fs::write(dir.path().join("hello.txt"), "changed by someone else\n").unwrap();
            write_file_checked(dir.path(), "hello.txt", "overwritten\n", None).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "overwritten\n"
            );
            let fresh = write_file_checked(dir.path(), "brand-new.txt", "hello\n", None).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("brand-new.txt")).unwrap(),
                "hello\n"
            );
            assert_eq!(fresh.size, "hello\n".len() as u64);
        }

        /// A guarded write shortens the file: the guard must ftruncate, not
        /// leave the tail of the previous, longer content behind.
        #[test]
        fn a_guarded_write_truncates_a_longer_previous_content() {
            let dir = worktree();
            std::fs::write(dir.path().join("hello.txt"), "a very long line indeed\n").unwrap();
            let before = read_file(dir.path(), "hello.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            write_file_checked(dir.path(), "hello.txt", "no\n", Some(&expected)).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
                "no\n"
            );
        }

        /// The guard must not become a way around the symlink refusal.
        #[test]
        fn a_guarded_write_still_refuses_a_symlink_leaf() {
            let dir = worktree();
            let outside = tempfile::tempdir().unwrap();
            let secret = outside.path().join("secret.txt");
            std::fs::write(&secret, "top secret\n").unwrap();
            std::os::unix::fs::symlink(&secret, dir.path().join("link.txt")).unwrap();
            let stamp = FileStamp {
                modified: None,
                size: 0,
            };
            assert!(write_file_checked(dir.path(), "link.txt", "pwned", Some(&stamp)).is_err());
            assert_eq!(std::fs::read_to_string(&secret).unwrap(), "top secret\n");
        }

        /// The asymmetry that made every symlinked open file read as stale
        /// forever: the read stamps the TARGET it actually read from (an
        /// fstat of the opened descriptor), while the info route stats the
        /// LINK, deliberately and permanently, because the info panel
        /// describes the link itself. Comparing one against the other is
        /// comparing two different files, so the info route carries BOTH.
        #[test]
        fn a_symlinks_info_carries_its_targets_stamp_beside_the_links_own() {
            let dir = worktree();
            std::os::unix::fs::symlink(dir.path().join("hello.txt"), dir.path().join("link.txt"))
                .unwrap();

            let read = read_file(dir.path(), "link.txt").unwrap();
            let info = entry_info(dir.path(), "link.txt").unwrap();

            assert_eq!(info.kind, EntryKind::Symlink);
            // The link's own stat is untouched: the panel still describes the
            // link, and its size is the length of the stored target path.
            assert_ne!(info.size, read.size);
            // ...and the target's stat is what the editor's freshness check
            // has to compare its read against.
            assert_eq!(info.target_size, read.size);
            assert_eq!(info.target_modified, read.modified);
        }

        /// Only a symlink whose target stats gets the extra fields, so the
        /// client can tell "no target" from "target unchanged" by presence.
        #[test]
        fn a_plain_file_and_a_dangling_link_carry_no_target_stamp() {
            let dir = worktree();
            std::os::unix::fs::symlink("nowhere.txt", dir.path().join("dangling.txt")).unwrap();

            let plain = entry_info(dir.path(), "hello.txt").unwrap();
            assert_eq!(plain.target_modified, None);
            assert_eq!(plain.target_size, None);

            let dangling = entry_info(dir.path(), "dangling.txt").unwrap();
            assert_eq!(dangling.kind, EntryKind::Symlink);
            assert_eq!(dangling.target_modified, None);
            assert_eq!(dangling.target_size, None);
        }

        /// Reads that hand back no text still hand back a stamp. Without one
        /// the editor has no baseline at all for a binary or read-only tab,
        /// so it never checks and never notices the file moving.
        #[test]
        fn a_binary_and_a_read_only_read_still_carry_their_stamps() {
            let dir = worktree();
            std::fs::write(dir.path().join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
            let binary = read_file(dir.path(), "blob.bin").unwrap();
            assert!(binary.binary && binary.content.is_empty());
            assert_eq!(binary.size, Some(4));
            assert!(binary.modified.is_some());

            std::os::unix::fs::symlink(dir.path().join("hello.txt"), dir.path().join("link.txt"))
                .unwrap();
            let read_only = read_file(dir.path(), "link.txt").unwrap();
            assert!(read_only.read_only);
            assert_eq!(read_only.size, Some("hi\nthere\n".len() as u64));
            assert!(read_only.modified.is_some());
        }

        /// A token in hand means "update the file I read". When that file AND
        /// the directory it lived in are both gone, the honest answer is still
        /// the deleted conflict: the create-validation refusal that used to
        /// come out first answered a question nobody asked, and reached the
        /// browser as a 400 the conflict dialog cannot route.
        #[test]
        fn a_token_survives_the_parent_directory_going_too() {
            let dir = worktree();
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub/note.txt"), "notes\n").unwrap();
            let before = read_file(dir.path(), "sub/note.txt").unwrap();
            let expected = FileStamp {
                modified: before.modified.clone(),
                size: before.size.unwrap(),
            };
            std::fs::remove_dir_all(dir.path().join("sub")).unwrap();

            let err = write_file_checked(dir.path(), "sub/note.txt", "back\n", Some(&expected))
                .unwrap_err();
            let conflict = err
                .downcast_ref::<WriteConflict>()
                .expect("a token plus a missing file is a conflict, not a create");
            assert!(conflict.deleted);
            assert!(!dir.path().join("sub").exists());
        }
    }
}
