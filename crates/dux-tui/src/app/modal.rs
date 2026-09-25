//! The modal registry: what kind of thing each modal is, and what it owes.
//!
//! Built as an exhaustive match with no `_` arm, the way
//! [`super::overlay_dismiss::outside_click_policy`] is: a new [`PromptState`]
//! variant does not compile until someone has answered "what family is this, and
//! does it need a confirm button?".
//!
//! # The four families
//!
//! Derived from what a keystroke means in the modal, not from how it looks:
//!
//! | Family | Shape | Up/Down | Left/Right | Enter | Space |
//! |---|---|---|---|---|---|
//! | [`ModalFamily::Report`] | read-only prose, scrollable | scroll the body |, | dismiss | scroll/dismiss |
//! | [`ModalFamily::Confirm`] | prose, two buttons, maybe a checkbox |, | move focus between controls | act on the focused control | act on the focused control |
//! | [`ModalFamily::Picker`] | rows with a selection cursor, maybe a filter | move the SELECTION (a value, not focus) |, | pick the selected row | typed into the filter |
//! | [`ModalFamily::Form`] | fields plus buttons |, | belongs to the CARET; never reaches the binding lookup | see the dual-mode rule below | typed when a field has focus |
//!
//! A Picker's Up/Down changes a value (which row is selected); a Confirm's
//! Left/Right changes focus and nothing else. A Form's horizontal arrows belong
//! to the text caret, which is why [`crate::keybindings::text_field_owns_key`]
//! gates the binding lookup there (see [`binding_lookup_is_suppressed`]).
//!
//! # What this registry enforces, and what it only declares
//!
//! The compiler enforces the exhaustive matches in [`modal_spec`] and
//! [`prompt_text_inputs`]: adding a [`PromptState`] variant is a build error
//! until somebody classifies it. That gate holds whether or not any code reads
//! the result, which is why every item here carries `#[allow(dead_code)]`.
//!
//! [`ModalSpec`], [`ModalFamily`], [`KNOWN_DUAL_MODE_VIOLATIONS`] and
//! [`ModalSpec::satisfies_dual_mode_rule`] are read by the guard tests and by
//! nothing on the render or input path, so a misdeclared family or a dual-mode
//! violation is caught only by `cargo test`, and only as far as the fixtures in
//! `every_prompt` reach.
//!
//! The families themselves are enforced nowhere: no dispatcher consults
//! `spec.family` before routing a key, so a modal declared `Report` whose
//! handler moves a selection cursor compiles and ships. Declare the family
//! honestly, and change the declaration in the same edit as the key behaviour.
//!
//! Coverage is `PromptState`, not every typing surface in dux: the
//! commit-message pane (`App::commit_input`) and the startup-log viewer
//! (`App::startup_log_viewer`) are dual-mode text surfaces that are not
//! `PromptState` variants, so nothing here guards them.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::input::contains_point;
use super::text_input::TextInput;
use super::{App, OverlayMouseLayout, PromptState};
use crate::keybindings::{Action, text_field_owns_key};

/// What kind of thing a modal is, in terms of what its keys mean.
///
/// See the module docs for the full table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Not called from the render or input paths: the registry is a declaration
// whose value is the exhaustive match the compiler checks whether or not
// anything reads the result. Do not "clean it up" by deleting an unread arm.
#[allow(dead_code)]
pub(crate) enum ModalFamily {
    /// Read-only and scrollable. No focus concept, because there is nothing to
    /// focus: the vertical keys scroll the body.
    Report,
    /// Prose and two buttons, sometimes a checkbox. Horizontal keys move focus
    /// between those controls; Space and Enter act on whichever has it.
    Confirm,
    /// Rows with a selection cursor, optionally filtered. The vertical keys
    /// move the SELECTION, a value, not focus, and Enter picks it.
    Picker,
    /// Text fields plus buttons. The horizontal keys belong to the caret.
    Form,
}

/// Everything the registry declares about one open modal, as one struct behind
/// one match: two exhaustive matches over the same enum would be two places to
/// forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct ModalSpec {
    /// Which of the four families this modal belongs to.
    pub(crate) family: ModalFamily,
    /// Whether the modal contains a full-text (multiline) field. Not knowable
    /// from the type, since multiline-ness is a runtime flag set at construction
    /// by [`TextInput::with_multiline`], so this is a claim the guard tests check
    /// against a real instance through [`prompt_text_inputs`].
    pub(crate) multiline_field: bool,
    /// Whether the modal publishes a button that commits it (Apply, Save,
    /// Delete, Quit) as opposed to one that merely dismisses it (Close, OK).
    /// Also a claim, checked by rendering the variant and asking
    /// [`layout_publishes_confirm_button`].
    pub(crate) confirm_button: bool,
}

#[allow(dead_code)]
impl ModalSpec {
    const fn new(family: ModalFamily, multiline_field: bool, confirm_button: bool) -> Self {
        Self {
            family,
            multiline_field,
            confirm_button,
        }
    }

    /// The dual-mode rule: **a modal containing a multi-line text field must
    /// have a confirm button.**
    ///
    /// With a button, Enter engages the field while it is unengaged, inserts a
    /// newline while it is engaged, and activates the control focus is on.
    /// Without one there is no third meaning for Enter to land on. A modal with
    /// only single-line fields needs no button, because Enter submits and
    /// nothing competes for it. House style for modals, not a logical necessity.
    pub(crate) fn satisfies_dual_mode_rule(self) -> bool {
        !self.multiline_field || self.confirm_button
    }
}

/// The modals that break the dual-mode rule today, by title.
///
/// Empty, and it must stay that way. The test asserts this set exactly, so a new
/// violator cannot be added without writing its name here and defending it in
/// review. The list should only ever shrink.
#[allow(dead_code)]
pub(crate) const KNOWN_DUAL_MODE_VIOLATIONS: &[&str] = &[];

