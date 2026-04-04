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

/// Pre-rendered diff ready for display.
pub struct DiffOutput {
    pub lines: Vec<Line<'static>>,
}

/// Compute a syntax-highlighted, unified diff for a single file.
///
/// `worktree_path` is the root of the git worktree and `rel_path` is the
/// file path relative to it (as reported by `git status --porcelain`).
pub fn diff_file(worktree_path: &Path, rel_path: &str, theme: &AppTheme) -> Result<DiffOutput> {
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
                    Some(theme.diff_remove_bg),
                    &mut hl_old,
                ),
                ChangeTag::Insert => ("+", theme.diff_add, Some(theme.diff_add_bg), &mut hl_new),
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

        let output = diff_file(repo, "image.bin", &AppTheme::default_dark()).unwrap();
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
}
