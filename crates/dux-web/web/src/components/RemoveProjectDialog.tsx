import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { closeRemoveProject, removeProject, useDux } from "@/lib/store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

export function RemoveProjectDialog() {
  const { removeProjectTarget, spine } = useDux()

  // Deliberately skips useVanishedTargetGuard: this dialog opens for ghost
  // projects with no live project record, so a vanish guard would dismiss the
  // exact case it exists for.
  const isOpen = removeProjectTarget !== null
  const project = spine?.projects.find((p) => p.id === removeProjectTarget)
  // For an orphaned ("ghost") project there is no project record, so fall back
  // to the short-id name the sidebar shows for its group.
  const orphanName = spine?.sidebar.groups.find(
    (g) => g.project_id === removeProjectTarget,
  )?.name
  const name = project?.name ?? orphanName ?? "this project"
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === removeProjectTarget,
    ).length ?? 0

  function handleConfirm() {
    if (!removeProjectTarget) return
    removeProject(removeProjectTarget)
    closeRemoveProject()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeRemoveProject()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Remove project?</DialogTitle>
          <DialogDescription>
            This removes &ldquo;{name}&rdquo;
            {agentCount > 0
              ? ` and deletes its ${agentCount} agent${agentCount === 1 ? "" : "s"}`
              : ""}{" "}
            from dux. Worktrees on disk are kept.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={closeRemoveProject}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Remove
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
