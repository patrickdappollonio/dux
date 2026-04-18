use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CheckboxState {
    Normal,
    Hovered,
    Focused,
    Disabled,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckboxLayout {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) height: u16,
}

impl CheckboxLayout {
    pub(crate) fn empty() -> Self {
        Self {
            lines: Vec::new(),
            height: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Checkbox<'a> {
    label: &'a str,
    checked: bool,
    state: CheckboxState,
}

impl<'a> Checkbox<'a> {
    const PREFIX: &'static str = " ";
    const GAP: &'static str = " ";
    const INDENT: &'static str = "     ";

    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label,
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
        let wrapped = wrap_checkbox_label(self.label, label_width);
        let marker = if self.checked { "[x]" } else { "[ ]" };
        let mut lines = Vec::with_capacity(wrapped.len().max(1));

        for (index, text) in wrapped.into_iter().enumerate() {
            if index == 0 {
                lines.push(Line::from(vec![
                    Span::raw(Self::PREFIX),
                    Span::styled(marker.to_string(), marker_style),
                    Span::raw(Self::GAP),
                    Span::styled(text, label_style),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw(Self::INDENT),
                    Span::styled(text, label_style),
                ]));
            }
        }

        CheckboxLayout {
            height: lines.len() as u16,
            lines,
        }
    }

    pub(crate) fn marker_style(&self, base_marker_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_marker_style,
            CheckboxState::Hovered => base_marker_style.add_modifier(Modifier::BOLD),
            CheckboxState::Focused => base_marker_style.add_modifier(Modifier::BOLD),
            CheckboxState::Disabled => base_marker_style,
        }
    }

    pub(crate) fn label_style(&self, base_label_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_label_style,
            CheckboxState::Hovered => base_label_style.add_modifier(Modifier::BOLD),
            CheckboxState::Focused => base_label_style.add_modifier(Modifier::BOLD),
            CheckboxState::Disabled => base_label_style,
        }
    }
}

impl Widget for CheckboxLayout {
    fn render(self, area: Rect, buf: &mut Buffer) {
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
    fn checkbox_hover_and_focus_bolden_label_and_marker() {
        let marker = Style::default();
        let label = Style::default();
        let hovered = Checkbox::new("Label").state(CheckboxState::Hovered);
        let focused = Checkbox::new("Label").state(CheckboxState::Focused);

        assert!(
            hovered
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
}
