//! The modal registry: what kind of thing each modal IS, and what it owes.
//!
//! dux has about thirty-five modals. Left to itself, each one grows its own
//! key handling, its own focus concept, and its own idea of what Enter means,
//! and the drift is invisible until a user hits it. This module is the place
//! that stops that, and it is deliberately built the same way
//! [`super::overlay_dismiss::outside_click_policy`] is, an EXHAUSTIVE match
//! with no `_` arm, because that is the one anti-drift device in this codebase
//! that has actually held. A new [`PromptState`] variant does not compile until
//! someone has answered, on purpose, "what family is this, and does it need a
//! confirm button?".
//!
//! # The four families
//!
//! Derived from what a keystroke MEANS in the modal, not from how it looks:
//!
//! | Family | Shape | Up/Down | Left/Right | Enter | Space |
//! |---|---|---|---|---|---|
//! | [`ModalFamily::Report`] | read-only prose, scrollable | scroll the body |, | dismiss | scroll/dismiss |
//! | [`ModalFamily::Confirm`] | prose, two buttons, maybe a checkbox |, | move focus between controls | act on the focused control | act on the focused control |
//! | [`ModalFamily::Picker`] | rows with a selection cursor, maybe a filter | move the SELECTION (a value, not focus) |, | pick the selected row | typed into the filter |
//! | [`ModalFamily::Form`] | fields plus buttons |, | belongs to the CARET; never reaches the binding lookup | see the dual-mode rule below | typed when a field has focus |
//!
//! The two easy-to-blur distinctions are worth stating outright. A Picker's
//! Up/Down changes a VALUE (which row is selected); a Confirm's Left/Right
//! changes FOCUS and nothing else, wiring a movement key straight to a value
//! is the bug the "movement keys move focus" tenet exists to prevent. And a
//! Form's horizontal arrows belong to the text caret, which is why
//! [`crate::keybindings::text_field_owns_key`] has to gate the binding lookup
//! there (see [`binding_lookup_is_suppressed`]).
//!
//! # What this registry actually enforces, and what it only documents
//!
//! Be precise about this, because the difference decides how much a green
//! suite is worth.
//!
//! **Genuinely enforced by the compiler:** the exhaustive match in
//! [`modal_spec`] (and the one in [`prompt_text_inputs`]). Adding a
//! [`PromptState`] variant is a build error until somebody classifies it. That
//! gate holds whether or not any code reads the result, and it is the reason
//! every item in this module carries `#[allow(dead_code)]` rather than being
//! deleted.
//!
//! **Enforced only by the guard tests below:** everything else. [`ModalSpec`],
//! [`ModalFamily`], [`KNOWN_DUAL_MODE_VIOLATIONS`] and
//! [`ModalSpec::satisfies_dual_mode_rule`] are read by `mod tests` and by
//! nothing on the render or input path. `cargo test` is what catches a family
//! misdeclared or a dual-mode violation, so those checks are as strong as the
//! fixtures in `every_prompt` are complete, and no stronger.
//!
//! **Not enforced at all:** the four families are a DESCRIPTION of what a
//! keystroke should mean. No dispatcher consults `spec.family` before routing
//! a key. A modal declared `Report` whose handler moves a selection cursor
//! compiles, renders and ships; only a human reading both halves will notice.
//! Write the family down honestly, and when you change a modal's key
//! behaviour, change its declaration in the same edit.
//!
//! # What this registry cannot see
//!
//! **Coverage is `PromptState`, not "every typing surface in dux."** Two real
//! dual-mode text surfaces are NOT `PromptState` variants and are therefore
//! invisible here:
//!
//! * the **commit-message pane** (`App::commit_input`, a `with_multiline(4)`
//!   field living in the files pane), and
//! * the **startup-log viewer** (`App::startup_log_viewer`, a fullscreen
//!   overlay with its own search row).
//!
//! Neither is reachable from any function in this module, and no test here says
//! anything about them. Do not read a green suite as "every modal in dux is
//! covered"; read it as "every `PromptState` variant is covered". If either of
//! those surfaces is ever routed through `PromptState`, it joins the registry
//! automatically, and until then, changing them is unguarded.

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
// Not called from the render or input paths, and that is correct: the
// registry is a DECLARATION whose value is the exhaustive match itself, which
// the compiler checks whether or not anything reads the result. Its consumers
// are the guard tests below and in `render.rs`, and the modal migration phase
// that follows. Do not "clean it up" by deleting an unread arm.
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

