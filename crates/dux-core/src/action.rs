//! The `Action` enum: dux's transport-agnostic command vocabulary.
//!
//! Every bindable/invokable action is an `Action`. `config_name` returns the
//! stable snake_case identifier used as the `[keys]` config key; it is also the
//! intended command id for future surfaces (web, server mode) per the design,
//! but no surface dispatch exists yet. The TUI's key-parsing, default key
//! tables, and runtime binding lookup live in the binary's `keybindings`
//! module, not here.

/// Unique identifier for every bindable action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // Navigation
    MoveDown,
    MoveUp,
    // Projects pane
    ToggleProject,
    NewAgent,
    NewAgentFromPr,
    NewAgentFromWorktree,
    ManageProjects,
    ManageWorktrees,
    FilterAgents,
    ForkAgent,
    ChangeAgentProvider,
    ChangeDefaultProvider,
    ChangeProjectDefaultProvider,
    FocusAgent,
    OpenProjectBrowser,
    CopyPath,
    OpenWorktreeInEditor,
    ChooseWorktreeEditor,
    RefreshProject,
    CheckoutProjectDefaultBranch,
    ReconnectAgent,
    DeleteSession,
    DeleteTerminal,
    // Agent pane
    InteractAgent,
    ShowTerminal,
    // Agent tabs (Center-scope; non-interactive only)
    NextTab,
    PrevTab,
    NewTab,
    CloseTab,
    SelectTab1,
    SelectTab2,
    SelectTab3,
    SelectTab4,
    SelectTab5,
    SelectTab6,
    SelectTab7,
    SelectTab8,
    SelectTab9,
    OpenMacroBar,
    OpenCurrentPullRequest,
    AttachPullRequest,
    DetachPullRequest,
    ResumePullRequestAutodetection,
    ToggleFullscreen,
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollToBottom,
    ScrollToTop,
    // Files pane (git staging)
    OpenDiff,
    StageUnstage,
    CommitChanges,
    DiscardChanges,
    EngageCommitInput,
    PushToRemote,
    PullFromRemote,
    SearchFiles,
    SearchNext,
    // Commit message editor
    ExitCommitInput,
    // Global
    FocusNext,
    FocusPrev,
    OpenPalette,
    ToggleResizeMode,
    ToggleSidebar,
    ToggleGitPane,
    ToggleHelp,
    ForceRedraw,
    Quit,
    CloseOverlay,
    // Resize mode
    ResizeGrow,
    ResizeShrink,
    // Overlays and dialogs
    SearchToggle,
    GoToPath,
    ExitPathEditorOnProjectAdd,
    OpenEntry,
    AddCurrentDir,
    OpenStartupCommandLogFile,
    OpenStartupCommandLogFolder,
    Confirm,
    ToggleSelection,
    ToggleMarked,
    // Palette-only (no direct keybinding)
    KillRunning,
    /// Web-only: open the Monaco config.toml editor, reached from the web app
    /// menu's Configuration submenu. Has no TUI behavior (the TUI edits config
    /// via `dux config` and the configure-* commands) and no palette registry
    /// row, so the TUI never lists or dispatches it. The variant and its
    /// `BINDING_DEFS` entry stay because `config.rs::validate_keys` accepts a
    /// user `[keys]` action name only if it is present in that table.
    EditConfig,
    /// Web-only: open the Preferences dialog (browser tab title + favicon color
    /// plus the grouped ui/capabilities/defaults preferences), reached from the
    /// web app menu's cog. Has no TUI behavior (the TUI sets
    /// `config.server.title`/`favicon` via `dux config`) and no palette registry
    /// row. Kept for the same `validate_keys` reason as `EditConfig` above.
    RenameWebInstance,
    NewTerminal,
    /// Palette-only: open the project chooser to pick a project, then spawn a
    /// project-owned terminal at that project's repo root. Distinct from
    /// `NewTerminal`, which is agent-scoped. No default keybinding.
    NewProjectTerminal,
    /// Palette-only: spawn a terminal that belongs to nothing, in the user's
    /// home directory. Distinct from `NewTerminal` (agent-scoped) and
    /// `NewProjectTerminal` (project-scoped): this one needs nothing selected
    /// and no owner to exist. No default keybinding.
    NewStandaloneTerminal,
    RenameSession,
    DeleteProject,
    RemoveProject,
    SortAgents,
    RemoveGitPane,
    EditMacros,
    /// Create a new macro from the macro list.
    NewMacro,
    /// Delete the highlighted macro from the macro list.
    DeleteMacro,
    /// Empty the focused full-text field (a modal body, or the commit message).
    ClearTextField,
    DebugInput,
    ToggleDiffLineNumbers,
    ResourceMonitor,
    ToggleGithubIntegration,
    ToggleCopyOnSelect,
    ToggleProjectAutoReopenAgents,
    ToggleAgentAutoReopen,
    ConfigureStartupCommand,
    ConfigureGlobalEnv,
    ConfigureProjectEnv,
    RerunStartupCommandOnAgent,
    ReadStartupCommandLogs,
    ToggleRandomizedPetNameDefault,
    TogglePrBannerPosition,
    ForceReconnectAgent,
    /// Palette-only: recompute the selected agent's changed files right now.
    /// dux polls for changes a user makes outside it (from a terminal, say),
    /// so this is the way to stop waiting for the next poll. No default
    /// keybinding.
    RefreshChanges,
    ChangeTheme,
    ReloadConfig,
    StartWebServer,
    ToggleAlwaysShowTabs,
    /// Open the read-only Agent Info modal for the focused agent (name, provider,
    /// branch lineage, worktree, status).
    OpenAgentInfo,
    /// Open the first-run welcome screen on demand. Palette-only (no default
    /// keybinding). Works even when `[ui] disable_automated_welcome_screen` is
    /// set: that flag suppresses only the AUTOMATIC showing.
    ShowWelcomeScreen,
    /// Open the what's-new screen for the running version on demand, fetching the
    /// release notes if they are not cached. Palette-only (no default
    /// keybinding). Works even when `[ui] disable_release_notes` is set, and
    /// fetches regardless: that flag governs the automatic screen, not this one's
    /// contents.
    ShowReleaseNotes,
    // Manual reordering (the TUI equivalent of the web's drag-to-reorder). Each
    // moves the selected agent/terminal in the sidebar and switches the sort to
    // manual. Palette-only (no default keybinding).
    MoveAgentUp,
    MoveAgentDown,
    MoveAgentTop,
    MoveAgentBottom,
    MoveTerminalUp,
    MoveTerminalDown,
    MoveTerminalTop,
    MoveTerminalBottom,
}

