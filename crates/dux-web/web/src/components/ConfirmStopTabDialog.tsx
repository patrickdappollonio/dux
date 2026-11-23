import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { InlineCode } from "@/components/ui/inline-code"
import { closeDetachesAgent, tabProseLabel } from "@/lib/agentTabs"
import { sessionLabel } from "@/lib/agentWorkspace"
import { closeStopTab, stopTab, useDux } from "@/lib/store"

// Confirmation before stopping a tab: the tab stays in the strip, dormant, so
// unlike ConfirmCloseTabDialog it names no successor and deletes nothing.
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

  const isOpen = stopTabTarget !== null && tab !== undefined
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

  return (
    <Modal
      kind="confirm"
      open={stopTabTarget !== null}
      vanished={tab === undefined}
      onClose={closeStopTab}
      row={stopTabTarget && { sessionId: stopTabTarget.sessionId }}
      title="Stop tab?"
      description={
        <>
          Stop the <InlineCode>{tabLabel}</InlineCode> tab on{" "}
          <InlineCode>{agentLabel}</InlineCode>? This ends its session,
          interrupting whatever it is doing. The tab stays in the strip,
          ready to start again.
          {willDetach
            ? " It's this agent's last running tab, so the agent detaches and stays in Projects, reopenable."
            : ""}
        </>
      }
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Stop tab",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </Modal>
  )
}
