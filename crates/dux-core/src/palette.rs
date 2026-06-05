//! Surface-aware command-palette registry: the single source of truth for
//! every command the `Ctrl-P` palette can run.
//!
//! Each [`PaletteCommand`] carries the action it dispatches, the dashed command
//! name and description shown in the palette, and the [`PaletteSurface`] that
//! declares which UI(s) surface it as a *global, parameter-free* palette entry.
//!
//! Parity is by construction: name and description live here once, so the TUI
//! and the web cannot drift. Keybindings remain TUI-side (`keybindings.rs`),
//! because they are a TUI concern; this table is transport-agnostic.
//!
//! ## What "surface" means
//!
//! A command is [`PaletteSurface::Both`] or [`PaletteSurface::Web`] only when
//! the web has a *global* handler that needs no per-row context. Many TUI
//! palette commands are inherently per-project, per-session, or per-terminal:
//! on the web those live as parameterized row/menu/dialog actions, not as
//! global palette entries, so they are marked [`PaletteSurface::Tui`] here with
//! a comment naming the web's equivalent surface. The exhaustiveness test in
//! `keybindings.rs` guarantees every TUI palette command appears in this table
//! exactly once.

use crate::action::Action;

/// Which UI surfaces expose a command as a global, parameter-free palette entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteSurface {
    /// TUI palette only. Either the command makes no sense on the web, or the
    /// web exposes the same capability through a parameterized per-row /
    /// per-project / per-session action (a menu, dialog, or inline control)
    /// rather than a global palette command.
    Tui,
    /// Web palette only. (None today — every web palette command is also a TUI
    /// command — but the variant exists so a future web-only command is honest.)
    #[allow(dead_code)]
    Web,
    /// Both surfaces expose it as a global palette command.
    Both,
}

impl PaletteSurface {
    /// Whether this command should appear in the TUI palette listing.
    pub fn in_tui(self) -> bool {
        matches!(self, PaletteSurface::Tui | PaletteSurface::Both)
    }

    /// Whether this command should appear in the web palette listing.
    pub fn in_web(self) -> bool {
        matches!(self, PaletteSurface::Web | PaletteSurface::Both)
    }
}

/// One row of the palette registry.
pub struct PaletteCommand {
    /// The action this command dispatches (the join key to TUI keybindings).
    pub action: Action,
    /// The dashed command name shown and matched in the palette (e.g.
    /// `start-web-server`). This is the stable command id for both surfaces.
    pub name: &'static str,
    /// One-line description shown alongside the name in the palette.
    pub description: &'static str,
    /// Which surfaces expose this as a global palette command.
    pub surface: PaletteSurface,
}

