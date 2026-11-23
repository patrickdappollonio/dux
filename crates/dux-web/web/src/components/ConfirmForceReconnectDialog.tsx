import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { InlineCode } from "@/components/ui/inline-code"
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
  const isOpen = forceReconnectTarget !== null && session !== undefined
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

  return (
    <Modal
      kind="confirm"
      open={forceReconnectTarget !== null}
      vanished={session === undefined}
      onClose={closeForceReconnect}
      row={forceReconnectTarget === null ? null : { sessionId: forceReconnectTarget }}
      title={
        <>
          Force recreate {name ? <InlineCode>{name}</InlineCode> : "agent"}?
        </>
      }
      description="Do you want to force reconnect the agent? This will start a fresh session instead of continuing the existing session."
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Force recreate",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </Modal>
  )
}
