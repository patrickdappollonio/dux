//! Headless, serializable diff source shared by web clients. Returns the two raw
//! sides of a single file (its content at HEAD and its working-copy content) as
//! whole text, decoded by the editor's encoding rules (`text_encoding`), leaving
//! the actual diff rendering to the client. The web UI runs Monaco's `DiffEditor`
//! over these two sides; binary content is reported as `binary: true` with empty
//! sides. No syntax highlighting, no
//! terminal/ratatui types: the TUI keeps its own syntect+ratatui diff renderer
//! in `dux-tui/src/diff.rs`.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Context;
use serde::Serialize;

use crate::text::group_digits;
use crate::worktree_file::MAX_EDITABLE_BYTES;

/// How many lines of a past-the-ceiling diff both surfaces show. Enough to read
/// the shape of the change; small enough that the browser and the terminal pane
/// stay responsive holding it.
pub const DIFF_HEAD_MAX_LINES: usize = 4_000;

/// How far the line count is allowed to run before it stops counting. A diff
/// this long is already unreadable, and walking a multi-gigabyte patch to reach
/// an exact figure buys nothing; past it the total is reported as a floor.
pub const DIFF_HEAD_MAX_COUNTED_LINES: usize = 2_000_000;

/// How many bytes of patch the head may carry, whatever the line count says.
///
/// [`DIFF_HEAD_MAX_LINES`] alone is no bound for a minified bundle, where four
/// thousand lines can be a hundred megabytes. Kept lines stop at the last one
/// that fits, so the text still ends on a line boundary and `shown_lines` still
/// counts exactly what is in it.
pub const DIFF_HEAD_MAX_BYTES: usize = 2 << 20;

/// How many bytes of each side are sampled to decide text versus binary.
const BINARY_SAMPLE_BYTES: usize = 8 * 1024;

/// The first [`DIFF_HEAD_MAX_LINES`] lines of git's own unified patch for one
/// file, with an honest account of how much was left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one diff, sent to the web client as JSON booleans"
)]
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
    /// True when either side is binary (a NUL, a binary signature, or git's
    /// own binary verdict). `text` is then empty and the caller should say so
    /// rather than render a patch.
    pub binary: bool,
    /// The WHATWG name of the encoding the patch's content lines were read in.
    /// `None` for binary.
    pub encoding: Option<String>,
    /// The encoding is the windows-1252 last resort for content that is not
    /// UTF-8, not something the file said.
    pub encoding_fallback: bool,
}

/// Read the head of git's own diff for one file, for the case where dux's
/// in-process diff refuses the pair as too large.
///
/// The deliberate exception to computing diffs in process: a file past the
/// ceiling would hold a dux worker for minutes, while git answers in well under
/// a second and this only ever reads the first few thousand lines of what it
/// says.
pub fn diff_head_via_git(worktree: &Path, rel_path: &str) -> anyhow::Result<DiffHead> {
    diff_head_via_git_as(worktree, rel_path, None)
}