/// The palette registry. Order mirrors `keybindings::BINDING_DEFS` so the TUI
/// palette listing stays byte-identical to its previous (BINDING_DEFS-ordered)
/// output, and the web "Commands" group renders in the same canonical order.
///
/// Every entry here joins 1:1 to a `BINDING_DEFS` entry by [`Action`] (the TUI
/// attaches keybindings and dispatches through that join). The exhaustiveness
/// pin lives in `keybindings.rs` (`palette_listing_matches_core_registry`).
pub const PALETTE_COMMANDS: &[PaletteCommand] = &[
    // ── Projects pane ─────────────────────────────────────────────
    PaletteCommand {
        action: Action::ToggleProject,
        name: "toggle-project",
        description: "Collapse or expand the selected project's agents",
        // Per-project: web collapses/expands projects directly in the sidebar.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::NewAgent,
        name: "new-agent",
        description: "Create a new agent for the selected project",
        // Per-project: web's new-agent dialog is launched per project row/menu.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::NewAgentFromPr,
        name: "new-agent-from-pr",
        description: "Create a new agent from a GitHub pull request",
        // Per-project: web exposes "New agent from PR in <project>" per project.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::NewAgentFromWorktree,
        name: "new-agent-from-worktree",
        description: "Create a new agent from an existing git worktree",
        // Per-project: web's attach-worktree dialog is launched per project.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ForkAgent,
        name: "fork-agent",
        description: "Fork the selected agent into a fresh worktree and session",
        // Per-session: web exposes "Fork agent…" from the session context.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ChangeAgentProvider,
        name: "change-agent-provider",
        description: "Swap this worktree's provider",
        // Per-session: web exposes the provider picker from the session menu.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ChangeDefaultProvider,
        name: "change-default-provider",
        description: "Change the global default provider for new agents in projects without a project-specific override",
        // TUI-only: the web has no wire command or UI for the global default
        // provider; project defaults are edited per project instead.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ChangeProjectDefaultProvider,
        name: "change-project-default-provider",
        description: "Change the selected project's default provider for future agents in that project only",
        // Per-project: web edits this in the project settings dialog.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ChangeTheme,
        name: "change-theme",
        description: "Switch the dux color theme",
        // TUI-only: the web has no theme switcher (it follows the browser/CSS).
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ReloadConfig,
        name: "reload-config",
        description: "Reload config.toml after validating it",
        // Both: web sends the `reload_config` wire command from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::StartWebServer,
        name: "start-web-server",
        description: "Stop the TUI and serve the dux web UI over your running agents",
        // TUI-only: this IS the escape hatch INTO the web UI; meaningless once
        // you are already in the web UI.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleProjectAutoReopenAgents,
        name: "toggle-project-auto-reopen-agents",
        description: "Opt the selected project in or out of startup agent reopening",
        // Per-project: web edits this in the project settings dialog.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleAgentAutoReopen,
        name: "toggle-agent-auto-reopen",
        description: "Opt the selected agent in or out of startup reopening",
        // Per-session: web toggles this from the session actions group.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ConfigureStartupCommand,
        name: "configure-startup-command",
        description: "Configure the selected project's startup command",
        // Per-project: web edits this in the project settings dialog.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ConfigureGlobalEnv,
        name: "configure-global-env",
        description: "Configure environment variables inherited by every project",
        // Both: web opens the global-environment dialog from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::ConfigureProjectEnv,
        name: "configure-project-env",
        description: "Configure environment variables for the selected project's agents and terminals",
        // Per-project: web edits this in the project settings dialog.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::RerunStartupCommandOnAgent,
        name: "rerun-startup-command-on-agent",
        description: "Rerun the selected agent's startup command",
        // Per-session: not surfaced as a global web command.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ReadStartupCommandLogs,
        name: "read-startup-command-logs",
        description: "Read startup command logs for the selected agent or project",
        // TUI-only: opens server-side log files in a local viewer (a server-side
        // footgun on the web; no remote log viewer is built).
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::FocusAgent,
        name: "show-agent",
        description: "Show and focus the selected agent",
        // Per-session: web's "Switch session" group selects an agent.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::OpenProjectBrowser,
        name: "add-project",
        description: "Open the project browser",
        // Both: web opens the add-project dialog from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::CopyPath,
        name: "copy-path",
        description: "Copy the selected agent's worktree path",
        // TUI-only: a server-side filesystem path is meaningless to copy in a
        // remote browser.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::OpenWorktreeInEditor,
        name: "open-worktree",
        description: "Open the selected agent worktree in the configured editor",
        // TUI-only: launches a local editor on the server host (server-side
        // footgun; nothing the browser can do).
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ChooseWorktreeEditor,
        name: "open-worktree-with",
        description: "Choose which editor should open the selected agent worktree",
        // TUI-only: same server-side editor launch as `open-worktree`.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::RefreshProject,
        name: "pull-project",
        description: "Git pull the selected project checkout",
        // Per-project: web exposes "Pull <project>…" per project row.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::CheckoutProjectDefaultBranch,
        name: "checkout-project-default-branch",
        description: "Check out the selected project's default branch",
        // Per-project: web exposes "Checkout default branch for <project>…".
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ReconnectAgent,
        name: "reconnect-agent",
        description: "Restart the CLI for the selected agent",
        // Per-session: web exposes "Reconnect" from the session actions group.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ShowTerminal,
        name: "show-terminal",
        description: "Open the first companion terminal, or launch a new one",
        // Per-session: web manages companion terminals per session inline.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::DeleteSession,
        name: "delete-agent",
        description: "Delete the selected agent session",
        // Per-session: web deletes a session via its per-row delete + confirm.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::DeleteTerminal,
        name: "delete-terminal",
        description: "Delete the selected companion terminal",
        // Per-terminal: web deletes terminals via their per-row delete + confirm.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::OpenCurrentPullRequest,
        name: "open-current-pr",
        description: "Open the selected agent's current pull request in the default browser",
        // Per-session: web links to the PR directly from the session's PR badge.
        surface: PaletteSurface::Tui,
    },
    // ── Global ────────────────────────────────────────────────────
    PaletteCommand {
        action: Action::ToggleSidebar,
        name: "toggle-sidebar",
        description: "Collapse or expand the projects sidebar",
        // TUI-only: web layout is responsive; focus is the mode, no manual
        // pane collapse command.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleGitPane,
        name: "toggle-git-pane",
        description: "Collapse or expand the git pane",
        // TUI-only: TUI-specific pane layout.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleHelp,
        name: "help",
        description: "Open the help overlay",
        // TUI-only: the help overlay enumerates TUI keybindings, which do not
        // apply to the web.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ForceRedraw,
        name: "force-redraw",
        description: "Force a full terminal redraw (clears rendering artifacts)",
        // TUI-only: a terminal-redraw concept with no web analog.
        surface: PaletteSurface::Tui,
    },
    // ── Palette-only (no direct keybinding) ────────────────────────
    PaletteCommand {
        action: Action::KillRunning,
        name: "kill-running",
        description: "Open a modal to kill running agents and companion terminals",
        // TUI-only (audit decision): web kills runtimes via per-row delete +
        // confirm, not a bulk kill modal.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::NewTerminal,
        name: "new-terminal",
        description: "Spawn a new companion terminal for the selected agent",
        // Per-session: web spawns companion terminals per session inline.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::RenameSession,
        name: "rename-agent",
        description: "Rename the selected agent session",
        // Per-session: web exposes "Rename…" from the session actions group.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::DeleteProject,
        name: "delete-project",
        description: "Remove the selected project and its sessions",
        // TUI-only (audit decision): web offers remove-only (keeps files), not
        // a destructive project-and-sessions delete.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::RemoveProject,
        name: "remove-project",
        description: "Remove project from app (keeps files on disk)",
        // Per-project: web removes a project from its per-project menu/dialog.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::SortAgentsByUpdated,
        name: "sort-agents-by-updated",
        description: "Sort agents by most recently updated",
        // Both: web sorts via `sortAgents("updated")` from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::SortAgentsByCreated,
        name: "sort-agents-by-created",
        description: "Sort agents by creation date (newest first)",
        // Both: web sorts via `sortAgents("created")` from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::SortAgentsByName,
        name: "sort-agents-by-name",
        description: "Sort agents alphabetically by name",
        // Both: web sorts via `sortAgents("name")` from the palette.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::RemoveGitPane,
        name: "toggle-remove-git-pane",
        description: "Remove or restore the git pane entirely",
        // TUI-only: TUI-specific pane layout.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::EditMacros,
        name: "edit-macros",
        description: "Edit text macros for interactive mode",
        // TUI-only: macros are an interactive-mode (PTY key-forwarding) feature
        // with no web equivalent.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::DebugInput,
        name: "input-debugging",
        description: "Open input event debugger to inspect keyboard and mouse events",
        // TUI-only: inspects raw terminal input events.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleDiffLineNumbers,
        name: "toggle-diff-line-numbers",
        description: "Toggle line numbers in diff view",
        // Both: web flips its diff gutters via `toggleDiffLineNumbers()`.
        surface: PaletteSurface::Both,
    },
    PaletteCommand {
        action: Action::ResourceMonitor,
        name: "resource-monitor",
        description: "Show CPU and memory usage for dux and all running agents",
        // TUI-only (audit decision): the resource monitor is not built for web.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleGithubIntegration,
        name: "toggle-github-integration",
        description: "Toggle GitHub PR integration",
        // TUI-only: no wire command or web UI exists to toggle this globally.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ToggleRandomizedPetNameDefault,
        name: "toggle-randomized-pet-name-default",
        description: "Toggle whether new agent prompts start with a random pet name",
        // TUI-only: no wire command or web UI exists to toggle this default
        // (the web new-agent dialog has its own per-open randomize checkbox).
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::TogglePrBannerPosition,
        name: "toggle-pr-banner-position",
        description: "Move PR banner between top and bottom of agent pane",
        // TUI-only: no wire command exists; the web reads pr_banner_position
        // from config and mobile always pins it to the top.
        surface: PaletteSurface::Tui,
    },
    PaletteCommand {
        action: Action::ForceReconnectAgent,
        name: "force-reconnect-agent",
        description: "Force-reconnect the agent with a fresh session (no --continue)",
        // Per-session: web exposes "Force reconnect (fresh)" in session actions.
        surface: PaletteSurface::Tui,
    },
];

