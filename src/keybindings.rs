use crokey::{KeyCombination, KeyCombinationFormat, key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Unique identifier for every bindable action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // Navigation
    MoveDown,
    MoveUp,
    // Projects pane
    ToggleProject,
    NewAgent,
    FocusAgent,
    OpenProjectBrowser,
    CopyPath,
    CycleProvider,
    RefreshProject,
    ReconnectAgent,
    DeleteSession,
    // Agent pane
    InteractAgent,
    ExitInteractive,
    ToggleFullscreen,
    ScrollPageUp,
    ScrollPageDown,
    // Files pane (git staging)
    OpenDiff,
    StageUnstage,
    CommitChanges,
    GenerateCommitMessage,
    DiscardChanges,
    EngageCommitInput,
    PushToRemote,
    PullFromRemote,
    // Commit message editor
    ExitCommitInput,
    // Global
    FocusNext,
    FocusPrev,
    OpenPalette,
    ToggleResizeMode,
    ToggleSidebar,
    ToggleHelp,
    Quit,
    CloseOverlay,
    // Resize mode
    ResizeGrow,
    ResizeShrink,
    // Overlays and dialogs
    SearchToggle,
    GoToPath,
    OpenEntry,
    AddCurrentDir,
    Confirm,
    ToggleSelection,
    // Palette-only (no direct keybinding)
    RenameSession,
    DeleteProject,
    RemoveProject,
}

/// Where a binding's key combo is matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingScope {
    Global,
    Left,
    Center,
    Files,
    Interactive,
    Resize,
    Palette,
    Browser,
    Dialog,
    CommitInput,
}

impl BindingScope {
    /// Human-readable scope name for error messages and diagnostics.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Left => "Projects pane",
            Self::Center => "Agent pane",
            Self::Files => "Files pane",
            Self::Interactive => "Interactive mode",
            Self::Resize => "Resize mode",
            Self::Palette => "Command palette",
            Self::Browser => "Project browser",
            Self::Dialog => "Dialog",
            Self::CommitInput => "Commit input",
        }
    }
}

/// Where a binding's hint appears in the status bar cheatsheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintContext {
    LeftProject,
    LeftSession,
    Center,
    Files,
    CommitInput,
}

pub struct HelpEntry {
    pub section: &'static str,
    pub description: &'static str,
}

pub struct PaletteEntry {
    pub name: &'static str,
    pub description: &'static str,
}

/// Static definition of a binding. Used as the template for generating default
/// config and for carrying metadata (scopes, help sections, palette entries,
/// hint contexts). Key combos and display labels are resolved at runtime from
/// the config file via [`RuntimeBindings`].
pub struct BindingDef {
    pub action: Action,
    pub default_keys: &'static [KeyCombination],
    pub scopes: &'static [BindingScope],
    pub help: Option<HelpEntry>,
    pub hint_contexts: &'static [(HintContext, &'static str)],
    pub palette: Option<PaletteEntry>,
}