/// Everything the registry declares about one open modal.
///
/// Deliberately ONE struct behind ONE match rather than a family match plus a
/// fields match: two exhaustive matches over the same enum are two places to
/// forget, and the whole value of the device is that forgetting is impossible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct ModalSpec {
    /// Which of the four families this modal belongs to.
    pub(crate) family: ModalFamily,
    /// Whether the modal contains a FULL-TEXT (multiline) field.
    ///
    /// Declared here, but not knowable from the type: multiline-ness is a
    /// runtime flag set by [`TextInput::with_multiline`] at construction. This
    /// field is therefore a CLAIM, and `modal_spec_matches_a_real_instance` is
    /// what turns the claim into a guard, by building the variant for real and
    /// asking [`prompt_text_inputs`].
    pub(crate) multiline_field: bool,
    /// Whether the modal publishes a button that COMMITS it (Apply, Save,
    /// Delete, Quit, …) as opposed to one that merely dismisses it (Close, OK).
    ///
    /// Also a claim, checked the same way, by rendering the variant and asking
    /// [`layout_publishes_confirm_button`] what reached
    /// `OverlayMouseLayoutState::active`.
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
    /// With a button, Enter is unambiguous at every moment: it ENGAGES the
    /// field while the field is unengaged, inserts a NEWLINE while it is, and
    /// ACTIVATES whatever the focus is on when focus is on a button. Without
    /// one there is no third meaning for Enter to land on, and the modal has to
    /// invent something.
    ///
    /// A modal with only single-line fields needs no button, because Enter
    /// submits and nothing competes for it.
    ///
    /// This is a DESIGN choice, not a logical necessity, see the note on
    /// [`KNOWN_DUAL_MODE_VIOLATIONS`] and the counterexample recorded there.
    pub(crate) fn satisfies_dual_mode_rule(self) -> bool {
        !self.multiline_field || self.confirm_button
    }
}

/// The modals that break the dual-mode rule today, by title.
///
/// **Empty, and it must stay that way.** It held the three configure modals
/// (startup command, project environment, global environment), each a
/// single-control form with a `with_multiline` field and no button at all;
/// they were given the Cancel/Save pair in the same change that redefined
/// their Enter to ENGAGE the field. **The test asserts this set EXACTLY**, so
/// a new violator cannot be added without writing its name here and defending
/// it in review.
///
/// This list should only ever shrink.
///
/// ---
///
/// The rule these three break is a product decision, and it is worth being
/// honest that it is not forced by logic: dux already ships a dual-mode field
/// with no confirm button whose Enter is perfectly unambiguous. The
/// commit-message pane binds "submit" to a different key entirely, so Enter is
/// only ever a newline there and no third meaning is needed. (It is also not a
/// `PromptState` variant, so it is outside this registry, see the module
/// docs.) The rule is the house style for MODALS, chosen because a modal's
/// Enter is otherwise overloaded, not a claim that no other design can work.
#[allow(dead_code)]
pub(crate) const KNOWN_DUAL_MODE_VIOLATIONS: &[&str] = &[];

