//! Click-outside modal dismissal.
//!
//! Esc and a click outside a modal are the same intent arriving through two
//! different input devices. They therefore run the SAME code: the mouse path
//! decides *whether* to dismiss (the pure policy in this module) and then hands
//! off to [`App::cancel_prompt`], which routes every variant to the cancel path
//! its Esc key already uses. A bare `self.prompt = PromptState::None` from the
//! mouse path is not acceptable — several modals revert a live theme preview,
//! restore a parent prompt, or stamp a version as seen on dismissal, and a
//! second, parallel "close" would silently skip that and then drift.
//!
//! The geometry comes from [`OverlayMouseLayoutState::frame`], recorded during
//! render because a modal's rect does not survive the frame it was painted in.

use super::input::contains_point;
use super::*;

/// What an outside click should do to the prompt that is currently open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutsideClickPolicy {
    /// Dismiss, through the variant's real cancel path.
    Cancel,
    /// Swallow the click and leave the modal open.
    Ignore,
}

/// True only for a left-button PRESS that landed outside a recorded modal rect.
///
/// Down-only and left-only on purpose:
/// * a press that starts inside the modal and releases outside it is a drag off
///   a control, not a dismissal (that gesture already means "cancel this
///   button"), so `Up`/`Drag` must not dismiss;
/// * a stray right-click is not a dismissal either.
///
/// `None` (no modal painted this frame) is not outside anything — see the
/// fail-closed contract on [`OverlayMouseLayoutState::frame`].
pub(super) fn click_outside_frame(frame: Option<Rect>, mouse: &MouseEvent) -> bool {
    let Some(rect) = frame else {
        return false;
    };
    matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && !contains_point(rect, mouse.column, mouse.row)
}

/// The per-variant outside-click policy.
///
/// The match is EXHAUSTIVE with no `_` arm, and that is the whole anti-drift
/// device: a new modal cannot be added to [`PromptState`] without a compile
/// error here forcing a deliberate decision about its outside-click behaviour.
/// Do not add a catch-all arm.
pub(super) fn outside_click_policy(prompt: &PromptState) -> OutsideClickPolicy {
    use OutsideClickPolicy::{Cancel, Ignore};
    match prompt {
        // Nothing open: nothing to dismiss.
        PromptState::None => Ignore,

        // Informational and pick-one surfaces: an outside click is unambiguous.
        PromptState::AgentInfo(_)
        | PromptState::FirstLoad(_)
        | PromptState::ResourceMonitor { .. }
        | PromptState::DebugInput { .. }
        | PromptState::StartupCommandLogs(_)
        | PromptState::PickEditor { .. }
        | PromptState::PickProjectWorktree(_)
        | PromptState::PickProject { .. }
        | PromptState::ChangeAgentProvider(_)
        | PromptState::ChangeDefaultProvider(_)
        | PromptState::ChangeProjectDefaultProvider(_)
        | PromptState::ChangeTheme(_)
        | PromptState::AddProjectFailed { .. }
        | PromptState::ConfigReloadFailed { .. }
        | PromptState::Command { .. } => Cancel,

        // Confirmations. Dismissal IS cancel here, and every one of these
        // already defaults its focus to Cancel, so an outside click resolves
        // them the same way the safe default button does.
        PromptState::ConfirmDeleteAgent { .. }
        | PromptState::ConfirmDeleteTerminal { .. }
        | PromptState::ConfirmCloseTab { .. }
        | PromptState::ConfirmDiscardFile { .. }
        | PromptState::ConfirmQuit { .. }
        | PromptState::ConfirmKillRunning(_)
        | PromptState::ConfirmInitRepo { .. }
        | PromptState::ConfirmCreateInitialCommit { .. }
        | PromptState::ConfirmNonDefaultBranch { .. }
        | PromptState::ConfirmUseExistingBranch { .. } => Cancel,

        // The macro editor is two modals in one variant. Its nested
        // delete-confirm is a confirmation like any other and dismisses; the
        // editor underneath holds unsaved free text and does not.
        PromptState::EditMacros {
            pending_delete: Some(_),
            ..
        } => Cancel,

        // Everything below holds unsaved free text the user typed, or a
        // multi-step selection they built up, so a stray click must not throw
        // it away.
        //
        // PHASE 2 REPLACES THE SWALLOW WITH A VISUAL REFUSAL (a "blink" of the
        // modal frame) rather than with a dismissal. Do not "simplify" these to
        // `Cancel`: the swallow is the current, deliberate answer, and the
        // planned answer is a cue, not a close.
        PromptState::EditMacros { .. }
        | PromptState::BrowseProjects { .. }
        | PromptState::ConfigureStartupCommand { .. }
        | PromptState::ConfigureProjectEnv { .. }
        | PromptState::ConfigureGlobalEnv { .. }
        | PromptState::RenameSession { .. }
        | PromptState::PullRequestInput { .. }
        | PromptState::NameNewAgent { .. }
        | PromptState::KillRunning(_) => Ignore,
    }
}

