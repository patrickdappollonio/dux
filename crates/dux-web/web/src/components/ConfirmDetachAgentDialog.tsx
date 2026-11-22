import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { sessionLabel } from "@/lib/agentWorkspace"
import { detachConfirmProse, shutdownGraceSeconds } from "@/lib/detachAgent"
import { renderProse } from "@/lib/prose"
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
  // dormant tab left over from a restart keeps nothing alive. The shared body
  // builder decides whether that earns a sentence.
  const liveTabs = session?.tabs.filter((t) => t.has_live_process).length ?? 0

  // Closes itself when the agent leaves the live view model, like every other
  // target-keyed dialog.
  const isOpen = stopAgentTarget !== null && session !== undefined
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    stopAgentTarget,
  )

  async function handleConfirm() {
    if (!stopAgentTarget) return
    const target = stopAgentTarget
    const done = await confirm((accepted) =>
      killSessionPty(target, false, accepted),
    )
    if (done) closeStopAgent()
  }

  return (
    <Modal
      kind="confirm"
      open={stopAgentTarget !== null}
      vanished={session === undefined}
      onClose={closeStopAgent}
      title="Detach agent?"
      description={renderProse(
        detachConfirmProse(label, shutdownGraceSeconds(bootstrap), liveTabs),
      )}
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Detach",
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {blockers?.length ? <AttachedSection blockers={blockers} /> : null}
    </Modal>
  )
}
