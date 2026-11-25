//! The name chip: how a variable inside a dialog's prose looks.
//!
//! Every branch, path, file, command, and agent, project, terminal or provider
//! name a dialog sentence carries renders as one span in the dialog body's own
//! colors swapped (the modal surface `overlay_bg` on the body text `text_fg`),
//! padded by one cell on each side, with no bold and no quotes: the chip is the
//! delimiter. Swapping the body's pair means a chip reads exactly as well as
//! the sentence around it, in every theme, with nothing to configure. A padded
//! chip is exactly as wide as the quoted text it replaces, so a sentence that
//! used to quote its names keeps its width. It is the terminal counterpart of
//! the web's inline code chip. The prose around a chip is painted in `text_fg`
//! on the modal surface, never the host terminal's default foreground.
//!
//! Sentences are built as [`Prose`] (constant words and names), including the
//! ones dux-core shares with the web, and turned into spans only here. A call
//! site never styles a name by hand.

use dux_core::prose::{Prose, ProseSegment};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// One name as a chip.
///
/// The padding is an ordinary space, so a name copied out of the host
/// terminal carries nothing invisible. What keeps the chip whole when a body
/// wraps is [`super::wrap_styled_lines`], which treats a span carrying the chip
/// marker ([`crate::theme::NAME_CHIP_MARKER`]) as one unbreakable unit, inner
/// spaces and pads included. A chip must therefore reach the screen through
/// that wrapper: ratatui's own `Wrap` would break at a pad or between the words
/// of "My Cool Project".
pub(crate) fn name_chip(name: &str, theme: &Theme) -> Span<'static> {
    Span::styled(format!(" {name} "), theme.name_style())
}

/// A sentence as spans: constant words in `text_style`, every name as a chip.
/// A name built quoted loses its quotes, because the chip replaces them.
pub(crate) fn prose_spans(prose: &Prose, text_style: Style, theme: &Theme) -> Vec<Span<'static>> {
    prose
        .segments()
        .iter()
        .map(|segment| match segment {
            ProseSegment::Text(text) => Span::styled(text.clone(), text_style),
            ProseSegment::Name { name, .. } => name_chip(name, theme),
        })
        .collect()
}

/// An info row that names one thing: `label` in `label_style`, then the name
/// as a chip (" Agent:  feat/login ").
pub(crate) fn labelled_name(
    label: &str,
    name: &str,
    label_style: Style,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), label_style),
        name_chip(name, theme),
    ])
}

/// A sentence as lines, split at every line break in its words, each line
/// starting with `indent` in `text_style` (the dialogs' one-cell body margin).
pub(crate) fn prose_lines(
    prose: &Prose,
    indent: &str,
    text_style: Style,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let start = || vec![Span::styled(indent.to_string(), text_style)];
    let mut lines = Vec::new();
    let mut current = start();
    for segment in prose.segments() {
        match segment {
            ProseSegment::Text(text) => {
                for (index, piece) in text.split('\n').enumerate() {
                    if index > 0 {
                        lines.push(Line::from(std::mem::replace(&mut current, start())));
                    }
                    if !piece.is_empty() {
                        current.push(Span::styled(piece.to_string(), text_style));
                    }
                }
            }
            ProseSegment::Name { name, .. } => current.push(name_chip(name, theme)),
        }
    }
    lines.push(Line::from(current));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn theme() -> Theme {
        let mut theme = Theme::default_dark();
        theme.text_fg = Color::Rgb(4, 5, 6);
        theme.overlay_bg = Color::Rgb(1, 2, 3);
        theme
    }

    #[test]
    fn a_name_is_padded_by_one_cell_in_the_dialog_colors_swapped_and_never_bold() {
        let chip = name_chip("feat/login", &theme());
        assert_eq!(
            chip.content, " feat/login ",
            "the pads are ordinary spaces, so a copied name carries nothing invisible"
        );
        // The dialog surface's color on the dialog body text's color.
        assert_eq!(chip.style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(chip.style.bg, Some(Color::Rgb(4, 5, 6)));
        assert!(crate::theme::is_name_chip(chip.style));
        assert!(!chip.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_padded_chip_is_as_wide_as_the_quoted_name_it_replaces() {
        for name in ["main", "feat/ünïcode", "日本語"] {
            let quoted = Span::raw(format!("\"{name}\""));
            assert_eq!(name_chip(name, &theme()).width(), quoted.width(), "{name}");
        }
    }

    #[test]
    fn prose_becomes_words_in_the_text_style_and_names_as_chips_without_quotes() {
        let text = Style::default().fg(Color::Yellow);
        let prose = Prose::new()
            .text("Delete ")
            .quoted("feat/login")
            .text(" in ")
            .name("~/src/dux")
            .text("?");
        let spans = prose_spans(&prose, text, &theme());
        let contents: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            contents,
            vec!["Delete ", " feat/login ", " in ", " ~/src/dux ", "?"]
        );
        assert_eq!(spans[0].style, text);
        assert_eq!(spans[1].style, theme().name_style());
        assert_eq!(spans[3].style, theme().name_style());
        let rendered: String = contents.concat();
        assert!(!rendered.contains('"'), "{rendered}");
    }

    #[test]
    fn an_info_row_is_its_label_then_the_name_as_a_chip() {
        let label = Style::default().fg(Color::Gray);
        let line = labelled_name(" Agent: ", "feat/login", label, &theme());
        let contents: Vec<&str> = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(contents, vec![" Agent: ", " feat/login "]);
        assert_eq!(line.spans[0].style, label);
        assert_eq!(line.spans[1].style, theme().name_style());
    }

    #[test]
    fn prose_lines_break_at_line_breaks_and_indent_every_line() {
        let prose = Prose::new()
            .text("Recreate at ")
            .name("~/wt")
            .text("?\n\nIf branch ")
            .quoted("main")
            .text(" exists.");
        let lines = prose_lines(&prose, " ", Style::default(), &theme());
        let texts: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![" Recreate at  ~/wt ?", " ", " If branch  main  exists."]
        );
        assert_eq!(lines[2].spans[2].style, theme().name_style());
    }
}
