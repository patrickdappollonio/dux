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
  ArrowDownAZ,
  ArrowUpDown,
  Braces,
  CalendarPlus,
  Clock,
  FileCode,
  Globe,
  OctagonX,
  RefreshCw,
  SlidersHorizontal,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import { toast } from "sonner"

import { configApi } from "@/lib/configApi"
import {
  openConfigEditor,
  openCustomizeWebapp,
  openGlobalEnv,
  openKillRunning,
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

/**
 * The app menu, top level first.
 *
 * Takes no context today: every entry is unconditional. Resist adding gating
 * without a real reason: an entry that appears and disappears is harder to
 * learn than one that is always there and explains itself when used.
 */
export function appMenuModel(): AppMenuEntry[] {
  return [
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
          icon: Braces,
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
      id: "stop-running-agents",
      // Neutral, not destructive-tinted: the trailing "…" plus the dialog's own
      // confirmation are the danger signal (CLAUDE.md menu tenet).
      title: "Stop running agents…",
      icon: OctagonX,
      run: () => openKillRunning(),
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
