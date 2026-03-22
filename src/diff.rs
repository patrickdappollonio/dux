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

/// Compute a syntax-highlighted, unified diff for a single file.
///
/// `worktree_path` is the root of the git worktree and `rel_path` is the
/// file path relative to it (as reported by `git status --porcelain`).
pub fn diff_file(worktree_path: &Path, rel_path: &str, theme: &AppTheme) -> Result<DiffOutput> {
    let old_text = crate::git::file_at_head(worktree_path, rel_path)?.unwrap_or_default();
    let abs_path = worktree_path.join(rel_path);
    let new_text = fs::read_to_string(&abs_path).unwrap_or_default();

    if old_text == new_text {
        return Ok(DiffOutput {
            lines: vec![Line::from("No changes.")],
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
        lines.push(Line::from(Span::styled(
            hunk.header().to_string(),
            Style::default().fg(theme.diff_hunk),
        )));

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
                    Some(Color::Rgb(60, 20, 20)),
                    &mut hl_old,
                ),
                ChangeTag::Insert => (
                    "+",
                    theme.diff_add,
                    Some(Color::Rgb(20, 50, 20)),
                    &mut hl_new,
                ),
                ChangeTag::Equal => (" ", Color::Reset, None, &mut hl_new),
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
            lines.push(Line::from(spans));
        }
    }

    if lines.len() <= 2 {
        // Only headers, no actual hunks (e.g. binary file or mode change).
        lines.push(Line::from("No text diff available."));
    }

    Ok(DiffOutput { lines })
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