impl Action {
    /// The snake_case config key for this action.
    pub fn config_name(self) -> &'static str {
        match self {
            Action::MoveDown => "move_down",
            Action::MoveUp => "move_up",
            Action::ToggleProject => "toggle_project",
            Action::NewAgent => "new_agent",
            Action::FocusAgent => "focus_agent",
            Action::OpenProjectBrowser => "open_project_browser",
            Action::CopyPath => "copy_path",
            Action::CycleProvider => "cycle_provider",
            Action::RefreshProject => "refresh_project",
            Action::ReconnectAgent => "reconnect_agent",
            Action::DeleteSession => "delete_session",
            Action::InteractAgent => "interact_agent",
            Action::ExitInteractive => "exit_interactive",
            Action::ToggleFullscreen => "toggle_fullscreen",
            Action::ScrollPageUp => "scroll_page_up",
            Action::ScrollPageDown => "scroll_page_down",
            Action::OpenDiff => "open_diff",
            Action::StageUnstage => "stage_unstage",
            Action::CommitChanges => "commit_changes",
            Action::GenerateCommitMessage => "generate_commit_message",
            Action::DiscardChanges => "discard_changes",
            Action::EngageCommitInput => "engage_commit_input",
            Action::PushToRemote => "push_to_remote",
            Action::PullFromRemote => "pull_from_remote",
            Action::ExitCommitInput => "exit_commit_input",
            Action::FocusNext => "focus_next",
            Action::FocusPrev => "focus_prev",
            Action::OpenPalette => "open_palette",
            Action::ToggleResizeMode => "toggle_resize_mode",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::ToggleHelp => "toggle_help",
            Action::Quit => "quit",
            Action::CloseOverlay => "close_overlay",
            Action::ResizeGrow => "resize_grow",
            Action::ResizeShrink => "resize_shrink",
            Action::SearchToggle => "search_toggle",
            Action::GoToPath => "go_to_path",
            Action::OpenEntry => "open_entry",
            Action::AddCurrentDir => "add_current_dir",
            Action::Confirm => "confirm",
            Action::ToggleSelection => "toggle_selection",
            Action::RenameSession => "rename_session",
            Action::DeleteProject => "delete_project",
            Action::RemoveProject => "remove_project",
        }
    }

    /// Human description used as a TOML comment in the config file.
    pub fn config_description(self) -> &'static str {
        match self {
            Action::MoveDown => "Navigate down through projects, sessions, files, and lists.",
            Action::MoveUp => "Navigate up through projects, sessions, files, and lists.",
            Action::ToggleProject => "Collapse or expand the selected project.",
            Action::NewAgent => "Create a new agent session (worktree).",
            Action::FocusAgent => "Focus the selected agent's output pane.",
            Action::OpenProjectBrowser => "Open the project browser.",
            Action::CopyPath => "Copy the selected agent's worktree path.",
            Action::CycleProvider => "Cycle the default provider for the selected project.",
            Action::RefreshProject => "Git pull the selected project checkout.",
            Action::ReconnectAgent => "Restart the CLI for the selected agent.",
            Action::DeleteSession => "Delete the selected session and worktree.",
            Action::InteractAgent => "Start a prompt turn for the agent.",
            Action::ExitInteractive => "Exit interactive mode (stop forwarding keys to agent).",
            Action::ToggleFullscreen => "Toggle fullscreen overlay for the agent terminal.",
            Action::ScrollPageUp => "Scroll up one page in the agent output.",
            Action::ScrollPageDown => "Scroll down one page in the agent output.",
            Action::OpenDiff => "Open the selected file's diff.",
            Action::StageUnstage => "Stage or unstage the selected file.",
            Action::CommitChanges => "Commit staged changes.",
            Action::GenerateCommitMessage => "Generate an AI commit message.",
            Action::DiscardChanges => "Discard changes to the selected file.",
            Action::EngageCommitInput => "Open the commit message editor.",
            Action::PushToRemote => "Push to remote.",
            Action::PullFromRemote => "Pull from remote.",
            Action::ExitCommitInput => "Exit the commit message editor.",
            Action::FocusNext => "Focus the next pane.",
            Action::FocusPrev => "Focus the previous pane.",
            Action::OpenPalette => "Open the command palette.",
            Action::ToggleResizeMode => "Enter resize mode to resize side panes.",
            Action::ToggleSidebar => "Toggle the projects sidebar.",
            Action::ToggleHelp => "Toggle the help overlay.",
            Action::Quit => "Quit the application.",
            Action::CloseOverlay => "Close the current overlay or dialog.",
            Action::ResizeGrow => "Grow the left pane width.",
            Action::ResizeShrink => "Shrink the left pane width.",
            Action::SearchToggle => "Toggle search mode in palette and project browser.",
            Action::GoToPath => "Open path editor in the project browser.",
            Action::OpenEntry => "Open or navigate into the selected entry in the project browser.",
            Action::AddCurrentDir => "Add the current directory as a project.",
            Action::Confirm => "Confirm the selected action in a dialog.",
            Action::ToggleSelection => "Toggle between options in a confirmation dialog.",
            Action::RenameSession => "Rename the selected agent session.",
            Action::DeleteProject => "Remove the selected project and its sessions.",
            Action::RemoveProject => "Remove project from app (keeps files on disk).",
        }
    }

    /// Help section name for the help overlay.
    pub fn help_section(self) -> Option<&'static str> {
        match self {
            Action::MoveDown
            | Action::MoveUp
            | Action::ToggleProject
            | Action::NewAgent
            | Action::FocusAgent
            | Action::OpenProjectBrowser
            | Action::CopyPath
            | Action::CycleProvider
            | Action::RefreshProject
            | Action::InteractAgent
            | Action::ReconnectAgent
            | Action::DeleteSession => Some("Projects pane"),
            Action::ExitInteractive
            | Action::ToggleFullscreen
            | Action::ScrollPageUp
            | Action::ScrollPageDown => Some("Agent pane"),
            Action::OpenDiff
            | Action::StageUnstage
            | Action::CommitChanges
            | Action::GenerateCommitMessage
            | Action::DiscardChanges
            | Action::EngageCommitInput
            | Action::PushToRemote
            | Action::PullFromRemote => Some("Files pane"),
            Action::ExitCommitInput => Some("Commit input"),
            Action::FocusNext
            | Action::FocusPrev
            | Action::OpenPalette
            | Action::ToggleResizeMode
            | Action::ToggleSidebar
            | Action::ToggleHelp
            | Action::Quit
            | Action::CloseOverlay => Some("Global"),
            Action::ResizeGrow | Action::ResizeShrink => Some("Resize mode"),
            Action::SearchToggle
            | Action::GoToPath
            | Action::OpenEntry
            | Action::AddCurrentDir
            | Action::Confirm
            | Action::ToggleSelection => Some("Overlays"),
            Action::RenameSession | Action::DeleteProject | Action::RemoveProject => None,
        }
    }
}

