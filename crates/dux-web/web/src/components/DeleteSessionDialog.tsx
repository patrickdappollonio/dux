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
import { InlineCode } from "@/components/ui/inline-code"
import {
  AttachedSection,
  GuardedConfirmButton,
} from "@/components/AttachedSection"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import {
  folderWorkspace,
  managedWorkspace,
  sessionLabel,
  type AgentWorkspaceWire,
} from "@/lib/agentWorkspace"
import {
  branchesCheckFailedProse,
  branchesCheckingProse,
  currentBranchLabelProse,
  branchLossProse,
  currentBranchWarningProse,
  otherBranchesLabelProse,
  otherBranchesWarningProse,
  protectedBranchesProse,
} from "@/lib/deleteAgent"
import { type Prose, renderProse } from "@/lib/prose"
import {
  type BranchBoxAnswer,
  type BranchOffer,
  type DeleteBranchAnswers,
  type OfferedBranch,
  sessionsApi,
} from "@/lib/sessionsApi"
import type { SessionView } from "@/lib/types"
import { closeDelete, deleteSession, useDux } from "@/lib/store"

// Where the branch offer stands. The boxes answer only from a READY offer,
// whose entries are pinned to the commits the server read; until then, and
// when the check failed, they are disabled and every branch is kept. A ready
// offer may still have no first box: the project's own branch is offered in
// no box, and a branch git could not read is a failed check for that box.
type OfferState =
  | { state: "pending" }
  | { state: "failed"; reason: string }
  | { state: "ready"; offer: BranchOffer }

const PENDING: OfferState = { state: "pending" }

function lossAnswer(
  count: { count: number; has_remote_refs: boolean } | null,
  error: string | null,
): { count: number; has_remote_refs: boolean } | { failed: string } {
  return count ?? { failed: error ?? "no count came back" }
}

// What deleting the boxes ticked right now loses, or `null` with nothing
// ticked; it changes on every tick.
function lossProse(
  offer: BranchOffer,
  deleteCurrent: boolean,
  deleteOthers: boolean,
): Prose | null {
  const current = deleteCurrent ? offer.current : null
  const others = deleteOthers ? offer.others : []
  const deleted = [
    ...(current ? [current.name] : []),
    ...others.map((other) => other.name),
  ]
  if (deleted.length === 0) return null
  const lost =
    current && others.length > 0
      ? lossAnswer(offer.both_unpushed, offer.both_unpushed_error)
      : current
        ? lossAnswer(offer.current_unpushed, offer.current_unpushed_error)
        : lossAnswer(offer.others_unpushed, offer.others_unpushed_error)
  return branchLossProse(deleted, lost)
}

// The boxes' defaults: the first ticked only for a branch dux made or one
// created after the agent started; the second only when dux can prove every
// branch in it is its own.
function currentDefault(current: OfferedBranch): boolean {
  return current.origin === "dux" || current.origin === "after"
}

function othersDefault(others: OfferedBranch[]): boolean {
  return others.length > 0 && others.every((other) => other.origin === "dux")
}

// A box over `branches`, ticked or not: a ticked one pins each branch to the
// commit the offer read, and either way it says which branches it showed.
function boxAnswer(branches: OfferedBranch[], ticked: boolean): BranchBoxAnswer {
  return {
    delete: ticked,
    branches: ticked ? branches.map(({ name, oid }) => ({ name, oid })) : [],
    offered: branches.map(({ name }) => name),
  }
}

function unchecked(
  why: "pending" | "failed" | "not_offered",
): BranchBoxAnswer {
  return { delete: false, branches: [], unchecked: why }
}

// What the delete carries for the two boxes. With no ready offer both boxes
// say why they were not answered and pin nothing, so nothing is deleted on a
// guess; a box the dialog did not show says so. Neither is ever left out,
// since a box left out lets the server decide.
function branchAnswers(
  offer: OfferState,
  deleteCurrent: boolean,
  deleteOthers: boolean,
): DeleteBranchAnswers {
  if (offer.state !== "ready") {
    const why = unchecked(offer.state)
    return { current: why, others: why }
  }
  // Decided from the name the offer was read for, never from where the
  // worktree is by now.
  const { current, others, protected: own, current_name } = offer.offer
  return {
    current: current
      ? boxAnswer([current], deleteCurrent)
      : unchecked(own.includes(current_name) ? "not_offered" : "failed"),
    others:
      others.length === 0
        ? unchecked("not_offered")
        : boxAnswer(others, deleteOthers),
  }
}

type ManagedWorkspace = Extract<AgentWorkspaceWire, { kind: "managed" }>

// Everything the dialog says about the agent it is about. A missing agent
// answers with no name and no boxes, which is the state the vanish guard
// closes on.
interface DeleteSessionView {
  name: string | undefined
  // The MANAGED identity, when there is one. Every worktree and branch
  // affordance hangs off it, so a standalone agent's boxes do not exist rather
  // than unticking.
  managed: ManagedWorkspace | null
  folder: Extract<AgentWorkspaceWire, { kind: "folder" }> | null
}

function deleteSessionView(session: SessionView | undefined): DeleteSessionView {
  return {
    name: session ? sessionLabel(session) : undefined,
    managed: session ? managedWorkspace(session.workspace) : null,
    folder: session ? folderWorkspace(session.workspace) : null,
  }
}