/// The registry. `None` means "no modal is open" ([`PromptState::None`]).
///
/// The match is exhaustive with no `_` arm: a new `PromptState` variant is a
/// compile error here until its family and its two obligations are declared. Do
/// not add a catch-all arm, and do not group a new variant into an existing arm
/// without checking that all three answers really are the same.
#[allow(dead_code)]
pub(crate) fn modal_spec(prompt: &PromptState) -> Option<ModalSpec> {
    use ModalFamily::{Confirm, Form, Picker, Report};
    let spec = match prompt {
        PromptState::None => return None,

        // ── Report ──────────────────────────────────────────────────────
        // Read-only bodies. Their one button dismisses; none of them commits
        // anything, so none of them publishes a confirm button.
        PromptState::AgentInfo(_)
        | PromptState::AddProjectFailed { .. }
        | PromptState::FirstLoad(_)
        | PromptState::DebugInput { .. } => ModalSpec::new(Report, false, false),

        // A Picker despite reading like a report: a `ListState` cursor over the
        // runs, vertical keys that move that selection (the output pane scrolls
        // on the paging keys), a filter, and a confirm key that promotes the
        // selected run to the fullscreen viewer. Its one button is a Close, a way
        // out and not a commit, so the confirm-button claim stays false.
        PromptState::StartupCommandLogs(_) => ModalSpec::new(Picker, false, false),

        // A Picker despite reading like a report: a `ListState` selection cursor,
        // vertical keys that move that cursor (a value, not a scroll offset and
        // not focus), and a confirm key that acts on the selected row by
        // expanding it rather than choosing it and closing.
        PromptState::ResourceMonitor { .. } => ModalSpec::new(Picker, false, false),

        // ── Confirm ─────────────────────────────────────────────────────
        PromptState::ConfirmDeleteAgent { .. }
        | PromptState::ConfirmDeleteTerminal { .. }
        | PromptState::ConfirmCloseTab { .. }
        | PromptState::ConfirmDetachAgent { .. }
        | PromptState::ConfirmRecreateWorkingCopy { .. }
        | PromptState::ConfirmCheckoutDefaultBranch { .. }
        // Prose and a Cancel / Danger pair, Cancel focused: the project-scoped
        // deletes, the same questions the browser's dialogs ask.
        | PromptState::ConfirmDeleteProject { .. }
        | PromptState::ConfirmRemoveProject { .. }
        | PromptState::ConfirmQuit { .. }
        | PromptState::ConfirmDiscardFile { .. }
        | PromptState::ConfirmKillRunning(_)
        | PromptState::ConfirmInitRepo { .. }
        | PromptState::ConfirmCreateInitialCommit { .. }
        | PromptState::ConfirmNonDefaultBranch { .. }
        | PromptState::ConfirmUseExistingBranch { .. }
        // Prose, a conditional checkbox and a Cancel/Delete pair; horizontal
        // keys move focus and Space acts on what has it.
        | PromptState::ConfirmDeleteWorktree(_)
        | PromptState::ConfigReloadFailed { .. } => ModalSpec::new(Confirm, false, true),

        // ── Picker ──────────────────────────────────────────────────────
        // A selection cursor over rows; Enter picks. Their filter rows are
        // type-immediately and single-line, so no dual-mode question arises.
        // A picker gets no Cancel and no Apply: its footer names the keys
        // through the bindings, and a button label cannot stay truthful once a
        // user rebinds.
        PromptState::Command { .. }
        | PromptState::BrowseProjects { .. }
        | PromptState::PickEditor { .. }
        | PromptState::PickProject { .. }
        | PromptState::PickProjectWorktree(_)
        // The worktree manager: rows with a selection cursor over the
        // REMOVABLE worktrees, and a confirm key that acts on the selection by
        // raising the removal confirmation. No buttons, so no confirm button.
        | PromptState::ManageWorktrees(_)
        | PromptState::ChangeTheme(_)
        | PromptState::ChangeAgentProvider(_)
        | PromptState::ChangeDefaultProvider(_)
        | PromptState::ChangeProjectDefaultProvider(_)
        // Three modes, the saved one marked, and picking one applies it. Rows and
        // nothing else, so no buttons and no focus concept.
        | PromptState::SetTailscaleMode(_) => ModalSpec::new(Picker, false, false),

        // The one picker that keeps its buttons: they are distinct actions (kill
        // the hovered runtime, kill the marked ones, kill everything the filter
        // shows), not a confirm/cancel pair restating what Enter does. Do not
        // remove them for consistency.
        PromptState::KillRunning(_) => ModalSpec::new(Picker, false, true),

        // ── Form ────────────────────────────────────────────────────────
        // Single-line field plus (for two of them) checkboxes. Enter submits,
        // so the rule asks no button of them.
        PromptState::RenameSession { .. }
        | PromptState::NameNewAgent { .. }
        | PromptState::PullRequestInput { .. }
        | PromptState::AttachPullRequestInput { .. }
        // The standalone-agent name field: one single-line control, so Enter
        // submits and the rule asks no button of it either.
        | PromptState::NameStandaloneAgent { .. } => ModalSpec::new(Form, false, false),

        // The three configure modals: one full-text field plus Cancel/Save.
        // They were the dual-mode rule's only violators and are now compliant,
        // so `KNOWN_DUAL_MODE_VIOLATIONS` is empty.
        PromptState::ConfigureStartupCommand { .. }
        | PromptState::ConfigureProjectEnv { .. }
        | PromptState::ConfigureGlobalEnv { .. } => ModalSpec::new(Form, true, true),

        // ── The one variant that is two modals ──────────────────────────
        // `EditMacros` serves two families depending on its own state, so the
        // registry answers as a function of state rather than of the variant.
        // The arms below must stay ordered most-specific first.
        PromptState::EditMacros {
            pending_delete: Some(_),
            ..
        } => ModalSpec::new(Confirm, false, true),
        // The editor: a name field, a multiline body, a surface selector, and
        // Cancel/Save. Compliant with the dual-mode rule, and the reference for
        // what compliance looks like.
        PromptState::EditMacros {
            editing: Some(_), ..
        } => ModalSpec::new(Form, true, true),
        // The list underneath: rows with a selection cursor, no buttons,
        // publishing its rows as `OverlayMouseLayout::EditMacroList`.
        PromptState::EditMacros { .. } => ModalSpec::new(Picker, false, false),
    };
    Some(spec)
}

/// Every [`TextInput`] the open modal owns, so the registry's `multiline_field`
/// claim can be checked against a live instance instead of trusted. Exhaustive
/// for the same reason [`modal_spec`] is: a new variant that quietly grows a
/// text field would otherwise sail past the dual-mode check.
#[allow(dead_code)]
pub(crate) fn prompt_text_inputs(prompt: &PromptState) -> Vec<&TextInput> {
    match prompt {
        PromptState::None
        | PromptState::AgentInfo(_)
        | PromptState::AddProjectFailed { .. }
        | PromptState::FirstLoad(_)
        | PromptState::DebugInput { .. }
        | PromptState::ResourceMonitor { .. }
        | PromptState::ConfigReloadFailed { .. }
        | PromptState::ConfirmDeleteAgent { .. }
        | PromptState::ConfirmDeleteTerminal { .. }
        | PromptState::ConfirmCloseTab { .. }
        | PromptState::ConfirmDetachAgent { .. }
        | PromptState::ConfirmRecreateWorkingCopy { .. }
        | PromptState::ConfirmCheckoutDefaultBranch { .. }
        | PromptState::ConfirmDeleteProject { .. }
        | PromptState::ConfirmRemoveProject { .. }
        | PromptState::ConfirmQuit { .. }
        | PromptState::ConfirmDiscardFile { .. }
        | PromptState::ConfirmInitRepo { .. }
        | PromptState::ConfirmCreateInitialCommit { .. }
        | PromptState::ConfirmNonDefaultBranch { .. }
        | PromptState::ConfirmUseExistingBranch { .. }
        | PromptState::PickEditor { .. }
        | PromptState::PickProjectWorktree(_)
        | PromptState::ManageWorktrees(_)
        | PromptState::ConfirmDeleteWorktree(_)
        | PromptState::ChangeTheme(_)
        | PromptState::ChangeAgentProvider(_)
        | PromptState::ChangeDefaultProvider(_)
        | PromptState::ChangeProjectDefaultProvider(_)
        | PromptState::SetTailscaleMode(_) => Vec::new(),

        PromptState::Command { input, .. }
        | PromptState::ConfigureStartupCommand { input, .. }
        | PromptState::ConfigureProjectEnv { input, .. }
        | PromptState::ConfigureGlobalEnv { input, .. }
        | PromptState::RenameSession { input, .. }
        | PromptState::PullRequestInput { input, .. }
        | PromptState::AttachPullRequestInput { input, .. }
        | PromptState::NameStandaloneAgent { input, .. }
        | PromptState::NameNewAgent { input, .. } => vec![input],

        PromptState::StartupCommandLogs(prompt) => vec![&prompt.filter],
        PromptState::PickProject { list, .. } => vec![&list.filter],
        PromptState::KillRunning(prompt) => vec![&prompt.list.filter],
        PromptState::ConfirmKillRunning(prompt) => vec![&prompt.previous.list.filter],
        PromptState::BrowseProjects {
            filter, path_input, ..
        } => vec![filter, path_input],
        PromptState::EditMacros { editing, .. } => editing
            .as_ref()
            .map(|state| vec![&state.name_input, &state.text_input])
            .unwrap_or_default(),
    }
}

/// Whether the open modal really does hold a full-text field, asked of the live
/// value rather than of the table.
#[allow(dead_code)]
pub(crate) fn prompt_has_multiline_field(prompt: &PromptState) -> bool {
    prompt_text_inputs(prompt)
        .iter()
        .any(|input| input.is_multiline())
}

