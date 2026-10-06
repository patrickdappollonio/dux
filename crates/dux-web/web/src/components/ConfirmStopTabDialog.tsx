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
import { closeDetachesAgent, tabProseLabel } from "@/lib/agentTabs"
import { sessionLabel } from "@/lib/agentWorkspace"
import { closeStopTab, stopTab, useDux } from "@/lib/store"

// Confirmation before stopping a tab, from a running tab's menu. Stopping ends
// the tab's process and keeps the tab, dormant, in the strip, so unlike a
// close it names no successor and deletes nothing; the agent detaches only
// when this was its last running tab. Cancel is the default focus.
export function ConfirmStopTabDialog() {
  const { stopTabTarget, spine } = useDux()

  const session = stopTabTarget
    ? spine?.sessions.find((s) => s.id === stopTabTarget.sessionId)
    : undefined
  const tab = stopTabTarget
    ? session?.tabs.find((t) => t.id === stopTabTarget.tabId)
    : undefined
  // Named as the terminal UI's dialog names them: the tab by its strip label,
  // the agent by its display name.
  const tabLabel =
    session && tab ? (tabProseLabel(session.tabs, tab.id) ?? tab.provider) : ""
  const agentLabel = session ? sessionLabel(session) : ""
  const willDetach = closeDetachesAgent(session, tab)

  // Closes the dialog when the tab (or its whole session) vanishes from the
  // ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    stopTabTarget !== null,
    tab !== undefined,
    closeStopTab,
  )
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    stopTabTarget && `${stopTabTarget.sessionId}/${stopTabTarget.tabId}`,
  )

  async function handleConfirm() {
    if (!stopTabTarget) return
    const { sessionId, tabId } = stopTabTarget
    const done = await confirm((accepted) => stopTab(sessionId, tabId, accepted))
    if (done) closeStopTab()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeStopTab()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Stop tab?</DialogTitle>
          <DialogDescription>
            Stop the <InlineCode>{tabLabel}</InlineCode> tab on{" "}
            <InlineCode>{agentLabel}</InlineCode>? This ends its session,
            interrupting whatever it is doing. The tab stays in the strip,
            ready to start again.
            {willDetach
              ? " It's this agent's last running tab, so the agent detaches and stays in Projects, reopenable."
              : ""}
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
            onClick={closeStopTab}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Stop tab"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
