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
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

export function DeleteSessionDialog() {
  const { deleteTarget, spine } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const name = session?.title || session?.branch_name
  // The component stays mounted across opens, so a vanish-close must also
  // reset the checkbox, otherwise the NEXT delete confirm opens pre-checked
  // with "also delete the worktree". Wrap the hook's close callback to do both.
  const isOpen = useVanishedTargetGuard(
    deleteTarget !== null,
    session !== undefined,
    () => {
      setDeleteWorktree(false)
      closeDelete()
    },
  )

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
      <DialogContent showCloseButton={false} destructive>
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
            {/* The branch is named because this path deletes it: the agent's
               current branch and, when it drifted, the one it was born on. The
               TUI's checkbox has always said so. */}
            Also delete the git worktree and its branch (irreversible)
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
