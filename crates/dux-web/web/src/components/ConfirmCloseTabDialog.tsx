import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { InlineCode } from "@/components/ui/inline-code"
import { closeTabConsequences, sessionAsShown } from "@/lib/agentTabs"
import { DOCS_AGENT_TABS_CLOSING } from "@/lib/docs"
import { renderProse } from "@/lib/prose"
import { closeCloseTab, closeTab, useDux } from "@/lib/store"

// Confirmation before closing a tab. The agent's first tab reaches this dialog
// like any other: the session slot is a pointer, so closing the tab holding it
// hands the slot to the leftmost other tab rather than being refused.
//
// Two gestures deliberately do not come here. An agent's only tab has no
// successor, so the strip's menu item is disabled with the reason rather than
// opening a dialog the server would refuse. The Task Manager's row for the
// first tab is a Stop, because a process monitor ends a process rather than
// deleting the row it shows numbers for (ConfirmStopAgentDialog).
//
// The copy states only the consequences that apply (`closeTabConsequences`),
// and Cancel is the default focus.
export function ConfirmCloseTabDialog() {
  const { closeTabTarget, spine, pendingTabOrder, pendingSlotTab } = useDux()

  // The strip as the screen shows it, so the tab named as taking the slot is the
  // leftmost one the user sees, not the one the server's older order has first.
  const listed = closeTabTarget
    ? spine?.sessions.find((s) => s.id === closeTabTarget.sessionId)
    : undefined
  const session =
    listed && closeTabTarget
      ? sessionAsShown(
          listed,
          pendingTabOrder[closeTabTarget.sessionId],
          pendingSlotTab[closeTabTarget.sessionId],
        )
      : undefined
  const tab = closeTabTarget
    ? session?.tabs.find((t) => t.id === closeTabTarget.tabId)
    : undefined
  // The successor is named the way the strip labels it, so the disambiguating
  // suffix survives when two tabs share a provider. The server builds its
  // post-close status from the same rule, so the confirmation and the toast
  // after it cannot name different tabs.
  const { sessionLabel, willDetach, successorLabel } = closeTabConsequences(
    session,
    tab,
  )

  // Closes the dialog when the tab (or its whole session) vanishes from the
  // ViewModel.
  const isOpen = closeTabTarget !== null && tab !== undefined
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    closeTabTarget && `${closeTabTarget.sessionId}/${closeTabTarget.tabId}`,
  )

  async function handleConfirm() {
    if (!closeTabTarget) return
    const { sessionId, tabId } = closeTabTarget
    const done = await confirm((accepted) =>
      closeTab(sessionId, tabId, accepted),
    )
    if (done) closeCloseTab()
  }

  return (
    <Modal
      kind="confirm"
      open={closeTabTarget !== null}
      vanished={tab === undefined}
      onClose={closeCloseTab}
      title="Close tab?"
      description={
        <>
          This ends {renderProse(sessionLabel)} in this tab and deletes the
          tab for good. A new tab always starts fresh, so use your
          provider&rsquo;s own history command to get back to this
          conversation.
          {willDetach
            ? " It's this agent's last live tab, so the agent detaches and stays in Projects, reopenable."
            : ""}
          {successorLabel ? (
            <>
              {" "}
              <InlineCode>{successorLabel}</InlineCode> takes its place as the
              agent&rsquo;s first tab.
            </>
          ) : null}{" "}
          <a
            href={DOCS_AGENT_TABS_CLOSING}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary underline underline-offset-2"
          >
            How closing a tab works →
          </a>
        </>
      }
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Close tab",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </Modal>
  )
}
