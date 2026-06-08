//! Safe working-copy file read/write for client-supplied worktree paths, used
//! by the web editor. Every access goes through [`crate::git::resolve_worktree_path`]
//! (the path-escape boundary). Reads reject binary content (the editor only
//! edits text); writes refuse to create new files (the editor edits files that
//! already exist on disk).
//!
//! This module enforces only path-safety and text-ness. Whether a path is a
//! file git is actually tracking/changing is a separate gate enforced by the
//! caller (the web layer's changed-files membership check), so `.git/` internals
//! and ignored files never reach here.

use std::path::Path;

use serde::Serialize;

use crate::git::resolve_worktree_path;

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
    // Check the size BEFORE reading so an oversized file never buffers into RAM.
    let size = std::fs::metadata(&path)?.len();
    if size > MAX_EDITABLE_BYTES {
        anyhow::bail!("file too large to edit: {size} bytes (limit {MAX_EDITABLE_BYTES})");
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

/// Overwrite an EXISTING worktree file with new text. Refuses to create new
/// files or to write through a non-regular-file path.
pub fn write_file(worktree: &Path, rel_path: &str, content: &str) -> anyhow::Result<()> {
    let path = resolve_worktree_path(worktree, rel_path)?;
    if !path.is_file() {
        anyhow::bail!("not an existing file: {rel_path}");
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
    fn write_refuses_to_create_new_file() {
        let dir = worktree();
        assert!(write_file(dir.path(), "brand-new.txt", "x").is_err());
        assert!(!dir.path().join("brand-new.txt").exists());
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
}
