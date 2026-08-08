// The SHARED item lists for the two creation menus: "more ways to create an
// agent" and "more ways to add a project". Pure data, no React at module
// scope, in the same spirit as appMenu.ts.
//
// One definition, three presentations: the sidebar's split-button dropdowns
// (NewAgentSplitButton, AddProjectMenuItems) and the cog app menu's creation
// submenus (appMenu.ts) all render these lists, so the surfaces cannot drift
// on labels, icons, order, gating, or the store action an item calls.
//
// The items deliberately match the AppMenuItem shape minus the `kind` tag
// (appMenu.ts adds it), so appMenu.ts can embed them without re-authoring and
// without this module importing appMenu types back (no module cycle).

import {
  Bot,
  FolderGit2,
  FolderPlus,
  GitPullRequest,
  type LucideIcon,
} from "lucide-react"

import {
  openAddProject,
  openAddProjectForInit,
  openCreateAgentFromPr,
  openNewAgentPicker,
} from "@/lib/store"

export interface CreationMenuItem {
  /** Stable, test-facing id; unique across BOTH lists, because appMenu.ts
   *  splices them into one tree whose ids must stay globally unique. */
  id: string
  /** Human title. Every creation variant opens a dialog, so all carry "…". */
  title: string
  icon: LucideIcon
  run: () => void
}

/**
 * The agent-creation variants, in the sidebar's order. Each opens the same
 * picker armed with an intent (the user picks the target project first); the
 * from-PR variant is the exception: it opens its dialog directly with no
 * project, because the reference leads and dux resolves the project from it.
 * That variant is gated on GitHub / `gh` availability, matching the sidebar
 * split button and the per-project `⋯` menu.
 */
export function newAgentMenuItems(ctx: {
  ghAvailable: boolean
}): CreationMenuItem[] {
  const items: CreationMenuItem[] = [
    {
      id: "new-agent-plain",
      title: "New agent…",
      icon: Bot,
      run: () => openNewAgentPicker("new"),
    },
  ]
  if (ctx.ghAvailable) {
    items.push({
      id: "new-agent-from-pr",
      title: "New agent from PR…",
      icon: GitPullRequest,
      run: () => openCreateAgentFromPr(null),
    })
  }
  items.push({
    id: "new-agent-from-worktree",
    title: "New agent from existing worktree…",
    icon: FolderGit2,
    run: () => openNewAgentPicker("from_worktree"),
  })
  return items
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
      id: "add-project-picker",
      title: "Add project…",
      icon: FolderGit2,
      run: () => openAddProject(),
    },
    {
      id: "init-repository",
      title: "Initialize a repository…",
      icon: FolderPlus,
      run: () => openAddProjectForInit(),
    },
  ]
}
