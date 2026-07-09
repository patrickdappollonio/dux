import { useEffect } from "react"

// Close a target-keyed dialog when the entity it points at disappears from
// the live ViewModel while the dialog is open (deleted from another connected
// client, or torn down by the server). `targetSet` is whether the dialog's
// store target is set; `present` is whether that target's entity still
// resolves, the caller computes it however its lookup works (a flat
// spine.sessions find, a nested tab/terminal find, a session AND its project,
// or a non-spine slice like the changed-files list). Returns the dialog's
// effective open state: target set AND entity present, so a vanished target
// never renders a stale body even for the frame before the effect runs.
//
// Deliberate non-users: RemoveProjectDialog opens for ghost/orphaned projects
// that have no live project record (vanishing is its use case), and
// CreateAgentDialog's target carries its own draft/mode data with entities
// used only for cosmetic labels.
export function useVanishedTargetGuard(
  targetSet: boolean,
  present: boolean,
  close: () => void,
): boolean {
  useEffect(() => {
    if (targetSet && !present) close()
  }, [targetSet, present, close])
  return targetSet && present
}
