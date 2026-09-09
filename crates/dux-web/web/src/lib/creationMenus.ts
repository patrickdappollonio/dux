// The shared item lists behind every creation menu: ways to create an agent
// (plus the one terminal that belongs to nothing), and ways to add a project.
// Pure data, no React at module scope.
//
// Every surface renders these lists, so none of them can drift on labels, icons,
// order, gating, or the store action an item calls. The cog menu renders them
// verbatim; the launcher corner's ⋯ drops the one item its own filled verb
// already is and regroups the rest under headings, which is presentation.
//
// The entries carry the same `kind` tag the app menu's own do, so `appMenu.ts`
// splices them in without importing anything back (no module cycle).

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

/** A rendered row of a creation menu. There is no group-label kind: the cog's
 *  submenu titles already say what a heading would, and the launcher's ⋯ adds
 *  its own headings in the renderer through `splitCreationGroups`. */
export type CreationMenuItem = CreationMenuAction | CreationMenuSeparator

/** The id of the plain "New agent…" item. The launcher corner's filled verb is
 *  that action, so the corner's ⋯ filters this one id out rather than offering
 *  the same click twice; nowhere else may filter.
 *
 *  The filter is unconditional, the flipped "Add project" verb included: the ⋯
 *  menu stays constant so its rows never move under the cursor. */
export const NEW_AGENT_PLAIN_ID = "new-agent-plain"

/**
 * The "New" menu, in the sidebar's order: the agent-creation variants, then a
 * rule, then the standalone terminal.
 *
 * Each agent variant opens the same picker armed with an intent, except the
 * from-PR one, which opens its dialog with no project because the reference
 * leads and dux resolves the project from it. It is gated on `gh` availability,
 * matching the launcher corner's and the per-project `⋯` menus.
 *
 * The standalone terminal belongs in this list rather than a row's `⋯` menu
 * because it is global and parameter-free, and this list is what gives it the
 * same home on every surface.
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
    // A standalone agent is still an agent, so it sits above the rule with the
    // other agent variants: the rule separates agents from terminals, not the
    // two things that belong to nothing.
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
      // The location is spelled out because a menu item that opens a shell has
      // to say where it lands, and home is the answer nobody guesses.
      title: "New standalone terminal in your home folder",
      icon: SquareTerminal,
      run: () => createStandaloneTerminal(),
    },
  )
  return items
}

/**
 * A creation list split at its separators into the chunks a labeled menu wants,
 * separator rows dropped, so headings stay presentation and this stays one list.
 *
 * Partitioning rather than filtering by id, so moving an item across the rule
 * moves it in the labeled menu too. Empty chunks are kept, so a chunk's position
 * is stable whatever a gate hides; the renderer decides whether to draw a
 * heading with no rows under it.
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
 * The add-project variants, in the sidebar's order. Both open the same picker:
 * the intent only changes a header hint, and the real action comes from the
 * server's inspection, so either row reaches either outcome.
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