// ── Keybinding resolution semantics ──────────────────────────────────────
//
// Declaration order within each scope determines:
//   - hint display order in the status bar cheatsheet
//   - help entry order within each help section
//   - tiebreaker when the same key is bound to multiple actions in the
//     same scope (first match wins). This is intentional and by design.
//
// Pane bindings come first so their hints appear before global hints
// which are appended at the end of every context.
//
// Key matching rules (see `lookup()`):
//   - Plain bindings (no modifiers) only match events with no modifiers.
//     A binding for `d` will not fire on Ctrl+d.
//   - Modifier bindings require the incoming event to contain at least
//     those modifiers (subset match).
//
// Case handling:
//   - `crokey::parse()` lowercases its input, so a bare "P" in config
//     silently becomes lowercase "p". Use `normalize_key_string()` before
//     parsing to rewrite "P" → "shift-p" (crokey convention).
//   - `crokey::normalized()` canonicalizes case:
//       Char('P') + no mods  →  Char('P') + SHIFT
//       Char('p') + SHIFT    →  Char('P') + SHIFT
//     Both forms are equivalent after normalization, so `p` and `shift-p`
//     can coexist as distinct bindings in the same scope.
//
// Conflict detection (`detect_conflicts()`) runs at startup and rejects
// configs where the same normalized key is bound to two actions in
// overlapping scopes.
pub const BINDING_DEFS: &[BindingDef] = &[
    // ── Navigation (Left / Files / Palette / Browser) ─────────────
    BindingDef {
        action: Action::MoveDown,
        default_keys: &[key!(j), key!(Down)],
        scopes: &[
            BindingScope::Left,
            BindingScope::Files,
            BindingScope::Palette,
            BindingScope::Browser,
        ],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Move through projects and sessions",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Move"),
            (HintContext::LeftSession, "Move"),
            (HintContext::Files, "Move"),
        ],
        palette: None,
    },
    BindingDef {
        action: Action::MoveUp,
        default_keys: &[key!(k), key!(Up)],
        scopes: &[
            BindingScope::Left,
            BindingScope::Files,
            BindingScope::Palette,
            BindingScope::Browser,
        ],
        help: None, // covered by MoveDown's combined label
        hint_contexts: &[],
        palette: None,
    },
    // ── Projects pane ─────────────────────────────────────────────
    BindingDef {
        action: Action::ToggleProject,
        default_keys: &[key!(space)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Collapse/expand project",
        }),
        hint_contexts: &[(HintContext::LeftProject, "Toggle")],
        palette: Some(PaletteEntry {
            name: "toggle-project",
            description: "Collapse or expand the selected project's agents",
        }),
    },
    BindingDef {
        action: Action::NewAgent,
        default_keys: &[key!(n)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "New agent session (creates worktree)",
        }),
        hint_contexts: &[(HintContext::LeftProject, "New agent")],
        palette: Some(PaletteEntry {
            name: "new-agent",
            description: "Create a new agent for the selected project",
        }),
    },
    BindingDef {
        action: Action::FocusAgent,
        default_keys: &[key!(enter)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Focus the selected agent",
        }),
        hint_contexts: &[
            (HintContext::LeftSession, "Focus"),
            (HintContext::Center, "Interact"),
        ],
        palette: None,
    },
    BindingDef {
        action: Action::OpenProjectBrowser,
        default_keys: &[key!(a)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Open project browser",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Add project"),
            (HintContext::LeftSession, "Add project"),
        ],
        palette: Some(PaletteEntry {
            name: "add-project",
            description: "Open the project browser",
        }),
    },
    BindingDef {
        action: Action::CopyPath,
        default_keys: &[key!(y)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Copy agent worktree path",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Copy path"),
            (HintContext::LeftSession, "Copy path"),
        ],
        palette: Some(PaletteEntry {
            name: "copy-path",
            description: "Copy the selected agent's worktree path",
        }),
    },
    BindingDef {
        action: Action::CycleProvider,
        default_keys: &[key!(d)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Cycle default provider",
        }),
        hint_contexts: &[(HintContext::LeftProject, "Provider")],
        palette: Some(PaletteEntry {
            name: "provider",
            description: "Toggle the selected project's default provider",
        }),
    },
    BindingDef {
        action: Action::RefreshProject,
        default_keys: &[key!(u)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Refresh checkout (git pull --ff-only)",
        }),
        hint_contexts: &[(HintContext::LeftProject, "Pull")],
        palette: Some(PaletteEntry {
            name: "refresh-project",
            description: "Git pull the selected project checkout",
        }),
    },
    BindingDef {
        action: Action::ReconnectAgent,
        default_keys: &[key!(r)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Restart agent CLI",
        }),
        hint_contexts: &[(HintContext::Center, "Reconnect")],
        palette: Some(PaletteEntry {
            name: "reconnect-agent",
            description: "Restart the CLI for the selected agent",
        }),
    },
    BindingDef {
        action: Action::DeleteSession,
        default_keys: &[key!(ctrl - d)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Delete selected session/worktree",
        }),
        hint_contexts: &[(HintContext::LeftSession, "Delete")],
        palette: Some(PaletteEntry {
            name: "delete-agent",
            description: "Delete the selected agent session",
        }),
    },
    // ── Agent pane ────────────────────────────────────────────────
    BindingDef {
        action: Action::InteractAgent,
        default_keys: &[key!(i)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Enter interactive mode for the selected agent",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ExitInteractive,
        default_keys: &[key!(ctrl - g)],
        scopes: &[BindingScope::Interactive],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Exit interactive mode",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ToggleFullscreen,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ScrollPageUp,
        default_keys: &[key!(ctrl - b), key!(pageup)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Scroll up one page",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ScrollPageDown,
        default_keys: &[key!(ctrl - f), key!(pagedown)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Scroll down one page",
        }),
        hint_contexts: &[],
        palette: None,
    },
    // ── Files pane (git staging) ──────────────────────────────────
    BindingDef {
        action: Action::OpenDiff,
        default_keys: &[key!(enter)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Open selected file diff",
        }),
        hint_contexts: &[(HintContext::Files, "Diff")],
        palette: None,
    },
    BindingDef {
        action: Action::StageUnstage,
        default_keys: &[key!(space)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Stage or unstage selected file",
        }),
        hint_contexts: &[(HintContext::Files, "Stage/Unstage")],
        palette: None,
    },
    BindingDef {
        action: Action::CommitChanges,
        default_keys: &[key!(c)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Commit staged changes",
        }),
        hint_contexts: &[(HintContext::Files, "Commit")],
        palette: None,
    },
    BindingDef {
        action: Action::GenerateCommitMessage,
        default_keys: &[key!(ctrl - g)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Generate AI commit message",
        }),
        hint_contexts: &[(HintContext::Files, "AI msg")],
        palette: None,
    },
    BindingDef {
        action: Action::DiscardChanges,
        default_keys: &[key!(ctrl - d)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Discard changes to selected file",
        }),
        hint_contexts: &[(HintContext::Files, "Discard")],
        palette: None,
    },
    BindingDef {
        action: Action::EngageCommitInput,
        default_keys: &[key!(i)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Write a commit message",
        }),
        hint_contexts: &[(HintContext::Files, "Commit msg")],
        palette: None,
    },
    BindingDef {
        action: Action::PushToRemote,
        default_keys: &[key!(u)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Push to remote",
        }),
        hint_contexts: &[(HintContext::Files, "Push")],
        palette: None,
    },
    BindingDef {
        action: Action::PullFromRemote,
        default_keys: &[key!(p)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Pull from remote",
        }),
        hint_contexts: &[(HintContext::Files, "Pull")],
        palette: None,
    },
    // ── Commit message editor ─────────────────────────────────────
    BindingDef {
        action: Action::ExitCommitInput,
        default_keys: &[key!(ctrl - g), key!(esc)],
        scopes: &[BindingScope::CommitInput],
        help: Some(HelpEntry {
            section: "Commit input",
            description: "Exit commit input",
        }),
        hint_contexts: &[(HintContext::CommitInput, "Exit")],
        palette: None,
    },
    // ── Global ────────────────────────────────────────────────────
    // (placed after pane bindings so palette / help appear last in hints)
    BindingDef {
        action: Action::FocusNext,
        default_keys: &[key!(tab)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Focus next pane",
        }),
        hint_contexts: &[(HintContext::Center, "Next"), (HintContext::Files, "Next")],
        palette: None,
    },
    BindingDef {
        action: Action::FocusPrev,
        default_keys: &[key!(shift - tab)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Focus previous pane",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::OpenPalette,
        default_keys: &[key!(ctrl - p)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Open command palette",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Palette"),
            (HintContext::LeftSession, "Palette"),
            (HintContext::Center, "Palette"),
            (HintContext::Files, "Palette"),
        ],
        palette: None,
    },
    BindingDef {
        action: Action::ToggleResizeMode,
        default_keys: &[key!(ctrl - w)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Resize mode (h/l side panes)",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ToggleSidebar,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('['),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Toggle sidebar",
        }),
        hint_contexts: &[],
        palette: Some(PaletteEntry {
            name: "toggle-sidebar",
            description: "Collapse or expand the projects sidebar",
        }),
    },
    BindingDef {
        action: Action::ToggleHelp,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Toggle help",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Help"),
            (HintContext::LeftSession, "Help"),
            (HintContext::Center, "Help"),
            (HintContext::Files, "Help"),
        ],
        palette: Some(PaletteEntry {
            name: "help",
            description: "Open the help overlay",
        }),
    },
    BindingDef {
        action: Action::Quit,
        default_keys: &[key!(q), key!(ctrl - c)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Quit",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::CloseOverlay,
        default_keys: &[key!(esc)],
        scopes: &[
            BindingScope::Global,
            BindingScope::Palette,
            BindingScope::Browser,
            BindingScope::Dialog,
        ],
        help: Some(HelpEntry {
            section: "Global",
            description: "Close the current overlay or dialog",
        }),
        hint_contexts: &[],
        palette: None,
    },
    // ── Resize mode ───────────────────────────────────────────────
    BindingDef {
        action: Action::ResizeGrow,
        default_keys: &[key!(l), key!(Right)],
        scopes: &[BindingScope::Resize],
        help: Some(HelpEntry {
            section: "Resize mode",
            description: "Grow the left pane width",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ResizeShrink,
        default_keys: &[key!(h), key!(Left)],
        scopes: &[BindingScope::Resize],
        help: Some(HelpEntry {
            section: "Resize mode",
            description: "Shrink the left pane width",
        }),
        hint_contexts: &[],
        palette: None,
    },
    // ── Overlays and dialogs ──────────────────────────────────────
    BindingDef {
        action: Action::SearchToggle,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Palette, BindingScope::Browser],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Toggle search mode",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::GoToPath,
        default_keys: &[key!(g)],
        scopes: &[BindingScope::Browser],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Open path editor in the project browser",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::OpenEntry,
        default_keys: &[key!(enter), key!(Right), key!(l)],
        scopes: &[BindingScope::Browser],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Open or navigate into the selected entry",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::AddCurrentDir,
        default_keys: &[key!(o)],
        scopes: &[BindingScope::Browser],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Add the current directory as a project",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::Confirm,
        default_keys: &[key!(enter)],
        scopes: &[BindingScope::Dialog, BindingScope::Palette],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Confirm the selected action",
        }),
        hint_contexts: &[],
        palette: None,
    },
    BindingDef {
        action: Action::ToggleSelection,
        default_keys: &[key!(h), key!(l), key!(Left), key!(Right), key!(tab)],
        scopes: &[BindingScope::Dialog],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Toggle between options in a confirmation dialog",
        }),
        hint_contexts: &[],
        palette: None,
    },
    // ── Palette-only (no direct keybinding) ────────────────────────
    BindingDef {
        action: Action::RenameSession,
        default_keys: &[key!(e)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Rename the selected agent session",
        }),
        hint_contexts: &[(HintContext::LeftSession, "Rename")],
        palette: Some(PaletteEntry {
            name: "rename-agent",
            description: "Rename the selected agent session",
        }),
    },
    BindingDef {
        action: Action::DeleteProject,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
        palette: Some(PaletteEntry {
            name: "delete-project",
            description: "Remove the selected project and its sessions",
        }),
    },
    BindingDef {
        action: Action::RemoveProject,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
        palette: Some(PaletteEntry {
            name: "remove-project",
            description: "Remove project from app (keeps files on disk)",
        }),
    },
];

