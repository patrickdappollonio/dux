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
import { closeTabConsequences } from "@/lib/agentTabs"
import { DOCS_AGENT_TABS_CLOSING } from "@/lib/docs"
import { closeCloseTab, closeTab, useDux } from "@/lib/store"

// Confirmation before closing a tab. The agent's first tab reaches this dialog
// like any other: the session slot is a pointer, so closing the tab holding it
// hands the slot to the next tab in strip order rather than being refused.
//
// Two gestures deliberately do not come here. An agent's ONLY tab has no
// successor, so the server refuses that close and the tab strip's menu item is
// disabled with the reason rather than opening a dialog that would promise a
// detach and then 400. And the Task Manager's row for the first tab is a Stop,
// because a process monitor ends a process rather than deleting the row it is
// showing numbers for (ConfirmStopAgentDialog).
//
// Closing always confirms (matching the TUI), the copy states only the
// consequences that apply (`closeTabConsequences`), and Cancel is the default
// focus.
export function ConfirmCloseTabDialog() {
  const { closeTabTarget, spine } = useDux()

  const session = closeTabTarget
    ? spine?.sessions.find((s) => s.id === closeTabTarget.sessionId)
    : undefined
  const tab = closeTabTarget
    ? session?.tabs.find((t) => t.id === closeTabTarget.tabId)
    : undefined
  // The successor is named the way the strip labels it, upper-cased for prose:
  // the disambiguating suffix matters when two tabs share a provider ("Codex 2"
  // is a pill the user can point at, "codex" is two of them). The status message
  // the server sends back after the close is built from the same rule in Rust,
  // so the confirmation and the toast that follows it cannot name different tabs.
  const { sessionLabel, willDetach, successorLabel } = closeTabConsequences(
    session,
    tab,
  )

  // Closes the dialog when the tab (or its whole session) vanishes from the
  // ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    closeTabTarget !== null,
    tab !== undefined,
    closeCloseTab,
  )

  function handleConfirm() {
    if (!closeTabTarget) return
    closeTab(closeTabTarget.sessionId, closeTabTarget.tabId)
    closeCloseTab()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeCloseTab()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Close tab?</DialogTitle>
          <DialogDescription>
            {`This ends ${sessionLabel} in this tab and deletes the tab for good. A new tab always starts fresh, so use your provider's own history command to get back to this conversation.`}
            {willDetach
              ? " It's this agent's last live tab, so the agent detaches and stays in Projects, reopenable."
              : ""}
            {successorLabel
              ? ` The next tab, ${successorLabel}, takes its place as the agent's first tab.`
              : ""}{" "}
            <a
              href={DOCS_AGENT_TABS_CLOSING}
              target="_blank"
              rel="noopener noreferrer"
              className="text-primary underline underline-offset-2"
            >
              How closing a tab works →
            </a>
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI. shadcn/radix buttons
              activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeCloseTab}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Close tab
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