impl Action {
    /// The snake_case config key for this action.
    pub fn config_name(self) -> &'static str {
        match self {
            Action::MoveDown => "move_down",
            Action::MoveUp => "move_up",
            Action::ToggleProject => "toggle_project",
            Action::NewAgent => "new_agent",
            Action::NewAgentFromPr => "new_agent_from_pr",
            Action::NewAgentFromWorktree => "new_agent_from_worktree",
            Action::ManageProjects => "manage_projects",
            Action::ManageWorktrees => "manage_worktrees",
            Action::FilterAgents => "filter_agents",
            Action::ForkAgent => "fork_agent",
            Action::ChangeAgentProvider => "change_agent_provider",
            Action::ChangeDefaultProvider => "change_default_provider",
            Action::ChangeProjectDefaultProvider => "change_project_default_provider",
            Action::FocusAgent => "focus_agent",
            Action::OpenProjectBrowser => "open_project_browser",
            Action::CopyPath => "copy_path",
            Action::OpenWorktreeInEditor => "open_worktree_in_editor",
            Action::ChooseWorktreeEditor => "choose_worktree_editor",
            Action::RefreshProject => "refresh_project",
            Action::CheckoutProjectDefaultBranch => "checkout_project_default_branch",
            Action::ReconnectAgent => "reconnect_agent",
            Action::DeleteSession => "delete_session",
            Action::DeleteTerminal => "delete_terminal",
            Action::InteractAgent => "interact_agent",
            Action::ShowTerminal => "show_terminal",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::NewTab => "new_tab",
            Action::CloseTab => "close_tab",
            Action::SelectTab1 => "select_tab_1",
            Action::SelectTab2 => "select_tab_2",
            Action::SelectTab3 => "select_tab_3",
            Action::SelectTab4 => "select_tab_4",
            Action::SelectTab5 => "select_tab_5",
            Action::SelectTab6 => "select_tab_6",
            Action::SelectTab7 => "select_tab_7",
            Action::SelectTab8 => "select_tab_8",
            Action::SelectTab9 => "select_tab_9",
            Action::OpenMacroBar => "open_macro_bar",
            Action::OpenCurrentPullRequest => "open_current_pull_request",
            Action::AttachPullRequest => "attach_pull_request",
            Action::DetachPullRequest => "detach_pull_request",
            Action::ResumePullRequestAutodetection => "resume_pull_request_autodetection",
            Action::ToggleFullscreen => "toggle_fullscreen",
            Action::ScrollPageUp => "scroll_page_up",
            Action::ScrollPageDown => "scroll_page_down",
            Action::ScrollLineUp => "scroll_line_up",
            Action::ScrollLineDown => "scroll_line_down",
            Action::ScrollToBottom => "scroll_to_bottom",
            Action::ScrollToTop => "scroll_to_top",
            Action::OpenDiff => "open_diff",
            Action::StageUnstage => "stage_unstage",
            Action::CommitChanges => "commit_changes",
            Action::DiscardChanges => "discard_changes",
            Action::EngageCommitInput => "engage_commit_input",
            Action::PushToRemote => "push_to_remote",
            Action::PullFromRemote => "pull_from_remote",
            Action::SearchFiles => "search_files",
            Action::SearchNext => "search_next",
            Action::ExitCommitInput => "exit_commit_input",
            Action::FocusNext => "focus_next",
            Action::FocusPrev => "focus_prev",
            Action::OpenPalette => "open_palette",
            Action::ToggleResizeMode => "toggle_resize_mode",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::ToggleGitPane => "toggle_git_pane",
            Action::ToggleHelp => "toggle_help",
            Action::Quit => "quit",
            Action::CloseOverlay => "close_overlay",
            Action::ResizeGrow => "resize_grow",
            Action::ResizeShrink => "resize_shrink",
            Action::SearchToggle => "search_toggle",
            Action::GoToPath => "go_to_path",
            Action::ExitPathEditorOnProjectAdd => "exit_path_editor_on_project_add",
            Action::OpenEntry => "open_entry",
            Action::AddCurrentDir => "add_current_dir",
            Action::OpenStartupCommandLogFile => "open_startup_command_log_file",
            Action::OpenStartupCommandLogFolder => "open_startup_command_log_folder",
            Action::Confirm => "confirm",
            Action::ToggleSelection => "toggle_selection",
            Action::ToggleMarked => "toggle_marked",
            Action::KillRunning => "kill_running",
            Action::EditConfig => "edit_config",
            Action::RenameWebInstance => "rename_web_instance",
            Action::NewTerminal => "new_terminal",
            Action::NewProjectTerminal => "new_project_terminal",
            Action::NewStandaloneTerminal => "new_standalone_terminal",
            Action::RenameSession => "rename_session",
            Action::DeleteProject => "delete_project",
            Action::RemoveProject => "remove_project",
            Action::SortAgents => "sort_agents",
            Action::ForceRedraw => "force_redraw",
            Action::RemoveGitPane => "remove_git_pane",
            Action::EditMacros => "edit_macros",
            Action::NewMacro => "new_macro",
            Action::DeleteMacro => "delete_macro",
            Action::ClearTextField => "clear_text_field",
            Action::DebugInput => "debug_input",
            Action::ToggleDiffLineNumbers => "toggle_diff_line_numbers",
            Action::ResourceMonitor => "resource_monitor",
            Action::ToggleGithubIntegration => "toggle_github_integration",
            Action::ToggleCopyOnSelect => "toggle_copy_on_select",
            Action::ToggleProjectAutoReopenAgents => "toggle_project_auto_reopen_agents",
            Action::ToggleAgentAutoReopen => "toggle_agent_auto_reopen",
            Action::ConfigureStartupCommand => "configure_startup_command",
            Action::ConfigureGlobalEnv => "configure_global_env",
            Action::ConfigureProjectEnv => "configure_project_env",
            Action::RerunStartupCommandOnAgent => "rerun_startup_command_on_agent",
            Action::ReadStartupCommandLogs => "read_startup_command_logs",
            Action::ToggleRandomizedPetNameDefault => "toggle_randomized_pet_name_default",
            Action::TogglePrBannerPosition => "toggle_pr_banner_position",
            Action::ForceReconnectAgent => "force_reconnect_agent",
            Action::RefreshChanges => "refresh_changes",
            Action::ChangeTheme => "change_theme",
            Action::ReloadConfig => "reload_config",
            Action::StartWebServer => "start_web_server",
            Action::ToggleAlwaysShowTabs => "toggle_always_show_tabs",
            Action::OpenAgentInfo => "open_agent_info",
            Action::ShowWelcomeScreen => "show_welcome_screen",
            Action::ShowReleaseNotes => "show_release_notes",
            Action::MoveAgentUp => "move_agent_up",
            Action::MoveAgentDown => "move_agent_down",
            Action::MoveAgentTop => "move_agent_top",
            Action::MoveAgentBottom => "move_agent_bottom",
            Action::MoveTerminalUp => "move_terminal_up",
            Action::MoveTerminalDown => "move_terminal_down",
            Action::MoveTerminalTop => "move_terminal_top",
            Action::MoveTerminalBottom => "move_terminal_bottom",
        }
    }

    /// Human description used as a TOML comment in the config file.
    pub fn config_description(self) -> &'static str {
        match self {
            Action::MoveDown => "Navigate down through projects, sessions, files, and lists.",
            Action::MoveUp => "Navigate up through projects, sessions, files, and lists.",
            Action::ToggleProject => "Collapse or expand the selected project.",
            Action::NewAgent => "Create a new agent session (worktree).",
            Action::NewAgentFromPr => "Create a new agent session from a GitHub pull request.",
            Action::NewAgentFromWorktree => "Create a new agent from an existing git worktree.",
            Action::ManageProjects => {
                "Choose a project to target for project-scoped palette actions."
            }
            Action::ManageWorktrees => {
                "Remove a worktree dux manages for a project, and optionally its branch."
            }
            Action::FilterAgents => "Filter the agent list by name, branch, project, or provider.",
            Action::ForkAgent => "Fork the selected agent into a fresh worktree and session.",
            Action::ChangeAgentProvider => {
                "Swap the selected agent worktree to a different provider."
            }
            Action::ChangeDefaultProvider => {
                "Change the global default provider used for new agent sessions in projects without an explicit project override."
            }
            Action::ChangeProjectDefaultProvider => {
                "Change the selected project's default provider used for new agent sessions in that project only."
            }
            Action::FocusAgent => {
                "Focus the selected agent's pane. With the pane focused, this key types into a live agent and launches a dormant one."
            }
            Action::OpenProjectBrowser => "Open the project browser.",
            Action::CopyPath => "Copy the selected agent's worktree path.",
            Action::OpenWorktreeInEditor => {
                "Open the selected agent worktree in the configured editor."
            }
            Action::ChooseWorktreeEditor => {
                "Open a picker and choose which editor should open the selected agent worktree."
            }
            Action::RefreshProject => "Git pull the selected project checkout.",
            Action::CheckoutProjectDefaultBranch => {
                "Check out the default branch for the selected project."
            }
            Action::ReconnectAgent => "Restart the CLI for the selected agent.",
            Action::DeleteSession => "Delete the selected session and worktree.",
            Action::DeleteTerminal => "Delete the selected companion terminal.",
            Action::InteractAgent => {
                "Open the selected agent fullscreen, where keys go to the agent verbatim."
            }
            Action::ShowTerminal => {
                "Open the first companion terminal for the selected agent, or launch a new one if none exists."
            }
            Action::NextTab => "Focus the next tab of the selected agent.",
            Action::PrevTab => "Focus the previous tab of the selected agent.",
            Action::NewTab => "Add a tab to the selected agent (starts fresh).",
            Action::CloseTab => "Close the focused tab of the selected agent.",
            Action::SelectTab1 => "Focus tab 1 of the selected agent.",
            Action::SelectTab2 => "Focus tab 2 of the selected agent.",
            Action::SelectTab3 => "Focus tab 3 of the selected agent.",
            Action::SelectTab4 => {
                "Focus tab 4 of the selected agent. Ships with no default key: most terminals send the same byte for Ctrl-4 and Ctrl-\\ (the macro bar), so bind your own key here for direct access."
            }
            Action::SelectTab5 => "Focus tab 5 of the selected agent.",
            Action::SelectTab6 => "Focus tab 6 of the selected agent.",
            Action::SelectTab7 => "Focus tab 7 of the selected agent.",
            Action::SelectTab8 => "Focus tab 8 of the selected agent.",
            Action::SelectTab9 => "Focus tab 9 of the selected agent.",
            Action::NewTerminal => "Spawn a new companion terminal for the selected agent.",
            Action::NewProjectTerminal => "Open a terminal for a project you pick.",
            Action::NewStandaloneTerminal => {
                "Open a standalone terminal in your home directory, belonging to no project or agent."
            }
            Action::OpenMacroBar => {
                "Open the macro command bar to send text macros. Works over the windowed agent pane and in fullscreen."
            }
            Action::OpenCurrentPullRequest => {
                "Open the selected agent's current pull request in the default browser."
            }
            Action::AttachPullRequest => {
                "Attach a GitHub pull request to the selected agent, pausing autodetection."
            }
            Action::DetachPullRequest => {
                "Detach the selected agent's pull request and stop looking for one on it, until you attach a pull request by hand or resume autodetection."
            }
            Action::ResumePullRequestAutodetection => {
                "Start looking for a pull request on the selected agent's branch again, after a detach."
            }
            Action::ToggleFullscreen => {
                "Toggle the agent pane between windowed and fullscreen. Windowed, typing reaches the agent while dux chords stay active; fullscreen, keys go to the agent verbatim."
            }
            Action::ScrollPageUp => {
                "Scroll up one page in the agent output. Forwarded to the app itself when it owns the screen (see each provider's forward_scroll)."
            }
            Action::ScrollPageDown => {
                "Scroll down one page in the agent output. Forwarded to the app itself when it owns the screen (see each provider's forward_scroll)."
            }
            Action::ScrollLineUp => {
                "Scroll up one line in any scrollable view. In a typing agent pane this key reaches the agent until the view is scrolled back."
            }
            Action::ScrollLineDown => {
                "Scroll down one line in any scrollable view. In a typing agent pane this key reaches the agent until the view is scrolled back."
            }
            Action::ScrollToBottom => "Exit scroll mode and jump to the latest output.",
            Action::ScrollToTop => "Jump to the top of the scrollback buffer.",
            Action::OpenDiff => "Open the selected file's diff.",
            Action::StageUnstage => "Stage or unstage the selected file.",
            Action::CommitChanges => "Commit staged changes.",
            Action::DiscardChanges => "Discard changes to the selected file.",
            Action::EngageCommitInput => "Open the commit message editor.",
            Action::PushToRemote => "Push to remote.",
            Action::PullFromRemote => "Pull from remote.",
            Action::SearchFiles => "Start searching changed files in the files pane.",
            Action::SearchNext => "Jump to the next active search match in the files pane.",
            Action::ExitCommitInput => "Exit the commit message editor.",
            Action::FocusNext => "Focus the next pane.",
            Action::FocusPrev => "Focus the previous pane.",
            Action::OpenPalette => "Open the command palette.",
            Action::ToggleResizeMode => "Enter resize mode to resize side panes.",
            Action::ToggleSidebar => "Toggle the projects sidebar.",
            Action::ToggleGitPane => "Toggle the git pane.",
            Action::ToggleHelp => "Toggle the help overlay.",
            Action::Quit => "Quit the application.",
            Action::CloseOverlay => "Close the current overlay or dialog.",
            Action::ResizeGrow => "Grow the left pane width.",
            Action::ResizeShrink => "Shrink the left pane width.",
            Action::SearchToggle => "Toggle search mode in search-capable lists and overlays.",
            Action::GoToPath => "Open path editor in the project browser.",
            Action::ExitPathEditorOnProjectAdd => "Exit typed-path mode in the project browser.",
            Action::OpenEntry => "Open or navigate into the selected entry in the project browser.",
            Action::AddCurrentDir => "Add the current directory as a project.",
            Action::OpenStartupCommandLogFile => "Open the selected startup command log file.",
            Action::OpenStartupCommandLogFolder => "Open the selected startup command log folder.",
            Action::Confirm => "Confirm the selected action in a dialog.",
            Action::ToggleSelection => "Toggle between options in a confirmation dialog.",
            Action::ToggleMarked => "Toggle the hovered runtime in the kill-running modal.",
            Action::KillRunning => {
                "Open the kill-running modal for agents and companion terminals."
            }
            Action::EditConfig => "Edit config.toml in an editor.",
            Action::RenameWebInstance => {
                "Rename this dux instance (browser tab title + favicon color)."
            }
            Action::RenameSession => "Rename the selected agent session.",
            Action::DeleteProject => "Remove the selected project and its sessions.",
            Action::RemoveProject => "Remove project from app (keeps files on disk).",
            Action::SortAgents => {
                "Cycle the agent-list sort mode (active, updated, created, name)."
            }
            Action::ForceRedraw => "Force a full terminal redraw.",
            Action::RemoveGitPane => "Remove or restore the git pane.",
            Action::EditMacros => "Open the text macros editor.",
            Action::NewMacro => "Create a new macro from the macro list.",
            Action::DeleteMacro => "Delete the highlighted macro from the macro list.",
            Action::ClearTextField => {
                "Empty the focused full-text field: a modal body, or the commit message."
            }
            Action::DebugInput => "Open input event debugger to inspect keyboard and mouse events.",
            Action::ToggleDiffLineNumbers => "Toggle line numbers in diff view.",
            Action::ResourceMonitor => "Show CPU and memory usage for dux and all running agents.",
            Action::ToggleGithubIntegration => "Toggle GitHub PR integration.",
            Action::ToggleCopyOnSelect => "Toggle auto-copying selected text in the web terminal.",
            Action::ToggleProjectAutoReopenAgents => {
                "Toggle startup auto-reopen for agents in the selected project."
            }
            Action::ToggleAgentAutoReopen => "Toggle startup auto-reopen for the selected agent.",
            Action::ConfigureStartupCommand => {
                "Configure the selected project's startup command for newly created agents."
            }
            Action::ConfigureGlobalEnv => {
                "Configure environment variables for every project's agents and terminals."
            }
            Action::ConfigureProjectEnv => {
                "Configure the selected project's environment variables for agents and terminals."
            }
            Action::RerunStartupCommandOnAgent => {
                "Rerun the selected agent's project startup command."
            }
            Action::ReadStartupCommandLogs => {
                "Open startup command logs for the selected agent or project."
            }
            Action::ToggleRandomizedPetNameDefault => {
                "Toggle whether the agent name prompt starts with a random pet name."
            }
            Action::TogglePrBannerPosition => {
                "Move PR banner between top and bottom of agent pane."
            }
            Action::ForceReconnectAgent => "Restart the agent without resuming the prior session.",
            Action::RefreshChanges => {
                "Recompute the selected agent's changed files without waiting for the next poll."
            }
            Action::ChangeTheme => "Open a picker to switch the dux color theme.",
            Action::ReloadConfig => "Reload the configuration file.",
            Action::StartWebServer => {
                "Tear down the TUI and serve the dux web UI over the same agents."
            }
            Action::ToggleAlwaysShowTabs => {
                "Toggle always showing the agent tab strip, even with a single tab."
            }
            Action::OpenAgentInfo => {
                "Show the selected agent's details: provider, branch lineage, worktree, and status."
            }
            Action::ShowWelcomeScreen => "Open the dux welcome screen.",
            Action::ShowReleaseNotes => "Open the release notes for the running dux version.",
            Action::MoveAgentUp => {
                "Move the selected agent up one position (switches sorting to manual)."
            }
            Action::MoveAgentDown => {
                "Move the selected agent down one position (switches sorting to manual)."
            }
            Action::MoveAgentTop => {
                "Move the selected agent to the top (switches sorting to manual)."
            }
            Action::MoveAgentBottom => {
                "Move the selected agent to the bottom (switches sorting to manual)."
            }
            Action::MoveTerminalUp => {
                "Move the selected terminal up one position (switches sorting to manual)."
            }
            Action::MoveTerminalDown => {
                "Move the selected terminal down one position (switches sorting to manual)."
            }
            Action::MoveTerminalTop => {
                "Move the selected terminal to the top (switches sorting to manual)."
            }
            Action::MoveTerminalBottom => {
                "Move the selected terminal to the bottom (switches sorting to manual)."
            }
        }
    }

    /// Help section name for the help overlay.
    pub fn help_section(self) -> Option<&'static str> {
        match self {
            Action::MoveDown
            | Action::MoveUp
            | Action::ToggleProject
            | Action::NewAgent
            | Action::NewAgentFromWorktree
            | Action::FilterAgents
            | Action::ForkAgent
            | Action::ChangeAgentProvider
            | Action::FocusAgent
            | Action::OpenProjectBrowser
            | Action::CopyPath
            | Action::OpenWorktreeInEditor
            | Action::ChooseWorktreeEditor
            | Action::RefreshProject
            | Action::CheckoutProjectDefaultBranch
            | Action::InteractAgent
            | Action::ReconnectAgent
            | Action::DeleteSession
            | Action::DeleteTerminal => Some("Projects pane"),
            Action::NewAgentFromPr | Action::ManageProjects | Action::ManageWorktrees => None,
            Action::OpenMacroBar
            | Action::OpenCurrentPullRequest
            | Action::ToggleFullscreen
            | Action::ScrollPageUp
            | Action::ScrollPageDown
            | Action::ShowTerminal
            | Action::NextTab
            | Action::PrevTab
            | Action::NewTab
            | Action::CloseTab
            | Action::SelectTab1
            | Action::SelectTab2
            | Action::SelectTab3
            | Action::SelectTab4
            | Action::SelectTab5
            | Action::SelectTab6
            | Action::SelectTab7
            | Action::SelectTab8
            | Action::SelectTab9 => Some("Agent pane"),
            Action::ScrollLineUp
            | Action::ScrollLineDown
            | Action::ScrollToBottom
            | Action::ScrollToTop => Some("Scrolling"),
            Action::OpenDiff
            | Action::StageUnstage
            | Action::CommitChanges
            | Action::DiscardChanges
            | Action::EngageCommitInput
            | Action::PushToRemote
            | Action::PullFromRemote
            | Action::SearchFiles
            | Action::SearchNext => Some("Files pane"),
            Action::ExitCommitInput => Some("Commit input"),
            Action::FocusNext
            | Action::FocusPrev
            | Action::OpenPalette
            | Action::ToggleResizeMode
            | Action::ToggleSidebar
            | Action::ToggleGitPane
            | Action::RemoveGitPane
            | Action::ToggleHelp
            | Action::ForceRedraw
            | Action::Quit
            | Action::CloseOverlay => Some("Global"),
            Action::ResizeGrow | Action::ResizeShrink => Some("Resize mode"),
            Action::SearchToggle
            | Action::GoToPath
            | Action::ExitPathEditorOnProjectAdd
            | Action::OpenEntry
            | Action::AddCurrentDir
            | Action::OpenStartupCommandLogFile
            | Action::OpenStartupCommandLogFolder
            | Action::Confirm
            | Action::ToggleSelection
            | Action::ToggleMarked
            | Action::NewMacro
            | Action::DeleteMacro
            | Action::ClearTextField => Some("Overlays"),
            Action::KillRunning
            | Action::EditConfig
            | Action::RenameWebInstance
            | Action::NewTerminal
            | Action::NewProjectTerminal
            | Action::NewStandaloneTerminal
            | Action::RenameSession
            | Action::DeleteProject
            | Action::RemoveProject
            | Action::SortAgents
            | Action::EditMacros
            | Action::DebugInput
            | Action::ToggleDiffLineNumbers
            | Action::ResourceMonitor
            | Action::ToggleGithubIntegration
            | Action::ToggleCopyOnSelect
            | Action::ToggleProjectAutoReopenAgents
            | Action::ToggleAgentAutoReopen
            | Action::ConfigureStartupCommand
            | Action::ConfigureGlobalEnv
            | Action::ConfigureProjectEnv
            | Action::RerunStartupCommandOnAgent
            | Action::ReadStartupCommandLogs
            | Action::ToggleRandomizedPetNameDefault
            | Action::TogglePrBannerPosition
            | Action::ForceReconnectAgent
            | Action::AttachPullRequest
            | Action::DetachPullRequest
            | Action::ResumePullRequestAutodetection
            | Action::RefreshChanges
            | Action::ChangeDefaultProvider
            | Action::ChangeProjectDefaultProvider
            | Action::ChangeTheme
            | Action::ReloadConfig
            | Action::StartWebServer
            | Action::ToggleAlwaysShowTabs
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
            | Action::MoveTerminalBottom => None,
        }
    }
}
