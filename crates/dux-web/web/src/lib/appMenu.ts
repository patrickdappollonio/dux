// The definition of the web UI's app menu, rendered by the desktop cog flyout
// and the mobile hub's bottom sheet alike. The rules it lives under:
//
// - This file is the source of truth for the menu. `dux_core::palette` is the
//   TUI palette's, and nothing holds the two together, so a new palette command
//   warrants an entry here only by a deliberate decision (see CLAUDE.md).
// - The menu has no keyboard shortcut: it is reached by Tab and driven with
//   Enter, Space and the arrow keys.
// - The module is pure data, so the menu is assertable without mounting React,
//   and neither renderer hand-authors items.
// - Global actions and dialogs belong here; preferences do not. A preference is
//   a row in `settingsDescriptors.ts`, and a per-entity action lives in that
//   entity's own `⋯` menu.

import {
  Activity,
  ArrowDownAZ,
  PartyPopper,
  ArrowUpDown,
  CalendarPlus,
  Clock,
  FileCode,
  FolderGit2,
  FolderKanban,
  GitPullRequestArrow,
  Globe,
  LogOut,
  Plus,
  RefreshCw,
  Rocket,
  SlidersHorizontal,
  SquarePen,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import { notifyError } from "@/lib/notify"

import { signOut } from "@/lib/authGate"
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
  openProjects,
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
  /** Whether `gh` is usable, which gates the from-PR agent variant here as it
   *  does in the launcher corner's and the per-project `⋯` menus. */
  ghAvailable: boolean
  /** Whether the integration is switched on, a different question from whether
   *  `gh` works. It gates "Re-check GitHub", which exists for when `gh` does
   *  not work and so must never be gated on `ghAvailable`. */
  githubIntegrationEnabled: boolean
  /** Whether this browser holds a session it can end: a password is set and
   *  this page signed in with it. Absent means no, because on a server with no
   *  password, or a connection the password does not apply to, there is nothing
   *  to sign out of and an entry that did nothing would be a lie. */
  canSignOut?: boolean
}

/**
 * The app menu, top level first. Resist gating entries on the context without a
 * real reason: an entry that appears and disappears is harder to learn than one
 * that is always there and explains itself when used.
 */
export function appMenuModel(ctx: AppMenuContext): AppMenuEntry[] {
  // The creation submenus splice in the shared lists from `creationMenus.ts`
  // verbatim, the same ones the launcher corner renders. An annotation rather
  // than a cast, so a future divergence between the two unions fails here
  // instead of rendering wrong.
  const asEntries = (
    items: ReturnType<typeof addProjectMenuItems>,
  ): AppMenuEntry[] => items
  return [
    // The creation submenus open the menu, outranking Preferences, because
    // creating something is the most common reason to reach for the cog. No
    // trailing "…" on a submenu title: it opens a list, and the "…" lives on
    // the variants inside. Titled "New" with a Plus rather than "New agent"
    // with a Bot, because the list carries the standalone terminal too, and
    // this is that terminal's one home in the cog menu.
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
    // Every project and orphaned group, each with its whole menu. Beside the
    // way to add one because it is the way to manage one: without it, a
    // project with no agents is reachable only through the New agent picker.
    {
      kind: "item",
      id: "projects",
      title: "Projects…",
      icon: FolderKanban,
      run: () => openProjects(),
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
      // Plain action items rather than radio items: `sortAgents` is a one-shot
      // reorder into the user's manual drag order, so there is no persisted sort
      // key a checkmark could read, and it would be false the moment a row is
      // dragged. Adding one means adding a real persisted sort mode first.
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
          // No ellipsis: it runs immediately. The engine's routed status reports
          // success, so only a failure needs a toast of dux's own.
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
        // No ellipsis: it runs immediately. Gated on the integration being on
        // and never on `gh` working, because the state it exists to escape is
        // exactly the one where `gh` does not work.
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
    // Separates Configuration from the Task Manager.
    { kind: "separator", id: "sep-agents" },
    {
      kind: "item",
      id: "task-manager",
      // Neutral rather than destructive-tinted: the trailing "…" and the
      // dialog's own confirmations are the danger signal. `Activity` rather than
      // `OctagonX`, because the surface is one you mostly read.
      title: "Task Manager…",
      icon: Activity,
      run: () => openTaskManager(),
    },
    { kind: "separator", id: "sep-about" },
    // Actions rather than preferences: their `ui.disable_*` counterparts are
    // Preferences rows, and those flags suppress only the automatic screens, so
    // these entries keep working however they are set.
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
    // Last, behind its own separator, so it is never the item a hurried click
    // lands on. No ellipsis and no confirm: it loses nothing (the URL and every
    // unsaved draft stay in this page and come back with the next sign-in), and
    // the login page that replaces the app says what happened.
    ...(ctx.canSignOut
      ? ([
          { kind: "separator", id: "sep-sign-out" },
          {
            kind: "item",
            id: "sign-out",
            title: "Sign out",
            icon: LogOut,
            run: () => {
              void signOut().then((answer) => {
                if (answer.kind === "unreachable") {
                  notifyError(
                    "Could not reach dux to sign out, so this browser is still signed in. Try again once dux is reachable.",
                  )
                } else if (answer.kind === "refused") {
                  notifyError(answer.message)
                }
              })
            },
          },
        ] as AppMenuEntry[])
      : []),
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
