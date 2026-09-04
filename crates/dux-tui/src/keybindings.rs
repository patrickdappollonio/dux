use crokey::{KeyCombination, KeyCombinationFormat, key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use dux_core::action::Action;

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
    RuntimeKill,
    StartupCommandLogs,
    MacroList,
    /// The "New agent in project" chooser. A scope of its own rather than a
    /// share of `Palette`, which every picker resolves through: an action put
    /// there would arm its key inside all of them.
    ProjectChooser,
    Dialog,
    CommitInput,
    Help,
}

impl BindingScope {
    /// All scope variants, for iteration in diagnostics.
    pub const ALL: &[BindingScope] = &[
        Self::Global,
        Self::Left,
        Self::Center,
        Self::Files,
        Self::Interactive,
        Self::Resize,
        Self::Palette,
        Self::Browser,
        Self::RuntimeKill,
        Self::StartupCommandLogs,
        Self::MacroList,
        Self::ProjectChooser,
        Self::Dialog,
        Self::CommitInput,
        Self::Help,
    ];

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
            Self::RuntimeKill => "Kill running modal",
            Self::StartupCommandLogs => "Startup command logs modal",
            Self::MacroList => "Text macros list",
            Self::ProjectChooser => "Project chooser",
            Self::Dialog => "Dialog",
            Self::CommitInput => "Commit input",
            Self::Help => "Help overlay",
        }
    }
}

/// Where a binding's hint appears in the status bar cheatsheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintContext {
    LeftProject,
    LeftSession,
    LeftTerminal,
    Center,
    Files,
    CommitInput,
}

pub struct HelpEntry {
    pub section: &'static str,
    pub description: &'static str,
}

