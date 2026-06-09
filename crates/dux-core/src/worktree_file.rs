//! Safe working-copy file read/write for client-supplied worktree paths, used by
//! the web editor. The editor works against the WORKTREE itself: any file inside
//! it can be read, written, or CREATED — the only constraint is containment.
//! Every access goes through [`crate::git::resolve_worktree_path`] (the
//! path-escape + `.git`-rejection boundary); reads additionally reject binary and
//! oversized content, and writes refuse to follow symlinks or create through a
//! parent that resolves outside the worktree or into `.git`. There is no
//! git-tracked/changed-file gate here — that is the changes pane's concern.

use std::path::Path;

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
}

fn is_text(bytes: &[u8]) -> bool {
    // Matches the diff engine / TUI `is_renderable_text`: `content_inspector`
    // catches UTF-8 byte streams that nonetheless carry NULs/control bytes,
    // which `String::from_utf8` alone would accept and render garbled.
    bytes.is_empty() || content_inspector::inspect(bytes) == content_inspector::ContentType::UTF_8
}

/// Read a worktree file's working copy as text. A missing file is an error.
/// Binary content yields `binary: true` with empty `content`.
pub fn read_file(worktree: &Path, rel_path: &str) -> anyhow::Result<WorktreeFile> {
    let path = resolve_worktree_path(worktree, rel_path)?;
    // No-follow stat: detect a symlink even when its target is missing (a
    // dangling symlink that `exists()` reports as absent, which the boundary's
    // existence-gated escape check would skip). The editor only edits regular
    // files, so refuse symlinks outright. The same stat gives the size, so an
    // oversized file is rejected BEFORE it ever buffers into RAM.
    let meta = std::fs::symlink_metadata(&path)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("refusing to read through a symlink: {rel_path}");
    }
    if meta.len() > MAX_EDITABLE_BYTES {
        anyhow::bail!(
            "file too large to edit: {} bytes (limit {MAX_EDITABLE_BYTES})",
            meta.len()
        );
    }
    let bytes = std::fs::read(&path)?;
    if !is_text(&bytes) {
        return Ok(WorktreeFile {
            path: rel_path.to_string(),
            binary: true,
            content: String::new(),
        });
    }
    Ok(WorktreeFile {
        path: rel_path.to_string(),
        binary: false,
        content: String::from_utf8(bytes).unwrap_or_default(),
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

    #[test]
    fn symlink_escaping_worktree_is_rejected() {
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        assert!(read_file(dir.path(), "link.txt").is_err());
        assert!(write_file(dir.path(), "link.txt", "x").is_err());
        // The escape was refused, so the outside file is untouched.
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "top secret\n"
        );
    }

    #[test]
    fn dangling_symlink_is_rejected_before_its_target_can_appear() {
        // A symlink whose target does not exist at check time: `exists()` (which
        // follows) reports absent, so the boundary's escape check is skipped —
        // but the no-follow stat still sees the symlink and refuses it, closing
        // the window where the target could appear before the write follows it.
        let dir = worktree();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("not-yet.txt");
        std::os::unix::fs::symlink(&target, dir.path().join("dangling.txt")).unwrap();
        assert!(!target.exists());
        assert!(read_file(dir.path(), "dangling.txt").is_err());
        assert!(write_file(dir.path(), "dangling.txt", "pwned").is_err());
        // The write was refused, so the (now-relevant) target was never created.
        assert!(!target.exists());
    }

    #[test]
    fn git_directory_is_refused() {
        let dir = worktree();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
        assert!(read_file(dir.path(), ".git/config").is_err());
        assert!(write_file(dir.path(), ".git/config", "x").is_err());
        // Untouched.
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
        assert!(read_file(dir.path(), "vendor/repo/.git/config").is_err());
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

    #[test]
    fn symlink_into_git_directory_is_refused() {
        // A symlinked dir resolving into .git sidesteps the literal name check;
        // the canonical realpath check must still refuse it (read and create).
        let dir = worktree();
        std::fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\nsecret=1\n").unwrap();
        std::os::unix::fs::symlink(dir.path().join(".git"), dir.path().join("gitlink")).unwrap();

        assert!(read_file(dir.path(), "gitlink/config").is_err());
        assert!(write_file(dir.path(), "gitlink/hooks/post-checkout", "#!/bin/sh").is_err());
        assert!(!dir.path().join(".git/hooks/post-checkout").exists());
    }
}
