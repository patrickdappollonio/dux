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
//!
//! Not every modal may be dismissed this way. Nine of them hold unsaved free
//! text or a built-up selection, so the policy answers their outside click with
//! a REFUSAL instead: a short, self-terminating blink of the modal's border
//! ring, armed here and painted by `themed_overlay_block`. The click is still
//! answered, just not with a close — which is the point, since a swallowed
//! click teaches the user nothing.

use super::input::contains_point;
use super::*;

/// What an outside click should do to the prompt that is currently open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutsideClickPolicy {
    /// Dismiss, through the variant's real cancel path.
    Cancel,
    /// Refuse, visibly: leave the modal open, touch nothing the user typed, and
    /// blink the modal's frame so the refusal is not silent.
    Blink,
    /// Swallow the click. Reserved for [`PromptState::None`], where there is no
    /// modal to dismiss and nothing to blink.
    Ignore,
}

/// How long the refusal cue lasts, in milliseconds, measured from the click.
///
/// The cue MUST end, and end at rest: past this point
/// [`refusal_blink_highlight_phase`] is `false` forever, so the modal renders
/// byte-identically to one that never blinked, and
/// [`refusal_blink_is_running`] is `false`, so the run loop drops back to its
/// lazy poll instead of spinning at 30fps over a finished animation.
pub(super) const REFUSAL_BLINK_MS: u128 = 800;

/// The on/off pattern of the refusal cue: highlight, off, highlight, off, done.
///
/// Deliberately the same 200ms phase length as [`attention_blink_phase`], so the
/// app has ONE blink cadence rather than two that almost match. The difference
/// is that this one is a one-shot: the attention blink loops forever on a 2s
/// cycle because the condition it reports persists, while a refused click is an
/// event that is over once it has been acknowledged.
pub(super) fn refusal_blink_highlight_phase(elapsed_ms: u128) -> bool {
    refusal_blink_is_running(elapsed_ms) && !matches!(elapsed_ms % 400, 200..=399)
}

/// Whether the cue is still running, and therefore whether the run loop must
/// keep polling at animation cadence.
pub(super) fn refusal_blink_is_running(elapsed_ms: u128) -> bool {
    elapsed_ms < REFUSAL_BLINK_MS
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
    use OutsideClickPolicy::{Blink, Cancel, Ignore};
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
        | PromptState::ManageWorktrees(_)
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
        | PromptState::ConfirmUseExistingBranch { .. }
        // The worktree-removal confirm cancels back to the manager it was
        // raised from, exactly as its Esc arm does.
        | PromptState::ConfirmDeleteWorktree(_) => Cancel,

        // `EditMacros` is three modals in one variant, so it answers three
        // ways. Its nested delete-confirm is a confirmation like any other and
        // dismisses. Its LIST is a Picker over saved rows, holding no unsaved
        // text and no built-up selection, so it dismisses too, like every
        // other picker above; it used to blink, alone among them, on a
        // justification ("unsaved free text, or a multi-step selection") that
        // is true of the EDITOR and not of the list. The editor itself, below,
        // is the half that really does hold unsaved text.
        PromptState::EditMacros {
            pending_delete: Some(_),
            ..
        }
        | PromptState::EditMacros {
            editing: None,
            pending_delete: None,
            ..
        } => Cancel,

        // Everything below holds unsaved free text the user typed, or a
        // multi-step selection they built up, so a stray click must not throw
        // it away. It does not swallow the click either: the modal blinks its
        // frame so the user can see the click landed and was refused, rather
        // than learning nothing and clicking again harder. Do not "simplify"
        // these to `Cancel` — the answer to an outside click here is a cue, not
        // a close.
        PromptState::EditMacros { .. }
        | PromptState::BrowseProjects { .. }
        | PromptState::ConfigureStartupCommand { .. }
        | PromptState::ConfigureProjectEnv { .. }
        | PromptState::ConfigureGlobalEnv { .. }
        | PromptState::RenameSession { .. }
        | PromptState::PullRequestInput { .. }
        | PromptState::AttachPullRequestInput { .. }
        | PromptState::NameNewAgent { .. }
        | PromptState::KillRunning(_) => Blink,
    }
}