/// Static definition of a binding. Used as the template for generating default
/// config and for carrying metadata (scopes, help sections, hint contexts). Key
/// combos and display labels are resolved at runtime from the config file via
/// [`RuntimeBindings`]. Palette command names and descriptions live in the
/// surface-aware core registry ([`dux_core::palette`]); the runtime palette
/// listing joins them to bindings by [`Action`].
pub struct BindingDef {
    pub action: Action,
    pub default_keys: &'static [KeyCombination],
    pub scopes: &'static [BindingScope],
    pub help: Option<HelpEntry>,
    pub hint_contexts: &'static [(HintContext, &'static str)],
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
            BindingScope::RuntimeKill,
            BindingScope::StartupCommandLogs,
            BindingScope::Help,
        ],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Move through projects and sessions",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Move"),
            (HintContext::LeftSession, "Move"),
            (HintContext::LeftTerminal, "Move"),
            (HintContext::Files, "Move"),
        ],
    },
    BindingDef {
        action: Action::MoveUp,
        default_keys: &[key!(k), key!(Up)],
        scopes: &[
            BindingScope::Left,
            BindingScope::Files,
            BindingScope::Palette,
            BindingScope::Browser,
            BindingScope::RuntimeKill,
            BindingScope::StartupCommandLogs,
            BindingScope::Help,
        ],
        help: None, // covered by MoveDown's combined label
        hint_contexts: &[],
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
    },
    BindingDef {
        // Sits beside `new_agent` deliberately: the `<n>` flow only ever
        // reaches a managed worktree, so without a key of its own the folder
        // agent is discoverable through the palette alone.
        //
        // Two keys and two scopes, and the defaults are a flat CROSS PRODUCT
        // rather than one key per surface: the chord also fires in the agents
        // pane and the bare letter also fires in an idle chooser, both
        // accepted. What each key is for is the chooser's search row, which
        // types the letters the moment it is engaged (see
        // `text_field_owns_key`): the bare letter is the pane-and-idle-list
        // key, and the chord is the one that still reaches dux while a filter
        // is being typed. A terminal or multiplexer that swallows the chord
        // never delivers it; the bare key, the palette command and a rebinding
        // are the ways round that.
        //
        // `new_standalone_terminal` beside it stays key-less on purpose: a
        // shell in a directory is a smaller act than starting an agent, and it
        // has no pane of its own to be discovered from.
        action: Action::NewStandaloneAgent,
        default_keys: &[key!(s), key!(ctrl - s)],
        scopes: &[BindingScope::Left, BindingScope::ProjectChooser],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Start a standalone agent in a folder you choose",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Standalone"),
            (HintContext::LeftSession, "Standalone"),
            // The terminals subsection is still the agents pane, and creating
            // an agent needs nothing selected, so the key works and is named
            // there too.
            (HintContext::LeftTerminal, "Standalone"),
        ],
    },
    BindingDef {
        action: Action::NewAgentFromPr,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::NewAgentFromWorktree,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: opens the project chooser to pick a project that
        // subsequent project-scoped palette commands act on. No default key.
        action: Action::ManageProjects,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: opens the project chooser, then the worktree manager
        // for the picked project. No default key.
        action: Action::ManageWorktrees,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Enters filter mode over the whole sidebar: the flat agent list and the
        // terminal list below it. `/` is free in the Left scope (it is only bound
        // in Files/Browser today, and scopes are independent). In filter mode
        // printable keys type the query, so navigation falls back to the arrow
        // keys, mirroring the project browser; those arrows cross the
        // agents/terminals boundary, so every result is reachable whichever kind
        // of row matched.
        action: Action::FilterAgents,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Filter agents and terminals by name, branch, project, or folder",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Filter"),
            (HintContext::LeftSession, "Filter"),
        ],
    },
    BindingDef {
        action: Action::ForkAgent,
        default_keys: &[key!(f)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Fork selected agent into a fresh worktree",
        }),
        hint_contexts: &[(HintContext::LeftSession, "Fork")],
    },
    BindingDef {
        action: Action::ChangeAgentProvider,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ChangeDefaultProvider,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ChangeProjectDefaultProvider,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ChangeTheme,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ReloadConfig,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::StartWebServer,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::StartBackgroundServer,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::StopBackgroundServer,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // No default key: a tri-state a user changes when they move between
        // networks does not earn a chord, and the palette is where it lives.
        action: Action::SetTailscaleMode,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleProjectAutoReopenAgents,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleAgentAutoReopen,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ConfigureStartupCommand,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ConfigureGlobalEnv,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ConfigureProjectEnv,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::RerunStartupCommandOnAgent,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ReadStartupCommandLogs,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::FocusAgent,
        default_keys: &[key!(enter)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Focus the selected agent and type into it (launches a dormant one)",
        }),
        hint_contexts: &[
            (HintContext::LeftSession, "Focus"),
            (HintContext::LeftTerminal, "Focus"),
            // With the center pane focused this key is TYPED into a live
            // agent (it is a typing-owned default) and launches a dormant
            // one, so the footer word is "Type", not "Interact".
            (HintContext::Center, "Type"),
        ],
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
    },
    BindingDef {
        action: Action::CopyPath,
        default_keys: &[key!(y)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Copy agent directory",
        }),
        hint_contexts: &[
            (HintContext::LeftProject, "Copy path"),
            (HintContext::LeftSession, "Copy path"),
        ],
    },
    BindingDef {
        action: Action::OpenWorktreeInEditor,
        default_keys: &[key!(o)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Open selected agent's directory in the default editor",
        }),
        hint_contexts: &[(HintContext::LeftSession, "Open")],
    },
    BindingDef {
        action: Action::ChooseWorktreeEditor,
        default_keys: &[key!(shift - o)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Choose an editor and open the selected agent worktree",
        }),
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::CheckoutProjectDefaultBranch,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::ShowTerminal,
        default_keys: &[key!(t)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Open/launch companion terminal",
        }),
        hint_contexts: &[
            (HintContext::LeftSession, "Terminal"),
            (HintContext::Center, "Terminal"),
        ],
    },
    // ── Agent tabs (Center-scope; never in fullscreen) ────────────
    // Only the Ctrl arrows (and Ctrl-1..9) are tab keys: the focused center
    // pane types into a live agent, so a plain arrow must reach the agent as a
    // caret move ([`center_typing_owns_key`]), not switch tabs. A terminal that
    // cannot deliver a modified arrow can rebind these actions to any
    // deliverable chord.
    BindingDef {
        action: Action::NextTab,
        default_keys: &[key!(ctrl - Right)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Next tab",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::PrevTab,
        default_keys: &[key!(ctrl - Left)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Previous tab",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab1,
        default_keys: &[key!(ctrl - 1)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Focus tab by number (tab 4 ships with no default key)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab2,
        default_keys: &[key!(ctrl - 2)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab3,
        default_keys: &[key!(ctrl - 3)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // No default key. Under the legacy terminal protocol Ctrl-4 IS
        // Ctrl-\ (both arrive as byte 0x1c, and `normalize_ctrl_punct` folds
        // them together), and that key belongs to the macro bar
        // (`OpenMacroBar`), which gained Center scope for the minimized
        // typeable pane. Tab 4 stays reachable via
        // NextTab/PrevTab or a custom rebind; a user who rebinds the macro
        // bar off Ctrl-\ can give `select_tab_4` the key back.
        action: Action::SelectTab4,
        default_keys: &[],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab5,
        default_keys: &[key!(ctrl - 5)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab6,
        default_keys: &[key!(ctrl - 6)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab7,
        default_keys: &[key!(ctrl - 7)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab8,
        default_keys: &[key!(ctrl - 8)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SelectTab9,
        default_keys: &[key!(ctrl - 9)],
        scopes: &[BindingScope::Center],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::NewTab,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::CloseTab,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::DeleteSession,
        default_keys: &[key!(ctrl - d)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Delete selected session/worktree",
        }),
        hint_contexts: &[
            (HintContext::LeftSession, "Delete"),
            (HintContext::LeftTerminal, "Delete"),
            (HintContext::Center, "Delete"),
        ],
    },
    BindingDef {
        action: Action::DeleteTerminal,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    // ── Agent pane ────────────────────────────────────────────────
    BindingDef {
        action: Action::InteractAgent,
        default_keys: &[key!(i)],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Open the selected agent fullscreen (keys go to it verbatim)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        // Center scope: the chord also opens the bar over the
        // minimized typeable pane; being a chord, the typing bypass never
        // swallows it.
        action: Action::OpenMacroBar,
        default_keys: &[key!(ctrl - '\\')],
        scopes: &[BindingScope::Interactive, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Open the macro bar to send text macros (windowed or fullscreen)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::OpenCurrentPullRequest,
        default_keys: &[key!(p)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Open current pull request in the default browser",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: attach (pin) a GitHub PR to the selected agent.
        action: Action::AttachPullRequest,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: detach the agent's pull request and stop looking.
        action: Action::DetachPullRequest,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: the way back from a detach.
        action: Action::ResumePullRequestAutodetection,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // The fullscreen toggle. In the Center scope it maximizes the focused
        // agent (launching a dormant tab fullscreen-seeking); in Interactive
        // scope its byte pattern minimizes; in the Left scope it reopens
        // fullscreen for a live selected agent without launching a dormant
        // one. It absorbed the retired `exit_interactive` action, whose
        // bindings the config loader folds into this one.
        action: Action::ToggleFullscreen,
        default_keys: &[key!(ctrl - g)],
        scopes: &[
            BindingScope::Interactive,
            BindingScope::Center,
            BindingScope::Left,
        ],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Toggle fullscreen (fullscreen keys go to the agent verbatim)",
        }),
        hint_contexts: &[(HintContext::Center, "Fullscreen")],
    },
    BindingDef {
        action: Action::ScrollPageUp,
        default_keys: &[key!(pageup)],
        scopes: &[
            BindingScope::Center,
            BindingScope::Interactive,
            BindingScope::Help,
        ],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Scroll up one page (forwarded when the app owns the screen)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ScrollPageDown,
        default_keys: &[key!(pagedown)],
        scopes: &[
            BindingScope::Center,
            BindingScope::Interactive,
            BindingScope::Help,
        ],
        help: Some(HelpEntry {
            section: "Agent pane",
            description: "Scroll down one page (forwarded when the app owns the screen)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ScrollLineUp,
        default_keys: &[key!(Up)],
        scopes: &[BindingScope::Interactive, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Scrolling",
            description: "Scroll up one line (in a typing pane, only while scrolled back)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ScrollLineDown,
        default_keys: &[key!(Down), key!(space)],
        scopes: &[BindingScope::Interactive, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Scrolling",
            description: "Scroll down one line (in a typing pane, only while scrolled back)",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ScrollToBottom,
        default_keys: &[key!(q), key!(end)],
        scopes: &[
            BindingScope::Interactive,
            BindingScope::Center,
            BindingScope::Help,
        ],
        help: Some(HelpEntry {
            section: "Scrolling",
            description: "Exit scroll mode and jump to latest output",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ScrollToTop,
        default_keys: &[key!(home)],
        scopes: &[
            BindingScope::Interactive,
            BindingScope::Center,
            BindingScope::Help,
        ],
        help: Some(HelpEntry {
            section: "Scrolling",
            description: "Jump to top of scrollback",
        }),
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::SearchFiles,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Search changed files",
        }),
        hint_contexts: &[(HintContext::Files, "Search")],
    },
    BindingDef {
        action: Action::SearchNext,
        default_keys: &[key!(n)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            description: "Jump to next search match",
        }),
        hint_contexts: &[(HintContext::Files, "Next match")],
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
    },
    // ── Global ────────────────────────────────────────────────────
    // (placed after pane bindings so palette / help appear last in hints)
    BindingDef {
        action: Action::FocusNext,
        default_keys: &[key!(tab), key!(ctrl - o)],
        scopes: &[BindingScope::Global, BindingScope::RuntimeKill],
        help: Some(HelpEntry {
            section: "Global",
            description: "Focus next pane",
        }),
        hint_contexts: &[(HintContext::Center, "Next"), (HintContext::Files, "Next")],
    },
    BindingDef {
        action: Action::FocusPrev,
        default_keys: &[key!(shift - tab), key!(ctrl - y)],
        scopes: &[BindingScope::Global, BindingScope::RuntimeKill],
        help: Some(HelpEntry {
            section: "Global",
            description: "Focus previous pane",
        }),
        hint_contexts: &[],
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
            (HintContext::LeftTerminal, "Palette"),
            (HintContext::Center, "Palette"),
            (HintContext::Files, "Palette"),
        ],
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
    },
    BindingDef {
        action: Action::ToggleGitPane,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char(']'),
            KeyModifiers::NONE,
        )],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Toggle git pane",
        }),
        hint_contexts: &[],
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
            (HintContext::LeftTerminal, "Help"),
            (HintContext::Center, "Help"),
            (HintContext::Files, "Help"),
        ],
    },
    BindingDef {
        action: Action::ForceRedraw,
        default_keys: &[key!(ctrl - l)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Force a full terminal redraw",
        }),
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::CloseOverlay,
        default_keys: &[key!(esc)],
        scopes: &[
            BindingScope::Global,
            BindingScope::Palette,
            BindingScope::Browser,
            BindingScope::RuntimeKill,
            BindingScope::Dialog,
        ],
        help: Some(HelpEntry {
            section: "Global",
            description: "Close the current overlay or dialog",
        }),
        hint_contexts: &[],
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
    },
    // ── Overlays and dialogs ──────────────────────────────────────
    BindingDef {
        action: Action::SearchToggle,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )],
        scopes: &[
            BindingScope::Browser,
            BindingScope::RuntimeKill,
            BindingScope::StartupCommandLogs,
            // The project chooser (`PickProject`) falls through to the Palette
            // scope for this shared vocabulary; the command palette itself
            // ignores plain chars, so `/` only reaches search-capable list
            // modals here.
            BindingScope::Palette,
        ],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Toggle search mode",
        }),
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::ExitPathEditorOnProjectAdd,
        default_keys: &[key!(ctrl - g)],
        scopes: &[BindingScope::Browser],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Exit typed-path mode in the project browser",
        }),
        hint_contexts: &[],
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
    },
    BindingDef {
        action: Action::OpenStartupCommandLogFile,
        default_keys: &[key!(o)],
        scopes: &[BindingScope::StartupCommandLogs],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Open selected startup command log file",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::OpenStartupCommandLogFolder,
        default_keys: &[KeyCombination::one_key(
            KeyCode::Char('O'),
            KeyModifiers::SHIFT,
        )],
        scopes: &[BindingScope::StartupCommandLogs],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Open selected startup command log folder",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::Confirm,
        default_keys: &[key!(enter)],
        scopes: &[
            BindingScope::Dialog,
            BindingScope::Palette,
            BindingScope::RuntimeKill,
        ],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Confirm the selected action",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleSelection,
        default_keys: &[
            key!(h),
            key!(l),
            key!(Left),
            key!(Right),
            key!(tab),
            key!(shift - tab),
        ],
        scopes: &[BindingScope::Dialog],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Toggle between options in a confirmation dialog",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleMarked,
        default_keys: &[key!(space)],
        scopes: &[BindingScope::RuntimeKill],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Toggle the hovered runtime selection",
        }),
        hint_contexts: &[],
    },
    // ── Palette-only (no direct keybinding) ────────────────────────
    BindingDef {
        action: Action::KillRunning,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    // Web-surface action, deliberately inert in the TUI (no key, no scope, no
    // help, no palette row): the web's app menu opens the Monaco config.toml
    // editor for it. The `BindingDef` stays because `config.rs` validates every
    // user `[keys]` action name against BINDING_DEFS, and dropping it would reject
    // an existing config that binds `edit_config`.
    BindingDef {
        action: Action::EditConfig,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    // Web-surface action, deliberately inert in the TUI (see `EditConfig`
    // above): the web's app menu opens the Preferences dialog for it.
    BindingDef {
        action: Action::RenameWebInstance,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    // Web-surface action, deliberately inert in the TUI (see `EditConfig`
    // above): highlight-to-copy is a browser-terminal behavior, exposed on the
    // web as the `ui.copy_on_select` Preferences row. That row writes through
    // the generic settings PATCH, but `WireCommand::ToggleCopyOnSelect` still
    // backs `POST /api/v1/ui/toggle-copy-on-select`.
    BindingDef {
        action: Action::ToggleCopyOnSelect,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::NewTerminal,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: opens the project chooser to spawn a project terminal.
        // No default key (like manage-projects).
        action: Action::NewProjectTerminal,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: opens a terminal owned by nothing, in the home
        // directory. No default key (like new-terminal-for-project).
        action: Action::NewStandaloneTerminal,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::RenameSession,
        default_keys: &[key!(e)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Rename the selected agent session",
        }),
        hint_contexts: &[
            (HintContext::LeftSession, "Rename"),
            (HintContext::Center, "Rename"),
        ],
    },
    BindingDef {
        action: Action::OpenAgentInfo,
        default_keys: &[],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            description: "Show agent info (branch lineage, worktree, status)",
        }),
        hint_contexts: &[],
    },
    // The two first-load screens, on demand. Deliberately NO default keybinding:
    // they are read-once screens, so consuming a hotkey for either would be a bad
    // trade. The palette is how you reach them.
    BindingDef {
        action: Action::ShowWelcomeScreen,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ShowReleaseNotes,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::DeleteProject,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::RemoveProject,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::SortAgents,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::RemoveGitPane,
        default_keys: &[key!(ctrl - ']')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            description: "Remove or restore git pane",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::EditMacros,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::NewMacro,
        default_keys: &[key!(n)],
        scopes: &[BindingScope::MacroList],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Macro list: create a new macro",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::DeleteMacro,
        default_keys: &[key!(d), key!(Delete)],
        scopes: &[BindingScope::MacroList],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Macro list: delete the highlighted macro",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ClearTextField,
        default_keys: &[key!(ctrl - d)],
        scopes: &[BindingScope::Dialog, BindingScope::CommitInput],
        help: Some(HelpEntry {
            section: "Overlays",
            description: "Empty the focused full-text field",
        }),
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::DebugInput,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleDiffLineNumbers,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ResourceMonitor,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleGithubIntegration,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::RecheckGithub,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleAlwaysShowTabs,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleTabReachesAgent,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ToggleRandomizedPetNameDefault,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::TogglePrBannerPosition,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::ForceReconnectAgent,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        // Palette-only: forces a changed-files recompute for the selected agent.
        // No default key.
        action: Action::RefreshChanges,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    // Manual reordering: palette-only (no default keys, no help section), the TUI
    // equivalent of the web's drag-to-reorder.
    BindingDef {
        action: Action::MoveAgentUp,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveAgentDown,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveAgentTop,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveAgentBottom,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveTerminalUp,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveTerminalDown,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveTerminalTop,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
    },
    BindingDef {
        action: Action::MoveTerminalBottom,
        default_keys: &[],
        scopes: &[],
        help: None,
        hint_contexts: &[],
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

/// Keys that a dialog's single-line text field owns, so the dialog's binding
/// lookup never sees them.
///
/// The rename-agent and new-agent-name modals pair a text field with
/// checkboxes. Plain characters type into the field and the horizontal arrows
/// move its caret (modified arrows too: the field maps `Alt`/`Ctrl` arrows to
/// word movement), so neither may be claimed by a Dialog-scope binding there.
/// `Action::ToggleSelection` is bound by default to `h`/`l`/`Left`/`Right` as
/// well as `Tab`/`Shift-Tab`, which is right for the button-only confirmation
/// dialogs that make up almost every Dialog-scope consumer but wrong for these
/// two.
///
/// This is the ONE predicate behind both halves of that behaviour: the input
/// layer suppresses the lookup with it (`app::input`), and the renderer picks
/// the footer's key with it via
/// [`RuntimeBindings::label_for_text_field_dialog`]. Keeping both on this
/// function is what stops the hint from naming a key the field swallows.
///
/// Accepts anything that converts into a [`KeyCombination`], so the input layer
/// can hand it a raw crossterm `KeyEvent` and the renderer a stored binding key.
pub fn text_field_owns_key(key: impl Into<KeyCombination>) -> bool {
    let key: KeyCombination = key.into();
    match key.codes {
        crokey::OneToThree::One(KeyCode::Char(_)) => !key.modifiers.contains(KeyModifiers::CONTROL),
        crokey::OneToThree::One(KeyCode::Left) | crokey::OneToThree::One(KeyCode::Right) => true,
        _ => false,
    }
}

/// The one sentence every surface uses for the state `tab_reaches_agent` can
/// strand a user in: the option is on and the user's own `focus_next` and
/// `focus_prev` bindings are all keys the typeable pane types, so no keystroke
/// moves focus out of it. dux warns rather than refusing (the mouse and the
/// palette still work, and it does not overrule a user's bindings), so the
/// sentence has to carry the fix with it.
pub const NO_PANE_CHORD_ADVICE: &str = "no pane chord reaches dux from the typeable center pane: \
     every key bound to focus_next and focus_prev types into the agent there. Rebind focus_next \
     under [keys] in config.toml (Ctrl-o is its default) to get a keyboard way out.";

/// Keys the minimized, typeable center pane forwards to the agent's PTY
/// instead of resolving as dux bindings.
///
/// The routing rule is: chords belong to dux, typing belongs to the agent.
/// Ctrl and Alt chords, Tab/Shift-Tab and the page keys resolve through the
/// binding ladder as usual; unmodified (or Shift-modified) printables, Enter,
/// Backspace, Delete, Esc, the arrows and Home/End type into the agent. "Dux
/// wins" cannot apply literally to plain letters, or `q`, `?`, `[` and `]`
/// would make the pane untypeable.
///
/// `tab_reaches_agent` (`[ui] tab_reaches_agent`) is the one opt-in that moves
/// the line: with it on, Tab and both spellings of Shift-Tab type into the
/// agent too, and the chords bound to `focus_next`/`focus_prev` are what move
/// panes. Pass the RAW crossterm event, before backtab normalization, so both
/// spellings are seen.
///
/// The ONE deliberate chord exception is Ctrl+c: it forwards to the agent (the
/// encoder emits 0x03, SIGINT) because interrupting the agent quickly is the
/// common intent, and Quit stays reachable via `q` from any non-typeable pane
/// and via the palette.
///
/// The sibling of [`text_field_owns_key`]: `app::input::handle_key` suppresses
/// the Global and Center binding lookups with it while the pane is typeable
/// and at the live edge, so hints must not advertise a key it swallows.
pub fn center_typing_owns_key(key: &KeyEvent, tab_reaches_agent: bool) -> bool {
    let mods = key.modifiers;
    if mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::ALT) {
        // Ctrl+c is the one chord the agent gets (see above). Any extra
        // modifier turns it back into a dux-side chord.
        return key.code == KeyCode::Char('c') && mods == KeyModifiers::CONTROL;
    }
    if tab_reaches_agent && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        return true;
    }
    matches!(
        key.code,
        KeyCode::Char(_)
            | KeyCode::Enter
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Esc
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
    )
}

/// Normalize `BackTab` (sent by crossterm for shift-tab) into `Tab + SHIFT`
/// so that `key!(shift-tab)` from crokey matches the actual terminal event.
fn normalize_backtab(kc: KeyCombination) -> KeyCombination {
    if matches!(kc.codes, crokey::OneToThree::One(KeyCode::BackTab)) {
        KeyCombination::new(KeyCode::Tab, kc.modifiers | KeyModifiers::SHIFT)
    } else {
        kc
    }
}

/// Crossterm maps Ctrl+punctuation bytes 0x1C..0x1F to Ctrl+'4'..'7' instead
/// of the actual characters `\`, `]`, `^`, `_`. Normalize the digit back to
/// the real punctuation so that `key!(ctrl - ']')` matches what the terminal
/// actually delivers.
fn normalize_ctrl_punct(kc: KeyCombination) -> KeyCombination {
    if !kc.modifiers.contains(KeyModifiers::CONTROL) {
        return kc;
    }
    let replacement = match kc.codes {
        crokey::OneToThree::One(KeyCode::Char('4')) => '\\',
        crokey::OneToThree::One(KeyCode::Char('5')) => ']',
        crokey::OneToThree::One(KeyCode::Char('6')) => '^',
        crokey::OneToThree::One(KeyCode::Char('7')) => '_',
        _ => return kc,
    };
    KeyCombination::new(KeyCode::Char(replacement), kc.modifiers)
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
                // The palette name/description come from the core registry
                // (`dux_core::palette`), the single source of truth for the TUI
                // palette. Every row there is listed by the TUI; an action with
                // no row (e.g. `EditConfig`, which only ever had a web-palette
                // row) simply carries no palette name and is never listed. The
                // exhaustiveness test pins that join in both directions.
                let core_palette = dux_core::palette::find_by_action(def.action);
                RuntimeBinding {
                    action: def.action,
                    keys,
                    scopes: def.scopes,
                    help_section: def.help.as_ref().map(|h| h.section),
                    help_description: def.help.as_ref().map(|h| h.description),
                    hint_contexts: def.hint_contexts,
                    palette_name: core_palette.map(|c| c.name),
                    palette_description: core_palette.map(|c| c.description),
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
    /// Ctrl+d does not accidentally match a plain `d` binding.
    pub fn lookup(&self, key: &KeyEvent, scope: BindingScope) -> Option<Action> {
        let incoming =
            normalize_ctrl_punct(normalize_backtab(KeyCombination::from(*key).normalized()));
        self.bindings
            .iter()
            .filter(|b| b.scopes.contains(&scope))
            .find(|b| {
                b.keys.iter().any(|k| {
                    let norm = normalize_ctrl_punct(normalize_backtab(k.normalized()));
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

    /// First key combination of an action that satisfies `reachable`.
    ///
    /// A surface that routes some keys elsewhere before consulting the
    /// bindings (see [`text_field_owns_key`]) must not advertise a key it
    /// swallows. Passing the surface's own suppression rule here keeps the
    /// hint and the routing reading from one predicate.
    pub fn first_key_reaching(
        &self,
        action: Action,
        reachable: impl Fn(KeyCombination) -> bool,
    ) -> Option<KeyCombination> {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .and_then(|b| b.keys.iter().copied().find(|k| reachable(*k)))
    }

    /// Display label for the first key combination of an action that satisfies
    /// `reachable`, or `None` when the surface suppresses every one of them.
    pub fn label_for_reaching(
        &self,
        action: Action,
        reachable: impl Fn(KeyCombination) -> bool,
    ) -> Option<String> {
        self.first_key_reaching(action, reachable)
            .map(|k| self.format.to_string(k))
    }

    /// Display label for an action inside a dialog whose single-line text field
    /// owns the letters and the horizontal arrows ([`text_field_owns_key`]).
    ///
    /// `None` means the user has rebound the action so that every one of its
    /// keys is swallowed by the field there; a caller must then drop the hint
    /// rather than name a key that types a character.
    pub fn label_for_text_field_dialog(&self, action: Action) -> Option<String> {
        self.label_for_reaching(action, |k| !text_field_owns_key(k))
    }

    /// Display label for an action's first key that still reaches the bindings
    /// from the typeable center pane ([`center_typing_owns_key`]).
    ///
    /// `None` means every one of the action's keys types into the agent there,
    /// so the hint must drop rather than name a key the pane swallows. This is
    /// what keeps the hint from naming Tab once `tab_reaches_agent` hands it
    /// over.
    pub fn label_for_typeable_center(
        &self,
        action: Action,
        tab_reaches_agent: bool,
    ) -> Option<String> {
        self.label_for_reaching(action, |k| {
            let event: KeyEvent = k.into();
            !center_typing_owns_key(&event, tab_reaches_agent)
        })
    }

    /// Whether `tab_reaches_agent` leaves the typeable center pane with no
    /// keyboard way out: every key bound to `focus_next` AND every key bound to
    /// `focus_prev` types into the agent there, so nothing on the keyboard
    /// moves focus off the pane. A user who rebinds both to Tab alone lands
    /// here, and the surfaces answer with [`NO_PANE_CHORD_ADVICE`].
    pub fn typeable_center_traps_focus(&self, tab_reaches_agent: bool) -> bool {
        tab_reaches_agent
            && self
                .label_for_typeable_center(Action::FocusNext, true)
                .is_none()
            && self
                .label_for_typeable_center(Action::FocusPrev, true)
                .is_none()
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

    /// All palette-visible bindings matching a filter string, in two tiers.
    ///
    /// `direct` is the palette's original rule, unchanged: case-insensitive,
    /// runs of whitespace and dashes interchangeable, and the whole query has
    /// to appear CONTIGUOUSLY in the command name or, failing that, in its
    /// description. It is a phrase match, so `open current pr` finds
    /// `open-current-pr` but `new tab` finds nothing.
    ///
    /// `other` is the looser tier, listed after the phrase hits: the words in
    /// any order, each of them possibly partial, ranked by
    /// [`dux_core::palette::search`], with everything already in `direct`
    /// removed. It is usually a multi-word query that lands here, but not
    /// only: the two tiers tokenize differently, and a single word can reach
    /// the second tier through that gap. The scorer strips apostrophes, so
    /// `agents` reaches a description that says `agent's` while the phrase
    /// tier, which matches the text as written, does not.
    pub fn palette_matches(&self, input: &str) -> PaletteMatches<'_> {
        let direct = self.direct_palette_matches(input);
        let other = dux_core::palette::search::ranked_matches(input)
            .into_iter()
            .map(|hit| dux_core::palette::PALETTE_COMMANDS[hit.index].name)
            .filter(|name| {
                !direct
                    .iter()
                    .any(|binding| binding.palette_name == Some(*name))
            })
            .filter_map(|name| {
                self.bindings
                    .iter()
                    .find(|binding| binding.palette_name == Some(name))
            })
            .collect();
        PaletteMatches { direct, other }
    }

    /// Both tiers of [`RuntimeBindings::palette_matches`] as one flat list,
    /// the direct hits first. That flat list is what the palette draws and
    /// what its cursor indexes; the tiers exist to fix the order, not to be
    /// shown apart.
    pub fn filtered_palette(&self, input: &str) -> Vec<&RuntimeBinding> {
        let matches = self.palette_matches(input);
        let mut flat = matches.direct;
        flat.extend(matches.other);
        flat
    }

    /// The contiguous-phrase tier, which is the palette's original matching.
    fn direct_palette_matches(&self, input: &str) -> Vec<&RuntimeBinding> {
        let needle = normalize_palette_match(input);
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
                if normalize_palette_match(name).contains(&needle) {
                    name_matches.push(b);
                } else if let Some(desc) = b.palette_description
                    && normalize_palette_match(desc).contains(&needle)
                {
                    desc_matches.push(b);
                }
            }
        }
        name_matches.extend(desc_matches);
        name_matches
    }
}

/// The palette's matches in two tiers rather than one merged ranking,
/// because the looser hits are meant to sit BELOW the exact ones rather than
/// be interleaved with them by a score. Nothing draws a boundary between
/// them; the split is only how the flat list gets its order.
pub struct PaletteMatches<'a> {
    /// Contiguous-phrase matches, in the palette's canonical table order.
    pub direct: Vec<&'a RuntimeBinding>,
    /// Looser matches (words in any order, partial words), best first, with
    /// the direct hits removed.
    pub other: Vec<&'a RuntimeBinding>,
}

/// Lowercase `s` and collapse any run of whitespace or dash characters into a
/// single `-`, with no leading or trailing separator. This lets the palette
/// match natural-language queries like `open current pr` against dashed
/// command names like `open-current-pr`.
fn normalize_palette_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_sep = false;
    for c in s.chars() {
        if c.is_whitespace() || c == '-' {
            if !out.is_empty() {
                last_was_sep = true;
            }
        } else {
            if last_was_sep {
                out.push('-');
                last_was_sep = false;
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
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
    let na = normalize_ctrl_punct(normalize_backtab(a.normalized()));
    let nb = normalize_ctrl_punct(normalize_backtab(b.normalized()));
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

/// The scope a collision between these two scopes should be REPORTED under, or
/// `None` when the same keystroke can never reach both.
///
/// Usually two scopes collide only with themselves, but a surface whose key
/// handler consults one scope and then FALLS THROUGH to another makes those two
/// collide even though no binding lists both: the fallback is shadowed by the
/// first lookup, silently, which is exactly what this detector exists to
/// refuse. Such a pair reports under the surface the user can actually see, not
/// whichever of the two happened to be declared first, so the message points at
/// the modal where the shadowing is visible.
///
/// The one such ladder today is the project chooser, whose handler asks
/// `ProjectChooser` and then `Palette` (`app::input`). Add a rule here whenever
/// a handler grows another fallback, or the shadowing goes unreported.
fn conflict_scope(a: BindingScope, b: BindingScope) -> Option<BindingScope> {
    if a == b {
        return Some(a);
    }
    match (a, b) {
        (BindingScope::ProjectChooser, BindingScope::Palette)
        | (BindingScope::Palette, BindingScope::ProjectChooser) => {
            Some(BindingScope::ProjectChooser)
        }
        _ => None,
    }
}

/// Detect key combination conflicts across bindings that share scopes.
///
/// Returns all pairs of actions that bind the same normalized key in at least
/// one overlapping scope (see [`conflict_scope`], which also decides which
/// scope such a pair is reported under). This is called during config
/// validation to prevent silent shadowing (where declaration order would pick a
/// winner).
pub fn detect_conflicts(keys: &crate::config::KeysConfig) -> Vec<KeyConflict> {
    let resolved = resolve_keys(keys);
    let format = config_format();
    let mut conflicts = Vec::new();

    for i in 0..resolved.len() {
        for j in (i + 1)..resolved.len() {
            let (action_a, keys_a, scopes_a) = &resolved[i];
            let (action_b, keys_b, scopes_b) = &resolved[j];

            // Find the scopes a keystroke could reach both bindings through,
            // each named the way it will be reported.
            let mut shared_scopes: Vec<BindingScope> = Vec::new();
            for sa in scopes_a.iter() {
                for sb in scopes_b.iter() {
                    if let Some(scope) = conflict_scope(*sa, *sb)
                        && !shared_scopes.contains(&scope)
                    {
                        shared_scopes.push(scope);
                    }
                }
            }
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

// ---------------------------------------------------------------------------
// Byte-level binding matching for raw stdin passthrough in interactive mode.
// ---------------------------------------------------------------------------

/// A single intercepted binding: the raw byte pattern and the action it maps
/// to. `conditional` is true for bindings that only fire when scrollback is
/// active (ScrollLineUp/Down).
#[derive(Debug, Clone)]
pub struct InteractiveByteBinding {
    pub pattern: Vec<u8>,
    pub action: Action,
    pub conditional: bool,
}

/// Precomputed byte patterns for all `Interactive`-scoped bindings.
/// Built once at startup and stored on `App`.
#[derive(Debug, Clone)]
pub struct InteractiveBytePatterns {
    pub bindings: Vec<InteractiveByteBinding>,
}

impl InteractiveBytePatterns {
    /// Match a raw byte sequence against the intercepted bindings.
    /// Returns the action and whether it's conditional on scrollback.
    pub fn match_sequence(&self, seq: &[u8]) -> Option<(Action, bool)> {
        self.bindings
            .iter()
            .find(|b| b.pattern == seq)
            .map(|b| (b.action, b.conditional))
    }
}

impl RuntimeBindings {
    /// The raw byte patterns of every key bound to `action`, whatever its
    /// scope. Keys with no byte form are skipped.
    pub fn byte_patterns_for(&self, action: Action) -> Vec<Vec<u8>> {
        self.bindings
            .iter()
            .filter(|rb| rb.action == action)
            .flat_map(|rb| rb.keys.iter().filter_map(key_combination_to_bytes))
            .collect()
    }

    /// Build byte patterns for all `Interactive`-scoped bindings.
    /// Each key combination is converted to its raw terminal byte
    /// representation. Bindings that cannot be byte-encoded are skipped.
    pub fn interactive_byte_patterns(&self) -> InteractiveBytePatterns {
        let conditional_actions = [
            Action::ScrollLineUp,
            Action::ScrollLineDown,
            Action::ScrollToBottom,
            Action::ScrollToTop,
        ];
        let mut bindings = Vec::new();
        for rb in &self.bindings {
            if !rb.scopes.contains(&BindingScope::Interactive) {
                continue;
            }
            for kc in &rb.keys {
                if let Some(bytes) = key_combination_to_bytes(kc) {
                    bindings.push(InteractiveByteBinding {
                        pattern: bytes,
                        action: rb.action,
                        conditional: conditional_actions.contains(&rb.action),
                    });
                }
            }
        }
        InteractiveBytePatterns { bindings }
    }
}

/// Convert a `KeyCombination` to the raw byte sequence a terminal would send.
/// Returns `None` for key types that can't be represented as bytes (e.g. mouse
/// buttons or function keys beyond F12).
///
/// This is a thin adapter over `key_encode::encode_key`, which owns the one
/// key-to-bytes table; do not grow a second copy of that table here.
pub(crate) fn key_combination_to_bytes(kc: &KeyCombination) -> Option<Vec<u8>> {
    use crokey::OneToThree::One;

    let norm = kc.normalized();
    match norm.codes {
        // app_cursor is false here on purpose: these patterns match bytes the
        // HOST terminal sends to dux, and dux never sets DECCKM on the host,
        // so the host always sends cursor keys in the CSI form. The child's
        // DECCKM state is a property of the child PTY and is irrelevant to
        // what arrives on dux's own stdin.
        One(code) => crate::key_encode::encode_key(code, norm.modifiers, false),
        _ => None,
    }
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
    fn text_field_owns_letters_and_horizontal_arrows_only() {
        // Owned by the field: it types these or moves its caret with them.
        for key in [
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
        ] {
            assert!(text_field_owns_key(key), "{key:?} belongs to the field");
        }
        // Not owned: these still reach the dialog's bindings.
        for key in [
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
        ] {
            assert!(!text_field_owns_key(key), "{key:?} reaches the bindings");
        }
    }

    #[test]
    fn center_typing_owns_typing_keys_and_leaves_the_chords_to_dux() {
        // Typing-owned: these forward into the agent's PTY when the minimized
        // center pane is typeable.
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            // The one deliberate chord exception: Ctrl+c interrupts the agent.
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert!(
                center_typing_owns_key(&key, false),
                "{key:?} types into the agent"
            );
        }
        // Chords belong to dux: these keep resolving through the bindings.
        for key in [
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            // Extra modifiers on Ctrl+c turn it back into a dux-side chord.
            KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ] {
            assert!(
                !center_typing_owns_key(&key, false),
                "{key:?} reaches the bindings"
            );
        }
    }

    /// `tab_reaches_agent` moves exactly Tab and its two shift spellings across
    /// the line, and nothing else: the page keys and the other chords stay dux's
    /// in both settings.
    #[test]
    fn tab_reaches_agent_hands_over_tab_and_shift_tab_only() {
        let tabs = [
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        ];
        for key in tabs {
            assert!(
                !center_typing_owns_key(&key, false),
                "{key:?} moves panes while the option is off"
            );
            assert!(
                center_typing_owns_key(&key, true),
                "{key:?} types into the agent while the option is on"
            );
        }
        for key in [
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
        ] {
            assert!(
                !center_typing_owns_key(&key, true),
                "{key:?} stays a dux chord whatever the option says"
            );
        }
    }

    /// The trap is the pane actions bound to nothing but keys the pane types,
    /// and only while the option is on: with it off, Tab still moves panes, so
    /// the same bindings are perfectly usable.
    #[test]
    fn typeable_center_traps_focus_only_when_every_pane_key_is_typed() {
        let defaults = default_bindings();
        assert!(
            !defaults.typeable_center_traps_focus(true),
            "the default Ctrl-o and Ctrl-y are the way out"
        );

        let tab_only = RuntimeBindings::new(
            |action| match action {
                Action::FocusNext => vec![key!(tab)],
                Action::FocusPrev => vec![key!(shift - tab)],
                _ => BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default(),
            },
            true,
        );
        assert!(
            tab_only.typeable_center_traps_focus(true),
            "with both pane actions on Tab alone, nothing moves focus off the pane"
        );
        assert!(
            !tab_only.typeable_center_traps_focus(false),
            "the same bindings are fine while Tab still moves panes"
        );
    }

    #[test]
    fn label_for_text_field_dialog_skips_the_suppressed_keys() {
        let bindings = default_bindings();
        // The shared default list starts with `h`, which the field types.
        assert_eq!(bindings.label_for(Action::ToggleSelection), "h");
        assert_eq!(
            bindings
                .label_for_text_field_dialog(Action::ToggleSelection)
                .as_deref(),
            Some("Tab"),
            "the hint must name the first key that still reaches the action"
        );

        // Rebound to keys the field owns entirely: no honest label exists.
        let letters_only = RuntimeBindings::new(
            |action| {
                if action == Action::ToggleSelection {
                    vec![key!(h), key!(l)]
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
        assert_eq!(
            letters_only.label_for_text_field_dialog(Action::ToggleSelection),
            None
        );
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
    fn rename_session_available_in_left_and_center() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Left),
            Some(Action::RenameSession)
        );
        assert_eq!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::RenameSession)
        );
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
    fn lookup_ctrl_close_bracket_matches() {
        let bindings = default_bindings();
        // Crossterm delivers Ctrl+] as Char('5') + CONTROL (byte 0x1D maps
        // to '5' in crossterm's parser). The normalize_ctrl_punct layer
        // should remap this so the binding fires.
        let key = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Global),
            Some(Action::RemoveGitPane),
        );
    }

    #[test]
    fn crokey_parses_ctrl_bracket_as_real_char() {
        // Verify that crokey::parse("ctrl-]") produces Char(']') + CONTROL,
        // meaning users can write `ctrl-]` in their config file and it will
        // match after normalize_ctrl_punct remaps the crossterm event.
        let kc = crokey::parse("ctrl-]").unwrap();
        assert_eq!(kc.codes, crokey::OneToThree::One(KeyCode::Char(']')));
        assert!(kc.modifiers.contains(KeyModifiers::CONTROL));
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

    /// The standalone-agent action is reachable from the agents pane and from
    /// inside the project chooser. The defaults are a flat cross product, so
    /// each key fires in both scopes; what matters is that a bare letter works
    /// where a list is being walked and the chord works where a filter is being
    /// typed.
    #[test]
    fn new_standalone_agent_is_bound_in_the_left_pane_and_the_project_chooser() {
        let bindings = default_bindings();
        for key in [
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        ] {
            for scope in [BindingScope::Left, BindingScope::ProjectChooser] {
                assert_eq!(
                    bindings.lookup(&key, scope),
                    Some(Action::NewStandaloneAgent),
                    "{key:?} must reach the standalone agent in {scope:?}"
                );
            }
        }
    }

    /// The chooser gets a scope of its own rather than borrowing `Palette`,
    /// which every other picker resolves through: putting the action there
    /// would arm the key inside every one of them.
    #[test]
    fn the_standalone_key_stays_out_of_the_shared_palette_scope() {
        let bindings = default_bindings();
        for key in [
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        ] {
            assert_ne!(
                bindings.lookup(&key, BindingScope::Palette),
                Some(Action::NewStandaloneAgent),
                "{key:?} must not fire in every Palette-scoped modal"
            );
        }
    }

    /// The two labels the surfaces name: the agents-pane footer takes the first
    /// key, and the chooser's footer, whose filter owns the letters, takes the
    /// first key that filter does NOT swallow.
    #[test]
    fn the_standalone_labels_follow_the_surface_that_names_them() {
        let bindings = default_bindings();
        assert_eq!(bindings.label_for(Action::NewStandaloneAgent), "s");
        assert_eq!(
            bindings.label_for_text_field_dialog(Action::NewStandaloneAgent),
            Some("Ctrl-s".to_string())
        );
    }

    /// A rebinding that leaves only keys the filter types must leave the
    /// chooser's footer with nothing to name, rather than naming a key that
    /// types a character.
    #[test]
    fn a_letters_only_rebinding_leaves_the_chooser_footer_no_label() {
        let bindings = RuntimeBindings::new(
            |action| {
                if action == Action::NewStandaloneAgent {
                    vec![key!(s)]
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
        assert_eq!(
            bindings.label_for_text_field_dialog(Action::NewStandaloneAgent),
            None
        );
    }

    /// The chooser scope is a real scope: diagnostics iterate `ALL`, so a scope
    /// missing from it is invisible to every conflict report.
    #[test]
    fn the_project_chooser_scope_is_listed_and_named() {
        assert!(BindingScope::ALL.contains(&BindingScope::ProjectChooser));
        assert_eq!(
            BindingScope::ProjectChooser.display_name(),
            "Project chooser"
        );
    }

    /// The help overlay is built from the `BindingDef`, independently of the
    /// config template's core `help_section`, so both have to be right.
    #[test]
    fn the_help_overlay_lists_the_standalone_agent_under_the_projects_pane() {
        let bindings = default_bindings();
        let sections = bindings.help_sections();
        let (_, entries) = sections
            .iter()
            .find(|(name, _)| *name == "Projects pane")
            .expect("the Projects pane section exists");
        assert!(
            entries.iter().any(|(label, desc)| label == "s/Ctrl-s"
                && *desc == "Start a standalone agent in a folder you choose"),
            "got {entries:?}"
        );
    }

    /// The chooser's handler asks its own scope and then falls through to
    /// `Palette`, so a key bound in both is shadowed at runtime with nothing
    /// said. The detector has to see that ladder, even though no binding lists
    /// both scopes.
    #[test]
    fn a_rebinding_onto_the_choosers_palette_fallback_is_reported() {
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings
            .insert("new_standalone_agent".to_string(), vec!["/".to_string()]);
        let conflicts = detect_conflicts(&keys);
        let reported: Vec<&KeyConflict> = conflicts
            .iter()
            .filter(|c| {
                (c.action_a == "new_standalone_agent" && c.action_b == "search_toggle")
                    || (c.action_a == "search_toggle" && c.action_b == "new_standalone_agent")
            })
            .collect();
        assert!(
            !reported.is_empty(),
            "the shadowed search key must be reported, got: {conflicts:?}"
        );
        for conflict in reported {
            assert_eq!(conflict.key_label, "/", "got {conflict:?}");
            // The message names the surface the user can actually see, which is
            // the chooser. "Command palette" would send them looking at a modal
            // where nothing is wrong.
            assert_eq!(
                conflict.scope,
                BindingScope::ProjectChooser,
                "got {conflict:?}"
            );
        }
    }

    /// The action's config id is real: a user who writes it under `[keys]` gets
    /// their key instead of the defaults, in both of its scopes.
    #[test]
    fn new_standalone_agent_round_trips_through_the_config_id() {
        let mut keys = crate::config::KeysConfig::default();
        keys.bindings.insert(
            "new_standalone_agent".to_string(),
            vec!["ctrl-y".to_string()],
        );
        let resolved = resolve_keys(&keys);
        let (_, combos, scopes) = resolved
            .iter()
            .find(|(action, _, _)| *action == Action::NewStandaloneAgent)
            .expect("the action must be in the resolved table");
        assert_eq!(combos, &vec![key!(ctrl - y)]);
        assert!(scopes.contains(&BindingScope::Left));
        assert!(scopes.contains(&BindingScope::ProjectChooser));
    }

    #[test]
    fn open_current_pr_key_is_center_pane_only() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);

        assert_eq!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::OpenCurrentPullRequest)
        );
        assert_ne!(
            bindings.lookup(&key, BindingScope::Left),
            Some(Action::OpenCurrentPullRequest)
        );
        assert_ne!(
            bindings.lookup(&key, BindingScope::Files),
            Some(Action::OpenCurrentPullRequest)
        );
        assert_ne!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::OpenCurrentPullRequest)
        );
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

    /// An action that ships with no default key but still carries a help entry
    /// (SelectTab4) must not render a broken row with an empty key badge:
    /// `help_sections` skips label-less entries.
    #[test]
    fn help_sections_skips_unbound_actions_instead_of_rendering_empty_keys() {
        let bindings = default_bindings();
        assert_eq!(
            bindings.labels_for(Action::SelectTab4),
            "",
            "fixture: select_tab_4 ships unbound (Ctrl-4 is Ctrl-\\ under the legacy protocol)"
        );
        for (section, entries) in bindings.help_sections() {
            for (label, desc) in entries {
                assert!(
                    !label.is_empty(),
                    "help section {section:?} rendered an empty key label for {desc:?}"
                );
            }
        }
    }

    fn palette_names<'a>(bindings: &[&'a RuntimeBinding]) -> Vec<&'a str> {
        bindings
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect()
    }

    #[test]
    fn palette_matches_puts_a_reordered_query_in_the_second_tier() {
        let bindings = default_bindings();
        let matches = bindings.palette_matches("new tab");
        assert!(
            matches.direct.is_empty(),
            "`new tab` is not a contiguous phrase anywhere: {:?}",
            palette_names(&matches.direct)
        );
        assert_eq!(palette_names(&matches.other), vec!["new-agent-tab"]);
    }

    #[test]
    fn palette_matches_never_repeats_a_direct_hit_in_the_second_tier() {
        let bindings = default_bindings();
        let matches = bindings.palette_matches("sort agents");
        assert!(
            palette_names(&matches.direct).contains(&"sort-agents"),
            "the phrase tier must still own this one: {:?}",
            palette_names(&matches.direct)
        );
        assert!(
            !palette_names(&matches.other).contains(&"sort-agents"),
            "a direct hit must not be repeated in the second tier"
        );
    }

    #[test]
    fn palette_matches_has_no_second_tier_for_an_empty_or_single_word_query() {
        let bindings = default_bindings();
        let empty = bindings.palette_matches("");
        assert!(!empty.direct.is_empty());
        assert!(
            empty.other.is_empty(),
            "an empty query lists everything once"
        );

        // A single token that already appears as a substring is never
        // demoted: the phrase tier owns it and the second tier stays empty.
        for query in ["agent", "wor", "pr", "toggle"] {
            let matches = bindings.palette_matches(query);
            assert!(
                matches.other.is_empty(),
                "single-token query {query:?} produced a second tier: {:?}",
                palette_names(&matches.other)
            );
        }
    }

    #[test]
    fn a_single_word_reaches_the_second_tier_through_an_apostrophe() {
        // The two tiers tokenize differently and that gap is deliberate: the
        // scorer drops apostrophes, so `agents` finds the descriptions that
        // say `agent's`, which the phrase tier cannot see.
        let bindings = default_bindings();
        let matches = bindings.palette_matches("agents");
        assert!(
            palette_names(&matches.direct).contains(&"sort-agents"),
            "the plural is a substring of several names, so the phrase tier \
             still leads: {:?}",
            palette_names(&matches.direct)
        );
        assert_eq!(
            palette_names(&matches.other),
            vec![
                "rerun-startup-command-on-agent",
                "copy-path",
                "open-worktree",
                "open-worktree-with",
                "show-terminal",
                "open-current-pr",
                "detach-pull-request",
                "resume-pull-request-autodetection",
                "new-terminal-for-agent",
                "agent-info",
                "refresh-changes",
            ]
        );
    }

    #[test]
    fn filtered_palette_is_both_tiers_flattened_in_order() {
        let bindings = default_bindings();
        let matches = bindings.palette_matches("agent tab");
        let mut expected = palette_names(&matches.direct);
        expected.extend(palette_names(&matches.other));
        assert_eq!(
            palette_names(&bindings.filtered_palette("agent tab")),
            expected
        );
        assert!(
            !matches.direct.is_empty() && !matches.other.is_empty(),
            "fixture must exercise both tiers"
        );
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
        // A filter that matched nothing would satisfy the "every row matches"
        // check below, so pin what the query must FIND before checking that it
        // found nothing else.
        let names: Vec<&str> = results.iter().filter_map(|b| b.palette_name).collect();
        assert!(
            names.contains(&"toggle-sidebar") && names.contains(&"toggle-git-pane"),
            "the toggle commands must be among the matches: {names:?}"
        );
        assert!(results.iter().all(|b| {
            b.palette_name.unwrap().contains("toggle")
                || b.palette_description
                    .unwrap()
                    .to_lowercase()
                    .contains("toggle")
        }));
    }

    #[test]
    fn normalize_palette_match_collapses_separators() {
        assert_eq!(normalize_palette_match(""), "");
        assert_eq!(normalize_palette_match("   "), "");
        assert_eq!(normalize_palette_match("---"), "");
        assert_eq!(
            normalize_palette_match("open-current-pr"),
            "open-current-pr"
        );
        assert_eq!(
            normalize_palette_match("open current pr"),
            "open-current-pr"
        );
        assert_eq!(
            normalize_palette_match("  open   current\tpr  "),
            "open-current-pr"
        );
        assert_eq!(normalize_palette_match("Fork Agent"), "fork-agent");
        // Mixed dashes and spaces collapse to a single separator each.
        assert_eq!(normalize_palette_match("a - b -- c"), "a-b-c");
    }

    #[test]
    fn filtered_palette_treats_spaces_as_dashes_in_query() {
        // Users often type natural-language queries with spaces (e.g. "open current pr")
        // instead of typing the dashed command name verbatim. The palette should still
        // match the underlying command name "open-current-pr".
        let bindings = default_bindings();

        let results = bindings.filtered_palette("open current pr");
        let names: Vec<&str> = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect();
        assert!(
            names.contains(&"open-current-pr"),
            "expected 'open-current-pr' in results, got {names:?}"
        );

        let results = bindings.filtered_palette("change agent provider");
        let names: Vec<&str> = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect();
        assert!(
            names.contains(&"change-agent-provider"),
            "expected 'change-agent-provider' in results, got {names:?}"
        );

        // Multiple/leading/trailing whitespace should not break matching.
        let results = bindings.filtered_palette("  fork   agent  ");
        let names: Vec<&str> = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect();
        assert!(
            names.contains(&"fork-agent"),
            "expected 'fork-agent' in results, got {names:?}"
        );

        // Existing dashed-input behavior must still work.
        let results = bindings.filtered_palette("open-current-pr");
        let names: Vec<&str> = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect();
        assert!(
            names.contains(&"open-current-pr"),
            "expected dashed query to still match, got {names:?}"
        );
    }

    #[test]
    fn filtered_palette_includes_companion_terminal_commands() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("terminal");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"show-terminal"));
    }

    #[test]
    fn filtered_palette_includes_fork_agent_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("fork");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"fork-agent"));
    }

    #[test]
    fn filtered_palette_includes_new_agent_from_pr_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("pr");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"new-agent-from-pr"));
    }

    #[test]
    fn filtered_palette_includes_change_agent_provider_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("provider");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"change-agent-provider"));
        assert!(names.contains(&"change-default-provider"));
        assert!(names.contains(&"change-project-default-provider"));
    }

    #[test]
    fn provider_palette_descriptions_clarify_scope() {
        let bindings = default_bindings();
        let global = bindings
            .filtered_palette("change-default-provider")
            .into_iter()
            .find(|binding| binding.palette_name == Some("change-default-provider"))
            .expect("global provider palette entry");
        assert_eq!(
            global.palette_description,
            Some(
                "Change the global default provider for new agents in projects without a project-specific override"
            )
        );

        let project = bindings
            .filtered_palette("change-project-default-provider")
            .into_iter()
            .find(|binding| binding.palette_name == Some("change-project-default-provider"))
            .expect("project provider palette entry");
        assert_eq!(
            project.palette_description,
            Some(
                "Change the selected project's default provider for future agents in that project only"
            )
        );
    }

    #[test]
    fn filtered_palette_includes_kill_running_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("kill");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"kill-running"));
    }

    #[test]
    fn filtered_palette_includes_resource_monitor_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("resource");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"resource-monitor"));
    }

    #[test]
    fn filtered_palette_includes_reload_config_command() {
        let bindings = default_bindings();
        let results = bindings.filtered_palette("reload");
        let names = results
            .iter()
            .filter_map(|binding| binding.palette_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"reload-config"));
    }

    #[test]
    fn resource_monitor_config_name_round_trip() {
        assert_eq!(Action::ResourceMonitor.config_name(), "resource_monitor");
    }

    // EXHAUSTIVENESS PIN: the core registry
    // (`dux_core::palette::PALETTE_COMMANDS`) is the single source of truth for
    // palette command names and descriptions. Every core command must:
    //   1. join to a BINDING_DEFS entry by Action (so the TUI can attach
    //      keybindings and dispatch), and
    //   2. surface in the runtime palette listing with byte-identical name and
    //      description.
    // Conversely, the runtime palette listing must contain exactly the core
    // commands, no more and no less. This makes name/description parity true by
    // construction: adding or renaming a palette command in the core registry
    // without a matching action fails this gate.
    #[test]
    fn palette_listing_matches_core_registry() {
        use dux_core::palette;

        let bindings = default_bindings();
        let listed: std::collections::HashMap<&str, &str> = bindings
            .bindings
            .iter()
            .filter_map(|b| Some((b.palette_name?, b.palette_description?)))
            .collect();

        for cmd in palette::PALETTE_COMMANDS {
            assert!(
                BINDING_DEFS.iter().any(|d| d.action == cmd.action),
                "core palette command \"{}\" has no BINDING_DEFS entry to join on",
                cmd.name
            );
            let desc = listed.get(cmd.name).unwrap_or_else(|| {
                panic!(
                    "core palette command \"{}\" is missing from the runtime \
                     palette listing",
                    cmd.name
                )
            });
            assert_eq!(
                *desc, cmd.description,
                "palette description drift for \"{}\"",
                cmd.name
            );
        }

        // The listing contains nothing the core registry didn't surface.
        assert_eq!(
            listed.len(),
            palette::PALETTE_COMMANDS.len(),
            "the runtime palette listing has entries not present in the core \
             registry"
        );
    }

    /// The three actions whose ONLY palette row was a web-palette row keep their
    /// `BindingDef` but lose the row. Both halves matter:
    ///
    /// - No registry row: the web has no palette to surface them in, and the TUI
    ///   never listed them (their surface was `Web`), so a row would be dead
    ///   weight that the TUI palette would now wrongly list.
    /// - Keeps its `BindingDef`: `config.rs::validate_keys` rejects any `[keys]`
    ///   action name absent from `BINDING_DEFS`, so removing these would turn an
    ///   existing user's `edit_config = ["ctrl-e"]` into a hard config error.
    ///   They stay inert exactly as they are today (no keys, no scopes, no help).
    #[test]
    fn web_only_palette_rows_are_gone_but_their_actions_remain_bindable() {
        for action in [
            Action::EditConfig,
            Action::RenameWebInstance,
            Action::ToggleCopyOnSelect,
        ] {
            assert!(
                dux_core::palette::find_by_action(action).is_none(),
                "{action:?} should have no palette registry row"
            );
            assert!(
                BINDING_DEFS.iter().any(|d| d.action == action),
                "{action:?} must keep its BINDING_DEFS entry or config.rs's \
                 validate_keys would reject a user config that binds it"
            );
        }
    }

    /// THE "TUI PALETTE IS UNCHANGED" PIN.
    ///
    /// The exact set of commands the `Ctrl-p` palette lists, captured from the
    /// registry as it stood BEFORE `PaletteSurface` was removed (when the
    /// listing was `find_by_action(..).filter(|c| c.surface.in_tui())`). The
    /// surface collapse dropped only the three Web-only rows, which `in_tui()`
    /// already filtered out of this listing, so this list had to survive the
    /// refactor byte-for-byte.
    ///
    /// This is a deliberate hand-maintained list, NOT derived from
    /// `PALETTE_COMMANDS`: a derived list would tautologically agree with any
    /// registry edit and prove nothing. Adding or removing a TUI palette
    /// command is a real user-facing change: update this list in the same
    /// commit, on purpose.
    #[test]
    fn tui_palette_listing_is_the_expected_command_set() {
        let bindings = default_bindings();
        let mut listed: Vec<&str> = bindings
            .bindings
            .iter()
            .filter_map(|b| b.palette_name)
            .collect();
        listed.sort_unstable();

        let expected = [
            "add-project",
            "agent-info",
            "attach-pull-request",
            "change-agent-provider",
            "change-default-provider",
            "change-project-default-provider",
            "change-theme",
            "checkout-project-default-branch",
            "close-tab",
            "configure-global-env",
            "configure-project-env",
            "configure-startup-command",
            "copy-path",
            "delete-agent",
            "delete-project",
            "delete-terminal",
            "detach-pull-request",
            "edit-macros",
            "filter-agents",
            "force-reconnect-agent",
            "force-redraw",
            "fork-agent",
            "help",
            "input-debugging",
            "kill-running",
            "manage-projects",
            "manage-worktrees",
            "move-agent-bottom",
            "move-agent-down",
            "move-agent-top",
            "move-agent-up",
            "move-terminal-bottom",
            "move-terminal-down",
            "move-terminal-top",
            "move-terminal-up",
            "new-agent",
            "new-agent-from-pr",
            "new-agent-from-worktree",
            "new-agent-tab",
            "new-standalone-agent",
            "new-standalone-terminal",
            "new-terminal-for-agent",
            "new-terminal-for-project",
            "open-current-pr",
            "open-worktree",
            "open-worktree-with",
            "pull-project",
            "read-startup-command-logs",
            "recheck-github",
            "reconnect-agent",
            "refresh-changes",
            "reload-config",
            "remove-project",
            "rename-agent",
            "rerun-startup-command-on-agent",
            "resource-monitor",
            "resume-pull-request-autodetection",
            "set-tailscale-mode",
            "show-agent",
            "show-release-notes",
            "show-terminal",
            "show-welcome-screen",
            "sort-agents",
            "start-background-server",
            "start-web-server",
            "stop-background-server",
            "toggle-agent-auto-reopen",
            "toggle-always-show-tabs",
            "toggle-diff-line-numbers",
            "toggle-git-pane",
            "toggle-github-integration",
            "toggle-pr-banner-position",
            "toggle-project",
            "toggle-project-auto-reopen-agents",
            "toggle-randomized-pet-name-default",
            "toggle-remove-git-pane",
            "toggle-sidebar",
            "toggle-tab-to-agent",
        ];

        assert_eq!(
            listed, expected,
            "the TUI command palette's listing changed. The web app menu is a \
             SEPARATE surface (crates/dux-web/web/src/lib/appMenu.ts) and must \
             never move this list. If this change is a deliberate TUI palette \
             addition/removal, update `expected` here in the same commit."
        );
    }

    #[test]
    fn left_scope_resolves_t_to_show_terminal() {
        let bindings = default_bindings();
        let t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&t, BindingScope::Left),
            Some(Action::ShowTerminal)
        );
    }

    #[test]
    fn left_scope_resolves_f_to_fork_agent() {
        let bindings = default_bindings();
        let f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&f, BindingScope::Left),
            Some(Action::ForkAgent)
        );
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
    fn start_web_server_is_palette_only_with_no_binding() {
        let def = BINDING_DEFS
            .iter()
            .find(|d| d.action == Action::StartWebServer)
            .expect("StartWebServer must be registered in BINDING_DEFS");

        // It is a palette command with a name in the core registry…
        let palette = dux_core::palette::find_by_action(Action::StartWebServer)
            .expect("StartWebServer must expose a core palette entry");
        assert_eq!(palette.name, "start-web-server");
        assert!(!palette.description.is_empty());

        // …but has no default key binding (palette-only) and no help section.
        assert!(
            def.default_keys.is_empty(),
            "StartWebServer must have no default keybinding"
        );
        assert!(def.scopes.is_empty());
        assert!(def.help.is_none());
        assert_eq!(Action::StartWebServer.config_name(), "start_web_server");

        // A default RuntimeBindings resolves no key to it in any scope.
        let bindings = default_bindings();
        assert!(
            bindings.label_for(Action::StartWebServer).is_empty(),
            "no key should be bound to StartWebServer by default"
        );

        // It shows up in the command palette listing.
        assert!(
            bindings
                .filtered_palette("")
                .iter()
                .any(|b| b.palette_name == Some("start-web-server")),
            "StartWebServer must appear in the command palette"
        );
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
    fn pane_chords_move_focus_in_the_global_scope() {
        let bindings = default_bindings();
        for (key, action) in [
            (
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
                Action::FocusNext,
            ),
            (
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
                Action::FocusPrev,
            ),
        ] {
            assert_eq!(
                bindings.lookup(&key, BindingScope::Global),
                Some(action),
                "{key:?} must move panes"
            );
        }
    }

    /// The legacy keyboard protocol has no Ctrl-i: the byte it sends is Tab.
    /// A default bound to a chord spelling of that byte never fires and shadows
    /// whatever Tab is bound to, so every default that encodes to `0x09` must
    /// be spelled `tab`.
    #[test]
    fn every_default_encoding_to_tabs_byte_is_spelled_tab() {
        assert_eq!(
            key_combination_to_bytes(&key!(ctrl - i)),
            Some(vec![0x09]),
            "Ctrl-i is Tab on the wire"
        );
        for def in BINDING_DEFS {
            for key in def.default_keys {
                if key_combination_to_bytes(key) == Some(vec![0x09]) {
                    assert!(
                        matches!(key.codes, crokey::OneToThree::One(KeyCode::Tab)),
                        "{:?} defaults to {}, which the terminal delivers as Tab",
                        def.action,
                        format_key_for_config(*key)
                    );
                }
            }
        }
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
    fn lookup_shift_tab_matches_dialog_toggle_selection() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Dialog),
            Some(Action::ToggleSelection),
            "shift-tab (BackTab) should match ToggleSelection in dialogs"
        );
    }

    #[test]
    fn new_actions_are_in_binding_defs() {
        let actions_in_defs: Vec<Action> = BINDING_DEFS.iter().map(|d| d.action).collect();
        assert!(actions_in_defs.contains(&Action::CloseOverlay));
        assert!(actions_in_defs.contains(&Action::ResizeGrow));
        assert!(actions_in_defs.contains(&Action::StageUnstage));
        assert!(actions_in_defs.contains(&Action::ExitCommitInput));
        assert!(actions_in_defs.contains(&Action::PushToRemote));
        assert!(actions_in_defs.contains(&Action::AddCurrentDir));
        assert!(actions_in_defs.contains(&Action::ExitPathEditorOnProjectAdd));
        assert!(actions_in_defs.contains(&Action::SearchFiles));
        assert!(actions_in_defs.contains(&Action::SearchNext));
        assert!(actions_in_defs.contains(&Action::ForceRedraw));
    }

    #[test]
    fn files_scope_resolves_slash_to_search_toggle() {
        let bindings = default_bindings();
        let slash = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&slash, BindingScope::Files),
            Some(Action::SearchFiles)
        );
    }

    #[test]
    fn files_scope_resolves_n_to_search_next() {
        let bindings = default_bindings();
        let n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&n, BindingScope::Files),
            Some(Action::SearchNext)
        );
    }

    #[test]
    fn every_keyed_binding_has_help_entry() {
        // Every BindingDef that has keys assigned should have a help entry,
        // so it appears in the help overlay. MoveUp is the sole exception
        // because it's shown via MoveDown's combined "j/k" label.
        for def in BINDING_DEFS {
            // SelectTab2..9 are shown via SelectTab1's combined "Focus tab 1-9"
            // label, mirroring how MoveUp rides MoveDown's "j/k" label.
            if def.default_keys.is_empty()
                || def.action == Action::MoveUp
                || matches!(
                    def.action,
                    Action::SelectTab2
                        | Action::SelectTab3
                        | Action::SelectTab4
                        | Action::SelectTab5
                        | Action::SelectTab6
                        | Action::SelectTab7
                        | Action::SelectTab8
                        | Action::SelectTab9
                )
            {
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
        // A plain 'p' binding should NOT match Shift+p.
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
            "plain 'p' binding should not match Shift+p"
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

    #[test]
    fn help_scope_resolves_scroll_keys() {
        let bindings = default_bindings();
        // j/k/Up/Down → MoveDown/MoveUp
        let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&j, BindingScope::Help),
            Some(Action::MoveDown)
        );
        assert_eq!(
            bindings.lookup(&k, BindingScope::Help),
            Some(Action::MoveUp)
        );
        assert_eq!(
            bindings.lookup(&down, BindingScope::Help),
            Some(Action::MoveDown)
        );
        assert_eq!(
            bindings.lookup(&up, BindingScope::Help),
            Some(Action::MoveUp)
        );

        // PgUp/PgDn → ScrollPageUp/ScrollPageDown
        let pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
        let pgdn = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&pgup, BindingScope::Help),
            Some(Action::ScrollPageUp)
        );
        assert_eq!(
            bindings.lookup(&pgdn, BindingScope::Help),
            Some(Action::ScrollPageDown)
        );
    }

    #[test]
    fn help_scope_rejects_unrelated_actions() {
        let bindings = default_bindings();
        // 'n' is NewAgent in Left scope, should not resolve in Help
        let n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(bindings.lookup(&n, BindingScope::Help), None);
    }

    // ── key_combination_to_bytes tests ─────────────────────────────────

    #[test]
    fn bytes_ctrl_letter() {
        let kc = key!(ctrl - g);
        assert_eq!(key_combination_to_bytes(&kc), Some(vec![0x07]));
        let kc_a = key!(ctrl - a);
        assert_eq!(key_combination_to_bytes(&kc_a), Some(vec![0x01]));
    }

    #[test]
    fn bytes_plain_char() {
        let kc = key!(space);
        assert_eq!(key_combination_to_bytes(&kc), Some(vec![0x20]));
    }

    #[test]
    fn bytes_page_up_down() {
        let kc_up = key!(pageup);
        assert_eq!(
            key_combination_to_bytes(&kc_up),
            Some(vec![0x1b, b'[', b'5', b'~'])
        );
        let kc_down = key!(pagedown);
        assert_eq!(
            key_combination_to_bytes(&kc_down),
            Some(vec![0x1b, b'[', b'6', b'~'])
        );
    }

    #[test]
    fn bytes_arrow_keys() {
        assert_eq!(
            key_combination_to_bytes(&key!(up)),
            Some(vec![0x1b, b'[', b'A'])
        );
        assert_eq!(
            key_combination_to_bytes(&key!(down)),
            Some(vec![0x1b, b'[', b'B'])
        );
    }

    #[test]
    fn bytes_enter_backspace() {
        assert_eq!(key_combination_to_bytes(&key!(enter)), Some(vec![0x0d]));
        assert_eq!(key_combination_to_bytes(&key!(backspace)), Some(vec![0x7f]));
    }

    #[test]
    fn interactive_byte_patterns_matches_defaults() {
        let bindings = default_bindings();
        let patterns = bindings.interactive_byte_patterns();
        // ToggleFullscreen default is Ctrl-g → 0x07.
        let result = patterns.match_sequence(&[0x07]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, Action::ToggleFullscreen);
        assert!(!result.unwrap().1); // not conditional
    }

    #[test]
    fn interactive_byte_patterns_scroll_line_is_conditional() {
        let bindings = default_bindings();
        let patterns = bindings.interactive_byte_patterns();
        // ScrollLineUp default is Up → ESC [ A
        let result = patterns.match_sequence(&[0x1b, b'[', b'A']);
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, Action::ScrollLineUp);
        assert!(result.unwrap().1); // conditional
    }

    #[test]
    fn interactive_byte_patterns_no_match() {
        let bindings = default_bindings();
        let patterns = bindings.interactive_byte_patterns();
        // Random byte sequence should not match
        assert!(patterns.match_sequence(&[0x01]).is_none());
    }

    #[test]
    fn empty_keys_config_resolves_all_default_bindings() {
        // KeysConfig::default() carries no bindings;
        // RuntimeBindings must still resolve defaults from BINDING_DEFS so runtime
        // keybindings are unchanged.
        let rb = RuntimeBindings::from_keys_config(&dux_core::config::KeysConfig::default());
        assert!(!rb.label_for(Action::OpenPalette).is_empty());
        assert!(!rb.label_for(Action::Quit).is_empty());
    }
}
