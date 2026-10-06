import {
  AttachedSection,
  GuardedConfirmButton,
} from "@/components/AttachedSection"
import { useAttachedOverride } from "@/hooks/use-attached-override"
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
import { deleteProjectProse } from "@/lib/projectConfirm"
import { renderProse } from "@/lib/prose"

// The destructive cascade counterpart to `RemoveProjectDialog`: it also deletes
// every agent's worktree from disk, so the copy spells that out. Offered only for
// a real project, so its open state goes through the vanish guard and the dialog
// closes itself rather than acting on a target the ViewModel no longer has.
export function DeleteProjectDialog() {
  const { deleteProjectTarget, spine } = useDux()

  const project = spine?.projects.find((p) => p.id === deleteProjectTarget)
  const isOpen = useVanishedTargetGuard(
    deleteProjectTarget !== null,
    project !== undefined,
    closeDeleteProject,
  )
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(isOpen)
  const name = project?.name
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === deleteProjectTarget,
    ).length ?? 0

  async function handleConfirm() {
    if (!deleteProjectTarget) return
    const target = deleteProjectTarget
    const done = await confirm((force) => deleteProject(target, force))
    if (done) closeDeleteProject()
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
            {renderProse(deleteProjectProse(name, agentCount))}
          </DialogDescription>
        </DialogHeader>
        <AttachedSection blockers={blockers} />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeDeleteProject}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Delete"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
