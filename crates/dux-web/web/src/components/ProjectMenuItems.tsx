import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import {
  createProjectTerminal,
  openAttachWorktree,
  openCheckoutDefaultBranch,
  openCreateAgent,
  openCreateAgentFromPr,
  openDeleteProject,
  openProjectInfo,
  openProjectSettings,
  openProjectStartupLogs,
  openRemoveProject,
  pullProject,
  useDux,
} from "@/lib/store"
import {
  Bot,
  Download,
  FolderGit2,
  FolderX,
  GitBranch,
  GitPullRequest,
  Info,
  ScrollText,
  Settings,
  SquareTerminal,
  Trash2,
} from "lucide-react"

/**
 * The shared body of a project's actions dropdown, items only: the caller supplies
 * its own wrapper, since desktop and mobile anchor it differently. An orphaned
 * group shows only "Remove project…", which clears its ghost sessions, and
 * "New agent from PR…" is hidden where `gh` is unavailable.
 */
export function ProjectMenuItems({ id }: { id: string }) {
  const { spine, bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  const project = spine?.projects.find((p) => p.id === id)
  const orphaned = project === undefined

  return (
    <>
      {!orphaned && (
        <>
          <DropdownMenuItem onClick={() => openCreateAgent(id)}>
            <Bot />
            New agent…
          </DropdownMenuItem>
          {ghAvailable && (
            <DropdownMenuItem onClick={() => openCreateAgentFromPr(id)}>
              <GitPullRequest />
              New agent from PR…
            </DropdownMenuItem>
          )}
          {/* The per-project worktree manager: list, adopt one as an agent, or
             remove one. Labelled for what it opens rather than for adoption
             alone, which is now one action inside it. The global creation menu
             keeps its "New agent from existing worktree…" wording, because
             there the surface really is an agent-creation entry point that
             happens to route through a project picker. */}
          <DropdownMenuItem onClick={() => openAttachWorktree(id)}>
            <FolderGit2 />
            Worktrees…
          </DropdownMenuItem>
          {/* A project terminal: a plain shell at the project's repo root with
              no agent attached. Immediate action (no trailing "…"), mirroring
              the agent menu's own terminal entry; disabled when the project's
              path is missing on disk (there is no root to open a shell at).
              The label names the root rather than the ownership, because where
              the shell lands is what the reader cannot otherwise guess. */}
          <DropdownMenuItem
            disabled={project.path_missing}
            onClick={() => createProjectTerminal(id)}
          >
            <SquareTerminal />
            New terminal at the project root
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => pullProject(id)}>
            <Download />
            Pull project…
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => openCheckoutDefaultBranch(id)}>
            <GitBranch />
            Checkout default branch…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => openProjectInfo(id)}>
            <Info />
            Project info…
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => openProjectSettings(id)}>
            <Settings />
            Project settings…
          </DropdownMenuItem>
          {/* PROJECT scope of the startup-command log viewer: every run across
              every agent of this project. The agent row's ⋯ menu carries the
              AGENT scope as the plainer "Startup command logs…", so this one
              spells out how wide it is; the two must never read alike. Not
              destructive, so no confirmation. */}
          <DropdownMenuItem onClick={() => openProjectStartupLogs(id)}>
            <ScrollText />
            Startup command logs for all agents…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          {/* The destructive cascade (also deletes agents' worktrees on disk).
              Only for a real project: the DeleteProject command 404s on an
              orphaned ghost, and there is nothing on disk to cascade. Neutral
              color; the trailing "…" plus the confirm dialog are the danger
              signal. */}
          <DropdownMenuItem onClick={() => openDeleteProject(id)}>
            <FolderX />
            Delete project…
          </DropdownMenuItem>
        </>
      )}
      <DropdownMenuItem onClick={() => openRemoveProject(id)}>
        <Trash2 />
        Remove project…
      </DropdownMenuItem>
    </>
  )
}