/// Whether a published mouse layout carries a button that commits the modal.
///
/// Commit versus dismiss: an `ok_button` on an error report or a `close_button`
/// on a log viewer is a way out, not a third meaning for Enter to land on, so
/// neither counts. Exhaustive with no `_` arm.
#[allow(dead_code)]
pub(crate) fn layout_publishes_confirm_button(layout: &OverlayMouseLayout) -> bool {
    match layout {
        // Nothing published, or nothing but a way out.
        OverlayMouseLayout::None
        | OverlayMouseLayout::Help
        | OverlayMouseLayout::Command { .. }
        | OverlayMouseLayout::BrowseProjects { .. }
        | OverlayMouseLayout::ChangeAgentProvider { .. }
        | OverlayMouseLayout::ChangeDefaultProvider { .. }
        | OverlayMouseLayout::ChangeProjectDefaultProvider { .. }
        | OverlayMouseLayout::SetTailscaleMode { .. }
        | OverlayMouseLayout::AddProjectFailed { .. }
        | OverlayMouseLayout::AgentInfo { .. }
        | OverlayMouseLayout::FirstLoad { .. }
        | OverlayMouseLayout::PickEditor { .. }
        | OverlayMouseLayout::PickProjectWorktree { .. }
        | OverlayMouseLayout::ManageWorktrees { .. }
        | OverlayMouseLayout::PickProject { .. }
        | OverlayMouseLayout::ChangeTheme { .. }
        | OverlayMouseLayout::EditMacroList { .. }
        | OverlayMouseLayout::ResourceMonitor { .. }
        | OverlayMouseLayout::StartupCommandLogs { .. }
        | OverlayMouseLayout::RenameSession { .. }
        // The PR modal's one button hands over to the project picker; it does
        // not commit the form. Its field is single-line, so Enter still submits
        // and the dual-mode rule asks no confirm button of it.
        | OverlayMouseLayout::PullRequestInput { .. }
        // One single-line field and nothing else: Enter submits, so the
        // dual-mode rule asks no confirm button of it.
        | OverlayMouseLayout::AttachPullRequestInput { .. }
        // Likewise one single-line field and nothing else.
        | OverlayMouseLayout::NameStandaloneAgent { .. }
        | OverlayMouseLayout::NameNewAgent { .. } => false,

        // A button that commits.
        OverlayMouseLayout::KillRunning { .. }
        | OverlayMouseLayout::ConfirmKillRunning { .. }
        | OverlayMouseLayout::ConfirmDeleteAgent { .. }
        | OverlayMouseLayout::ConfirmDeleteWorktree { .. }
        | OverlayMouseLayout::ConfirmDeleteTerminal { .. }
        | OverlayMouseLayout::ConfirmCloseTab { .. }
        | OverlayMouseLayout::ConfirmDetachAgent { .. }
        | OverlayMouseLayout::ConfirmRecreateWorkingCopy { .. }
        | OverlayMouseLayout::ConfirmCheckoutDefaultBranch { .. }
        | OverlayMouseLayout::ConfirmDeleteProject { .. }
        | OverlayMouseLayout::ConfirmRemoveProject { .. }
        | OverlayMouseLayout::ConfirmDeleteMacro { .. }
        | OverlayMouseLayout::ConfirmQuit { .. }
        | OverlayMouseLayout::ConfirmDiscardFile { .. }
        | OverlayMouseLayout::ConfirmCreateInitialCommit { .. }
        | OverlayMouseLayout::ConfirmInitRepo { .. }
        | OverlayMouseLayout::ConfirmNonDefaultBranch { .. }
        | OverlayMouseLayout::ConfirmUseExistingBranch { .. }
        | OverlayMouseLayout::ConfigReloadFailed { .. }
        | OverlayMouseLayout::ConfigureStartupCommand { .. }
        | OverlayMouseLayout::EditMacros { .. } => true,
    }
}

// ── The chrome trio ─────────────────────────────────────────────────────────

/// The geometry a modal needs back after its frame is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModalFrame {
    /// The modal's outer rect, borders included. Already recorded as the
    /// topmost modal's rect for the click-outside engine.
    pub(crate) area: Rect,
    /// The area inside the border ring, where the modal's own content goes.
    pub(crate) inner: Rect,
}

impl App {
    /// Open a modal: dim the app behind it, clear and claim its rect, and paint
    /// the titled border ring.
    ///
    /// [`App::clear_overlay_area`] is the one chokepoint that records the
    /// topmost modal's rect for the click-outside engine, which fails closed, so
    /// a modal that clears its area some other way becomes undismissable by
    /// mouse. [`App::themed_overlay_block`]'s border ring doubles as the refusal
    /// cue for an outside click answered with a blink rather than a close.
    ///
    /// `area` stays the caller's: modals size themselves by percentage, by exact
    /// cells, or by content.
    pub(crate) fn open_modal_frame(
        &self,
        frame: &mut Frame,
        title: &str,
        area: Rect,
    ) -> ModalFrame {
        self.render_dim_overlay(frame);
        self.clear_overlay_area(frame, area);
        let block = self.themed_overlay_block(title);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        ModalFrame { area, inner }
    }
}

// ── Click routing ───────────────────────────────────────────────────────────

/// Which published control a click landed on, or `None` for a click that hit
/// no control.
///
/// On a hit the caller moves focus to that control and then acts on it, in that
/// order: acting alone leaves the modal's visible focus pointing somewhere the
/// next keystroke will act on instead.
///
/// `targets` is the modal's published rects in any order; overlapping rects
/// resolve to the first match, so publish the topmost control first.
pub(crate) fn click_target<T: Copy>(targets: &[(Rect, T)], column: u16, row: u16) -> Option<T> {
    targets
        .iter()
        .find(|(rect, _)| contains_point(*rect, column, row))
        .map(|&(_, target)| target)
}

// ── The key ladder ──────────────────────────────────────────────────────────

/// One rung of the ladder every modal's key handler walks: close, then move
/// focus, then act on focus, then fall through to whatever text field has focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModalKeyStep {
    /// Dismiss the modal, through its real cancel path (never a bare
    /// `prompt = None`; see [`super::overlay_dismiss`]).
    Close,
    /// Move focus. `true` is forwards.
    MoveFocus(bool),
    /// The confirm key. What it means is the family's business: a `Confirm` or
    /// `Picker` acts on whatever has focus, while a `Form` whose fields are all
    /// single-line submits whichever control has focus. Collapsing this into
    /// [`ModalKeyStep::ActivateFocus`] would change the rename-agent modal,
    /// where Enter submits while the checkbox has focus and Space toggles it.
    Confirm,
    /// Space, with focus NOT on a text field: act on the focused control,
    /// activate a button, toggle a checkbox.
    ActivateFocus,
    /// Nobody claimed it. Hand it to the focused text field, if there is one.
    FallThroughToField,
}

/// Whether the binding lookup must be skipped for this key.
///
/// True exactly when a text field has focus and the field owns the key, which
/// keeps plain characters and the horizontal arrows away from the bindings: the
/// movement action's default key set includes the horizontal arrows, so without
/// this gate Left in the rename-agent modal flips the "also rename the git
/// branch" checkbox instead of moving the caret.
///
/// The renderer must ask the same question when it picks the footer's key
/// (`RuntimeBindings::label_for_text_field_dialog`), so the hint can never name
/// a key the field swallows.
pub(crate) fn binding_lookup_is_suppressed(key: KeyEvent, text_field_focused: bool) -> bool {
    text_field_focused && text_field_owns_key(key)
}

/// Classify a key into its rung, given the action the bindings resolved it to
/// (or `None`, whether because nothing is bound or because
/// [`binding_lookup_is_suppressed`] said not to look).
///
/// `text_field_focused` only affects Space: Space is content in both kinds of
/// text field, so it may only act on focus when focus is sitting on a button or
/// a checkbox.
pub(crate) fn modal_key_step(
    action: Option<Action>,
    key: KeyEvent,
    text_field_focused: bool,
) -> ModalKeyStep {
    match action {
        Some(Action::CloseOverlay) => ModalKeyStep::Close,
        Some(Action::ToggleSelection) => ModalKeyStep::MoveFocus(!focus_move_is_reverse(key)),
        Some(Action::Confirm) => ModalKeyStep::Confirm,
        _ if key.code == KeyCode::Char(' ') && !text_field_focused => ModalKeyStep::ActivateFocus,
        _ => ModalKeyStep::FallThroughToField,
    }
}

