// The shared item lists behind every creation surface: ways to create an agent
// (plus the one terminal that belongs to nothing), and ways to add a project.
// Pure data, no React at module scope.
//
// Every surface renders these lists, so none of them can drift on labels, icons,
// order, gating, or the store action an item calls. The cog menu renders them
// verbatim; the launcher's New dialog and the launcher corner's menu regroup
// them under captions and word each one in a sentence (`newLauncherModel`),
// which is presentation.
//
// The entries carry the same `kind` tag the app menu's own do, so `appMenu.ts`
// splices them in without importing anything back (no module cycle).

import {
  Bot,
  CloudDownload,
  FolderGit2,
  FolderOpen,
  FolderPlus,
  GitBranchPlus,
  GitPullRequest,
  SquareTerminal,
  Terminal,
  type LucideIcon,
} from "lucide-react"

import {
  createStandaloneTerminal,
  openAddProject,
  openAddProjectForInit,
  openCloneProject,
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
 *  submenu titles already say what a heading would, and the New dialog adds its
 *  own captions through `newLauncherModel`. */
export type CreationMenuItem = CreationMenuAction | CreationMenuSeparator

/** The id of the plain "New agent…" item. */
export const NEW_AGENT_PLAIN_ID = "new-agent-plain"

/** What starting a standalone agent is: its icon and the store action that
 *  opens its folder picker. The "New standalone agent…" item and the New agent
 *  picker's footer both read it, so the second door cannot grow a second flow
 *  or a different glyph. */
export const STANDALONE_AGENT_CREATION = {
  icon: FolderOpen,
  run: () => openStandaloneAgentPicker(),
} as const satisfies { icon: LucideIcon; run: () => void }

/**
 * The "New" menu, in the sidebar's order: the agent-creation variants, then a
 * rule, then the standalone terminal.
 *
 * Each agent variant opens the same picker armed with an intent, except the
 * from-PR one, which opens its dialog with no project because the reference
 * leads and dux resolves the project from it. It is gated on `gh` availability,
 * matching the per-project `⋯` menus.
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
      ...STANDALONE_AGENT_CREATION,
    },
    { kind: "separator", id: "sep-new-terminals" },
    {
      kind: "item",
      id: "new-standalone-terminal",
      // The location is spelled out because a menu item that opens a shell has
      // to say where it lands, and home is the answer nobody guesses.
      title: "New standalone terminal in your home folder",
      icon: Terminal,
      run: () => createStandaloneTerminal(),
    },
  )
  return items
}

/**
 * A creation list split at its separators into chunks, separator rows dropped,
 * so grouping stays presentation and this stays one list.
 *
 * Partitioning rather than filtering by id, so moving an item across the rule
 * moves it in the grouped presentation too. Empty chunks are kept, so a chunk's
 * position is stable whatever a gate hides.
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
 * The add-project variants, in the sidebar's order. "Add project" and
 * "Initialize a repository" open the same picker: the intent only changes a
 * header hint, and the real action comes from the server's inspection, so either
 * row reaches either outcome. "Clone a repository" is the odd one out: it starts
 * from a remote address rather than a folder, so it opens its own dialog.
 */
export function addProjectMenuItems(): CreationMenuItem[] {
  return [
    {
      kind: "item",
      id: "add-project-picker",
      title: "Add project…",
      icon: FolderPlus,
      run: () => openAddProject(),
    },
    {
      kind: "item",
      id: "clone-repository",
      title: "Clone a repository…",
      icon: CloudDownload,
      run: () => openCloneProject(),
    },
    {
      kind: "item",
      id: "init-repository",
      title: "Initialize a repository…",
      icon: GitBranchPlus,
      run: () => openAddProjectForInit(),
    },
  ]
}

/** The New dialog's one row that is not in the shared lists: it opens the
 *  agent-picking step (inside the dialog, or as a dialog of its own from the
 *  launcher corner's menu) rather than a store action. */
export const NEW_AGENT_TERMINAL_ID = "new-terminal-for-agent"

/** What a New dialog row does when activated: run a shared list's own action,
 *  or open the dialog's agent-picker step. */
export type NewLauncherAction =
  | { kind: "run"; run: () => void }
  | { kind: "pick-agent" }

export interface NewLauncherRow {
  id: string
  title: string
  /** One plain sentence saying what the row makes, for someone who has not met
   *  the word yet. */
  excerpt: string
  /** The title as a menu entry: a trailing "…" iff a dialog follows. */
  menuTitle: string
  /** One short sentence for the launcher corner's menu, where the row sits
   *  under its title in a narrow popup. Null for the one row the menu does not
   *  list, because the corner's main half is that row. */
  summary: string | null
  icon: LucideIcon
  /** True when activating it opens a further step, which earns a chevron. */
  opensStep: boolean
  disabled: boolean
  action: NewLauncherAction
}

export interface NewLauncherGroup {
  id: "agents" | "terminals" | "projects"
  title: string
  caption: string
  /** A sentence leading the group, said only when it is the next thing to do. */
  lead: string | null
  rows: NewLauncherRow[]
}

// The launcher's own words for the shared items, keyed by their ids: the New
// dialog's title and sentence, and the shorter sentence the corner's menu shows.
// The titles drop the menus' trailing "…" because the dialog's chevron already
// says a step follows; `menuTitle` puts it back for the menu.
const LAUNCHER_COPY: Record<
  string,
  { title: string; excerpt: string; summary?: string }
> = {
  [NEW_AGENT_PLAIN_ID]: {
    title: "New agent",
    excerpt:
      "Start an AI coding tool on one of your projects. It gets its own copy of the code on a new branch, so your work stays untouched.",
  },
  "new-agent-from-pr": {
    title: "New agent from PR",
    excerpt:
      "Pick up a GitHub pull request. dux fetches its branch and starts an agent on it, ready to review or carry on.",
    summary: "Start an agent on a GitHub pull request's branch.",
  },
  "new-agent-from-worktree": {
    title: "New agent from existing worktree",
    excerpt:
      "Start an agent in a copy of a project you already have on disk, on the branch it's already on.",
    summary: "Use a copy of a project already on disk, on its branch.",
  },
  "new-standalone-agent": {
    title: "New standalone agent",
    excerpt:
      "Start an agent in any folder on this computer. No project or git needed.",
    summary: "Run an agent in any folder, no project or git needed.",
  },
  "new-standalone-terminal": {
    title: "New standalone terminal",
    excerpt:
      "Open a command line in your home folder, not tied to any project. Opens right away.",
    summary: "Opens right away, in your home folder.",
  },
  "add-project-picker": {
    title: "Add project",
    excerpt: "Point dux at a git repository that's already on this computer.",
    summary: "Point dux at a git repository on this computer.",
  },
  "clone-repository": {
    title: "Clone a repository",
    excerpt:
      "Download a repository from GitHub or any git address, then add it as a project.",
    summary: "Download a repository, then add it as a project.",
  },
  "init-repository": {
    title: "Initialize a repository",
    excerpt:
      "Turn a folder into a brand-new git repository and add it as a project.",
    summary: "Turn a folder into a new git repository and project.",
  },
}

// The agent variants that pick a project first, and so have nothing to offer in
// a workspace with none.
const NEEDS_A_PROJECT = new Set([
  NEW_AGENT_PLAIN_ID,
  "new-agent-from-pr",
  "new-agent-from-worktree",
])

/**
 * The New dialog's groups, built from the shared lists so every row runs the
 * very action the cog menu's copy of it does.
 *
 * `projectCount` and `agentCount` are null until the spine arrives, and an
 * unloaded list is not a zero: only a confirmed zero leads with Projects and
 * disables the rows that would open a picker with nothing in it.
 */
export function newLauncherModel(ctx: {
  ghAvailable: boolean
  projectCount: number | null
  agentCount: number | null
}): NewLauncherGroup[] {
  const noProjects = ctx.projectCount === 0
  const noAgents = ctx.agentCount === 0
  const row = (entry: CreationMenuAction): NewLauncherRow => {
    const copy = LAUNCHER_COPY[entry.id]
    if (!copy) throw new Error(`New dialog has no words for ${entry.id}`)
    // The shared lists' own contract: a trailing "…" means a dialog follows.
    const opensStep = entry.title.endsWith("…")
    return {
      id: entry.id,
      ...copy,
      summary: copy.summary ?? null,
      menuTitle: opensStep ? `${copy.title}…` : copy.title,
      icon: entry.icon,
      opensStep,
      disabled: noProjects && NEEDS_A_PROJECT.has(entry.id),
      action: { kind: "run", run: entry.run },
    }
  }
  const [agentItems = [], terminalItems = []] = splitCreationGroups(
    newMenuItems({ ghAvailable: ctx.ghAvailable }),
  )
  const projectItems = addProjectMenuItems().filter(
    (entry): entry is CreationMenuAction => entry.kind === "item",
  )
  const agents: NewLauncherGroup = {
    id: "agents",
    title: "Agents",
    caption:
      "AI coding tools, like Claude Code or Codex, each in its own terminal",
    lead: null,
    rows: agentItems.map(row),
  }
  const terminals: NewLauncherGroup = {
    id: "terminals",
    title: "Terminals",
    caption: "A plain command line, like your terminal app",
    lead: null,
    rows: [
      {
        id: NEW_AGENT_TERMINAL_ID,
        title: "New terminal for an agent",
        excerpt:
          "Open a command line in an agent's copy of the code, to run its tests or look around beside it.",
        menuTitle: "New terminal for an agent…",
        summary: "Open a command line in an agent's copy of the code.",
        icon: SquareTerminal,
        opensStep: true,
        disabled: noAgents,
        action: { kind: "pick-agent" },
      },
      ...terminalItems.map(row),
    ],
  }
  const projects: NewLauncherGroup = {
    id: "projects",
    title: "Projects",
    caption: "Git repositories dux knows about; agents work on copies of them",
    lead: noProjects ? "Add a project first, then start agents on it." : null,
    rows: projectItems.map(row),
  }
  return noProjects
    ? [projects, agents, terminals]
    : [agents, terminals, projects]
}

/** The live state `newLauncherModel` reads from. */
interface LauncherState {
  bootstrap: { gh_available?: boolean } | null
  spine: { projects: unknown[]; sessions: unknown[] } | null
}

// Each input on its own, as a primitive, so a component can subscribe to just
// these through `useDuxSelector` and re-render only when one of them changes.
// The counts stay null until the spine arrives.
export const selectGhAvailable = (state: LauncherState): boolean =>
  state.bootstrap?.gh_available ?? false
export const selectProjectCount = (state: LauncherState): number | null =>
  state.spine ? state.spine.projects.length : null
export const selectAgentCount = (state: LauncherState): number | null =>
  state.spine ? state.spine.sessions.length : null

/** What `newLauncherModel` reads from the live state: `gh` availability and the
 *  two counts. */
export function newLauncherContext(
  state: LauncherState,
): Parameters<typeof newLauncherModel>[0] {
  return {
    ghAvailable: selectGhAvailable(state),
    projectCount: selectProjectCount(state),
    agentCount: selectAgentCount(state),
  }
}

/**
 * The launcher corner's two halves, from the New dialog's own groups: the main
 * half is the plain "New agent" row, and the menu is every other row, under the
 * same groups, in the same order, with the same gating.
 */
export function launcherCornerModel(groups: NewLauncherGroup[]): {
  primary: NewLauncherRow
  menu: NewLauncherGroup[]
} {
  const primary = groups
    .flatMap((group) => group.rows)
    .find((row) => row.id === NEW_AGENT_PLAIN_ID)
  if (!primary) throw new Error("The launcher has no plain New agent row")
  const menu = groups
    .map((group) => ({
      ...group,
      rows: group.rows.filter((row) => row.id !== NEW_AGENT_PLAIN_ID),
    }))
    .filter((group) => group.rows.length > 0)
  return { primary, menu }
}
