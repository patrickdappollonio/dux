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
    ExitInteractive,
    OpenMacroBar,
    OpenCurrentPullRequest,
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
    GenerateCommitMessage,
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
    NewTerminal,
    RenameSession,
    DeleteProject,
    RemoveProject,
    SortAgentsByUpdated,
    SortAgentsByCreated,
    SortAgentsByName,
    RemoveGitPane,
    EditMacros,
    DebugInput,
    ToggleDiffLineNumbers,
    ResourceMonitor,
    ToggleGithubIntegration,
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
    ChangeTheme,
    ReloadConfig,
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
            Action::ExitInteractive => "exit_interactive",
            Action::OpenMacroBar => "open_macro_bar",
            Action::OpenCurrentPullRequest => "open_current_pull_request",
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
            Action::GenerateCommitMessage => "generate_commit_message",
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
            Action::NewTerminal => "new_terminal",
            Action::RenameSession => "rename_session",
            Action::DeleteProject => "delete_project",
            Action::RemoveProject => "remove_project",
            Action::SortAgentsByUpdated => "sort_agents_by_updated",
            Action::SortAgentsByCreated => "sort_agents_by_created",
            Action::SortAgentsByName => "sort_agents_by_name",
            Action::ForceRedraw => "force_redraw",
            Action::RemoveGitPane => "remove_git_pane",
            Action::EditMacros => "edit_macros",
            Action::DebugInput => "debug_input",
            Action::ToggleDiffLineNumbers => "toggle_diff_line_numbers",
            Action::ResourceMonitor => "resource_monitor",
            Action::ToggleGithubIntegration => "toggle_github_integration",
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
            Action::ChangeTheme => "change_theme",
            Action::ReloadConfig => "reload_config",
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
            Action::FocusAgent => "Focus the selected agent's output pane.",
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
            Action::InteractAgent => "Start a prompt turn for the agent.",
            Action::ShowTerminal => {
                "Open the first companion terminal for the selected agent, or launch a new one if none exists."
            }
            Action::NewTerminal => "Spawn a new companion terminal for the selected agent.",
            Action::ExitInteractive => "Exit interactive mode (stop forwarding keys to agent).",
            Action::OpenMacroBar => "Open the macro command bar to send text macros.",
            Action::OpenCurrentPullRequest => {
                "Open the selected agent's current pull request in the default browser."
            }
            Action::ToggleFullscreen => "Toggle fullscreen overlay for the agent terminal.",
            Action::ScrollPageUp => "Scroll up one page in the agent output.",
            Action::ScrollPageDown => "Scroll down one page in the agent output.",
            Action::ScrollLineUp => "Scroll up one line in any scrollable view.",
            Action::ScrollLineDown => "Scroll down one line in any scrollable view.",
            Action::ScrollToBottom => "Exit scroll mode and jump to the latest output.",
            Action::ScrollToTop => "Jump to the top of the scrollback buffer.",
            Action::OpenDiff => "Open the selected file's diff.",
            Action::StageUnstage => "Stage or unstage the selected file.",
            Action::CommitChanges => "Commit staged changes.",
            Action::GenerateCommitMessage => "Generate an AI commit message.",
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
            Action::RenameSession => "Rename the selected agent session.",
            Action::DeleteProject => "Remove the selected project and its sessions.",
            Action::RemoveProject => "Remove project from app (keeps files on disk).",
            Action::SortAgentsByUpdated => "Sort agents by most recently updated.",
            Action::SortAgentsByCreated => "Sort agents by creation date (newest first).",
            Action::SortAgentsByName => "Sort agents alphabetically by name.",
            Action::ForceRedraw => "Force a full terminal redraw.",
            Action::RemoveGitPane => "Remove or restore the git pane.",
            Action::EditMacros => "Open the text macros editor.",
            Action::DebugInput => "Open input event debugger to inspect keyboard and mouse events.",
            Action::ToggleDiffLineNumbers => "Toggle line numbers in diff view.",
            Action::ResourceMonitor => "Show CPU and memory usage for dux and all running agents.",
            Action::ToggleGithubIntegration => "Toggle GitHub PR integration.",
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
            Action::ChangeTheme => "Open a picker to switch the dux color theme.",
            Action::ReloadConfig => "Reload the configuration file.",
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
            Action::NewAgentFromPr => None,
            Action::ExitInteractive
            | Action::OpenMacroBar
            | Action::OpenCurrentPullRequest
            | Action::ToggleFullscreen
            | Action::ScrollPageUp
            | Action::ScrollPageDown
            | Action::ShowTerminal => Some("Agent pane"),
            Action::ScrollLineUp
            | Action::ScrollLineDown
            | Action::ScrollToBottom
            | Action::ScrollToTop => Some("Scrolling"),
            Action::OpenDiff
            | Action::StageUnstage
            | Action::CommitChanges
            | Action::GenerateCommitMessage
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
            | Action::ToggleMarked => Some("Overlays"),
            Action::KillRunning
            | Action::NewTerminal
            | Action::RenameSession
            | Action::DeleteProject
            | Action::RemoveProject
            | Action::SortAgentsByUpdated
            | Action::SortAgentsByCreated
            | Action::SortAgentsByName
            | Action::EditMacros
            | Action::DebugInput
            | Action::ToggleDiffLineNumbers
            | Action::ResourceMonitor
            | Action::ToggleGithubIntegration
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
            | Action::ChangeDefaultProvider
            | Action::ChangeProjectDefaultProvider
            | Action::ChangeTheme
            | Action::ReloadConfig => None,
        }
    }
}
