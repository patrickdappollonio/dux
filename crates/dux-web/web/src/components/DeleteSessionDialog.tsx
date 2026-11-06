import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react"
import { Check, CircleAlert, Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { CheckboxRow } from "@/components/CheckboxRow"
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
  boxCountFailures,
  branchesCheckFailedProse,
  branchesCheckedProse,
  currentBranchCheckFailedProse,
  branchesCheckingProse,
  currentBranchLabelProse,
  branchLossProse,
  currentBranchWarningProse,
  otherBranchesLabelProse,
  otherBranchesWarningProse,
  protectedBranchesProse,
  type UnpushedAnswer,
} from "@/lib/deleteAgent"
import { type Prose, renderProse } from "@/lib/prose"
import { cn } from "@/lib/utils"
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
// no box, and a branch git could not read is a failed check for that box, as
// is a box whose loss git could not count.
type OfferState =
  | { state: "pending" }
  | { state: "failed"; reason: string }
  | { state: "ready"; offer: BranchOffer }

const PENDING: OfferState = { state: "pending" }

// A ready offer with no first box for a branch that is not the project's own:
// git could not read the branch the worktree is on.
function currentBranchFailed(offer: BranchOffer): boolean {
  return offer.current === null && !offer.protected.includes(offer.current_name)
}

// What the check says as a whole, and the sentence it says it in. A ready offer
// whose current branch could not be read is a failed check, so the pill and the
// first box never disagree, and so is one whose commits git could not count.
type BranchCheck =
  | { state: "pending" }
  | { state: "checked"; prose: Prose }
  | { state: "none" }
  | { state: "failed"; prose: Prose }

function branchCheck(offer: OfferState): BranchCheck {
  if (offer.state === "pending") return offer
  if (offer.state === "failed")
    return { state: "failed", prose: branchesCheckFailedProse(offer.reason) }
  const ready = offer.offer
  if (currentBranchFailed(ready))
    return {
      state: "failed",
      prose: currentBranchCheckFailedProse(
        ready.current_name,
        "git could not read the branch",
      ),
    }
  const answer = branchesCheckedProse(
    ready.current?.name ?? null,
    lossAnswer(ready.current_unpushed, ready.current_unpushed_error),
    ready.others.map((other) => other.name),
    lossAnswer(ready.others_unpushed, ready.others_unpushed_error),
    lossAnswer(ready.both_unpushed, ready.both_unpushed_error),
  )
  if ("failed" in answer) return { state: "failed", prose: answer.failed }
  return answer.ok === null
    ? { state: "none" }
    : { state: "checked", prose: answer.ok }
}

// How long the answer stays up before the pill leaves.
const CHECKED_PILL_MS = 3000

// The offer for one open of the dialog, kept under the agent it was asked for
// so a dialog that moves straight to another agent never shows the first
// one's answer. `settled` is set once the checked pill has had its time.
interface OfferEntry {
  target: string
  offer: OfferState
  settled: boolean
}

function lossAnswer(
  count: { count: number; has_remote_refs: boolean } | null,
  error: string | null,
): UnpushedAnswer {
  return count ?? { failed: error ?? "no count came back" }
}

// Why git could not count what each box (`[first, second]`) would lose. Such
// a box cannot be answered: it is disabled and unticked, and its branches are
// kept as a failed check.
function countFailures(offer: BranchOffer): [string | null, string | null] {
  return boxCountFailures(
    offer.current === null
      ? null
      : lossAnswer(offer.current_unpushed, offer.current_unpushed_error),
    offer.others.length === 0
      ? null
      : lossAnswer(offer.others_unpushed, offer.others_unpushed_error),
    lossAnswer(offer.both_unpushed, offer.both_unpushed_error),
  )
}

// What deleting the boxes ticked right now loses, or `null` with nothing
// ticked or nothing lost; it changes on every tick.
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
  const { current, others } = offer.offer
  const [currentUncounted, othersUncounted] = countFailures(offer.offer)
  return {
    current: current
      ? currentUncounted !== null
        ? unchecked("failed")
        : boxAnswer([current], deleteCurrent)
      : unchecked(currentBranchFailed(offer.offer) ? "failed" : "not_offered"),
    others:
      others.length === 0
        ? unchecked("not_offered")
        : othersUncounted !== null
          ? unchecked("failed")
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

// Grows its content in and shrinks it away by animating the row track rather
// than popping it, so the centered dialog resizes smoothly instead of jumping.
// The spacing above the content rides inside it, so a closed one takes no
// room at all; a closed one stays mounted but is out of reach.
function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      className={cn(
        "grid transition-[grid-template-rows,opacity] duration-200 ease-out motion-reduce:transition-none",
        open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
      )}
      aria-hidden={!open}
      inert={!open}
    >
      <div className="min-h-0 overflow-hidden">
        <div className="flex flex-col gap-4 pt-4">{children}</div>
      </div>
    </div>
  )
}

