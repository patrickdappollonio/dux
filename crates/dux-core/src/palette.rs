//! The command-palette registry: the single source of truth for every command
//! the TUI's `Ctrl-p` palette can run.
//!
//! Each [`PaletteCommand`] carries the action it dispatches plus the dashed
//! command name and description shown in the palette. Keybindings remain
//! TUI-side (`keybindings.rs`), because they are a TUI concern; this table is
//! transport-agnostic.
//!
//! ## Scope: this table is the TUI palette, and only the TUI palette
//!
//! The web UI has no command palette. Its equivalent surface is the cog **app
//! menu**, which is defined client-side in
//! `crates/dux-web/web/src/lib/appMenu.ts` and does NOT read this table. The
//! two surfaces are independent: there is no projection and no cross-language
//! pin holding them together.
//!
//! This table therefore carries no per-surface metadata. It previously had a
//! `PaletteSurface` enum marking rows as Tui/Web/Both, which existed only to
//! feed a web projection; that projection is gone, and the enum's own doc
//! comment had already rotted (it claimed no row used `Web` while three did).
//!
//! When you add a command here, decide explicitly whether it also warrants an
//! entry in the web app menu (see CLAUDE.md); nothing will fail if you skip
//! it. Many commands here are inherently per-project, per-session, or
//! per-terminal; on the web those live as parameterized row/menu/dialog
//! actions rather than global menu entries. The per-row comments below record
//! that reasoning. The exhaustiveness test in `keybindings.rs` guarantees
//! every command in this table is listed by the TUI palette exactly once.

use crate::action::Action;

/// One row of the palette registry.
pub struct PaletteCommand {
    /// The action this command dispatches (the join key to TUI keybindings).
    pub action: Action,
    /// The dashed command name shown and matched in the palette (e.g.
    /// `start-web-server`). This is the stable command id.
    pub name: &'static str,
    /// One-line description shown alongside the name in the palette.
    pub description: &'static str,
}

