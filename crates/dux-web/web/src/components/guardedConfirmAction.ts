import type { ModalButtonAction } from "@/components/modal"
import { type AttachedBlocker, mayGoAheadOver } from "@/lib/attached"

const DRIVING_BLOCKER_REASON =
  "Another device is driving this right now. Take it over on this device first."

/** A guarded confirm's destructive action. Once the server has named a device
 * that only has the target open, it becomes the override ("… anyway"); while
 * one is driving there is no going ahead from here. */
export function guardedConfirmAction({
  verb,
  blockers,
  pending,
  onConfirm,
  drivenHint = null,
}: {
  verb: string
  blockers: readonly AttachedBlocker[] | null
  pending: boolean
  onConfirm: () => void
  /** What this page already knows another device drives, before sending. */
  drivenHint?: string | null
}): ModalButtonAction {
  const driven = blockers !== null && !mayGoAheadOver(blockers)
  return {
    label: blockers === null || driven ? verb : `${verb} anyway`,
    tone: "destructive",
    busy: pending,
    disabledReason: drivenHint ?? (driven ? DRIVING_BLOCKER_REASON : null),
    onSelect: onConfirm,
  }
}
