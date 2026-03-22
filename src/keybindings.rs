use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Unique identifier for every bindable action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveDown,
    MoveUp,
    ToggleProject,
    NewAgent,
    FocusAgent,
    OpenProjectBrowser,
    CopyPath,
    CycleProvider,
    RefreshProject,
    ReconnectAgent,
    DeleteSession,
    InteractAgent,
    ScrollPageUp,
    ScrollPageDown,
    OpenDiff,
    FocusNext,
    FocusPrev,
    OpenPalette,
    ToggleResizeMode,
    ToggleSidebar,
    ToggleHelp,
    Quit,
    DeleteProject,
    RemoveProject,
    // ── Commit input (native / informational) ─────────────────────
    GenerateCommitMessage,
    InsertNewline,
    CommitChanges,
}

/// Where a binding's key combo is matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingScope {
    Global,
    Left,
    Center,
    Files,
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

/// A physical key combination to match against.
#[derive(Clone, Copy, Debug)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub const fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
        }
    }
    pub const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
        }
    }
    pub const fn key(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
        }
    }
}

pub struct HelpEntry {
    pub section: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub struct PaletteEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub shortcut: Option<&'static str>,
}

pub struct Binding {
    pub action: Action,
    pub keys: &'static [KeyCombo],
    pub scopes: &'static [BindingScope],
    pub help: Option<HelpEntry>,
    pub hints: &'static [(HintContext, &'static str, &'static str)],
    pub palette: Option<PaletteEntry>,
    /// When `true`, the key combo is handled natively by the input handler
    /// (not via the `lookup()` dispatch). The binding exists only for
    /// documentation, hints, and the help overlay.
    pub native: bool,
}

impl Binding {
    /// Check whether a key event matches this binding.
    /// Plain bindings (no modifiers) reject Ctrl/Alt combos so that e.g.
    /// Ctrl+D does not accidentally match a plain `d` binding.
    pub fn matches(&self, key: &KeyEvent) -> bool {
        self.keys.iter().any(|k| {
            if k.code != key.code {
                return false;
            }
            if k.modifiers.is_empty() {
                !key.modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            } else {
                key.modifiers.contains(k.modifiers)
            }
        })
    }
}

