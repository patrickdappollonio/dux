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

fn is_text(bytes: &[u8]) -> bool {
    // Matches the diff engine / TUI `is_renderable_text`: `content_inspector`
    // catches UTF-8 byte streams that nonetheless carry NULs/control bytes,
    // which `String::from_utf8` alone would accept and render garbled.
    bytes.is_empty() || content_inspector::inspect(bytes) == content_inspector::ContentType::UTF_8
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

    if !is_text(&bytes) {
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

/// Write text to a worktree file, creating it if it does not exist (the editor
/// can save brand-new, uncommitted files). The only constraint is containment:
/// the target — and, when creating, its parent directory — must stay inside the
/// worktree. Refuses to write THROUGH a symlink (an existing one, or a dangling
/// one whose target could appear between the boundary's existence check and the
/// write) and refuses to write to a directory/fifo/device.
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
    std::fs::write(&path, content)?;
    Ok(())
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
pub fn rename_entry(worktree: &Path, from_rel: &str, to_rel: &str) -> anyhow::Result<()> {
    let src = resolve_worktree_path(worktree, from_rel)?;
    let dst = resolve_worktree_path(worktree, to_rel)?;
    src.symlink_metadata()
        .map_err(|e| anyhow::anyhow!("rename source does not exist: {from_rel}: {e}"))?;
    if dst.symlink_metadata().is_ok() {
        anyhow::bail!("refusing to rename, destination already exists: {to_rel}");
    }
    let dst_parent = dst.parent().unwrap_or(worktree);
    if !is_under(worktree, dst_parent) {
        anyhow::bail!(
            "cannot rename: destination's parent directory is missing or outside the worktree: {to_rel}"
        );
    }
    if resolves_into_git_dir(worktree, dst_parent) {
        anyhow::bail!("refusing to rename into the git directory: {to_rel}");
    }
    std::fs::rename(&src, &dst)
        .map_err(|e| anyhow::anyhow!("cannot rename {from_rel} to {to_rel}: {e}"))?;
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
/// target, so an escaping-target symlink is a legitimate delete target — only
/// the literal path and its PARENT's containment matter here.
pub fn delete_entry(worktree: &Path, rel_path: &str) -> anyhow::Result<()> {
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
    if rp
        .iter()
        .any(|c| c.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")))
    {
        anyhow::bail!("refusing to access the git directory: {rel_path}");
    }
    let path = worktree.join(rel_path);
    // No-follow stat on the literal path: existence and kind of the entry
    // ITSELF, never its symlink target.
    let meta = path
        .symlink_metadata()
        .map_err(|e| anyhow::anyhow!("delete target does not exist: {rel_path}: {e}"))?;
    // Containment is checked on the PARENT directory (canonicalized, so an
    // intermediate symlink that escapes the worktree is still caught), not on
    // the leaf, since the leaf may legitimately be an escaping symlink.
    let parent = path.parent().unwrap_or(worktree);
    if !is_under(worktree, parent) {
        anyhow::bail!("path escapes worktree: {rel_path}");
    }
    if resolves_into_git_dir(worktree, parent) {
        anyhow::bail!("refusing to access the git directory: {rel_path}");
    }
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
    fn delete_refuses_worktree_root() {
        let dir = worktree();
        assert!(delete_entry(dir.path(), ".").is_err() || delete_entry(dir.path(), "").is_err());
        assert!(dir.path().exists());
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
}
