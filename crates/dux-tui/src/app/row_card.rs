//! Whether this surface may change the selected agent's row while the take-over
//! card is over it, which actions would, and the view a take-over gives back.

use std::cell::{Cell, RefCell};

use super::pty_ownership::{PtyTakeoverCard, UNNAMED_DEVICE};
use super::{App, FullscreenOverlay, InputTarget};
use crate::keybindings::Action;
use crate::model::SessionSurface;

/// Whether `action`, on the selected agent, would change its row: start or
/// stop any of it, add, close or reorder a tab or terminal, or change what the
/// agent is. Exhaustive, so a new action does not compile until it is decided.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per action, so a new action does not compile until it is decided"
)]
pub(crate) fn changes_agent_row(action: Action) -> bool {
    match action {
        Action::FocusAgent
        | Action::InteractAgent
        | Action::ToggleFullscreen
        | Action::ReconnectAgent
        | Action::ForceReconnectAgent
        | Action::DeleteSession
        | Action::RenameSession
        | Action::DetachAgent
        | Action::RecreateWorkingCopy
        | Action::RerunStartupCommandOnAgent
        | Action::ChangeAgentProvider
        | Action::ToggleAgentAutoReopen
        | Action::NewTab
        | Action::CloseTab
        | Action::StopTab
        | Action::MoveTabLeft
        | Action::MoveTabRight
        | Action::ShowTerminal
        | Action::CycleTerminalLayout
        | Action::NewTerminal
        | Action::CloseTerminal => true,
        Action::MoveDown
        | Action::MoveUp
        | Action::ToggleProject
        | Action::NewAgent
        | Action::NewAgentFromPr
        | Action::NewAgentFromWorktree
        | Action::ManageProjects
        | Action::ManageWorktrees
        | Action::FilterAgents
        | Action::ForkAgent
        | Action::ChangeDefaultProvider
        | Action::ChangeProjectDefaultProvider
        | Action::OpenProjectBrowser
        | Action::CloneProject
        | Action::CopyPath
        | Action::OpenWorktreeInEditor
        | Action::ChooseWorktreeEditor
        | Action::RefreshProject
        | Action::CheckoutProjectDefaultBranch
        | Action::ChangeProjectBaseBranch
        | Action::NextTab
        | Action::PrevTab
        | Action::SelectTab1
        | Action::SelectTab2
        | Action::SelectTab3
        | Action::SelectTab4
        | Action::SelectTab5
        | Action::SelectTab6
        | Action::SelectTab7
        | Action::SelectTab8
        | Action::SelectTab9
        | Action::OpenMacroBar
        | Action::OpenCurrentPullRequest
        | Action::NextPullRequest
        | Action::PullRequests
        | Action::PullRequestsAttach
        | Action::PullRequestHideOrDetach
        | Action::PullRequestsToggleHidden
        | Action::PullRequestsToggleView
        | Action::PullRequestsToggleSort
        | Action::PullRequestMoveUp
        | Action::PullRequestMoveDown
        | Action::PullRequestMoveTop
        | Action::ScrollPageUp
        | Action::ScrollPageDown
        | Action::ScrollLineUp
        | Action::ScrollLineDown
        | Action::ScrollToBottom
        | Action::ScrollToTop
        | Action::OpenDiff
        | Action::StageUnstage
        | Action::CommitChanges
        | Action::DiscardChanges
        | Action::EngageCommitInput
        | Action::PushToRemote
        | Action::PullFromRemote
        | Action::SearchFiles
        | Action::SearchNext
        | Action::ExitCommitInput
        | Action::FocusNext
        | Action::FocusPrev
        | Action::OpenPalette
        | Action::ToggleResizeMode
        | Action::ToggleSidebar
        | Action::ToggleGitPane
        | Action::ToggleHelp
        | Action::ForceRedraw
        | Action::Quit
        | Action::CloseOverlay
        | Action::ResizeGrow
        | Action::ResizeShrink
        | Action::SearchToggle
        | Action::GoToPath
        | Action::ExitPathEditorOnProjectAdd
        | Action::OpenEntry
        | Action::AddCurrentDir
        | Action::OpenStartupCommandLogFile
        | Action::OpenStartupCommandLogFolder
        | Action::Confirm
        | Action::ToggleSelection
        | Action::ToggleMarked
        | Action::KillRunning
        | Action::ClearStatusLine
        | Action::EditConfig
        | Action::RenameWebInstance
        | Action::NewProjectTerminal
        | Action::NewStandaloneTerminal
        | Action::NewStandaloneAgent
        | Action::DeleteProject
        | Action::RemoveProject
        | Action::SortAgents
        | Action::RemoveGitPane
        | Action::EditMacros
        | Action::NewMacro
        | Action::DeleteMacro
        | Action::ClearTextField
        | Action::DebugInput
        | Action::ToggleDiffLineNumbers
        | Action::ResourceMonitor
        | Action::ToggleGithubIntegration
        | Action::RecheckGithub
        | Action::ToggleCopyOnSelect
        | Action::ToggleProjectAutoReopenAgents
        | Action::ConfigureStartupCommand
        | Action::ConfigureGlobalEnv
        | Action::ConfigureProjectEnv
        | Action::ReadStartupCommandLogs
        | Action::ToggleRandomizedPetNameDefault
        | Action::TogglePrBannerPosition
        | Action::RefreshChanges
        | Action::ChangeTheme
        | Action::ReloadConfig
        | Action::StartWebServer
        | Action::StartBackgroundServer
        | Action::StopBackgroundServer
        | Action::ViewServerLog
        | Action::SetTailscaleMode
        | Action::ToggleAlwaysShowTabs
        | Action::ToggleTabReachesAgent
        | Action::ToggleNerdFontIcons
        | Action::OpenAgentInfo
        | Action::ShowWelcomeScreen
        | Action::ShowReleaseNotes
        | Action::MoveAgentUp
        | Action::MoveAgentDown
        | Action::MoveAgentTop
        | Action::MoveAgentBottom
        | Action::MoveTerminalUp
        | Action::MoveTerminalDown
        | Action::MoveTerminalTop
        | Action::MoveTerminalBottom => false,
    }
}