const HELP_SECTION_ORDER: &[&str] = &[
    "Global",
    "Projects pane",
    "Agent pane",
    "Files pane",
    "Commit input",
    "Resize mode",
    "Overlays",
];

/// Normalize `BackTab` (sent by crossterm for shift-tab) into `Tab + SHIFT`
/// so that `key!(shift-tab)` from crokey matches the actual terminal event.
fn normalize_backtab(kc: KeyCombination) -> KeyCombination {
    if matches!(kc.codes, crokey::OneToThree::One(KeyCode::BackTab)) {
        KeyCombination::new(KeyCode::Tab, kc.modifiers | KeyModifiers::SHIFT)
    } else {
        kc
    }
}

/// Returns the shared display format: lowercase modifiers, dash separator.
/// e.g. `ctrl-p`, `shift-tab`, `space`, `enter`.
/// Format for UI display: title-case modifiers, natural key names.
/// e.g. `Ctrl-p`, `Shift-Tab`, `PgDn`, `Enter`.
pub fn display_format() -> KeyCombinationFormat {
    KeyCombinationFormat::default()
}

/// Format a key combo for display in the UI.
#[cfg(test)]
pub fn format_key(kc: KeyCombination) -> String {
    display_format().to_string(kc)
}

/// Format for config file serialization: all lowercase.
/// e.g. `ctrl-p`, `shift-tab`, `pgdn`, `enter`.
fn config_format() -> KeyCombinationFormat {
    KeyCombinationFormat::default().with_lowercase_modifiers()
}

