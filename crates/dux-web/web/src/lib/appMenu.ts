// THE definition of the web UI's app menu: the cog menu in the desktop header
// and the mobile hub's bottom sheet render from this one structure.
//
// SOURCE OF TRUTH. The Rust `dux_core::palette` registry is NOT the source of
// truth for this menu; it is the source of truth for the TUI's `Ctrl-p` command
// palette, and nothing else. The two surfaces are now independent: there is no
// bootstrap projection and no cross-language pin holding them together. When you
// add a `Ctrl-p` palette command, decide explicitly whether it warrants an entry
// here (see CLAUDE.md). Nothing will fail if you skip it, which is exactly why
// it has to be a deliberate step.
//
// The web has no command palette and this menu has NO keyboard shortcut. It is
// reachable by Tab, opened with Enter/Space, and driven with the arrow keys.
//
// This module is pure data: no React, no I/O at module scope, so the whole menu
// is constructible and assertable without mounting anything. `AppMenu.tsx`
// renders it as a desktop flyout and `AppMenuSheet.tsx` as a mobile bottom sheet
// with drill-down (a hover flyout cannot work on touch). NEITHER renderer
// hand-authors items, so the two presentations cannot drift.
//
// What belongs here: GLOBAL actions (things that happen) and dialogs. What does
// NOT: user preferences. A preference is a row in `settingsDescriptors.ts`,
// reached through "Preferences…" below. Per-agent/per-project/per-file actions
// belong in that row's own `⋯` menu.