/// All palette commands surfaced on the web, in registry (canonical) order.
pub fn web_palette_commands() -> impl Iterator<Item = &'static PaletteCommand> {
    PALETTE_COMMANDS.iter().filter(|c| c.surface.in_web())
}

/// Look up a palette command by the action it dispatches.
pub fn find_by_action(action: Action) -> Option<&'static PaletteCommand> {
    PALETTE_COMMANDS.iter().find(|c| c.action == action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in PALETTE_COMMANDS {
            assert!(
                seen.insert(c.name),
                "duplicate palette command name: {}",
                c.name
            );
        }
    }

    #[test]
    fn actions_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in PALETTE_COMMANDS {
            assert!(
                seen.insert(c.action),
                "duplicate palette command action for name: {}",
                c.name
            );
        }
    }

    // TWO-SIDED PIN (Rust half): the exact set of web-surfaced palette command
    // ids. The vitest counterpart pins the TS handler-map keys to this same
    // list — see `crates/dux-web/web/src/lib/paletteRegistry.test.ts`. Changing
    // one surface without the other fails a gate. Keep this list alphabetized.
    #[test]
    fn web_surface_ids_match_expected_pin() {
        let expected = [
            "add-project",
            "configure-global-env",
            "reload-config",
            "sort-agents-by-created",
            "sort-agents-by-name",
            "sort-agents-by-updated",
            "toggle-diff-line-numbers",
        ];
        let mut actual: Vec<&str> = web_palette_commands().map(|c| c.name).collect();
        actual.sort_unstable();
        assert_eq!(
            actual, expected,
            "web-surfaced palette ids drifted from the pin. If this is intentional, \
             update BOTH this list and EXPECTED_WEB_COMMANDS in paletteRegistry.test.ts."
        );
    }
}
