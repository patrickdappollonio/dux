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
import {
  folderWorkspace,
  managedWorkspace,
  sessionLabel,
} from "@/lib/agentWorkspace"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

export function DeleteSessionDialog() {
  const { deleteTarget, spine } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const name = session ? sessionLabel(session) : undefined
  // The MANAGED identity, when there is one. A standalone agent has none, and
  // every worktree affordance below hangs off this: there is no worktree to
  // remove and no branch to delete, so the checkbox is not merely unchecked,
  // it does not exist. The offer cannot be rendered, so it cannot be ticked.
  const managed = session ? managedWorkspace(session.workspace) : null
  const folder = session ? folderWorkspace(session.workspace) : null
  // dux force-deletes the branch only when dux created it. An agent attached to
  // an existing branch, or adopted along with an existing worktree, gives up its
  // worktree and keeps its branch, so the checkbox must not promise otherwise.
  const provenance = managed?.branch_provenance ?? "created"
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
    // A standalone agent has no worktree to remove, and the server REFUSES a
    // worktree-removing delete on one rather than downgrading it quietly. The
    // checkbox does not exist for one, but this component stays mounted across
    // opens, so a tick left over from a managed agent would otherwise ride along
    // and wedge the delete in a refusal with no control on screen to clear.
    deleteSession(deleteTarget, managed ? deleteWorktree : false)
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
        {folder && (
          // A standalone agent: dux's record of it goes and the user's folder
          // is exactly as it was. Said out loud, because the sentence above on
          // its own reads as though something on disk went with it.
          <p className="text-sm text-muted-foreground">
            Its folder &ldquo;
            <span className="break-all font-mono">{folder.folder_label}</span>
            &rdquo; is left untouched: dux never creates, moves or removes a
            standalone agent&rsquo;s folder. Anything the agent wrote there is
            still there.
          </p>
        )}
        {managed && (
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
        )}
        {managed && branchIsKept && (
          <p className="text-sm text-muted-foreground">
            The branch &ldquo;
            <span className="break-all">
              {managed.initial_branch || managed.branch_name}
            </span>
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
        {managed && branchIsKept && (
          // The manual override, named where the user is standing. dux has no
          // command that deletes a branch it did not create, so when this
          // verdict is wrong for a branch (an agent created from a pull request
          // by a dux older than this one recorded the branch as pre-existing)
          // the way through is the project's Worktrees dialog, which honors the
          // checkbox rather than the provenance.
          <p className="text-sm text-muted-foreground">
            To delete it anyway, leave the worktree in place here, then remove
            it from the project&rsquo;s Worktrees dialog, leaving its branch box
            ticked.
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
