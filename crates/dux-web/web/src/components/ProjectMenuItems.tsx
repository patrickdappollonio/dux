import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import {
  openAttachWorktree,
  openCheckoutDefaultBranch,
  openCreateAgent,
  openCreateAgentFromPr,
  openProjectInfo,
  openProjectSettings,
  openRemoveProject,
  pullProject,
  useDux,
} from "@/lib/store"
import {
  Bot,
  Download,
  FolderGit2,
  GitBranch,
  GitPullRequest,
  Info,
  Settings,
  Trash2,
} from "lucide-react"

/**
 * The shared body of a project's actions dropdown, rendered by both the desktop
 * sidebar and the mobile shell so the two menus never drift. The caller supplies
 * its own <DropdownMenuContent> wrapper (desktop and mobile anchor it
 * differently); this renders only the items.
 *
 * An orphaned group (a session whose project record is gone) has no real
 * project to act on — most actions would 404 on the server — so its menu shows
 * only "Remove project…", which clears the ghost's orphaned sessions. The
 * "New agent from PR…" item is hidden when GitHub integration / `gh` is
 * unavailable, mirroring the TUI (which gates `new-agent-from-pr` the same way;
 * the server also rejects the command in that state).
 */
export function ProjectMenuItems({ id }: { id: string }) {
  const { spine, bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  const orphaned = !spine?.projects.some((p) => p.id === id)

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
          <DropdownMenuItem onClick={() => openAttachWorktree(id)}>
            <FolderGit2 />
            Attach worktree…
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
          <DropdownMenuSeparator />
        </>
      )}
      <DropdownMenuItem onClick={() => openRemoveProject(id)}>
        <Trash2 />
        Remove project…
      </DropdownMenuItem>
    </>
  )
}