/// Whether a focus-movement key means "backwards". The movement action carries
/// no direction of its own, so the key that triggered it supplies one. Mirrors
/// `super::input::focus_move_is_reverse`, kept here so the ladder stays a pure
/// function the tests can drive without an `App`.
fn focus_move_is_reverse(key: KeyEvent) -> bool {
    use ratatui::crossterm::event::KeyModifiers;
    matches!(key.code, KeyCode::BackTab)
        || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
        || matches!(key.code, KeyCode::Left | KeyCode::Char('h'))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn no_modal_open_has_no_spec() {
        assert_eq!(modal_spec(&PromptState::None), None);
        assert!(prompt_text_inputs(&PromptState::None).is_empty());
    }

    #[test]
    fn the_dual_mode_rule_only_bites_multiline_modals() {
        let single_line_no_button = ModalSpec::new(ModalFamily::Form, false, false);
        let multiline_no_button = ModalSpec::new(ModalFamily::Form, true, false);
        let multiline_with_button = ModalSpec::new(ModalFamily::Form, true, true);
        assert!(single_line_no_button.satisfies_dual_mode_rule());
        assert!(!multiline_no_button.satisfies_dual_mode_rule());
        assert!(multiline_with_button.satisfies_dual_mode_rule());
    }

    /// The resource monitor is a Picker, not a Report. It was declared Report
    /// while its handler moved a `selected_row` and its confirm key expanded
    /// the selected row, which is a Picker in this registry's own terms. The
    /// declaration is now pinned so the two halves cannot drift apart again.
    #[test]
    fn the_resource_monitor_is_a_picker() {
        let monitor = PromptState::ResourceMonitor {
            rows: Vec::new(),
            scroll_offset: 0,
            selected_row: 0,
            expanded: std::collections::HashSet::new(),
            last_refresh: std::time::Instant::now(),
            short_window_sample: false,
        };
        assert_eq!(
            modal_spec(&monitor).map(|spec| spec.family),
            Some(ModalFamily::Picker),
            "rows plus a selection cursor plus a confirm key acting on the \
             selection is a Picker"
        );
    }

    /// The startup-log modal is a Picker, not a Report, for exactly the reasons
    /// the resource monitor is. It was declared a Report while nothing could
    /// open it, and "read-only body, vertical keys scroll it, nothing to focus"
    /// was never what its code did: it renders a `ListState` cursor over the
    /// runs, its vertical keys move that SELECTION (the body scrolls on the
    /// paging keys instead), it carries a filter, and its confirm key acts on
    /// the selection by promoting that run to the fullscreen viewer. Opening it
    /// on the read-logs journey is what made the mislabel reachable.
    #[test]
    fn the_startup_log_modal_is_a_picker() {
        let logs = PromptState::StartupCommandLogs(StartupCommandLogPrompt {
            scope_label: "demo".to_string(),
            entries: Vec::new(),
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            content: String::new(),
            scroll_offset: 0,
            wrap_width: 0,
            focus: StartupCommandLogFocus::List,
        });
        assert_eq!(
            modal_spec(&logs).map(|spec| spec.family),
            Some(ModalFamily::Picker),
            "rows plus a selection cursor plus a confirm key acting on the \
             selection is a Picker"
        );
        assert!(
            !prompt_has_multiline_field(&logs),
            "its one field is the filter, which is a type-immediately search \
             row and must stay single-line"
        );
    }

    #[test]
    fn edit_macros_reports_a_different_family_per_state() {
        // Proved against real values in `render.rs`'s fixture test; here we only
        // pin that the three arms are distinguishable and ordered correctly.
        use super::super::{MacroEditFocus, MacroEditState, PendingMacroDelete};
        let list = PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: None,
            pending_delete: None,
        };
        let editor = PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: Some(MacroEditState {
                id: None,
                name_input: TextInput::new(),
                text_input: TextInput::new().with_multiline(8),
                surface: crate::config::MacroSurface::Both,
                focus: MacroEditFocus::Name,
            }),
            pending_delete: None,
        };
        let deleting = PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: None,
            pending_delete: Some(PendingMacroDelete {
                name: "m".to_string(),
                focus: ConfirmFocus::Cancel,
            }),
        };
        assert_eq!(
            modal_spec(&list).map(|spec| spec.family),
            Some(ModalFamily::Picker)
        );
        assert_eq!(
            modal_spec(&editor).map(|spec| spec.family),
            Some(ModalFamily::Form)
        );
        assert_eq!(
            modal_spec(&deleting).map(|spec| spec.family),
            Some(ModalFamily::Confirm)
        );
        // And the state-dependence reaches the multiline claim too.
        assert!(!prompt_has_multiline_field(&list));
        assert!(prompt_has_multiline_field(&editor));
    }

    // -- The fixtures: every variant, built for real and rendered ------------
    //
    // The table above is only a CLAIM until something builds each variant and
    // checks it. These fixtures are that something. They are deliberately
    // constructed by hand rather than by calling the app's `open_*` helpers:
    // the point is to pin what the variant IS, not to re-test the code that
    // opens it.

    use super::super::first_load::{FirstLoadButton, FirstLoadPrompt};
    use super::super::test_support::{default_bindings, test_app};
    use super::super::{
        AgentInfoPrompt, AgentInfoTone, ChangeAgentProviderMode, ChangeAgentProviderOption,
        ChangeAgentProviderPrompt, ChangeDefaultProviderOption, ChangeDefaultProviderPrompt,
        ChangeProjectDefaultProviderOption, ChangeProjectDefaultProviderPrompt, ChangeThemePrompt,
        ConfigReloadFailedFocus, ConfigureFieldFocus, ConfirmDeleteWorktreePrompt, ConfirmFocus,
        ConfirmKillRunningPrompt, ConfirmNonDefaultBranchFocus, DeleteAgentFocus,
        DeleteWorktreeFocus, KillRunningAction, KillRunningFocus, KillRunningPrompt,
        MacroEditFocus, MacroEditState, ManageWorktreesPrompt, NameNewAgentFocus,
        PendingMacroDelete, PickProjectWorktreePrompt, ProjectChooserIntent, RenameSessionFocus,
        SearchableList, StartupCommandLogFocus, StartupCommandLogPrompt,
    };
    use crate::model::ProviderKind;
    use dux_core::worker::{BranchWarningKind, CreateAgentRequest};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::collections::{BTreeSet, HashSet};
    use std::path::PathBuf;
    use std::time::Instant;

    fn macro_edit_state() -> MacroEditState {
        MacroEditState {
            id: None,
            name_input: TextInput::with_text("greet".to_string()),
            // The editor's body is the one COMPLIANT dual-mode modal: a
            // multiline field with a Save button.
            text_input: TextInput::with_text("hello".to_string()).with_multiline(8),
            surface: crate::config::MacroSurface::Both,
            focus: MacroEditFocus::Name,
        }
    }

    fn kill_running_prompt() -> KillRunningPrompt {
        KillRunningPrompt {
            runtimes: Vec::new(),
            list: SearchableList::new(),
            selected_ids: HashSet::new(),
            focus: KillRunningFocus::List,
        }
    }

    fn new_project_request(project: &crate::model::Project) -> CreateAgentRequest {
        CreateAgentRequest::NewProject {
            project: project.clone(),
            custom_name: None,
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        }
    }

    /// Every `PromptState` variant, in a state a user can really reach, paired
    /// with the name the registry knows it by.
    ///
    /// `EditMacros` appears THREE times, once per state, because it is one
    /// variant serving three modals - see the note on `modal_spec`.
    fn manage_worktrees_prompt(project: &crate::model::Project) -> ManageWorktreesPrompt {
        ManageWorktreesPrompt {
            project: project.clone(),
            entries: Vec::new(),
            loading: false,
            selected: None,
            error: None,
        }
    }

    /// Every modal as the registry's fixtures build it. Shared with the confirm
    /// dialog's own journey tests, so a new Confirm-family modal is covered
    /// there the moment it has a fixture here.
    pub(in crate::app) fn every_prompt(app: &App) -> Vec<(&'static str, PromptState)> {
        let project = app.engine.projects[0].clone();
        vec![
            (
                "Command",
                PromptState::Command {
                    input: TextInput::new(),
                    selected: 0,
                },
            ),
            (
                "BrowseProjects",
                PromptState::BrowseProjects {
                    purpose: crate::app::BrowsePurpose::AddProject,
                    current_dir: PathBuf::from("/tmp"),
                    entries: Vec::new(),
                    loading: false,
                    selected: 0,
                    filter: TextInput::new(),
                    searching: false,
                    editing_path: false,
                    path_input: TextInput::new(),
                    tab_completions: Vec::new(),
                    tab_index: 0,
                },
            ),
            (
                "AddProjectFailed",
                PromptState::AddProjectFailed {
                    message: "nope".to_string(),
                    return_prompt: Box::new(PromptState::None),
                    scroll: 0,
                },
            ),
            (
                "ConfirmInitRepo",
                PromptState::ConfirmInitRepo {
                    path: "/tmp/x".to_string(),
                    name: "x".to_string(),
                    candidates: Vec::new(),
                    focus: ConfirmFocus::Cancel,
                    return_prompt: Box::new(PromptState::None),
                },
            ),
            (
                "ChangeAgentProvider",
                PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
                    session_id: "s1".to_string(),
                    tab_id: "s1".to_string(),
                    session_label: "my cool agent".to_string(),
                    worktree_path: "/tmp/wt".to_string(),
                    options: vec![ChangeAgentProviderOption {
                        provider: ProviderKind::new("claude"),
                        supports_resume: true,
                        resume_available: false,
                        is_current: true,
                    }],
                    selected: 0,
                    mode: ChangeAgentProviderMode::Retarget,
                }),
            ),
            (
                "AgentInfo",
                PromptState::AgentInfo(AgentInfoPrompt {
                    session_label: "my cool agent".to_string(),
                    lines: vec![("Name: agent".to_string(), AgentInfoTone::Neutral)],
                }),
            ),
            (
                "FirstLoad",
                PromptState::FirstLoad(FirstLoadPrompt {
                    focus: FirstLoadButton::Primary,
                    ..FirstLoadPrompt::welcome(
                        dux_core::welcome_screen::welcome_screen(&app.engine.paths.config_path),
                        false,
                    )
                }),
            ),
            (
                "ChangeDefaultProvider",
                PromptState::ChangeDefaultProvider(ChangeDefaultProviderPrompt {
                    current: ProviderKind::new("claude"),
                    options: vec![ChangeDefaultProviderOption {
                        provider: ProviderKind::new("claude"),
                        is_current: true,
                    }],
                    selected: 0,
                }),
            ),
            (
                "ChangeProjectDefaultProvider",
                PromptState::ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    current: ProviderKind::new("claude"),
                    global_default: ProviderKind::new("claude"),
                    inherits_global_default: true,
                    options: vec![ChangeProjectDefaultProviderOption {
                        provider: None,
                        is_current: true,
                    }],
                    selected: 0,
                }),
            ),
            (
                "SetTailscaleMode",
                PromptState::SetTailscaleMode(crate::app::SetTailscaleModePrompt {
                    current: dux_core::config::TailscaleMode::Auto,
                    options: vec![crate::app::SetTailscaleModeOption {
                        mode: dux_core::config::TailscaleMode::Auto,
                        is_current: true,
                    }],
                    selected: 0,
                    serving: false,
                }),
            ),
            (
                "ChangeTheme",
                PromptState::ChangeTheme(ChangeThemePrompt {
                    options: crate::theme::discover_available(&app.engine.paths),
                    selected: 0,
                    current: "dux-dark".to_string(),
                }),
            ),
            (
                "ConfigureStartupCommand",
                PromptState::ConfigureStartupCommand {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    input: TextInput::with_text("npm install".to_string()).with_multiline(6),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            (
                "ConfigureProjectEnv",
                PromptState::ConfigureProjectEnv {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    input: TextInput::with_text("K=V".to_string()).with_multiline(8),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            (
                "ConfigureGlobalEnv",
                PromptState::ConfigureGlobalEnv {
                    project_name: "All projects".to_string(),
                    input: TextInput::with_text("K=V".to_string()).with_multiline(8),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            (
                "StartupCommandLogs",
                PromptState::StartupCommandLogs(StartupCommandLogPrompt {
                    scope_label: "my cool project".to_string(),
                    entries: Vec::new(),
                    selected: 0,
                    filter: TextInput::new(),
                    searching: false,
                    content: String::new(),
                    scroll_offset: 0,
                    wrap_width: 0,
                    focus: StartupCommandLogFocus::List,
                }),
            ),
            (
                "PickProject",
                PromptState::PickProject {
                    intent: ProjectChooserIntent::NewAgent,
                    entries: Vec::new(),
                    list: SearchableList::new(),
                },
            ),
            (
                "PickProjectWorktree",
                PromptState::PickProjectWorktree(PickProjectWorktreePrompt {
                    project: project.clone(),
                    entries: Vec::new(),
                    loading: false,
                    selected: None,
                    error: None,
                }),
            ),
            (
                "ManageWorktrees",
                PromptState::ManageWorktrees(manage_worktrees_prompt(&project)),
            ),
            (
                "ConfirmDeleteWorktree",
                PromptState::ConfirmDeleteWorktree(Box::new(ConfirmDeleteWorktreePrompt {
                    previous: manage_worktrees_prompt(&project),
                    project: project.clone(),
                    path: PathBuf::from("/tmp/worktrees/demo/free"),
                    label: "free".to_string(),
                    branch: Some("free".to_string()),
                    dirty: false,
                    delete_branch: true,
                    focus: DeleteWorktreeFocus::Cancel,
                })),
            ),
            (
                "KillRunning",
                PromptState::KillRunning(kill_running_prompt()),
            ),
            (
                "ConfirmKillRunning",
                PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
                    previous: kill_running_prompt(),
                    action: KillRunningAction::Visible,
                    target_ids: Vec::new(),
                    focus: ConfirmFocus::Cancel,
                }),
            ),
            (
                "ConfigReloadFailed",
                PromptState::ConfigReloadFailed {
                    error: "bad toml".to_string(),
                    recover_old_config: false,
                    focus: ConfigReloadFailedFocus::Close,
                    scroll: 0,
                },
            ),
            (
                "ConfirmDeleteAgent",
                PromptState::ConfirmDeleteAgent {
                    delete_branch: false,
                    unpushed_commits: None,
                    session_id: "s1".to_string(),
                    agent_label: "my cool agent".to_string(),
                    target: crate::app::DeleteAgentTarget::Managed {
                        branch_name: "b".to_string(),
                        initial_branch: "wt-branch".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_shared: false,
                    },
                    focus: DeleteAgentFocus::Cancel,
                    delete_worktree: false,
                },
            ),
            (
                "ConfirmDeleteTerminal",
                PromptState::ConfirmDeleteTerminal {
                    terminal_id: "t1".to_string(),
                    terminal_label: "My Cool Terminal".to_string(),
                    foreground_cmd: None,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmCloseTab",
                PromptState::ConfirmCloseTab {
                    session_id: "s1".to_string(),
                    tab_id: "t1".to_string(),
                    provider_label: "Claude Code".to_string(),
                    promoted_label: Some("Codex Two".to_string()),
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmDetachAgent",
                PromptState::ConfirmDetachAgent {
                    session_id: "s1".to_string(),
                    label: "my cool agent".to_string(),
                    grace_seconds: 30,
                    live_tabs: 1,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmRecreateWorkingCopy",
                PromptState::ConfirmRecreateWorkingCopy {
                    session_id: "s1".to_string(),
                    worktree_path: std::path::PathBuf::from("/tmp/worktrees/repo/feat"),
                    branch_name: "feat".to_string(),
                    source_branch: "main".to_string(),
                    conversation_resumes: true,
                    running_providers: vec!["claude".to_string()],
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmCheckoutDefaultBranch",
                PromptState::ConfirmCheckoutDefaultBranch {
                    project_id: "p1".to_string(),
                    project_name: "My Cool Project".to_string(),
                    stored_base: Some("develop".to_string()),
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmDeleteProject",
                PromptState::ConfirmDeleteProject {
                    project_id: "p1".to_string(),
                    project_name: "My Cool Project".to_string(),
                    agent_count: 2,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmRemoveProject",
                PromptState::ConfirmRemoveProject {
                    project_id: "p1".to_string(),
                    project_name: "My Cool Project".to_string(),
                    agent_count: 0,
                    orphaned: false,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmQuit",
                PromptState::ConfirmQuit {
                    agent_count: 1,
                    terminal_count: 0,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmDiscardFile",
                PromptState::ConfirmDiscardFile {
                    file_path: "my notes.txt".to_string(),
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmCreateInitialCommit",
                PromptState::ConfirmCreateInitialCommit {
                    path: "/tmp/x".to_string(),
                    name: "x".to_string(),
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "RenameSession",
                PromptState::RenameSession {
                    session_id: "s1".to_string(),
                    input: TextInput::with_text("name".to_string()),
                    rename_branch: false,
                    focus: RenameSessionFocus::Input,
                    branch_named: true,
                },
            ),
            (
                "PullRequestInput",
                PromptState::PullRequestInput {
                    focus: crate::app::PullRequestInputFocus::Input,
                    project: Some(project.clone()),
                    input: TextInput::new(),
                },
            ),
            (
                "AttachPullRequestInput",
                PromptState::AttachPullRequestInput {
                    session_id: "s1".to_string(),
                    current_pr: Some("#42 (open) Fix the frobnicator".to_string()),
                    input: TextInput::new(),
                },
            ),
            (
                "NameStandaloneAgent",
                PromptState::NameStandaloneAgent {
                    folder: "/home/ada/my notes".to_string(),
                    input: TextInput::new(),
                },
            ),
            (
                "NameNewAgent",
                PromptState::NameNewAgent {
                    request: new_project_request(&project),
                    input: TextInput::new(),
                    randomize_name: false,
                    randomized_name: None,
                    copy_changes: false,
                    focus: NameNewAgentFocus::Input,
                },
            ),
            (
                "PickEditor",
                PromptState::PickEditor {
                    session_label: "my cool agent".to_string(),
                    worktree_path: "/tmp/wt".to_string(),
                    editors: Vec::new(),
                    selected: 0,
                },
            ),
            (
                "EditMacros(list)",
                PromptState::EditMacros {
                    entries: vec![(
                        "m1".to_string(),
                        "hello".to_string(),
                        crate::config::MacroSurface::Both,
                    )],
                    selected: 0,
                    editing: None,
                    pending_delete: None,
                },
            ),
            (
                "EditMacros(editor)",
                PromptState::EditMacros {
                    entries: Vec::new(),
                    selected: 0,
                    editing: Some(macro_edit_state()),
                    pending_delete: None,
                },
            ),
            (
                "EditMacros(delete-confirm)",
                PromptState::EditMacros {
                    entries: Vec::new(),
                    selected: 0,
                    editing: None,
                    pending_delete: Some(PendingMacroDelete {
                        name: "m1".to_string(),
                        focus: ConfirmFocus::Cancel,
                    }),
                },
            ),
            (
                "ConfirmNonDefaultBranch",
                PromptState::ConfirmNonDefaultBranch {
                    add: crate::app::PendingProjectAdd {
                        path: project.path.clone(),
                        name: project.name.clone(),
                    },
                    current_branch: "feature".to_string(),
                    kind: BranchWarningKind::Known {
                        default_branch: "main".to_string(),
                    },
                    focus: ConfirmNonDefaultBranchFocus::Cancel,
                    checkout_default: false,
                },
            ),
            (
                "ConfirmUseExistingBranch",
                PromptState::ConfirmUseExistingBranch {
                    request: new_project_request(&project),
                    branch_name: "b".to_string(),
                    location: crate::git::BranchLocation::Local,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "DebugInput",
                PromptState::DebugInput {
                    lines: Vec::new(),
                    scroll_offset: 0,
                },
            ),
            (
                "ResourceMonitor",
                PromptState::ResourceMonitor {
                    rows: Vec::new(),
                    scroll_offset: 0,
                    selected_row: 0,
                    expanded: HashSet::new(),
                    last_refresh: Instant::now(),
                    short_window_sample: false,
                },
            ),
        ]
    }

    /// Render one prompt and report what it published as its mouse layout.
    fn render_and_capture(app: &mut App, prompt: PromptState) -> OverlayMouseLayout {
        app.prompt = prompt;
        let backend = TestBackend::new(160, 60);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        app.overlay_layout.active
    }

    /// The variant a prompt is, by name: `Debug` prints it first.
    fn variant_name(prompt: &PromptState) -> String {
        format!("{prompt:?}")
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect()
    }

    /// Every `PromptState` variant, read off the enum's own source, so a
    /// variant added there is counted here without anyone listing it.
    fn declared_variants() -> BTreeSet<String> {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub(crate) enum PromptState {\n")
            .expect("the PromptState enum");
        let body = &source[start..];
        let body =
            &body[body.find('\n').expect("enum line") + 1..body.find("\n}\n").expect("enum end")];
        body.lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("    ")?;
                if rest.starts_with(' ') || rest.starts_with('/') || rest.starts_with('#') {
                    return None;
                }
                let name: String = rest
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                name.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
                    .then_some(name)
            })
            .collect()
    }

    /// `every_prompt` is the fixture every structural guard below drives, so it
    /// must hold every variant a user can see. A new `PromptState` variant with
    /// no fixture fails here, before any guard can silently skip it.
    #[test]
    fn every_prompt_covers_every_variant() {
        let app = test_app(default_bindings());
        let covered: BTreeSet<String> = every_prompt(&app)
            .iter()
            .map(|(name, prompt)| {
                assert!(
                    name.starts_with(&variant_name(prompt)),
                    "the fixture labelled {name:?} is a {:?}",
                    variant_name(prompt)
                );
                variant_name(prompt)
            })
            .collect();
        let mut declared = declared_variants();
        assert!(declared.len() > 20, "the enum scan found {declared:?}");
        assert!(declared.remove("None"), "None is a variant, and no modal");
        assert_eq!(
            covered, declared,
            "every PromptState variant needs an entry in every_prompt"
        );
    }

    /// Quoted runs a dialog may still show, with the reason each is not a
    /// name: (variant, the quoted run).
    const QUOTED_RUNS_ALLOWED: &[(&str, &str)] = &[];

    /// Every straight-quoted run on screen that is not on the same screen with
    /// no dialog open.
    fn quoted_runs(screen: &str, baseline: &str) -> Vec<String> {
        let runs = |text: &str| -> Vec<String> {
            text.lines()
                .flat_map(|row| {
                    let parts: Vec<&str> = row.split('"').collect();
                    parts
                        .iter()
                        .enumerate()
                        .filter(|(index, part)| {
                            index % 2 == 1 && index + 1 < parts.len() && !part.trim().is_empty()
                        })
                        .map(|(_, part)| format!("\"{part}\""))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let before = runs(baseline);
        runs(screen)
            .into_iter()
            .filter(|run| !before.contains(run))
            .collect()
    }

    fn painted(app: &mut App, prompt: PromptState) -> String {
        app.prompt = prompt;
        let backend = TestBackend::new(160, 60);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buf = terminal.backend().buffer().clone();
        let width = usize::from(buf.area.width);
        buf.content()
            .iter()
            .map(|cell| cell.symbol().to_string())
            .collect::<Vec<_>>()
            .chunks(width)
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The terminal UI's half of the chip rule's drift guard: every dialog, as
    /// the registry's fixtures build it, is painted and read back, and a name in
    /// straight quotes anywhere in it fails, whichever dialog it is. A new
    /// dialog is covered the moment it has a fixture, which the test above
    /// demands.
    #[test]
    fn no_dialog_quotes_a_name() {
        let mut app = test_app(default_bindings());
        let baseline = painted(&mut app, PromptState::None);
        let mut offenders = Vec::new();
        for (name, prompt) in every_prompt(&app) {
            let screen = painted(&mut app, prompt);
            for run in quoted_runs(&screen, &baseline) {
                if !QUOTED_RUNS_ALLOWED.contains(&(name, run.as_str())) {
                    offenders.push(format!("{name}: {run}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these dialogs quote a name instead of drawing it as a chip:\n{}",
            offenders.join("\n")
        );
    }

    /// Paint `prompt` into a `width` x `height` buffer.
    fn painted_buffer(
        app: &mut App,
        prompt: PromptState,
        width: u16,
        height: u16,
    ) -> ratatui::buffer::Buffer {
        app.prompt = prompt;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        terminal.backend().buffer().clone()
    }

    /// Paint `prompt` twice: as the theme draws it, and as a probe with the
    /// text-input caret recolored to colors no chip uses. A cell whose colors
    /// differ between the two is the caret's, whatever colors the theme gave
    /// it; a chip's colors come from the body tokens and never move.
    fn painted_with_caret_probe(
        app: &mut App,
        prompt: PromptState,
        width: u16,
        height: u16,
    ) -> (ratatui::buffer::Buffer, ratatui::buffer::Buffer) {
        use ratatui::style::Color;
        let buf = painted_buffer(app, prompt.clone(), width, height);
        let theme = app.theme;
        app.theme.input_cursor_fg = Color::Rgb(0x12, 0x34, 0x56);
        app.theme.input_cursor_bg = Color::Rgb(0x65, 0x43, 0x21);
        let chip = theme.name_style();
        assert!(
            chip.fg != Some(app.theme.input_cursor_fg)
                && chip.bg != Some(app.theme.input_cursor_bg),
            "the probe's caret colors must not be the chip's"
        );
        let probe = painted_buffer(app, prompt, width, height);
        app.theme = theme;
        (buf, probe)
    }

    /// Every chip-colored run the dialog added to a row that begins mid-name,
    /// as `(row, run)`. A cell is chip-colored when it carries the chip's
    /// foreground AND background in `buf` and still does in `probe` (the same
    /// dialog painted with the caret recolored, see
    /// [`painted_with_caret_probe`]): several themes paint a text-input caret
    /// in exactly the chip's two colors, and the probe is what tells that
    /// caret from a name. A chip opens on its pad space, so a run that does
    /// not is the continuation of a name the wrap cut across rows. A run that
    /// opens on its pad but ends early is a name clipped by the edge of a row
    /// that does not wrap, which is a different question. Cells already
    /// chip-colored with no dialog open belong to the screen behind it.
    fn split_chips(
        buf: &ratatui::buffer::Buffer,
        probe: &ratatui::buffer::Buffer,
        baseline: &ratatui::buffer::Buffer,
        chip: (ratatui::style::Color, ratatui::style::Color),
    ) -> Vec<(u16, String)> {
        let area = buf.area;
        let mut found = Vec::new();
        for y in 0..area.height {
            let mut x = 0;
            while x < area.width {
                let is_chip = |x: u16| {
                    (buf[(x, y)].fg, buf[(x, y)].bg) == chip
                        && (probe[(x, y)].fg, probe[(x, y)].bg) == chip
                        && baseline[(x, y)] != buf[(x, y)]
                };
                if !is_chip(x) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < area.width && is_chip(x) {
                    x += 1;
                }
                let run: String = (start..x).map(|cx| buf[(cx, y)].symbol()).collect();
                if !run.starts_with(' ') {
                    found.push((y, run));
                }
            }
        }
        found
    }

    /// The chip rule's other half: a name is one unit, so no dialog may cut
    /// one across rows, at any width where the name fits on a row at all. The
    /// fixtures carry multi-word names where a name is free text, because a
    /// space inside a name is exactly where a word wrap would cut it.
    #[test]
    fn no_dialog_splits_a_chip_across_rows() {
        let mut offenders = Vec::new();
        // github_light paints its text-input caret in exactly the chip's two
        // colors, so it is where a caret could pass for half a chip.
        for theme in [None, Some("github_light")] {
            let mut app = test_app(default_bindings());
            if let Some(id) = theme {
                app.theme = crate::theme::load(id, &app.engine.paths).expect("theme loads");
            }
            let theme = theme.unwrap_or("default");
            app.engine.projects[0].name = "My Cool Project".to_string();
            let chip_style = app.theme.name_style();
            let chip = (
                chip_style.fg.expect("the chip names its text color"),
                chip_style.bg.expect("the chip names its background"),
            );
            for width in 44..=100u16 {
                let baseline = painted_buffer(&mut app, PromptState::None, width, 40);
                for (name, prompt) in every_prompt(&app) {
                    let (buf, probe) = painted_with_caret_probe(&mut app, prompt, width, 40);
                    for (row, run) in split_chips(&buf, &probe, &baseline, chip) {
                        offenders.push(format!(
                            "{name} ({theme}) at width {width}, row {row}: {run:?}"
                        ));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these dialogs cut a name across rows:\n{}",
            offenders.join("\n")
        );
    }

    /// No dialog paints text in the host terminal's default foreground. That
    /// default is whatever the terminal was configured with (white, in a dark
    /// terminal), so on a light theme's modal surface it is unreadable: text on
    /// the surface takes a theme color, the body's own `text_fg` when nothing
    /// more specific applies. Asked on a light theme, where the failure shows.
    #[test]
    fn no_dialog_paints_text_in_the_terminal_default_foreground() {
        let mut app = test_app(default_bindings());
        app.theme = crate::theme::load("github_light", &app.engine.paths).expect("github_light");
        let surface = app.theme.overlay_bg;
        let (width, height) = (100, 40);
        let baseline = painted_buffer(&mut app, PromptState::None, width, height);
        let mut offenders = Vec::new();
        for (name, prompt) in every_prompt(&app) {
            let buf = painted_buffer(&mut app, prompt, width, height);
            for y in 0..height {
                let row: String = (0..width)
                    .filter(|&x| {
                        let cell = &buf[(x, y)];
                        cell != &baseline[(x, y)]
                            && cell.bg == surface
                            && cell.fg == ratatui::style::Color::Reset
                            && !cell.symbol().trim().is_empty()
                    })
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect();
                if !row.is_empty() {
                    offenders.push(format!("{name}, row {y}: {row:?}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these dialogs paint text in the terminal's default foreground:\n{}",
            offenders.join("\n")
        );
    }

    /// The split scan on the shapes it must catch and the ones it must not.
    #[test]
    fn the_split_chip_scan_tells_a_whole_chip_from_half_of_one() {
        use ratatui::buffer::Buffer;
        use ratatui::style::{Color, Style};
        let colors = (Color::Rgb(4, 5, 6), Color::Rgb(1, 2, 3));
        let chip = Style::default().fg(colors.0).bg(colors.1);
        let blank = Buffer::empty(Rect::new(0, 0, 12, 2));
        let mut buf = blank.clone();
        buf.set_string(0, 0, "a ", Style::default());
        buf.set_string(2, 0, " My Cool ", chip);
        assert!(
            split_chips(&buf, &buf, &blank, colors).is_empty(),
            "a whole chip"
        );
        let mut clipped = blank.clone();
        clipped.set_string(8, 0, " My ", chip);
        clipped.set_string(10, 1, " M", chip);
        assert!(
            split_chips(&clipped, &clipped, &blank, colors).is_empty(),
            "a chip clipped by the row's edge is not a split"
        );
        let mut cut = blank.clone();
        cut.set_string(9, 0, " My", chip);
        cut.set_string(0, 1, "Cool ", chip);
        assert_eq!(
            split_chips(&cut, &cut, &blank, colors),
            vec![(1, "Cool ".to_string())],
            "the continuation row is what proves the cut"
        );
        assert!(
            split_chips(&cut, &cut, &cut, colors).is_empty(),
            "chip colors already on screen with no dialog open are not the dialog's"
        );
        let mut caret = blank.clone();
        caret.set_string(0, 0, "ab", Style::default());
        caret.set_string(1, 0, "b", chip);
        let mut recolored = caret.clone();
        recolored.set_string(1, 0, "b", Style::default().fg(Color::Black));
        assert!(
            split_chips(&caret, &recolored, &blank, colors).is_empty(),
            "a caret in the chip's colors is not half a chip once the probe moves it"
        );
        assert_eq!(
            split_chips(&caret, &caret, &blank, colors),
            vec![(0, "b".to_string())],
            "without the probe that caret reads as a cut chip"
        );
    }

    /// On a theme whose text-input caret is painted in exactly the chip's two
    /// colors, a real dialog with its caret on a letter mid-name is not half a
    /// chip: the caret's lone cell opens on a letter, as a cut chip's
    /// continuation does, so only knowing which cells are the caret's tells
    /// them apart.
    #[test]
    fn the_split_chip_scan_passes_a_caret_in_the_chip_colors() {
        let mut app = test_app(default_bindings());
        app.theme = crate::theme::load("github_light", &app.engine.paths).expect("github_light");
        let chip_style = app.theme.name_style();
        let chip = (
            chip_style.fg.expect("the chip names its text color"),
            chip_style.bg.expect("the chip names its background"),
        );
        assert_eq!(
            (app.theme.input_cursor_fg, app.theme.input_cursor_bg),
            chip,
            "github_light no longer paints its caret in the chip colors; pick a theme that does"
        );
        let mut input = TextInput::with_text("name".to_string());
        input.cursor = 1;
        let prompt = PromptState::RenameSession {
            session_id: "s1".to_string(),
            input,
            rename_branch: false,
            focus: RenameSessionFocus::Input,
            branch_named: true,
        };
        let (width, height) = (80, 30);
        let baseline = painted_buffer(&mut app, PromptState::None, width, height);
        let (buf, probe) = painted_with_caret_probe(&mut app, prompt, width, height);
        let caret: Vec<(u16, u16)> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let cell = &buf[(x, y)];
                (cell.fg, cell.bg) == chip && cell.symbol() == "a"
            })
            .collect();
        assert_eq!(caret.len(), 1, "the caret sits on the one letter a");
        assert!(split_chips(&buf, &probe, &baseline, chip).is_empty());
    }

    /// The scan itself, on the shapes it has to catch and the ones it must not.
    #[test]
    fn the_quoted_run_scan_finds_quoted_names_only() {
        assert_eq!(
            quoted_runs("│ Delete \"feat/x\" now? │", ""),
            vec!["\"feat/x\"".to_string()]
        );
        assert_eq!(
            quoted_runs("│ a \"b\" c \"d e\" │", ""),
            vec!["\"b\"".to_string(), "\"d e\"".to_string()]
        );
        assert!(quoted_runs("│ Delete  feat/x  now? │", "").is_empty());
        assert!(quoted_runs("│ a lone \" quote │", "").is_empty());
        assert!(
            quoted_runs("│ \"x\" │", "│ \"x\" │").is_empty(),
            "a run already on screen with no dialog open is not the dialog's"
        );
    }

    /// The registry's `multiline_field` claim, checked against a real instance
    /// of every variant. Nothing here trusts the table.
    #[test]
    fn the_multiline_claim_matches_a_real_instance_of_every_variant() {
        let app = test_app(default_bindings());
        for (name, prompt) in every_prompt(&app) {
            let spec = modal_spec(&prompt).unwrap_or_else(|| panic!("{name} has no spec"));
            assert_eq!(
                spec.multiline_field,
                prompt_has_multiline_field(&prompt),
                "{name}: the table claims multiline_field = {}, but the built value disagrees",
                spec.multiline_field
            );
        }
    }

    /// The registry's `confirm_button` claim, checked by actually PAINTING each
    /// variant and reading the rects it published. A claim about a button that
    /// nothing renders would otherwise sail through review.
    #[test]
    fn the_confirm_button_claim_matches_what_every_variant_renders() {
        let mut app = test_app(default_bindings());
        for (name, prompt) in every_prompt(&app) {
            let spec = modal_spec(&prompt).unwrap_or_else(|| panic!("{name} has no spec"));
            let layout = render_and_capture(&mut app, prompt);
            assert_eq!(
                spec.confirm_button,
                layout_publishes_confirm_button(&layout),
                "{name}: the table claims confirm_button = {}, but it rendered {layout:?}",
                spec.confirm_button
            );
        }
    }

    /// EXACTLY the modals named in `KNOWN_DUAL_MODE_VIOLATIONS` break the
    /// dual-mode rule, and nothing else does. That list is currently EMPTY, so
    /// this asserts that no modal breaks the rule at all. Asserting the set
    /// rather than a count or a subset is what makes a new violator impossible
    /// to add without writing its name there and defending it in review, and
    /// what makes fixing one force its name back out.
    #[test]
    fn exactly_the_known_violators_break_the_dual_mode_rule() {
        let app = test_app(default_bindings());
        let mut violators: Vec<&'static str> = every_prompt(&app)
            .into_iter()
            .filter_map(|(name, prompt)| {
                let spec = modal_spec(&prompt)?;
                (!spec.satisfies_dual_mode_rule()).then_some(name)
            })
            .collect();
        violators.sort_unstable();
        let mut expected = KNOWN_DUAL_MODE_VIOLATIONS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            violators, expected,
            "the dual-mode violator set changed; update KNOWN_DUAL_MODE_VIOLATIONS \
             (it should only ever SHRINK)"
        );
    }

    /// The macro editor is the reference for a COMPLIANT dual-mode modal, and
    /// it is the state-dependent half of the `EditMacros` decision. Both facts
    /// are checked against real, rendered values.
    #[test]
    fn the_macro_editor_is_a_compliant_dual_mode_modal() {
        let mut app = test_app(default_bindings());
        let editor = PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: Some(macro_edit_state()),
            pending_delete: None,
        };
        let spec = modal_spec(&editor).expect("spec");
        assert!(spec.multiline_field);
        assert!(prompt_has_multiline_field(&editor));
        let layout = render_and_capture(&mut app, editor);
        assert!(layout_publishes_confirm_button(&layout));
        assert!(spec.satisfies_dual_mode_rule());
    }

    #[test]
    fn the_ladder_runs_close_then_move_then_act_then_fall_through() {
        assert_eq!(
            modal_key_step(Some(Action::CloseOverlay), key(KeyCode::Esc), false),
            ModalKeyStep::Close
        );
        assert_eq!(
            modal_key_step(Some(Action::ToggleSelection), key(KeyCode::Tab), false),
            ModalKeyStep::MoveFocus(true)
        );
        assert_eq!(
            modal_key_step(Some(Action::ToggleSelection), key(KeyCode::BackTab), false),
            ModalKeyStep::MoveFocus(false)
        );
        assert_eq!(
            modal_key_step(Some(Action::ToggleSelection), key(KeyCode::Left), false),
            ModalKeyStep::MoveFocus(false)
        );
        assert_eq!(
            modal_key_step(Some(Action::Confirm), key(KeyCode::Enter), false),
            ModalKeyStep::Confirm
        );
        assert_eq!(
            modal_key_step(None, key(KeyCode::Char('x')), true),
            ModalKeyStep::FallThroughToField
        );
    }

    #[test]
    fn space_acts_on_a_focused_button_and_types_into_a_focused_field() {
        assert_eq!(
            modal_key_step(None, key(KeyCode::Char(' ')), false),
            ModalKeyStep::ActivateFocus
        );
        assert_eq!(
            modal_key_step(None, key(KeyCode::Char(' ')), true),
            ModalKeyStep::FallThroughToField
        );
    }

    #[test]
    fn the_field_gate_only_closes_while_a_field_has_focus() {
        // The shipped bug: Left is in the movement action's default key set.
        assert!(binding_lookup_is_suppressed(key(KeyCode::Left), true));
        assert!(binding_lookup_is_suppressed(key(KeyCode::Char('a')), true));
        // With focus on a checkbox the field owns nothing, so movement works.
        assert!(!binding_lookup_is_suppressed(key(KeyCode::Left), false));
        // Tab is never owned by the field, so it stays a focus key in both.
        assert!(!binding_lookup_is_suppressed(key(KeyCode::Tab), true));
    }
}