import {
  Activity,
  ArrowDownAZ,
  Bot,
  PartyPopper,
  ArrowUpDown,
  CalendarPlus,
  Clock,
  FileCode,
  FolderGit2,
  Globe,
  RefreshCw,
  Rocket,
  SlidersHorizontal,
  SquarePen,
  SquareTerminal,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import { toast } from "sonner"

import { configApi } from "@/lib/configApi"
import { addProjectMenuItems, newAgentMenuItems } from "@/lib/creationMenus"
import {
  openConfigEditor,
  openReleaseNotes,
  openWelcomeScreen,
  openCustomizeWebapp,
  openGlobalEnv,
  openTaskManager,
  openMacrosDialog,
  sortAgents,
  createStandaloneTerminal,
} from "@/lib/store"

export type AppMenuEntry = AppMenuItem | AppMenuSubmenu | AppMenuSeparator

export interface AppMenuItem {
  kind: "item"
  /** Stable, test-facing id. NOT a Rust palette id: this menu is client-owned. */
  id: string
  /** Human title. Trailing "…" iff it opens a dialog or asks to confirm. */
  title: string
  icon: LucideIcon
  run: () => void
}

export interface AppMenuSubmenu {
  kind: "submenu"
  id: string
  title: string
  icon: LucideIcon
  entries: AppMenuEntry[]
}

export interface AppMenuSeparator {
  kind: "separator"
  /** Ids are unique across the tree, separators included, so both renderers can
   *  key off `id` without inventing an index-based key. */
  id: string
}

export interface AppMenuContext {
  /** Whether GitHub / `gh` integration is usable (`bootstrap.gh_available`).
   *  Gates the from-PR agent variant, exactly as the sidebar's split button
   *  and the per-project `⋯` menu gate theirs. */
  ghAvailable: boolean
}

/**
 * The app menu, top level first.
 *
 * The context gates the ONE conditional entry (the from-PR agent variant,
 * which is meaningless without `gh`). Resist adding more gating without a
 * real reason: an entry that appears and disappears is harder to learn than
 * one that is always there and explains itself when used.
 */
export function appMenuModel(ctx: AppMenuContext): AppMenuEntry[] {
  // The creation submenus mirror the sidebar's split-button menus: their
  // entries are the shared lists in creationMenus.ts, spliced in verbatim so
  // the cog menu and the sidebar cannot drift (appMenu.test.ts pins this).
  const asItems = (items: ReturnType<typeof addProjectMenuItems>) =>
    items.map((item): AppMenuItem => ({ kind: "item", ...item }))
  return [
    // The two creation submenus OPEN the menu: creating an agent or adding a
    // project is the most common reason to reach for the cog, so they
    // outrank Preferences. They are the menu's twins of the
    // sidebar's New-agent and Add-project split buttons; their entries are
    // the shared lists spliced in verbatim (see asItems above). No trailing
    // "…" on the submenu titles: a submenu opens a list, not a dialog; the
    // "…" lives on the variants inside.
    {
      kind: "submenu",
      id: "new-agent",
      title: "New agent",
      icon: Bot,
      entries: asItems(newAgentMenuItems(ctx)),
    },
    {
      kind: "submenu",
      id: "add-project",
      title: "Add project",
      icon: FolderGit2,
      entries: asItems(addProjectMenuItems()),
    },
    { kind: "separator", id: "sep-create" },
    {
      kind: "item",
      id: "preferences",
      title: "Preferences…",
      icon: SlidersHorizontal,
      run: () => openCustomizeWebapp(),
    },
    { kind: "separator", id: "sep-preferences" },
    {
      kind: "submenu",
      id: "sort-agents",
      title: "Sort agents by",
      icon: ArrowUpDown,
      // Plain action items, deliberately NOT radio items and with no checkmark.
      // `sortAgents` is a one-shot reorder: it computes an order and POSTs it as
      // the user's manual drag order, which they are then free to drag around.
      // There is no persisted sort key to read, so a selected-state indicator
      // would be arbitrary at best and false the moment a row is dragged. Do not
      // add one without first adding a real persisted sort mode (a config field,
      // server support, and drag reconciliation).
      entries: [
        {
          kind: "item",
          id: "sort-updated",
          title: "Recently updated",
          icon: Clock,
          run: () => sortAgents("updated"),
        },
        {
          kind: "item",
          id: "sort-created",
          title: "Created",
          icon: CalendarPlus,
          run: () => sortAgents("created"),
        },
        {
          kind: "item",
          id: "sort-name",
          title: "Name",
          icon: ArrowDownAZ,
          run: () => sortAgents("name"),
        },
      ],
    },
    {
      kind: "submenu",
      id: "configuration",
      title: "Configuration",
      icon: Wrench,
      entries: [
        {
          kind: "item",
          id: "edit-config",
          title: "Edit config file…",
          icon: FileCode,
          run: () => openConfigEditor(),
        },
        {
          kind: "item",
          id: "edit-macros",
          title: "Edit macros…",
          icon: SquarePen,
          run: () => openMacrosDialog(),
        },
        {
          kind: "item",
          id: "global-env",
          title: "Global environment…",
          icon: Globe,
          run: () => openGlobalEnv(),
        },
        { kind: "separator", id: "sep-config" },
        {
          kind: "item",
          id: "reload-config",
          // No ellipsis: this runs immediately, with no dialog and no
          // confirmation. The engine's routed status reports success; only a
          // failure needs a toast of our own.
          title: "Reload config",
          icon: RefreshCw,
          run: () => {
            configApi
              .reload()
              .catch((e) =>
                toast.error(
                  e instanceof Error ? e.message : "Could not reload the config.",
                ),
              )
          },
        },
      ],
    },
    { kind: "separator", id: "sep-agents" },
    {
      kind: "item",
      id: "new-standalone-terminal",
      // GLOBAL and parameter-free: it needs no agent, no project and nothing
      // selected, which is exactly why it belongs here rather than in a row's
      // own `⋯` menu (the twin of the TUI's `new-standalone-terminal` palette
      // command). No trailing "…": it opens the terminal immediately, with no
      // dialog and nothing to confirm.
      title: "New standalone terminal",
      icon: SquareTerminal,
      run: () => createStandaloneTerminal(),
    },
    {
      kind: "item",
      id: "task-manager",
      // Neutral, not destructive-tinted: the trailing "…" plus the dialog's own
      // confirmations are the danger signal (CLAUDE.md menu tenet).
      //
      // `Activity`, not `OctagonX`: stopping is now one action among several on
      // a surface you mostly READ (what is running, and what it costs). The
      // pulse line is the near-universal OS activity-monitor idiom.
      title: "Task Manager…",
      icon: Activity,
      run: () => openTaskManager(),
    },
    { kind: "separator", id: "sep-about" },
    // ACTIONS, not preferences: each opens a dialog, so each keeps a leading
    // icon and a trailing "…". Their `ui.disable_*` counterparts are Preferences
    // rows, and those flags suppress only the AUTOMATIC screens — these entries
    // keep working regardless, which is the whole reason they exist.
    {
      kind: "item",
      id: "welcome-screen",
      title: "Welcome screen…",
      icon: Rocket,
      run: () => openWelcomeScreen(),
    },
    {
      kind: "item",
      id: "release-notes",
      title: "What's new…",
      icon: PartyPopper,
      run: () => openReleaseNotes(),
    },
  ]
}

/** Depth-first lookup of a submenu by id, for tests and the mobile drill-down. */
export function findSubmenu(
  entries: AppMenuEntry[],
  id: string,
): AppMenuSubmenu | null {
  for (const entry of entries) {
    if (entry.kind !== "submenu") continue
    if (entry.id === id) return entry
    const nested = findSubmenu(entry.entries, id)
    if (nested) return nested
  }
  return null
}