impl App {
    /// Cancel the open prompt through the SAME path its Esc key uses, and
    /// report whether anything was cancelled.
    ///
    /// Every arm either calls the variant's existing `resolve_*`/`cancel_*`
    /// helper or reproduces the exact statements its Esc arm runs. The ones
    /// that must not be a bare `prompt = None` are called out inline; blanking
    /// them would leave a previewed theme applied, destroy a parent prompt, or
    /// skip the first-load version stamp.
    ///
    /// DELIBERATELY SETS NO STATUS MESSAGE of its own. The keyboard ladder
    /// announces some dismissals; firing that on every stray click would be
    /// noise for something the user just watched happen. (Helpers that own a
    /// message — the Kill Running ones — keep theirs, since that is their
    /// cancel path and parity with Esc is the point.)
    pub(crate) fn cancel_prompt(&mut self) -> bool {
        // A press cannot outlive the modal it was made in.
        self.pressed_button = None;

        match &self.prompt {
            PromptState::None => return false,

            // Plain closes: these match their Esc arms exactly.
            PromptState::AgentInfo(_)
            | PromptState::ResourceMonitor { .. }
            | PromptState::DebugInput { .. }
            | PromptState::PickEditor { .. }
            | PromptState::PickProjectWorktree(_)
            | PromptState::PickProject { .. }
            | PromptState::ChangeAgentProvider(_)
            | PromptState::ChangeDefaultProvider(_)
            | PromptState::ChangeProjectDefaultProvider(_)
            | PromptState::Command { .. }
            | PromptState::ConfirmNonDefaultBranch { .. } => {
                self.prompt = PromptState::None;
            }

            // Closing the log viewer must also drop the drag-selection state,
            // or a stale selection outlives the modal that owned it.
            PromptState::StartupCommandLogs(_) => {
                self.prompt = PromptState::None;
                self.startup_log_selection = None;
            }

            // Dismissal is what records the running version as seen, so this
            // must go through the stamping helper.
            PromptState::FirstLoad(_) => self.dismiss_first_load_prompt(),

            // Reverts the live preview. A bare close would leave the
            // previewed theme applied.
            PromptState::ChangeTheme(_) => self.cancel_change_theme(),

            // Restores `return_prompt` — the project browser plus the path the
            // user typed. A bare close destroys both.
            PromptState::AddProjectFailed { .. } => {
                self.resolve_add_project_failed();
            }

            // Same resolve path as its Close button (see `handle_prompt_key`,
            // which now also routes Esc here).
            PromptState::ConfigReloadFailed { .. } => {
                self.resolve_config_reload_failed(false);
            }

            // Restores the KillRunning list this confirm was raised from, so
            // cancelling the nested confirm steps back rather than closing the
            // whole stack.
            PromptState::ConfirmKillRunning(_) => {
                self.resolve_confirm_kill_running(false);
            }

            // Restores its `return_prompt` (the browser, as the user left it).
            PromptState::ConfirmInitRepo { .. } => {
                self.resolve_confirm_init_repo(false);
            }

            // Drops the pending add and explains why (its helper's message).
            PromptState::ConfirmCreateInitialCommit { .. } => {
                self.resolve_confirm_create_initial_commit(false);
            }

            // Confirmations: route through the resolve helpers so the
            // not-confirmed branch stays in one place. Their `bool` is "should
            // the app exit", and cancelling never exits, which the caller's
            // `false` return preserves.
            PromptState::ConfirmDeleteAgent { .. } => {
                self.resolve_confirm_delete_agent(false);
            }
            PromptState::ConfirmDeleteTerminal { .. } => {
                self.resolve_confirm_delete_terminal(false);
            }
            PromptState::ConfirmCloseTab { .. } => {
                self.resolve_confirm_close_tab(false);
            }
            PromptState::ConfirmDiscardFile { .. } => {
                self.resolve_confirm_discard_file(false);
            }
            PromptState::ConfirmQuit { .. } => {
                self.resolve_confirm_quit(false);
            }
            PromptState::ConfirmUseExistingBranch { .. } => {
                self.resolve_confirm_use_existing_branch(false);
            }

            // The nested macro delete-confirm: clears the confirm and leaves
            // the macro editor open, exactly as its Esc and Cancel button do.
            PromptState::EditMacros {
                pending_delete: Some(_),
                ..
            } => {
                self.resolve_confirm_delete_macro(false);
            }

            // Not dismissible by an outside click (see `outside_click_policy`).
            // Reached only if a caller ignores the policy, so it is a no-op
            // rather than a surprise close.
            PromptState::EditMacros { .. }
            | PromptState::BrowseProjects { .. }
            | PromptState::ConfigureStartupCommand { .. }
            | PromptState::ConfigureProjectEnv { .. }
            | PromptState::ConfigureGlobalEnv { .. }
            | PromptState::RenameSession { .. }
            | PromptState::PullRequestInput { .. }
            | PromptState::NameNewAgent { .. }
            | PromptState::KillRunning(_) => return false,
        }
        true
    }