/// A row's card and the pty it was read from; `None` when there is no card.
type RowAnswer = Option<(PtyTakeoverCard, String)>;

/// The selected agent row's card, asked of the registry once per pass of
/// input or drawing rather than once per question; outside a pass every ask is live.
#[derive(Default)]
pub(crate) struct RowCardMemo {
    passes: Cell<u32>,
    answer: RefCell<Option<(String, RowAnswer)>>,
}

impl RowCardMemo {
    pub(crate) fn begin(&self) {
        self.passes.set(self.passes.get() + 1);
    }

    pub(crate) fn end(&self) {
        let passes = self.passes.get().saturating_sub(1);
        self.passes.set(passes);
        if passes == 0 {
            self.forget();
        }
    }

    /// Drop the answer, for when this surface itself changed who drives.
    pub(crate) fn forget(&self) {
        self.answer.take();
    }

    /// The answer for the row `probe` belongs to, asking `ask` only when this
    /// pass has not asked about that pty yet.
    pub(crate) fn answer(&self, probe: &str, ask: impl FnOnce() -> RowAnswer) -> RowAnswer {
        if self.passes.get() == 0 {
            return ask();
        }
        if let Some((asked, answer)) = self.answer.borrow().as_ref()
            && asked == probe
        {
            return answer.clone();
        }
        let answer = ask();
        *self.answer.borrow_mut() = Some((probe.to_string(), answer.clone()));
        answer
    }
}

/// What a take-over gives back to the agent's row: the view this surface had
/// when the card came up over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowView {
    pub(crate) input_target: InputTarget,
    pub(crate) fullscreen_overlay: FullscreenOverlay,
    pub(crate) session_surface: SessionSurface,
}

impl App {
    /// Run `act` as one pass, so every question about the selected row inside
    /// it costs the registry one ask.
    pub(crate) fn in_row_pass<T>(&mut self, act: impl FnOnce(&mut Self) -> T) -> T {
        self.row_card_memo.begin();
        let out = act(self);
        self.row_card_memo.end();
        out
    }

