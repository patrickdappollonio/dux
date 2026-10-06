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
import { removeProjectProse } from "@/lib/projectConfirm"
import { renderProse } from "@/lib/prose"
import { closeRemoveProject, removeProject, useDux } from "@/lib/store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

export function RemoveProjectDialog() {
  const { removeProjectTarget, spine } = useDux()

  // Deliberately skips useVanishedTargetGuard: this dialog opens for ghost
  // projects with no live project record, so a vanish guard would dismiss the
  // exact case it exists for.
  const isOpen = removeProjectTarget !== null
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(isOpen)
  const project = spine?.projects.find((p) => p.id === removeProjectTarget)
  // For an orphaned ("ghost") project there is no project record, so fall back
  // to the short-id name the sidebar shows for its group.
  const orphanName = spine?.sidebar.groups.find(
    (g) => g.project_id === removeProjectTarget,
  )?.name
  const name = project?.name ?? orphanName
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === removeProjectTarget,
    ).length ?? 0

  async function handleConfirm() {
    if (!removeProjectTarget) return
    const target = removeProjectTarget
    const done = await confirm((force) => removeProject(target, force))
    if (done) closeRemoveProject()
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
            {renderProse(removeProjectProse(name, agentCount))}
          </DialogDescription>
        </DialogHeader>
        <AttachedSection blockers={blockers} />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeRemoveProject}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Remove"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