    /// The mouse-side entry point: dismiss the open prompt when this event is
    /// an outside click and the prompt's policy says to.
    ///
    /// Called from the ONE place in `handle_prompt_mouse` where the hit-test
    /// has already returned `None`, so it can never preempt a button, a list
    /// row, a checkbox, a text input, or a modal's deliberate blank
    /// misclick-safe spacer row.
    pub(super) fn dismiss_prompt_on_outside_click(&mut self, mouse: &MouseEvent) -> bool {
        if !click_outside_frame(self.overlay_layout.frame.get(), mouse) {
            return false;
        }
        if outside_click_policy(&self.prompt) != OutsideClickPolicy::Cancel {
            return false;
        }
        self.cancel_prompt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::render::centered_rect_exact;
    use crate::app::test_support::{default_bindings, test_app};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const TERM: (u16, u16) = (120, 40);

    /// Every test that exercises a click RENDERS first. A modal's outer rect
    /// exists only as a by-product of a real render, so a test that installs an
    /// overlay layout synthetically (as the older mouse tests do) records no
    /// rect at all and would prove nothing about dismissal either way.
    fn render(app: &mut App) {
        let backend = TestBackend::new(TERM.0, TERM.1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn left_down(column: u16, row: u16) -> MouseEvent {
        mouse(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    fn esc(app: &mut App) {
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .expect("esc");
    }

    /// The recorded modal rect. `expect` on purpose: a test that means to click
    /// relative to a modal must fail loudly if nothing was recorded.
    fn frame_rect(app: &App) -> Rect {
        app.overlay_layout
            .frame
            .get()
            .expect("a rendered modal records its outer rect")
    }

    fn prompt_kind(app: &App) -> std::mem::Discriminant<PromptState> {
        std::mem::discriminant(&app.prompt)
    }

    fn status_text(app: &App) -> Option<String> {
        app.status.most_recent_tui().map(|(_, text)| text)
    }

    fn agent_info_prompt() -> PromptState {
        PromptState::AgentInfo(AgentInfoPrompt {
            session_label: "agent-branch".to_string(),
            lines: vec![("Provider: codex".to_string(), AgentInfoTone::Neutral)],
        })
    }

    fn rename_session_prompt() -> PromptState {
        let mut input = TextInput::new();
        input.set_text("half-typed-name".to_string());
        PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input,
            rename_branch: false,
            focus: RenameSessionFocus::Input,
        }
    }

    fn confirm_quit_prompt() -> PromptState {
        PromptState::ConfirmQuit {
            agent_count: 1,
            terminal_count: 0,
            confirm_selected: false,
        }
    }

    fn edit_macros_with_pending_delete() -> PromptState {
        PromptState::EditMacros {
            entries: vec![(
                "greet".to_string(),
                "hello".to_string(),
                crate::config::MacroSurface::Agent,
            )],
            selected: 0,
            editing: None,
            pending_delete: Some(PendingMacroDelete {
                name: "greet".to_string(),
                confirm_selected: false,
            }),
        }
    }

    // ─────────────────────────── the pure policy ───────────────────────────

    #[test]
    fn click_outside_frame_requires_a_recorded_rect() {
        // Fail-closed: with nothing recorded, no click is "outside" anything.
        assert!(!click_outside_frame(None, &left_down(0, 0)));
    }

    #[test]
    fn click_outside_frame_matches_only_a_left_press_outside_the_rect() {
        let rect = Some(Rect::new(10, 5, 20, 10));

        assert!(click_outside_frame(rect, &left_down(0, 0)));
        assert!(!click_outside_frame(rect, &left_down(15, 8)));
        // A press that starts inside and releases outside is a drag off a
        // control, not a dismissal.
        assert!(!click_outside_frame(
            rect,
            &mouse(MouseEventKind::Up(MouseButton::Left), 0, 0)
        ));
        assert!(!click_outside_frame(
            rect,
            &mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0)
        ));
        // A stray right-click is not a dismissal either.
        assert!(!click_outside_frame(
            rect,
            &mouse(MouseEventKind::Down(MouseButton::Right), 0, 0)
        ));
        assert!(!click_outside_frame(
            rect,
            &mouse(MouseEventKind::ScrollUp, 0, 0)
        ));
    }

    #[test]
    fn policy_covers_the_settled_table() {
        assert_eq!(
            outside_click_policy(&PromptState::None),
            OutsideClickPolicy::Ignore
        );
        assert_eq!(
            outside_click_policy(&agent_info_prompt()),
            OutsideClickPolicy::Cancel
        );
        assert_eq!(
            outside_click_policy(&confirm_quit_prompt()),
            OutsideClickPolicy::Cancel
        );
        assert_eq!(
            outside_click_policy(&rename_session_prompt()),
            OutsideClickPolicy::Ignore
        );
        // The macro editor is Ignore, but the delete-confirm nested inside it
        // is a confirmation and dismisses.
        assert_eq!(
            outside_click_policy(&edit_macros_with_pending_delete()),
            OutsideClickPolicy::Cancel
        );
        assert_eq!(
            outside_click_policy(&PromptState::EditMacros {
                entries: Vec::new(),
                selected: 0,
                editing: None,
                pending_delete: None,
            }),
            OutsideClickPolicy::Ignore
        );
    }

    // ──────────────────────── dismissal, end to end ────────────────────────

    #[test]
    fn outside_click_dismisses_a_cancel_modal() {
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn outside_click_sets_no_status_message() {
        // Deliberate: the keyboard ladder announces some dismissals, but a
        // stray click does not need narrating.
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        render(&mut app);
        let before = status_text(&app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(status_text(&app), before, "the dismissal announced itself");
    }

    #[test]
    fn outside_click_is_ignored_for_an_unsaved_text_modal() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        match &app.prompt {
            PromptState::RenameSession { input, .. } => {
                assert_eq!(input.text, "half-typed-name");
            }
            other => panic!("expected the rename prompt to survive, got {other:?}"),
        }
    }

    #[test]
    fn a_right_click_outside_does_not_dismiss() {
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        render(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 0, 0));

        assert!(matches!(app.prompt, PromptState::AgentInfo(_)));
    }

    #[test]
    fn a_press_inside_released_outside_does_not_dismiss() {
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        render(&mut app);
        let rect = frame_rect(&app);

        // Press on inert padding inside the modal, release far outside it.
        app.handle_mouse(left_down(rect.x + 2, rect.y + 1));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));

        assert!(matches!(app.prompt, PromptState::AgentInfo(_)));
    }

    #[test]
    fn a_click_inside_the_modal_never_dismisses_it() {
        let mut app = test_app(default_bindings());
        app.prompt = confirm_quit_prompt();
        render(&mut app);
        let rect = frame_rect(&app);

        // Inside the border, on no target at all — including the blank
        // misclick-safe spacer rows a dialog deliberately keeps.
        for row in rect.y..rect.y + rect.height {
            app.handle_mouse(left_down(rect.x + 1, row));
            assert!(
                matches!(app.prompt, PromptState::ConfirmQuit { .. }),
                "a click at row {row} inside the modal dismissed it"
            );
        }
    }

    #[test]
    fn clicking_a_modal_button_still_activates_it() {
        let mut app = test_app(default_bindings());
        app.prompt = confirm_quit_prompt();
        render(&mut app);
        let cancel = match app.overlay_layout.active {
            OverlayMouseLayout::ConfirmQuit { cancel_button, .. } => cancel_button,
            other => panic!("expected the quit dialog layout, got {other:?}"),
        };

        // Down arms the press (and must not dismiss) …
        app.handle_mouse(left_down(cancel.x + 1, cancel.y + 1));
        assert!(matches!(app.prompt, PromptState::ConfirmQuit { .. }));
        assert!(app.pressed_button.is_some());
        // … Up on the same button fires Cancel.
        let exit = app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            cancel.x + 1,
            cancel.y + 1,
        ));

