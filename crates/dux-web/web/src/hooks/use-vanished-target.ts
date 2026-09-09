import { useEffect } from "react"

// Close a target-keyed dialog when the entity it points at disappears from the live ViewModel
// while the dialog is open. `targetSet` is whether the dialog's store target is set; `present`
// is whether that target's entity still resolves, which the caller computes however its lookup
// works. Returns the dialog's effective open state, target set and entity present, so a
// vanished target never renders a stale body even for the frame before the effect runs.
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
