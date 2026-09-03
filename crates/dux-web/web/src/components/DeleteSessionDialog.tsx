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
// `subject` is the branch's name once there are two branches on screen, because
// the provenance is recorded about the birth branch and "this branch" would no
// longer say which. Mirrors `BranchProvenance::kept_reason` and the TUI's
// `delete_agent_branch_warning`.
function existedBeforeSentence(provenance: string, subject: string): string {
  if (provenance === "adopted")
    return `${subject} came with the worktree this agent adopted.`
  if (provenance === "unknown") return `${subject} is not one dux created.`
  return `${subject} existed before the agent.`
}

// The branch checkbox's label, naming every branch the tick would delete. A box
// that promised one deletion and performed two would be taking consent it was
// never given.
function branchCheckboxLabel(branches: string[]): string {
  if (branches.length === 0) return ""
  if (branches.length === 1) return `Also delete the branch ${branches[0]}`
  return `Also delete the branches ${branches.join(" and ")}`
}

// The warning under the branch box, or `null` when there is nothing to warn
// about. Two reasons, either enough on its own: the branch predates the agent,
// so it was never dux's to delete, or the agent drifted and the tick takes a
// second branch with it. `unpushed` is null while the count is in flight and
// when git could not answer; the count sentence is absent in both cases.
function branchWarning(
  provenance: string,
  branches: string[],
  unpushed: { count: number; has_remote_refs: boolean } | null,
): string | null {
  const drifted = branches.length > 1
  const predates = provenance !== "created"
  if (!drifted && !predates) return null
  const parts: string[] = []
  if (drifted) {
    parts.push(
      `The worktree moved from ${branches[1]} onto ${branches[0]}, so deleting the agent removes both.`,
    )
  }
  if (predates) {
    parts.push(
      existedBeforeSentence(provenance, drifted ? branches[1] : "This branch"),
    )
  }
  if (unpushed !== null && unpushed.count > 0) {
    const count = unpushed.count
    const plural = count === 1 ? "commit" : "commits"
    // A repository with no remote-tracking refs has held nothing back; it has
    // nowhere to have pushed to, and the count is its whole history. "Not
    // pushed anywhere" reads there as an accusation about work that was never
    // going anywhere, so the sentence says what is actually true.
    if (unpushed.has_remote_refs) {
      parts.push(
        drifted
          ? `They have ${count} ${plural} not pushed anywhere between them.`
          : `It has ${count} ${plural} not pushed anywhere.`,
      )
    } else {
      const subject = drifted ? "them" : "it"
      const possessive = drifted ? "their" : "its"
      // A single commit gets its own clause: "all 1 of its commits" is the
      // sentence admitting it was assembled rather than written.
      const existence =
        count === 1
          ? `${possessive} only commit exists`
          : `all ${count} of ${possessive} commits exist`
      parts.push(
        `Nothing on ${subject} has been pushed anywhere: ${existence} only on this machine.`,
      )
    }
  }
  return parts.join(" ")
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
  const [unpushed, setUnpushed] = useState<
    { count: number; has_remote_refs: boolean } | null
  >(null)
  // The branches the server says the delete would remove, straight from the
  // answer that counted them. Rendered in preference to working the pair out
  // here a second time, so what this dialog asks about is what the server would
  // actually delete. `null` until the answer lands, and the local pair below
  // stands in until then.
  const [answeredBranches, setAnsweredBranches] = useState<string[] | null>(
    null,
  )

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
  // Every branch the box names and the count is about: the one the worktree is
  // on now and, when the agent has drifted, the one it was born on. Both are
  // deleted, so both are named. The server's answer wins once it lands; this is
  // the same rule (`BranchDeleteInputs::warned_branches`) applied to the spine
  // so the label is never blank while the git call is out.
  const localBranches = managed
    ? managed.initial_branch && managed.initial_branch !== managed.branch_name
      ? [managed.branch_name, managed.initial_branch]
      : [managed.branch_name]
    : []
  const warnedBranches = answeredBranches ?? localBranches
  const branchWarningText = branchWarning(provenance, warnedBranches, unpushed)
  // The box starts in the provenance default: ticked for a branch dux made,
  // unticked for one that predates the agent. Both are overridable, which is
  // the whole point of it being a control.
  const deleteBranch = branchAnswer ?? branchIsDuxs

  function reset() {
    setDeleteWorktree(false)
    setBranchAnswer(null)
    setUnpushed(null)
    setAnsweredBranches(null)
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

  // Asked exactly when the branch offer is on screen, because the answer is
  // where both the branch names and the count come from. With the worktree kept
  // there is no branch box at all, so nothing here would be rendered and the
  // git call would buy nothing.
  const askUnpushed = isOpen && managed !== null && deleteWorktree
  useEffect(() => {
    if (!askUnpushed || !deleteTarget) return
    let live = true
    sessionsApi
      .branchUnpushed(deleteTarget)
      .then((answer) => {
        if (!live) return
        setUnpushed(answer.unpushed)
        setAnsweredBranches(answer.branches)
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
            <label htmlFor="delete-branch" className="break-all text-sm">
              {branchCheckboxLabel(warnedBranches)}
            </label>
          </div>
        )}
        {managed && deleteWorktree && branchWarningText !== null && (
          // The danger sits in the warning text, never in a red checkbox: the
          // box is an ordinary control and the sentence under it is what says
          // what is at stake.
          <p className="text-sm text-destructive">{branchWarningText}</p>
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