impl App {
    /// Arm — or RE-arm — the refusal cue on the modal that is open right now.
    ///
    /// Unconditionally overwrites any cue already running, so a user who clicks
    /// outside twice sees the cue twice instead of the second click being
    /// swallowed by the first cue's tail. That is the whole reason this is a
    /// plain assignment and not a `get_or_insert_with`.
    pub(crate) fn start_refusal_blink(&mut self) {
        self.refusal_blink = Some(RefusalBlink {
            started: Instant::now(),
            prompt: std::mem::discriminant(&self.prompt),
        });
    }

    /// Milliseconds since the cue was armed, or `None` when no cue is armed or
    /// the modal that armed it is no longer the one on screen.
    fn refusal_blink_elapsed_ms(&self) -> Option<u128> {
        let blink = self.refusal_blink?;
        (blink.prompt == std::mem::discriminant(&self.prompt))
            .then(|| blink.started.elapsed().as_millis())
    }

    /// Whether the refusal cue is still running. Read by
    /// [`App::any_row_animating`], which is what keeps the run loop redrawing
    /// at animation cadence for exactly as long as the cue lasts.
    pub(crate) fn refusal_blink_running(&self) -> bool {
        self.refusal_blink_elapsed_ms()
            .is_some_and(refusal_blink_is_running)
    }

