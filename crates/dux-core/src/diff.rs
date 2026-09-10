//! Headless, serializable diff source shared by web clients. Returns the two raw
//! sides of a single file (its content at HEAD and its working-copy content) as
//! whole UTF-8 text, leaving the actual diff rendering to the client. The web UI
//! runs Monaco's DiffEditor over these two sides; non-UTF-8/binary content is
//! reported as `binary: true` with empty sides. No syntax highlighting, no
//! terminal/ratatui types: the TUI keeps its own syntect+ratatui diff renderer
//! in `dux-tui/src/diff.rs`.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Context;
use serde::Serialize;

use crate::text::count_of;
use crate::worktree_file::MAX_EDITABLE_BYTES;

/// How many lines of a past-the-ceiling diff both surfaces show. Enough to read
/// the shape of the change; small enough that the browser and the terminal pane
/// stay responsive holding it.
pub const DIFF_HEAD_MAX_LINES: usize = 4_000;

/// How far the line count is allowed to run before it stops counting. A diff
/// this long is already unreadable, and walking a multi-gigabyte patch to reach
/// an exact figure buys nothing; past it the total is reported as a floor.
pub const DIFF_HEAD_MAX_COUNTED_LINES: usize = 2_000_000;

/// How many bytes of each side are sampled to decide text versus binary.
const BINARY_SAMPLE_BYTES: usize = 8 * 1024;

/// The first [`DIFF_HEAD_MAX_LINES`] lines of git's own unified patch for one
/// file, with an honest account of how much was left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffHead {
    /// The kept lines, newline-joined. Always ends at a line boundary.
    pub text: String,
    pub shown_lines: usize,
    /// The patch's whole line count, or the point counting stopped when
    /// `total_is_at_least` is set.
    pub total_lines: usize,
    /// True when lines were dropped: `shown_lines < total_lines`.
    pub truncated: bool,
    /// True when counting stopped at [`DIFF_HEAD_MAX_COUNTED_LINES`], so
    /// `total_lines` is a floor rather than the figure.
    pub total_is_at_least: bool,
    /// True when either side is non-UTF-8/binary. `text` is then empty and the
    /// caller should say so rather than render a patch.
    pub binary: bool,
}

/// Read the head of git's own diff for one file, for the case where dux's
/// in-process diff refuses the pair as too large.
///
/// The deliberate exception to computing diffs in process: a file past the
/// ceiling would hold a dux worker for minutes, while git answers in well under
/// a second and this only ever reads the first few thousand lines of what it
/// says.
pub fn diff_head_via_git(worktree: &Path, rel_path: &str) -> anyhow::Result<DiffHead> {
    let working_path = crate::git::resolve_worktree_path(worktree, rel_path)?;

    let head_prefix = crate::git::file_prefix_at_head(worktree, rel_path, BINARY_SAMPLE_BYTES)?;
    let working_prefix = read_prefix(&working_path, BINARY_SAMPLE_BYTES)?;
    if head_prefix.is_none() && working_prefix.is_none() {
        anyhow::bail!("file not found in the worktree or at HEAD: {rel_path}");
    }
    let sample_is_text =
        |sample: &Option<Vec<u8>>| sample.as_deref().is_none_or(is_renderable_text);
    if !sample_is_text(&head_prefix) || !sample_is_text(&working_prefix) {
        return Ok(DiffHead {
            text: String::new(),
            shown_lines: 0,
            total_lines: 0,
            truncated: false,
            total_is_at_least: false,
            binary: true,
        });
    }

    let mut command = Command::new("git");
    command.args(["-C", worktree.to_string_lossy().as_ref()]);
    // Pin the settings that change the patch's shape, then bound the pathspec
    // with `--` so a dash-leading name cannot be read as an option.
    command.args([
        "-c",
        "core.quotePath=false",
        "-c",
        "diff.noprefix=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
    ]);
    if head_prefix.is_some() {
        command.args(["HEAD", "--", rel_path]);
    } else {
        // Absent at HEAD: git's index-based diff has nothing to compare, so the
        // untracked file is diffed against the empty file directly.
        command.args(["--no-index", "--", "/dev/null", rel_path]);
    }

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run git diff for {rel_path}"))?;
    let stdout = child
        .stdout
        .take()
        .context("git diff produced no output stream")?;

    let mut text = String::new();
    let mut shown_lines = 0usize;
    let mut total_lines = 0usize;
    let mut total_is_at_least = false;
    for line in BufReader::new(stdout).split(b'\n') {
        let line = line.with_context(|| format!("could not read git diff for {rel_path}"))?;
        total_lines += 1;
        if shown_lines < DIFF_HEAD_MAX_LINES {
            text.push_str(&String::from_utf8_lossy(&line));
            text.push('\n');
            shown_lines += 1;
        }
        if total_lines >= DIFF_HEAD_MAX_COUNTED_LINES {
            total_is_at_least = true;
            break;
        }
    }

    if total_is_at_least {
        let _ = child.kill();
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child.wait()?;
    // `--no-index` reports "the two files differ" as exit code 1, which is the
    // normal answer here, not a failure. Anything else is one.
    let acceptable = matches!(status.code(), Some(0) | Some(1));
    if !acceptable && !total_is_at_least {
        anyhow::bail!("git diff failed for {rel_path}: {}", stderr.trim());
    }

    Ok(DiffHead {
        truncated: shown_lines < total_lines,
        text,
        shown_lines,
        total_lines,
        total_is_at_least,
        binary: false,
    })
}

/// Read at most `limit` bytes of a working copy, or `None` when it is absent.
/// A symlink is refused, matching [`file_diff_contents`].
fn read_prefix(path: &Path, limit: usize) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("refusing to diff through a symlink: {}", path.display())
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("could not stat {}", path.display())),
    }
    let file =
        std::fs::File::open(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut buffer = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut buffer)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(Some(buffer))
}