// Follows its content's height with a transition, so a change of what the
// content says (one line becoming two) resizes the centered dialog smoothly
// instead of in one frame. Until the first measurement the height is natural.
function AnimatedHeight({ children }: { children: ReactNode }) {
  const inner = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState<number | null>(null)
  useEffect(() => {
    const el = inner.current
    if (!el || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.borderBoxSize?.[0]
      setHeight(box ? box.blockSize : el.getBoundingClientRect().height)
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [])
  return (
    <div
      className="overflow-hidden transition-[height] duration-200 ease-out motion-reduce:transition-none"
      style={height === null ? undefined : { height }}
    >
      <div ref={inner} className="flex flex-col items-start">
        {children}
      </div>
    </div>
  )
}

// Whether `el`'s text has wrapped past its first line, followed as its size
// changes. A line height the page cannot read reads as one line.
function useWrapped(el: RefObject<HTMLElement | null>): boolean {
  const [wrapped, setWrapped] = useState(false)
  useEffect(() => {
    const node = el.current
    if (!node || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.borderBoxSize?.[0]
      const height = box ? box.blockSize : node.getBoundingClientRect().height
      const line = parseFloat(getComputedStyle(node).lineHeight)
      setWrapped(Number.isFinite(line) && height > line * 1.5)
    })
    observer.observe(node)
    return () => observer.disconnect()
  }, [el])
  return wrapped
}

// One pill for the whole branch check, drawn from the dialog's first frame so
// its arrival moves nothing: a spinner while git is asked, a check with the
// answer once it answered, and the warning tone with the reason when it could
// not answer. Every state has an icon of its own, so the tone is never the only
// signal. The icon sits on the first line however many the text wraps to, and
// a pill that wraps becomes a rounded rectangle, since a capsule that tall has
// corners eating its text. A chip moves to the next line whole rather than
// splitting, unless it is longer than a line on its own; its negative margin
// keeps it from making its line taller than the text's, so a wrapped pill
// grows by whole lines and its padding stays even.
function BranchCheckPill({
  check,
}: {
  check: Exclude<BranchCheck, { state: "none" }>
}) {
  const text = useRef<HTMLSpanElement>(null)
  const wrapped = useWrapped(text)
  const [icon, prose, tone] =
    check.state === "pending"
      ? [
          <Loader2 className="mt-px size-3.5 shrink-0 motion-safe:animate-spin" />,
          branchesCheckingProse(),
          "border-border text-muted-foreground",
        ]
      : check.state === "checked"
        ? [
            <Check className="mt-px size-3.5 shrink-0 text-dux-success" />,
            check.prose,
            "border-dux-success/40 bg-dux-success/10",
          ]
        : [
            <CircleAlert className="mt-px size-3.5 shrink-0 text-dux-warning" />,
            check.prose,
            "border-dux-warning/40 bg-dux-warning/10",
          ]
  return (
    <div
      role="status"
      className={cn(
        "flex items-start gap-1.5 self-start border px-2.5 py-[5px] text-xs",
        check.state === "failed" || wrapped ? "rounded-lg" : "rounded-full",
        tone,
      )}
    >
      {icon}
      <span
        ref={text}
        className="min-w-0 [&_[data-slot=inline-code]]:-my-[3px] [&_[data-slot=inline-code]]:inline-block [&_[data-slot=inline-code]]:leading-none"
      >
        {renderProse(prose)}
      </span>
    </div>
  )
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
  const [entry, setEntry] = useState<OfferEntry | null>(null)

  const session = spine?.sessions.find((s) => s.id === deleteTarget)
  const ours = entry !== null && entry.target === deleteTarget ? entry : null
  const offer = ours?.offer ?? PENDING
  const { name, managed, folder } = deleteSessionView(session)
  const ready = offer.state === "ready" ? offer.offer : null
  const current = ready?.current ?? null
  const others = ready?.others ?? []
  const own = ready?.protected ?? []
  const [currentUncounted, othersUncounted] = ready
    ? countFailures(ready)
    : [null, null]
  const currentAnswerable = current !== null && currentUncounted === null
  const othersAnswerable = others.length > 0 && othersUncounted === null
  const deleteCurrent =
    current && currentAnswerable
      ? (currentAnswer ?? currentDefault(current))
      : false
  const deleteOthers = othersAnswerable
    ? (othersAnswer ?? othersDefault(others))
    : false
  const check = branchCheck(offer)
  // The first box is left out only for the project's own branch; a failed
  // check keeps it, disabled, with the reason said once, in the pill.
  const firstBoxShown =
    ready === null || current !== null || currentBranchFailed(ready)

  function reset() {
    setDeleteWorktree(false)
    setCurrentAnswer(null)
    setOthersAnswer(null)
    setEntry(null)
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

  // Asked once per open, as the dialog opens, so the answer is in hand by the
  // time the worktree box is ticked; ticking it again never asks again.
  const askOffer = isOpen && managed !== null
  const asked = ours !== null
  useEffect(() => {
    if (!askOffer || !deleteTarget || asked) return
    let live = true
    const answered = (next: OfferState) => {
      if (live) setEntry({ target: deleteTarget, offer: next, settled: false })
    }
    sessionsApi
      .branchUnpushed(deleteTarget)
      .then((answer) => answered({ state: "ready", offer: answer }))
      .catch((e: unknown) =>
        answered({
          state: "failed",
          reason: e instanceof Error ? e.message : String(e),
        }),
      )
    return () => {
      live = false
    }
  }, [askOffer, deleteTarget, asked])

  // A failed check keeps its pill, since the reason is the only place it is said.
  const checked = ours !== null && branchCheck(ours.offer).state === "checked"
  const checkedPending = checked && !ours?.settled
  useEffect(() => {
    if (!checkedPending) return
    const timer = setTimeout(
      () =>
        setEntry((prev) => (prev === null ? prev : { ...prev, settled: true })),
      CHECKED_PILL_MS,
    )
    return () => clearTimeout(timer)
  }, [checkedPending])
  // Nothing offered (the project's own branch) leaves nothing to say, so the
  // pending pill, which had to be drawn before the dialog could know, goes at once.
  const nothingOffered = ours !== null && branchCheck(ours.offer).state === "none"
  const pillShown = !(checked && ours?.settled) && !nothingOffered

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
          // One child of the dialog's grid, so the rows collapsing inside it
          // take their spacing with them instead of leaving a gap behind.
          <div className="flex flex-col">
            <CheckboxRow
              id="delete-worktree"
              checked={deleteWorktree}
              onCheckedChange={setDeleteWorktree}
            >
              Also delete the git worktree (irreversible)
            </CheckboxRow>
            {/* Revealed by the worktree box rather than sitting beside it disabled:
                git will not delete a branch still checked out, so there is
                nothing to offer. */}
            <Collapse open={deleteWorktree}>
              {firstBoxShown && (
                <CheckboxRow
                  id="delete-branch"
                  checked={deleteCurrent}
                  disabled={!currentAnswerable}
                  onCheckedChange={setCurrentAnswer}
                >
                  {renderProse(
                    currentBranchLabelProse(
                      current?.name ?? ready?.current_name ?? managed.branch_name,
                    ),
                  )}
                </CheckboxRow>
              )}
              {ready && current && (
                <BranchWarning prose={currentBranchWarningProse(current.origin)} />
              )}
              {ready && others.length > 0 && (
                <>
                  <CheckboxRow
                    id="delete-other-branches"
                    checked={deleteOthers}
                    disabled={!othersAnswerable}
                    onCheckedChange={setOthersAnswer}
                  >
                    {renderProse(
                      otherBranchesLabelProse(others.map((o) => o.name)),
                    )}
                  </CheckboxRow>
                  <BranchWarning
                    prose={otherBranchesWarningProse(others, deleteOthers)}
                  />
                </>
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
            </Collapse>
            <Collapse open={pillShown}>
              <AnimatedHeight>
                <BranchCheckPill
                  check={check.state === "none" ? { state: "pending" } : check}
                />
              </AnimatedHeight>
            </Collapse>
            {/* Misclick-safe spacing between the checkboxes and the buttons,
                one grid gap plus the h-2 every other dialog puts there. The
                pill already stands between them, so while it is up the
                spacing goes, and it grows back as the pill leaves rather than
                popping in after it. */}
            <div
              data-testid="delete-footer-spacing"
              className={cn(
                "shrink-0 transition-[height] duration-200 ease-out motion-reduce:transition-none",
                pillShown ? "h-0" : "h-6",
              )}
            />
          </div>
        )}
        <AttachedSection blockers={blockers} />
        {/* Misclick-safe spacing; a managed agent's rides with its pill. */}
        {!managed && <div className="h-2" />}
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
