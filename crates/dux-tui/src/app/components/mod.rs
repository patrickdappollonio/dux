//! Reusable terminal UI components shared across modal dialogs and panes.
//!
//! Each submodule defines a self-contained widget with its own state types,
//! layout helpers, and rendering logic. Components do not know about
//! [`super::App`] — callers wire focus state and theme colors in. Keeping
//! components decoupled lets new modal dialogs reuse them without growing
//! per-call rendering boilerplate, and leaves room to lift the directory
//! into its own crate later if external consumers appear.

pub(crate) mod button;
pub(crate) mod checkbox;
pub(crate) mod focus_ring;
pub(crate) mod hint_bar;
pub(crate) mod scroll_marker;
pub(crate) mod wrap_lines;

pub(crate) use button::{
    Button, ButtonKind, ButtonPressedTarget, PressedButton, button_state_for, shared_button_width,
};
pub(crate) use checkbox::{Checkbox, CheckboxState};
pub(crate) use focus_ring::next_focus;
pub(crate) use hint_bar::{Hint, modal_hint_line};
pub(crate) use scroll_marker::render_scroll_marker;
/// The marker geometry is re-exported for the tests that assert a marker cannot
/// land on a content cell; the renderers reach it through
/// [`render_scroll_marker`].
#[cfg(test)]
pub(crate) use scroll_marker::{MARKER_GLYPHS, scroll_marker_rect};
pub(crate) use wrap_lines::wrap_styled_lines;