// Declaration order within each scope determines:
//   - hint display order in the status bar cheatsheet
//   - help entry order within each help section
//
// Pane bindings come first so their hints appear before global hints
// (^P, ?) which are appended at the end of every context.
pub const BINDINGS: &[Binding] = &[
    // ── Left / Files pane ──────────────────────────────────────────
    Binding {
        action: Action::MoveDown,
        keys: &[KeyCombo::char('j'), KeyCombo::key(KeyCode::Down)],
        scopes: &[BindingScope::Left, BindingScope::Files],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "j/k",
            description: "Move through projects and sessions",
        }),
        hints: &[
            (HintContext::LeftProject, "j/k", "Move"),
            (HintContext::LeftSession, "j/k", "Move"),
            (HintContext::Files, "j/k", "Move"),
        ],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::MoveUp,
        keys: &[KeyCombo::char('k'), KeyCombo::key(KeyCode::Up)],
        scopes: &[BindingScope::Left, BindingScope::Files],
        help: None, // covered by MoveDown's "j/k" label
        hints: &[],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::ToggleProject,
        keys: &[KeyCombo::char(' ')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "Space",
            description: "Collapse/expand project",
        }),
        hints: &[(HintContext::LeftProject, "Space", "Toggle")],
        palette: Some(PaletteEntry {
            name: "toggle-project",
            description: "Collapse or expand the selected project's agents",
            shortcut: Some("Space"),
        }),
        native: false,
    },
    Binding {
        action: Action::NewAgent,
        keys: &[KeyCombo::char('n')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "n",
            description: "New agent session (creates worktree)",
        }),
        hints: &[(HintContext::LeftProject, "n", "New agent")],
        palette: Some(PaletteEntry {
            name: "new-agent",
            description: "Create a new agent for the selected project",
            shortcut: Some("n"),
        }),
        native: false,
    },
    Binding {
        action: Action::FocusAgent,
        keys: &[KeyCombo::key(KeyCode::Enter)],
        scopes: &[BindingScope::Left],
        help: None,
        hints: &[(HintContext::LeftSession, "Enter", "Focus")],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::OpenProjectBrowser,
        keys: &[KeyCombo::char('a')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "a",
            description: "Open project browser",
        }),
        hints: &[
            (HintContext::LeftProject, "a", "Add project"),
            (HintContext::LeftSession, "a", "Add project"),
        ],
        palette: Some(PaletteEntry {
            name: "add-project",
            description: "Open the project browser",
            shortcut: Some("a"),
        }),
        native: false,
    },
    Binding {
        action: Action::CopyPath,
        keys: &[KeyCombo::char('y')],
        scopes: &[BindingScope::Left],
        help: None,
        hints: &[
            (HintContext::LeftProject, "y", "Copy path"),
            (HintContext::LeftSession, "y", "Copy path"),
        ],
        palette: Some(PaletteEntry {
            name: "copy-path",
            description: "Copy the selected agent's worktree path",
            shortcut: Some("y"),
        }),
        native: false,
    },
    Binding {
        action: Action::CycleProvider,
        keys: &[KeyCombo::char('d')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "d",
            description: "Cycle default provider",
        }),
        hints: &[(HintContext::LeftProject, "d", "Provider")],
        palette: Some(PaletteEntry {
            name: "provider",
            description: "Toggle the selected project's default provider",
            shortcut: Some("d"),
        }),
        native: false,
    },
    Binding {
        action: Action::RefreshProject,
        keys: &[KeyCombo::char('u')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "u",
            description: "Refresh checkout (git pull --ff-only)",
        }),
        hints: &[(HintContext::LeftProject, "u", "Pull")],
        palette: Some(PaletteEntry {
            name: "refresh-project",
            description: "Git pull the selected project checkout",
            shortcut: Some("u"),
        }),
        native: false,
    },
    Binding {
        action: Action::ReconnectAgent,
        keys: &[KeyCombo::char('r'), KeyCombo::key(KeyCode::Enter)],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "r",
            description: "Restart agent CLI",
        }),
        hints: &[(HintContext::LeftSession, "r", "Reconnect")],
        palette: Some(PaletteEntry {
            name: "reconnect-agent",
            description: "Restart the CLI for the selected agent",
            shortcut: None,
        }),
        native: false,
    },
    Binding {
        action: Action::DeleteSession,
        keys: &[KeyCombo::ctrl('d')],
        scopes: &[BindingScope::Left],
        help: Some(HelpEntry {
            section: "Projects pane",
            label: "^D",
            description: "Delete selected session/worktree",
        }),
        hints: &[(HintContext::LeftSession, "^D", "Delete")],
        palette: Some(PaletteEntry {
            name: "delete-agent",
            description: "Delete the selected agent session",
            shortcut: None,
        }),
        native: false,
    },
    // ── Center pane ────────────────────────────────────────────────
    Binding {
        action: Action::InteractAgent,
        keys: &[KeyCombo::char('i')],
        scopes: &[BindingScope::Left, BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            label: "i",
            description: "Start a prompt turn for the agent",
        }),
        hints: &[(HintContext::Center, "i", "Interact")],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::ScrollPageUp,
        keys: &[KeyCombo::ctrl('b'), KeyCombo::key(KeyCode::PageUp)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            label: "^B/PgUp",
            description: "Scroll up one page",
        }),
        hints: &[],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::ScrollPageDown,
        keys: &[KeyCombo::ctrl('f'), KeyCombo::key(KeyCode::PageDown)],
        scopes: &[BindingScope::Center],
        help: Some(HelpEntry {
            section: "Agent pane",
            label: "^F/PgDn",
            description: "Scroll down one page",
        }),
        hints: &[],
        palette: None,
        native: false,
    },
    // ── Files pane ─────────────────────────────────────────────────
    Binding {
        action: Action::OpenDiff,
        keys: &[KeyCombo::key(KeyCode::Enter)],
        scopes: &[BindingScope::Files],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "Enter",
            description: "Open selected file diff",
        }),
        hints: &[(HintContext::Files, "Enter", "Diff")],
        palette: None,
        native: false,
    },
    // ── Global ─────────────────────────────────────────────────────
    // (placed after pane bindings so ^P / ? appear last in hints)
    Binding {
        action: Action::FocusNext,
        keys: &[KeyCombo::key(KeyCode::Tab)],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "Tab",
            description: "Focus next pane",
        }),
        hints: &[
            (HintContext::Center, "Tab", "Next"),
            (HintContext::Files, "Tab", "Next"),
        ],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::FocusPrev,
        keys: &[KeyCombo::key(KeyCode::BackTab)],
        scopes: &[BindingScope::Global],
        help: None,
        hints: &[],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::OpenPalette,
        keys: &[KeyCombo::ctrl('p')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "^P",
            description: "Open command palette",
        }),
        hints: &[
            (HintContext::LeftProject, "^P", "Palette"),
            (HintContext::LeftSession, "^P", "Palette"),
            (HintContext::Center, "^P", "Palette"),
            (HintContext::Files, "^P", "Palette"),
        ],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::ToggleResizeMode,
        keys: &[KeyCombo::ctrl('w')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "^W",
            description: "Resize mode (h/l side panes)",
        }),
        hints: &[],
        palette: None,
        native: false,
    },
    Binding {
        action: Action::ToggleSidebar,
        keys: &[KeyCombo::char('[')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "[",
            description: "Toggle sidebar",
        }),
        hints: &[],
        palette: Some(PaletteEntry {
            name: "toggle-sidebar",
            description: "Collapse or expand the projects sidebar",
            shortcut: Some("["),
        }),
        native: false,
    },
    Binding {
        action: Action::ToggleHelp,
        keys: &[KeyCombo::char('?')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "?",
            description: "Toggle help",
        }),
        hints: &[
            (HintContext::LeftProject, "?", "Help"),
            (HintContext::LeftSession, "?", "Help"),
            (HintContext::Center, "?", "Help"),
            (HintContext::Files, "?", "Help"),
        ],
        palette: Some(PaletteEntry {
            name: "help",
            description: "Open the help overlay",
            shortcut: Some("?"),
        }),
        native: false,
    },
    Binding {
        action: Action::Quit,
        keys: &[KeyCombo::char('q'), KeyCombo::ctrl('c')],
        scopes: &[BindingScope::Global],
        help: Some(HelpEntry {
            section: "Global",
            label: "q",
            description: "Quit",
        }),
        hints: &[],
        palette: None,
        native: false,
    },
    // ── Palette-only (no direct keybinding) ────────────────────────
    Binding {
        action: Action::DeleteProject,
        keys: &[],
        scopes: &[],
        help: None,
        hints: &[],
        palette: Some(PaletteEntry {
            name: "delete-project",
            description: "Remove the selected project and its sessions",
            shortcut: None,
        }),
        native: false,
    },
    // ── Files pane (native — handled directly in handle_files_key) ──────
    Binding {
        action: Action::ToggleProject, // reused: Space in files = stage/unstage
        keys: &[KeyCombo::char(' ')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "Space",
            description: "Stage or unstage selected file",
        }),
        hints: &[(HintContext::Files, "Space", "Stage/Unstage")],
        palette: None,
        native: true,
    },
    Binding {
        action: Action::CommitChanges,
        keys: &[KeyCombo::char('c')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "c",
            description: "Commit staged changes",
        }),
        hints: &[(HintContext::Files, "c", "Commit")],
        palette: None,
        native: true,
    },
    Binding {
        action: Action::GenerateCommitMessage,
        keys: &[KeyCombo::ctrl('g')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "^G",
            description: "Generate AI commit message",
        }),
        hints: &[(HintContext::Files, "^G", "AI msg")],
        palette: None,
        native: true,
    },
    Binding {
        action: Action::DeleteSession, // reused: ^D in files = discard
        keys: &[KeyCombo::ctrl('d')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "^D",
            description: "Discard changes to selected file",
        }),
        hints: &[(HintContext::Files, "^D", "Discard")],
        palette: None,
        native: true,
    },
    Binding {
        action: Action::InteractAgent, // reused: i in files = engage commit input
        keys: &[KeyCombo::char('i')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Files pane",
            label: "i",
            description: "Write a commit message",
        }),
        hints: &[(HintContext::Files, "i", "Commit msg")],
        palette: None,
        native: true,
    },
    // ── Commit input (native keybindings — handled directly, not via lookup) ──
    Binding {
        action: Action::GenerateCommitMessage, // ^G exits commit input
        keys: &[KeyCombo::ctrl('g')],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Commit input",
            label: "^G",
            description: "Exit commit input",
        }),
        hints: &[], // works but not shown in hint bar
        palette: None,
        native: true,
    },
    Binding {
        action: Action::InsertNewline,
        keys: &[KeyCombo::key(KeyCode::Esc)],
        scopes: &[],
        help: Some(HelpEntry {
            section: "Commit input",
            label: "Esc",
            description: "Exit commit input",
        }),
        hints: &[(HintContext::CommitInput, "Esc", "Exit")],
        palette: None,
        native: true,
    },
    Binding {
        action: Action::RemoveProject,
        keys: &[],
        scopes: &[],
        help: None,
        hints: &[],
        palette: Some(PaletteEntry {
            name: "remove-project",
            description: "Remove project from app (keeps files on disk)",
            shortcut: None,
        }),
        native: false,
    },
];