    /// Whether this surface may not change the selected agent's row, saying
    /// so on the status line when it may not: who drives it, and how to take
    /// it over.
    pub(crate) fn refuse_row_change(&mut self) -> bool {
        let Some((card, _)) = self.agent_row_card() else {
            return false;
        };
        let name = self.selected_session().map_or_else(
            || "this agent".to_string(),
            dux_core::model::AgentSession::display_label,
        );
        let key = self.bindings.label_for(Action::FocusAgent);
        let who = match card {
            PtyTakeoverCard::Elsewhere { device } => format!(
                "{name} is active on {}",
                device.as_deref().unwrap_or(UNNAMED_DEVICE)
            ),
            PtyTakeoverCard::Free => {
                format!("Nobody is driving {name}, this window included")
            }
        };
        self.set_info(format!(
            "{who}, so nothing changed. Press {key} on the card in the center pane to take \
             it over first."
        ));
        true
    }

    fn current_row_view(&self) -> RowView {
        RowView {
            input_target: self.input_target,
            fullscreen_overlay: self.fullscreen_overlay,
            session_surface: self.session_surface,
        }
    }

    /// Hold the selected agent's view when the card comes up over its row, and
    /// give it back when the card lifts with the row still running. A view held
    /// for another row is dropped: a take-over never undoes a selection change.
    pub(crate) fn keep_row_view(&mut self) {
        let selected = self.selected_session().map(|session| session.id.clone());
        let covered = self.agent_row_card().is_some();
        match (self.held_row_view.take(), selected) {
            (Some((held, view)), Some(selected)) if held == selected => {
                if covered {
                    self.held_row_view = Some((held, view));
                } else if self.selected_row_probe().is_some() {
                    self.input_target = view.input_target;
                    self.fullscreen_overlay = view.fullscreen_overlay;
                    self.session_surface = view.session_surface;
                    self.mark_frame_dirty();
                }
            }
            (_, Some(selected)) if covered => {
                self.held_row_view = Some((selected, self.current_row_view()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::pty_ownership::REGISTRY_ASKS;
    use crate::app::pty_ownership::tests::{app_with_a_live_pty_running, render_rows};
    use crate::app::{CenterPart, DockLayout, FocusPane, PromptState};
    use crate::keybindings::{BINDING_DEFS, BindingScope, ExtendedKeys, TabKey};
    use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
    use dux_core::background_serve::TuiOwnership;
    use dux_core::ids::{TabId, TabIdRef};

    const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
    const SESSION: &str = "session-1";
    const SLOT: &str = "session-1-slot";

    /// Who drives the row in a fixture.
    #[derive(Clone, Copy, PartialEq)]
    enum Driver {
        Browser,
        Nobody,
    }

    /// An agent with one live tab and one terminal, the dock focused in a split,
    /// drawn once with `driver` driving its row.
    fn covered(driver: Driver) -> (App, TuiOwnership) {
        let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 60");
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = Vec::new();
        let (terminal, _) = app
            .engine
            .create_companion_terminal(SESSION, 24, 80)
            .expect("companion terminal");
        app.focus = FocusPane::Center;
        app.active_terminal_id = Some(terminal);
        app.session_surface = SessionSurface::Terminal;
        app.dock_layouts
            .insert(SESSION.to_string(), DockLayout::Split);
        app.reconcile_center_target();
        if driver == Driver::Browser {
            let conn = seat.owners.next_conn_id();
            seat.owners
                .claim_for_resize(
                    SLOT,
                    conn,
                    dux_core::pty_owners::ResizeIntent::Plain,
                    Some(BROWSER),
                    |_| {},
                )
                .epoch
                .expect("the browser claimed the row");
        }
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_some(), "the card is up");
        (app, seat)
    }

    /// Give the agent a second, dormant provider tab and focus it.
    fn focus_a_dormant_extra_tab(app: &mut App) {
        let session = app.engine.sessions[0].clone();
        app.engine.session_store.create_session(&session).unwrap();
        let tab = dux_core::model::AgentTab {
            id: "tab-2".to_string(),
            session_id: session.id.clone(),
            provider: dux_core::model::ProviderKind::new("claude"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        app.engine.session_store.insert_agent_tab(&tab).unwrap();
        app.engine.agent_tabs.insert(TabId::new("tab-2"), tab);
        app.set_focused_tab(SESSION, "tab-2");
        let _ = render_rows(app, 160, 40);
    }

    /// Every state the card can be up in, each named for the assertion messages.
    fn covered_states() -> Vec<(&'static str, App, TuiOwnership)> {
        let (driven, seat) = covered(Driver::Browser);
        let (free, free_seat) = covered(Driver::Nobody);
        let (mut stopped, stopped_seat) = covered(Driver::Browser);
        stopped.engine.providers.remove(TabIdRef::new(SLOT));
        let _ = render_rows(&mut stopped, 160, 40);
        let (mut dormant, dormant_seat) = covered(Driver::Browser);
        focus_a_dormant_extra_tab(&mut dormant);
        vec![
            ("a browser drives, the dock focused", driven, seat),
            ("nobody drives", free, free_seat),
            (
                "a stopped agent beside its running terminal",
                stopped,
                stopped_seat,
            ),
            ("a dormant extra tab focused", dormant, dormant_seat),
        ]
    }

    /// Everything about the agent's row a refused act could change, and what a
    /// take-over gives back.
    #[derive(Debug, PartialEq)]
    struct RowState {
        label: String,
        provider: String,
        auto_reopen: bool,
        tabs: Vec<String>,
        live_tabs: Vec<String>,
        terminals: Vec<String>,
        launched: Vec<String>,
        owner: Option<u64>,
        armed: bool,
        layout: Option<DockLayout>,
        pct: u16,
        view: RowView,
        front: Option<String>,
        dock_front: Option<String>,
    }

    /// The view a take-over gives back: the one held under the card, or the
    /// current one when nothing is held for the agent.
    fn view_to_restore(app: &App) -> RowView {
        match &app.held_row_view {
            Some((session_id, view)) if session_id == SESSION => *view,
            _ => app.current_row_view(),
        }
    }

    fn row_state(app: &App, seat: &TuiOwnership) -> RowState {
        let session = app
            .engine
            .sessions
            .iter()
            .find(|session| session.id == SESSION)
            .expect("the agent");
        let tabs = app.session_tab_ids(SESSION);
        let terminals = app.dock_terminal_ids(SESSION);
        let mut launched: Vec<String> = app.tui_launched_ptys.iter().cloned().collect();
        launched.sort();
        RowState {
            label: session.display_label(),
            provider: session.provider.as_str().to_string(),
            auto_reopen: session.auto_reopen_enabled,
            live_tabs: tabs
                .iter()
                .filter(|tab| app.engine.providers.contains_key(TabIdRef::new(tab)))
                .cloned()
                .collect(),
            tabs,
            owner: seat.owners.current_owner(&terminals[0]).0,
            terminals,
            launched,
            armed: app.pending_pty_takeover.is_some(),
            layout: app.dock_layouts.get(SESSION).copied(),
            pct: app.terminal_dock_pct,
            view: view_to_restore(app),
            front: app.active_terminal_id.clone(),
            dock_front: app.dock_active.get(SESSION).cloned(),
        }
    }

    /// The actions that may act under the card, and why. Every other action,
    /// from a key or the palette, is refused out loud.
    const ALLOWED: &[(Action, &str)] = &[
        (Action::MoveDown, "moves the selection"),
        (Action::MoveUp, "moves the selection"),
        (Action::NextTab, "selecting a tab is viewing"),
        (Action::PrevTab, "selecting a tab is viewing"),
        (Action::SelectTab1, "selecting a tab is viewing"),
        (Action::SelectTab2, "selecting a tab is viewing"),
        (Action::SelectTab3, "selecting a tab is viewing"),
        (Action::SelectTab4, "selecting a tab is viewing"),
        (Action::SelectTab5, "selecting a tab is viewing"),
        (Action::SelectTab6, "selecting a tab is viewing"),
        (Action::SelectTab7, "selecting a tab is viewing"),
        (Action::SelectTab8, "selecting a tab is viewing"),
        (Action::SelectTab9, "selecting a tab is viewing"),
        (Action::ScrollPageUp, "scrolls nothing the card shows"),
        (Action::ScrollPageDown, "scrolls nothing the card shows"),
        (Action::ScrollLineUp, "scrolls nothing the card shows"),
        (Action::ScrollLineDown, "scrolls nothing the card shows"),
        (Action::ScrollToBottom, "scrolls nothing the card shows"),
        (Action::ScrollToTop, "scrolls nothing the card shows"),
        (Action::FocusNext, "moves between panes"),
        (Action::FocusPrev, "moves between panes"),
        (Action::CloseOverlay, "closes a view"),
        (Action::OpenPalette, "read-only"),
        (Action::ToggleHelp, "read-only"),
        (Action::ForceRedraw, "read-only"),
        (Action::OpenAgentInfo, "read-only"),
        (Action::ReadStartupCommandLogs, "read-only"),
        (Action::ViewServerLog, "read-only"),
        (Action::ResourceMonitor, "read-only"),
        (Action::DebugInput, "read-only"),
        (Action::ShowWelcomeScreen, "read-only"),
        (Action::ShowReleaseNotes, "read-only"),
        (Action::ClearStatusLine, "read-only"),
        (Action::CopyPath, "read-only"),
        (Action::OpenWorktreeInEditor, "opens the folder elsewhere"),
        (Action::ChooseWorktreeEditor, "opens the folder elsewhere"),
        (
            Action::OpenMacroBar,
            "a macro is a write, refused by the typing gate",
        ),
        (Action::FilterAgents, "a view of the list"),
        (Action::SortAgents, "a view of the list"),
        (Action::ToggleProject, "a view of the list"),
        (Action::ToggleSidebar, "a view of this window"),
        (Action::ToggleGitPane, "a view of this window"),
        (Action::RemoveGitPane, "a view of this window"),
        (Action::ToggleResizeMode, "a view of this window"),
        (Action::ToggleDiffLineNumbers, "a preference"),
        (Action::TogglePrBannerPosition, "a preference"),
        (Action::ToggleAlwaysShowTabs, "a preference"),
        (Action::ToggleTabReachesAgent, "a preference"),
        (Action::ToggleNerdFontIcons, "a preference"),
        (Action::ToggleRandomizedPetNameDefault, "a preference"),
        (Action::ChangeTheme, "a preference"),
        (Action::ChangeDefaultProvider, "a preference"),
        (Action::ReloadConfig, "the whole config"),
        (Action::EditMacros, "the whole config"),
        (Action::ConfigureGlobalEnv, "the whole config"),
        (Action::SetTailscaleMode, "the serve"),
        (Action::StartWebServer, "the serve"),
        (Action::StartBackgroundServer, "the serve"),
        (Action::ToggleGithubIntegration, "the whole workspace"),
        (Action::RecheckGithub, "the whole workspace"),
        (Action::Quit, "the whole workspace"),
        (Action::NewAgent, "starts another row"),
        (Action::NewAgentFromPr, "starts another row"),
        (Action::NewAgentFromWorktree, "starts another row"),
        (Action::NewStandaloneAgent, "starts another row"),
        (Action::NewStandaloneTerminal, "starts another row"),
        (Action::NewProjectTerminal, "starts another row"),
        (
            Action::ForkAgent,
            "starts another row; the web keeps it too",
        ),
        (Action::OpenProjectBrowser, "adds a project"),
        (Action::CloneProject, "adds a project"),
        (Action::ManageProjects, "the projects"),
        (
            Action::ConfigureStartupCommand,
            "the project's; the web keeps it too",
        ),
        (
            Action::ConfigureProjectEnv,
            "the project's; the web keeps it too",
        ),
        (Action::MoveTerminalUp, "a terminal row's own place"),
        (Action::MoveTerminalDown, "a terminal row's own place"),
        (Action::MoveTerminalTop, "a terminal row's own place"),
        (Action::MoveTerminalBottom, "a terminal row's own place"),
        (
            Action::ChangeProjectDefaultProvider,
            "project-level, decided separately",
        ),
        (
            Action::ToggleProjectAutoReopenAgents,
            "project-level, decided separately",
        ),
        (Action::RefreshProject, "project-level, decided separately"),
        (
            Action::CheckoutProjectDefaultBranch,
            "project-level, decided separately",
        ),
        (
            Action::ChangeProjectBaseBranch,
            "project-level, decided separately",
        ),
        (Action::DeleteProject, "project-level, decided separately"),
        (Action::RemoveProject, "project-level, decided separately"),
        (Action::ManageWorktrees, "project-level, decided separately"),
        (
            Action::RefreshChanges,
            "the Changes pane, decided separately",
        ),
        (
            Action::OpenCurrentPullRequest,
            "pull requests, decided separately",
        ),
        (Action::NextPullRequest, "pull requests, decided separately"),
        (Action::PullRequests, "pull requests, decided separately"),
        (Action::KillRunning, "the kill list, decided separately"),
        (
            Action::MoveAgentUp,
            "the row's place in the list, decided separately",
        ),
        (
            Action::MoveAgentDown,
            "the row's place in the list, decided separately",
        ),
        (
            Action::MoveAgentTop,
            "the row's place in the list, decided separately",
        ),
        (
            Action::MoveAgentBottom,
            "the row's place in the list, decided separately",
        ),
        (
            Action::StopBackgroundServer,
            "the serve, decided separately",
        ),
    ];

    /// Allowed actions this walk does not run, because they reach outside the
    /// test (a process, a browser, the clipboard, the serve) or move the
    /// selection off the agent.
    const NOT_RUN: &[Action] = &[
        Action::Quit,
        Action::MoveDown,
        Action::MoveUp,
        Action::StartWebServer,
        Action::StartBackgroundServer,
        Action::StopBackgroundServer,
        Action::CopyPath,
        Action::OpenWorktreeInEditor,
        Action::ChooseWorktreeEditor,
        Action::OpenCurrentPullRequest,
        Action::ToggleGithubIntegration,
        Action::RecheckGithub,
        Action::ReloadConfig,
        Action::NewStandaloneTerminal,
        Action::RefreshProject,
        Action::CheckoutProjectDefaultBranch,
        Action::ChangeProjectBaseBranch,
        Action::DeleteProject,
        Action::RemoveProject,
        Action::ManageWorktrees,
        Action::RefreshChanges,
        Action::KillRunning,
        Action::MoveAgentUp,
        Action::MoveAgentDown,
        Action::MoveAgentTop,
        Action::MoveAgentBottom,
        Action::ToggleProjectAutoReopenAgents,
    ];

    fn allowed(action: Action) -> bool {
        ALLOWED.iter().any(|(allowed, _)| *allowed == action)
    }

    /// Close whatever an allowed act opened, the way a user dismisses it.
    fn settle(app: &mut App) {
        while app.close_top_overlay() {}
        app.prompt = PromptState::None;
        app.help_scroll = None;
        app.macro_bar = None;
        app.server_log_viewer = None;
        app.resize_mode = false;
        app.agent_filter = None;
    }

    /// One act under the card, judged: the row and what a take-over gives
    /// back are untouched, and an act not on the allowed list says it was refused
    /// and opens nothing.
    fn judge(
        app: &mut App,
        seat: &TuiOwnership,
        what: &str,
        action: Action,
        act: impl FnOnce(&mut App),
    ) {
        let before = row_state(app, seat);
        app.status = dux_core::statusline::KeyedStatusController::with_clear_after(
            std::time::Duration::from_secs(60),
        );
        act(app);
        let _ = render_rows(app, 160, 40);
        if !allowed(action) {
            let said: Vec<String> = app
                .status
                .snapshot()
                .into_iter()
                .map(|status| status.message)
                .collect();
            let said = said.join(" | ");
            assert!(
                said.contains("so nothing changed"),
                "{what}: not refused: {said:?}"
            );
            assert!(
                !said.contains("erminal"),
                "{what}: the refusal names terminals"
            );
            assert!(
                matches!(app.prompt, PromptState::None),
                "{what}: opened {:?}",
                std::mem::discriminant(&app.prompt)
            );
        }
        settle(app);
        let _ = render_rows(app, 160, 40);
        assert_eq!(row_state(app, seat), before, "{what}");
        assert!(app.agent_row_card().is_some(), "{what}: the card came down");
    }

    /// The user's rule: under the card nothing from this window changes the
    /// agent's row, and only the card's own button claims it. Every palette
    /// command and every binding of the Projects pane, the agent pane and
    /// Global is walked in each state the card can be up in, so an action added
    /// later fails here until it is refused or listed in `ALLOWED` with a reason.
    #[test]
    fn under_the_card_no_command_or_key_changes_the_agents_row() {
        for (state, mut app, seat) in covered_states() {
            for command in dux_core::palette::PALETTE_COMMANDS {
                if NOT_RUN.contains(&command.action) {
                    continue;
                }
                app.focus = FocusPane::Center;
                let what = format!("{state}: palette {}", command.name);
                judge(&mut app, &seat, &what, command.action, |app| {
                    app.execute_command(command.name).unwrap();
                });
            }
            for def in BINDING_DEFS {
                for scope in def.scopes {
                    let pane = match scope {
                        BindingScope::Left => FocusPane::Left,
                        BindingScope::Center | BindingScope::Global => FocusPane::Center,
                        _ => continue,
                    };
                    for combination in def.default_keys {
                        let key = KeyEvent::new(*combination.codes.first(), combination.modifiers);
                        let resolved = app
                            .bindings
                            .lookup(&key, BindingScope::Global)
                            .or_else(|| app.bindings.lookup(&key, *scope));
                        if resolved != Some(def.action) || NOT_RUN.contains(&def.action) {
                            continue;
                        }
                        // In the agent pane the card's own button is this
                        // binding, and a key that types is swallowed by the card.
                        let typing = crate::keybindings::center_typing_owns_key(
                            &key,
                            TabKey::MovesPanes,
                            ExtendedKeys::Off,
                        );
                        if pane == FocusPane::Center && (def.action == Action::FocusAgent || typing)
                        {
                            continue;
                        }
                        let what = format!("{state}: {:?} in {scope:?} ({key:?})", def.action);
                        judge(&mut app, &seat, &what, def.action, |app| {
                            app.focus = pane;
                            app.handle_key(key).unwrap();
                        });
                    }
                }
            }
        }
    }

    /// A click on a pill of the agent's strip selects the tab and launches
    /// nothing, and a drag reorders nothing.
    #[test]
    fn under_the_card_the_agents_strip_selects_but_does_not_reorder_or_launch() {
        let (mut app, seat) = covered(Driver::Browser);
        focus_a_dormant_extra_tab(&mut app);
        app.set_focused_tab(SESSION, SLOT);
        let _ = render_rows(&mut app, 160, 40);
        let before = row_state(&app, &seat);
        let pill = |app: &App, tab: &str| {
            let rect = app
                .agent_tab_regions
                .iter()
                .find(|(id, _)| id == tab)
                .map(|(_, rect)| *rect)
                .expect("the strip shows");
            (rect.x + rect.width / 2, rect.y + 1)
        };
        let (x, y) = pill(&app, "tab-2");
        let (slot_x, _) = pill(&app, SLOT);
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), x),
            (MouseEventKind::Drag(MouseButton::Left), slot_x),
            (MouseEventKind::Up(MouseButton::Left), slot_x),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row: y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
        }
        let _ = render_rows(&mut app, 160, 40);
        assert_eq!(
            app.focused_tab_id(SESSION),
            "tab-2",
            "the click selected it"
        );
        assert_eq!(row_state(&app, &seat), before);
    }

    /// A focused dormant tab does not lift the card a running tab of the same
    /// row is under, and the take-over is carried by that running tab.
    #[test]
    fn a_dormant_tab_beside_a_driven_one_is_covered_and_taken_over_through_it() {
        let (mut app, _recorded, seat) = app_with_a_live_pty_running("sleep 60");
        app.focus = FocusPane::Center;
        let browser = seat.owners.next_conn_id();
        seat.owners.join_row(SLOT, app.engine.pty_row(SLOT));
        seat.owners
            .claim(SLOT, browser)
            .expect("the browser claimed the row");
        focus_a_dormant_extra_tab(&mut app);
        assert!(app.card_covers_center());
        app.execute_command("show-agent").unwrap();
        assert!(
            app.tui_launched_ptys.is_empty(),
            "the dormant tab was launched"
        );

        app.take_over_focused_pty();
        let _ = render_rows(&mut app, 160, 40);
        assert!(!app.card_covers_center());
        assert!(
            seat.owners
                .is_owner(SLOT, app.pty_ownership_conn_id().unwrap())
        );
    }

    /// Opening and closing views under the card, leaving fullscreen
    /// included, changes nothing a take-over gives back: the dock, maximized and
    /// typed into, comes back exactly so.
    #[test]
    fn take_over_gives_back_the_view_the_card_came_up_over() {
        let (mut app, seat) = covered(Driver::Nobody);
        app.take_over_focused_pty();
        let _ = render_rows(&mut app, 160, 40);
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        app.input_target = InputTarget::Terminal;
        let _ = render_rows(&mut app, 160, 40);
        let browser = seat.owners.next_conn_id();
        seat.owners.claim_for_resize(
            SLOT,
            browser,
            dux_core::pty_owners::ResizeIntent::TakeOver {
                expected_owner: None,
            },
            Some(BROWSER),
            |_| {},
        );
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_some(), "the browser took the row");
        app.execute_command("agent-info").unwrap();
        assert!(matches!(app.prompt, PromptState::AgentInfo(_)));
        settle(&mut app);
        let _ = render_rows(&mut app, 160, 40);

        app.focus = FocusPane::Center;
        app.take_over_focused_pty();
        let _ = render_rows(&mut app, 160, 40);
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_none());
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);
        assert_eq!(app.focused_part(), CenterPart::Dock);
    }

