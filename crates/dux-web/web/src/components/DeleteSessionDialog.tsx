import { useEffect, useState } from "react"
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
import { sessionsApi } from "@/lib/sessionsApi"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

// The sentence naming why a branch predates the agent, one clause per
// provenance. The unrecognized one gets its own rather than borrowing "existed
// before this agent": that is a claim about a branch nothing here can make.
// Mirrors `BranchProvenance::kept_reason`.
function existedBeforeSentence(provenance: string): string {
  if (provenance === "adopted")
    return "This branch came with the worktree this agent adopted."
  if (provenance === "unknown") return "This branch is not one dux created."
  return "This branch existed before the agent."
}

export function DeleteSessionDialog() {
  const { deleteTarget, spine } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)
  // `null` is "the user has not touched the box for this agent", which renders
  // as the provenance default. Kept separate from the default so reopening on
  // another agent picks up ITS default rather than the previous agent's answer.
  const [branchAnswer, setBranchAnswer] = useState<boolean | null>(null)
  // How much work ticking the branch box would destroy. Arrives after the
  // dialog opens: it is a git call, so it runs on the server off the reactor
  // and the warning line grows a sentence when the answer lands. `null` all the
  // way through means git could not answer, and the dialog then says nothing
  // about it rather than guessing a number.
  const [unpushed, setUnpushed] = useState<number | null>(null)

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const name = session ? sessionLabel(session) : undefined
  // The MANAGED identity, when there is one. A standalone agent has none, and
  // every worktree and branch affordance below hangs off this: there is no
  // worktree to remove and no branch to delete, so the checkboxes are not
  // merely unchecked, they do not exist. The offer cannot be rendered, so it
  // cannot be ticked.
  const managed = session ? managedWorkspace(session.workspace) : null
  const folder = session ? folderWorkspace(session.workspace) : null
  const provenance = managed?.branch_provenance ?? "created"
  const branchIsDuxs = provenance === "created"
  // The branch the box names and the count is about: the one the agent was BORN
  // on, which is the one whose provenance the warning is about. Matches
  // `BranchDeleteInputs::warned_branch` on the server, so the two can never name
  // different branches.
  const warnedBranch = managed
    ? managed.initial_branch || managed.branch_name
    : ""
  // The box starts in the provenance default: ticked for a branch dux made,
  // unticked for one that predates the agent. Both are overridable, which is
  // the whole point of it being a control.
  const deleteBranch = branchAnswer ?? branchIsDuxs

  function reset() {
    setDeleteWorktree(false)
    setBranchAnswer(null)
    setUnpushed(null)
  }

  // The component stays mounted across opens, so a vanish-close must also
  // reset the boxes, otherwise the NEXT delete confirm opens pre-checked.
  // Wrap the hook's close callback to do both.
  const isOpen = useVanishedTargetGuard(
    deleteTarget !== null,
    session !== undefined,
    () => {
      reset()
      closeDelete()
    },
  )

  // Asked only where the warning would use it: a branch dux created is going
  // because the user made it, and counting its commits would buy a git call per
  // open to say nothing new.
  const askUnpushed = isOpen && managed !== null && !branchIsDuxs
  useEffect(() => {
    if (!askUnpushed || !deleteTarget) return
    let live = true
    sessionsApi
      .branchUnpushed(deleteTarget)
      .then((answer) => {
        if (live) setUnpushed(answer.unpushed_commits)
      })
      // A failure is simply "no number to show". The dialog is already telling
      // the user the branch predates the agent, which is the part that must not
      // depend on git answering.
      .catch(() => {})
    return () => {
      live = false
    }
  }, [askUnpushed, deleteTarget])

  function handleConfirm() {
    if (!deleteTarget) return
    // A standalone agent has no worktree to remove, and the server REFUSES a
    // worktree-removing delete on one rather than downgrading it quietly. The
    // checkboxes do not exist for one, but this component stays mounted across
    // opens, so a tick left over from a managed agent would otherwise ride along
    // and wedge the delete in a refusal with no control on screen to clear.
    const removeWorktree = managed ? deleteWorktree : false
    // The branch answer is sent only when the box was actually on screen. With
    // the worktree kept there is no branch offer at all, and a standalone agent
    // has no branch, so both send `null` and the server keeps its own default.
    const branchAnswerToSend = removeWorktree ? deleteBranch : null
    deleteSession(deleteTarget, removeWorktree, branchAnswerToSend)
    reset()
    closeDelete()
  }

  function handleCancel() {
    reset()
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
              Also delete the git worktree (irreversible)
            </label>
          </div>
        )}
        {managed && deleteWorktree && (
          // Revealed by the worktree box rather than sitting beside it,
          // disabled: git will not delete a branch that is still checked out in
          // a worktree, so with the worktree kept there is genuinely nothing on
          // offer here, and a permanently greyed control that happens to look
          // ticked promises exactly the deletion it cannot do.
          <div className="flex items-center gap-2">
            <Checkbox
              id="delete-branch"
              checked={deleteBranch}
              onCheckedChange={setBranchAnswer}
            />
            <label htmlFor="delete-branch" className="text-sm">
              Also delete the branch &ldquo;
              <span className="break-all">{warnedBranch}</span>&rdquo;
            </label>
          </div>
        )}
        {managed && deleteWorktree && !branchIsDuxs && (
          // The danger sits in the warning text, never in a red checkbox: the
          // box is an ordinary control and the sentence under it is what says
          // this one is not dux's branch to remove.
          <p className="text-sm text-muted-foreground">
            {existedBeforeSentence(provenance)}
            {unpushed !== null && unpushed > 0 && (
              <>
                {" "}
                It has {unpushed}{" "}
                {unpushed === 1 ? "commit" : "commits"} not pushed anywhere.
              </>
            )}
          </p>
        )}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={handleCancel} autoFocus>
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
