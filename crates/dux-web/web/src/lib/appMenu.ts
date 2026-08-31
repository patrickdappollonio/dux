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
  PartyPopper,
  ArrowUpDown,
  CalendarPlus,
  Clock,
  FileCode,
  FolderGit2,
  GitPullRequestArrow,
  Globe,
  Plus,
  RefreshCw,
  Rocket,
  SlidersHorizontal,
  SquarePen,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import { notifyError } from "./notify"

import { configApi } from "@/lib/configApi"
import { addProjectMenuItems, newMenuItems } from "@/lib/creationMenus"
import {
  openConfigEditor,
  openReleaseNotes,
  openWelcomeScreen,
  openCustomizeWebapp,
  openGlobalEnv,
  openTaskManager,
  openMacrosDialog,
  sortAgents,
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
   *  Gates the from-PR agent variant, exactly as the launcher corner's `⋯`
   *  menu and the per-project `⋯` menu gate theirs. */
  ghAvailable: boolean
  /** Whether the integration is switched ON (`bootstrap.github_integration`),
   *  which is a different question from whether `gh` currently works. Gates
   *  "Re-check GitHub", which exists precisely for when `gh` does NOT work and
   *  so must never be gated on `ghAvailable`. */
  githubIntegrationEnabled: boolean
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
  // The creation submenus mirror the launcher corner's `⋯` menu: their
  // entries are the shared lists in creationMenus.ts, spliced into the cog
  // verbatim (appMenu.test.ts pins this; the corner renders the same lists
  // grouped under headings and minus the one item its own verb already is,
  // which is presentation, not a second list). The shared
  // entries carry the same `kind` tag this tree uses, items and separators
  // alike, so the splice is a plain assignment: the annotation, not a cast,
  // is what makes the compiler own the "same shape" claim, so a future
  // divergence between the two unions fails here instead of rendering wrong.
  const asEntries = (
    items: ReturnType<typeof addProjectMenuItems>,
  ): AppMenuEntry[] => items
  return [
    // The two creation submenus OPEN the menu: creating something or adding a
    // project is the most common reason to reach for the cog, so they
    // outrank Preferences. They are the menu's twins of the launcher
    // corner's grouped `⋯` menu; their entries are
    // the shared lists spliced in verbatim (see asEntries above). No trailing
    // "…" on the submenu titles: a submenu opens a list, not a dialog; the
    // "…" lives on the variants inside.
    //
    // Titled "New" and iconed Plus rather than "New agent" / Bot: the list
    // carries the standalone terminal too, so a Bot would promise agents only.
    // This submenu is the standalone terminal's ONE home in the cog menu; it
    // used to also sit at the top level, and that duplicate is deliberately
    // gone.
    {
      kind: "submenu",
      id: "new-agent",
      title: "New",
      icon: Plus,
      entries: asEntries(newMenuItems(ctx)),
    },
    {
      kind: "submenu",
      id: "add-project",
      title: "Add project",
      icon: FolderGit2,
      entries: asEntries(addProjectMenuItems()),
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
                notifyError(
                  e instanceof Error ? e.message : "Could not reload the config.",
                ),
              )
          },
        },
        // No ellipsis: it runs immediately. Present whenever the integration is
        // ON, and deliberately NOT gated on gh currently working, because the
        // state it exists to escape is exactly the one where gh is not working.
        // Restarting dux would clear a stale gh answer too, and would take every
        // running agent with it.
        ...(ctx.githubIntegrationEnabled
          ? ([
              {
                kind: "item",
                id: "recheck-github",
                title: "Re-check GitHub",
                icon: GitPullRequestArrow,
                run: () => {
                  configApi
                    .recheckGithub()
                    .catch((e) =>
                      notifyError(
                        e instanceof Error
                          ? e.message
                          : "Could not ask the server to re-check the GitHub CLI.",
                      ),
                    )
                },
              },
            ] as AppMenuEntry[])
          : []),
      ],
    },
    // Still here now that the standalone terminal has moved into the "New"
    // submenu: this rule separates Configuration from the Task Manager.
    { kind: "separator", id: "sep-agents" },
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