/// [`diff_head_via_git`] reading the file's lines in `requested` rather than by
/// the editor's automatic rule (UTF-8 when the shown content is, otherwise
/// windows-1252). A requested encoding the content is not valid in is refused
/// rather than shown with replacement characters.
pub fn diff_head_via_git_as(
    worktree: &Path,
    rel_path: &str,
    requested: Option<&'static encoding_rs::Encoding>,
) -> anyhow::Result<DiffHead> {
    let working_path = crate::git::resolve_worktree_path(worktree, rel_path)?;

    let head_prefix = crate::git::file_prefix_at_head(worktree, rel_path, BINARY_SAMPLE_BYTES)?;
    let working_prefix = read_prefix(&working_path, BINARY_SAMPLE_BYTES)?;
    if head_prefix.is_none() && working_prefix.is_none() {
        anyhow::bail!("file not found in the worktree or at HEAD: {rel_path}");
    }
    // Binary is real binary only: a NUL or a known signature in the head of
    // either side (UTF-16 included, whose zero bytes git calls binary too).
    // Text in any encoding is text, decoded below.
    let sample_is_text = |sample: &Option<Vec<u8>>| {
        sample.as_deref().is_none_or(|b| {
            !b.contains(&0)
                && content_inspector::inspect(b) != content_inspector::ContentType::BINARY
        })
    };
    if !sample_is_text(&head_prefix) || !sample_is_text(&working_prefix) {
        return Ok(binary_head());
    }

    // Absent at HEAD means untracked: git's index-based diff has nothing to
    // compare, so the file is diffed against the empty file directly, and that
    // form alone answers "the two differ" with exit code 1.
    let no_index = head_prefix.is_none();

    let mut command = Command::new("git");
    command.args(["-C", worktree.to_string_lossy().as_ref()]);
    // Pin every setting that changes the patch's shape or its length. A reader
    // with `diff.context = 10` must not get different counts from the same
    // file, and a textconv filter must not run a program of the user's choosing
    // over a file dux is only trying to show the head of.
    command.args([
        "-c",
        "core.quotePath=false",
        "-c",
        "diff.noprefix=false",
        "-c",
        "diff.algorithm=myers",
        "-c",
        "diff.context=3",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ]);
    if no_index {
        command.args(["--no-index", "--", "/dev/null", rel_path]);
    } else {
        command.args(["HEAD", "--", rel_path]);
    }
    // The binary marker below is one of the few git messages this READS rather
    // than shows, so the child is pinned to the C locale: a translated build
    // would otherwise print a sentence no prefix check recognises.
    command.env("LC_ALL", "C").env("LANG", "C");

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run git diff for {rel_path}"))?;

    // Drain stderr on a thread of its own. A full stderr pipe blocks git mid
    // write, and git would then never finish the stdout this is reading, which
    // is a deadlock rather than a slow answer.
    let stderr_pipe = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_string(&mut buffer);
        }
        buffer
    });

    // Everything past the spawn goes through this, so no early return can leave
    // a child behind.
    /// Whether the child is killed before it is waited for.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Reap {
        /// Kill it first: its output is no longer wanted.
        KillFirst,
        /// Let it finish on its own.
        LetFinish,
    }
    fn reap(child: &mut std::process::Child, how: Reap) -> Option<i32> {
        if how == Reap::KillFirst {
            let _ = child.kill();
        }
        child.wait().ok().and_then(|status| status.code())
    }

    let Some(stdout) = child.stdout.take() else {
        reap(&mut child, Reap::KillFirst);
        anyhow::bail!("git diff produced no output stream for {rel_path}");
    };

    // The kept lines as git wrote them. The file's own bytes sit after a
    // one-byte marker in each hunk line, and are decoded once the encoding
    // is known; the byte cap is checked again on the decoded text.
    let mut kept_lines: Vec<Vec<u8>> = Vec::new();
    let mut kept_bytes = 0usize;
    let mut shown_lines = 0usize;
    let mut total_lines = 0usize;
    let mut total_is_at_least = false;
    let mut saw_hunk = false;
    let mut saw_binary_marker = false;
    let mut read_error = None;
    for line in BufReader::new(stdout).split(b'\n') {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                read_error = Some(e);
                break;
            }
        };
        total_lines += 1;
        if !saw_hunk && line.starts_with(b"@@") {
            saw_hunk = true;
        }
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            saw_binary_marker = true;
        }
        if shown_lines < DIFF_HEAD_MAX_LINES && kept_bytes + line.len() < DIFF_HEAD_MAX_BYTES {
            kept_bytes += line.len() + 1;
            kept_lines.push(line);
            shown_lines += 1;
        }
        if total_lines >= DIFF_HEAD_MAX_COUNTED_LINES {
            total_is_at_least = true;
            break;
        }
    }

    let code = reap(
        &mut child,
        if total_is_at_least {
            Reap::KillFirst
        } else {
            Reap::LetFinish
        },
    );
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Some(e) = read_error {
        return Err(e).with_context(|| format!("could not read git diff for {rel_path}"));
    }
    // Exit code 1 is how `--no-index` says the two files differ, which is the
    // normal answer on that branch alone. On the HEAD branch it is a failure
    // like any other.
    let acceptable = code == Some(0) || (no_index && code == Some(1));
    if !acceptable && !total_is_at_least {
        anyhow::bail!("git diff failed for {rel_path}: {}", stderr.trim());
    }

    // A file marked `-diff` in .gitattributes is one git refuses to diff, so it
    // prints the binary summary instead of hunks. That is the binary verdict,
    // not a three-line patch.
    if saw_binary_marker && !saw_hunk {
        return Ok(binary_head());
    }

    let (encoding, encoding_fallback, mut lines) = decode_patch_lines(&kept_lines, requested)?;
    // A decoded line can be longer than its bytes (windows-1252 above 0x7F is
    // two or three UTF-8 bytes), so the cap is applied again to the text,
    // dropping whole lines from the end so it still ends on a boundary.
    let mut size: usize = lines.iter().map(|l| l.len() + 1).sum();
    while size >= DIFF_HEAD_MAX_BYTES {
        let Some(last) = lines.pop() else { break };
        size -= last.len() + 1;
    }
    let shown_lines = lines.len();
    let mut text = String::with_capacity(size);
    for line in &lines {
        text.push_str(line);
        text.push('\n');
    }

    Ok(DiffHead {
        truncated: shown_lines < total_lines,
        text,
        shown_lines,
        total_lines,
        total_is_at_least,
        binary: false,
        encoding: Some(encoding.name().to_string()),
        encoding_fallback,
    })
}

