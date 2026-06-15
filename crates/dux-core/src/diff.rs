//! Headless, serializable diff source shared by web clients. Returns the two raw
//! sides of a single file — its content at HEAD and its working-copy content — as
//! whole UTF-8 text, leaving the actual diff rendering to the client. The web UI
//! runs Monaco's DiffEditor over these two sides; non-UTF-8/binary content is
//! reported as `binary: true` with empty sides. No syntax highlighting, no
//! terminal/ratatui types — the TUI keeps its own syntect+ratatui diff renderer
//! in `dux-tui/src/diff.rs`.

use std::path::Path;

use anyhow::Context;
use serde::Serialize;

use crate::worktree_file::MAX_EDITABLE_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffContents {
    pub path: String,
    /// File content at HEAD. Empty when the path is absent at HEAD (a newly
    /// added/untracked file) — the client then renders an all-insert diff.
    pub original: String,
    /// Working-copy content on disk. Empty when the path was deleted from the
    /// working tree — the client then renders an all-delete diff.
    pub modified: String,
    /// True when either side is non-UTF-8/binary. `original` and `modified` are
    /// then empty and the client should refuse to render a text diff.
    pub binary: bool,
}

/// Whether a byte slice is renderable UTF-8 text (empty counts as text).
/// `content_inspector` catches UTF-8 byte streams that nonetheless contain
/// NUL/control bytes, which `String::from_utf8` alone would accept and render
/// garbled — matching the TUI's `is_renderable_text`.
fn is_renderable_text(bytes: &[u8]) -> bool {
    bytes.is_empty() || content_inspector::inspect(bytes) == content_inspector::ContentType::UTF_8
}