/// Format a key combo for config file serialization (all lowercase).
pub fn format_key_for_config(kc: KeyCombination) -> String {
    config_format().to_string(kc).to_lowercase()
}

/// Normalize a config key string to crokey convention before parsing.
///
/// `crokey::parse()` lowercases its input, so a bare uppercase letter like
/// `"P"` silently becomes lowercase `"p"`. This helper rewrites bare
/// uppercase letters to the explicit shift form (`"P"` → `"shift-p"`) so
/// that `crokey::parse` produces the correct `KeyCombination`.
pub fn normalize_key_string(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 1 && chars[0].is_ascii_uppercase() {
        format!("shift-{}", chars[0].to_ascii_lowercase())
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// RuntimeBindings: the runtime-resolved keybinding table built from config.
// ---------------------------------------------------------------------------

/// A single runtime-resolved binding.
pub struct RuntimeBinding {
    pub action: Action,
    pub keys: Vec<KeyCombination>,
    pub scopes: &'static [BindingScope],
    pub help_section: Option<&'static str>,
    pub help_description: Option<&'static str>,
    pub hint_contexts: &'static [(HintContext, &'static str)],
    pub palette_name: Option<&'static str>,
    pub palette_description: Option<&'static str>,
}

pub struct RuntimeBindings {
    bindings: Vec<RuntimeBinding>,
    #[allow(dead_code)] // Will be used to filter terminal-native hints
    pub show_terminal_keys: bool,
    format: KeyCombinationFormat,
}

impl RuntimeBindings {
    /// Build runtime bindings from a [`KeysConfig`].
    /// Parses key strings from the config, falling back to defaults for
    /// missing or unparseable entries.
    pub fn from_keys_config(keys: &crate::config::KeysConfig) -> Self {
        Self::new(
            |action| {
                let config_name = action.config_name();
                match keys.bindings.get(config_name) {
                    Some(key_strs) => key_strs
                        .iter()
                        .filter_map(|s| crokey::parse(&normalize_key_string(s)).ok())
                        .collect(),
                    None => BINDING_DEFS
                        .iter()
                        .find(|d| d.action == action)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default(),
                }
            },
            keys.show_terminal_keys,
        )
    }

    /// Build runtime bindings from parsed config keys.
    /// `keys_for` returns the parsed `KeyCombination`s for a given action.
    pub fn new(keys_for: impl Fn(Action) -> Vec<KeyCombination>, show_terminal_keys: bool) -> Self {
        let format = display_format();
        let bindings = BINDING_DEFS
            .iter()
            .map(|def| {
                let keys = keys_for(def.action);
                RuntimeBinding {
                    action: def.action,
                    keys,
                    scopes: def.scopes,
                    help_section: def.help.as_ref().map(|h| h.section),
                    help_description: def.help.as_ref().map(|h| h.description),
                    hint_contexts: def.hint_contexts,
                    palette_name: def.palette.as_ref().map(|p| p.name),
                    palette_description: def.palette.as_ref().map(|p| p.description),
                }
            })
            .collect();
        Self {
            bindings,
            show_terminal_keys,
            format,
        }
    }

    /// Find the action for a key event in the given scope.
    /// Plain bindings (no modifiers) reject Ctrl/Alt combos so that e.g.
    /// Ctrl+D does not accidentally match a plain `d` binding.
    pub fn lookup(&self, key: &KeyEvent, scope: BindingScope) -> Option<Action> {
        let incoming = normalize_backtab(KeyCombination::from(*key).normalized());
        self.bindings
            .iter()
            .filter(|b| b.scopes.contains(&scope))
            .find(|b| {
                b.keys.iter().any(|k| {
                    let norm = normalize_backtab(k.normalized());
                    if norm.codes != incoming.codes {
                        return false;
                    }
                    if norm.modifiers.is_empty() {
                        // Plain binding: reject if any modifier is pressed
                        incoming.modifiers.is_empty()
                    } else {
                        incoming.modifiers.contains(norm.modifiers)
                    }
                })
            })
            .map(|b| b.action)
    }

    /// Display label for the first key combo of an action.
    /// Uses natural casing (e.g. "PgDn", "shift-Tab") suitable for UI display.
    pub fn label_for(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .and_then(|b| b.keys.first())
            .map(|k| self.format.to_string(*k))
            .unwrap_or_default()
    }

