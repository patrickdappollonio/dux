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
import { forceStopConfirmBody } from "@/lib/detachAgent"
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

  function handleConfirm() {
    if (!forceStopAgentTarget) return
    killSessionPty(forceStopAgentTarget, true)
    closeForceStopAgent()
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeForceStopAgent()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Force stop agent?</DialogTitle>
          <DialogDescription>{forceStopConfirmBody(label)}</DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeForceStopAgent}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Force stop
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
