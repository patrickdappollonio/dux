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

export function RemoveProjectDialog() {
  const { removeProjectTarget, viewModel } = useDux()

  const isOpen = removeProjectTarget !== null
  const project = viewModel?.projects.find((p) => p.id === removeProjectTarget)
  const name = project?.name ?? "this project"

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
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Remove project?</DialogTitle>
          <DialogDescription>
            This removes &ldquo;{name}&rdquo; from the workspace. Worktrees on
            disk are not deleted.
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