    /// Display labels for all key combos of an action, joined with `/`.
    /// Uses natural casing (e.g. "ctrl-f/PgDn") suitable for UI display.
    pub fn labels_for(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .map(|b| {
                b.keys
                    .iter()
                    .map(|k| self.format.to_string(*k))
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default()
    }

    /// Combined label for two related actions (e.g. MoveDown + MoveUp → "j/k").
    /// Takes the first key from each action.
    pub fn combined_label(&self, a: Action, b: Action) -> String {
        let la = self.label_for(a);
        let lb = self.label_for(b);
        if la.is_empty() && lb.is_empty() {
            String::new()
        } else if la.is_empty() {
            lb
        } else if lb.is_empty() {
            la
        } else {
            format!("{la}/{lb}")
        }
    }

    /// Status-bar hints for a given context, in display order.
    pub fn hints_for(&self, ctx: HintContext) -> Vec<(String, &'static str)> {
        let mut result = Vec::new();
        for b in &self.bindings {
            for &(hint_ctx, desc) in b.hint_contexts {
                if hint_ctx == ctx {
                    // For MoveDown/MoveUp, show combined "j/k" style label
                    let label = if b.action == Action::MoveDown {
                        self.combined_label(Action::MoveDown, Action::MoveUp)
                    } else {
                        self.label_for(b.action)
                    };
                    result.push((label, desc));
                }
            }
        }
        result
    }

    /// Help overlay sections grouped by section name, in display order.
    pub fn help_sections(&self) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
        let mut sections: Vec<(&str, Vec<(String, &str)>)> = HELP_SECTION_ORDER
            .iter()
            .map(|&s| (s, Vec::new()))
            .collect();
        for b in &self.bindings {
            if let (Some(section), Some(description)) = (b.help_section, b.help_description) {
                // For MoveDown, show combined "j/k" label
                let label = if b.action == Action::MoveDown {
                    self.combined_label(Action::MoveDown, Action::MoveUp)
                } else {
                    self.labels_for(b.action)
                };
                if label.is_empty() {
                    continue;
                }
                for sec in &mut sections {
                    if sec.0 == section {
                        sec.1.push((label, description));
                        break;
                    }
                }
            }
        }
        sections.retain(|(_, entries)| !entries.is_empty());
        sections
    }

    /// All palette-visible bindings matching a filter string.
    pub fn filtered_palette(&self, input: &str) -> Vec<&RuntimeBinding> {
        let needle = input.trim().to_lowercase();
        if needle.is_empty() {
            return self
                .bindings
                .iter()
                .filter(|b| b.palette_name.is_some())
                .collect();
        }
        let mut name_matches = Vec::new();
        let mut desc_matches = Vec::new();
        for b in &self.bindings {
            if let Some(name) = b.palette_name {
                if name.contains(&needle) {
                    name_matches.push(b);
                } else if let Some(desc) = b.palette_description {
                    if desc.to_lowercase().contains(&needle) {
                        desc_matches.push(b);
                    }
                }
            }
        }
        name_matches.extend(desc_matches);
        name_matches
    }
}

// ---------------------------------------------------------------------------
// Conflict detection: reject configs with duplicate keys in the same scope.
// ---------------------------------------------------------------------------

/// A detected conflict: the same key is bound to two actions in a shared scope.
#[derive(Debug, Clone)]
pub struct KeyConflict {
    pub key_label: String,
    pub scope: BindingScope,
    pub action_a: &'static str,
    pub action_b: &'static str,
}

/// Check whether two `KeyCombination` values would conflict in `lookup()`.
///
/// Mirrors the matching semantics of `RuntimeBindings::lookup()`:
/// - Plain bindings (no modifiers) only conflict with other plain bindings.
/// - Modifier bindings conflict only when modifiers are identical.
fn keys_conflict(a: &KeyCombination, b: &KeyCombination) -> bool {
    let na = normalize_backtab(a.normalized());
    let nb = normalize_backtab(b.normalized());
    if na.codes != nb.codes {
        return false;
    }
    match (na.modifiers.is_empty(), nb.modifiers.is_empty()) {
        (true, true) => true,
        (true, false) | (false, true) => false,
        (false, false) => na.modifiers == nb.modifiers,
    }
}

/// Build the resolved key list for each action (config overrides, falling
/// back to defaults), applying `normalize_key_string` to config values.
fn resolve_keys(
    keys: &crate::config::KeysConfig,
) -> Vec<(Action, Vec<KeyCombination>, &'static [BindingScope])> {
    BINDING_DEFS
        .iter()
        .map(|def| {
            let resolved = match keys.bindings.get(def.action.config_name()) {
                Some(key_strs) => key_strs
                    .iter()
                    .filter_map(|s| crokey::parse(&normalize_key_string(s)).ok())
                    .collect(),
                None => def.default_keys.to_vec(),
            };
            (def.action, resolved, def.scopes)
        })
        .collect()
}

/// Detect key combination conflicts across bindings that share scopes.
///
/// Returns all pairs of actions that bind the same normalized key in at least
/// one overlapping scope. This is called during config validation to prevent
/// silent shadowing (where declaration order would pick a winner).
pub fn detect_conflicts(keys: &crate::config::KeysConfig) -> Vec<KeyConflict> {
    let resolved = resolve_keys(keys);
    let format = config_format();
    let mut conflicts = Vec::new();

    for i in 0..resolved.len() {
        for j in (i + 1)..resolved.len() {
            let (action_a, keys_a, scopes_a) = &resolved[i];
            let (action_b, keys_b, scopes_b) = &resolved[j];

            // Find scopes shared between the two bindings.
            let shared_scopes: Vec<BindingScope> = scopes_a
                .iter()
                .filter(|s| scopes_b.contains(s))
                .copied()
                .collect();
            if shared_scopes.is_empty() {
                continue;
            }

            // Check every key pair for conflicts.
            for ka in keys_a {
                for kb in keys_b {
                    if keys_conflict(ka, kb) {
                        let label = format.to_string(ka.normalized()).to_lowercase();
                        for &scope in &shared_scopes {
                            conflicts.push(KeyConflict {
                                key_label: label.clone(),
                                scope,
                                action_a: action_a.config_name(),
                                action_b: action_b.config_name(),
                            });
                        }
                    }
                }
            }
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_bindings() -> RuntimeBindings {
        RuntimeBindings::new(
            |action| {
                BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default()
            },
            true,
        )
    }

    #[test]
    fn lookup_finds_action_in_correct_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Left),
            Some(Action::MoveDown)
        );
        assert_eq!(bindings.lookup(&key, BindingScope::Center), None);
    }

