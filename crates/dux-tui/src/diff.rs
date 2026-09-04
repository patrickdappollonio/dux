use std::path::Path;

use anyhow::Result;
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{Change, ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SynColor, FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::theme::Theme as AppTheme;

/// Cached syntax highlighting resources to avoid reloading on every diff.
pub struct SyntaxCache {
    pub syntax_set: SyntaxSet,
    pub theme_set: ThemeSet,
}

impl SyntaxCache {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }
}

/// Pre-rendered diff ready for display.
pub struct DiffOutput {
    pub lines: Vec<Line<'static>>,
    /// Display-column width of the gutter (line numbers + separator + prefix).
    /// Zero when line numbers are disabled.
    pub gutter_width: usize,
}

fn diff_line_number_width(text_diff: &TextDiff<'_, '_, str>, enabled: bool) -> usize {
    if !enabled {
        return 0;
    }

    let mut maximum = 1;
    for hunk in text_diff.unified_diff().context_radius(3).iter_hunks() {
        for change in hunk.iter_changes() {
            maximum = maximum.max(change.old_index().map_or(0, |index| index + 1));
            maximum = maximum.max(change.new_index().map_or(0, |index| index + 1));
        }
    }
    maximum.to_string().len()
}

fn change_line_numbers(change: &Change<&str>, tag: ChangeTag, width: usize) -> String {
    let old = match tag {
        ChangeTag::Delete | ChangeTag::Equal => {
            format!("{:>width$}", change.old_index().unwrap_or(0) + 1)
        }
        ChangeTag::Insert => " ".repeat(width),
    };
    let new = match tag {
        ChangeTag::Insert | ChangeTag::Equal => {
            format!("{:>width$}", change.new_index().unwrap_or(0) + 1)
        }
        ChangeTag::Delete => " ".repeat(width),
    };
    format!("{old} {new} ")
}

struct ChangeRenderContext<'a> {
    theme: &'a AppTheme,
    cache: &'a SyntaxCache,
    show_line_numbers: bool,
    line_number_width: usize,
    tab_width: u16,
    /// False once the file is big enough that syntect's per-line cost is the
    /// dominant one (see [`DiffSizeVerdict`]). The added/removed coloring is
    /// unaffected; only the token colors inside a line go away.
    highlight: bool,
}

fn render_change_line<'a>(
    change: &Change<&str>,
    context: &ChangeRenderContext<'_>,
    old_highlighter: &mut HighlightLines<'a>,
    new_highlighter: &mut HighlightLines<'a>,
) -> Line<'static> {
    let theme = context.theme;
    let tag = change.tag();
    let (prefix, base_fg, background, highlighter) = match tag {
        ChangeTag::Delete => (
            "-",
            theme.diff_remove,
            Some(theme.diff_remove_bg),
            old_highlighter,
        ),
        ChangeTag::Insert => (
            "+",
            theme.diff_add,
            Some(theme.diff_add_bg),
            new_highlighter,
        ),
        ChangeTag::Equal => (" ", Color::Reset, None, new_highlighter),
    };
    let content = expand_tabs(change.value().trim_end_matches('\n'), context.tab_width);
    let mut spans = Vec::new();
    if context.show_line_numbers {
        spans.push(Span::styled(
            change_line_numbers(change, tag, context.line_number_width),
            Style::default().fg(theme.diff_line_number_fg),
        ));
        spans.push(Span::styled(
            "│",
            Style::default().fg(theme.diff_line_number_sep),
        ));
    }

    // `None` means "render this line plainly", which a skipped highlight and a
    // highlighter that could not parse the line reach by different routes and
    // want the same answer to.
    let highlighted = if context.highlight {
        highlighter
            .highlight_line(&content, &context.cache.syntax_set)
            .ok()
    } else {
        None
    };

    match highlighted {
        Some(ranges) if tag == ChangeTag::Equal => {
            spans.push(Span::styled(prefix, Style::default().fg(base_fg)));
            spans.extend(
                ranges
                    .into_iter()
                    .map(|(style, text)| Span::styled(text.to_string(), syntect_to_ratatui(style))),
            );
        }
        Some(ranges) => {
            let background = background.unwrap_or(Color::Reset);
            spans.push(Span::styled(
                prefix,
                Style::default().fg(base_fg).bg(background),
            ));
            spans.extend(ranges.into_iter().map(|(style, text)| {
                Span::styled(text.to_string(), syntect_to_ratatui(style).bg(background))
            }));
        }
        None => spans.push(Span::styled(
            format!("{prefix}{content}"),
            Style::default()
                .fg(base_fg)
                .bg(background.unwrap_or(Color::Reset)),
        )),
    }
    Line::from(spans)
}

/// Everything that identifies one diff request, so an answer that arrives after
/// the user has moved on can be recognised and dropped.
///
/// The paths say WHICH file, and the two render settings say which shape of it:
/// toggling line numbers re-requests the same file and the old answer, keyed on
/// the path alone, would be indistinguishable from the new one. `seq` separates
/// two requests that agree on all of that, which is what a plain refresh of an
/// unchanged view is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffRequestKey {
    pub(crate) worktree_path: String,
    pub(crate) rel_path: String,
    pub(crate) show_line_numbers: bool,
    pub(crate) tab_width: u16,
    pub(crate) seq: u64,
}

