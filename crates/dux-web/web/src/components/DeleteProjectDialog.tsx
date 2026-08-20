import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeDeleteProject, deleteProject, useDux } from "@/lib/store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

// The destructive cascade counterpart to RemoveProjectDialog: this variant also
// deletes every agent's worktree from disk, so its copy spells that out. Unlike
// RemoveProjectDialog (which opens for ghost/orphaned projects with no live
// record), this is only offered for a real project, so it routes its open state
// through the vanish guard — if the project disappears from the live ViewModel
// mid-open, the dialog closes itself rather than acting on a stale target.
export function DeleteProjectDialog() {
  const { deleteProjectTarget, spine } = useDux()

  const project = spine?.projects.find((p) => p.id === deleteProjectTarget)
  const isOpen = useVanishedTargetGuard(
    deleteProjectTarget !== null,
    project !== undefined,
    closeDeleteProject,
  )
  const name = project?.name ?? "this project"
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === deleteProjectTarget,
    ).length ?? 0
  // Name the agents and their worktrees only when there are any; a project with
  // no agents has no worktrees to mention.
  const cascadeClause =
    agentCount > 0
      ? `, its ${agentCount} agent${agentCount === 1 ? "" : "s"}, and ${
          agentCount === 1 ? "its worktree" : "their worktrees"
        } on disk`
      : ""

  function handleConfirm() {
    if (!deleteProjectTarget) return
    deleteProject(deleteProjectTarget)
    closeDeleteProject()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDeleteProject()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Delete project?</DialogTitle>
          <DialogDescription>
            This deletes &ldquo;{name}&rdquo;{cascadeClause} from dux. This is
            irreversible. The source checkout is kept.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeDeleteProject}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
