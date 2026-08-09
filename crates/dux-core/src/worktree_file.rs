//! Safe working-copy file read/write for client-supplied worktree paths, used by
//! the web editor. The editor works against the WORKTREE itself: any file inside
//! it can be read, written, or CREATED — the only constraint is containment.
//! Reads use a read-permissive resolver that allows `.git/` paths (returning them
//! as `read_only`); writes keep the full guards and refuse `.git/` and outside-
//! resolving symlinks. There is no git-tracked/changed-file gate here — that is
//! the changes pane's concern.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::git::{is_under, resolve_worktree_path, resolves_into_git_dir};

/// Largest working copy the editor will load. Beyond this, Monaco bogs down and
/// the read would buffer the whole file into memory and a JSON response, so the
/// reader refuses instead. Source files are far smaller; this only excludes
/// generated blobs that happen to appear in `git status`.
pub const MAX_EDITABLE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeFile {
    pub path: String,
    /// True when the working copy is non-UTF-8/binary — `content` is then empty
    /// and the editor refuses to open it.
    pub binary: bool,
    pub content: String,
    /// True when the file was opened read-only — an outside-resolving symlink
    /// or a `.git/` path. The UI must grey out Save and ignore the dirty guard.
    #[serde(default)]
    pub read_only: bool,
}

/// Resolve `worktree/rel_path` for READ-only access, bypassing the literal
/// `.git`-component rejection so `.git/*` files can be opened. Returns
/// `(abs_path, is_git_dir, is_outside)`.
///
/// - `is_git_dir`: true when the canonical real path is inside a `.git`
///   directory (the caller must set `read_only = true`).
/// - `is_outside`: true when the canonical real path escapes the worktree via
///   ANY symlink — intermediate OR leaf. The caller must set `read_only = true`
///   and skip the normal O_NOFOLLOW branch, reading from the resolved target
///   instead. (Dangling symlinks whose parent cannot be canonicalized are
///   surfaced as an error, same as today.)
///
/// Traversal attacks (absolute paths, `..`) are still rejected with an error.
///
/// This is `pub` (not `pub(crate)`) because `dux-web`'s `file_routes` uses it
/// for the read-permissive resolver in `read_raw` (cross-crate call).
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
    // not-yet-existing leaf is the parent-canonicalized path — a `.git/new-file`
    // that doesn't exist would return false from the canonical check but true
    // from the literal check.
    let is_git_literal = rp
        .iter()
        .any(|c| c.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")));

    // Also check whether the canonical real path passes through a `.git`
    // directory — covers symlinks that redirect INTO a `.git/` dir even when
    // the literal path has no `.git` component.
    let is_git_canonical = real
        .strip_prefix(&real_worktree)
        .map(|rel| {
            rel.components().any(|comp| {
                comp.as_os_str()
                    .to_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(".git"))
            })
        })
        .unwrap_or(false);

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
    use rustix::fs::{Mode, OFlags, open as rustix_open};
    use std::io::Read;
    use std::os::unix::io::FromRawFd;
    use std::os::unix::io::IntoRawFd;

    // O_NOFOLLOW | O_RDONLY. rustix's OFlags::NOFOLLOW is available when the
    // `fs` feature is enabled (already is — see Cargo.toml workspace dep).
    let fd = rustix_open(abs_path, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty())
        .map_err(|e| anyhow::anyhow!("open {}: {e}", abs_path.display()))?;

    // SAFETY: we own the fd returned by rustix_open; it is valid and open.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Read a worktree file's working copy as text. A missing file is an error.
