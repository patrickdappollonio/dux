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
/// used at render time: `Focused` highlights via the theme's button
/// colors, `Disabled` dims and drops the bold modifier so the button
/// reads as unavailable, and `Normal` falls back to the standard hint
/// text color. `Disabled` overrides any focus state — callers should set
/// it when the underlying action can't be taken right now (e.g. an apply
/// button when the current selection is already applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonState {
    Normal,
    Focused,
    Disabled,
}

/// Identifier for every modal button that can be activated by mouse. Lives
/// next to [`Button`] so the press-tracking type is colocated with the
/// widget it describes. The conversion from the broader hit-test target
/// `PromptMouseTarget` is implemented in `app::input` (where that enum
/// lives) and returns `None` for non-button targets — list rows, text
/// inputs, and checkboxes — so the press machinery cannot accidentally
/// arm those cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonPressedTarget {
    RuntimeKillCancel,
    RuntimeKillHovered,
    RuntimeKillSelected,
    RuntimeKillVisible,
    ConfirmKillCancel,
    ConfirmKillConfirm,
    ConfirmDeleteCancel,
    ConfirmDeleteConfirm,
    /// The worktree MANAGER's removal confirmation (distinct from the agent
    /// delete above: it removes a worktree nobody holds).
    ConfirmDeleteWorktreeCancel,
    ConfirmDeleteWorktreeConfirm,
    ConfirmDeleteTerminalCancel,
    ConfirmDeleteTerminalConfirm,
    ConfirmCloseTabCancel,
    ConfirmCloseTabConfirm,
    /// The first-tab warning's single dismiss button. It commits nothing: the
    /// modal exists to say the gesture has no effect here.
    ConfirmDeleteMacroCancel,
    ConfirmDeleteMacroConfirm,
    /// The macro EDITOR's own buttons (distinct from the nested delete-confirm
    /// above): Cancel abandons the edit, Save writes the macro to config.
    EditMacroCancel,
    EditMacroSave,
    /// The three `Configure*` modals' shared pair: Cancel abandons the edit,
    /// Save writes the startup command / environment block.
    ConfigureFieldCancel,
    ConfigureFieldSave,
    ConfirmQuitCancel,
    ConfirmQuitConfirm,
    ConfirmDiscardCancel,
    ConfirmDiscardConfirm,
    ConfirmCreateInitialCommitCancel,
    ConfirmCreateInitialCommitConfirm,
    ConfirmInitRepoCancel,
    ConfirmInitRepoConfirm,
    ConfirmNonDefaultBranchCancel,
    ConfirmNonDefaultBranchAdd,
    ConfirmUseExistingBranchCancel,
    ConfirmUseExistingBranchUse,
    ConfigReloadFailedClose,
    ConfigReloadFailedApply,
    AddProjectFailedOk,
    /// The Create-Agent-From-PR modal's secondary action, offered only when no
    /// project has been chosen: it hands over to the project selector and comes
    /// back in project-first mode.
    PullRequestChooseProject,
    AgentInfoClose,
    StartupCommandLogsClose,
    /// The first-load modal's two pill buttons. Unlike every other entry here
    /// they are NOT drawn by [`Button`] — the approved first-load design uses
    /// one-row accent-filled pills, not the 3-row bordered widget — but they ride
    /// the same press machinery so a mouse click behaves identically.
    FirstLoadPrimary,
    FirstLoadSecondary,
    /// The take-over card's single button. The card is deliberately NOT a modal
    /// (it must not block pane or tab navigation), so its press is tracked in a
    /// field of its own rather than in `App::pressed_button`, which the modal
    /// machinery owns and wipes on every non-prompt mouse event. What it shares
    /// with every other button here is [`button_state_for`], so the pressed look
    /// cannot drift from the rest of the app.
    TakeOverCard,
}

/// In-flight state for a button the user is currently pressing. `target`
/// records which button received the original mouse-down; `inside` tracks
/// whether the cursor is still over that same button right now. The
/// release handler fires the button's action only when `inside` is true,
/// matching the universal GUI convention where dragging off a button
/// before release cancels the click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PressedButton {
    pub(crate) target: ButtonPressedTarget,
    pub(crate) inside: bool,
}