/// The palette registry. Order mirrors `keybindings::BINDING_DEFS` so the TUI
/// palette listing renders in a stable, canonical order.
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
    },
    PaletteCommand {
        action: Action::NewAgent,
        name: "new-agent",
        description: "Create a new agent for the selected project",
        // Per-project: web's new-agent dialog is launched per project row/menu.
    },
    PaletteCommand {
        action: Action::NewAgentFromPr,
        name: "new-agent-from-pr",
        description: "Create a new agent from a GitHub pull request",
        // Per-project: web exposes "New agent from PR in <project>" per project.
    },
    PaletteCommand {
        action: Action::NewAgentFromWorktree,
        name: "new-agent-from-worktree",
        description: "Create a new agent from an existing git worktree",
        // Per-project: web's attach-worktree dialog is launched per project.
    },
    PaletteCommand {
        action: Action::ForkAgent,
        name: "fork-agent",
        description: "Fork the selected agent into a fresh worktree and session",
        // Per-session: web exposes "Fork agent…" from the session context.
    },
    PaletteCommand {
        action: Action::ChangeAgentProvider,
        name: "change-agent-provider",
        description: "Swap this worktree's provider",
        // Per-session: web exposes the provider picker from the session menu.
    },
    PaletteCommand {
        action: Action::NewTab,
        name: "new-agent-tab",
        description: "Add a tab to the selected agent, choosing its provider",
        // Per-session: web adds tabs from the tab strip's + button.
    },
    PaletteCommand {
        action: Action::CloseTab,
        name: "close-tab",
        description: "Close the focused tab of the selected agent",
    },
    PaletteCommand {
        action: Action::ChangeDefaultProvider,
        name: "change-default-provider",
        description: "Change the global default provider for new agents in projects without a project-specific override",
        // TUI-only: the web has no wire command or UI for the global default
        // provider; project defaults are edited per project instead.
    },
    PaletteCommand {
        action: Action::ChangeProjectDefaultProvider,
        name: "change-project-default-provider",
        description: "Change the selected project's default provider for future agents in that project only",
        // Per-project: web edits this in the project settings dialog.
    },
    PaletteCommand {
        action: Action::ChangeTheme,
        name: "change-theme",
        description: "Switch the dux color theme",
        // TUI-only: the web has no theme switcher (it follows the browser/CSS).
    },
    PaletteCommand {
        action: Action::ReloadConfig,
        name: "reload-config",
        description: "Reload config.toml after validating it",
        // GLOBAL: reloads the whole config; it acts on no selected target.
        // Web equivalent: the app menu's Configuration submenu.
    },
    PaletteCommand {
        action: Action::StartWebServer,
        name: "start-web-server",
        description: "Stop the TUI and serve the dux web UI over your running agents",
        // TUI-only: this IS the escape hatch INTO the web UI; meaningless once
        // you are already in the web UI.
    },
    PaletteCommand {
        action: Action::ToggleProjectAutoReopenAgents,
        name: "toggle-project-auto-reopen-agents",
        description: "Opt the selected project in or out of startup agent reopening",
        // Per-project: web edits this in the project settings dialog.
    },
    PaletteCommand {
        action: Action::ToggleAgentAutoReopen,
        name: "toggle-agent-auto-reopen",
        description: "Opt the selected agent in or out of startup reopening",
        // Per-session: web toggles this from the session actions group.
    },
    PaletteCommand {
        action: Action::ConfigureStartupCommand,
        name: "configure-startup-command",
        description: "Configure the selected project's startup command",
        // Per-project: web edits this in the project settings dialog.
    },
    PaletteCommand {
        action: Action::ConfigureGlobalEnv,
        name: "configure-global-env",
        description: "Configure environment variables inherited by every project",
        // GLOBAL: the global env applies to every project; no target.
        // Web equivalent: the app menu's Configuration submenu.
    },
    PaletteCommand {
        action: Action::ConfigureProjectEnv,
        name: "configure-project-env",
        description: "Configure environment variables for the selected project's agents and terminals",
        // Per-project: web edits this in the project settings dialog.
    },
    PaletteCommand {
        action: Action::RerunStartupCommandOnAgent,
        name: "rerun-startup-command-on-agent",
        description: "Rerun the selected agent's startup command",
        // Per-session: not surfaced as a global web command.
    },
    PaletteCommand {
        action: Action::ReadStartupCommandLogs,
        name: "read-startup-command-logs",
        description: "Read startup command logs for the selected agent or project",
        // TUI-only: opens server-side log files in a local viewer (a server-side
        // footgun on the web; no remote log viewer is built).
    },
    PaletteCommand {
        action: Action::FocusAgent,
        name: "show-agent",
        description: "Show and focus the selected agent",
        // Per-session: web's "Switch session" group selects an agent.
    },
    PaletteCommand {
        action: Action::OpenProjectBrowser,
        name: "add-project",
        description: "Open the project browser",
        // TUI-only: opens the project browser to add a NEW project. The web
        // UI surfaces this through a dedicated Add-project button.
    },
    PaletteCommand {
        action: Action::CopyPath,
        name: "copy-path",
        description: "Copy the selected agent's worktree path",
        // TUI-only: a server-side filesystem path is meaningless to copy in a
        // remote browser.
    },
    PaletteCommand {
        action: Action::OpenWorktreeInEditor,
        name: "open-worktree",
        description: "Open the selected agent worktree in the configured editor",
        // TUI-only: launches a local editor on the server host (server-side
        // footgun; nothing the browser can do).
    },
    PaletteCommand {
        action: Action::ChooseWorktreeEditor,
        name: "open-worktree-with",
        description: "Choose which editor should open the selected agent worktree",
        // TUI-only: same server-side editor launch as `open-worktree`.
    },
    PaletteCommand {
        action: Action::RefreshProject,
        name: "pull-project",
        description: "Git pull the selected project checkout",
        // Per-project: web exposes "Pull <project>…" per project row.
    },
    PaletteCommand {
        action: Action::CheckoutProjectDefaultBranch,
        name: "checkout-project-default-branch",
        description: "Check out the selected project's default branch",
        // Per-project: web exposes "Checkout default branch for <project>…".
    },
    PaletteCommand {
        action: Action::ReconnectAgent,
        name: "reconnect-agent",
        description: "Restart the CLI for the selected agent",
        // TUI-only: plain (resume) reconnect has no web surface; the web's
        // agent menu deliberately offers only the confirmed force variant.
    },
    PaletteCommand {
        action: Action::ShowTerminal,
        name: "show-terminal",
        description: "Open the first companion terminal, or launch a new one",
        // Per-session: web manages companion terminals per session inline.
    },
    PaletteCommand {
        action: Action::DeleteSession,
        name: "delete-agent",
        description: "Delete the selected agent session",
        // Per-session: web deletes a session via its per-row delete + confirm.
    },
    PaletteCommand {
        action: Action::DeleteTerminal,
        name: "delete-terminal",
        description: "Delete the selected companion terminal",
        // Per-terminal: web deletes terminals via their per-row delete + confirm.
    },
    PaletteCommand {
        action: Action::OpenCurrentPullRequest,
        name: "open-current-pr",
        description: "Open the selected agent's current pull request in the default browser",
        // Per-session: web links to the PR directly from the session's PR badge.
    },
    // ── Global ────────────────────────────────────────────────────
    PaletteCommand {
        action: Action::ToggleSidebar,
        name: "toggle-sidebar",
        description: "Collapse or expand the projects sidebar",
        // TUI-only: web layout is responsive; focus is the mode, no manual
        // pane collapse command.
    },
    PaletteCommand {
        action: Action::ToggleGitPane,
        name: "toggle-git-pane",
        description: "Collapse or expand the git pane",
        // TUI-only: TUI-specific pane layout.
    },
    PaletteCommand {
        action: Action::ToggleHelp,
        name: "help",
        description: "Open the help overlay",
        // TUI-only: the help overlay enumerates TUI keybindings, which do not
        // apply to the web.
    },
    PaletteCommand {
        action: Action::ForceRedraw,
        name: "force-redraw",
        description: "Force a full terminal redraw (clears rendering artifacts)",
        // TUI-only: a terminal-redraw concept with no web analog.
    },
    // ── Palette-only (no direct keybinding) ────────────────────────
    PaletteCommand {
        action: Action::KillRunning,
        name: "kill-running",
        description: "Open a modal to kill running agents and companion terminals",
        // GLOBAL: acts on every running agent/terminal; no target. Web
        // equivalent: the app menu's "Task Manager…" opens TaskManagerDialog,
        // which lists dux itself, every running agent tab, and every
        // companion terminal with live CPU/RSS numbers. Each row's Stop
        // control CONFIRMS before acting (via the existing close-tab/delete-
        // terminal dialogs) rather than force-killing on click; agents
        // detach via WireCommand::KillSessionPty, terminals via
        // DeleteTerminal. A "Stop all…" action confirms once and stops
        // everything.
    },
    PaletteCommand {
        action: Action::NewTerminal,
        name: "new-terminal",
        description: "Spawn a new companion terminal for the selected agent, or a project terminal for the selected project",
        // Per-session/per-project: web spawns terminals from the row menus inline.
    },
    PaletteCommand {
        action: Action::RenameSession,
        name: "rename-agent",
        description: "Rename the selected agent session",
        // Per-session: web exposes "Rename…" from the session actions group.
    },
    PaletteCommand {
        action: Action::OpenAgentInfo,
        name: "agent-info",
        description: "Show the selected agent's details and branch lineage",
        // Per-session: the web exposes "Agent info…" from the session ⋯ menu.
    },
    PaletteCommand {
        action: Action::DeleteProject,
        name: "delete-project",
        description: "Remove the selected project and its sessions",
        // TUI-only (audit decision): web offers remove-only (keeps files), not
        // a destructive project-and-sessions delete.
    },
    PaletteCommand {
        action: Action::RemoveProject,
        name: "remove-project",
        description: "Remove project from app (keeps files on disk)",
        // Per-project: web removes a project from its per-project menu/dialog.
    },
    PaletteCommand {
        action: Action::SortAgentsByUpdated,
        name: "sort-agents-by-updated",
        description: "Sort agents by most recently updated",
        // GLOBAL: reorders every project's agents; no target.
        // Web equivalent: the app menu's "Sort agents by" submenu.
    },
    PaletteCommand {
        action: Action::SortAgentsByCreated,
        name: "sort-agents-by-created",
        description: "Sort agents by creation date (newest first)",
        // GLOBAL: reorders every project's agents; no target.
        // Web equivalent: the app menu's "Sort agents by" submenu.
    },
    PaletteCommand {
        action: Action::SortAgentsByName,
        name: "sort-agents-by-name",
        description: "Sort agents alphabetically by name",
        // GLOBAL: reorders every project's agents; no target.
        // Web equivalent: the app menu's "Sort agents by" submenu.
    },
    PaletteCommand {
        action: Action::RemoveGitPane,
        name: "toggle-remove-git-pane",
        description: "Remove or restore the git pane entirely",
        // The web mirrors this as hide/show of its Changes pane, but as a
        // PREFERENCE (ui.show_changes_pane in the Preferences dialog) plus a
        // live toggle in the Changes actions menu, not as an app-menu entry.
    },
    PaletteCommand {
        action: Action::EditMacros,
        name: "edit-macros",
        description: "Edit text macros for interactive mode",
        // GLOBAL: editing macros is config-wide (the whole `[macros]` map), not a
        // per-target action. Web equivalent: the app menu's Configuration submenu
        // opens the macro-editor dialog (list/add/edit/delete; saves wholesale via
        // `update_macros`). Running a macro is the per-target action and stays off
        // both surfaces' global menus (it lives in the terminal-pane popover).
    },
    PaletteCommand {
        action: Action::DebugInput,
        name: "input-debugging",
        description: "Open input event debugger to inspect keyboard and mouse events",
        // TUI-only: inspects raw terminal input events.
    },
    PaletteCommand {
        action: Action::ToggleDiffLineNumbers,
        name: "toggle-diff-line-numbers",
        description: "Toggle line numbers in diff view",
        // TUI-only: toggles the TUI diff overlay's gutters. The web renders diffs
        // in Monaco's DiffEditor, which manages its own line-number gutters.
    },
    PaletteCommand {
        action: Action::ResourceMonitor,
        name: "resource-monitor",
        description: "Show CPU and memory usage for dux and all running agents",
        // TUI-only (audit decision): the resource monitor is not built for web.
    },
    PaletteCommand {
        action: Action::ToggleGithubIntegration,
        name: "toggle-github-integration",
        description: "Toggle GitHub PR integration",
        // The web exposes this as a PREFERENCE row (ui.github_integration in the
        // Preferences dialog), not an app-menu entry. That row still writes through
        // WireCommand::ToggleGithubIntegration, which flips the flag AND drives the
        // engine's PR-sync side effects.
    },
    PaletteCommand {
        action: Action::ToggleAlwaysShowTabs,
        name: "toggle-always-show-tabs",
        description: "Toggle always showing the agent tab strip, even with a single tab",
        // The web exposes this as a PREFERENCE row (ui.always_show_tab_strip in
        // the Preferences dialog), not an app-menu entry.
    },
    PaletteCommand {
        action: Action::ToggleRandomizedPetNameDefault,
        name: "toggle-randomized-pet-name-default",
        description: "Toggle whether new agent prompts start with a random pet name",
        // The web exposes this as a PREFERENCE row
        // (defaults.enable_randomized_pet_name_by_default in the Preferences
        // dialog), not an app-menu entry. The web new-agent dialog still has its
        // own per-open randomize checkbox, seeded from this default.
    },
    PaletteCommand {
        action: Action::TogglePrBannerPosition,
        name: "toggle-pr-banner-position",
        description: "Move PR banner between top and bottom of agent pane",
        // The web exposes this as a PREFERENCE row (ui.pr_banner_position in the
        // Preferences dialog), not an app-menu entry. Mobile always pins the
        // banner to the top regardless.
    },
    PaletteCommand {
        action: Action::ForceReconnectAgent,
        name: "force-reconnect-agent",
        description: "Force-reconnect the agent with a fresh session (no --continue)",
        // Per-session: web exposes "Force recreate agent…" in the agent menu,
        // gated by a confirmation dialog.
    },
];

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
    fn new_agent_tab_command_is_named_new_agent_tab() {
        assert!(
            PALETTE_COMMANDS
                .iter()
                .any(|c| c.action == Action::NewTab && c.name == "new-agent-tab"),
            "expected a new-agent-tab palette command for Action::NewTab"
        );
        assert!(
            !PALETTE_COMMANDS.iter().any(|c| c.name == "new-tab"),
            "the old new-tab palette command name should no longer exist"
        );
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

    // NOTE: this registry deliberately has NO cross-language pin to the web.
    // It used to carry two: `web_surface_ids_match_expected_pin` (the exact set
    // of web-surfaced ids) and `web_pin_matches_the_typescript_pin` (which
    // parsed the vitest file to catch a one-sided edit). Both are gone along
    // with the web projection they guarded: the web app menu
    // (`crates/dux-web/web/src/lib/appMenu.ts`) is now an independent,
    // client-owned surface with its own titles, its own item set, and submenus,
    // so a pin claiming a relationship between the two would be a false record.
    // Keeping the surfaces in step is a deliberate human step; see CLAUDE.md.
}
