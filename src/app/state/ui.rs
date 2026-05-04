//! Visual/interaction state grouped from the App god-object (audit02 P1-V).
//!
//! These fields cluster around: pane focus, fullscreen overlays, prompt/modal
//! stack, mouse layout caches, welcome screen rotation, and the explicit
//! force-redraw flag. Rendering code touches these heavily; worker callbacks
//! generally do not.

use super::super::{
    FocusPane, FullscreenOverlay, InputTarget, MacroBarState, MouseLayoutState,
    OverlayMouseLayoutState, PromptState, RecentMouseClick, ResizeDragState, components,
};

pub(crate) struct UiState {
    pub(crate) focus: FocusPane,
    pub(crate) fullscreen_overlay: FullscreenOverlay,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) help_scroll: Option<u16>,
    pub(crate) last_help_height: u16,
    pub(crate) last_help_lines: u16,
    pub(crate) resize_mode: bool,
    pub(crate) left_collapsed: bool,
    pub(crate) right_collapsed: bool,
    pub(crate) right_hidden: bool,
    pub(crate) mouse_layout: MouseLayoutState,
    pub(crate) overlay_layout: OverlayMouseLayoutState,
    pub(crate) mouse_drag: Option<ResizeDragState>,
    pub(crate) last_mouse_click: Option<RecentMouseClick>,
    /// Tracks an in-flight modal-button press: which button received
    /// mouse-down and whether the cursor is still inside it. Set on
    /// `MouseEventKind::Down(Left)` over a button, updated on `Drag`,
    /// cleared on `Up` (firing the button's action only when the cursor
    /// is still inside) and on any keystroke or modal-close event.
    pub(crate) pressed_button: Option<components::PressedButton>,
    pub(crate) macro_bar: Option<MacroBarState>,
    pub(crate) force_redraw: bool,
    pub(crate) welcome_tip_index: usize,
    /// Whether the ASCII logo was rendered in the previous frame.
    pub(crate) welcome_logo_visible: bool,
    /// The left-pane selection index when the logo last rendered a tip.
    pub(crate) welcome_tip_selection: usize,
    /// When true, show the alternate (duck) logo instead of the text logo.
    pub(crate) welcome_logo_alt: bool,
    pub(crate) pr_banner_at_bottom: bool,
}