/// A finished diff, tagged with the request that asked for it.
pub(crate) struct DiffAnswer {
    pub(crate) key: DiffRequestKey,
    /// The rendered diff, or the message the read failed with. Carried rather
    /// than flattened into empty lines: "git could not answer" and "this file
    /// has no changes" are different facts.
    pub(crate) result: Result<DiffOutput, String>,
}

/// Compute one diff on a thread of its own and post the answer back.
///
/// The only way to reach [`diff_file`] from outside this module. The theme is
/// copied and the syntax cache is shared by `Arc`, so the worker owns
/// everything it renders with and the run loop keeps drawing while it works.
pub(crate) fn spawn_diff_job(
    key: DiffRequestKey,
    theme: AppTheme,
    cache: std::sync::Arc<SyntaxCache>,
    tx: std::sync::mpsc::Sender<DiffAnswer>,
) {
    std::thread::spawn(move || {
        let result = diff_file(
            Path::new(&key.worktree_path),
            &key.rel_path,
            &theme,
            &cache,
            key.show_line_numbers,
            key.tab_width,
        )
        .map_err(|error| format!("{error:#}"));
        let _ = tx.send(DiffAnswer { key, result });
    });
}

/// Combined size of the two versions, in bytes, above which the diff is
/// rendered with no syntax highlighting.
///
/// A megabyte of source is where syntect stops being free: it highlights every
/// line of every hunk, a whole-file rewrite makes every line a hunk line, and
/// the per-line cost is what turns a diff that took a moment into one that
/// takes seconds. Dropping the token colors keeps the part of a diff people
/// actually read (which lines went, which arrived) and costs only the colors
/// inside them.
pub(crate) const DIFF_PLAIN_ABOVE_BYTES: usize = 1 << 20;

/// Combined line count of the two versions above which the diff is rendered
/// with no syntax highlighting.
///
/// Bytes alone miss the shape that hurts most: a file of very short lines is
/// cheap to read and expensive to highlight, because the cost is per line
/// rather than per byte. Twenty thousand lines is comfortably above anything
/// hand-written and comfortably below the point where the wait is noticeable.
pub(crate) const DIFF_PLAIN_ABOVE_LINES: usize = 20_000;

/// Combined size of the two versions, in bytes, above which dux says so
/// instead of diffing.
///
/// The diff itself, not the highlighting, is what costs here: `similar` is
/// superlinear in the number of differing lines, so a pair of multi-megabyte
/// files can hold a worker thread for minutes. Sixteen megabytes is far past
/// anything a person reads in a terminal pane and still leaves generated
/// lockfiles and vendored sources comfortably inside.
pub(crate) const DIFF_REFUSE_ABOVE_BYTES: usize = 16 << 20;

/// Combined line count of the two versions above which dux says so instead of
/// diffing. The line-shaped twin of [`DIFF_REFUSE_ABOVE_BYTES`], for the same
/// reason [`DIFF_PLAIN_ABOVE_LINES`] exists.
pub(crate) const DIFF_REFUSE_ABOVE_LINES: usize = 200_000;

/// What the two versions' size says dux should do with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffSizeVerdict {
    /// Small enough for the ordinary syntax-highlighted diff.
    Full,
    /// Diff it, but without syntax highlighting.
    Plain,
    /// Too large to diff at all; say so rather than freezing on it.
    TooLarge,
}

/// Judge the two versions by both bytes and lines, taking the worse answer.
///
/// Sums the two sides rather than looking at the larger one: the work is done
/// over both, and a one-line file replaced by a ten-megabyte one costs the same
/// as two five-megabyte ones.
pub(crate) fn diff_size_verdict(old: &[u8], new: &[u8]) -> DiffSizeVerdict {
    let bytes = old.len() + new.len();
    let lines = count_lines(old) + count_lines(new);
    if bytes > DIFF_REFUSE_ABOVE_BYTES || lines > DIFF_REFUSE_ABOVE_LINES {
        DiffSizeVerdict::TooLarge
    } else if bytes > DIFF_PLAIN_ABOVE_BYTES || lines > DIFF_PLAIN_ABOVE_LINES {
        DiffSizeVerdict::Plain
    } else {
        DiffSizeVerdict::Full
    }
}

