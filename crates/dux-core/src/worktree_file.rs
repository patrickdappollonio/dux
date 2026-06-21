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
/// `(abs_path, is_git_dir)`. `is_git_dir` is true when the path is inside a
/// `.git` directory (the caller must set `read_only = true`). Traversal attacks
/// (absolute paths, `..`) are still rejected.
fn resolve_worktree_path_for_read(
    worktree: &Path,
    rel_path: &str,
) -> anyhow::Result<(PathBuf, bool)> {
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
    // Check whether the literal path contains a `.git` component. We check this
    // first because `resolves_into_git_dir` uses canonicalize, which requires the
    // path to exist — a `.git/new-file` that doesn't yet exist would return false
    // from the canonical check but true from the literal check.
    let is_git_literal = rp
        .iter()
        .any(|c| c.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")));
    let is_git = is_git_literal || resolves_into_git_dir(worktree, &joined);
    Ok((joined, is_git))
}

/// Read bytes from `abs_path` using `O_NOFOLLOW | O_RDONLY`, closing the
/// TOCTOU window between the caller's stat and the actual read. On Linux,
/// opening a symlink with `O_NOFOLLOW` fails with ELOOP. The caller must have
/// already confirmed (via stat) that the target is the intended file; if a race
/// causes the path to change to a symlink between stat and here, this open will
/// fail rather than silently following the new link.
fn read_nofollow(abs_path: &Path) -> anyhow::Result<Vec<u8>> {
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
/// Symlinks whose target resolves INSIDE the worktree are read normally
/// (`read_only: false`). Symlinks whose target resolves OUTSIDE the worktree
/// are read with `read_only: true`. `.git/` paths are always `read_only: true`.
/// Dangling symlinks (target does not exist) return an error.
pub fn read_file(worktree: &Path, rel_path: &str) -> anyhow::Result<WorktreeFile> {
    // Use the read-permissive resolver (allows .git/ paths).
    let (path, is_git_dir) = resolve_worktree_path_for_read(worktree, rel_path)?;

    // No-follow stat: tells us (a) the entry kind, (b) whether it is a symlink,
    // and (c) the size. For regular files the no-follow open below is identical
    // to a regular open. For symlinks we need to additionally resolve the target
    // to check containment.
    let meta = std::fs::symlink_metadata(&path)?;

    let (bytes, read_only) = if meta.file_type().is_symlink() {
        // Resolve the target to check whether it is inside the worktree.
        let target = std::fs::canonicalize(&path)?; // follows the link
        let inside = is_under(worktree, &target);

        // Stat the TARGET (not the symlink) for the size check.
        // `meta` is symlink_metadata — its .len() is the byte-length of the
        // symlink path string, not the content size. Use std::fs::metadata(&target)
        // which follows the link and gives the real file size.
        let target_meta = std::fs::metadata(&target)?;
        if target_meta.len() > MAX_EDITABLE_BYTES {
            anyhow::bail!(
                "file too large to edit: {} bytes (limit {MAX_EDITABLE_BYTES})",
                target_meta.len()
            );
        }

        // Open the TARGET file (not the symlink) with O_NOFOLLOW pointing at
        // the already-resolved target path. If the target changed to yet
        // another symlink between canonicalize() and here, O_NOFOLLOW refuses
        // it (the race window is millisecond-scale and the failure is safe).
        let bytes = read_nofollow(&target)?;
        (bytes, !inside || is_git_dir)
    } else {
        if meta.len() > MAX_EDITABLE_BYTES {
            anyhow::bail!(
                "file too large to edit: {} bytes (limit {MAX_EDITABLE_BYTES})",
                meta.len()
            );
        }
        // Regular file (or other non-symlink kind). Use O_NOFOLLOW so a
        // time-of-check / time-of-use race that replaces the file with a
        // symlink between our stat and open fails safely.
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
    fn read_file_follows_symlink_inside_worktree_as_read_write() {
        let dir = worktree();
        // A symlink INSIDE the worktree pointing to another file inside it.
        std::os::unix::fs::symlink(dir.path().join("hello.txt"), dir.path().join("link.txt"))
            .unwrap();
        let f = read_file(dir.path(), "link.txt").unwrap();
        assert_eq!(f.content, "hi\nthere\n");
        assert!(!f.read_only, "in-tree symlink must not be read_only");
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
}