    #[test]
    fn lookup_plain_key_rejects_ctrl_modifier() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(bindings.lookup(&key, BindingScope::Left), None);
    }

    #[test]
    fn lookup_ctrl_combo_matches() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Global),
            Some(Action::OpenPalette)
        );
    }

    #[test]
    fn label_for_returns_display_label() {
        let bindings = default_bindings();
        let label = bindings.label_for(Action::OpenPalette);
        assert_eq!(label, "Ctrl-p");
    }

    #[test]
    fn labels_for_joins_multiple_keys() {
        let bindings = default_bindings();
        let labels = bindings.labels_for(Action::Quit);
        assert_eq!(labels, "q/Ctrl-c");
    }

    #[test]
    fn combined_label_for_move() {
        let bindings = default_bindings();
        let label = bindings.combined_label(Action::MoveDown, Action::MoveUp);
        assert_eq!(label, "j/k");
    }

    #[test]
    fn hints_for_returns_dynamic_labels() {
        let bindings = default_bindings();
        let hints = bindings.hints_for(HintContext::LeftProject);
        assert!(!hints.is_empty());
        // MoveDown hint should show combined j/k label
        let move_hint = hints.iter().find(|(_, desc)| *desc == "Move");
        assert!(move_hint.is_some());
        assert_eq!(move_hint.unwrap().0, "j/k");
    }

    #[test]
    fn help_sections_produces_valid_sections() {
        let bindings = default_bindings();
        let sections = bindings.help_sections();
        assert!(!sections.is_empty());
        let section_names: Vec<_> = sections.iter().map(|(n, _)| *n).collect();
        assert!(section_names.contains(&"Global"));
        assert!(section_names.contains(&"Projects pane"));
    }

    #[test]
    fn filtered_palette_returns_all_when_empty() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("");
        assert!(results.len() >= 2); // at least delete-project and remove-project
    }

    #[test]
    fn filtered_palette_filters_by_name() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("toggle");
        assert!(results.iter().all(|b| {
            b.palette_name.unwrap().contains("toggle")
                || b.palette_description
                    .unwrap()
                    .to_lowercase()
                    .contains("toggle")
        }));
    }

    #[test]
    fn every_action_has_config_name() {
        // Ensure no action panics when asked for config_name
        for def in BINDING_DEFS {
            let name = def.action.config_name();
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn format_key_display_uses_title_case_modifiers() {
        let kc = key!(ctrl - p);
        assert_eq!(format_key(kc), "Ctrl-p");

        let kc2 = key!(enter);
        assert_eq!(format_key(kc2), "Enter");

        let kc3 = key!(space);
        assert_eq!(format_key(kc3), "Space");
    }

    #[test]
    fn format_key_for_config_is_all_lowercase() {
        assert_eq!(format_key_for_config(key!(ctrl - p)), "ctrl-p");
        assert_eq!(format_key_for_config(key!(shift - tab)), "shift-tab");
        assert_eq!(format_key_for_config(key!(enter)), "enter");
    }

    #[test]
    fn lookup_shift_tab_matches_backtab() {
        let bindings = default_bindings();
        // Crossterm sends BackTab with SHIFT for shift-tab
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Global),
            Some(Action::FocusPrev),
            "shift-tab (BackTab) should match FocusPrev"
        );
    }

    #[test]
    fn new_actions_are_in_binding_defs() {
        let actions_in_defs: Vec<Action> = BINDING_DEFS.iter().map(|d| d.action).collect();
        assert!(actions_in_defs.contains(&Action::ExitInteractive));
        assert!(actions_in_defs.contains(&Action::CloseOverlay));
        assert!(actions_in_defs.contains(&Action::ResizeGrow));
        assert!(actions_in_defs.contains(&Action::StageUnstage));
        assert!(actions_in_defs.contains(&Action::ExitCommitInput));
        assert!(actions_in_defs.contains(&Action::PushToRemote));
        assert!(actions_in_defs.contains(&Action::AddCurrentDir));
    }

    #[test]
    fn every_keyed_binding_has_help_entry() {
        // Every BindingDef that has keys assigned should have a help entry,
        // so it appears in the help overlay. MoveUp is the sole exception
        // because it's shown via MoveDown's combined "j/k" label.
        for def in BINDING_DEFS {
            if def.default_keys.is_empty() || def.action == Action::MoveUp {
                continue;
            }
            assert!(
                def.help.is_some(),
                "Action {:?} has keys but no help entry — add help: Some(HelpEntry {{ ... }})",
                def.action,
            );
        }
    }

    // ── normalize_key_string tests ──────────────────────────────────────

    #[test]
    fn normalize_key_string_bare_uppercase() {
        assert_eq!(normalize_key_string("P"), "shift-p");
        assert_eq!(normalize_key_string("G"), "shift-g");
        assert_eq!(normalize_key_string("A"), "shift-a");
    }

    #[test]
    fn normalize_key_string_lowercase_unchanged() {
        assert_eq!(normalize_key_string("p"), "p");
        assert_eq!(normalize_key_string("j"), "j");
    }

    #[test]
    fn normalize_key_string_modifier_combo_unchanged() {
        assert_eq!(normalize_key_string("ctrl-p"), "ctrl-p");
        assert_eq!(normalize_key_string("ctrl-shift-p"), "ctrl-shift-p");
    }

    #[test]
    fn normalize_key_string_shift_letter_unchanged() {
        assert_eq!(normalize_key_string("shift-p"), "shift-p");
    }

    #[test]
    fn normalize_key_string_special_keys_unchanged() {
        assert_eq!(normalize_key_string("enter"), "enter");
        assert_eq!(normalize_key_string("space"), "space");
        assert_eq!(normalize_key_string("tab"), "tab");
    }

    // ── Conflict detection tests ────────────────────────────────────────

    #[test]
    fn detect_conflicts_same_key_same_scope() {
        // Bind "x" to both toggle_project and new_agent — both are in Left scope.
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings
            .insert("toggle_project".to_string(), vec!["x".to_string()]);
        keys.bindings
            .insert("new_agent".to_string(), vec!["x".to_string()]);
        let conflicts = detect_conflicts(&keys);
        assert!(
            conflicts.iter().any(|c| c.key_label == "x"
                && ((c.action_a == "toggle_project" && c.action_b == "new_agent")
                    || (c.action_a == "new_agent" && c.action_b == "toggle_project"))),
            "expected conflict on 'x' between toggle_project and new_agent, got: {conflicts:?}"
        );
    }

    #[test]
    fn detect_conflicts_different_scopes_no_conflict() {
        // "enter" in Left-only vs Files-only should not conflict.
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings
            .insert("focus_agent".to_string(), vec!["enter".to_string()]);
        keys.bindings
            .insert("open_diff".to_string(), vec!["enter".to_string()]);
        let conflicts = detect_conflicts(&keys);
        // focus_agent is Left scope, open_diff is Files scope — no overlap.
        let bad = conflicts.iter().any(|c| {
            (c.action_a == "focus_agent" && c.action_b == "open_diff")
                || (c.action_a == "open_diff" && c.action_b == "focus_agent")
        });
        assert!(!bad, "should not conflict across non-overlapping scopes");
    }

    #[test]
    fn detect_conflicts_plain_vs_modifier_no_conflict() {
        // "d" and "ctrl-d" in the same scope should not conflict.
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings
            .insert("quit".to_string(), vec!["d".to_string()]);
        keys.bindings
            .insert("toggle_project".to_string(), vec!["ctrl-d".to_string()]);
        let conflicts = detect_conflicts(&keys);
        let bad = conflicts.iter().any(|c| {
            (c.action_a == "quit" && c.action_b == "toggle_project")
                || (c.action_a == "toggle_project" && c.action_b == "quit")
        });
        assert!(
            !bad,
            "plain 'd' and 'ctrl-d' should not conflict: {conflicts:?}"
        );
    }

    #[test]
    fn detect_conflicts_default_config_clean() {
        let keys = crate::config::KeysConfig::default();
        let conflicts = detect_conflicts(&keys);
        assert!(
            conflicts.is_empty(),
            "default config should have no conflicts, found: {conflicts:?}"
        );
    }

    // ── Resolution semantics tests ──────────────────────────────────────
    // These document intentional behavior for future contributors/agents.

    #[test]
    fn lookup_declaration_order_wins() {
        // Build bindings where two actions share the same key in the same scope.
        // The one declared earlier in BINDING_DEFS should win.
        let bindings = RuntimeBindings::new(
            |action| {
                if action == Action::MoveDown || action == Action::MoveUp {
                    // Both bound to 'x' in Left scope
                    vec![crokey::parse("x").unwrap()]
                } else {
                    BINDING_DEFS
                        .iter()
                        .find(|d| d.action == action)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default()
                }
            },
            true,
        );
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        // MoveDown is declared before MoveUp in BINDING_DEFS
        assert_eq!(
            bindings.lookup(&key, BindingScope::Left),
            Some(Action::MoveDown),
            "first action in BINDING_DEFS should win when keys overlap"
        );
    }

    #[test]
    fn lookup_plain_key_ignores_shift_variant() {
        // A plain 'p' binding should NOT match Shift+P.
        let bindings = RuntimeBindings::new(
            |action| {
                if action == Action::Quit {
                    vec![crokey::parse("p").unwrap()]
                } else {
                    BINDING_DEFS
                        .iter()
                        .find(|d| d.action == action)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default()
                }
            },
            true,
        );
        let shift_p = KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert_ne!(
            bindings.lookup(&shift_p, BindingScope::Global),
            Some(Action::Quit),
            "plain 'p' binding should not match Shift+P"
        );
    }

    #[test]
    fn lookup_shift_key_ignores_plain() {
        // A 'shift-p' binding should NOT match plain 'p'.
        let bindings = RuntimeBindings::new(
            |action| {
                if action == Action::Quit {
                    vec![crokey::parse("shift-p").unwrap()]
                } else {
                    BINDING_DEFS
                        .iter()
                        .find(|d| d.action == action)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default()
                }
            },
            true,
        );
        let plain_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_ne!(
            bindings.lookup(&plain_p, BindingScope::Global),
            Some(Action::Quit),
            "shift-p binding should not match plain p"
        );
    }

    #[test]
    fn lookup_shift_p_and_plain_p_coexist() {
        // 'p' and 'shift-p' bound to different actions should both resolve correctly.
        let bindings = RuntimeBindings::new(
            |action| {
                if action == Action::Quit {
                    vec![crokey::parse("p").unwrap()]
                } else if action == Action::ToggleHelp {
                    vec![crokey::parse("shift-p").unwrap()]
                } else {
                    BINDING_DEFS
                        .iter()
                        .find(|d| d.action == action)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default()
                }
            },
            true,
        );
        let plain_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        let shift_p = KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT);
        assert_eq!(
            bindings.lookup(&plain_p, BindingScope::Global),
            Some(Action::Quit),
            "plain 'p' should resolve to Quit"
        );
        assert_eq!(
            bindings.lookup(&shift_p, BindingScope::Global),
            Some(Action::ToggleHelp),
            "shift-p should resolve to Help"
        );
    }

    #[test]
    fn normalized_uppercase_matches_shift() {
        // Char('P') with no modifiers should normalize identically to
        // Char('p') with SHIFT — both represent the same physical keypress.
        let upper = KeyCombination::new(KeyCode::Char('P'), KeyModifiers::NONE).normalized();
        let shift = KeyCombination::new(KeyCode::Char('p'), KeyModifiers::SHIFT).normalized();
        assert_eq!(
            upper.codes, shift.codes,
            "key codes should match after normalization"
        );
        assert_eq!(
            upper.modifiers, shift.modifiers,
            "modifiers should match after normalization"
        );
    }
}