    /// The header's terminal count never lets this window work out how many
    /// terminals an agent it does not drive has, whichever row is selected:
    /// a project or standalone terminal's own row is counted as ever.
    #[test]
    fn the_header_counts_no_terminal_of_a_row_this_window_does_not_drive() {
        let (mut app, _seat) = covered(Driver::Browser);
        app.engine
            .create_companion_terminal(SESSION, 24, 80)
            .expect("a second terminal");
        crate::app::test_support::add_terminal(
            &mut app,
            "solo",
            dux_core::model::TerminalOwner::Standalone,
        );
        app.rebuild_left_items();
        let header = |app: &mut App| render_rows(app, 200, 40)[0].clone();
        let with_agent = header(&mut app);
        app.reselect_left_terminal("solo");
        let with_terminal = header(&mut app);
        for row in [&with_agent, &with_terminal] {
            assert!(
                row.contains("1 terminal") && !row.contains("terminals"),
                "{row}"
            );
        }

        app.reselect_left_session(SESSION);
        app.reconcile_center_target();
        app.focus = FocusPane::Center;
        app.take_over_focused_pty();
        let _ = render_rows(&mut app, 200, 40);
        assert!(header(&mut app).contains("3 terminals"));
    }

    /// Asking who drives costs the registry a constant per frame and per key,
    /// whatever the agent holds, and nothing when nothing is serving.
    #[test]
    fn who_drives_is_asked_a_constant_number_of_times_per_frame_and_key() {
        let asks = |app: &mut App, act: &dyn Fn(&mut App)| {
            REGISTRY_ASKS.with(|asks| asks.set(0));
            act(app);
            REGISTRY_ASKS.with(std::cell::Cell::get)
        };
        let frame = |app: &mut App| {
            let _ = render_rows(app, 160, 40);
        };
        let key = |app: &mut App| {
            app.handle_key(KeyEvent::new(
                crossterm::event::KeyCode::Char('x'),
                crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();
        };
        let mut counts = Vec::new();
        for extra_terminals in [0, 3] {
            let (mut app, _seat) = covered(Driver::Browser);
            for _ in 0..extra_terminals {
                app.engine
                    .create_companion_terminal(SESSION, 24, 80)
                    .unwrap();
            }
            frame(&mut app);
            let covered_frame = asks(&mut app, &frame);
            let covered_key = asks(&mut app, &key);
            app.take_over_focused_pty();
            frame(&mut app);
            frame(&mut app);
            assert!(app.agent_row_card().is_none());
            let driving_frame = asks(&mut app, &frame);
            let driving_key = asks(&mut app, &key);
            counts.push((covered_frame, driving_frame, covered_key, driving_key));
        }
        assert_eq!(counts[0], counts[1], "the cost grew with the terminals");
        let (covered_frame, driving_frame, covered_key, driving_key) = counts[0];
        assert!(covered_frame <= 4, "{counts:?}");
        assert!(driving_frame <= 5, "{counts:?}");
        assert!(covered_key <= 3 && driving_key <= 3, "{counts:?}");

        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        assert_eq!(asks(&mut app, &frame), 0, "nothing serving asks nothing");
    }
}
