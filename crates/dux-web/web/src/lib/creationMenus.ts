// The SHARED item lists for the two creation menus: the "New" menu (ways to
// create an agent, plus the one terminal that belongs to nothing) and "more
// ways to add a project". Pure data, no React at module scope, in the same
// spirit as appMenu.ts.
//
// One definition, three presentations: the launcher corner's ⋯ menu
// (CreationOverflowMenuItems, rendered by the sidebar footer, the mobile hub
// and the collapsed rail alike) and the cog app menu's two creation submenus
// (appMenu.ts) all render these lists, so the surfaces cannot drift on labels,
// icons, order, gating, or the store action an item calls.
//
// The cog renders them VERBATIM; the launcher's ⋯ drops the one item its own
// filled verb already is (see NEW_AGENT_PLAIN_ID below) and regroups the rest
// under headings (see splitCreationGroups). Both are presentation, not a
// second definition.
//
// The entries carry the same `kind` tag the app menu's own entries do, so
// appMenu.ts can splice them in without re-authoring them and without this
// module importing appMenu types back (no module cycle).

import {
  Bot,
  FolderGit2,
  FolderOpen,
  FolderPlus,
  GitPullRequest,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react"

import {
  createStandaloneTerminal,
  openAddProject,
  openAddProjectForInit,
  openCreateAgentFromPr,
  openNewAgentPicker,
  openStandaloneAgentPicker,
} from "@/lib/store"

export interface CreationMenuAction {
  kind: "item"
  /** Stable, test-facing id; unique across BOTH lists, because appMenu.ts
   *  splices them into one tree whose ids must stay globally unique. */
  id: string
  /** Human title. Trailing "…" iff the item opens a dialog: every agent and
   *  project variant does, and the standalone terminal deliberately does not,
   *  because it opens the terminal immediately with nothing to confirm. */
  title: string
  icon: LucideIcon
  run: () => void
}

export interface CreationMenuSeparator {
  kind: "separator"
  /** Ids stay unique across the tree, separators included, so every renderer
   *  can key off `id` instead of inventing an index-based key. */
  id: string
}

/** A rendered row of a creation menu. Separator-only, with no group-label kind:
 *  neither cog renderer supports labels, so a heading would have to be invented
 *  in two places at once. The launcher's ⋯ adds ITS headings in the renderer
 *  (splitCreationGroups below), where the cog's submenu titles already say the
 *  same thing. A rule between the groups says enough here. */
export type CreationMenuItem = CreationMenuAction | CreationMenuSeparator

/** The id of the plain "New agent…" item. Exported because the launcher
 *  corner's filled verb IS that action, so the corner's ⋯ filters this one id
 *  out rather than offering the same click twice. Nowhere else may filter.
 *
 *  The filter is UNCONDITIONAL, including while the verb has flipped to "Add
 *  project" on an empty workspace: the ⋯ menu is deliberately constant so its
 *  rows never move under the cursor, which means the flipped state duplicates
 *  "Add project…" (verb and menu row) and hides nothing. Duplicating the row
 *  the verb already is costs a redundant row; moving rows around costs the
 *  user their muscle memory, and the flip is the transient state. The plain
 *  agent losing its last row in that one state is not a hole either: the flip
 *  happens exactly when there is no project to create an agent in. */
export const NEW_AGENT_PLAIN_ID = "new-agent-plain"

/**
 * The "New" menu, in the sidebar's order: the agent-creation variants, then a
 * rule, then the standalone terminal.
 *
 * Each agent variant opens the same picker armed with an intent (the user picks
 * the target project first); the from-PR variant is the exception: it opens its
 * dialog directly with no project, because the reference leads and dux resolves
 * the project from it. That variant is gated on GitHub / `gh` availability,
 * matching the launcher corner's `⋯` menu and the per-project `⋯` menu.
 *
 * The standalone terminal lives here rather than in a row's own `⋯` menu
 * because it is GLOBAL and parameter-free: it needs no agent, no project and
 * nothing selected (it is the twin of the TUI's `new-standalone-terminal`
 * palette command). Keeping it in this one list gives it the same home on
 * every surface, the cog menu included, which splices this list wholesale.
 */
export function newMenuItems(ctx: {
  ghAvailable: boolean
}): CreationMenuItem[] {
  const items: CreationMenuItem[] = [
    {
      kind: "item",
      id: NEW_AGENT_PLAIN_ID,
      title: "New agent…",
      icon: Bot,
      run: () => openNewAgentPicker("new"),
    },
  ]
  if (ctx.ghAvailable) {
    items.push({
      kind: "item",
      id: "new-agent-from-pr",
      title: "New agent from PR…",
      icon: GitPullRequest,
      run: () => openCreateAgentFromPr(null),
    })
  }
  items.push(
    {
      kind: "item",
      id: "new-agent-from-worktree",
      title: "New agent from existing worktree…",
      icon: FolderGit2,
      run: () => openNewAgentPicker("from_worktree"),
    },
    // A standalone agent is still an AGENT, so it sits with the agent variants
    // above the rule rather than beside the standalone terminal below it: the
    // rule separates the two kinds of thing, not the two "belongs to nothing"
    // ones. It is named to match the standalone terminal so a user who has met
    // one recognises the other.
    {
      kind: "item",
      id: "new-standalone-agent",
      title: "New standalone agent…",
      icon: FolderOpen,
      run: () => openStandaloneAgentPicker(),
    },
    { kind: "separator", id: "sep-new-terminals" },
    {
      kind: "item",
      id: "new-standalone-terminal",
      // The location is spelled out for the same reason the agent and project
      // entries spell theirs out: a menu item that opens a shell has to say
      // where it lands, and home is the one answer nobody guesses.
      title: "New standalone terminal in your home folder",
      icon: SquareTerminal,
      run: () => createStandaloneTerminal(),
    },
  )
  return items
}

/**
 * A creation list split at its separators into the chunks a labeled menu wants,
 * separator rows dropped. The launcher corner's ⋯ renders each chunk under its
 * own heading ("Agents", then "Terminals"), so the headings stay presentation
 * and this stays the one list.
 *
 * Partitioning rather than filtering by id: a data change that moves an item
 * across the rule moves it in the labeled menu too, with nothing to keep in
 * step. Empty chunks are KEPT so a chunk's position is stable no matter what a
 * gate hides (the from-PR variant with `gh` unavailable); the renderer decides
 * whether a heading with no rows under it is worth drawing.
 */
export function splitCreationGroups(
  items: CreationMenuItem[],
): CreationMenuAction[][] {
  const groups: CreationMenuAction[][] = [[]]
  for (const entry of items) {
    if (entry.kind === "separator") {
      groups.push([])
      continue
    }
    groups[groups.length - 1].push(entry)
  }
  return groups
}

/**
 * The add-project variants, in the sidebar's order. Both open the SAME picker;
 * the intent only changes a header hint, and the primary-action ladder decides
 * the real action from the server's inspection, so "Initialize a repository…"
 * stays discoverable in the menu while remaining reachable through the plain
 * picker too.
 */
export function addProjectMenuItems(): CreationMenuItem[] {
  return [
    {
      kind: "item",
      id: "add-project-picker",
      title: "Add project…",
      icon: FolderGit2,
      run: () => openAddProject(),
    },
    {
      kind: "item",
      id: "init-repository",
      title: "Initialize a repository…",
      icon: FolderPlus,
      run: () => openAddProjectForInit(),
    },
  ]
}
