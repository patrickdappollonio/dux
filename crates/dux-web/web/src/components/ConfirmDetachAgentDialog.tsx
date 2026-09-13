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
import { detachConfirmBody, shutdownGraceSeconds } from "@/lib/detachAgent"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { closeStopAgent, killSessionPty, useDux } from "@/lib/store"

// The confirmation behind detaching an agent, from the agent's row menu and
// from the Task Manager's Stop on an agent row. ONE dialog for both, so the two
// entry points cannot promise different things.
//
// It lives at the app root rather than inside the Task Manager, because the row
// menu opens it with the Task Manager closed.
//
// Detaching is the polite path: dux asks the process to shut down, waits the
// configured grace, then forces it. The body says both halves, and it takes the
// number from the bootstrap document, because the wait is a setting.
export function ConfirmDetachAgentDialog() {
  const { stopAgentTarget, spine, bootstrap } = useDux()

  const session = stopAgentTarget
    ? spine?.sessions.find((s) => s.id === stopAgentTarget)
    : undefined
  const label = session ? sessionLabel(session) : ""
  // How many processes this actually ends. Counted by liveness, because a
  // dormant tab left over from a restart keeps nothing alive.
  const liveTabs =
    session?.tabs.filter((t) => t.has_live_process).length ?? 0

  // Closes itself when the agent leaves the live view model, like every other
  // target-keyed dialog.
  const isOpen = useVanishedTargetGuard(
    stopAgentTarget !== null,
    session !== undefined,
    closeStopAgent,
  )

  function handleConfirm() {
    if (!stopAgentTarget) return
    killSessionPty(stopAgentTarget)
    closeStopAgent()
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeStopAgent()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Detach agent?</DialogTitle>
          <DialogDescription>
            {detachConfirmBody(label, shutdownGraceSeconds(bootstrap))}
            {liveTabs > 1
              ? ` All ${formatRegularCount(liveTabs, "running tab")} stop together.`
              : ""}
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeStopAgent}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Detach
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
