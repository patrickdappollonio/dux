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
import { closeDetachesAgent, closeTabConsequences } from "@/lib/agentTabs"
import { renderProse } from "@/lib/prose"
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
  const { sessionLabel } = closeTabConsequences(session, tab)
  const willDetach = closeDetachesAgent(session, tab)

  // Closes the dialog when the tab (or its whole session) vanishes from the
  // ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    stopTabTarget !== null,
    tab !== undefined,
    closeStopTab,
  )

  function handleConfirm() {
    if (!stopTabTarget) return
    stopTab(stopTabTarget.sessionId, stopTabTarget.tabId)
    closeStopTab()
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
            This ends {renderProse(sessionLabel)} in this tab, interrupting
            whatever it is doing. The tab stays in the strip, ready to start
            again.
            {willDetach
              ? " It's this agent's last running tab, so the agent detaches and stays in Projects, reopenable."
              : ""}
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeStopTab}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Stop tab
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
