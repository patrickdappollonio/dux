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
import {
  closeForceReconnect,
  reconnectSession,
  useDux,
} from "@/lib/store"

// Confirmation before force-recreating an agent ("Force recreate agent…" in the
// agent ⋯ menus). A forced reconnect relaunches the provider WITHOUT resume
// args, so the current conversation is abandoned for a fresh session. That is
// worth a deliberate confirm rather than a single misclickable menu item.
// Cancel is the default focus, matching the other confirm dialogs.
export function ConfirmForceReconnectDialog() {
  const { forceReconnectTarget, spine } = useDux()

  // Resolve the session from the ViewModel so an agent deleted while the dialog
  // is open closes it instead of confirming against a gone target.
  const session = forceReconnectTarget
    ? spine?.sessions.find((s) => s.id === forceReconnectTarget)
    : undefined
  // Closes the dialog when the agent vanishes from the ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    forceReconnectTarget !== null,
    session !== undefined,
    closeForceReconnect,
  )
  const name = session ? session.title || session.branch_name : ""

  function handleConfirm() {
    if (!forceReconnectTarget) return
    reconnectSession(forceReconnectTarget, true)
    closeForceReconnect()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeForceReconnect()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            Force recreate <span className="break-all">{name || "agent"}</span>?
          </DialogTitle>
          <DialogDescription>
            Do you want to force reconnect the agent? This will start a fresh
            session instead of continuing the existing session.
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI. shadcn/base-ui
              buttons activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeForceReconnect}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Force recreate
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