/// Read the two sides of a single file's working-tree-vs-HEAD diff as whole text:
/// `original` is the file at HEAD, `modified` is the working copy on disk. A path
/// absent on one side yields an empty string there (added → empty original;
/// deleted → empty modified). Non-UTF-8 content on either side yields
/// `binary: true` with empty sides.
///
/// SECURITY: `rel_path` must be worktree-relative. Absolute paths, any
/// `..`/root/prefix component, and symlinks that escape the worktree are
/// rejected, since the web passes client-supplied paths here.
pub fn file_diff_contents(worktree: &Path, rel_path: &str) -> anyhow::Result<DiffContents> {
    // Reject absolute paths, `..`/root components, the `.git` dir, and symlinks
    // that escape the worktree.
    let working_path = crate::git::resolve_worktree_path(worktree, rel_path)?;

    // Cap the HEAD side by object size (`cat-file -s`, no inflate) BEFORE buffering
    // the blob, mirroring `read_file`'s working-copy cap so a huge committed file
    // can't be loaded into memory + JSON. `None` means the path is absent at HEAD
    // (new/untracked); `Some` records that HEAD has a version.
    let head_size = crate::git::blob_size_at_head(worktree, rel_path)?;
    if let Some(size) = head_size
        && size > MAX_EDITABLE_BYTES
    {
        anyhow::bail!("file too large to diff: {size} bytes at HEAD (limit {MAX_EDITABLE_BYTES})");
    }

    // Working side via a no-follow stat: refuse symlinks (consistent with
    // `read_file` — the boundary's existence-gated escape check can miss a
    // dangling or in-worktree symlink) and cap the size before buffering. A
    // missing path means no working copy (a deletion, or absent); any other
    // stat/read error propagates rather than silently rendering an empty side.
    let working_meta = match std::fs::symlink_metadata(&working_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                anyhow::bail!("refusing to diff through a symlink: {rel_path}");
            }
            if meta.len() > MAX_EDITABLE_BYTES {
                anyhow::bail!(
                    "file too large to diff: {} bytes (limit {MAX_EDITABLE_BYTES})",
                    meta.len()
                );
            }
            Some(meta)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e).with_context(|| format!("could not stat working copy of {rel_path}"));
        }
    };

    // A path present neither at HEAD nor on disk is not a real file (a stale or
    // mistyped path). Error rather than render a confusing all-blank diff.
    if head_size.is_none() && working_meta.is_none() {
        anyhow::bail!("file not found in the worktree or at HEAD: {rel_path}");
    }

    let new_bytes = if working_meta.is_some() {
        std::fs::read(&working_path)
            .with_context(|| format!("could not read working copy of {rel_path}"))?
    } else {
        Vec::new()
    };
    let old_bytes = crate::git::file_bytes_at_head(worktree, rel_path)?.unwrap_or_default();

    if !is_renderable_text(&old_bytes) || !is_renderable_text(&new_bytes) {
        return Ok(DiffContents {
            path: rel_path.to_string(),
            original: String::new(),
            modified: String::new(),
            binary: true,
        });
    }

    Ok(DiffContents {
        path: rel_path.to_string(),
        original: String::from_utf8(old_bytes).unwrap_or_default(),
        modified: String::from_utf8(new_bytes).unwrap_or_default(),
        binary: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Initialize a git repo in a tempdir with one committed file `a.txt`.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "hello\n").expect("write file");
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    /// Commit `content` to `rel` in the repo, replacing any prior version.
    fn commit_file(dir: &Path, rel: &str, content: &str) {
        commit_file_bytes(dir, rel, content.as_bytes());
    }

    /// Like [`commit_file`] but commits raw bytes (for binary-at-HEAD cases).
    fn commit_file_bytes(dir: &Path, rel: &str, content: &[u8]) {
        std::fs::write(dir.join(rel), content).expect("write file");
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["add", rel]);
        run(&["commit", "-q", "-m", "update"]);
    }

    #[test]
    fn modified_file_returns_both_sides() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "line1\nline2\nline3\n");
        std::fs::write(repo.path().join("f.txt"), "line1\nCHANGED\nline3\n").expect("overwrite");

        let c = file_diff_contents(repo.path(), "f.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "line1\nline2\nline3\n");
        assert_eq!(c.modified, "line1\nCHANGED\nline3\n");
    }

    #[test]
    fn unchanged_file_returns_equal_sides() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "alpha\nbeta\n");

        let c = file_diff_contents(repo.path(), "f.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, c.modified);
        assert_eq!(c.original, "alpha\nbeta\n");
    }

    #[test]
    fn new_untracked_file_has_empty_original() {
        let repo = init_repo();
        std::fs::write(repo.path().join("new.txt"), "a\nb\n").expect("write new");

        let c = file_diff_contents(repo.path(), "new.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "");
        assert_eq!(c.modified, "a\nb\n");
    }

    #[test]
    fn deleted_file_has_empty_modified() {
        let repo = init_repo();
        commit_file(repo.path(), "gone.txt", "to be removed\n");
        std::fs::remove_file(repo.path().join("gone.txt")).expect("remove");

        let c = file_diff_contents(repo.path(), "gone.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "to be removed\n");
        assert_eq!(c.modified, "");
    }

    #[test]
    fn binary_file_is_flagged_with_empty_sides() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "text\n");
        std::fs::write(repo.path().join("f.txt"), [0u8, 159u8, 146u8, 150u8]).expect("overwrite");

        let c = file_diff_contents(repo.path(), "f.txt").expect("contents");
        assert!(c.binary);
        assert_eq!(c.original, "");
        assert_eq!(c.modified, "");
    }

    /// A UTF-8 byte stream that nonetheless contains a NUL must be treated as
    /// binary (matching the TUI's content_inspector check), not rendered as text.
    #[test]
    fn utf8_with_nul_is_binary() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "text\n");
        std::fs::write(repo.path().join("f.txt"), b"valid\0utf8\n").expect("overwrite");

        let c = file_diff_contents(repo.path(), "f.txt").expect("contents");
        assert!(c.binary, "UTF-8-with-NUL should be flagged binary");
    }

    #[test]
    fn path_traversal_is_rejected() {
        let repo = init_repo();
        assert!(file_diff_contents(repo.path(), "../escape.txt").is_err());
        assert!(file_diff_contents(repo.path(), "/etc/passwd").is_err());
        // Interior `..` is rejected too (components are not normalized away).
        assert!(file_diff_contents(repo.path(), "a/../../b").is_err());
    }

    /// A symlink inside the worktree that points OUTSIDE it must be refused — the
    /// component check alone wouldn't catch this, and the web reads client-
    /// supplied paths.
    #[test]
    fn symlink_escaping_worktree_is_rejected() {
        let repo = init_repo();
        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), "top secret\n").expect("write secret");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            repo.path().join("link.txt"),
        )
        .expect("symlink");

        assert!(
            file_diff_contents(repo.path(), "link.txt").is_err(),
            "a symlink resolving outside the worktree must be rejected"
        );
    }

    /// An in-worktree symlink (target inside the tree, so the escape check passes)
    /// must still be refused by the no-follow stat — matching `read_file`, which
    /// refuses all symlinks. Closes the read-path inconsistency.
    #[test]
    fn in_worktree_symlink_is_refused() {
        let repo = init_repo();
        std::fs::write(repo.path().join("real.txt"), "real\n").expect("write real");
        std::os::unix::fs::symlink(repo.path().join("real.txt"), repo.path().join("link.txt"))
            .expect("symlink");
        let err = file_diff_contents(repo.path(), "link.txt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("symlink"), "unexpected error: {err}");
    }

    /// A working copy larger than the cap is refused before it is buffered.
    #[test]
    fn oversized_working_file_is_refused() {
        let repo = init_repo();
        let big = vec![b'a'; (MAX_EDITABLE_BYTES + 1) as usize];
        std::fs::write(repo.path().join("big.txt"), &big).expect("write big");
        let err = file_diff_contents(repo.path(), "big.txt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("too large"), "unexpected error: {err}");
    }

    /// Binary content at HEAD (not just in the working copy) flags the diff binary.
    #[test]
    fn binary_file_at_head_is_flagged() {
        let repo = init_repo();
        // Commit raw binary bytes, then replace with text on disk.
        commit_file_bytes(repo.path(), "f.bin", &[0u8, 159u8, 146u8, 150u8]);
        std::fs::write(repo.path().join("f.bin"), "now text\n").expect("overwrite");

        let c = file_diff_contents(repo.path(), "f.bin").expect("contents");
        assert!(c.binary, "binary-at-HEAD should flag the diff binary");
        assert_eq!(c.original, "");
        assert_eq!(c.modified, "");
    }

    /// A path absent both at HEAD and on disk is a stale/typo path — it errors
    /// rather than returning a confusing all-blank diff.
    #[test]
    fn absent_on_both_sides_errors() {
        let repo = init_repo();
        let err = file_diff_contents(repo.path(), "never-existed.txt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }
}