    /// Whether the cue is in a highlight phase right now — the one thing the
    /// renderer asks. False both between the two flashes and forever after the
    /// cue ends, which is what makes the settled modal byte-identical to one
    /// that never blinked.
    pub(crate) fn refusal_blink_highlight(&self) -> bool {
        self.refusal_blink_elapsed_ms()
            .is_some_and(refusal_blink_highlight_phase)
    }

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
            | PromptState::ManageWorktrees(_)
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
            // Steps back to the manager's list rather than closing the stack,
            // the kill-running idiom.
            PromptState::ConfirmDeleteWorktree(_) => {
                self.resolve_confirm_delete_worktree(false);
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

            // The macro LIST closes the overlay, exactly as its close binding
            // does. Nothing is saved and nothing is lost: the rows on screen
            // are the ones already in config.
            PromptState::EditMacros {
                editing: None,
                pending_delete: None,
                ..
            } => {
                self.prompt = PromptState::None;
            }

            // Not dismissible by an outside click (see `outside_click_policy`).
            // Reached only if a caller ignores the policy, so it is a no-op
            // rather than a surprise close.
            //
            // The macro EDITOR is deliberately absent from the "matches its
            // Esc arm" contract above, and cannot be added to it: its Esc is
            // state-dependent (in the engaged body it leaves edit mode; in the
            // editor it cancels the edit back to the list), so there is no
            // single statement to mirror. A stray click gets the blink
            // instead, which is the right answer for a surface holding
            // unsaved text. (The list state is handled above: it has a single
            // unambiguous Esc, which closes the overlay.)
            PromptState::EditMacros { .. }
            | PromptState::BrowseProjects { .. }
            | PromptState::ConfigureStartupCommand { .. }
            | PromptState::ConfigureProjectEnv { .. }
            | PromptState::ConfigureGlobalEnv { .. }
            | PromptState::RenameSession { .. }
            | PromptState::PullRequestInput { .. }
            | PromptState::AttachPullRequestInput { .. }
            | PromptState::NameNewAgent { .. }
            | PromptState::KillRunning(_) => return false,
        }
        true
    }

    /// The mouse-side entry point: answer an outside click the way the open
    /// prompt's policy says to — dismiss it, or refuse it visibly — and report
    /// whether anything was dismissed.
    ///
    /// Called from the ONE place in `handle_prompt_mouse` where the hit-test
    /// has already returned `None`, so it can never preempt a button, a list
    /// row, a checkbox, a text input, or a modal's deliberate blank
    /// misclick-safe spacer row. That also means a click INSIDE a refusing
    /// modal never arms the cue: it was handled by the control it hit.
    pub(super) fn dismiss_prompt_on_outside_click(&mut self, mouse: &MouseEvent) -> bool {
        if !click_outside_frame(self.overlay_layout.frame.get(), mouse) {
            return false;
        }
        match outside_click_policy(&self.prompt) {
            OutsideClickPolicy::Cancel => self.cancel_prompt(),
            OutsideClickPolicy::Blink => {
                self.start_refusal_blink();
                false
            }
            OutsideClickPolicy::Ignore => false,
        }
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
            branch_named: true,
        }
    }

    fn confirm_quit_prompt() -> PromptState {
        PromptState::ConfirmQuit {
            agent_count: 1,
            terminal_count: 0,
            focus: ConfirmFocus::Cancel,
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
                focus: ConfirmFocus::Cancel,
            }),
        }
    }

    /// The macro EDITOR open over the list: the one `EditMacros` state that
    /// refuses an outside click, because it is the one holding unsaved text.
    fn edit_macros_with_open_editor() -> PromptState {
        PromptState::EditMacros {
            entries: vec![(
                "greet".to_string(),
                "hello".to_string(),
                crate::config::MacroSurface::Agent,
            )],
            selected: 0,
            editing: Some(crate::app::MacroEditState {
                id: Some("greet".to_string()),
                name_input: crate::app::TextInput::with_text("greet".to_string()),
                text_input: crate::app::TextInput::with_text("hello, unsaved".to_string())
                    .with_multiline(8),
                surface: crate::config::MacroSurface::Agent,
                focus: crate::app::MacroEditFocus::Text,
            }),
            pending_delete: None,
        }
    }

    /// Every modal that refuses an outside click, paired with the text (or
    /// built-up selection) whose loss is the reason it refuses. Built from the
    /// app so the two variants that carry real project/request payloads can use
    /// the seeded fixture project.
    fn refusing_prompts(app: &App) -> Vec<(&'static str, PromptState)> {
        let project = app.engine.projects[0].clone();
        vec![
            ("EditMacros", edit_macros_with_open_editor()),
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
                    path_input: TextInput::with_text("half-typed-path".to_string()),
                    tab_completions: Vec::new(),
                    tab_index: 0,
                },
            ),
            (
                "ConfigureStartupCommand",
                PromptState::ConfigureStartupCommand {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    input: TextInput::with_text("half-typed-name".to_string()),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            (
                "ConfigureProjectEnv",
                PromptState::ConfigureProjectEnv {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    input: TextInput::with_text("half-typed-name".to_string()),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            (
                "ConfigureGlobalEnv",
                PromptState::ConfigureGlobalEnv {
                    project_name: project.name.clone(),
                    input: TextInput::with_text("half-typed-name".to_string()),
                    focus: ConfigureFieldFocus::default(),
                },
            ),
            ("RenameSession", rename_session_prompt()),
            (
                "PullRequestInput",
                PromptState::PullRequestInput {
                    focus: crate::app::PullRequestInputFocus::Input,
                    project: Some(project),
                    input: TextInput::with_text("half-typed-name".to_string()),
                },
            ),
            (
                "AttachPullRequestInput",
                PromptState::AttachPullRequestInput {
                    session_id: "session-1".to_string(),
                    current_pr: None,
                    input: TextInput::with_text("half-typed-name".to_string()),
                },
            ),
            ("NameNewAgent", name_new_agent_prompt(app)),
            (
                "KillRunning",
                PromptState::KillRunning(KillRunningPrompt {
                    runtimes: Vec::new(),
                    list: SearchableList::new(),
                    selected_ids: std::collections::HashSet::new(),
                    focus: KillRunningFocus::List,
                }),
            ),
        ]
    }

    fn name_new_agent_prompt(app: &App) -> PromptState {
        PromptState::NameNewAgent {
            request: CreateAgentRequest::NewProject {
                project: app.engine.projects[0].clone(),
                custom_name: None,
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: false,
            },
            input: TextInput::with_text("half-typed-name".to_string()),
            randomize_name: false,
            randomized_name: None,
            copy_changes: false,
            focus: NameNewAgentFocus::Input,
        }
    }

    /// The one piece of user work each refusing modal is holding, read back out
    /// of the live prompt so a test can prove the refusal touched none of it.
    fn held_text(prompt: &PromptState) -> String {
        match prompt {
            PromptState::EditMacros { entries, .. } => format!("{entries:?}"),
            PromptState::BrowseProjects { path_input, .. } => path_input.text.clone(),
            PromptState::ConfigureStartupCommand { input, .. }
            | PromptState::ConfigureProjectEnv { input, .. }
            | PromptState::ConfigureGlobalEnv { input, .. }
            | PromptState::RenameSession { input, .. }
            | PromptState::PullRequestInput { input, .. }
            | PromptState::AttachPullRequestInput { input, .. }
            | PromptState::NameNewAgent { input, .. } => input.text.clone(),
            PromptState::KillRunning(prompt) => format!("{:?}", prompt.selected_ids),
            other => panic!("not a refusing modal: {other:?}"),
        }
    }

    /// Wind an armed cue back in time, so a test can observe a later phase of
    /// it without sleeping. Wall-clock, so this is all it takes.
    fn advance_blink(app: &mut App, by_ms: u64) {
        let blink = app
            .refusal_blink
            .as_mut()
            .expect("a cue must be armed before it can be advanced");
        blink.started -= Duration::from_millis(by_ms);
    }

    fn render_to_buffer(app: &mut App) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(TERM.0, TERM.1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        terminal.backend().buffer().clone()
    }

    // ───────────────────────── the refusal cue ─────────────────────────

    #[test]
    fn refusal_blink_phase_flashes_twice_then_ends_at_rest() {
        // Flash one.
        assert!(refusal_blink_highlight_phase(0));
        assert!(refusal_blink_highlight_phase(199));
        assert!(!refusal_blink_highlight_phase(200));
        assert!(!refusal_blink_highlight_phase(399));
        // Flash two.
        assert!(refusal_blink_highlight_phase(400));
        assert!(refusal_blink_highlight_phase(599));
        assert!(!refusal_blink_highlight_phase(600));
        assert!(!refusal_blink_highlight_phase(799));
        // Over. It must stay over forever, never wrap into a third flash.
        assert!(refusal_blink_is_running(799));
        assert!(!refusal_blink_is_running(REFUSAL_BLINK_MS));
        for elapsed in [800, 801, 1_000, 5_000, 60_000, 86_400_000] {
            assert!(
                !refusal_blink_highlight_phase(elapsed),
                "the cue restarted itself at {elapsed}ms"
            );
            assert!(!refusal_blink_is_running(elapsed));
        }
    }

    #[test]
    fn policy_blinks_for_every_modal_holding_unsaved_work() {
        let app = test_app(default_bindings());
        for (name, prompt) in refusing_prompts(&app) {
            assert_eq!(
                outside_click_policy(&prompt),
                OutsideClickPolicy::Blink,
                "{name} must refuse visibly, not silently"
            );
        }
        // Ignore is now reserved for "no modal is open".
        assert_eq!(
            outside_click_policy(&PromptState::None),
            OutsideClickPolicy::Ignore
        );
    }

    #[test]
    fn outside_click_blinks_every_refusing_modal_and_keeps_its_work() {
        for (name, prompt) in refusing_prompts(&test_app(default_bindings())) {
            let mut app = test_app(default_bindings());
            app.prompt = prompt;
            let kind = prompt_kind(&app);
            let work = held_text(&app.prompt);
            let baseline = render_to_buffer(&mut app);
            assert!(
                !app.refusal_blink_running(),
                "{name} blinked before a click"
            );

            app.handle_mouse(left_down(0, 0));

            assert_eq!(prompt_kind(&app), kind, "{name} was dismissed");
            assert_eq!(held_text(&app.prompt), work, "{name} lost the user's work");
            assert!(app.refusal_blink_running(), "{name} refused silently");
            assert!(
                app.refusal_blink_highlight(),
                "{name} armed a cue that starts invisible"
            );
            // State is not the cue: prove the refusal actually reaches the
            // screen for THIS modal, and then leaves no trace behind.
            assert_ne!(
                render_to_buffer(&mut app),
                baseline,
                "{name} armed a cue that renders nothing"
            );
            advance_blink(&mut app, REFUSAL_BLINK_MS as u64);
            assert_eq!(
                render_to_buffer(&mut app),
                baseline,
                "{name} did not settle back to its unblinked frame"
            );
        }
    }

    #[test]
    fn the_cue_settles_back_to_a_frame_that_never_blinked() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        let baseline = render_to_buffer(&mut app);

        app.handle_mouse(left_down(0, 0));
        assert_ne!(
            render_to_buffer(&mut app),
            baseline,
            "the first flash never showed"
        );
        advance_blink(&mut app, 450);
        assert_ne!(
            render_to_buffer(&mut app),
            baseline,
            "the second flash never showed"
        );

        // Past the cue's duration the modal must be indistinguishable from one
        // that was never clicked: no frozen-bright, no frozen-dim.
        advance_blink(&mut app, REFUSAL_BLINK_MS as u64);
        assert_eq!(
            render_to_buffer(&mut app),
            baseline,
            "the cue froze instead of settling back to rest"
        );
        assert!(!app.refusal_blink_running());
    }

    #[test]
    fn a_second_outside_click_restarts_the_cue() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));
        // Wind on to the dark tail of the cue, where a swallowed second click
        // would leave the modal looking untouched.
        advance_blink(&mut app, 700);
        assert!(app.refusal_blink_running());
        assert!(!app.refusal_blink_highlight());

        render(&mut app);
        app.handle_mouse(left_down(0, 0));

        assert!(
            app.refusal_blink_highlight(),
            "the second click was swallowed instead of re-flashing"
        );
        // And it is a genuine restart, not an extension of the first cue.
        advance_blink(&mut app, 799);
        assert!(app.refusal_blink_running());
        advance_blink(&mut app, 1);
        assert!(!app.refusal_blink_running());
    }

    #[test]
    fn a_dismissing_modal_still_dismisses_and_never_blinks() {
        let mut app = test_app(default_bindings());
        app.prompt = agent_info_prompt();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(app.prompt, PromptState::None));
        assert!(
            !app.refusal_blink_running(),
            "a modal that dismissed also blinked"
        );
        assert!(app.refusal_blink.is_none());
    }

    #[test]
    fn the_nested_macro_delete_confirm_dismisses_without_blinking() {
        // The one variant that is a refusing modal and a dismissing modal at
        // once, depending on `pending_delete`.
        let mut app = test_app(default_bindings());
        app.prompt = edit_macros_with_pending_delete();
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(matches!(
            app.prompt,
            PromptState::EditMacros {
                pending_delete: None,
                ..
            }
        ));
        assert!(!app.refusal_blink_running());
    }

    /// The macro LIST is a Picker over saved rows. It holds no unsaved text and
    /// no multi-step selection, so it must cancel on an outside click like
    /// every other picker. It used to blink, alone among them, on a
    /// justification that was true of the editor and not of the list.
    #[test]
    fn the_macro_list_cancels_on_an_outside_click_like_every_other_picker() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::EditMacros {
            entries: vec![(
                "greet".to_string(),
                "hello".to_string(),
                crate::config::MacroSurface::Agent,
            )],
            selected: 0,
            editing: None,
            pending_delete: None,
        };
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert!(
            matches!(app.prompt, PromptState::None),
            "the macro list must close, got {:?}",
            app.prompt
        );
        assert!(
            !app.refusal_blink_running(),
            "a modal that dismissed also blinked"
        );
    }

    #[test]
    fn the_open_macro_editor_blinks_and_keeps_the_unsaved_edit() {
        // The refusing-prompts fixture covers the macro LIST. The editor is the
        // half that actually holds unsaved text, so it gets its own case.
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::EditMacros {
            entries: vec![(
                "greet".to_string(),
                "hello".to_string(),
                crate::config::MacroSurface::Agent,
            )],
            selected: 0,
            editing: Some(crate::app::MacroEditState {
                id: Some("greet".to_string()),
                name_input: crate::app::TextInput::with_text("greet".to_string()),
                text_input: crate::app::TextInput::with_text("hello, unsaved".to_string())
                    .with_multiline(8),
                surface: crate::config::MacroSurface::Agent,
                focus: crate::app::MacroEditFocus::Text,
            }),
            pending_delete: None,
        };
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        match &app.prompt {
            PromptState::EditMacros {
                editing: Some(state),
                ..
            } => assert_eq!(state.text_input.text, "hello, unsaved"),
            other => panic!("the editor must survive an outside click, got {other:?}"),
        }
        assert!(
            app.refusal_blink_running(),
            "the refusal must be visible, not silent"
        );
    }

    #[test]
    fn the_run_loop_animates_while_the_cue_runs_and_goes_quiet_after() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        render(&mut app);
        assert!(
            !app.any_row_animating(),
            "fixture must start with nothing animating, or this proves nothing"
        );

        app.handle_mouse(left_down(0, 0));
        assert!(app.any_row_animating(), "the cue would never be redrawn");

        // The dark half of a flash still needs the fast cadence: the next
        // flash is coming.
        advance_blink(&mut app, 300);
        assert!(app.any_row_animating());

        advance_blink(&mut app, 500);
        assert!(
            !app.any_row_animating(),
            "a finished cue kept the run loop hot"
        );
    }

    #[test]
    fn a_click_inside_a_refusing_modal_reaches_its_controls_and_does_not_blink() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        render(&mut app);
        let checkbox = match app.overlay_layout.active {
            OverlayMouseLayout::RenameSession { checkbox, .. } => {
                checkbox.expect("the rename dialog paints its checkbox")
            }
            ref other => panic!("expected the rename layout, got {other:?}"),
        };
        let before = match &app.prompt {
            PromptState::RenameSession { rename_branch, .. } => *rename_branch,
            other => panic!("expected the rename prompt, got {other:?}"),
        };

        app.handle_mouse(left_down(checkbox.rect.x, checkbox.rect.y));

        match &app.prompt {
            PromptState::RenameSession {
                rename_branch,
                input,
                ..
            } => {
                assert_ne!(*rename_branch, before, "the checkbox did not toggle");
                assert_eq!(input.text, "half-typed-name");
            }
            other => panic!("expected the rename prompt to survive, got {other:?}"),
        }
        assert!(
            !app.refusal_blink_running(),
            "a click INSIDE the modal was treated as a refusal"
        );
    }

    #[test]
    fn a_click_inside_a_refusing_modal_still_reaches_its_text_field() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_session_prompt();
        render(&mut app);
        let input = match app.overlay_layout.active {
            OverlayMouseLayout::RenameSession { input, .. } => input,
            ref other => panic!("expected the rename layout, got {other:?}"),
        };

        // Click a couple of characters into the field: the caret must move
        // there, and nothing may blink. The field renders one leading space,
        // so the fourth column into the box is text character 3.
        app.handle_mouse(left_down(input.x + 4, input.y));

        match &app.prompt {
            PromptState::RenameSession {
                input,
                focus: RenameSessionFocus::Input,
                ..
            } => {
                assert_eq!(input.text, "half-typed-name");
                assert_eq!(input.cursor, 3);
            }
            other => panic!("expected the rename input to take the click, got {other:?}"),
        }
        assert!(!app.refusal_blink_running());
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
            OutsideClickPolicy::Blink
        );
        // `EditMacros` answers three ways. Its nested delete-confirm is a
        // confirmation and dismisses; its list is a picker and dismisses; only
        // the open editor, which holds unsaved text, refuses.
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
            OutsideClickPolicy::Cancel
        );
        assert_eq!(
            outside_click_policy(&edit_macros_with_open_editor()),
            OutsideClickPolicy::Blink
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
    fn outside_click_is_refused_by_an_unsaved_text_modal() {
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
            wrap_width: 0,
            focus: StartupCommandLogFocus::List,
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

    // ────────────────────────── the help overlay ──────────────────────────
    //
    // Help is NOT a `PromptState` variant — it lives in `help_scroll` and is
    // handled in `handle_mouse` rather than the prompt mouse path, so
    // `outside_click_policy` (which takes a `&PromptState`) structurally cannot
    // reach it. It still dismisses on an outside click, reusing this module's
    // geometry rule (`click_outside_frame`) and the same fail-closed contract,
    // and closing through the same `close_help_overlay` helper the keyboard
    // ladder uses. These tests live here because that is what they exercise.

    fn open_help(app: &mut App) {
        app.help_scroll = Some(0);
    }

    fn toggle_help_key(app: &mut App) {
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .expect("toggle help");
    }

    #[test]
    fn outside_click_closes_the_help_overlay() {
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        render(&mut app);

        app.handle_mouse(left_down(0, 0));

        assert_eq!(app.help_scroll, None, "help survived a click outside it");
    }

    #[test]
    fn a_click_inside_the_help_overlay_does_not_close_it() {
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        render(&mut app);
        let rect = frame_rect(&app);

        for row in rect.y..rect.y + rect.height {
            app.handle_mouse(left_down(rect.x + 1, row));
            assert_eq!(
                app.help_scroll,
                Some(0),
                "a click at row {row} inside the help overlay closed it"
            );
        }
    }

    #[test]
    fn the_wheel_still_scrolls_help_both_ways_and_never_closes_it() {
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        render(&mut app);
        assert!(
            app.last_help_lines > app.last_help_height,
            "fixture must have more help content than fits, or scrolling proves nothing"
        );

        // Wheel over the page, and wheel outside it: neither is a dismissal.
        for (column, row) in [(frame_rect(&app).x + 1, frame_rect(&app).y + 1), (0, 0)] {
            app.handle_mouse(mouse(MouseEventKind::ScrollDown, column, row));
            assert_eq!(app.help_scroll, Some(3), "the wheel stopped scrolling down");
            app.handle_mouse(mouse(MouseEventKind::ScrollUp, column, row));
            assert_eq!(app.help_scroll, Some(0), "the wheel stopped scrolling up");
        }
    }

    #[test]
    fn help_fails_closed_when_no_rect_was_recorded() {
        // Help can be open UNDER a fullscreen overlay: `render_overlay` returns
        // before `render_help`, so no rect is recorded, yet the mouse still
        // routes into the help branch. Failing open would close it on any click.
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        render(&mut app);

        assert_eq!(app.overlay_layout.frame.get(), None);
        app.handle_mouse(left_down(0, 0));

        assert_eq!(
            app.help_scroll,
            Some(0),
            "help closed with no rect recorded"
        );
    }

    #[test]
    fn the_help_mouse_route_lands_where_the_keyboard_route_lands() {
        // Same scenario, two devices. Everything but the announcement must
        // match — including the scroll offset, which neither route may leave
        // behind for the next open.
        let by_key = {
            let mut app = test_app(default_bindings());
            open_help(&mut app);
            render(&mut app);
            app.help_scroll = Some(5);
            esc(&mut app);
            (app.help_scroll, app.last_help_lines, app.last_help_height)
        };
        let by_click = {
            let mut app = test_app(default_bindings());
            open_help(&mut app);
            render(&mut app);
            app.help_scroll = Some(5);
            app.handle_mouse(left_down(0, 0));
            (app.help_scroll, app.last_help_lines, app.last_help_height)
        };

        assert_eq!(by_key.0, None, "the keyboard route kept a scroll offset");
        assert_eq!(by_key, by_click);
    }

    #[test]
    fn closing_help_by_click_stays_silent_like_every_other_outside_click() {
        // Deliberate divergence from the keyboard ladder, which narrates the
        // dismissal: a click is self-evident, and the 26 modals that dismiss
        // through this engine already announce nothing.
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        render(&mut app);
        let before = status_text(&app);

        app.handle_mouse(left_down(0, 0));

        assert_eq!(app.help_scroll, None);
        assert_eq!(status_text(&app), before, "the dismissal announced itself");

        // The keyboard route still says how to get back.
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        esc(&mut app);
        assert!(
            status_text(&app).is_some_and(|text| text.contains("help")),
            "the keyboard ladder must keep its message"
        );
    }

    #[test]
    fn an_outside_click_on_help_never_asks_the_app_to_exit() {
        // `handle_mouse` returns "should the app exit"; a copy-paste returning
        // `true` here would quit dux on a stray click.
        let mut app = test_app(default_bindings());
        open_help(&mut app);
        render(&mut app);

        assert!(!app.handle_mouse(left_down(0, 0)));
        assert_eq!(app.help_scroll, None);
    }

    #[test]
    fn help_is_still_opened_and_closed_by_the_keyboard() {
        let mut app = test_app(default_bindings());
        assert_eq!(app.help_scroll, None);

        toggle_help_key(&mut app);
        assert_eq!(
            app.help_scroll,
            Some(0),
            "the toggle key stopped opening help"
        );

        // Pre-existing, and unchanged here: while help is open the help branch
        // in `handle_key` consumes every key, so the toggle key does not close
        // it — the close-overlay key does, via `close_top_overlay`, which
        // `handle_key` reaches before that branch.
        toggle_help_key(&mut app);
        assert_eq!(app.help_scroll, Some(0));
        esc(&mut app);
        assert_eq!(
            app.help_scroll, None,
            "the close-overlay key stopped closing help"
        );
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
                    focus: ConfirmFocus::Cancel,
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
                    focus: ConfirmFocus::Cancel,
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
                    focus: ConfirmFocus::Cancel,
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
