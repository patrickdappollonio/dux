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
        // TUI-only: the web sidebar has no project grouping to collapse. It
        // replaced the project -> agents tree with one flat, ordered agent list
        // (`lib/flatList.ts`), so there is no project header and no per-project
        // expand state; its collapsible sections are Terminals and the quiet
        // tail. The older "web collapses/expands projects in the sidebar" note
        // was false.
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
        action: Action::ManageProjects,
        name: "manage-projects",
        description: "Choose a project to target for project actions",
        // Per-project (chooser): opens the project chooser so project-scoped
        // palette commands act on the picked project. The web reaches every
        // project through its Add-project picker and per-project ⋯ menus, so
        // there is no equivalent app-menu entry.
    },
    PaletteCommand {
        action: Action::ManageWorktrees,
        name: "manage-worktrees",
        description: "Remove a worktree dux manages for a project, and optionally its branch",
        // Per-project: the web has this surface already, as the Worktrees
        // dialog reached from a project row. Both are the MANUAL OVERRIDE for
        // deleting a branch that deleting an agent will not touch, and both
        // drive `dux_core::worktree_manager`. No app-menu entry: the command
        // acts on one chosen project, not on the workspace.
    },
    PaletteCommand {
        action: Action::FilterAgents,
        name: "filter-agents",
        description: "Filter the agent list by name, branch, project, or folder",
        // GLOBAL (display-only): a live filter over the flat agent list, mirroring
        // the web sidebar/hub search box. It never mutates or persists sessions.
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
        // GLOBAL: the web has it as the "Default provider for new agents" row in
        // the Preferences dialog (`settingsDescriptors.ts`, key
        // `defaults.provider`), written through the settings PATCH in
        // `config_routes.rs`. The older "the web has no wire command or UI for
        // this" note was false in both halves.
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
        // TUI-only: the web has no theme switcher because it is pinned DARK (it
        // does not follow the browser, as this note used to say: `main.tsx` force-
        // adds the `.dark` class and the light tokens are inert).
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
        action: Action::StartBackgroundServer,
        name: "start-background-server",
        description: "Serve the dux web UI in the background and keep using the TUI",
        // TUI-only, and for the same reason as start-web-server: it is a way INTO
        // the web UI. Named distinctly from it on purpose, because the two do
        // different things to this terminal, and the flip is unaffected by it.
    },
    PaletteCommand {
        action: Action::StopBackgroundServer,
        name: "stop-background-server",
        description: "Stop serving the web UI in the background; your agents keep running",
        // TUI-only: stopping the server from inside the server would be sawing
        // off the branch. Closing the browser tab is the web-side equivalent.
    },
    PaletteCommand {
        action: Action::TakeOverTerminal,
        name: "take-over-terminal",
        description: "Drive the center terminal from here, demoting the device that has it",
        // TUI-only, and the mirror of the web's Take over button: the web already
        // has that button on the pane itself, so it needs no menu entry, and this
        // is where the same gesture lives on this surface.
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
        // Per-session: the web has it as "Rerun startup command" in the agent's ⋯
        // menu (`POST /api/v1/sessions/:id/rerun-startup-command`). Not a GLOBAL
        // web command, which is all the older note meant, but it read as "absent".
    },
    PaletteCommand {
        action: Action::ReadStartupCommandLogs,
        name: "read-startup-command-logs",
        description: "Read startup command logs for the selected agent or project",
        // Both scopes exist on the web too, as row-menu actions rather than one
        // command: the agent scope in the agent row's menu, the project scope in
        // the project row's menu (they read the same files over
        // `GET /api/v1/{sessions,projects}/:id/startup-logs`). The older
        // "TUI-only, no remote log viewer" note here was already false for the
        // agent scope and is now false for both.
    },
    PaletteCommand {
        action: Action::FocusAgent,
        name: "show-agent",
        description: "Show and focus the selected agent",
        // Per-session: on the web an agent is selected by clicking its sidebar or
        // mobile-hub row (`selectSession` in `store.ts`), including from the
        // collapsed icon rail. There is no named "Switch session" group anywhere
        // in the web UI, which is what this note used to claim.
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
        description: "Copy the selected agent's directory",
        // Per-session: the web has it as "Copy local path" in the agent's ⋯ menu
        // (`FlatAgentList.tsx`), copying `session.worktree_path` off the spine.
        // The older "TUI-only, a server-side path is meaningless in a remote
        // browser" note was false: the common case is a browser on the same
        // machine, where the path is exactly what the user wants to paste into a
        // terminal.
    },
    PaletteCommand {
        action: Action::OpenWorktreeInEditor,
        name: "open-worktree",
        description: "Open the selected agent's directory in the configured editor",
        // NEAR-EQUIVALENT, not absent: the web DOES spawn an editor on the server
        // host, from the code editor's "Open editor" control
        // (`EditorOverlay.tsx` -> `POST /api/v1/sessions/:id/files/open-in-editor`
        // -> `editor::launch_editor`). Two real differences, so this is not the
        // same command: it opens the CURRENTLY OPEN FILE, never the worktree root
        // (the handler requires the path to exist in the worktree), and the
        // control appears only while a file tab is open, disabled with an
        // explanatory tooltip unless `window.location.hostname` is a local
        // address (`lib/localAccess.ts`), since spawning a GUI editor only helps
        // when the server is the user's own machine. The older "TUI-only, nothing
        // the browser can do" note was false.
    },
    PaletteCommand {
        action: Action::ChooseWorktreeEditor,
        name: "open-worktree-with",
        description: "Choose which editor should open the selected agent's directory",
        // NEAR-EQUIVALENT, same shape as `open-worktree` above: the web's "Open
        // editor" control IS an editor picker (`lib/editors.ts` lists the
        // choices, and the handler honors an explicit pick or falls back to the
        // configured editor). Same two differences: it targets the open FILE
        // rather than the worktree root, and it is local-access gated.
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
    PaletteCommand {
        action: Action::AttachPullRequest,
        name: "attach-pull-request",
        description: "Attach a GitHub pull request to the selected agent",
        // Per-session: the web exposes "Attach pull request…" in the agent's
        // own ⋯ menu (already shipped), so no app-menu entry is warranted.
    },
    PaletteCommand {
        action: Action::DetachPullRequest,
        name: "detach-pull-request",
        description: "Detach the selected agent's pull request and stop looking for one on it",
        // Per-session: the web's agent ⋯ menu carries "Detach pull request",
        // so no app-menu entry is warranted.
    },
    PaletteCommand {
        action: Action::ResumePullRequestAutodetection,
        name: "resume-pull-request-autodetection",
        description: "Look for a pull request on the selected agent's branch again, after a detach",
        // Per-session: the web's agent ⋯ menu carries "Resume PR
        // autodetection" on the same gate, so no app-menu entry is warranted.
    },
    // ── Global ────────────────────────────────────────────────────
    PaletteCommand {
        action: Action::ToggleSidebar,
        name: "toggle-sidebar",
        description: "Collapse or expand the projects sidebar",
        // GLOBAL: the web collapses its sidebar too, via the `SidebarTrigger`
        // button and a Cmd/Ctrl-B chord (`components/ui/sidebar.tsx`); collapsed
        // it renders an icon rail rather than nothing. Desktop only, since the
        // mobile shell mounts no sidebar. The older "web layout is responsive, no
        // manual pane collapse" note was false.
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
        name: "new-terminal-for-agent",
        description: "Spawn a new companion terminal for the selected agent",
        // Per-session: web spawns agent terminals from the session row menu inline.
    },
    PaletteCommand {
        action: Action::NewProjectTerminal,
        name: "new-terminal-for-project",
        description: "Open a terminal for a project you pick",
        // Per-project: web spawns project terminals from the project row menu inline.
    },
    PaletteCommand {
        action: Action::NewStandaloneAgent,
        name: "new-standalone-agent",
        description: "Run an agent in a folder you already have",
        // Global and parameter-free (it opens a folder browser and needs
        // nothing selected), so it also earns a row in the web's creation menu
        // (see `crates/dux-web/web/src/lib/creationMenus.ts`). It sits beside
        // new-standalone-terminal on purpose: both are the "belongs to nothing"
        // shape, and naming them alike is how a user finds the second one.
    },
    PaletteCommand {
        action: Action::NewStandaloneTerminal,
        name: "new-standalone-terminal",
        description: "Open a standalone terminal in your home directory",
        // Global and parameter-free: it needs no agent, no project, and nothing
        // selected, so it also earns a row in the web app menu (see
        // `crates/dux-web/web/src/lib/appMenu.ts`).
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
        action: Action::ShowWelcomeScreen,
        name: "show-welcome-screen",
        description: "Show the dux welcome screen and getting-started steps",
        // GLOBAL and parameter-free. It also has no default keybinding: a
        // read-once screen is not worth a hotkey. The web shows the same screen
        // automatically on a first load and reaches it from its app menu.
    },
    PaletteCommand {
        action: Action::ShowReleaseNotes,
        name: "show-release-notes",
        description: "Show what's new in the running dux version",
        // GLOBAL and parameter-free, and no default keybinding for the same
        // reason as show-welcome-screen. May fetch from GitHub, because the user
        // asked for it; the automatic showing is what the `[ui]
        // disable_release_notes` flag suppresses.
    },
    PaletteCommand {
        action: Action::DeleteProject,
        name: "delete-project",
        description: "Remove the selected project and its sessions",
        // Per-project: the web offers BOTH, as separate items in the project's ⋯
        // menu (`ProjectMenuItems.tsx`) with separate confirmations: "Remove
        // project…" keeps the files, "Delete project…" is this destructive
        // cascade (`DELETE /api/v1/projects/:id?delete_worktrees=true`). The
        // older "web offers remove-only" note was false.
    },
    PaletteCommand {
        action: Action::RemoveProject,
        name: "remove-project",
        description: "Remove project from app (keeps files on disk)",
        // Per-project: web removes a project from its per-project menu/dialog.
    },
    PaletteCommand {
        action: Action::SortAgents,
        name: "sort-agents",
        description: "Cycle the agent-list sort mode (active, updated, created, name)",
        // GLOBAL: a display-only sort over the shared `config.ui.agent_sort`; no
        // target. Cycles the five TUI modes (active, updated, created, name A to Z,
        // name Z to A); it never reorders the stored order. Web near-equivalent:
        // the APP MENU's "Sort agents by" group (`appMenu.ts`), not a sidebar
        // control, and it is a different thing in two ways: it offers three keys
        // (recently updated, created, name) rather than these five, and it is a
        // ONE-SHOT reorder that computes an order and POSTs it as the user's
        // manual drag order, so there is no persisted sort key and no selectable
        // "manual" mode to offer.
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
        // GLOBAL: the web has it as the app menu's "Task Manager…"
        // (`appMenu.ts` -> `TaskManagerDialog.tsx`), reading the same per-process
        // CPU/RSS rows over `GET /api/v1/resources`. The older "not built for
        // web" note was false, and contradicted the `kill-running` note below,
        // which already described this same dialog.
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
    PaletteCommand {
        action: Action::RefreshChanges,
        name: "refresh-changes",
        description: "Recompute the selected agent's changed files right now",
        // Per-session, and the web has the same action in the Changes pane's
        // `⋯` menu rather than in the app menu, since it acts on whichever
        // agent that pane is showing.
    },
    // ── Manual reordering ─────────────────────────────────────────
    // The TUI equivalent of the web's drag-to-reorder. The web has no palette; it
    // reorders by dragging, over one flat agent list rather than within project
    // groups (`lib/flatList.ts`), with terminals dragged inside their own
    // section. So these have no web counterpart.
    PaletteCommand {
        action: Action::MoveAgentUp,
        name: "move-agent-up",
        description: "Move the selected agent up one position (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveAgentDown,
        name: "move-agent-down",
        description: "Move the selected agent down one position (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveAgentTop,
        name: "move-agent-top",
        description: "Move the selected agent to the top (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveAgentBottom,
        name: "move-agent-bottom",
        description: "Move the selected agent to the bottom (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveTerminalUp,
        name: "move-terminal-up",
        description: "Move the selected terminal up one position (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveTerminalDown,
        name: "move-terminal-down",
        description: "Move the selected terminal down one position (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveTerminalTop,
        name: "move-terminal-top",
        description: "Move the selected terminal to the top (sorting becomes manual)",
    },
    PaletteCommand {
        action: Action::MoveTerminalBottom,
        name: "move-terminal-bottom",
        description: "Move the selected terminal to the bottom (sorting becomes manual)",
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
