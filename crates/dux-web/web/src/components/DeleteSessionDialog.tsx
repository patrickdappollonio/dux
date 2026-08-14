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
  // dux force-deletes the branch only when dux created it. An agent attached to
  // an existing branch, or adopted along with an existing worktree, gives up its
  // worktree and keeps its branch, so the checkbox must not promise otherwise.
  // A server too old to send the field is a server that deletes the branch, so
  // the absent case takes the branch-deleting copy.
  const provenance = session?.branch_provenance ?? "created"
  const branchIsKept = provenance !== "created"
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
            {/* Two labels, because the checkbox means two different things.
               When dux created the branch it goes with the worktree, and the
               label says so; when the branch is the user's, dux keeps it and
               the label must not promise a deletion that will not happen. The
               TUI's checkbox makes the same distinction, from the same
               provenance. */}
            {branchIsKept
              ? "Also delete the git worktree, keeping its branch (irreversible)"
              : "Also delete the git worktree and its branch (irreversible)"}
          </label>
        </div>
        {branchIsKept && (
          <p className="text-sm text-muted-foreground">
            The branch &ldquo;{session?.initial_branch || session?.branch_name}
            &rdquo;{" "}
            {/* One clause per provenance, and the unrecognized one gets its
               own rather than borrowing "existed before this agent": that is
               a claim about a branch nothing here can make. Mirrors
               `BranchProvenance::kept_reason`. */}
            {provenance === "adopted"
              ? "came with the worktree this agent adopted"
              : provenance === "unknown"
                ? "is not a branch dux created"
                : "existed before this agent"}
            , so dux keeps it.
          </p>
        )}
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
