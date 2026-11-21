//! Whether this surface may change an agent's row while the take-over card is
//! over it: which actions and dialogs would, what is dropped when the card
//! comes up, and the view a take-over gives back. The calls that apply a
//! change ask the same question in `row_changes`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use super::pty_ownership::{PtyTakeoverCard, UNNAMED_DEVICE};
use super::{
    App, CompanionTerminal, DraggedDivider, FullscreenOverlay, InputTarget, PromptState,
    TerminalOwnerRef,
};
use crate::keybindings::Action;
use crate::model::SessionSurface;
use dux_core::ids::TabIdRef;
use dux_core::pty_owners::RowKey;

/// Whether `action`, on the selected agent, would change its row: start or
/// stop any of it, add, close or reorder a tab or terminal, or change what the
/// agent is. Exhaustive, so a new action does not compile until it is decided.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per action, so a new action does not compile until it is decided"
)]
pub(crate) fn changes_agent_row(action: Action) -> bool {
    match action {
        Action::ReconnectAgent
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
        // Looking: under the card each shows the card and starts nothing.
        Action::FocusAgent
        | Action::InteractAgent
        | Action::ToggleFullscreen
        | Action::MoveDown
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

/// The agent rows this surface drives, as of one generation of the registry.
struct DrivenRows {
    conn_id: u64,
    generation: u64,
    agents: HashSet<String>,
}

/// What this surface has heard from the registry: each agent row's card, once
/// per pass and live outside one, and the rows it drives, until ownership moves.
#[derive(Default)]
pub(crate) struct RowCardMemo {
    passes: Cell<u32>,
    answers: RefCell<HashMap<String, RowAnswer>>,
    /// The registry's generation this pass heard, and the seat it heard it as.
    generation: Cell<Option<(u64, u64)>>,
    driven: RefCell<Option<DrivenRows>>,
    placed: RefCell<(Option<u64>, HashSet<String>)>,
}

impl RowCardMemo {
    fn begin(&self) {
        self.passes.set(self.passes.get() + 1);
    }

    fn end(&self) {
        let passes = self.passes.get().saturating_sub(1);
        self.passes.set(passes);
        if passes == 0 {
            self.forget();
        }
    }

    /// Drop what this pass heard, for when this surface itself changed who drives.
    pub(crate) fn forget(&self) {
        self.answers.borrow_mut().clear();
        self.generation.set(None);
    }

    /// The card over `session_id`'s row, asking `ask` only when this pass has
    /// not asked about that row yet.
    fn answer(&self, session_id: &str, ask: impl FnOnce() -> RowAnswer) -> RowAnswer {
        if self.passes.get() == 0 {
            return ask();
        }
        if let Some(answer) = self.answers.borrow().get(session_id) {
            return answer.clone();
        }
        let answer = ask();
        self.answers
            .borrow_mut()
            .insert(session_id.to_string(), answer.clone());
        answer
    }

    /// Whether the seat `conn_id` has yet to place `pty_id` in its row, noting
    /// it placed: a pty never changes rows, so the registry is told once.
    pub(crate) fn first_placing(&self, conn_id: u64, pty_id: &str) -> bool {
        let mut placed = self.placed.borrow_mut();
        if placed.0 != Some(conn_id) {
            *placed = (Some(conn_id), HashSet::new());
        }
        placed.1.insert(pty_id.to_string())
    }

    /// Remember the registry's generation an answer of this pass came with,
    /// as the seat `conn_id` heard it.
    pub(crate) fn note_generation(&self, conn_id: u64, generation: u64) {
        if self.passes.get() > 0 {
            self.generation.set(Some((conn_id, generation)));
        }
    }

    /// The registry's generation as the seat `conn_id` hears it, asking `ask`
    /// only when this pass has heard it as that seat: a generation another
    /// seat heard is another registry's.
    fn generation(&self, conn_id: u64, ask: impl FnOnce() -> u64) -> u64 {
        if let Some((heard_by, generation)) = self.generation.get()
            && heard_by == conn_id
        {
            return generation;
        }
        let generation = ask();
        self.note_generation(conn_id, generation);
        generation
    }

    /// Forget every pty placed in its row that `exists` no longer knows.
    pub(crate) fn forget_placed_unless(&self, exists: impl Fn(&str) -> bool) {
        self.placed.borrow_mut().1.retain(|pty_id| exists(pty_id));
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
    /// Run `act` as one pass, so every question about a row inside it costs
    /// the registry one ask.
    pub(crate) fn in_row_pass<T>(&mut self, act: impl FnOnce(&mut Self) -> T) -> T {
        self.row_card_memo.begin();
        let out = act(self);
        self.row_card_memo.end();
        out
    }

    /// One pass of input. Work the card came up over is dropped first, and
    /// the event that found it is spent on that: `dropped` is its answer.
    pub(crate) fn in_input_pass<T>(&mut self, dropped: T, act: impl FnOnce(&mut Self) -> T) -> T {
        self.in_row_pass(|app| {
            if app.keep_row_view() {
                return dropped;
            }
            act(app)
        })
    }

    /// One live pty of `session_id`'s row: the registry keeps one driver per
    /// row, so any of them answers for all. The focused tab's comes first.
    pub(crate) fn row_probe_of(&self, session_id: &str) -> Option<String> {
        let live = |tab: &String| self.engine.providers.contains_key(TabIdRef::new(tab));
        let focused = self.focused_tab_id(session_id);
        if live(&focused) {
            return Some(focused);
        }
        self.session_tab_ids(session_id)
            .into_iter()
            .find(live)
            .or_else(|| self.dock_terminal_ids(session_id).into_iter().next())
    }

    /// The card over `session_id`'s row and the pty it was read from: `None`
    /// when this surface drives it, nothing of it runs, or nothing is serving.
    pub(crate) fn row_card_of(&self, session_id: &str) -> Option<(PtyTakeoverCard, String)> {
        self.pty_ownership_conn_id()?;
        let probe = self.row_probe_of(session_id)?;
        self.row_card_memo.answer(session_id, || {
            Some((self.pty_takeover_card(&probe)?, probe.clone()))
        })
    }

    /// The card over the selected agent's row, and the pty it was read from.
    pub(crate) fn agent_row_card(&self) -> Option<(PtyTakeoverCard, String)> {
        self.pty_ownership_conn_id()?;
        if self.selected_terminal_row().is_some() {
            return None;
        }
        self.row_card_of(&self.selected_session()?.id)
    }

    /// Whether this surface may not change the selected agent's row, saying so
    /// on the status line when it may not.
    pub(crate) fn refuse_row_change(&mut self) -> bool {
        if self.selected_terminal_row().is_some() {
            return false;
        }
        let Some(session_id) = self.selected_session().map(|session| session.id.clone()) else {
            return false;
        };
        self.refuse_row_change_of(&session_id)
    }

    /// Whether this surface may not change `session_id`'s row, saying on the
    /// status line who drives it and how to take it over when it may not.
    ///
    /// A take-over armed for the row and not yet carried by a frame is carried
    /// first, so a change in the same burst of input as the press applies
    /// once the claim has landed, and is refused when it has not.
    pub(crate) fn refuse_row_change_of(&mut self, session_id: &str) -> bool {
        let armed = Some(RowKey::Agent(session_id.to_string()));
        if self.pending_pty_takeover == armed
            && let Some(pty_id) = self.row_probe_of(session_id)
        {
            self.spend_take_over_on(&pty_id);
        }
        let Some((card, _)) = self.row_card_of(session_id) else {
            return false;
        };
        let name = self.engine.session_by_id(session_id).map_or_else(
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
            "{who}, so nothing changed. Press {key} on its card in the center pane to take \
             it over first."
        ));
        true
    }

    /// The agent rows this surface drives, read from the registry again only
    /// when ownership moved since the last read.
    fn driven_agent_rows(&self) -> std::cell::Ref<'_, Option<DrivenRows>> {
        let memo = &self.row_card_memo;
        if let Some(seat) = self.pty_ownership() {
            let generation = memo.generation(seat.conn_id, || seat.owners.ownership_generation());
            let fresh = memo.driven.borrow().as_ref().is_some_and(|driven| {
                driven.conn_id == seat.conn_id && driven.generation == generation
            });
            if !fresh {
                let agents = seat
                    .owners
                    .input_owners_snapshot()
                    .0
                    .into_iter()
                    .filter_map(|(row, owner)| match row {
                        RowKey::Agent(session_id) if owner == seat.conn_id => Some(session_id),
                        RowKey::Agent(_) | RowKey::Terminal(_) => None,
                    })
                    .collect();
                *memo.driven.borrow_mut() = Some(DrivenRows {
                    conn_id: seat.conn_id,
                    generation,
                    agents,
                });
            }
        } else {
            memo.driven.borrow_mut().take();
        }
        memo.driven.borrow()
    }

    /// The running terminals this surface may count: every terminal that is a
    /// row of the list, and an agent's only while this surface drives its row.
    pub(crate) fn shown_terminal_count(&self) -> usize {
        let terminals = &self.engine.companion_terminals;
        let docked = |terminal: &CompanionTerminal| match terminal.owner.as_ref() {
            TerminalOwnerRef::Session(session_id) => Some(session_id.to_string()),
            TerminalOwnerRef::Project(_) | TerminalOwnerRef::Standalone => None,
        };
        if self.pty_ownership_conn_id().is_none()
            || !terminals.values().any(|t| docked(t).is_some())
        {
            return terminals.len();
        }
        let driven = self.driven_agent_rows();
        let agents = driven.as_ref().map(|driven| &driven.agents);
        terminals
            .values()
            .filter(|terminal| {
                docked(terminal).is_none_or(|session_id| {
                    agents.is_some_and(|agents| agents.contains(&session_id))
                })
            })
            .count()
    }

    /// The agent whose row answering the open dialog would change, when it
    /// would. Exhaustive, so a new dialog does not compile until it is decided.
    pub(crate) fn prompt_row_target(&self) -> Option<String> {
        match &self.prompt {
            PromptState::ChangeAgentProvider(prompt) => Some(prompt.session_id.clone()),
            PromptState::ConfirmDeleteAgent { session_id, .. }
            | PromptState::ConfirmCloseTab { session_id, .. }
            | PromptState::ConfirmStopTab { session_id, .. }
            | PromptState::ConfirmDetachAgent { session_id, .. }
            | PromptState::ConfirmRecreateWorkingCopy { session_id, .. }
            | PromptState::RenameSession { session_id, .. } => Some(session_id.clone()),
            PromptState::ConfirmDeleteTerminal { terminal_id, .. } => self
                .engine
                .companion_terminals
                .get(terminal_id)
                .and_then(|terminal| match terminal.owner.as_ref() {
                    TerminalOwnerRef::Session(session_id) => Some(session_id.to_string()),
                    TerminalOwnerRef::Project(_) | TerminalOwnerRef::Standalone => None,
                }),
            PromptState::None
            | PromptState::Command { .. }
            | PromptState::BrowseProjects { .. }
            | PromptState::AddProjectFailed { .. }
            | PromptState::ConfirmInitRepo { .. }
            | PromptState::AgentInfo(_)
            | PromptState::FirstLoad(_)
            | PromptState::ChangeDefaultProvider(_)
            | PromptState::ChangeProjectDefaultProvider(_)
            | PromptState::SetTailscaleMode(_)
            | PromptState::ChangeTheme(_)
            | PromptState::ConfigureStartupCommand { .. }
            | PromptState::ConfigureProjectEnv { .. }
            | PromptState::ConfigureGlobalEnv { .. }
            | PromptState::StartupCommandLogs(_)
            | PromptState::PickProject { .. }
            | PromptState::ProjectActions(_)
            | PromptState::ProjectInfo(_)
            | PromptState::ChangeBaseBranch(_)
            | PromptState::ConfirmChangeBaseBranch(_)
            | PromptState::PickProjectWorktree(_)
            | PromptState::ManageWorktrees(_)
            | PromptState::ConfirmDeleteWorktree(_)
            | PromptState::LeftoverFolderActions(_)
            | PromptState::ConfirmDeleteLeftoverFolder(_)
            | PromptState::KillRunning(_)
            | PromptState::ConfirmKillRunning(_)
            | PromptState::ConfigReloadFailed { .. }
            | PromptState::ConfirmCheckoutDefaultBranch { .. }
            | PromptState::ConfirmDeleteProject { .. }
            | PromptState::ConfirmRemoveProject { .. }
            | PromptState::ConfirmQuit { .. }
            | PromptState::ConfirmDiscardFile { .. }
            | PromptState::ConfirmCreateInitialCommit { .. }
            | PromptState::PullRequestInput { .. }
            | PromptState::AttachPullRequestInput { .. }
            | PromptState::PullRequests(_)
            | PromptState::NameStandaloneAgent { .. }
            | PromptState::CloneProject { .. }
            | PromptState::NameNewAgent { .. }
            | PromptState::PickEditor { .. }
            | PromptState::EditMacros { .. }
            | PromptState::ConfirmNonDefaultBranch { .. }
            | PromptState::ConfirmUseExistingBranch { .. }
            | PromptState::DebugInput { .. }
            | PromptState::ResourceMonitor { .. } => None,
        }
    }

    /// Abandon a strip or divider drag, and a dialog whose answer would change
    /// a row, that the card has come up over, saying why. True when one went.
    fn drop_work_the_card_came_up_over(&mut self) -> bool {
        let mut dropped = false;
        if let Some(drag) = &self.tab_drag
            && self.row_card_of(&drag.session_id).is_some()
        {
            let (session_id, promoted) = (drag.session_id.clone(), drag.promoted);
            self.tab_drag = None;
            if promoted {
                self.refuse_row_change_of(&session_id);
            }
            dropped = true;
        }
        if self.mouse_drag == Some(DraggedDivider::Dock) && self.refuse_row_change() {
            self.abandon_dock_divider_drag();
            dropped = true;
        }
        if let Some(session_id) = self.prompt_row_target()
            && self.refuse_row_change_of(&session_id)
        {
            self.prompt = PromptState::None;
            dropped = true;
        }
        if dropped {
            self.mark_frame_dirty();
        }
        dropped
    }

    /// End a divider drag without saving it, the split share back to what it
    /// was when the drag began.
    pub(crate) fn abandon_dock_divider_drag(&mut self) {
        self.mouse_drag = None;
        self.terminal_dock_pct = self.dock_drag_began_pct;
    }

    fn current_row_view(&self) -> RowView {
        RowView {
            input_target: self.input_target,
            fullscreen_overlay: self.fullscreen_overlay,
            session_surface: self.session_surface,
        }
    }

    /// Hold the selected agent's view when the card comes up over its row and
    /// give it back when a take-over lifts the card. Leaving the row lets go
    /// of it, and serving ending lands the row windowed and not typing
    /// instead. True when work the card came up over was dropped.
    pub(crate) fn keep_row_view(&mut self) -> bool {
        let dropped = self.drop_work_the_card_came_up_over();
        let selected = self.selected_session().map(|session| session.id.clone());
        let covered = self.agent_row_card().is_some();
        match (self.held_row_view.take(), selected) {
            (Some((held, view)), Some(selected)) if held == selected => {
                if covered {
                    self.held_row_view = Some((held, view));
                } else if self.pty_ownership_conn_id().is_none() {
                    // Serving ended with no stop to land it (the serve retired
                    // itself): nothing was taken over, so nothing is given back.
                    self.land_windowed_and_not_typing();
                } else if self.row_probe_of(&selected).is_some() {
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
        dropped
    }

    /// Serving is ending: a maximized or typed-into view under a card would
    /// take the keyboard as the card goes, so it lands windowed and not typing.
    pub(crate) fn land_off_the_card(&mut self) {
        let typing = matches!(
            self.input_target,
            InputTarget::Agent | InputTarget::Terminal
        );
        let maximized = matches!(
            self.fullscreen_overlay,
            FullscreenOverlay::Agent | FullscreenOverlay::Terminal
        );
        let held = self.held_row_view.as_ref().is_some_and(|(_, view)| {
            view.input_target != InputTarget::None
                || view.fullscreen_overlay != FullscreenOverlay::None
        });
        if !(typing || maximized || held) || !self.card_covers_center() {
            return;
        }
        self.held_row_view = None;
        self.land_windowed_and_not_typing();
    }

    /// Leave the agent or terminal typed into or shown maximized.
    fn land_windowed_and_not_typing(&mut self) {
        if matches!(
            self.input_target,
            InputTarget::Agent | InputTarget::Terminal
        ) {
            self.input_target = InputTarget::None;
        }
        if matches!(
            self.fullscreen_overlay,
            FullscreenOverlay::Agent | FullscreenOverlay::Terminal
        ) {
            self.fullscreen_overlay = FullscreenOverlay::None;
        }
        self.mark_frame_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::pty_ownership::tests::{app_with_a_live_pty_running, render_rows};
    use crate::app::{CenterPart, ConfirmAnswer, DockLayout, FocusPane, PromptState};
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
            owner: seat.owners.current_owner(SLOT).0,
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
        (Action::FocusAgent, LOOKS),
        (Action::InteractAgent, LOOKS),
        (Action::ToggleFullscreen, LOOKS),
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
        (Action::OpenDiff, "the Changes pane, decided separately"),
        (Action::StageUnstage, "the Changes pane, decided separately"),
        (
            Action::CommitChanges,
            "the Changes pane, decided separately",
        ),
        (
            Action::DiscardChanges,
            "the Changes pane, decided separately",
        ),
        (
            Action::EngageCommitInput,
            "the Changes pane, decided separately",
        ),
        (Action::PushToRemote, "the Changes pane, decided separately"),
        (
            Action::PullFromRemote,
            "the Changes pane, decided separately",
        ),
        (Action::SearchFiles, "the Changes pane, decided separately"),
        (Action::SearchNext, "the Changes pane, decided separately"),
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

    /// Why an action is allowed when it shows the card and starts nothing,
    /// which the walk also holds it to by saying no refusal.
    const LOOKS: &str = "looks at the row, showing its card and starting nothing";

    /// Allowed actions this walk does not run, each for what running it would
    /// reach outside the test.
    const NOT_RUN: &[(Action, &str)] = &[
        (Action::StartWebServer, "hands the terminal to the server"),
        (Action::StartBackgroundServer, "binds real listeners"),
        (
            Action::StopBackgroundServer,
            "ends the serve and with it the card; its landing has a test of its own",
        ),
        (Action::CopyPath, "writes the system clipboard"),
        (Action::OpenWorktreeInEditor, "starts the user's editor"),
        (Action::ToggleGithubIntegration, "runs the gh probe"),
        (Action::RecheckGithub, "runs the gh probe"),
        (Action::RefreshProject, "runs git pull on a worker"),
        (Action::CheckoutProjectDefaultBranch, "runs git on a worker"),
        (Action::ChangeProjectBaseBranch, "runs git on a worker"),
        (Action::ManageWorktrees, "runs git on a worker"),
        (Action::RefreshChanges, "runs git on a worker"),
        (Action::PushToRemote, "runs git push on a worker"),
        (Action::PullFromRemote, "runs git pull on a worker"),
    ];

    fn not_run(action: Action) -> bool {
        NOT_RUN.iter().any(|(skipped, _)| *skipped == action)
    }

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
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.center_mode = crate::app::CenterMode::Agent;
        app.rebuild_left_items();
        app.reselect_left_session(SESSION);
        app.reconcile_center_target();
    }

    /// One act under the card, judged: the row and what a take-over gives
    /// back are untouched, and an act not on the allowed list says it was refused
    /// and opens nothing. An act that moves the selection off the row resets
    /// what a take-over gives back, as leaving the row does, so that alone may
    /// differ then.
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
        let left_the_row = app
            .selected_session()
            .is_none_or(|session| session.id != SESSION)
            || app.selected_terminal_row().is_some();
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
        } else if ALLOWED.contains(&(action, LOOKS)) {
            assert!(
                !said(app).contains("so nothing changed"),
                "{what}: looking was refused: {}",
                said(app)
            );
        }
        settle(app);
        let _ = render_rows(app, 160, 40);
        let after = row_state(app, seat);
        if left_the_row {
            let view = before.view;
            assert_eq!(RowState { view, ..after }, before, "{what}");
        } else {
            assert_eq!(after, before, "{what}");
        }
        assert!(app.agent_row_card().is_some(), "{what}: the card came down");
    }

    /// The user's rule: under the card nothing from this window changes the
    /// agent's row, and only the card's own button claims it. Every palette
    /// command and every binding of the Projects pane, the agent pane, the
    /// Changes pane, the maximized pane and Global is walked in each state the
    /// card can be up in, so an action added later fails here until it is
    /// refused or listed in `ALLOWED` with a reason.
    #[test]
    fn under_the_card_no_command_or_key_changes_the_agents_row() {
        for (state, mut app, seat) in covered_states() {
            for command in dux_core::palette::PALETTE_COMMANDS {
                if not_run(command.action) {
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
                        BindingScope::Center | BindingScope::Global | BindingScope::Interactive => {
                            FocusPane::Center
                        }
                        BindingScope::Files => FocusPane::Files,
                        // Bindings of a dialog or overlay: each is reached
                        // only through a command or key walked here.
                        _ => continue,
                    };
                    if not_run(def.action) {
                        continue;
                    }
                    if *scope == BindingScope::Interactive {
                        // Typed straight at the maximized pane, where the
                        // card's own button is the focus binding.
                        let button = app.bindings.byte_patterns_for(Action::FocusAgent);
                        let Some(bytes) = app
                            .bindings
                            .byte_patterns_for(def.action)
                            .into_iter()
                            .find(|bytes| bytes != b" " && !button.contains(bytes))
                        else {
                            continue;
                        };
                        let what = format!("{state}: {:?} typed at the maximized pane", def.action);
                        judge(&mut app, &seat, &what, def.action, |app| {
                            app.focus = FocusPane::Center;
                            app.fullscreen_overlay = FullscreenOverlay::Agent;
                            app.input_target = InputTarget::Agent;
                            app.process_raw_input_bytes(&bytes);
                        });
                        continue;
                    }
                    for combination in def.default_keys {
                        let key = KeyEvent::new(*combination.codes.first(), combination.modifiers);
                        let resolved = app
                            .bindings
                            .lookup(&key, BindingScope::Global)
                            .or_else(|| app.bindings.lookup(&key, *scope));
                        if resolved != Some(def.action) {
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

    /// The browser takes the row from whoever holds it.
    fn browser_takes(seat: &TuiOwnership) {
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
    }

    /// The fixture's agent with this window driving its row.
    fn driving() -> (App, TuiOwnership) {
        let (mut app, seat) = covered(Driver::Nobody);
        app.take_over_focused_pty();
        let _ = render_rows(&mut app, 160, 40);
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_none(), "this window drives");
        (app, seat)
    }

    fn said(app: &App) -> String {
        let said: Vec<String> = app
            .status
            .snapshot()
            .into_iter()
            .map(|status| status.message)
            .collect();
        said.join(" | ")
    }

    fn quiet(app: &mut App) {
        app.status = dux_core::statusline::KeyedStatusController::with_clear_after(
            std::time::Duration::from_secs(60),
        );
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )
    }

    /// How a row-changing action finishes once it has begun.
    enum Finish {
        /// It is applied where it starts, by the same key or command that the
        /// walk above refuses under the card: no dialog carries it further.
        AtOnce,
        /// It opens a dialog: how to get it open while driving, and its own
        /// confirming function, which a key reaches only through the dialog.
        Dialog {
            open: fn(&mut App),
            confirm: fn(&mut App),
        },
    }

    fn palette(app: &mut App, command: &str) {
        app.focus = FocusPane::Center;
        app.execute_command(command).unwrap();
    }

    /// Exhaustive over the actions the classifier says change the row, so a
    /// new one is not walked until somebody says how it finishes.
    fn finish(action: Action) -> Finish {
        use dux_core::engine::BranchRename;
        match action {
            Action::ReconnectAgent
            | Action::ForceReconnectAgent
            | Action::RerunStartupCommandOnAgent
            | Action::ToggleAgentAutoReopen
            | Action::MoveTabLeft
            | Action::MoveTabRight
            | Action::ShowTerminal
            | Action::CycleTerminalLayout
            | Action::NewTerminal => Finish::AtOnce,
            Action::RenameSession => Finish::Dialog {
                open: |app| palette(app, "rename-agent"),
                confirm: |app| {
                    app.apply_rename_session(SESSION, "renamed", BranchRename::NameOnly);
                },
            },
            Action::ChangeAgentProvider => Finish::Dialog {
                open: |app| palette(app, "change-agent-provider"),
                confirm: |app| {
                    another_provider(app);
                    app.apply_change_agent_provider().unwrap();
                },
            },
            Action::NewTab => Finish::Dialog {
                open: |app| {
                    app.session_surface = SessionSurface::Agent;
                    app.open_new_tab_provider_prompt().unwrap();
                },
                confirm: |app| app.apply_change_agent_provider().unwrap(),
            },
            Action::CloseTab => Finish::Dialog {
                open: |app| {
                    focus_a_dormant_extra_tab(app);
                    app.close_focused_tab_prompt();
                },
                confirm: |app| {
                    app.resolve_confirm_close_tab(ConfirmAnswer::Confirm);
                },
            },
            Action::StopTab => Finish::Dialog {
                open: App::stop_focused_tab_prompt,
                confirm: |app| {
                    app.resolve_confirm_stop_tab(ConfirmAnswer::Confirm);
                },
            },
            Action::DetachAgent => Finish::Dialog {
                open: |app| palette(app, "detach-agent"),
                confirm: |app| {
                    app.resolve_confirm_detach_agent(ConfirmAnswer::Confirm);
                },
            },
            Action::DeleteSession => Finish::Dialog {
                open: |app| {
                    app.session_surface = SessionSurface::Agent;
                    app.confirm_delete_selected_session();
                },
                confirm: |app| {
                    app.resolve_confirm_delete_agent(ConfirmAnswer::Confirm);
                },
            },
            Action::CloseTerminal => Finish::Dialog {
                open: App::confirm_close_focused_terminal,
                confirm: |app| {
                    app.resolve_confirm_delete_terminal(ConfirmAnswer::Confirm);
                },
            },
            Action::RecreateWorkingCopy => Finish::Dialog {
                open: |app| {
                    app.prompt = PromptState::ConfirmRecreateWorkingCopy {
                        session_id: SESSION.to_string(),
                        worktree_path: std::path::PathBuf::from("/gone"),
                        branch_name: "branch".to_string(),
                        source_branch: "main".to_string(),
                        conversation_resumes: true,
                        running_tabs: dux_core::working_copy::RunningTabs::default(),
                        restart_tabs: false,
                        restart_offer: Vec::new(),
                        focus: crate::app::RecreateWorkingCopyFocus::Cancel,
                        attached: Vec::new(),
                    };
                },
                confirm: |app| {
                    app.resolve_confirm_recreate_working_copy(ConfirmAnswer::Confirm);
                },
            },
            other => {
                assert!(
                    !changes_agent_row(other),
                    "say how {other:?} finishes: at once, or through a dialog"
                );
                Finish::AtOnce
            }
        }
    }

    /// Point the open provider picker at a provider the agent does not use.
    fn another_provider(app: &mut App) {
        let PromptState::ChangeAgentProvider(prompt) = &mut app.prompt else {
            panic!("the provider picker is open");
        };
        prompt.selected = prompt
            .options
            .iter()
            .position(|option| !option.is_current)
            .expect("a second provider is configured");
    }

    /// The user's rule holds whenever a change began: a dialog opened while
    /// this window drove the row changes nothing once another device drives
    /// it. Asked two ways for every action that goes through a dialog: the
    /// next key closes the dialog and says why, and the dialog's own
    /// confirming function, reached without the key, refuses by itself.
    #[test]
    fn a_dialog_opened_while_driving_changes_nothing_once_another_device_drives() {
        let mut actions: Vec<Action> = BINDING_DEFS.iter().map(|def| def.action).collect();
        actions.extend(
            dux_core::palette::PALETTE_COMMANDS
                .iter()
                .map(|command| command.action),
        );
        let mut walked = 0;
        for action in actions {
            let Finish::Dialog { open, confirm } = finish(action) else {
                continue;
            };
            walked += 1;
            for through_a_key in [true, false] {
                let what = format!("{action:?}, through a key: {through_a_key}");
                let (mut app, seat) = driving();
                open(&mut app);
                assert_eq!(
                    app.prompt_row_target().as_deref(),
                    Some(SESSION),
                    "{what}: its dialog is open on the agent: {}",
                    said(&app)
                );
                browser_takes(&seat);
                let before = row_state(&app, &seat);
                quiet(&mut app);
                if through_a_key {
                    app.handle_key(enter()).unwrap();
                } else {
                    confirm(&mut app);
                }
                assert!(
                    said(&app).contains("so nothing changed"),
                    "{what}: not refused: {:?}",
                    said(&app)
                );
                if through_a_key {
                    assert!(matches!(app.prompt, PromptState::None), "{what}");
                }
                app.prompt = PromptState::None;
                assert_eq!(row_state(&app, &seat), before, "{what}");
            }
        }
        assert!(walked >= 9, "the dialogs were walked");
    }

    /// Every engine call that changes an agent's row refuses for a row this
    /// window does not drive, whoever calls it and whenever.
    #[test]
    fn every_row_changing_engine_call_refuses_a_row_this_window_does_not_drive() {
        use dux_core::engine::{BranchRename, ReconnectMode, TabsAfterRecreate};
        let (mut app, seat) = covered(Driver::Browser);
        let terminal = app.dock_terminal_ids(SESSION)[0].clone();
        let provider = dux_core::model::ProviderKind::new("claude");
        let session = app.engine.sessions[0].clone();
        let request = app.agent_launch_request(
            session.clone(),
            dux_core::model::ConversationStart::Resume,
            dux_core::worker::AgentLaunchKind::Reconnect {
                status_message: String::new().into(),
            },
        );
        let before = row_state(&app, &seat);
        let refused = [
            app.row_rename_agent(SESSION, "renamed", BranchRename::NameOnly)
                .is_none(),
            app.row_create_tab(SESSION, provider.clone(), (24, 80))
                .is_none(),
            app.row_close_tab(SESSION, SLOT).is_none(),
            app.row_stop_tab(SESSION, SLOT).is_none(),
            app.row_change_tab_provider(SESSION, SLOT, &provider)
                .is_none(),
            app.row_toggle_auto_reopen(&session).is_none(),
            app.row_begin_detach(SESSION).is_none(),
            app.row_begin_delete(
                SESSION,
                dux_core::engine::WorktreeOnDelete::Keep,
                None,
                None,
            )
            .is_none(),
            app.row_begin_recreate(SESSION, TabsAfterRecreate::Leave)
                .is_none(),
            app.row_claim_startup_rerun(SESSION, "agent", "/tmp")
                .is_none(),
            app.row_reconnect_plan(SESSION, ReconnectMode::Force, (24, 80))
                .is_none(),
            app.row_dispatch_launch(request).is_none(),
            app.row_create_terminal(SESSION, 24, 80).is_none(),
            app.row_reorder_tabs(SESSION, vec![SLOT.to_string()])
                .is_none(),
            app.row_reorder_dock(SESSION, vec![terminal.clone()])
                .is_none(),
            app.row_close_terminal(&terminal).is_none(),
        ];
        assert!(refused.iter().all(|refused| *refused), "{refused:?}");
        assert_eq!(row_state(&app, &seat), before);
        assert!(said(&app).contains("so nothing changed"));
    }

    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
    }

    fn pill(regions: &[(String, ratatui::layout::Rect)], id: &str) -> (u16, u16) {
        let rect = regions
            .iter()
            .find(|(pill, _)| pill == id)
            .map(|(_, rect)| *rect)
            .expect("the strip shows the pill");
        (rect.x + rect.width / 2, rect.y + 1)
    }

    /// A pill picked up while driving and dropped after another device took
    /// the row reorders nothing, on the agent's strip and on the dock's.
    #[test]
    fn a_pill_drag_begun_while_driving_reorders_nothing_under_the_card() {
        let left = MouseButton::Left;
        let (mut app, seat) = driving();
        focus_a_dormant_extra_tab(&mut app);
        app.engine
            .create_companion_terminal(SESSION, 24, 80)
            .expect("a second terminal");
        app.set_focused_tab(SESSION, SLOT);
        let _ = render_rows(&mut app, 160, 40);
        let dock = app.dock_terminal_ids(SESSION);
        let strips = [
            (app.agent_tab_regions.clone(), "tab-2", SLOT),
            (
                app.mouse_layout.dock_pills.clone(),
                dock[1].as_str(),
                dock[0].as_str(),
            ),
        ];
        for (regions, picked, target) in strips {
            let app = &mut app;
            app.take_over_focused_pty();
            let _ = render_rows(app, 160, 40);
            let _ = render_rows(app, 160, 40);
            assert!(app.agent_row_card().is_none());
            let (x, y) = pill(&regions, picked);
            mouse(app, MouseEventKind::Down(left), x, y);
            assert!(app.tab_drag.is_some(), "{picked} was picked up");
            browser_takes(&seat);
            let before = row_state(app, &seat);
            let (x, y) = pill(&regions, target);
            mouse(app, MouseEventKind::Drag(left), x, y);
            mouse(app, MouseEventKind::Up(left), x, y);
            assert!(app.tab_drag.is_none());
            assert_eq!(row_state(app, &seat), before, "{picked} moved");
        }
    }

    /// The divider grabbed while driving and dragged after another device took
    /// the row moves neither the layout nor the split share, in memory or in
    /// the config a release would save, and the drag is over.
    #[test]
    fn a_divider_drag_begun_while_driving_changes_nothing_under_the_card() {
        let left = MouseButton::Left;
        let (mut app, seat) = driving();
        let divider = app.mouse_layout.dock_divider.expect("the split's divider");
        let stack = app.mouse_layout.center_stack.expect("the stack");
        let began = app.terminal_dock_pct;
        mouse(
            &mut app,
            MouseEventKind::Down(left),
            divider.x + 5,
            divider.y,
        );
        assert_eq!(app.mouse_drag, Some(crate::app::DraggedDivider::Dock));
        browser_takes(&seat);
        let before = RowState {
            pct: began,
            ..row_state(&app, &seat)
        };
        let saved = app.engine.config.ui.terminal_dock_pct;
        quiet(&mut app);
        for row in [divider.y - 3, stack.y] {
            mouse(&mut app, MouseEventKind::Drag(left), divider.x + 5, row);
        }
        mouse(&mut app, MouseEventKind::Up(left), divider.x + 5, stack.y);
        assert!(app.mouse_drag.is_none(), "the drag was abandoned");
        assert_eq!(row_state(&app, &seat), before);
        assert_eq!(app.engine.config.ui.terminal_dock_pct, saved);
        assert!(said(&app).contains("so nothing changed"), "{}", said(&app));
    }

    /// The card over a maximized agent or a maximized terminal of an agent:
    /// it is the row's card, speaking of the agent alone whoever drives or
    /// nobody does, typing under it reaches nothing, and its button, Space
    /// and the focus binding each take the row over, also where the part on
    /// screen has no process to carry the claim (a stopped agent beside its
    /// running terminal, a dormant focused tab). The maximized view comes back.
    #[test]
    fn a_maximized_parts_card_is_the_rows_and_takes_it_over_by_every_press() {
        #[derive(Clone, Copy, Debug)]
        enum Press {
            Space,
            FocusBinding,
            Button,
        }
        let views = [
            (FullscreenOverlay::Terminal, InputTarget::Terminal),
            (FullscreenOverlay::Agent, InputTarget::Agent),
        ];
        let cases = [
            (true, Driver::Browser, Press::Space),
            (false, Driver::Browser, Press::FocusBinding),
            (true, Driver::Nobody, Press::Button),
            (false, Driver::Nobody, Press::Space),
        ];
        for (overlay, target) in views {
            for (stopped, driver, press) in cases {
                let what = format!("{overlay:?}, stopped: {stopped}, {press:?}");
                let (mut app, seat) = driving();
                let conn = app.pty_ownership_conn_id().unwrap();
                if stopped {
                    app.engine.providers.remove(TabIdRef::new(SLOT));
                } else {
                    focus_a_dormant_extra_tab(&mut app);
                }
                app.fullscreen_overlay = overlay;
                app.input_target = target;
                let _ = render_rows(&mut app, 160, 40);
                let title = if driver == Driver::Browser {
                    browser_takes(&seat);
                    "Active on"
                } else {
                    seat.owners.release(SLOT, conn);
                    "Running in the background"
                };
                quiet(&mut app);
                let rows = render_rows(&mut app, 160, 40);
                assert!(app.agent_row_card().is_some(), "{what}");
                assert!(
                    rows.iter().any(|row| row.contains(title))
                        && rows.iter().any(|row| row.contains("Take over")),
                    "{what}: the maximized view shows the row's card:\n{}",
                    rows.join("\n")
                );
                let about_terminals: Vec<&String> =
                    rows.iter().filter(|row| row.contains("erminal")).collect();
                assert!(
                    about_terminals.is_empty(),
                    "{what}: the card speaks of the agent alone: {about_terminals:#?}"
                );
                assert!(!app.process_raw_input_bytes(b"x"));
                assert!(app.pending_pty_takeover.is_none(), "{what}: typing claimed");

                match press {
                    Press::Space => {
                        app.process_raw_input_bytes(b" ");
                    }
                    Press::FocusBinding => {
                        let bytes = app.bindings.byte_patterns_for(Action::FocusAgent);
                        app.process_raw_input_bytes(&bytes[0]);
                    }
                    Press::Button => {
                        let button = app.mouse_layout.takeover_button.expect("the button");
                        for kind in [
                            MouseEventKind::Down(MouseButton::Left),
                            MouseEventKind::Up(MouseButton::Left),
                        ] {
                            mouse(&mut app, kind, button.x + 2, button.y + 1);
                        }
                    }
                }
                let _ = render_rows(&mut app, 160, 40);
                let _ = render_rows(&mut app, 160, 40);
                assert!(
                    app.agent_row_card().is_none(),
                    "{what}: the take-over was not carried: {}",
                    said(&app)
                );
                let terminal = app.dock_terminal_ids(SESSION)[0].clone();
                assert!(seat.owners.is_owner(&terminal, conn), "{what}");
                assert_eq!(app.fullscreen_overlay, overlay, "{what}");
                assert_eq!(app.input_target, target, "{what}");
            }
        }
    }

    /// Looking is never refused: the focus key or a double click on a covered
    /// agent's row shows its card in the center pane with focus on it,
    /// whatever the pane showed before, and says nothing about a refusal.
    #[test]
    fn activating_a_covered_row_in_the_list_shows_its_card_and_refuses_nothing() {
        for by_mouse in [false, true] {
            let (mut app, seat) = covered(Driver::Browser);
            app.center_mode = crate::app::CenterMode::Diff {
                lines: std::sync::Arc::new(Vec::new()),
                scroll: 0,
                gutter_width: 0,
                worktree_path: String::new(),
                rel_path: "x".to_string(),
            };
            app.focus = FocusPane::Left;
            let _ = render_rows(&mut app, 160, 40);
            let before = row_state(&app, &seat);
            quiet(&mut app);
            if by_mouse {
                let (x, y) = app
                    .mouse_layout
                    .left_row_to_item
                    .iter()
                    .position(|item| *item == app.selected_left)
                    .map(|row| {
                        let list = app.mouse_layout.left_list;
                        (list.x + 2, list.y + u16::try_from(row).unwrap())
                    })
                    .expect("the agent's row is drawn");
                for _ in 0..2 {
                    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
                    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, y);
                }
            } else {
                let key = enter();
                assert_eq!(
                    app.bindings.lookup(&key, BindingScope::Left),
                    Some(Action::FocusAgent)
                );
                app.handle_key(key).unwrap();
            }
            let rows = render_rows(&mut app, 160, 40);
            assert!(
                matches!(app.center_mode, crate::app::CenterMode::Agent),
                "by mouse: {by_mouse}"
            );
            assert_eq!(app.focus, FocusPane::Center, "by mouse: {by_mouse}");
            assert!(rows.iter().any(|row| row.contains("Take over")));
            assert!(
                !said(&app).contains("nothing changed"),
                "by mouse {by_mouse}: looking was refused: {}",
                said(&app)
            );
            assert_eq!(row_state(&app, &seat), before);
        }
    }

    /// Serving stopping while a card is up never lands the user typing into a
    /// maximized terminal they were not looking at: the view the card hid
    /// comes back windowed.
    #[test]
    fn stopping_serving_under_a_card_lands_windowed_and_not_typing() {
        let (mut app, seat) = driving();
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        app.input_target = InputTarget::Terminal;
        let _ = render_rows(&mut app, 160, 40);
        browser_takes(&seat);
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_some());

        app.release_owned_ptys();
        app.companion = None;
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_none(), "nothing is serving");
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(app.input_target, InputTarget::None);
    }

    /// What asking who drives costs, counted as every acquisition of the
    /// registry's lock while this window handles one event: the `join_row`
    /// inside each question, the write and resize gates and any snapshot
    /// included.
    ///
    /// The bounds are what the same counting measured before the take-over
    /// card covered an agent's whole row (commit a688e61c), with this fixture
    /// and with three more terminals in the agent and four more driven rows in
    /// the workspace, which changed none of them:
    ///
    /// | one              | under the card | driving |
    /// |------------------|----------------|---------|
    /// | frame            | 12             | 8       |
    /// | key              | 2              | 6       |
    /// | mouse move       | 2              | 4       |
    /// | paste            | 3              | 2       |
    /// | palette command  | 0              | 0       |
    /// | center reconcile | 0              | 0       |
    /// | worker drain     | 0              | 0       |
    ///
    /// A command and a reconcile now ask once, because each has to know
    /// whether the card is over the row before it changes or redraws it.
    #[test]
    fn who_drives_costs_no_more_registry_locks_than_before_the_row_card() {
        const FRAME: (u64, u64) = (12, 8);
        const KEY: (u64, u64) = (2, 6);
        const MOUSE: (u64, u64) = (2, 4);
        const PASTE: (u64, u64) = (3, 2);
        const ONCE: (u64, u64) = (1, 1);
        const NEVER: (u64, u64) = (0, 0);
        type Act = fn(&mut App);
        let acts: [(&str, (u64, u64), Act); 7] = [
            ("frame", FRAME, |app| {
                let _ = render_rows(app, 160, 40);
            }),
            ("key", KEY, |app| {
                app.handle_key(KeyEvent::new(
                    crossterm::event::KeyCode::Char('x'),
                    crossterm::event::KeyModifiers::NONE,
                ))
                .unwrap();
            }),
            ("mouse move", MOUSE, |app| {
                mouse(app, MouseEventKind::Moved, 80, 20);
            }),
            ("palette command", ONCE, |app| {
                app.execute_command("agent-info").unwrap();
                app.prompt = PromptState::None;
            }),
            ("center reconcile", ONCE, App::reconcile_center_target),
            ("paste", PASTE, |app| app.handle_paste("x")),
            ("worker drain", NEVER, App::drain_events),
        ];
        let mut measured = Vec::new();
        for (extra_terminals, extra_rows) in [(0, 0), (3, 4)] {
            let (mut app, seat) = covered(Driver::Browser);
            for _ in 0..extra_terminals {
                app.engine
                    .create_companion_terminal(SESSION, 24, 80)
                    .unwrap();
            }
            for row in 0..extra_rows {
                let id = format!("solo-{row}");
                crate::app::test_support::add_terminal(
                    &mut app,
                    &id,
                    dux_core::model::TerminalOwner::Standalone,
                );
                seat.owners.claim(&id, seat.owners.next_conn_id());
            }
            app.rebuild_left_items();
            app.reselect_left_session(SESSION);
            let locks = |app: &mut App, act: Act| {
                let before = seat.owners.lock_acquisitions();
                act(app);
                seat.owners.lock_acquisitions() - before
            };
            let _ = render_rows(&mut app, 160, 40);
            let under_the_card: Vec<u64> = acts
                .iter()
                .map(|(_, _, act)| locks(&mut app, *act))
                .collect();
            app.focus = FocusPane::Center;
            app.take_over_focused_pty();
            let _ = render_rows(&mut app, 160, 40);
            let _ = render_rows(&mut app, 160, 40);
            assert!(app.agent_row_card().is_none());
            let driving: Vec<u64> = acts
                .iter()
                .map(|(_, _, act)| locks(&mut app, *act))
                .collect();
            measured.push((under_the_card, driving));
        }
        assert_eq!(
            measured[0], measured[1],
            "the cost grew with the agent's terminals or the workspace's rows"
        );
        let (under_the_card, driving) = &measured[0];
        for (index, (what, (covered_bound, driving_bound), _)) in acts.iter().enumerate() {
            assert!(
                under_the_card[index] <= *covered_bound && driving[index] <= *driving_bound,
                "{what}: {measured:?}"
            );
        }

        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.pty_ownership_conn_id().is_none(), "nothing is serving");
    }

    /// The default key that reaches `action` in `scope`.
    fn key_for(app: &App, action: Action, scope: BindingScope) -> KeyEvent {
        BINDING_DEFS
            .iter()
            .filter(|def| def.action == action)
            .flat_map(|def| def.default_keys)
            .map(|combination| KeyEvent::new(*combination.codes.first(), combination.modifiers))
            .find(|key| app.bindings.lookup(key, scope) == Some(action))
            .expect("a default key reaches the action")
    }

    /// Every way of looking at a covered agent works in each state the card
    /// can be up in, says nothing about a refusal, and starts, launches,
    /// reconnects or claims none of the row.
    #[test]
    fn looking_at_a_covered_agent_is_never_refused_and_starts_nothing() {
        type Look = fn(&mut App, &str);
        let looks: [(&str, Look); 4] = [
            ("palette show-agent from the list", |app, state| {
                app.focus = FocusPane::Left;
                app.execute_command("show-agent").unwrap();
                assert_eq!(app.focus, FocusPane::Center, "{state}");
            }),
            ("the list's interact key", |app, state| {
                app.focus = FocusPane::Left;
                let key = key_for(app, Action::InteractAgent, BindingScope::Left);
                app.handle_key(key).unwrap();
                assert_eq!(app.focus, FocusPane::Center, "{state}");
                assert_eq!(app.input_target, InputTarget::None, "{state}: typing");
                let live = app.engine.providers.contains_key(TabIdRef::new(SLOT));
                let maximized = app.fullscreen_overlay == FullscreenOverlay::Agent;
                assert_eq!(maximized, live, "{state}: maximized as on a driven row");
            }),
            (
                "the fullscreen key on the agent pane, twice",
                |app, state| {
                    app.focus = FocusPane::Center;
                    let key = key_for(app, Action::ToggleFullscreen, BindingScope::Center);
                    app.handle_key(key).unwrap();
                    assert_ne!(
                        app.fullscreen_overlay,
                        FullscreenOverlay::None,
                        "{state}: not maximized"
                    );
                    let rows = render_rows(app, 160, 40);
                    assert!(rows.iter().any(|row| row.contains("Take over")), "{state}");
                    app.handle_key(key).unwrap();
                    assert_eq!(
                        app.fullscreen_overlay,
                        FullscreenOverlay::None,
                        "{state}: still maximized"
                    );
                },
            ),
            ("a double click on the card", |app, state| {
                app.focus = FocusPane::Center;
                let cover = app.mouse_layout.card_cover.expect("the card is drawn");
                for _ in 0..2 {
                    for kind in [
                        MouseEventKind::Down(MouseButton::Left),
                        MouseEventKind::Up(MouseButton::Left),
                    ] {
                        mouse(app, kind, cover.x + 1, cover.y + 1);
                    }
                }
                // The card takes the pointer, so the double click is spent on it.
                assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None, "{state}");
            }),
        ];
        for (state, mut app, seat) in covered_states() {
            for (what, look) in looks {
                let what = format!("{state}: {what}");
                settle(&mut app);
                let _ = render_rows(&mut app, 160, 40);
                let before = row_state(&app, &seat);
                quiet(&mut app);
                look(&mut app, &what);
                let _ = render_rows(&mut app, 160, 40);
                assert!(
                    !said(&app).contains("so nothing changed"),
                    "{what}: refused: {}",
                    said(&app)
                );
                assert!(app.agent_row_card().is_some(), "{what}: the card came down");
                settle(&mut app);
                let _ = render_rows(&mut app, 160, 40);
                assert_eq!(row_state(&app, &seat), before, "{what}");
            }
        }
    }

    /// The serve retiring by itself while the card is up gives back none of a
    /// view that takes the keyboard: the user lands windowed and not typing.
    #[test]
    fn the_serve_retiring_under_the_card_lands_windowed_and_not_typing() {
        let (mut app, _seat) = covered(Driver::Browser);
        app.held_row_view = Some((
            SESSION.to_string(),
            RowView {
                input_target: InputTarget::Agent,
                fullscreen_overlay: FullscreenOverlay::Agent,
                session_surface: SessionSurface::Agent,
            },
        ));
        let _ = render_rows(&mut app, 160, 40);
        assert!(app.agent_row_card().is_some());
        app.companion = None;
        let _ = render_rows(&mut app, 160, 40);
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "typing after retirement"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "maximized after retirement"
        );
        assert!(app.held_row_view.is_none());
    }

    /// A change in the same burst of input as the take-over, before any frame
    /// has carried the claim, applies on the claim it lands itself.
    #[test]
    fn a_row_change_right_after_take_over_applies_once_the_claim_lands() {
        let (mut app, seat) = covered(Driver::Browser);
        app.focus = FocusPane::Center;
        app.take_over_focused_pty();
        quiet(&mut app);
        app.execute_command("rename-agent").unwrap();
        assert!(
            !said(&app).contains("so nothing changed"),
            "refused right after take-over: {}",
            said(&app)
        );
        assert!(matches!(app.prompt, PromptState::RenameSession { .. }));
        assert!(
            seat.owners
                .is_owner(SLOT, app.pty_ownership_conn_id().unwrap())
        );
        assert!(app.pending_pty_takeover.is_none(), "the arm was spent");
    }

    /// A divider drag the card cuts off mid-move leaves the split share as it
    /// was when the drag began, and config.toml as it was.
    #[test]
    fn a_divider_drag_cut_off_by_the_card_puts_the_split_share_back() {
        let left = MouseButton::Left;
        let (mut app, seat) = driving();
        let divider = app.mouse_layout.dock_divider.expect("the split's divider");
        let began = app.terminal_dock_pct;
        let saved = app.engine.config.ui.terminal_dock_pct;
        mouse(
            &mut app,
            MouseEventKind::Down(left),
            divider.x + 5,
            divider.y,
        );
        mouse(
            &mut app,
            MouseEventKind::Drag(left),
            divider.x + 5,
            divider.y - 3,
        );
        assert_ne!(app.terminal_dock_pct, began, "the drag moved the divider");
        browser_takes(&seat);
        mouse(
            &mut app,
            MouseEventKind::Drag(left),
            divider.x + 5,
            divider.y - 4,
        );
        assert!(app.mouse_drag.is_none(), "the drag was abandoned");
        assert_eq!(app.terminal_dock_pct, began);
        assert_eq!(app.engine.config.ui.terminal_dock_pct, saved);
    }

    /// Leaving the covered row and coming back resets what a take-over gives
    /// back to what the same trip gives with this window driving.
    #[test]
    fn leaving_a_covered_row_resets_what_take_over_gives_back() {
        let trip = |app: &mut App| {
            app.fullscreen_overlay = FullscreenOverlay::None;
            app.input_target = InputTarget::None;
            app.focus = FocusPane::Left;
            app.reselect_left_terminal("solo");
            app.reconcile_center_target();
            let _ = render_rows(app, 160, 40);
            app.reselect_left_session(SESSION);
            app.reconcile_center_target();
            let _ = render_rows(app, 160, 40);
        };
        let view = |app: &App| (app.input_target, app.fullscreen_overlay, app.focused_part());
        let mut views = Vec::new();
        for card_up in [false, true] {
            let (mut app, seat) = driving();
            crate::app::test_support::add_terminal(
                &mut app,
                "solo",
                dux_core::model::TerminalOwner::Standalone,
            );
            app.rebuild_left_items();
            app.reselect_left_session(SESSION);
            app.fullscreen_overlay = FullscreenOverlay::Terminal;
            app.input_target = InputTarget::Terminal;
            let _ = render_rows(&mut app, 160, 40);
            if card_up {
                browser_takes(&seat);
                let _ = render_rows(&mut app, 160, 40);
                assert!(app.agent_row_card().is_some());
            }
            trip(&mut app);
            if card_up {
                app.focus = FocusPane::Center;
                app.take_over_focused_pty();
                let _ = render_rows(&mut app, 160, 40);
                let _ = render_rows(&mut app, 160, 40);
                assert!(app.agent_row_card().is_none());
            }
            views.push(view(&app));
        }
        assert_eq!(views[1], views[0]);
    }

    /// The footer under the card offers the keys that still work and none
    /// that would be refused or would not do what it says: the list's focus
    /// key stays, and the pane's focus key is not offered as typing.
    #[test]
    fn the_footer_under_the_card_offers_what_still_works() {
        let (mut app, _seat) = covered(Driver::Browser);
        app.focus = FocusPane::Left;
        let list = app.footer_hints_for(crate::keybindings::HintContext::LeftSession);
        let words: Vec<&str> = list.iter().map(|(_, word)| *word).collect();
        assert!(words.contains(&"Focus"), "{words:?}");
        assert!(
            !words.contains(&"Rename") && !words.contains(&"Delete"),
            "{words:?}"
        );
        let pane = app.footer_hints_for(crate::keybindings::HintContext::Center);
        let words: Vec<&str> = pane.iter().map(|(_, word)| *word).collect();
        assert!(!words.contains(&"Type"), "{words:?}");
        assert!(words.contains(&"Fullscreen"), "{words:?}");
    }

    /// The take-over line names what is taken over: the agent over an agent's
    /// row, whichever part the card covers.
    #[test]
    fn taking_over_an_agents_row_says_agent() {
        for driver in [Driver::Browser, Driver::Nobody] {
            let (mut app, _seat) = covered(driver);
            quiet(&mut app);
            app.take_over_focused_pty();
            assert!(said(&app).contains("this agent"), "{}", said(&app));
            assert!(!said(&app).contains("erminal"), "{}", said(&app));
        }
    }
}
