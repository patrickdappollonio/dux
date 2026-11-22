import {
  AttachedSection,
  DrivenElsewhereSection,
} from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import {
  drivenElsewhereHint,
  useProjectDrivenElsewhere,
} from "@/hooks/use-agent-active-elsewhere"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { Modal } from "@/components/modal"
import { removeProjectProse } from "@/lib/projectConfirm"
import { renderProse } from "@/lib/prose"
import { closeRemoveProject, removeProject, useDux } from "@/lib/store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

export function RemoveProjectDialog() {
  const { removeProjectTarget, spine } = useDux()

  const project = spine?.projects.find((p) => p.id === removeProjectTarget)
  // For an orphaned ("ghost") project there is no project record, so fall back
  // to the short-id name the sidebar shows for its group.
  const orphanName = spine?.sidebar.groups.find(
    (g) => g.project_id === removeProjectTarget,
  )?.name
  const name = project?.name ?? orphanName
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === removeProjectTarget,
    ).length ?? 0
  // A ghost project has no record, only its group and its agents, so it is
  // gone once none of the three is left.
  const present = project !== undefined || orphanName !== undefined || agentCount > 0
  const isOpen = removeProjectTarget !== null && present
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    removeProjectTarget,
  )
  const driven = useProjectDrivenElsewhere(removeProjectTarget)

  async function handleConfirm() {
    if (!removeProjectTarget) return
    const target = removeProjectTarget
    const done = await confirm((accepted) => removeProject(target, accepted))
    if (done) closeRemoveProject()
  }

  return (
    <Modal
      kind="confirm"
      open={removeProjectTarget !== null}
      vanished={!present}
      onClose={closeRemoveProject}
      title="Remove project?"
      description={renderProse(removeProjectProse(name, agentCount))}
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Remove",
        drivenHint: drivenElsewhereHint(driven),
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {driven.length > 0 || blockers?.length ? (
        <>
          <DrivenElsewhereSection driven={driven} />
          <AttachedSection blockers={blockers} />
        </>
      ) : null}
    </Modal>
  )
}