        assert!(!exit);
        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn clicking_a_list_row_inside_a_modal_still_selects_it() {
        let mut app = test_app(default_bindings());
        app.open_change_theme_prompt().expect("theme picker");
        render(&mut app);
        let (list, offset) = match app.overlay_layout.active {
            OverlayMouseLayout::ChangeTheme { list, offset, .. } => (list, offset),
            other => panic!("expected the theme picker layout, got {other:?}"),
        };
        assert_eq!(offset, 0, "fixture assumes an unscrolled list");

        // Second visible row.
        app.handle_mouse(left_down(list.x + 1, list.y + 1));

        match &app.prompt {
            PromptState::ChangeTheme(prompt) => assert_eq!(prompt.selected, 1),
            other => panic!("expected the picker to stay open, got {other:?}"),
        }
    }

    #[test]
    fn no_recorded_rect_means_no_dismissal() {
        // A prompt can be open UNDER a fullscreen overlay: `render_overlay`
        // returns before `render_prompt`, so no modal rect is recorded, yet the
        // mouse still routes to prompt handling. Failing open here would
        // dismiss on any click at all.
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        render(&mut app);

        assert_eq!(app.overlay_layout.frame.get(), None);
        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::AgentInfo(_)));
    }

    #[test]
    fn nested_modals_record_the_topmost_rect() {
        let mut app = test_app(default_bindings());
        app.prompt = edit_macros_with_pending_delete();
        render(&mut app);
        let nested = frame_rect(&app);

        // The editor popup is a fixed 64x20; the delete-confirm painted on top
        // of it is smaller. Last write wins, so the store holds the confirm.
        let editor = centered_rect_exact(64, 20, Rect::new(0, 0, TERM.0, TERM.1));
        assert_ne!(nested, editor);
        assert!(nested.width < editor.width || nested.height < editor.height);
    }

    #[test]
    fn outside_click_dismisses_only_the_topmost_nested_modal() {
        let mut app = test_app(default_bindings());
        app.prompt = edit_macros_with_pending_delete();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        // The delete-confirm is gone; the editor underneath is untouched.
        match &app.prompt {
            PromptState::EditMacros {
                pending_delete,
                entries,
                ..
            } => {
                assert!(pending_delete.is_none());
                assert_eq!(entries.len(), 1);
            }
            other => panic!("expected the macro editor to stay open, got {other:?}"),
        }
    }

    #[test]
    fn outside_click_never_asks_the_app_to_exit() {
        // A copy-paste that returned `true` here would quit dux on a stray
        // click, so this is asserted on the most dangerous modal there is.
        let mut app = test_app(default_bindings());
        app.prompt = confirm_quit_prompt();
        render(&mut app);

        assert!(!app.handle_mouse(left_down(0, 0)));
        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn outside_click_clears_an_armed_press() {
        let mut app = test_app(default_bindings());
        app.prompt = confirm_quit_prompt();
        render(&mut app);
        let cancel = match app.overlay_layout.active {
            OverlayMouseLayout::ConfirmQuit { cancel_button, .. } => cancel_button,
            other => panic!("expected the quit dialog layout, got {other:?}"),
        };
        app.handle_mouse(left_down(cancel.x + 1, cancel.y + 1));
        assert!(app.pressed_button.is_some());

        // Re-render (the modal is still up) and press outside it.
        render(&mut app);
        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.pressed_button, None, "no press may outlive its modal");
    }

    #[test]
    fn outside_click_dismisses_the_startup_command_log_viewer() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::StartupCommandLogs(StartupCommandLogPrompt {
            scope_label: "demo".to_string(),
            entries: Vec::new(),
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            content: "line one\nline two".to_string(),
            scroll_offset: 0,
        });
        app.startup_log_selection = None;
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
        assert!(app.startup_log_selection.is_none());
    }

    #[test]
    fn outside_click_dismisses_the_resource_monitor() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ResourceMonitor {
            rows: Vec::new(),
            scroll_offset: 0,
            selected_row: 0,
            expanded: std::collections::HashSet::new(),
            last_refresh: std::time::Instant::now(),
            short_window_sample: false,
        };
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
    }

    // ───────────────── Esc / outside-click parity (the point) ─────────────────
    //
    // Each of these runs the SAME scenario twice on two fresh apps — cancelled
    // by Esc, cancelled by an outside click — and compares the state the two
    // routes leave behind. A second "close" path that merely agrees today would
    // pass the dismissal tests above and fail these.

    type CancelledState = (std::mem::Discriminant<PromptState>, String, Option<String>);

    /// Everything an outside click is expected to leave identical to Esc.
    /// Status text is included: where the cancel helper owns a message, both
    /// routes must produce it.
    fn cancelled_state(app: &App) -> CancelledState {
        (
            prompt_kind(app),
            format!("{:?}", app.theme.app_bg),
            status_text(app),
        )
    }

    fn parity<S, T>(setup: S, state: T)
    where
        S: Fn(&mut App),
        T: Fn(&App) -> CancelledState,
    {
        let by_esc = {
            let mut app = test_app(default_bindings());
            setup(&mut app);
            let before = prompt_kind(&app);
            esc(&mut app);
            assert_ne!(before, prompt_kind(&app), "Esc did not cancel the fixture");
            state(&app)
        };
        let by_click = {
            let mut app = test_app(default_bindings());
            setup(&mut app);
            render(&mut app);
            app.handle_mouse(left_down(0, 0));
            state(&app)
        };
        assert_eq!(by_esc, by_click);
    }

    #[test]
    fn parity_change_theme_reverts_the_live_preview() {
        parity(open_theme_picker_with_live_preview, cancelled_state);
    }

    /// Open the theme picker and move the cursor off the current theme so a
    /// live preview is actually applied — otherwise "cancel reverts the
    /// preview" has nothing to revert and the test would pass vacuously.
    fn open_theme_picker_with_live_preview(app: &mut App) {
        app.open_change_theme_prompt().expect("open theme picker");
        let before = format!("{:?}", app.theme.app_bg);
        let enough_themes = match &mut app.prompt {
            PromptState::ChangeTheme(prompt) => {
                let last = prompt.options.len() - 1;
                prompt.selected = if prompt.selected == last { 0 } else { last };
                prompt.options.len() > 1
            }
            other => panic!("expected the theme picker, got {other:?}"),
        };
        assert!(
            enough_themes,
            "need at least two themes for this test to mean anything"
        );
        app.preview_change_theme_selection();
        assert_ne!(
            before,
            format!("{:?}", app.theme.app_bg),
            "the preview must actually change the theme, or the revert proves nothing"
        );
    }

    #[test]
    fn parity_first_load_records_the_version_as_seen() {
        // The version stamp is the side effect that must not be skipped, so it
        // is compared instead of the theme colour here. Status text is
        // deliberately NOT compared: the keyboard route narrates the dismissal
        // (how to reopen the screen) and the click route stays silent by
        // design.
        let stamp = |app: &App| -> CancelledState {
            (
                prompt_kind(app),
                format!(
                    "{:?}",
                    app.engine
                        .session_store
                        .last_seen_version()
                        .expect("read the last-seen version stamp")
                ),
                None,
            )
        };
        parity(
            |app| {
                let config_path = app.engine.paths.config_path.clone();
                app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(
                    dux_core::welcome_screen::welcome_screen(&config_path),
                    true,
                ));
            },
            stamp,
        );
    }

    #[test]
    fn parity_add_project_failed_restores_its_return_prompt() {
        parity(
            |app| {
                app.prompt = PromptState::AddProjectFailed {
                    message: "not a git repository".to_string(),
                    return_prompt: Box::new(agent_info_prompt()),
                    scroll: 0,
                };
            },
            |app| {
                assert!(
                    matches!(app.prompt, PromptState::AgentInfo(_)),
                    "the return prompt must be restored, not destroyed"
                );
                cancelled_state(app)
            },
        );
    }

    #[test]
    fn parity_confirm_init_repo_restores_its_return_prompt() {
        parity(
            |app| {
                app.prompt = PromptState::ConfirmInitRepo {
                    path: "/tmp/plain-folder".to_string(),
                    name: "plain".to_string(),
                    candidates: vec!["node_modules".to_string()],
                    confirm_selected: false,
                    return_prompt: Box::new(agent_info_prompt()),
                };
            },
            |app| {
                assert!(
                    matches!(app.prompt, PromptState::AgentInfo(_)),
                    "the return prompt must be restored, not destroyed"
                );
                cancelled_state(app)
            },
        );
    }

    #[test]
    fn parity_confirm_create_initial_commit_drops_the_pending_add() {
        parity(
            |app| {
                app.prompt = PromptState::ConfirmCreateInitialCommit {
                    path: "/tmp/unborn-repo".to_string(),
                    name: "unborn".to_string(),
                    confirm_selected: false,
                };
            },
            |app| {
                assert!(matches!(app.prompt, PromptState::None));
                cancelled_state(app)
            },
        );
    }

    #[test]
    fn parity_confirm_kill_running_returns_to_the_kill_list() {
        parity(
            |app| {
                app.prompt = PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
                    previous: KillRunningPrompt {
                        runtimes: Vec::new(),
                        list: SearchableList::new(),
                        selected_ids: std::collections::HashSet::new(),
                        focus: KillRunningFocus::List,
                    },
                    action: KillRunningAction::Selected,
                    target_ids: Vec::new(),
                    confirm_selected: false,
                });
            },
            |app| {
                assert!(
                    matches!(app.prompt, PromptState::KillRunning(_)),
                    "cancelling the nested confirm must step back to the list, \
                     not close the whole stack"
                );
                cancelled_state(app)
            },
        );
    }

    #[test]
    fn parity_config_reload_failed_takes_the_close_button_path() {
        parity(
            |app| {
                app.prompt = PromptState::ConfigReloadFailed {
                    error: "invalid TOML at line 3".to_string(),
                    recover_old_config: false,
                    focus: ConfigReloadFailedFocus::Close,
                    scroll: 0,
                };
            },
            |app| {
                assert!(matches!(app.prompt, PromptState::None));
                cancelled_state(app)
            },
        );
    }
}