/// A requested encoding the patch's file lines are not valid in. Its own type,
/// so a caller can answer with exactly this reason rather than wrap it in the
/// story of how the diff head came to be asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchNotInEncoding {
    pub encoding: &'static encoding_rs::Encoding,
    /// 1-based line of the patch that does not decode.
    pub line: usize,
}

impl std::fmt::Display for PatchNotInEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the diff is not valid {}: line {} of the patch is not text in that encoding",
            self.encoding.name(),
            self.line
        )
    }
}

impl std::error::Error for PatchNotInEncoding {}

/// Decode a patch's kept lines. Header lines and hunk headers are git's own
/// (ASCII, with paths as UTF-8); in a hunk, everything after the leading ` `,
/// `+` or `-` is the file's bytes. Those are read in `requested`, refusing any
/// line that is not valid in it, or else as UTF-8 when every one is valid
/// UTF-8 and as windows-1252 (which reads every byte) otherwise.
///
/// Splitting at `\n` is safe for every encoding that gets here: UTF-16, the
/// one family whose characters contain that byte, is binary to git and to the
/// sample check above.
fn decode_patch_lines(
    lines: &[Vec<u8>],
    requested: Option<&'static encoding_rs::Encoding>,
) -> anyhow::Result<(&'static encoding_rs::Encoding, bool, Vec<String>)> {
    let mut in_hunk = false;
    let content: Vec<bool> = lines
        .iter()
        .map(|line| {
            if line.starts_with(b"@@") {
                in_hunk = true;
                return false;
            }
            in_hunk && matches!(line.first(), Some(b' ' | b'+' | b'-'))
        })
        .collect();
    let (encoding, fallback) = if let Some(encoding) = requested {
        (encoding, false)
    } else {
        let all_utf8 = lines
            .iter()
            .zip(&content)
            .all(|(line, is_content)| !is_content || std::str::from_utf8(&line[1..]).is_ok());
        if all_utf8 {
            (encoding_rs::UTF_8, false)
        } else {
            (encoding_rs::WINDOWS_1252, true)
        }
    };
    let mut out = Vec::with_capacity(lines.len());
    for (index, (line, is_content)) in lines.iter().zip(&content).enumerate() {
        if !is_content {
            out.push(String::from_utf8_lossy(line).into_owned());
            continue;
        }
        let Some(body) = encoding.decode_without_bom_handling_and_without_replacement(&line[1..])
        else {
            return Err(anyhow::Error::new(PatchNotInEncoding {
                encoding,
                line: index + 1,
            }));
        };
        let mut decoded = String::with_capacity(body.len() + 1);
        decoded.push(char::from(line[0]));
        decoded.push_str(&body);
        out.push(decoded);
    }
    Ok((encoding, fallback, out))
}

