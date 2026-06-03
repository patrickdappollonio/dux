import { useState } from "react"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

export function DeleteSessionDialog() {
  const { deleteTarget, viewModel } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)

  const isOpen = deleteTarget !== null
  const session = viewModel?.sessions.find((s) => s.id === deleteTarget)
  const name = session?.title || session?.branch_name

  function handleConfirm() {
    if (!deleteTarget) return
    deleteSession(deleteTarget, deleteWorktree)
    setDeleteWorktree(false)
    closeDelete()
  }

  function handleCancel() {
    setDeleteWorktree(false)
    closeDelete()
  }

  function handleOpenChange(open: boolean) {
    if (!open) handleCancel()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Delete agent?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          This removes the agent session &ldquo;{name}&rdquo; from dux.
        </p>
        <div className="flex items-center gap-2">
          <Checkbox
            id="delete-worktree"
            checked={deleteWorktree}
            onCheckedChange={setDeleteWorktree}
          />
          <label htmlFor="delete-worktree" className="text-sm">
            Also delete the git worktree on disk (irreversible)
          </label>
        </div>
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={handleCancel}>
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
