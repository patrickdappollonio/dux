//! Headless, serializable diff engine shared by web clients (and available to
//! any surface). Computes a working-tree-vs-HEAD diff for a single file as a
//! plain data structure — no syntax highlighting, no terminal/ratatui types.
//! The TUI keeps its own syntect+ratatui renderer in `dux-tui/src/diff.rs`;
//! unifying the two onto this engine is a future follow-up.

use std::path::{Component, Path};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line number in the old file (None for inserts).
    pub old_line: Option<usize>,
    /// 1-based line number in the new file (None for deletes).
    pub new_line: Option<usize>,
    /// Line content WITHOUT the trailing newline or the +/-/space prefix.
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffHunk {
    /// The unified-diff hunk header, e.g. "@@ -1,3 +1,4 @@".
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDiff {
    pub path: String,
    /// True when either side is non-UTF-8/binary — `hunks` is then empty.
    pub binary: bool,
    /// True when old == new (no textual changes).
    pub unchanged: bool,
    pub old_size: usize,
    pub new_size: usize,
    pub hunks: Vec<DiffHunk>,
}

/// Compute a working-tree-vs-HEAD diff for one file. Base is the file at HEAD;
/// new is the working copy on disk. Non-UTF-8 content yields `binary: true`
/// with no hunks.
///
/// SECURITY: `rel_path` must be worktree-relative. Absolute paths and any
/// `..`/root/prefix component are rejected, since the web passes
/// client-supplied paths here.
pub fn file_diff(worktree: &Path, rel_path: &str) -> anyhow::Result<FileDiff> {
    let rp = Path::new(rel_path);
    if rp.is_absolute()
        || rp.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        anyhow::bail!("invalid diff path: {rel_path}");
    }

    let old_bytes = crate::git::file_bytes_at_head(worktree, rel_path)?.unwrap_or_default();
    let new_bytes = std::fs::read(worktree.join(rel_path)).unwrap_or_default();
    let old_size = old_bytes.len();
    let new_size = new_bytes.len();

    if old_bytes == new_bytes {
        return Ok(FileDiff {
            path: rel_path.to_string(),
            binary: false,
            unchanged: true,
            old_size,
            new_size,
            hunks: Vec::new(),
        });
    }

    let (old_text, new_text) = match (String::from_utf8(old_bytes), String::from_utf8(new_bytes)) {
        (Ok(o), Ok(n)) => (o, n),
        _ => {
            return Ok(FileDiff {
                path: rel_path.to_string(),
                binary: true,
                unchanged: false,
                old_size,
                new_size,
                hunks: Vec::new(),
            });
        }
    };

    use similar::{ChangeTag, TextDiff};
    let text_diff = TextDiff::from_lines(&old_text, &new_text);
    let mut hunks = Vec::new();
    for hunk in text_diff.unified_diff().context_radius(3).iter_hunks() {
        let header = hunk.header().to_string();
        let mut lines = Vec::new();
        for change in hunk.iter_changes() {
            let kind = match change.tag() {
                ChangeTag::Delete => DiffLineKind::Delete,
                ChangeTag::Insert => DiffLineKind::Insert,
                ChangeTag::Equal => DiffLineKind::Context,
            };
            lines.push(DiffLine {
                kind,
                old_line: change.old_index().map(|i| i + 1),
                new_line: change.new_index().map(|i| i + 1),
                content: change.value().trim_end_matches('\n').to_string(),
            });
        }
        hunks.push(DiffHunk { header, lines });
    }

    Ok(FileDiff {
        path: rel_path.to_string(),
        binary: false,
        unchanged: false,
        old_size,
        new_size,
        hunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Initialize a git repo in a tempdir with one committed file `a.txt`.
    /// Mirrors `wire.rs`'s `init_repo` helper.
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
    fn modified_file_produces_hunks() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "line1\nline2\nline3\n");
        std::fs::write(repo.path().join("f.txt"), "line1\nCHANGED\nline3\n").expect("overwrite");

        let diff = file_diff(repo.path(), "f.txt").expect("diff");
        assert!(!diff.unchanged);
        assert!(!diff.binary);
        assert!(!diff.hunks.is_empty());

        let lines: Vec<&DiffLine> = diff.hunks.iter().flat_map(|h| h.lines.iter()).collect();
        assert!(lines.iter().any(|l| {
            l.kind == DiffLineKind::Insert && l.content == "CHANGED" && l.new_line == Some(2)
        }));
        assert!(lines.iter().any(|l| {
            l.kind == DiffLineKind::Delete && l.content == "line2" && l.old_line == Some(2)
        }));
    }

    #[test]
    fn unchanged_file_reports_unchanged() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "alpha\nbeta\n");

        let diff = file_diff(repo.path(), "f.txt").expect("diff");
        assert!(diff.unchanged);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn new_untracked_file_is_all_inserts() {
        let repo = init_repo();
        std::fs::write(repo.path().join("new.txt"), "a\nb\n").expect("write new");

        let diff = file_diff(repo.path(), "new.txt").expect("diff");
        assert!(!diff.binary);
        assert!(!diff.unchanged);

        let lines: Vec<&DiffLine> = diff.hunks.iter().flat_map(|h| h.lines.iter()).collect();
        assert!(!lines.iter().any(|l| l.kind == DiffLineKind::Delete));
        assert!(lines.iter().any(|l| l.kind == DiffLineKind::Insert));
    }

    #[test]
    fn binary_file_is_flagged() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "text\n");
        std::fs::write(repo.path().join("f.txt"), [0u8, 159u8, 146u8, 150u8]).expect("overwrite");

        let diff = file_diff(repo.path(), "f.txt").expect("diff");
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn path_traversal_is_rejected() {
        let repo = init_repo();
        assert!(file_diff(repo.path(), "../escape.txt").is_err());
        assert!(file_diff(repo.path(), "/etc/passwd").is_err());
    }
}