/// The answer for content no patch can describe: no text, no counts, and the
/// flag that tells the caller to say so.
fn binary_head() -> DiffHead {
    DiffHead {
        text: String::new(),
        shown_lines: 0,
        total_lines: 0,
        truncated: false,
        total_is_at_least: false,
        binary: true,
        encoding: None,
        encoding_fallback: false,
    }
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
    // Only ever a regular file: a FIFO put at the path would otherwise block
    // the read until something writes to it.
    let file = crate::worktree_file::open_regular_nofollow(
        path,
        rustix::fs::OFlags::RDONLY,
        rustix::fs::Mode::empty(),
        |e| anyhow::anyhow!("could not read {}: {e}", path.display()),
    )?;
    let mut buffer = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut buffer)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(Some(buffer))
}

/// The one sentence both surfaces put ABOVE a cut-short diff, word for word.
/// Empty when nothing was cut.
///
/// Above rather than below: a reader who has to scroll four thousand rows to
/// learn the diff was cut has already been misled by everything before it.
pub fn diff_head_banner(head: &DiffHead) -> String {
    if !head.truncated {
        return String::new();
    }
    let noun = if head.total_lines == 1 {
        "line"
    } else {
        "lines"
    };
    let lines = format!("{} {noun}", group_digits(head.total_lines));
    let total = if head.total_is_at_least {
        format!("more than {lines}")
    } else {
        lines
    };
    format!(
        "Diff cut here: showing the first {} of {total}. Open the file in your editor or run \
         git diff to see the rest.",
        group_digits(head.shown_lines)
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
    /// True when either side is binary. `original` and `modified` are
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
///
/// Two checks, because each misses what the other catches. `content_inspector`
/// catches UTF-8 byte streams that nonetheless contain NUL/control bytes, which
/// `std::str::from_utf8` alone would accept and render garbled. But it only
/// sniffs the head for NULs and magic numbers and never validates the encoding,
/// so it calls Latin-1 or Shift-JIS text "UTF-8"; every reader decodes after
/// this check, and a decode of those bytes yields nothing, which the editor
/// would serve as an empty buffer a save then writes over the real file. The
/// full validation is what keeps that from happening.
///
/// The SINGLE binary-vs-text predicate for the whole app, so a file is never
/// classified as binary on one surface and text on another.
pub fn is_renderable_text(bytes: &[u8]) -> bool {
    bytes.is_empty() || (std::str::from_utf8(bytes).is_ok() && sniffs_as_utf8(bytes))
}

fn sniffs_as_utf8(bytes: &[u8]) -> bool {
    content_inspector::inspect(bytes) == content_inspector::ContentType::UTF_8
}

/// Read the two sides of a single file's working-tree-vs-HEAD diff as whole
/// text: `original` is the file at HEAD, `modified` the working copy on disk. A
/// path absent on one side yields an empty string there. Each side is decoded by
/// the editor's encoding rules (see [`decode_diff_side`]); binary content on
/// either side yields `binary: true` with empty sides.
///
/// SECURITY: `rel_path` must be worktree-relative. Absolute paths, any `..`,
/// root or prefix component, and symlinks that escape the worktree are rejected,
/// since the web passes client-supplied paths here.
pub fn file_diff_contents(worktree: &Path, rel_path: &str) -> anyhow::Result<DiffContents> {
    file_diff_contents_as(worktree, rel_path, None)
}

/// [`file_diff_contents`] reading both sides in `encoding` where they decode in
/// it: the encoding the person reopened the file in, so the diff and the
/// editor tab show the same characters.
pub fn file_diff_contents_as(
    worktree: &Path,
    rel_path: &str,
    encoding: Option<&'static encoding_rs::Encoding>,
) -> anyhow::Result<DiffContents> {
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
        // No-follow and regular files only, like the editor's own read.
        crate::worktree_file::read_nofollow(&working_path)
            .with_context(|| format!("could not read working copy of {rel_path}"))?
    } else {
        Vec::new()
    };
    let old_bytes = crate::git::file_bytes_at_head(worktree, rel_path)?.unwrap_or_default();

    match (
        decode_diff_side(&old_bytes, encoding),
        decode_diff_side(&new_bytes, encoding),
    ) {
        (Some(original), Some(modified)) => Ok(DiffContents {
            path: rel_path.to_string(),
            original,
            modified,
            binary: false,
        }),
        _ => Ok(DiffContents {
            path: rel_path.to_string(),
            original: String::new(),
            modified: String::new(),
            binary: true,
        }),
    }
}

/// One side of a diff as text, by the editor's own rules: the encoding the
/// editor tab was reopened in when the side decodes cleanly in it, otherwise
/// the automatic ones (a byte-order mark, UTF-8, else windows-1252). The second
/// chance is deliberate and safe only because a diff is read-only: a file
/// converted between encodings has a HEAD side in the old one, and refusing the
/// whole diff over it would hide the change rather than protect anything.
fn decode_diff_side(
    bytes: &[u8],
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Option<String> {
    use crate::text_encoding::{Decoded, decode};
    if let Some(encoding) = encoding
        && let Ok(Decoded::Text(t)) = decode(bytes, Some(encoding))
    {
        return Some(t.text);
    }
    match decode(bytes, None) {
        Ok(Decoded::Text(t)) => Some(t.text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `content_inspector` only sniffs for NUL bytes and magic numbers, so on its
    /// own it calls Latin-1 and Shift-JIS "UTF-8". Those must not be text, or
    /// every reader that decodes after this check decodes them to nothing.
    #[test]
    fn is_renderable_text_refuses_bytes_that_are_not_utf8() {
        assert!(!is_renderable_text(b"caf\xe9 au lait\n"));
        // Shift-JIS for "nihongo".
        assert!(!is_renderable_text(&[0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea]));
        // An invalid byte far past the sniffed head is still caught.
        let mut late = vec![b'a'; 4096];
        late.push(0xe9);
        assert!(!is_renderable_text(&late));
        // A whole file that ends mid-scalar is not valid UTF-8 either.
        assert!(!is_renderable_text(&"caf\u{e9}".as_bytes()[..4]));
    }

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

    /// A windows-1252 change is a readable diff, decoded the way the editor
    /// opens the file, rather than a refusal.
    #[test]
    fn a_windows_1252_change_is_diffed_as_text() {
        let repo = init_repo();
        commit_file_bytes(repo.path(), "f.txt", b"caf\xe9\n");
        std::fs::write(repo.path().join("f.txt"), b"th\xe9\n").expect("overwrite");
        let c = file_diff_contents(repo.path(), "f.txt").expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "caf\u{e9}\n");
        assert_eq!(c.modified, "th\u{e9}\n");
    }

    /// The tab's chosen encoding reads both sides where it can, and a side it
    /// cannot read (the old encoding of a converted file) still shows by the
    /// automatic rules rather than hiding the whole change.
    #[test]
    fn a_requested_encoding_reads_the_sides_it_can() {
        let repo = init_repo();
        let sjis = [0x93, 0xfa, 0x96, 0x7b, b'\n'];
        commit_file_bytes(repo.path(), "f.txt", &sjis);
        std::fs::write(repo.path().join("f.txt"), "\u{65e5}\u{672c}\u{8a9e}\n").expect("overwrite");
        let c = file_diff_contents_as(repo.path(), "f.txt", Some(encoding_rs::SHIFT_JIS))
            .expect("contents");
        assert!(!c.binary);
        assert_eq!(c.original, "\u{65e5}\u{672c}\n");
        assert_eq!(c.modified, "\u{65e5}\u{672c}\u{8a9e}\n");
    }

    /// A UTF-8 byte stream that nonetheless contains a NUL must be treated as
    /// binary (matching the TUI's `content_inspector` check), not rendered as text.
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

    /// A large file that is not UTF-8 is a readable patch, read by the
    /// editor's own rule: UTF-8 when it is, otherwise windows-1252, said to be a
    /// fallback.
    #[test]
    fn a_windows_1252_head_is_readable_and_says_it_was_a_fallback() {
        let repo = init_repo();
        commit_file_bytes(repo.path(), "f.txt", b"caf\xe9\n");
        std::fs::write(repo.path().join("f.txt"), b"th\xe9\n").expect("overwrite");
        let head = diff_head_via_git(repo.path(), "f.txt").expect("head");
        assert!(!head.binary);
        assert!(head.text.contains("-caf\u{e9}\n"), "{}", head.text);
        assert!(head.text.contains("+th\u{e9}\n"), "{}", head.text);
        assert_eq!(head.encoding.as_deref(), Some("windows-1252"));
        assert!(head.encoding_fallback);
        assert!(head.text.ends_with('\n'));
    }

    #[test]
    fn a_utf8_head_says_utf8() {
        let repo = init_repo();
        commit_file(repo.path(), "f.txt", "caf\u{e9}\n");
        std::fs::write(repo.path().join("f.txt"), "th\u{e9}\n").expect("overwrite");
        let head = diff_head_via_git(repo.path(), "f.txt").expect("head");
        assert_eq!(head.encoding.as_deref(), Some("UTF-8"));
        assert!(!head.encoding_fallback);
        assert!(head.text.contains("+th\u{e9}\n"));
    }

    #[test]
    fn a_head_reads_in_a_requested_encoding_or_refuses_it_truthfully() {
        let repo = init_repo();
        commit_file_bytes(repo.path(), "f.txt", &[0x93, 0xfa, b'\n']);
        std::fs::write(repo.path().join("f.txt"), [0x93, 0xfa, 0x96, 0x7b, b'\n'])
            .expect("overwrite");
        let head =
            diff_head_via_git_as(repo.path(), "f.txt", Some(encoding_rs::SHIFT_JIS)).expect("head");
        assert!(head.text.contains("+\u{65e5}\u{672c}\n"), "{}", head.text);
        assert_eq!(head.encoding.as_deref(), Some("Shift_JIS"));
        assert!(!head.encoding_fallback);

        // These bytes are not UTF-8, and a requested decode that would need
        // replacement characters is refused rather than shown.
        let err = diff_head_via_git_as(repo.path(), "f.txt", Some(encoding_rs::UTF_8)).unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");
        // Typed, so the route answers with this reason alone.
        assert!(err.downcast_ref::<PatchNotInEncoding>().is_some());
    }

    /// UTF-16 is full of NULs, and git itself calls it binary.
    #[test]
    fn utf16_and_nul_bytes_are_still_binary_in_a_head() {
        let repo = init_repo();
        commit_file(repo.path(), "w.txt", "x\n");
        std::fs::write(repo.path().join("w.txt"), [0xFF, 0xFE, b'h', 0, b'\n', 0])
            .expect("overwrite");
        assert!(
            diff_head_via_git(repo.path(), "w.txt")
                .expect("head")
                .binary
        );
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

    /// A file the repository marks `-diff` is one git refuses to diff: it
    /// prints the binary summary instead of hunks. That is the binary verdict,
    /// not a three-line patch the viewer would render as content.
    #[test]
    fn a_file_marked_undiffable_by_gitattributes_reads_as_binary() {
        let repo = init_repo();
        commit_file(repo.path(), ".gitattributes", "*.gen -diff\n");
        commit_file(repo.path(), "build.gen", "one\ntwo\n");
        std::fs::write(repo.path().join("build.gen"), "one\nTWO\n").expect("overwrite");

        let head = diff_head_via_git(repo.path(), "build.gen").expect("head");
        assert!(
            head.binary,
            "git printed a binary summary, so this is not a patch: {:?}",
            head.text
        );
        assert_eq!(head.text, "");
        assert_eq!(head.total_lines, 0);
    }

    /// A reader's own `diff.context` must not move the counts dux reports: the
    /// same file has to produce the same banner on every machine.
    #[test]
    fn a_readers_diff_context_setting_does_not_move_the_counts() {
        let repo = init_repo();
        let base: String = (0..60).map(|i| format!("line {i}\n")).collect();
        commit_file(repo.path(), "ctx.txt", &base);
        let changed = base.replace("line 30\n", "CHANGED\n");
        std::fs::write(repo.path().join("ctx.txt"), changed).expect("overwrite");

        let pinned = diff_head_via_git(repo.path(), "ctx.txt").expect("head");

        // Ask the repository for ten lines of context and read it again. With
        // the setting pinned on the command line, the answer cannot change.
        let run = |args: &[&str]| {
            crate::git::test_support::git_command()
                .args(args)
                .current_dir(repo.path())
                .status()
                .expect("spawn git");
        };
        run(&["config", "diff.context", "10"]);
        let widened = diff_head_via_git(repo.path(), "ctx.txt").expect("head");

        assert_eq!(
            pinned.total_lines, widened.total_lines,
            "a repository-level diff.context changed the reported line count"
        );
        assert_eq!(pinned.text, widened.text);
    }

    /// Four thousand lines is no bound at all for a minified file, so the head
    /// carries a byte cap too, and it still ends on a line boundary.
    #[test]
    fn a_patch_of_enormous_lines_is_cut_by_bytes_not_only_by_lines() {
        let repo = init_repo();
        // Ten lines of a quarter megabyte each: far under the line cap, far
        // over the byte cap.
        let long = "x".repeat(256 * 1024);
        let base: String = (0..10).map(|i| format!("{long}{i}\n")).collect();
        commit_file(repo.path(), "min.js", &base);
        let changed: String = (0..10).map(|i| format!("{long}z{i}\n")).collect();
        std::fs::write(repo.path().join("min.js"), changed).expect("overwrite");

        let head = diff_head_via_git(repo.path(), "min.js").expect("head");
        assert!(
            head.shown_lines < DIFF_HEAD_MAX_LINES,
            "the byte cap, not the line cap, is what stopped this"
        );
        assert!(
            head.text.len() <= DIFF_HEAD_MAX_BYTES,
            "the head is {} bytes, over the cap",
            head.text.len()
        );
        assert!(head.truncated);
        assert!(
            head.text.is_empty() || head.text.ends_with('\n'),
            "the cut still lands on a line boundary"
        );
        assert_eq!(
            head.text.lines().count(),
            head.shown_lines,
            "shown_lines counts exactly what was kept"
        );
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
            encoding: None,
            encoding_fallback: false,
        };
        assert_eq!(
            diff_head_banner(&head),
            "Diff cut here: showing the first 4,000 of 12,345 lines. Open the file in your \
             editor or run git diff to see the rest."
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
            encoding: None,
            encoding_fallback: false,
        };
        assert!(
            diff_head_banner(&head).contains("of more than 2,000,000 lines."),
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