const HELP_SECTION_ORDER: &[&str] = &["Global", "Projects pane", "Agent pane", "Files pane", "Commit input"];

/// Find the action for a key event in the given scope.
pub fn lookup(key: &KeyEvent, scope: BindingScope) -> Option<Action> {
    BINDINGS
        .iter()
        .filter(|b| b.scopes.contains(&scope))
        .find(|b| b.matches(key))
        .map(|b| b.action)
}

/// Status-bar hints for a given context, in display order.
pub fn hints_for(ctx: HintContext) -> Vec<(&'static str, &'static str)> {
    let mut result = Vec::new();
    for binding in BINDINGS {
        for &(hint_ctx, label, desc) in binding.hints {
            if hint_ctx == ctx {
                result.push((label, desc));
            }
        }
    }
    result
}

/// Help overlay sections grouped by section name, in display order.
pub fn help_sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let mut sections: Vec<(&str, Vec<(&str, &str)>)> = HELP_SECTION_ORDER
        .iter()
        .map(|&s| (s, Vec::new()))
        .collect();
    for binding in BINDINGS {
        if let Some(ref help) = binding.help {
            for section in &mut sections {
                if section.0 == help.section {
                    section.1.push((help.label, help.description));
                    break;
                }
            }
        }
    }
    sections.retain(|(_, entries)| !entries.is_empty());
    sections
}

/// All palette-visible bindings matching a filter string.
pub fn filtered_palette(input: &str) -> Vec<&'static Binding> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return BINDINGS.iter().filter(|b| b.palette.is_some()).collect();
    }
    let mut name_matches = Vec::new();
    let mut desc_matches = Vec::new();
    for b in BINDINGS.iter() {
        if let Some(ref p) = b.palette {
            if p.name.contains(&needle) {
                name_matches.push(b);
            } else if p.description.to_lowercase().contains(&needle) {
                desc_matches.push(b);
            }
        }
    }
    name_matches.extend(desc_matches);
    name_matches
}
