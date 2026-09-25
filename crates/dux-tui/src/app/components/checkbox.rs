use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use dux_core::prose::Prose;

use super::name_chip::prose_lines;
use super::wrap_lines::wrap_styled_lines;
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckboxState {
    Normal,
    Focused,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckboxLayout {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) height: u16,
    background: Option<Color>,
}

impl CheckboxLayout {
    pub(crate) fn empty() -> Self {
        Self {
            lines: Vec::new(),
            height: 0,
            background: None,
        }
    }

    pub(crate) fn background(mut self, background: Color) -> Self {
        self.background = Some(background);
        self
    }
}

/// What a checkbox says: plain words, or a sentence that names something and
/// draws each name as the shared chip.
#[derive(Clone, Copy)]
enum CheckboxLabel<'a> {
    Plain(&'a str),
    Prose(&'a Prose, &'a Theme),
}

#[derive(Clone)]
pub(crate) struct Checkbox<'a> {
    label: CheckboxLabel<'a>,
    checked: bool,
    state: CheckboxState,
}

impl<'a> Checkbox<'a> {
    const PREFIX: &'static str = " ";
    const GAP: &'static str = " ";
    const INDENT: &'static str = "     ";

    pub(crate) const fn indent() -> &'static str {
        Self::INDENT
    }

    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label: CheckboxLabel::Plain(label),
            checked: false,
            state: CheckboxState::Normal,
        }
    }

    /// A checkbox whose label names something: every name in `prose` renders
    /// as the name chip, the words around it in the label style.
    pub(crate) fn with_prose(prose: &'a Prose, theme: &'a Theme) -> Self {
        Self {
            label: CheckboxLabel::Prose(prose, theme),
            checked: false,
            state: CheckboxState::Normal,
        }
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub(crate) fn state(mut self, state: CheckboxState) -> Self {
        self.state = state;
        self
    }

    pub(crate) fn layout(
        &self,
        max_width: u16,
        marker_style: Style,
        label_style: Style,
    ) -> CheckboxLayout {
        if max_width == 0 {
            return CheckboxLayout::empty();
        }

        let indent_width = Self::INDENT.chars().count();
        let available = usize::from(max_width);
        let label_width = available.saturating_sub(indent_width).max(1);
        let rows: Vec<Vec<Span<'static>>> = match self.label {
            CheckboxLabel::Plain(label) => wrap_checkbox_label(label, label_width)
                .into_iter()
                .map(|text| vec![Span::styled(text, label_style)])
                .collect(),
            CheckboxLabel::Prose(prose, theme) => {
                let sentence = prose_lines(prose, "", label_style, theme);
                let wrapped = wrap_styled_lines(&sentence, label_width);
                if wrapped.is_empty() {
                    vec![Vec::new()]
                } else {
                    wrapped.into_iter().map(|line| line.spans).collect()
                }
            }
        };
        let marker = if self.checked { "[x]" } else { "[ ]" };
        let mut lines = Vec::with_capacity(rows.len().max(1));

        for (index, row) in rows.into_iter().enumerate() {
            let mut spans = if index == 0 {
                vec![
                    Span::raw(Self::PREFIX),
                    Span::styled(marker.to_string(), marker_style),
                    Span::raw(Self::GAP),
                ]
            } else {
                vec![Span::raw(Self::INDENT)]
            };
            spans.extend(row);
            lines.push(Line::from(spans));
        }

        CheckboxLayout {
            height: lines.len() as u16,
            lines,
            background: None,
        }
    }

    pub(crate) fn inline_prefix(&self, marker_style: Style) -> Vec<Span<'static>> {
        vec![
            Span::raw(Self::PREFIX),
            Span::styled(
                if self.checked { "[x]" } else { "[ ]" }.to_string(),
                self.marker_style(marker_style),
            ),
            Span::raw(Self::GAP),
        ]
    }

    pub(crate) fn marker_style(&self, base_marker_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_marker_style,
            CheckboxState::Focused => base_marker_style.add_modifier(Modifier::BOLD),
        }
    }

    pub(crate) fn label_style(&self, base_label_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_label_style,
            CheckboxState::Focused => base_label_style.add_modifier(Modifier::BOLD),
        }
    }
}

impl Widget for CheckboxLayout {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Never index past the buffer: a caller on a tiny screen can hand in a
        // rect that reaches beyond it.
        let area = area.intersection(buf.area);
        let clear_style = self
            .background
            .map(|background| Style::default().bg(background))
            .unwrap_or_default();
        for clear_offset in 0..area.height {
            let y = area.y.saturating_add(clear_offset);
            for x_offset in 0..area.width {
                let cell = &mut buf[(area.x.saturating_add(x_offset), y)];
                cell.reset();
                cell.set_style(clear_style);
            }
        }

