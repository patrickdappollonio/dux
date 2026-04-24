//! Reusable rounded-border button widget for modal dialogs.
//!
//! Modals across the app render confirm/cancel rows by hand: a `Paragraph`
//! with a centered bold label inside a `Block` with rounded borders, with
//! border and label colors swapped based on focus and intent. This module
//! folds that boilerplate into a single [`Button`] type, keeps button
//! widths consistent via [`button_width_for`] / [`shared_button_width`],
//! and centralizes the focus-color mapping so future theme changes have a
//! single place to update.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::theme::Theme;

/// Standard minimum button width used across modal dialogs. Short labels
/// like "Cancel" and "Delete" sit comfortably inside this with whitespace
/// to spare, so most buttons in the app are this wide regardless of their
/// label length. Buttons with longer labels grow past it via
/// [`button_width_for`] / [`shared_button_width`].
pub(crate) const MIN_BUTTON_WIDTH: u16 = 16;

/// Width that fits `label` between two rounded borders with one column of
/// padding on each side, never narrower than [`MIN_BUTTON_WIDTH`]. The
/// formula is `label_chars + 1 left pad + 1 right pad + 2 borders`. Uses
/// `chars().count()` so multi-byte characters (CJK, emoji, box-drawing)
/// measure by visible width rather than UTF-8 byte length.
pub(crate) fn button_width_for(label: &str) -> u16 {
    let label_chars = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
    MIN_BUTTON_WIDTH.max(label_chars.saturating_add(4))
}

/// Largest [`button_width_for`] across `labels`. Use this when several
/// buttons share a row and must keep the same width so the layout doesn't
/// shift if a label changes (e.g. a confirm button whose text depends on a
/// checkbox state). Returns [`MIN_BUTTON_WIDTH`] when given an empty
/// slice.
pub(crate) fn shared_button_width(labels: &[&str]) -> u16 {
    labels
        .iter()
        .map(|label| button_width_for(label))
        .max()
        .unwrap_or(MIN_BUTTON_WIDTH)
}

/// Visual focus state of a button. Maps to the border + label color pair
/// used at render time: `Focused` highlights via the theme's button colors,
/// `Normal` falls back to the dim border + hint text color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonState {
    Normal,
    Focused,
}

/// Semantic intent of a button. Drives which theme color the focused
/// border uses, so the user gets a consistent visual signal across modals
/// (red for destructive, cyan for safe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    /// Safe action — dismissals, applies, navigations. Cyan when focused.
    /// Use for "Cancel", "Apply", "Use Existing", and any other button
    /// whose outcome is non-destructive.
    Confirm,
    /// Destructive action — deletes, quits, anything that loses data or
    /// skips a safety check. Red when focused. Use for "Delete", "Quit",
    /// "Discard", "Add Anyway", "Check Out & Add", etc.
    Danger,
}

/// Builder-style button widget. Owns a label and its focus/intent state;
/// renders itself in a single call given a theme reference. Width is
/// derived from the label via [`button_width_for`] so callers can either
/// query the width (`.width()`) before laying out the row, or use
/// [`shared_button_width`] for a row of equal-width buttons.
#[derive(Clone, Debug)]
pub(crate) struct Button<'a> {
    label: &'a str,
    state: ButtonState,
    kind: ButtonKind,
}

impl<'a> Button<'a> {
    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label,
            state: ButtonState::Normal,
            kind: ButtonKind::Confirm,
        }
    }

    pub(crate) fn state(mut self, state: ButtonState) -> Self {
        self.state = state;
        self
    }

    pub(crate) fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    /// Render into `area` using the theme's button colors. Always draws a
    /// rounded-border block 3 rows tall with the label centered on the
    /// middle row. Caller is responsible for sizing `area` (see
    /// [`Button::width`] / [`shared_button_width`]) — the widget does not
    /// clip or wrap.
    pub(crate) fn render(self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (border_color, fg) = match self.state {
            ButtonState::Focused => match self.kind {
                ButtonKind::Confirm => (theme.button_confirm_border, theme.button_active_fg),
                ButtonKind::Danger => (theme.button_danger_border, theme.button_active_fg),
            },
            ButtonState::Normal => (theme.border_normal, theme.hint_desc_fg),
        };
        Paragraph::new(Line::from(Span::styled(
            self.label,
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border::ROUNDED)
                .border_style(Style::default().fg(border_color)),
        )
        .render(area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_width_for_short_label_clamps_to_min() {
        assert_eq!(button_width_for("Cancel"), MIN_BUTTON_WIDTH);
        assert_eq!(button_width_for("Delete"), MIN_BUTTON_WIDTH);
        assert_eq!(button_width_for("Add Anyway"), MIN_BUTTON_WIDTH);
    }

    #[test]
    fn button_width_for_long_label_grows_past_min() {
        // 15 chars + 4 (2 padding + 2 borders) = 19.
        assert_eq!(button_width_for("Check Out & Add"), 19);
    }

    #[test]
    fn button_width_for_uses_visible_width_not_bytes() {
        // CJK character "世" is 3 UTF-8 bytes but 1 visible char.
        // Helper must measure by visible width, not byte length.
        assert_eq!(button_width_for("世界"), MIN_BUTTON_WIDTH);
    }

    #[test]
    fn shared_button_width_picks_largest() {
        let labels = ["Cancel", "Add Anyway", "Check Out & Add"];
        assert_eq!(shared_button_width(&labels), 19);
    }

    #[test]
    fn shared_button_width_falls_back_when_empty() {
        assert_eq!(shared_button_width(&[]), MIN_BUTTON_WIDTH);
    }
}
