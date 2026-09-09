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
  type AgentWorkspaceWire,
} from "@/lib/agentWorkspace"
import { sessionsApi } from "@/lib/sessionsApi"
import type { SessionView } from "@/lib/types"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

// Why a branch predates the agent, one clause per provenance; an unrecognized
// provenance gets its own, since nothing here can claim the branch is older.
// `subject` names the branch once there are two on screen.
function existedBeforeSentence(provenance: string, subject: string): string {
  if (provenance === "adopted")
    return `${subject} came with the worktree this agent adopted.`
  if (provenance === "unknown") return `${subject} is not one dux created.`
  return `${subject} existed before the agent.`
}

// The branch checkbox's label, naming EVERY branch the tick would delete: a box
// promising one deletion and performing two takes consent it was not given.
function branchCheckboxLabel(branches: string[]): string {
  if (branches.length === 0) return ""
  if (branches.length === 1) return `Also delete the branch ${branches[0]}`
  return `Also delete the branches ${branches.join(" and ")}`
}

type UnpushedCount = { count: number; has_remote_refs: boolean } | null

// How much work the tick would destroy, or `null` when there is nothing to say:
// no count yet, or none unpushed. `drifted` puts the sentence in the plural,
// because the tick then takes both branches.
function unpushedSentence(
  unpushed: UnpushedCount,
  drifted: boolean,
): string | null {
  if (unpushed === null || unpushed.count === 0) return null
  const count = unpushed.count
  const plural = count === 1 ? "commit" : "commits"
  if (unpushed.has_remote_refs) {
    return drifted
      ? `They have ${count} ${plural} not pushed anywhere between them.`
      : `It has ${count} ${plural} not pushed anywhere.`
  }
  // With no remote-tracking refs the count is the whole history rather than work
  // held back, so the sentence must not read as an accusation.
  const subject = drifted ? "them" : "it"
  const possessive = drifted ? "their" : "its"
  // A single commit gets its own clause: "all 1 of its commits" is the sentence
  // admitting it was assembled rather than written.
  const existence =
    count === 1
      ? `${possessive} only commit exists`
      : `all ${count} of ${possessive} commits exist`
  return `Nothing on ${subject} has been pushed anywhere: ${existence} only on this machine.`
}

// The warning under the branch box, or `null`: either the branch predates the
// agent, or drift means the tick takes a second branch. A null `unpushed` count
// (in flight, or unanswerable) drops the count sentence.
function branchWarning(
  provenance: string,
  branches: string[],
  unpushed: UnpushedCount,
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
  const unpushedText = unpushedSentence(unpushed, drifted)
  if (unpushedText !== null) parts.push(unpushedText)
  return parts.join(" ")
}

type ManagedWorkspace = Extract<AgentWorkspaceWire, { kind: "managed" }>

// Every branch the box names and the count covers: the one the worktree is on
// now and, on drift, the birth branch. Both are deleted, so both are named.
function localBranches(managed: ManagedWorkspace | null): string[] {
  if (!managed) return []
  const drifted =
    managed.initial_branch && managed.initial_branch !== managed.branch_name
  return drifted
    ? [managed.branch_name, managed.initial_branch]
    : [managed.branch_name]
}

// Everything the dialog says about the agent it is about, out of the agent and
// the two answers the server sends after it opens. A missing agent answers with
// no name and no boxes, which is the state the vanish guard closes on.
interface DeleteSessionView {
  name: string | undefined
  // The MANAGED identity, when there is one. Every worktree and branch
  // affordance hangs off it, so a standalone agent's boxes do not exist rather
  // than unticking.
  managed: ManagedWorkspace | null
  folder: Extract<AgentWorkspaceWire, { kind: "folder" }> | null
  branchIsDuxs: boolean
  branches: string[]
  warning: string | null
}

function deleteSessionView(
  session: SessionView | undefined,
  answeredBranches: string[] | null,
  unpushed: UnpushedCount,
): DeleteSessionView {
  const managed = session ? managedWorkspace(session.workspace) : null
  const provenance = managed?.branch_provenance ?? "created"
  // The branches the server says the delete would remove, preferred over
  // working the pair out again here; the local pair stands in until it lands.
  const branches = answeredBranches ?? localBranches(managed)
  return {
    name: session ? sessionLabel(session) : undefined,
    managed,
    folder: session ? folderWorkspace(session.workspace) : null,
    branchIsDuxs: provenance === "created",
    branches,
    warning: branchWarning(provenance, branches, unpushed),
  }
}

export function DeleteSessionDialog() {
  const { deleteTarget, spine } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)
  // `null` means the box is untouched for this agent and renders the provenance
  // default, so reopening on another agent picks up ITS default.
  const [branchAnswer, setBranchAnswer] = useState<boolean | null>(null)
  // How much work ticking the branch box would destroy, arriving after the dialog
  // opens. `null` throughout means git could not answer, and nothing is said.
  const [unpushed, setUnpushed] = useState<UnpushedCount>(null)
  // The branches the server says the delete would remove. `null` until it lands.
  const [answeredBranches, setAnsweredBranches] = useState<string[] | null>(
    null,
  )

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const { name, managed, folder, branchIsDuxs, branches, warning } =
    deleteSessionView(session, answeredBranches, unpushed)
  // The box starts in the provenance default: ticked for a branch dux made,
  // unticked for one that predates the agent. Both are overridable.
  const deleteBranch = branchAnswer ?? branchIsDuxs

  function reset() {
    setDeleteWorktree(false)
    setBranchAnswer(null)
    setUnpushed(null)
    setAnsweredBranches(null)
  }

  // The component stays mounted across opens, so a vanish-close must reset the
  // boxes too, or the next delete confirm opens pre-checked.
  const isOpen = useVanishedTargetGuard(
    deleteTarget !== null,
    session !== undefined,
    () => {
      reset()
      closeDelete()
    },
  )

  // Asked exactly when the branch offer is on screen, since the answer carries both
  // the names and the count; with the worktree kept there is nothing to render.
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
      // A failure is simply no number to show: the sentence saying the branch
      // predates the agent must not depend on git answering.
      .catch(() => {})
    return () => {
      live = false
    }
  }, [askUnpushed, deleteTarget])

  function handleConfirm() {
    if (!deleteTarget) return
    // The server REFUSES a worktree-removing delete on a standalone agent, and this
    // component outlives an open, so a leftover tick would wedge it with no control.
    const removeWorktree = managed ? deleteWorktree : false
    // The branch answer is sent only when the box was on screen; otherwise `null`,
    // and the server keeps its own default.
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
          // Said out loud because the sentence above reads as though something on
          // disk goes too: a standalone agent's folder is left exactly as it was.
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
          // Revealed by the worktree box rather than sitting beside it disabled: git
          // will not delete a branch still checked out, so there is nothing to offer.
          <div className="flex items-center gap-2">
            <Checkbox
              id="delete-branch"
              checked={deleteBranch}
              onCheckedChange={setBranchAnswer}
            />
            <label htmlFor="delete-branch" className="break-all text-sm">
              {branchCheckboxLabel(branches)}
            </label>
          </div>
        )}
        {managed && deleteWorktree && warning !== null && (
          // The danger sits in the warning text, never in a red checkbox: the box is
          // an ordinary control and the sentence under it says what is at stake.
          <p className="text-sm text-destructive">{warning}</p>
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