/// The registry. `None` means "no modal is open" ([`PromptState::None`]).
///
/// The match is EXHAUSTIVE with no `_` arm, and that is the entire point: a new
/// `PromptState` variant is a compile error here until its family and its two
/// obligations are declared. **Do not add a catch-all arm**, and do not group a
/// new variant into an existing arm without checking that all three answers
/// really are the same.
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

        // The startup-log modal was declared a Report on the same mistake, and
        // for the same reason it was never caught: while nothing outside tests
        // could open it, no user could notice that its keys did not mean what
        // Report says they mean. They never did. It renders a `ListState`
        // cursor over the runs, its vertical keys move that SELECTION (the
        // OUTPUT pane scrolls on the paging keys, which is why the vertical
        // keys are free), it carries a filter, and its confirm key acts on the
        // selection by promoting that run to the fullscreen viewer. Rows plus a
        // selection cursor plus a confirm key that acts on the selection is a
        // Picker. Its one button is a Close, a way out and not a commit, which
        // is why the confirm-button claim stays false.
        PromptState::StartupCommandLogs(_) => ModalSpec::new(Picker, false, false),

        // The resource monitor LOOKS like a report and was declared one, but
        // the declaration did not match the code. It renders a `ListState`
        // selection cursor, its vertical keys move that cursor (a value, not a
        // scroll offset and not focus), and its confirm key acts on the
        // selected row by expanding it. Rows plus a selection cursor plus a
        // confirm key that acts on the selection is this registry's own
        // definition of a Picker, so Picker is what it is. It is the one
        // picker whose confirm key EXPANDS the selected row instead of
        // choosing it and closing, which is a legitimate variation on
        // "Enter acts on the selection", not a different family.
        PromptState::ResourceMonitor { .. } => ModalSpec::new(Picker, false, false),

        // ── Confirm ─────────────────────────────────────────────────────
        PromptState::ConfirmDeleteAgent { .. }
        | PromptState::ConfirmDeleteTerminal { .. }
        | PromptState::ConfirmCloseTab { .. }
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

        // Prose plus ONE button, and that button DISMISSES rather than commits,
        // so `confirm_button` is false. Confirm rather than Report because its
        // body does not scroll and its button is a real focused control:
        // movement keys move focus (trivially, there being one control), Space
        // and Enter press it, Escape abandons.
        PromptState::FirstTabCannotClose { .. } => ModalSpec::new(Confirm, false, false),

        // ── Picker ──────────────────────────────────────────────────────
        // A selection cursor over rows; Enter picks. The filter rows these
        // carry are deliberately type-immediately and are single-line, so no
        // dual-mode question arises.
        //
        // **A picker gets no Cancel and no Apply.** Its footer already names
        // the keys, resolved through the bindings, and a button LABEL cannot
        // stay truthful once a user rebinds. The provider pickers'
        // active-provider cue lives on the row itself (see
        // `render::ACTIVE_PROVIDER_MARKER`), and their keys share one handler,
        // `App::handle_provider_picker_key`, reached through
        // `super::input::provider_picker_kind`.
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

        // Kill-running is the ONE picker that keeps its buttons, and it is not
        // an oversight to finish. Its three footer buttons are DISTINCT ACTIONS
        // (kill the hovered runtime, kill the marked ones, kill everything the
        // filter shows), not a confirm/cancel pair restating what Enter does,
        // so "a picker confirms by picking" says nothing about them. Do not
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
        // registry answers as a FUNCTION OF STATE rather than of the variant.
        // The three arms below must stay ordered most-specific first.
        //
        // The nested delete-confirm paints over whichever of the two is
        // underneath, and is an ordinary confirmation.
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
        // The list underneath: rows with a selection cursor, no buttons. It
        // resolves every key through the bindings and publishes its rows as
        // `OverlayMouseLayout::EditMacroList`, so it is a Picker in behaviour
        // and not only in the table.
        PromptState::EditMacros { .. } => ModalSpec::new(Picker, false, false),
    };
    Some(spec)
}

/// Every [`TextInput`] the open modal owns, so the registry's `multiline_field`
/// claim can be checked against a LIVE instance instead of trusted.
///
/// Exhaustive for the same reason [`modal_spec`] is: a new variant that quietly
/// grows a text field would otherwise sail past the dual-mode check.
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
        | PromptState::FirstTabCannotClose { .. }
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

/// Whether a published mouse layout carries a button that COMMITS the modal.
///
/// The distinction the dual-mode rule turns on is commit versus dismiss: an
/// `ok_button` on an error report or a `close_button` on a log viewer is a way
/// out, not a third meaning for Enter to land on, so neither counts. Exhaustive
/// with no `_` arm, so a new layout variant has to answer the question too.
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
        | OverlayMouseLayout::NameNewAgent { .. }
        // One button, and it is a way out: the modal commits nothing.
        | OverlayMouseLayout::FirstTabCannotClose { .. } => false,

        // A button that commits.
        OverlayMouseLayout::KillRunning { .. }
        | OverlayMouseLayout::ConfirmKillRunning { .. }
        | OverlayMouseLayout::ConfirmDeleteAgent { .. }
        | OverlayMouseLayout::ConfirmDeleteWorktree { .. }
        | OverlayMouseLayout::ConfirmDeleteTerminal { .. }
        | OverlayMouseLayout::ConfirmCloseTab { .. }
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
    /// These three steps open EVERY modal in dux, always in this order and
    /// always together, and two of them are load-bearing in ways a copy can get
    /// wrong. [`App::clear_overlay_area`] is the one chokepoint that records
    /// the topmost modal's rect for the click-outside engine, which FAILS
    /// CLOSED, a modal that clears its area some other way becomes
    /// undismissable by mouse. And [`App::themed_overlay_block`]'s border ring
    /// doubles as the refusal cue for an outside click that is answered with a
    /// blink rather than a close.
    ///
    /// `area` stays the caller's: modals size themselves by percentage, by
    /// exact cells, or by content, and folding that in would mean an enum of
    /// sizing modes with one arm per modal.
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
/// The caller's job on a hit is always the SAME TWO STEPS, in this order:
/// **move focus to that control, then act on it.** Not "act on it" alone, the
/// two surfaces have to agree about where focus is afterwards, or a click
/// leaves the modal's visible focus pointing somewhere the next keystroke will
/// act on instead. (`toggle_rename_session_branch` already does exactly this:
/// it sets `focus` to the checkbox before flipping it.)
///
/// `targets` is the modal's published rects in any order; overlapping rects
/// resolve to the FIRST match, so publish the topmost control first.
pub(crate) fn click_target<T: Copy>(targets: &[(Rect, T)], column: u16, row: u16) -> Option<T> {
    targets
        .iter()
        .find(|(rect, _)| contains_point(*rect, column, row))
        .map(|&(_, target)| target)
}

