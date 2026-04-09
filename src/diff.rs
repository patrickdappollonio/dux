use std::path::Path;

use anyhow::Result;
use content_inspector::{ContentType, inspect};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
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
}

/// Compute a syntax-highlighted, unified diff for a single file.
///
/// `worktree_path` is the root of the git worktree and `rel_path` is the
/// file path relative to it (as reported by `git status --porcelain`).
pub fn diff_file(
    worktree_path: &Path,
    rel_path: &str,
    theme: &AppTheme,
    cache: &SyntaxCache,
    show_line_numbers: bool,
) -> Result<DiffOutput> {
    let old_bytes = crate::git::file_bytes_at_head(worktree_path, rel_path)?.unwrap_or_default();
    let abs_path = worktree_path.join(rel_path);
    let new_bytes = std::fs::read(&abs_path).unwrap_or_default();

    if old_bytes == new_bytes {
        return Ok(DiffOutput {
            lines: vec![Line::from("No changes.")],
        });
    }

    if !is_renderable_text(&old_bytes) || !is_renderable_text(&new_bytes) {
        return Ok(binary_diff_output(
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

    // Compute gutter width from the maximum line number across all hunks.
    let ln_width = if show_line_numbers {
        let mut max_ln: usize = 1;
        for hunk in text_diff.unified_diff().context_radius(3).iter_hunks() {
            for change in hunk.iter_changes() {
                if let Some(idx) = change.old_index() {
                    max_ln = max_ln.max(idx + 1);
                }
                if let Some(idx) = change.new_index() {
                    max_ln = max_ln.max(idx + 1);
                }
            }
        }
        max_ln.to_string().len()
    } else {
        0
    };

    let gutter_style = Style::default().fg(theme.diff_line_number_fg);
    let sep_style = Style::default().fg(theme.diff_line_number_sep);

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
            let tag = change.tag();
            let text = change.value();

            let (prefix, base_fg, bg, highlighter) = match tag {
                ChangeTag::Delete => (
                    "-",
                    theme.diff_remove,
                    Some(theme.diff_remove_bg),
                    &mut hl_old,
                ),
                ChangeTag::Insert => ("+", theme.diff_add, Some(theme.diff_add_bg), &mut hl_new),
                ChangeTag::Equal => (" ", Color::Reset, None, &mut hl_new),
            };

            // Attempt syntax highlighting; fall back to plain coloring.
            let content = text.trim_end_matches('\n');
            let mut spans: Vec<Span<'static>> = Vec::new();

            // Line-number gutter.
            if show_line_numbers {
                let old_ln = match tag {
                    ChangeTag::Delete | ChangeTag::Equal => {
                        format!("{:>w$}", change.old_index().unwrap_or(0) + 1, w = ln_width)
                    }
                    ChangeTag::Insert => " ".repeat(ln_width),
                };
                let new_ln = match tag {
                    ChangeTag::Insert | ChangeTag::Equal => {
                        format!("{:>w$}", change.new_index().unwrap_or(0) + 1, w = ln_width)
                    }
                    ChangeTag::Delete => " ".repeat(ln_width),
                };
                spans.push(Span::styled(format!("{old_ln} {new_ln} "), gutter_style));
                spans.push(Span::styled("│", sep_style));
            }

            match highlighter.highlight_line(content, &cache.syntax_set) {
                Ok(ranges) if tag == ChangeTag::Equal => {
                    // Context lines: full syntax colors, no background tint.
                    spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default().fg(base_fg),
                    ));
                    spans.extend(
                        ranges
                            .into_iter()
                            .map(|(s, t)| Span::styled(t.to_string(), syntect_to_ratatui(s))),
                    );
                }
                Ok(ranges) => {
                    // Added/removed lines: syntax colors + tinted background.
                    spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default().fg(base_fg).bg(bg.unwrap_or(Color::Reset)),
                    ));
                    spans.extend(ranges.into_iter().map(|(s, t)| {
                        let mut style = syntect_to_ratatui(s);
                        if let Some(bg_color) = bg {
                            style = style.bg(bg_color);
                        }
                        Span::styled(t.to_string(), style)
                    }));
                }
                Err(_) => {
                    // Fallback: no syntax highlighting.
                    spans.push(Span::styled(
                        format!("{prefix}{content}"),
                        Style::default().fg(base_fg).bg(bg.unwrap_or(Color::Reset)),
                    ));
                }
            };
            lines.push(Line::from(spans));
        }
    }

    if lines.len() <= 2 {
        // Only headers, no actual hunks (e.g. binary file or mode change).
        lines.push(Line::from("No text diff available."));
    }

    Ok(DiffOutput { lines })
}

fn is_renderable_text(bytes: &[u8]) -> bool {
    bytes.is_empty() || matches!(inspect(bytes), ContentType::UTF_8)
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
        let output =
            diff_file(repo, "image.bin", &AppTheme::default_dark(), &cache, false).unwrap();
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
    fn line_numbers_appear_when_enabled() {
        let dir = setup_text_repo("hello.txt", "aaa\nbbb\nccc\n", "aaa\nbbb\nXXX\nccc\n");
        let cache = SyntaxCache::new();
        let output = diff_file(
            dir.path(),
            "hello.txt",
            &AppTheme::default_dark(),
            &cache,
            true,
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
}
