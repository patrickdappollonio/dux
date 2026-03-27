use std::fs;
use std::path::Path;

use anyhow::Result;
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SynColor, FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::theme::Theme as AppTheme;

/// Pre-rendered diff ready for display.
pub struct DiffOutput {
    pub lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiffRenderOptions {
    pub show_line_numbers: bool,
}

/// Compute a syntax-highlighted, unified diff for a single file.
///
/// `worktree_path` is the root of the git worktree and `rel_path` is the
/// file path relative to it (as reported by `git status --porcelain`).
pub fn diff_file(
    worktree_path: &Path,
    rel_path: &str,
    theme: &AppTheme,
    options: DiffRenderOptions,
) -> Result<DiffOutput> {
    let old_text = crate::git::file_at_head(worktree_path, rel_path)?.unwrap_or_default();
    let abs_path = worktree_path.join(rel_path);
    let new_text = fs::read_to_string(&abs_path).unwrap_or_default();
    let line_number_width = if options.show_line_numbers {
        max_line_number_width(&old_text, &new_text)
    } else {
        0
    };

    if old_text == new_text {
        return Ok(DiffOutput {
            lines: vec![Line::from(with_gutter(
                vec![Span::raw("No changes.")],
                None,
                None,
                line_number_width,
                theme,
                None,
            ))],
        });
    }

    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let syn_theme = &theme_set.themes["base16-ocean.dark"];

    let syntax = syntax_set
        .find_syntax_by_extension(
            Path::new(rel_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(""),
        )
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let text_diff = TextDiff::from_lines(&old_text, &new_text);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // File header.
    lines.push(Line::from(with_gutter(
        vec![Span::styled(
            format!("--- a/{rel_path}"),
            Style::default()
                .fg(theme.diff_file_header)
                .add_modifier(Modifier::BOLD),
        )],
        None,
        None,
        line_number_width,
        theme,
        None,
    )));
    lines.push(Line::from(with_gutter(
        vec![Span::styled(
            format!("+++ b/{rel_path}"),
            Style::default()
                .fg(theme.diff_file_header)
                .add_modifier(Modifier::BOLD),
        )],
        None,
        None,
        line_number_width,
        theme,
        None,
    )));

    for hunk in text_diff.unified_diff().context_radius(3).iter_hunks() {
        // Hunk header (@@ ... @@).
        lines.push(Line::from(with_gutter(
            vec![Span::styled(
                hunk.header().to_string(),
                Style::default().fg(theme.diff_hunk),
            )],
            None,
            None,
            line_number_width,
            theme,
            None,
        )));

        // We maintain two separate highlighters so that removed lines are
        // highlighted in the context of the old file and added/context lines
        // in the context of the new file. This avoids broken highlighting
        // when a change spans a multi-line construct.
        let mut hl_old = HighlightLines::new(syntax, syn_theme);
        let mut hl_new = HighlightLines::new(syntax, syn_theme);
        let mut old_line = hunk
            .ops()
            .first()
            .map(|op| op.old_range().start + 1)
            .unwrap_or(1);
        let mut new_line = hunk
            .ops()
            .first()
            .map(|op| op.new_range().start + 1)
            .unwrap_or(1);

        for change in hunk.iter_changes() {
            let tag = change.tag();
            let text = change.value();

            let line_count = count_diff_lines(text);
            let (prefix, base_fg, bg, highlighter, old_num, new_num) = match tag {
                ChangeTag::Delete => {
                    let current = old_line;
                    old_line += line_count;
                    (
                        "-",
                        theme.diff_remove,
                        Some(Color::Rgb(60, 20, 20)),
                        &mut hl_old,
                        Some(current),
                        None,
                    )
                }
                ChangeTag::Insert => {
                    let current = new_line;
                    new_line += line_count;
                    (
                        "+",
                        theme.diff_add,
                        Some(Color::Rgb(20, 50, 20)),
                        &mut hl_new,
                        None,
                        Some(current),
                    )
                }
                ChangeTag::Equal => {
                    let old_current = old_line;
                    let new_current = new_line;
                    old_line += line_count;
                    new_line += line_count;
                    (
                        " ",
                        Color::Reset,
                        None,
                        &mut hl_new,
                        Some(old_current),
                        Some(new_current),
                    )
                }
            };

            // Attempt syntax highlighting; fall back to plain coloring.
            let content = text.trim_end_matches('\n');
            let spans = match highlighter.highlight_line(content, &syntax_set) {
                Ok(ranges) if tag == ChangeTag::Equal => {
                    // Context lines: full syntax colors, no background tint.
                    let mut out = vec![Span::styled(
                        prefix.to_string(),
                        Style::default().fg(base_fg),
                    )];
                    out.extend(
                        ranges
                            .into_iter()
                            .map(|(s, t)| Span::styled(t.to_string(), syntect_to_ratatui(s))),
                    );
                    out
                }
                Ok(ranges) => {
                    // Added/removed lines: syntax colors + tinted background.
                    let mut out = vec![Span::styled(
                        prefix.to_string(),
                        Style::default().fg(base_fg).bg(bg.unwrap_or(Color::Reset)),
                    )];
                    out.extend(ranges.into_iter().map(|(s, t)| {
                        let mut style = syntect_to_ratatui(s);
                        if let Some(bg_color) = bg {
                            style = style.bg(bg_color);
                        }
                        Span::styled(t.to_string(), style)
                    }));
                    out
                }
                Err(_) => {
                    // Fallback: no syntax highlighting.
                    vec![Span::styled(
                        format!("{prefix}{content}"),
                        Style::default().fg(base_fg).bg(bg.unwrap_or(Color::Reset)),
                    )]
                }
            };
            lines.push(Line::from(with_gutter(
                spans,
                old_num,
                new_num,
                line_number_width,
                theme,
                bg,
            )));
        }
    }

    if lines.len() <= 2 {
        // Only headers, no actual hunks (e.g. binary file or mode change).
        lines.push(Line::from(with_gutter(
            vec![Span::raw("No text diff available.")],
            None,
            None,
            line_number_width,
            theme,
            None,
        )));
    }

    Ok(DiffOutput { lines })
}

fn with_gutter(
    mut content: Vec<Span<'static>>,
    old_num: Option<usize>,
    new_num: Option<usize>,
    width: usize,
    theme: &AppTheme,
    bg: Option<Color>,
) -> Vec<Span<'static>> {
    if width == 0 {
        return content;
    }

    let gutter_style = Style::default()
        .fg(theme.diff_line_number)
        .bg(bg.unwrap_or(Color::Reset));
    let old_text = old_num.map(|n| n.to_string()).unwrap_or_default();
    let new_text = new_num.map(|n| n.to_string()).unwrap_or_default();

    let mut spans = vec![Span::styled(
        format!("{old_text:>width$} {new_text:>width$} │ "),
        gutter_style,
    )];
    spans.append(&mut content);
    spans
}

fn max_line_number_width(old_text: &str, new_text: &str) -> usize {
    old_text
        .lines()
        .count()
        .max(new_text.lines().count())
        .max(1)
        .to_string()
        .len()
}

fn count_diff_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        let newline_count = text.chars().filter(|&ch| ch == '\n').count();
        if text.ends_with('\n') {
            newline_count.max(1)
        } else {
            newline_count + 1
        }
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

    #[test]
    fn gutter_formats_old_and_new_numbers() {
        let spans = with_gutter(
            vec![Span::raw("+fn main() {}")],
            Some(12),
            Some(18),
            3,
            &AppTheme::default_dark(),
            None,
        );
        assert_eq!(spans[0].content.as_ref(), " 12  18 │ ");
    }

    #[test]
    fn gutter_hides_numbers_when_disabled() {
        let spans = with_gutter(
            vec![Span::raw("context")],
            Some(3),
            Some(3),
            0,
            &AppTheme::default_dark(),
            None,
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "context");
    }

    #[test]
    fn count_diff_lines_handles_missing_trailing_newline() {
        assert_eq!(count_diff_lines("let x = 1;"), 1);
        assert_eq!(count_diff_lines("a\nb\n"), 2);
        assert_eq!(count_diff_lines("a\nb"), 2);
        assert_eq!(count_diff_lines(""), 0);
    }
}