/// Count the lines the diff will actually walk: newlines, plus a last line with
/// no newline after it.
fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// Compute a syntax-highlighted, unified diff for a single file.
///
/// `worktree_path` is the root of the git worktree and `rel_path` is the
/// file path relative to it (as reported by `git status --porcelain`).
///
/// Private on purpose: every one of the git reads, the whole-file read, the
/// line diff and the highlighting inside it is slow enough to freeze the run
/// loop on a large file, and the only way to reach it from the app is
/// [`spawn_diff_job`], which runs it on a thread of its own. Keeping it out of
/// the module's public surface is what makes "never on the UI thread" a
/// compile-time fact rather than a convention.
fn diff_file(
    worktree_path: &Path,
    rel_path: &str,
    theme: &AppTheme,
    cache: &SyntaxCache,
    show_line_numbers: bool,
    diff_tab_width: u16,
) -> Result<DiffOutput> {
    let old_bytes = crate::git::file_bytes_at_head(worktree_path, rel_path)?.unwrap_or_default();
    let abs_path = worktree_path.join(rel_path);
    let new_bytes = std::fs::read(&abs_path).unwrap_or_default();

    if old_bytes == new_bytes {
        return Ok(DiffOutput {
            lines: vec![Line::from("No changes.")],
            gutter_width: 0,
        });
    }

    if !dux_core::diff::is_renderable_text(&old_bytes)
        || !dux_core::diff::is_renderable_text(&new_bytes)
    {
        return Ok(binary_diff_output(
            rel_path,
            old_bytes.len(),
            new_bytes.len(),
            theme,
        ));
    }

    let verdict = diff_size_verdict(&old_bytes, &new_bytes);
    if verdict == DiffSizeVerdict::TooLarge {
        return Ok(too_large_diff_output(
            rel_path,
            old_bytes.len(),
            new_bytes.len(),
            theme,
        ));
    }

    let old_text = String::from_utf8(old_bytes).unwrap_or_default();
    let new_text = String::from_utf8(new_bytes).unwrap_or_default();

    let syn_theme = &cache.theme_set.themes["base16-ocean.dark"];

    let syntax = cache
        .syntax_set
        .find_syntax_by_extension(
            Path::new(rel_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(""),
        )
        .unwrap_or_else(|| cache.syntax_set.find_syntax_plain_text());

    let text_diff = TextDiff::from_lines(&old_text, &new_text);
    let mut lines: Vec<Line<'static>> = Vec::new();

    let ln_width = diff_line_number_width(&text_diff, show_line_numbers);

    let gutter_style = Style::default().fg(theme.diff_line_number_fg);
    let sep_style = Style::default().fg(theme.diff_line_number_sep);
    let change_context = ChangeRenderContext {
        theme,
        cache,
        show_line_numbers,
        line_number_width: ln_width,
        tab_width: diff_tab_width,
        highlight: verdict == DiffSizeVerdict::Full,
    };

    // File header.
    lines.push(Line::from(Span::styled(
        format!("--- a/{rel_path}"),
        Style::default()
            .fg(theme.diff_file_header)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("+++ b/{rel_path}"),
        Style::default()
            .fg(theme.diff_file_header)
            .add_modifier(Modifier::BOLD),
    )));

    for hunk in text_diff.unified_diff().context_radius(3).iter_hunks() {
        // Hunk header (@@ ... @@).
        let mut hunk_spans: Vec<Span<'static>> = Vec::new();
        if show_line_numbers {
            // Blank gutter for hunk headers.
            let blank = " ".repeat(ln_width);
            hunk_spans.push(Span::styled(format!("{blank} {blank} "), gutter_style));
            hunk_spans.push(Span::styled("│ ", sep_style));
        }
        hunk_spans.push(Span::styled(
            hunk.header().to_string(),
            Style::default().fg(theme.diff_hunk),
        ));
        lines.push(Line::from(hunk_spans));

        // We maintain two separate highlighters so that removed lines are
        // highlighted in the context of the old file and added/context lines
        // in the context of the new file. This avoids broken highlighting
        // when a change spans a multi-line construct.
        let mut hl_old = HighlightLines::new(syntax, syn_theme);
        let mut hl_new = HighlightLines::new(syntax, syn_theme);

        for change in hunk.iter_changes() {
            lines.push(render_change_line(
                &change,
                &change_context,
                &mut hl_old,
                &mut hl_new,
            ));
        }
    }

    if lines.len() <= 2 {
        // Only headers, no actual hunks (e.g. binary file or mode change).
        lines.push(Line::from("No text diff available."));
    }

    // Gutter width includes line numbers, separator, and the +/-/space prefix.
    // Layout: "{old_ln} {new_ln} │{prefix}" = 2*ln_width + 2 + 1 + 1 = 2*ln_width + 4.
    let gutter_width = if show_line_numbers {
        2 * ln_width + 4
    } else {
        0
    };

    Ok(DiffOutput {
        lines,
        gutter_width,
    })
}

/// What the pane shows instead of a diff dux refused to compute.
///
/// Honest rather than empty: it names the sizes it judged and says what the
/// user can still do, because the file itself is perfectly readable, just not
/// here.
fn too_large_diff_output(
    rel_path: &str,
    old_size: usize,
    new_size: usize,
    theme: &AppTheme,
) -> DiffOutput {
    DiffOutput {
        lines: vec![
            Line::from(Span::styled(
                format!("--- a/{rel_path}"),
                Style::default()
                    .fg(theme.diff_file_header)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!("+++ b/{rel_path}"),
                Style::default()
                    .fg(theme.diff_file_header)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("File too large to diff."),
            Line::from(format!("Old size: {old_size} bytes")),
            Line::from(format!("New size: {new_size} bytes")),
            Line::from(
                "Comparing versions this large would hold dux up for minutes, so it does not \
                 start. Open the file in your editor, or diff it with git directly.",
            ),
        ],
        gutter_width: 0,
    }
}

fn binary_diff_output(
    rel_path: &str,
    old_size: usize,
    new_size: usize,
    theme: &AppTheme,
) -> DiffOutput {
    DiffOutput {
        lines: vec![
            Line::from(Span::styled(
                format!("--- a/{rel_path}"),
                Style::default()
                    .fg(theme.diff_file_header)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!("+++ b/{rel_path}"),
                Style::default()
                    .fg(theme.diff_file_header)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("Binary file changed."),
            Line::from(format!("Old size: {old_size} bytes")),
            Line::from(format!("New size: {new_size} bytes")),
            Line::from("No text diff available for binary or non-UTF-8 content."),
        ],
        gutter_width: 0,
    }
}

/// Convert a syntect `Style` to a ratatui `Style`.
fn syntect_to_ratatui(style: SynStyle) -> Style {
    let fg = syntect_color(style.foreground);
    let mut ratatui_style = Style::default();
    if let Some(c) = fg {
        ratatui_style = ratatui_style.fg(c);
    }
    if style.font_style.contains(FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }
    ratatui_style
}

/// Convert a syntect RGBA color to a ratatui `Color`, ignoring fully
/// transparent colors (which syntect uses to mean "inherit").
fn syntect_color(c: SynColor) -> Option<Color> {
    if c.a == 0 {
        None
    } else {
        Some(Color::Rgb(c.r, c.g, c.b))
    }
}

/// Expand tab characters to spaces, aligning to tab stops of the given width.
/// If `tab_width` is 0, tabs are left as-is.
fn expand_tabs(line: &str, tab_width: u16) -> String {
    if tab_width == 0 || !line.contains('\t') {
        return line.to_string();
    }
    let tw = tab_width as usize;
    let mut out = String::with_capacity(line.len());
    let mut col: usize = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = tw - (col % tw);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// Split a list of spans at a display-column boundary.
///
/// Returns `(left, right)` where `left` contains the first `col` display
/// columns and `right` contains the remainder. Spans are split mid-span if
/// the boundary falls inside one. Uses character iteration rather than byte
/// slicing to handle multi-byte UTF-8 safely.
fn split_spans_at(spans: &[Span<'static>], col: usize) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut left: Vec<Span<'static>> = Vec::new();
    let mut right: Vec<Span<'static>> = Vec::new();
    let mut consumed: usize = 0;
    let mut past_split = false;

    for span in spans {
        if past_split {
            right.push(span.clone());
            continue;
        }

        let span_width = span.content.chars().count();
        if consumed + span_width <= col {
            // Entire span fits in the left side.
            left.push(span.clone());
            consumed += span_width;
            if consumed == col {
                past_split = true;
            }
        } else {
            // Split within this span.
            let take = col - consumed;
            let left_text: String = span.content.chars().take(take).collect();
            let right_text: String = span.content.chars().skip(take).collect();
            if !left_text.is_empty() {
                left.push(Span::styled(left_text, span.style));
            }
            if !right_text.is_empty() {
                right.push(Span::styled(right_text, span.style));
            }
            past_split = true;
            consumed = col;
        }
    }

    (left, right)
}

/// Build a continuation gutter from real gutter spans: replace every character
/// with a space except `│`, which is kept with its original style. This keeps
/// the gutter separator visually connected on wrapped lines.
fn blank_gutter_keeping_separator(gutter_spans: &[Span<'static>]) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::with_capacity(gutter_spans.len());
    for span in gutter_spans {
        if span.content.contains('│') {
            // Rebuild the span: spaces for every character except │.
            let text: String = span
                .content
                .chars()
                .map(|c| if c == '│' { '│' } else { ' ' })
                .collect();
            out.push(Span::styled(text, span.style));
        } else {
            let blanked: String = " ".repeat(span.content.chars().count());
            out.push(Span::styled(blanked, span.style));
        }
    }
    out
}

/// Find the best soft-break position within the first `max_col` characters of
/// `spans`. Returns the column *after* the last space found, so the space stays
/// on the current visual line and the next line starts with the following word.
/// Returns `None` if no space exists within the window.
fn find_soft_break(spans: &[Span<'static>], max_col: usize) -> Option<usize> {
    let mut last_space_end: Option<usize> = None;
    let mut col: usize = 0;
    for span in spans {
        for ch in span.content.chars() {
            if col >= max_col {
                return last_space_end;
            }
            if ch == ' ' {
                last_space_end = Some(col + 1);
            }
            col += 1;
        }
    }
    last_space_end
}

/// Wrap pre-rendered diff lines so that continuation lines are indented to
/// align with the content column (past the gutter).
///
/// When `gutter_width` is 0 this is a no-op — the caller should fall back to
/// `Paragraph::wrap()`.
pub fn wrap_diff_lines(
    lines: &[Line<'static>],
    total_width: usize,
    gutter_width: usize,
) -> Vec<Line<'static>> {
    if gutter_width == 0 || total_width <= gutter_width {
        return lines.to_vec();
    }

    let content_width = total_width - gutter_width;
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());

    for line in lines {
        let line_width = line.width();
        if line_width <= total_width {
            out.push(line.clone());
            continue;
        }

        // Separate gutter spans from content spans.
        let (gutter_spans, content_spans) = split_spans_at(&line.spans, gutter_width);

        // Build continuation gutter: blank out all characters except the │
        // separator so the gutter column stays visually connected.
        let continuation_gutter = blank_gutter_keeping_separator(&gutter_spans);

        // Split content into chunks of content_width.
        let mut remaining = content_spans;
        let mut first = true;
        loop {
            let remaining_width: usize = remaining.iter().map(|s| s.content.chars().count()).sum();
            if remaining_width == 0 {
                break;
            }

            let take = if remaining_width > content_width {
                // Prefer breaking at a word boundary (after the last space).
                find_soft_break(&remaining, content_width).unwrap_or(content_width)
            } else {
                remaining_width
            };
            let (chunk, rest) = split_spans_at(&remaining, take);

            let mut row_spans: Vec<Span<'static>> = if first {
                gutter_spans.clone()
            } else {
                continuation_gutter.clone()
            };
            row_spans.extend(chunk);
            out.push(Line::from(row_spans));

            remaining = rest;
            first = false;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn binary_files_render_summary_instead_of_text_diff() {
        let dir = tempdir().unwrap();
        let repo = dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo)
            .output()
            .unwrap();

        let file = repo.join("image.bin");
        std::fs::write(&file, [0_u8, 159, 146, 150]).unwrap();
        Command::new("git")
            .args(["add", "image.bin"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(repo)
            .output()
            .unwrap();

        std::fs::write(&file, [0_u8, 159, 146, 151, 152]).unwrap();

        let cache = SyntaxCache::new();
        let output = diff_file(
            repo,
            "image.bin",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();
        let rendered = output
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Binary file changed."))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Old size: 4 bytes"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("New size: 5 bytes"))
        );
    }

    /// Helper: create a git repo with a committed file and then modify it.
    fn setup_text_repo(filename: &str, initial: &str, modified: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let repo = dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo)
            .output()
            .unwrap();

        let file = repo.join(filename);
        std::fs::write(&file, initial).unwrap();
        Command::new("git")
            .args(["add", filename])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(repo)
            .output()
            .unwrap();

        std::fs::write(&file, modified).unwrap();
        dir
    }

    #[test]
    fn unchanged_file_returns_the_quiet_message_without_a_gutter() {
        let dir = setup_text_repo("same.txt", "unchanged\n", "unchanged\n");
        let output = diff_file(
            dir.path(),
            "same.txt",
            &AppTheme::default_dark(),
            &SyntaxCache::new(),
            true,
            4,
        )
        .unwrap();

        assert_eq!(output.lines, vec![Line::from("No changes.")]);
        assert_eq!(output.gutter_width, 0);
    }

    #[test]
    fn deleted_text_file_keeps_the_old_side_and_line_number_gutter() {
        let dir = setup_text_repo("gone.txt", "first\nsecond\n", "");
        let output = diff_file(
            dir.path(),
            "gone.txt",
            &AppTheme::default_dark(),
            &SyntaxCache::new(),
            true,
            4,
        )
        .unwrap();
        let rendered = output
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("-first")));
        assert!(rendered.iter().any(|line| line.contains("-second")));
        assert!(rendered.iter().any(|line| line.contains('│')));
        assert!(output.gutter_width > 0);
    }

    #[test]
    fn line_numbers_appear_when_enabled() {
        let dir = setup_text_repo("hello.txt", "aaa\nbbb\nccc\n", "aaa\nbbb\nXXX\nccc\n");
        let cache = SyntaxCache::new();
        let output = diff_file(
            dir.path(),
            "hello.txt",
            &AppTheme::default_dark(),
            &cache,
            true,
            4,
        )
        .unwrap();
        let rendered: Vec<String> = output.lines.iter().map(|l| l.to_string()).collect();

        // Context line "aaa" should show both old and new line numbers.
        // Max line is 4, so ln_width is 1 — numbers are right-aligned in 1 char.
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("1") && l.contains("aaa")),
            "expected line number 1 for context line 'aaa', got: {rendered:?}"
        );

        // Inserted line "XXX" should show only the new line number.
        let insert_line = rendered
            .iter()
            .find(|l| l.contains("XXX") && l.contains("+"))
            .expect("expected an inserted line containing XXX");
        assert!(
            insert_line.contains("3"),
            "expected new line number 3 for inserted line, got: {insert_line}"
        );

        // Gutter separator should be present.
        assert!(
            rendered.iter().any(|l| l.contains('│')),
            "expected gutter separator │"
        );
    }

    #[test]
    fn line_numbers_absent_when_disabled() {
        let dir = setup_text_repo("hello.txt", "aaa\n", "aaa\nbbb\n");
        let cache = SyntaxCache::new();
        let output = diff_file(
            dir.path(),
            "hello.txt",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();
        let rendered: Vec<String> = output.lines.iter().map(|l| l.to_string()).collect();

        // No gutter separator should be present in any content line.
        let has_gutter = rendered.iter().any(|l| l.contains('│'));
        assert!(
            !has_gutter,
            "expected no gutter separator when line numbers are disabled"
        );
    }

    #[test]
    fn expand_tabs_column_aware() {
        assert_eq!(expand_tabs("a\tb", 4), "a   b");
        assert_eq!(expand_tabs("\t\t", 4), "        ");
        assert_eq!(expand_tabs("ab\tc", 4), "ab  c");
        assert_eq!(expand_tabs("abcd\te", 4), "abcd    e");
        assert_eq!(expand_tabs("no tabs", 4), "no tabs");
        assert_eq!(expand_tabs("\t", 0), "\t");
    }

    #[test]
    fn tabs_expanded_to_spaces_in_diff() {
        let dir = setup_text_repo(
            "indent.rs",
            "fn main() {\n}\n",
            "fn main() {\n\t\tlet x = 1;\n}\n",
        );
        let cache = SyntaxCache::new();
        let output = diff_file(
            dir.path(),
            "indent.rs",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();
        let rendered: Vec<String> = output.lines.iter().map(|l| l.to_string()).collect();

        let insert_line = rendered
            .iter()
            .find(|l| l.contains("let x"))
            .expect("expected an inserted line containing 'let x'");
        // Two tabs at width 4 = 8 leading spaces.
        assert!(
            insert_line.contains("        let x"),
            "expected tabs expanded to 8 spaces, got: {insert_line}"
        );
        assert!(
            !insert_line.contains('\t'),
            "tab characters must not appear in rendered output"
        );
    }

    #[test]
    fn find_soft_break_returns_none_without_spaces() {
        let spans = vec![Span::raw("abcdefgh")];
        assert_eq!(find_soft_break(&spans, 5), None);
    }

    #[test]
    fn find_soft_break_returns_position_after_last_space() {
        let spans = vec![Span::raw("ab cd ef")];
        // max_col = 7 → chars 0..6: "ab cd e"
        // Spaces at col 2 and 5. Last space at col 5, so return 6.
        assert_eq!(find_soft_break(&spans, 7), Some(6));
    }

    #[test]
    fn find_soft_break_space_at_end_of_window() {
        let spans = vec![Span::raw("abcde fgh")];
        // max_col = 6 → chars 0..5: "abcde " — space at col 5, return 6.
        assert_eq!(find_soft_break(&spans, 6), Some(6));
    }

    #[test]
    fn find_soft_break_space_at_start() {
        let spans = vec![Span::raw(" abcdefgh")];
        // max_col = 5 → chars 0..4: " abcd" — space at col 0, return 1.
        assert_eq!(find_soft_break(&spans, 5), Some(1));
    }

    #[test]
    fn find_soft_break_across_multiple_spans() {
        let spans = vec![Span::raw("ab "), Span::raw("cd "), Span::raw("efgh")];
        // Combined: "ab cd efgh". max_col = 8 → chars 0..7: "ab cd ef"
        // Spaces at col 2, 5. Return 6.
        assert_eq!(find_soft_break(&spans, 8), Some(6));
    }

    #[test]
    fn find_soft_break_only_spaces() {
        let spans = vec![Span::raw("     ")];
        // max_col = 3 → chars 0..2: "   " — last space at col 2, return 3.
        assert_eq!(find_soft_break(&spans, 3), Some(3));
    }

    #[test]
    fn find_soft_break_space_just_outside_window() {
        // Space at col 5, but max_col = 5 means we only see cols 0..4.
        let spans = vec![Span::raw("abcde fgh")];
        assert_eq!(find_soft_break(&spans, 5), None);
    }

    #[test]
    fn find_soft_break_with_multibyte_chars() {
        // "a│b cd" — │ is 1 char. Positions: a=0, │=1, b=2, ' '=3, c=4, d=5
        let spans = vec![Span::raw("a│b cd")];
        assert_eq!(find_soft_break(&spans, 5), Some(4));
    }

    #[test]
    fn split_spans_at_boundary() {
        let spans = vec![Span::raw("abc"), Span::raw("defgh")];

        // Split at span boundary (col 3).
        let (left, right) = split_spans_at(&spans, 3);
        let left_text: String = left.iter().map(|s| s.content.as_ref()).collect();
        let right_text: String = right.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(left_text, "abc");
        assert_eq!(right_text, "defgh");
    }

    #[test]
    fn split_spans_at_mid_span() {
        let spans = vec![Span::raw("abcdefgh")];

        // Split in the middle (col 5).
        let (left, right) = split_spans_at(&spans, 5);
        let left_text: String = left.iter().map(|s| s.content.as_ref()).collect();
        let right_text: String = right.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(left_text, "abcde");
        assert_eq!(right_text, "fgh");
    }

    #[test]
    fn split_spans_preserves_style() {
        let style = Style::default().fg(Color::Red);
        let spans = vec![Span::styled("abcdef", style)];

        let (left, right) = split_spans_at(&spans, 3);
        assert_eq!(left[0].style, style);
        assert_eq!(right[0].style, style);
        assert_eq!(left[0].content.as_ref(), "abc");
        assert_eq!(right[0].content.as_ref(), "def");
    }

    #[test]
    fn split_spans_with_multibyte_chars() {
        // │ is U+2502 (3 bytes, 1 display column when counted by chars).
        let spans = vec![Span::raw("a│b")];

        let (left, right) = split_spans_at(&spans, 2);
        let left_text: String = left.iter().map(|s| s.content.as_ref()).collect();
        let right_text: String = right.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(left_text, "a│");
        assert_eq!(right_text, "b");
    }

    #[test]
    fn wrap_diff_lines_no_wrap_needed() {
        let lines = vec![Line::from(vec![
            Span::raw("123 "),
            Span::raw("│"),
            Span::raw(" content"),
        ])];
        // Total width is "123 │ content" = 14 chars. Width of 20 means no wrap.
        let wrapped = wrap_diff_lines(&lines, 20, 5);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].to_string(), "123 │ content");
    }

    #[test]
    fn wrap_diff_lines_wraps_with_gutter_indent() {
        // Gutter = "12 " (3 cols), content = "abcdefghij" (10 cols), total = 13
        let lines = vec![Line::from(vec![Span::raw("12 "), Span::raw("abcdefghij")])];
        // Total width 8, gutter 3 → content width 5 → "abcde" + "fghij"
        let wrapped = wrap_diff_lines(&lines, 8, 3);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].to_string(), "12 abcde");
        assert_eq!(wrapped[1].to_string(), "   fghij");
    }

    #[test]
    fn wrap_diff_lines_multiple_wraps() {
        // Gutter = "G" (1 col), content = "abcdefghijkl" (12 cols)
        let lines = vec![Line::from(vec![Span::raw("G"), Span::raw("abcdefghijkl")])];
        // Total width 5, gutter 1 → content width 4 → 3 visual lines
        let wrapped = wrap_diff_lines(&lines, 5, 1);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0].to_string(), "Gabcd");
        assert_eq!(wrapped[1].to_string(), " efgh");
        assert_eq!(wrapped[2].to_string(), " ijkl");
    }

    #[test]
    fn wrap_diff_lines_preserves_separator_on_continuation() {
        // Realistic gutter: "1 2 " (numbers) + "│" (sep) + "+content_that_is_long"
        let gutter_style = Style::default().fg(Color::Gray);
        let sep_style = Style::default().fg(Color::DarkGray);
        let lines = vec![Line::from(vec![
            Span::styled("1 2 ", gutter_style),
            Span::styled("│", sep_style),
            Span::raw("+abcdefghijklmno"),
        ])];
        // gutter_width = 6 (4 for numbers + 1 for │ + 1 for prefix)
        // total_width = 16, content_width = 10
        let wrapped = wrap_diff_lines(&lines, 16, 6);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].to_string(), "1 2 │+abcdefghij");
        // Continuation: blanked numbers, │ preserved, space for prefix
        assert_eq!(wrapped[1].to_string(), "    │ klmno");
        // Verify the │ span kept its style.
        let cont_spans = &wrapped[1].spans;
        let sep_span = cont_spans.iter().find(|s| s.content.contains('│')).unwrap();
        assert_eq!(sep_span.style, sep_style);
    }

    #[test]
    fn wrap_diff_lines_soft_wraps_at_spaces() {
        // Gutter = "G│" (2 cols), content = "+hello world foobar" (19 cols)
        let lines = vec![Line::from(vec![
            Span::raw("G"),
            Span::raw("│"),
            Span::raw("+hello world foobar"),
        ])];
        // total_width = 14, gutter = 2, content_width = 12
        // First 12 chars of content: "+hello world" — last space is at col 6,
        // so soft break after it (col 7): "+hello " on first line,
        // "world foobar" (12 chars, fits) on continuation.
        let wrapped = wrap_diff_lines(&lines, 14, 2);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].to_string(), "G│+hello ");
        assert_eq!(wrapped[1].to_string(), " │world foobar");
    }

    #[test]
    fn wrap_diff_lines_soft_wraps_with_multibyte() {
        // Content has multi-byte │ mixed with spaces.
        // Gutter = "G" (1 col), content = "a│b cdef ghij" (13 chars)
        let lines = vec![Line::from(vec![Span::raw("G"), Span::raw("a│b cdef ghij")])];
        // total_width = 8, gutter = 1, content_width = 7
        // First 7 chars: "a│b cde" — space at col 3, soft break → take 4
        // "a│b " on first line, remaining "cdef ghij" (9 chars)
        // Next 7 chars: "cdef gh" — space at col 4, soft break → take 5
        // "cdef " second line, remaining "ghij" (4 chars, fits)
        let wrapped = wrap_diff_lines(&lines, 8, 1);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0].to_string(), "Ga│b ");
        assert_eq!(wrapped[1].to_string(), " cdef ");
        assert_eq!(wrapped[2].to_string(), " ghij");
    }

    #[test]
    fn wrap_diff_lines_hard_breaks_without_spaces() {
        // No spaces in content → must hard-break.
        let lines = vec![Line::from(vec![Span::raw("G"), Span::raw("abcdefghijkl")])];
        let wrapped = wrap_diff_lines(&lines, 5, 1);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0].to_string(), "Gabcd");
        assert_eq!(wrapped[1].to_string(), " efgh");
        assert_eq!(wrapped[2].to_string(), " ijkl");
    }

    #[test]
    fn wrap_diff_lines_zero_gutter_is_noop() {
        let lines = vec![Line::from("a long line that should not be touched")];
        let wrapped = wrap_diff_lines(&lines, 10, 0);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].to_string(), lines[0].to_string());
    }

    #[test]
    fn gutter_width_matches_line_numbers() {
        let dir = setup_text_repo("gw.txt", "aaa\n", "aaa\nbbb\n");
        let cache = SyntaxCache::new();

        let with_ln = diff_file(
            dir.path(),
            "gw.txt",
            &AppTheme::default_dark(),
            &cache,
            true,
            4,
        )
        .unwrap();
        // Max line is 2, so ln_width = 1. gutter_width = 2*1 + 4 = 6.
        assert_eq!(with_ln.gutter_width, 6);

        let without_ln = diff_file(
            dir.path(),
            "gw.txt",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();
        assert_eq!(without_ln.gutter_width, 0);
    }

    #[test]
    fn the_size_verdict_takes_the_worse_of_bytes_and_lines() {
        let small = b"one\ntwo\n".as_slice();
        assert_eq!(diff_size_verdict(small, small), DiffSizeVerdict::Full);

        // Over the byte threshold on one side alone, with almost no lines.
        let wide = vec![b'x'; DIFF_PLAIN_ABOVE_BYTES + 1];
        assert_eq!(diff_size_verdict(&wide, b""), DiffSizeVerdict::Plain);

        // Over the line threshold, comfortably under the byte one.
        let tall = b"x\n".repeat(DIFF_PLAIN_ABOVE_LINES + 1);
        assert!(tall.len() < DIFF_PLAIN_ABOVE_BYTES);
        assert_eq!(diff_size_verdict(&tall, b""), DiffSizeVerdict::Plain);

        // The two sides are summed, so neither alone has to cross the line.
        let half = b"x\n".repeat(DIFF_PLAIN_ABOVE_LINES / 2 + 1);
        assert_eq!(diff_size_verdict(&half, &half), DiffSizeVerdict::Plain);

        let huge = b"x\n".repeat(DIFF_REFUSE_ABOVE_LINES + 1);
        assert_eq!(diff_size_verdict(&huge, b""), DiffSizeVerdict::TooLarge);
    }

    #[test]
    fn a_last_line_with_no_newline_still_counts() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"one\n"), 1);
        assert_eq!(count_lines(b"one"), 1);
        assert_eq!(count_lines(b"one\ntwo"), 2);
    }

    /// Below both thresholds the diff is highlighted, so a line of Rust comes
    /// back as several differently-styled spans rather than one.
    #[test]
    fn a_small_file_is_still_syntax_highlighted() {
        let dir = setup_text_repo("small.rs", "fn main() {}\n", "fn main() { let x = 1; }\n");
        let cache = SyntaxCache::new();

        let output = diff_file(
            dir.path(),
            "small.rs",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();

        let inserted = output
            .lines
            .iter()
            .find(|line| line.to_string().starts_with('+') && line.to_string().contains("let"))
            .expect("the added line");
        assert!(
            inserted.spans.len() > 2,
            "a highlighted line of Rust is split into token spans, got {:?}",
            inserted.spans
        );
    }

    /// Past the line threshold the token colors go away and the added/removed
    /// coloring stays: the line is one span carrying its `+` prefix.
    #[test]
    fn a_file_past_the_line_threshold_is_diffed_without_highlighting() {
        let base: String = "let value = 1;\n".repeat(DIFF_PLAIN_ABOVE_LINES / 2 + 10);
        let changed = format!("{base}let extra = 2;\n");
        assert!(
            base.len() + changed.len() < DIFF_PLAIN_ABOVE_BYTES,
            "this fixture must cross the LINE threshold and no other"
        );
        let dir = setup_text_repo("big.rs", &base, &changed);
        let cache = SyntaxCache::new();

        let output = diff_file(
            dir.path(),
            "big.rs",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();

        let inserted = output
            .lines
            .iter()
            .find(|line| line.to_string().starts_with("+let extra"))
            .expect("the added line");
        assert_eq!(
            inserted.spans.len(),
            1,
            "an unhighlighted line is one span, got {:?}",
            inserted.spans
        );
        assert_eq!(inserted.spans[0].content, "+let extra = 2;");
        assert_eq!(
            inserted.spans[0].style.fg,
            Some(AppTheme::default_dark().diff_add),
            "the added coloring survives losing the syntax highlighting"
        );
    }

    /// Past the hard cap dux says so in the pane rather than holding a worker
    /// thread for minutes on a diff nobody can read anyway.
    #[test]
    fn a_file_past_the_hard_cap_is_refused_in_words() {
        let base: String = "x\n".repeat(DIFF_REFUSE_ABOVE_LINES + 1);
        let changed = format!("{base}y\n");
        let dir = setup_text_repo("huge.txt", &base, &changed);
        let cache = SyntaxCache::new();

        let output = diff_file(
            dir.path(),
            "huge.txt",
            &AppTheme::default_dark(),
            &cache,
            false,
            4,
        )
        .unwrap();

        let rendered: Vec<String> = output.lines.iter().map(|line| line.to_string()).collect();
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("File too large to diff.")),
            "got {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("Old size:")),
            "the refusal names the sizes it judged, got {rendered:?}"
        );
        assert_eq!(output.gutter_width, 0);
    }

    /// The worker path is the only way in from the app, so it has to produce
    /// exactly what the direct call does, tagged with the request that asked.
    #[test]
    fn the_worker_posts_the_answer_back_tagged_with_its_request() {
        let dir = setup_text_repo("worker.txt", "aaa\n", "aaa\nbbb\n");
        let key = DiffRequestKey {
            worktree_path: dir.path().to_string_lossy().to_string(),
            rel_path: "worker.txt".to_string(),
            show_line_numbers: true,
            tab_width: 4,
            seq: 7,
        };
        let (tx, rx) = std::sync::mpsc::channel();

        spawn_diff_job(
            key.clone(),
            AppTheme::default_dark(),
            std::sync::Arc::new(SyntaxCache::new()),
            tx,
        );

        let answer = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the worker answers");
        assert_eq!(answer.key, key);
        let output = answer.result.expect("a diff");
        assert_eq!(output.gutter_width, 6);
        assert!(
            output
                .lines
                .iter()
                .any(|line| line.to_string().contains("+bbb"))
        );
    }
}