        for (offset, line) in self.lines.into_iter().enumerate() {
            let y = area.y.saturating_add(offset as u16);
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            line.render(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }
}

fn wrap_checkbox_label(label: &str, max_width: usize) -> Vec<String> {
    if label.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for paragraph in label.split('\n') {
        let mut current = String::new();
        let mut current_width = 0usize;
        for word in paragraph.split_whitespace() {
            let word_width = word.chars().count();
            if current.is_empty() {
                if word_width <= max_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_broken_word_lines(word, max_width, &mut lines);
                }
                continue;
            }

            let next_width = current_width + 1 + word_width;
            if next_width <= max_width {
                current.push(' ');
                current.push_str(word);
                current_width = next_width;
            } else {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
                if word_width <= max_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_broken_word_lines(word, max_width, &mut lines);
                }
            }
        }

        if !current.is_empty() {
            lines.push(current);
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_broken_word_lines(word: &str, max_width: usize, lines: &mut Vec<String>) {
    let mut chunk = String::new();
    let mut chunk_width = 0usize;
    for ch in word.chars() {
        if chunk_width == max_width {
            lines.push(std::mem::take(&mut chunk));
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += 1;
    }
    if !chunk.is_empty() {
        lines.push(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkbox_wraps_label_with_continuation_indent() {
        let checkbox = Checkbox::new("Also delete the worktree and branch")
            .checked(false)
            .state(CheckboxState::Normal);

        let layout = checkbox.layout(20, Style::default(), Style::default());

        assert_eq!(layout.height, 3);
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.lines[0].spans[0].content.as_ref(), " ");
        assert_eq!(layout.lines[0].spans[1].content.as_ref(), "[ ]");
        assert_eq!(layout.lines[1].spans[0].content.as_ref(), "     ");
        assert_eq!(layout.lines[2].spans[0].content.as_ref(), "     ");
    }

    #[test]
    fn checkbox_focus_boldens_label_and_marker() {
        let marker = Style::default();
        let label = Style::default();
        let focused = Checkbox::new("Label").state(CheckboxState::Focused);

        assert!(
            focused
                .marker_style(marker)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            focused
                .label_style(label)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn checkbox_inline_prefix_uses_shared_ascii_marker() {
        let unchecked = Checkbox::new("")
            .checked(false)
            .state(CheckboxState::Normal);
        let checked = Checkbox::new("").checked(true).state(CheckboxState::Normal);

        let unchecked_spans = unchecked.inline_prefix(Style::default());
        let checked_spans = checked.inline_prefix(Style::default());

        assert_eq!(unchecked_spans[1].content.as_ref(), "[ ]");
        assert_eq!(checked_spans[1].content.as_ref(), "[x]");
    }

    /// A label that names something keeps the name as a chip on the row it
    /// lands on, with the label's own style on the words around it, and the
    /// marker and indent geometry of a plain label.
    #[test]
    fn a_prose_label_draws_its_names_as_chips_and_wraps_with_the_same_indent() {
        let theme = crate::theme::Theme::default_dark();
        let prose = dux_core::prose::Prose::new()
            .text("Also delete the branch ")
            .name("feat/login");
        let label_style = Style::default().fg(Color::Yellow);
        let layout = Checkbox::with_prose(&prose, &theme).checked(true).layout(
            24,
            Style::default(),
            label_style,
        );

        let rows: Vec<String> = layout
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            rows,
            vec![" [x] Also delete the", "     branch  feat/login "]
        );
        assert_eq!(layout.height, 2);
        let chip = layout.lines[1]
            .spans
            .iter()
            .find(|span| span.content.contains("feat/login"))
            .expect("the name is on the second row");
        assert_eq!(chip.style, theme.name_style());
        assert_eq!(layout.lines[0].spans[3].style, label_style);
    }

    #[test]
    fn checkbox_indent_width_matches_indent_text() {
        assert_eq!(Checkbox::indent().chars().count(), 5);
    }

    #[test]
    fn checkbox_render_preserves_configured_background() {
        let layout = Checkbox::new("Label")
            .layout(
                12,
                Style::default().fg(Color::Yellow),
                Style::default().fg(Color::White),
            )
            .background(Color::Rgb(12, 34, 56));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 1));

        layout.render(buffer.area, &mut buffer);

        for x in 0..buffer.area.width {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(12, 34, 56));
        }
    }
}