/// Binary content yields `binary: true` with empty `content`.
///
/// ANY symlink leaf is read with `read_only: true` regardless of where its
/// target lives — `write_file` refuses all symlinks so the Save button would
/// always fail for them; marking them read-only surfaces this immediately.
/// `.git/` paths are always `read_only: true`.
/// Dangling symlinks (target does not exist) return an error.
pub fn read_file(worktree: &Path, rel_path: &str) -> anyhow::Result<WorktreeFile> {
    // Use the read-permissive resolver (allows .git/ paths).
    // `is_outside` is true when the canonical real path escapes the worktree
    // via ANY symlink — intermediate or leaf — so an intermediate-dir symlink
    // (e.g. `evil/ -> /etc`) is caught here before we ever reach the leaf check.
    let (path, is_git_dir, is_outside) = resolve_worktree_path_for_read(worktree, rel_path)?;

    // No-follow stat: tells us (a) the entry kind, (b) whether it is a symlink,
    // and (c) the size. For regular files the no-follow open below is identical
    // to a regular open. For symlinks we need to additionally resolve the target
    // to check containment.
    let meta = std::fs::symlink_metadata(&path)?;

    let (bytes, read_only) = if meta.file_type().is_symlink() || is_outside {
        // Either the leaf itself is a symlink, OR an intermediate path component
        // is a symlink that escapes the worktree. In both cases, resolve to the
        // canonical real path and read from there.
        //
        // For an intermediate-symlink escape, `path` is not itself a symlink at
        // the leaf but the canonical resolution already confirmed the real path
        // is outside — we still read via the resolved path so O_NOFOLLOW on
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
        let bytes = read_nofollow(&target)?;

        // `read_only` is true when: the leaf is ANY symlink (write_file refuses
        // all symlinks, so Save would always fail — mark it read-only up front),
        // OR the real path is outside the worktree (is_outside), OR the path is
        // inside .git/.
        (
            bytes,
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
        let bytes = read_nofollow(&path)?;
        (bytes, is_git_dir)
    };

    if !crate::diff::is_renderable_text(&bytes) {
        return Ok(WorktreeFile {
            path: rel_path.to_string(),
            binary: true,
            content: String::new(),
            read_only,
        });
    }
    Ok(WorktreeFile {
        path: rel_path.to_string(),
        binary: false,
        content: String::from_utf8(bytes).unwrap_or_default(),
        read_only,
    })
}

/// Write `content` to `abs_path` with `O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW`,
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
    use rustix::fs::{Mode, OFlags, open as rustix_open};
    use std::io::Write;
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    let fd = rustix_open(
        abs_path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o666),
    )
    .map_err(|e| {
        if e == rustix::io::Errno::LOOP {
            anyhow::anyhow!(
                "refusing to write through a symlink: {}",
                abs_path.display()
            )
        } else {
            anyhow::anyhow!("open {} for writing: {e}", abs_path.display())
        }
    })?;

    // SAFETY: we own the fd returned by rustix_open; it is valid and open.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Write text to a worktree file, creating it if it does not exist (the editor
