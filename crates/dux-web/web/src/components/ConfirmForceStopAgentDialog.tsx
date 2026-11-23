import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { sessionLabel } from "@/lib/agentWorkspace"
import { forceStopConfirmProse } from "@/lib/detachAgent"
import { renderProse } from "@/lib/prose"
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
  const isOpen = forceStopAgentTarget !== null && session !== undefined
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    forceStopAgentTarget,
  )

  async function handleConfirm() {
    if (!forceStopAgentTarget) return
    const target = forceStopAgentTarget
    const done = await confirm((accepted) =>
      killSessionPty(target, true, accepted),
    )
    if (done) closeForceStopAgent()
  }

  return (
    <Modal
      kind="confirm"
      open={forceStopAgentTarget !== null}
      vanished={session === undefined}
      onClose={closeForceStopAgent}
      row={forceStopAgentTarget === null ? null : { sessionId: forceStopAgentTarget }}
      title="Force stop agent?"
      description={renderProse(forceStopConfirmProse(label))}
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Force stop",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </Modal>
  )
}