// ── The key ladder ──────────────────────────────────────────────────────────

/// One rung of the ladder every modal's key handler walks.
///
/// The order is the ladder: close, then move focus, then act on focus, then
/// fall through to whatever text field has focus. Reproduced by hand in a
/// dozen modals today; this is the shape they all have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModalKeyStep {
    /// Dismiss the modal, through its real cancel path (never a bare
    /// `prompt = None`; see [`super::overlay_dismiss`]).
    Close,
    /// Move focus. `true` is forwards.
    MoveFocus(bool),
    /// The confirm key. What it means is the FAMILY's business, and the two
    /// answers really do differ: a `Confirm` or `Picker` modal acts on whatever
    /// has focus, while a `Form` whose fields are all single-line submits the
    /// form no matter which control has focus (that is the whole reason such a
    /// form needs no confirm button, see [`ModalSpec::satisfies_dual_mode_rule`]).
    /// Collapsing this into [`ModalKeyStep::ActivateFocus`] would silently
    /// change the rename-agent modal, where Enter submits while the checkbox
    /// has focus and Space toggles it.
    Confirm,
    /// Space, with focus NOT on a text field: act on the focused control,
    /// activate a button, toggle a checkbox.
    ActivateFocus,
    /// Nobody claimed it. Hand it to the focused text field, if there is one.
    FallThroughToField,
}

/// Whether the binding lookup must be SKIPPED for this key.
///
/// True exactly when a text field has focus and the field owns the key. This is
/// the gate that keeps plain characters and the horizontal arrows away from the
/// bindings, and it is not cosmetic: the movement action's default key set
/// includes the horizontal arrows, so without this gate pressing Left in the
/// rename-agent modal flips the "also rename the git branch" checkbox instead
/// of moving the caret, a shipped bug, and the reason
/// [`text_field_owns_key`] exists.
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
/// `text_field_focused` only affects Space: Space is CONTENT in both kinds of
/// text field, so it may only act on focus when focus is actually sitting on a
/// button or a checkbox. That is the "Space acts on what has focus" tenet,
/// which is about focus and never about the modal merely containing a button.
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

/// Whether a focus-movement key means "backwards".
///
/// The movement action carries no direction of its own, so the key that
/// triggered it supplies one. Mirrors `super::input::focus_move_is_reverse`;
/// kept here so the ladder stays a pure function the tests can drive without an
/// `App`.
fn focus_move_is_reverse(key: KeyEvent) -> bool {
    use ratatui::crossterm::event::KeyModifiers;
    matches!(key.code, KeyCode::BackTab)
        || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
        || matches!(key.code, KeyCode::Left | KeyCode::Char('h'))
}

#[cfg(test)]
mod tests {
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
    use dux_core::worker::{BranchWarningKind, CreateAgentRequest, NonDefaultBranchAction};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::collections::HashSet;
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

    fn every_prompt(app: &App) -> Vec<(&'static str, PromptState)> {
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
                    session_label: "agent".to_string(),
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
                    session_label: "agent".to_string(),
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
                    scope_label: "demo".to_string(),
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
                    session_id: "s1".to_string(),
                    agent_label: "b".to_string(),
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
                    terminal_label: "Terminal 1".to_string(),
                    foreground_cmd: None,
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "ConfirmCloseTab",
                PromptState::ConfirmCloseTab {
                    session_id: "s1".to_string(),
                    tab_id: "t1".to_string(),
                    provider_label: "Claude".to_string(),
                    focus: ConfirmFocus::Cancel,
                },
            ),
            (
                "FirstTabCannotClose",
                PromptState::FirstTabCannotClose {
                    session_id: "s1".to_string(),
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
                    file_path: "a.txt".to_string(),
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
                    folder: "/home/ada/notes".to_string(),
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
                    session_label: "agent".to_string(),
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
                    action: NonDefaultBranchAction::AddProject {
                        path: project.path.clone(),
                        name: project.name.clone(),
                        leading_branch: "main".to_string(),
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