/// can save brand-new, uncommitted files). The only constraint is containment:
/// the target — and, when creating, its parent directory — must stay inside the
/// worktree. Refuses to write THROUGH a symlink (an existing one, or a dangling
/// one whose target could appear between the boundary's existence check and the
/// write) and refuses to write to a directory/fifo/device.
///
/// The stat below is what produces the readable per-case error messages; it is
/// NOT what enforces the symlink rule, because an entry can be replaced between
/// the stat and the write. The [`write_nofollow`] open is the enforcement, and
/// it refuses a symlink leaf no matter when the link appeared.
pub fn write_file(worktree: &Path, rel_path: &str, content: &str) -> anyhow::Result<()> {
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
    write_nofollow(&path, content).map_err(|e| anyhow::anyhow!("{e} (writing {rel_path})"))
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
    std::fs::create_dir_all(&path)
        .map_err(|e| anyhow::anyhow!("cannot create directory {rel_path}: {e}"))?;
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
pub fn rename_entry(worktree: &Path, from_rel: &str, to_rel: &str) -> anyhow::Result<()> {
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

/// Rename `src` onto `dst`, refusing an occupied destination ATOMICALLY.
///
/// A stat followed by [`std::fs::rename`] is not the same promise: `rename(2)`
/// silently OVERWRITES (measured: file-onto-file and directory-onto-empty-
/// directory both succeed), so another client can create the destination in
/// the window between the two calls and lose it, and dux is explicitly
/// multi-client with no trash to recover from.
/// `renameat_with(RenameFlags::NOREPLACE)` makes the refusal the kernel's, in
/// the same syscall. rustix maps that flag to `RENAME_NOREPLACE` on Linux and
/// to `renameatx_np`'s `RENAME_EXCL` on macOS, which are dux's two supported
/// targets.
///
/// The fallback is narrow and stated rather than hidden: a filesystem with no
/// `renameat2` (and macOS before 10.12, where rustix finds no `renameatx_np`
/// and answers `ENOSYS`) reports `ENOSYS`/`EINVAL`/`ENOTSUP`, and only there
/// does this stat first and rename after, which is the older racy pair. There
/// and only there does the TOCTOU window still exist.
fn rename_no_replace(src: &Path, dst: &Path) -> Result<(), RenameNoReplaceError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(err) if err == Errno::EXIST || err == Errno::NOTEMPTY => {
            Err(RenameNoReplaceError::DestinationExists)
        }
        Err(err)
            if err == Errno::NOSYS
                || err == Errno::INVAL
                || err == Errno::NOTSUP
                || err == Errno::OPNOTSUPP =>
        {
            if dst.symlink_metadata().is_ok() {
                return Err(RenameNoReplaceError::DestinationExists);
            }
            std::fs::rename(src, dst).map_err(RenameNoReplaceError::Failed)
        }
        Err(err) => Err(RenameNoReplaceError::Failed(err.into())),
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
pub fn delete_entry(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
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
/// `resolve_worktree_path` refuses an absolute path, any `..` or `.` segment,
/// a `.git` component anywhere, and (for a path that exists, or a dangling
/// symlink) anything whose realpath escapes the worktree or lands inside a
/// `.git` directory.
///
/// That last check is what refuses a symlink escaping the tree, and the reason
/// is the TARGET PATH STRING, not the target's contents: the stat below is
/// `symlink_metadata`, which reports the LINK's own lstat and never the
/// target's size, mode or mtime, so nothing about the host file leaks that
/// way. What does leak is `symlink_target`, which the panel prints verbatim,
/// and `/root/.ssh/id_ed25519` is a disclosure on its own.
///
/// The stat is `symlink_metadata`, so nothing is followed and a symlink is
/// described as itself.
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
    let modified = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
    let bits = meta.permissions().mode() & 0o7777;
    let symlink_target = if ft.is_symlink() {
        std::fs::read_link(&path)
            .ok()
            .map(|t| t.to_string_lossy().into_owned())
    } else {
        None
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
        // symlinks — the Save button must be disabled rather than lie to the user.
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
        // .git/objects is excluded from the listing (Task 1) but the read endpoint
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
        // Read is now permissive — returns content but read_only = true.
        let f = read_file(dir.path(), "link.txt").unwrap();
        assert_eq!(f.content, "top secret\n");
        assert!(f.read_only, "out-of-tree symlink must be read_only");
        // Write is still refused — outside file untouched.
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
        // A NESTED repo's .git (vendored dep / submodule) must be unreachable —
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
    /// the non-symlink branch and returned `read_only: false` — serving an
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
        // `evil` is not itself a file — it is an intermediate directory component.
        std::os::unix::fs::symlink(outside_dir.path(), worktree_dir.path().join("evil")).unwrap();

        // rel_path = "evil/secret.txt"
        //   - The leaf "secret.txt" is a plain file (not a symlink).
        //   - The intermediate "evil/" is a symlink pointing outside the worktree.
        //   - Before the fix: symlink_metadata on the leaf says "regular file",
        //     so the code returned read_only: false — the bypass.
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

        /// The refusal is the KERNEL's, in the same syscall as the rename,
        /// with no stat in front of it: `rename(2)` on its own silently
        /// overwrites, so a stat-then-rename pair is a race two clients can
        /// lose a file to. This calls the primitive directly, precisely so
        /// nothing stats first.
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
}
