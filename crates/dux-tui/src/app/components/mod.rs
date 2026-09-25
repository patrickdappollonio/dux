//! Reusable terminal UI components shared across modal dialogs and panes.
//!
//! Each submodule defines a self-contained widget with its own state types,
//! layout helpers, and rendering logic. Components do not know about
//! [`super::App`]: callers wire focus state and theme colors in.

pub(crate) mod button;
pub(crate) mod centered;
pub(crate) mod checkbox;
pub(crate) mod ellipsis;
pub(crate) mod focus_ring;
pub(crate) mod hint_bar;
pub(crate) mod modal;
pub(crate) mod name_chip;
pub(crate) mod pane_card;
pub(crate) mod picker_list;
pub(crate) mod scroll_marker;
pub(crate) mod scroll_view;
pub(crate) mod wrap_lines;

pub(crate) use button::{
    BUTTON_HEIGHT, Button, ButtonKind, ButtonPressedTarget, PressedButton, button_row,
    button_state_for, button_width_for, shared_button_width,
};
pub(crate) use centered::render_centered_lines;
pub(crate) use checkbox::{Checkbox, CheckboxState};
pub(crate) use focus_ring::next_focus;
pub(crate) use hint_bar::{
    Hint, HintTone, fitted_hint_spans, hint_spans, modal_hint_line, pane_hint_line,
    pane_hint_line_after,
};
pub(crate) use modal::Modal;
pub(crate) use name_chip::{labelled_name, name_chip, prose_lines, prose_spans};
pub(crate) use pane_card::{CardBlockPlan, CardContent, PaneCardBlock, plan_pane_card};
pub(crate) use picker_list::{PickerList, PickerListLayout};
pub(crate) use scroll_marker::render_scroll_marker;
/// The marker geometry is re-exported for the tests that assert a marker cannot
/// land on a content cell; the renderers reach it through
/// [`render_scroll_marker`].
#[cfg(test)]
pub(crate) use scroll_marker::{MARKER_GLYPHS, scroll_marker_rect};
/// The indicator color, re-exported for the tests that check a surface wears it;
/// the renderers reach it through [`render_scroll_indicator`].
#[cfg(test)]
pub(crate) use scroll_view::scroll_indicator_color;
pub(crate) use scroll_view::{ScrollViewRender, render_scroll_indicator, render_scroll_view};
pub(crate) use wrap_lines::wrap_styled_lines;