/// Resolve the [`ButtonState`] for a given button at render time.
///
/// `pressed` is the app-wide press state (set on mouse-down, cleared on
/// release or any keystroke). When the press matches `target` *and* the
/// cursor is still inside the original button, the button shows the
/// focused look — providing immediate feedback that the press registered
/// without changing the keyboard-focus model. Dragging off the button
/// drops the override so the caller's regular `focused` signal takes
/// over again.
///
/// `enabled` always wins: a disabled button stays disabled even while
/// pressed, so a button that becomes unactivatable mid-drag does not
/// pretend it is still armed.
pub(crate) fn button_state_for(
    target: ButtonPressedTarget,
    pressed: Option<PressedButton>,
    focused: bool,
    enabled: bool,
) -> ButtonState {
    if !enabled {
        return ButtonState::Disabled;
    }
    if matches!(pressed, Some(p) if p.target == target && p.inside) {
        return ButtonState::Focused;
    }
    if focused {
        ButtonState::Focused
    } else {
        ButtonState::Normal
    }
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
            ButtonState::Disabled => (theme.border_normal, theme.hint_dim_desc_fg),
        };
        // Disabled buttons drop the BOLD modifier so they visually fade —
        // active and idle buttons stay bold to keep the row legible.
        let mut label_style = Style::default().fg(fg);
        if self.state != ButtonState::Disabled {
            label_style = label_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        // CENTERED BY HAND, not by `Alignment::Center`. Ratatui centers with
        // `(area_width / 2) - (label_width / 2)`, which halves each width on its
        // own and so rounds an odd label's offset UP: a nine-character label in a
        // fourteen-column button got three columns of padding on the left and two
        // on the right, one cell right of centre. Splitting the slack itself puts
        // the odd column on the right instead, which is where every other centred
        // thing in the app puts it, and leaves every even case exactly where it
        // already was.
        //
        // Measured in chars, the same unit `button_width_for` sizes the button in,
        // so the two cannot disagree about where the label ends.
        let label_w = u16::try_from(self.label.chars().count()).unwrap_or(u16::MAX);
        let pad = inner.width.saturating_sub(label_w) / 2;
        let label_area = Rect {
            x: inner.x.saturating_add(pad),
            width: inner.width.saturating_sub(pad),
            ..inner
        };
        Paragraph::new(Line::from(Span::styled(self.label, label_style)))
            .alignment(Alignment::Left)
            .render(label_area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label is centred, and an odd column of slack falls on the RIGHT.
    ///
    /// Ratatui's `Alignment::Center` halves each width on its own
    /// (`area / 2 - label / 2`), so an odd label in an even button came out one
    /// cell right of centre: "Take over" sat with three columns of padding on
    /// the left and two on the right. Measured off a rendered frame rather than
    /// asserted about the arithmetic, because the arithmetic is exactly what was
    /// wrong.
    #[test]
    fn a_buttons_label_is_centred_with_any_odd_column_on_the_right() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // "Take over" is 9 characters in the 16-wide minimum button: 5 columns
        // of slack, which cannot split evenly. "Cancel" is 6, which can.
        for (label, want_left, want_right) in [("Take over", 2, 3), ("Cancel", 4, 4)] {
            let width = button_width_for(label);
            let mut terminal = Terminal::new(TestBackend::new(width, 3)).expect("terminal");
            terminal
                .draw(|frame| {
                    Button::new(label).render(frame, frame.area(), &Theme::default_dark());
                })
                .expect("render frame");
            let buffer = terminal.backend().buffer();
            let inside: Vec<String> = (1..width - 1)
                .map(|x| buffer[(x, 1u16)].symbol().to_string())
                .collect();
            let left = inside.iter().take_while(|c| *c == " ").count();
            let right = inside.iter().rev().take_while(|c| *c == " ").count();
            assert_eq!(
                (left, right),
                (want_left, want_right),
                "\"{label}\" in a {width}-wide button rendered as {inside:?}"
            );
        }
    }

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

    fn pressed(target: ButtonPressedTarget, inside: bool) -> Option<PressedButton> {
        Some(PressedButton { target, inside })
    }

    #[test]
    fn button_state_for_pressed_inside_returns_focused() {
        // Holding the mouse on a button with the cursor still inside it
        // should always render as Focused, regardless of whether keyboard
        // focus was on it before the press.
        assert_eq!(
            button_state_for(
                ButtonPressedTarget::ConfirmKillConfirm,
                pressed(ButtonPressedTarget::ConfirmKillConfirm, true),
                false,
                true,
            ),
            ButtonState::Focused
        );
    }

    #[test]
    fn button_state_for_pressed_outside_falls_back_to_focused_signal() {
        // Drag-out drops the press visual; the underlying focus signal
        // takes over again so the keyboard-focused button stays
        // highlighted.
        assert_eq!(
            button_state_for(
                ButtonPressedTarget::ConfirmKillConfirm,
                pressed(ButtonPressedTarget::ConfirmKillConfirm, false),
                false,
                true,
            ),
            ButtonState::Normal
        );
        assert_eq!(
            button_state_for(
                ButtonPressedTarget::ConfirmKillConfirm,
                pressed(ButtonPressedTarget::ConfirmKillConfirm, false),
                true,
                true,
            ),
            ButtonState::Focused
        );
    }

    #[test]
    fn button_state_for_pressed_on_other_button_does_not_leak() {
        // A press on the Kill button must not visually affect Cancel.
        assert_eq!(
            button_state_for(
                ButtonPressedTarget::ConfirmKillCancel,
                pressed(ButtonPressedTarget::ConfirmKillConfirm, true),
                false,
                true,
            ),
            ButtonState::Normal
        );
    }

    #[test]
    fn button_state_for_disabled_overrides_press_and_focus() {
        // Disabled wins over both press and keyboard focus — a button that
        // becomes unactivatable mid-drag must not pretend it is armed.
        assert_eq!(
            button_state_for(
                ButtonPressedTarget::RuntimeKillSelected,
                pressed(ButtonPressedTarget::RuntimeKillSelected, true),
                true,
                false,
            ),
            ButtonState::Disabled
        );
    }

    #[test]
    fn button_state_for_no_press_uses_focus_signal() {
        assert_eq!(
            button_state_for(ButtonPressedTarget::ConfirmQuitConfirm, None, true, true,),
            ButtonState::Focused
        );
        assert_eq!(
            button_state_for(ButtonPressedTarget::ConfirmQuitConfirm, None, false, true,),
            ButtonState::Normal
        );
    }
}
