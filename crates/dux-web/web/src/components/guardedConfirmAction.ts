import type { ModalButtonAction } from "@/components/modal"
import { type AttachedBlocker, mayGoAheadOver } from "@/lib/attached"
import { joinWords } from "@/lib/changedFiles"
import { deviceLabel } from "@/lib/deviceLabel"

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
    disabledReason: drivenHint ?? (driven ? drivingReason(blockers) : null),
    onSelect: onConfirm,
  }
}

/** Why the confirm is withheld, naming each device that drives: one sentence
 * for a single row, another when the change would reach several rows. */
function drivingReason(blockers: readonly AttachedBlocker[]): string {
  const driving = blockers.filter((blocker) => blocker.driving)
  const labels = driving.map(
    (blocker) =>
      deviceLabel(blocker.device) ??
      (blocker.surface === "terminal_ui" ? "the dux TUI" : null),
  )
  const known = [...new Set(labels.filter((label) => label !== null))]
  const names =
    known.length === 0
      ? ["another device"]
      : labels.includes(null)
        ? [...known, "another device"]
        : known
  const who = joinWords(names)
  const subject = `${who.charAt(0).toUpperCase()}${who.slice(1)} ${names.length > 1 ? "are" : "is"}`
  const rows = new Set(
    driving.map(({ target }) => target.agent ?? `${target.kind}:${target.id}`),
  )
  return rows.size > 1
    ? `${subject} driving agents or terminals this would change. Take them over on this device first.`
    : `${subject} driving this right now. Take it over on this device first.`
}