/// The one sentence both surfaces put above a cut-short diff. Empty when
/// nothing was cut.
pub fn diff_head_banner(head: &DiffHead) -> String {
    if !head.truncated {
        return String::new();
    }
    let total = if head.total_is_at_least {
        format!("more than {}", count_of(head.total_lines, "line"))
    } else {
        count_of(head.total_lines, "line")
    };
    format!(
        "Diff cut here: showing the first {} of {total}. Open the file in your editor or run \
         git diff to see the rest.",
        head.shown_lines
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffContents {
    pub path: String,
    /// File content at HEAD. Empty when the path is absent at HEAD (a newly
    /// added/untracked file); the client then renders an all-insert diff.
    pub original: String,
    /// Working-copy content on disk. Empty when the path was deleted from the
    /// working tree; the client then renders an all-delete diff.
    pub modified: String,
    /// True when either side is non-UTF-8/binary. `original` and `modified` are
    /// then empty and the client should refuse to render a text diff.
    pub binary: bool,
}

/// The one refusal a caller answers with the diff head rather than an error.
///
/// Carried as a type rather than recognised by its wording: every other reason
/// [`file_diff_contents`] can refuse (containment, a symlink, a path that is
/// not there) is still an error, and telling them apart by string match is how
/// a reworded message quietly changes an HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTooLarge {
    pub message: String,
}

impl std::fmt::Display for DiffTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DiffTooLarge {}

/// Whether a byte slice is renderable UTF-8 text (empty counts as text).
/// `content_inspector` catches UTF-8 byte streams that nonetheless contain
/// NUL/control bytes, which `String::from_utf8` alone would accept and render
/// garbled.
///
/// The SINGLE binary-vs-text predicate for the whole app, so a file is never
/// classified as binary on one surface and text on another.
pub fn is_renderable_text(bytes: &[u8]) -> bool {
    bytes.is_empty() || content_inspector::inspect(bytes) == content_inspector::ContentType::UTF_8
}

/// Read the two sides of a single file's working-tree-vs-HEAD diff as whole
/// text: `original` is the file at HEAD, `modified` the working copy on disk. A
/// path absent on one side yields an empty string there. Non-UTF-8 content on
/// either side yields `binary: true` with empty sides.
///
/// SECURITY: `rel_path` must be worktree-relative. Absolute paths, any `..`,
/// root or prefix component, and symlinks that escape the worktree are rejected,
/// since the web passes client-supplied paths here.
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
        return Err(anyhow::Error::new(DiffTooLarge {
            message: format!(
                "file too large to diff: {size} bytes at HEAD (limit {MAX_EDITABLE_BYTES})"
            ),
        }));
    }

    // Working side via a no-follow stat: refuse symlinks (consistent with
    // `read_file`: the boundary's existence-gated escape check can miss a
    // dangling or in-worktree symlink) and cap the size before buffering. A
    // missing path means no working copy (a deletion, or absent); any other
    // stat/read error propagates rather than silently rendering an empty side.
    let working_meta = match std::fs::symlink_metadata(&working_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                anyhow::bail!("refusing to diff through a symlink: {rel_path}");
            }
            if meta.len() > MAX_EDITABLE_BYTES {
                return Err(anyhow::Error::new(DiffTooLarge {
                    message: format!(
                        "file too large to diff: {} bytes (limit {MAX_EDITABLE_BYTES})",
                        meta.len()
                    ),
                }));
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

    #[test]
    fn is_renderable_text_classifies_empty_utf8_and_binary() {
        // Empty is text (an added/deleted side renders as all-insert/all-delete).
        assert!(is_renderable_text(b""));
        // Plain ASCII / UTF-8 is text, including multi-byte scalars.
        assert!(is_renderable_text(b"fn main() {}\n"));
        assert!(is_renderable_text(
            "caf\u{e9} \u{2014} \u{1f600}".as_bytes()
        ));
        // A NUL byte marks the stream binary even though the rest is ASCII: this
        // is exactly what `String::from_utf8` would wrongly accept and render
        // garbled, and why the predicate uses content_inspector.
        assert!(!is_renderable_text(b"text\0more"));
        // A classic binary signature (PNG header) is not renderable text.
        assert!(!is_renderable_text(&[
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a
        ]));
    }

    /// Initialize a git repo in a tempdir with one committed file `a.txt`.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = crate::git::test_support::git_command()
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
            let ok = crate::git::test_support::git_command()
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
    fn deleted_file_whose_directory_was_pruned_has_empty_modified() {
        // The realistic `git rm sub/only-file` shape: git prunes the now-empty
        // parent directory too, so the working side's stat fails with a
        // missing PARENT, not just a missing file. That must still read as a
        // deletion (HEAD content vs empty), never an error: the web Changes
        // pane renders a deleted file's diff from exactly this state.
        let repo = init_repo();
        std::fs::create_dir(repo.path().join("sub")).expect("mkdir");
        commit_file(repo.path(), "sub/only.txt", "the only file\n");
        std::fs::remove_file(repo.path().join("sub/only.txt")).expect("remove file");
        std::fs::remove_dir(repo.path().join("sub")).expect("remove dir");

        let c = file_diff_contents(repo.path(), "sub/only.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "the only file\n");
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

    /// A symlink inside the worktree that points OUTSIDE it must be refused: the
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
    /// must still be refused by the no-follow stat, matching `read_file`, which
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
        let err = file_diff_contents(repo.path(), "big.txt").unwrap_err();
        assert!(
            err.downcast_ref::<DiffTooLarge>().is_some(),
            "the size refusal is typed so a caller can answer it with the head: {err}"
        );
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
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

    /// A path absent both at HEAD and on disk is a stale/typo path: it errors
    /// rather than returning a confusing all-blank diff.
    /// Commit a file whose diff is far longer than the head cap, so the
    /// fallback has something real to cut.
    fn commit_and_rewrite_long_file(repo: &Path, rel: &str, lines: usize) {
        let original: String = (0..lines).map(|i| format!("old line {i}\n")).collect();
        commit_file(repo, rel, &original);
        let rewritten: String = (0..lines).map(|i| format!("new line {i}\n")).collect();
        std::fs::write(repo.join(rel), rewritten).expect("rewrite");
    }

    #[test]
    fn a_long_diff_is_cut_at_the_head_cap_on_a_line_boundary() {
        let repo = init_repo();
        commit_and_rewrite_long_file(repo.path(), "long.txt", 5_000);

        let head = diff_head_via_git(repo.path(), "long.txt").expect("head");
        assert!(!head.binary);
        assert!(head.truncated, "a 10,000-line patch must be cut");
        assert!(
            !head.total_is_at_least,
            "the count fits well under the bound"
        );
        assert_eq!(head.shown_lines, DIFF_HEAD_MAX_LINES);
        assert!(head.total_lines > DIFF_HEAD_MAX_LINES);
        assert_eq!(
            head.text.lines().count(),
            DIFF_HEAD_MAX_LINES,
            "the kept text holds exactly the shown lines"
        );
        assert!(
            head.text.ends_with('\n'),
            "the cut lands on a line boundary, never mid-line"
        );
        assert!(
            head.text.starts_with("diff --git "),
            "the head starts at the patch's own first line: {:?}",
            head.text.chars().take(40).collect::<String>()
        );
    }

    #[test]
    fn a_short_diff_is_not_truncated_and_gets_no_banner() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "one\ntwo\n");
        std::fs::write(repo.path().join("f.txt"), "one\nTWO\n").expect("overwrite");

        let head = diff_head_via_git(repo.path(), "f.txt").expect("head");
        assert!(!head.truncated);
        assert_eq!(head.shown_lines, head.total_lines);
        assert!(head.text.contains("+TWO"));
        assert_eq!(diff_head_banner(&head), "");
    }

    #[test]
    fn an_untracked_file_diffs_against_the_empty_file() {
        let repo = init_repo();
        std::fs::write(repo.path().join("brand-new.txt"), "alpha\nbeta\n").expect("write");

        let head = diff_head_via_git(repo.path(), "brand-new.txt").expect("head");
        assert!(!head.binary);
        assert!(!head.truncated);
        assert!(
            head.text.contains("+alpha") && head.text.contains("+beta"),
            "an untracked file reads as all-insert: {}",
            head.text
        );
    }

    #[test]
    fn a_file_deleted_from_the_worktree_diffs_as_all_delete() {
        let repo = init_repo();
        commit_file(repo.path(), "gone.txt", "first\nsecond\n");
        std::fs::remove_file(repo.path().join("gone.txt")).expect("remove");

        let head = diff_head_via_git(repo.path(), "gone.txt").expect("head");
        assert!(!head.binary);
        assert!(
            head.text.contains("-first") && head.text.contains("-second"),
            "a deleted file reads as all-delete: {}",
            head.text
        );
    }

    #[test]
    fn a_binary_file_returns_the_binary_verdict_rather_than_a_patch() {
        let repo = init_repo();
        commit_file(repo.path(), "f.bin", "text\n");
        std::fs::write(repo.path().join("f.bin"), [0u8, 159, 146, 150]).expect("overwrite");

        let head = diff_head_via_git(repo.path(), "f.bin").expect("head");
        assert!(
            head.binary,
            "binary content must not be rendered as a patch"
        );
        assert_eq!(head.text, "");
        assert_eq!(head.total_lines, 0);
    }

    #[test]
    fn a_path_with_a_space_and_a_non_ascii_name_is_not_quoted_away() {
        let repo = init_repo();
        let rel = "sp ace/caf\u{e9}-\u{1f600}.txt";
        std::fs::create_dir(repo.path().join("sp ace")).expect("mkdir");
        commit_file(repo.path(), rel, "before\n");
        std::fs::write(repo.path().join(rel), "after\n").expect("overwrite");

        let head = diff_head_via_git(repo.path(), rel).expect("head");
        assert!(!head.binary);
        assert!(
            head.text.contains(rel),
            "core.quotePath=false keeps the real name in the patch: {}",
            head.text
        );
        assert!(head.text.contains("+after"));
    }

    #[test]
    fn the_banner_names_the_shown_and_total_line_counts() {
        let head = DiffHead {
            text: String::new(),
            shown_lines: 4_000,
            total_lines: 12_345,
            truncated: true,
            total_is_at_least: false,
            binary: false,
        };
        assert_eq!(
            diff_head_banner(&head),
            "Diff cut here: showing the first 4000 of 12345 lines. Open the file in your editor \
             or run git diff to see the rest."
        );
    }

    /// When counting stopped at the bound the banner says so rather than
    /// passing a floor off as the figure.
    #[test]
    fn the_banner_says_more_than_when_counting_stopped() {
        let head = DiffHead {
            text: String::new(),
            shown_lines: 4_000,
            total_lines: DIFF_HEAD_MAX_COUNTED_LINES,
            truncated: true,
            total_is_at_least: true,
            binary: false,
        };
        assert!(
            diff_head_banner(&head).contains("of more than 2000000 lines."),
            "unexpected banner: {}",
            diff_head_banner(&head)
        );
    }

    #[test]
    fn absent_on_both_sides_errors() {
        let repo = init_repo();
        let err = file_diff_contents(repo.path(), "never-existed.txt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }
}
