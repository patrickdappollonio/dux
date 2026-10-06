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
import { InlineCode } from "@/components/ui/inline-code"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import {
  closeForceReconnect,
  reconnectSession,
  useDux,
} from "@/lib/store"
import { sessionLabel } from "@/lib/agentWorkspace"

// Confirmation before force-recreating an agent. A forced reconnect relaunches
// the provider without resume args, abandoning the current conversation for a
// fresh session. Cancel takes focus, as in the other confirm dialogs.
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
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    forceReconnectTarget,
  )
  const name = session ? sessionLabel(session) : ""

  async function handleConfirm() {
    if (!forceReconnectTarget) return
    const target = forceReconnectTarget
    const done = await confirm((accepted) => reconnectSession(target, true, accepted))
    if (done) closeForceReconnect()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeForceReconnect()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            Force recreate{" "}
            {name ? <InlineCode>{name}</InlineCode> : "agent"}?
          </DialogTitle>
          <DialogDescription>
            Do you want to force reconnect the agent? This will start a fresh
            session instead of continuing the existing session.
          </DialogDescription>
        </DialogHeader>
        <AttachedSection blockers={blockers} />
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI. shadcn/base-ui
              buttons activate on Space/Enter natively. */}
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeForceReconnect}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Force recreate"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
