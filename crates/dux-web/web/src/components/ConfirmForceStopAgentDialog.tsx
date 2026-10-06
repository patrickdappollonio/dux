import {
  AttachedSection,
  GuardedConfirmButton,
} from "@/components/AttachedSection"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { sessionLabel } from "@/lib/agentWorkspace"
import { forceStopConfirmProse } from "@/lib/detachAgent"
import { renderProse } from "@/lib/prose"
import { closeForceStopAgent, killSessionPty, useDux } from "@/lib/store"

// The confirmation behind the Task Manager's Force stop on an agent row.
//
// A sibling of `ConfirmDetachAgentDialog` rather than a mode inside it: that
// dialog exists so the row menu and the palette cannot promise different
// things, and this one deliberately promises something else. The Task Manager is
// the panic surface, so its stop is immediate and quotes no shutdown grace.
//
// It lives at the app root beside its polite sibling, because both are
// target-keyed and neither belongs to the surface that opened it.
export function ConfirmForceStopAgentDialog() {
  const { forceStopAgentTarget, spine } = useDux()

  const session = forceStopAgentTarget
    ? spine?.sessions.find((s) => s.id === forceStopAgentTarget)
    : undefined
  const label = session ? sessionLabel(session) : ""

  // Closes itself when the agent leaves the live view model, like every other
  // target-keyed dialog.
  const isOpen = useVanishedTargetGuard(
    forceStopAgentTarget !== null,
    session !== undefined,
    closeForceStopAgent,
  )
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(isOpen)

  async function handleConfirm() {
    if (!forceStopAgentTarget) return
    const target = forceStopAgentTarget
    const done = await confirm((force) =>
      killSessionPty(target, true, force),
    )
    if (done) closeForceStopAgent()
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeForceStopAgent()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Force stop agent?</DialogTitle>
          <DialogDescription>
            {renderProse(forceStopConfirmProse(label))}
          </DialogDescription>
        </DialogHeader>
        <AttachedSection blockers={blockers} />
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeForceStopAgent}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Force stop"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