function BranchWarning({ prose }: { prose: Prose | null }) {
  if (prose === null || prose.length === 0) return null
  // The danger sits in the warning text, never in a red checkbox: the box is an
  // ordinary control and the sentence under it says what is at stake.
  return <p className="text-sm text-destructive">{renderProse(prose)}</p>
}

export function DeleteSessionDialog() {
  const { deleteTarget, spine } = useDux()
  const [deleteWorktree, setDeleteWorktree] = useState(false)
  // `null` means a box is untouched for this agent and renders its default, so
  // reopening on another agent picks up ITS default.
  const [currentAnswer, setCurrentAnswer] = useState<boolean | null>(null)
  const [othersAnswer, setOthersAnswer] = useState<boolean | null>(null)
  const [offer, setOffer] = useState<OfferState>(PENDING)

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const { name, managed, folder } = deleteSessionView(session)
  const ready = offer.state === "ready" ? offer.offer : null
  const current = ready?.current ?? null
  const others = ready?.others ?? []
  const own = ready?.protected ?? []
  const deleteCurrent = current
    ? (currentAnswer ?? currentDefault(current))
    : false
  const deleteOthers = ready ? (othersAnswer ?? othersDefault(others)) : false

  function reset() {
    setDeleteWorktree(false)
    setCurrentAnswer(null)
    setOthersAnswer(null)
    setOffer(PENDING)
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

  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    deleteTarget,
  )

  // Asked exactly when the branch offer is on screen; with the worktree kept
  // there is nothing to render.
  const askOffer = isOpen && managed !== null && deleteWorktree
  useEffect(() => {
    if (!askOffer || !deleteTarget) return
    let live = true
    sessionsApi
      .branchUnpushed(deleteTarget)
      .then((answer) => {
        if (live) setOffer({ state: "ready", offer: answer })
      })
      .catch((e: unknown) => {
        if (!live) return
        setOffer({
          state: "failed",
          reason: e instanceof Error ? e.message : String(e),
        })
      })
    return () => {
      live = false
    }
  }, [askOffer, deleteTarget])

  async function handleConfirm() {
    if (!deleteTarget) return
    // The server REFUSES a worktree-removing delete on a standalone agent, and this
    // component outlives an open, so a leftover tick would wedge it with no control.
    const removeWorktree = managed ? deleteWorktree : false
    // The branch answers are sent only when the boxes were on screen.
    const branchesToSend =
      removeWorktree && managed
        ? branchAnswers(offer, deleteCurrent, deleteOthers)
        : null
    const target = deleteTarget
    const done = await confirm((accepted) =>
      deleteSession(target, removeWorktree, branchesToSend, accepted),
    )
    if (!done) return
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
          This removes the agent session <InlineCode>{name}</InlineCode> from
          dux.
        </p>
        {folder && (
          // Said out loud because the sentence above reads as though something on
          // disk goes too: a standalone agent's folder is left exactly as it was.
          <p className="text-sm text-muted-foreground">
            Its folder <InlineCode>{folder.folder_label}</InlineCode> is left
            untouched: dux never creates, moves or removes a
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
          <>
            {(ready === null || current !== null) && (
              <div className="flex items-center gap-2">
                <Checkbox
                  id="delete-branch"
                  checked={deleteCurrent}
                  disabled={current === null}
                  onCheckedChange={setCurrentAnswer}
                />
                <label htmlFor="delete-branch" className="min-w-0 text-sm">
                  {renderProse(
                    currentBranchLabelProse(current?.name ?? managed.branch_name),
                  )}
                </label>
              </div>
            )}
            {ready &&
              current === null &&
              !own.includes(ready.current_name) && (
                // In the first box's place: only that box failed.
                <p className="text-sm text-muted-foreground">
                  {renderProse(
                    branchesCheckFailedProse("git could not read the branch"),
                  )}
                </p>
              )}
            {ready && current && (
              <BranchWarning prose={currentBranchWarningProse(current.origin)} />
            )}
            {ready && others.length > 0 && (
              <>
                <div className="flex items-center gap-2">
                  <Checkbox
                    id="delete-other-branches"
                    checked={deleteOthers}
                    onCheckedChange={setOthersAnswer}
                  />
                  <label
                    htmlFor="delete-other-branches"
                    className="min-w-0 text-sm"
                  >
                    {renderProse(
                      otherBranchesLabelProse(others.map((o) => o.name)),
                    )}
                  </label>
                </div>
                <BranchWarning
                  prose={otherBranchesWarningProse(others, deleteOthers)}
                />
              </>
            )}
            {offer.state !== "ready" && (
              <p className="text-sm text-muted-foreground">
                {renderProse(
                  offer.state === "pending"
                    ? branchesCheckingProse()
                    : branchesCheckFailedProse(offer.reason),
                )}
              </p>
            )}
            {ready && (
              <BranchWarning
                prose={lossProse(ready, deleteCurrent, deleteOthers)}
              />
            )}
            {own.length > 0 && (
              <p className="text-sm text-muted-foreground">
                {renderProse(protectedBranchesProse(own))}
              </p>
            )}
          </>
        )}
        <AttachedSection blockers={blockers} />
        <div className="h-2" />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            onClick={handleCancel}
            autoFocus
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
