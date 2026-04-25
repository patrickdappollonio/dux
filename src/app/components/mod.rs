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

pub(crate) use button::{Button, ButtonKind, ButtonState, shared_button_width};
pub(crate) use checkbox::{Checkbox, CheckboxState};
