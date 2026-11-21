import type { ReactNode } from "react"

import { Button } from "@/components/ui/button"
import {
  type AttachedBlocker,
  attachedDrivingLeadProse,
  attachedEntries,
  attachedEntryProse,
  attachedLeadProse,
  mayGoAheadOver,
} from "@/lib/attached"
import { renderProse } from "@/lib/prose"
import { useDux } from "@/lib/store"

// The parts every guarded confirmation shares (see `useAttachedOverride`).

/** Who the server named, listed under the dialog's body. Nothing until then. */
export function AttachedSection({
  blockers,
}: {
  blockers: readonly AttachedBlocker[] | null
}) {
  const { spine } = useDux()
  if (blockers === null || blockers.length === 0) return null
  const entries = attachedEntries(blockers, spine)
  // A device that is driving is named apart: the confirm button is withheld
  // for it (`GuardedConfirmButton`), so the list says what to do instead.
  const groups = [
    { lead: attachedDrivingLeadProse(), entries: entries.filter((entry) => entry.driving) },
    { lead: attachedLeadProse(), entries: entries.filter((entry) => !entry.driving) },
  ]
  return (
    <div className="space-y-2 text-sm">
      {groups.map(({ lead, entries }, group) =>
        entries.length === 0 ? null : (
          <div key={group} className="space-y-1">
            <p className="text-destructive">{renderProse(lead)}</p>
            <ul className="list-disc space-y-1 pl-5 text-muted-foreground">
              {entries.map((entry, index) => (
                <li key={index}>{renderProse(attachedEntryProse(entry))}</li>
              ))}
            </ul>
          </div>
        ),
      )}
    </div>
  )
}

/** A guarded dialog's destructive confirm. A double click's second click never
 * reaches the override that replaced the button under it. While a blocker is
 * driving there is no going ahead from here, so the button is disabled and the
 * section above it says to take over first. */
export function GuardedConfirmButton({
  verb,
  blockers,
  pending,
  onConfirm,
}: {
  verb: string
  blockers: readonly AttachedBlocker[] | null
  pending: boolean
  onConfirm: () => void
}): ReactNode {
  const driven = blockers !== null && !mayGoAheadOver(blockers)
  return (
    <Button
      variant="destructive"
      disabled={pending || driven}
      onClick={(event) => {
        if (blockers !== null && event.detail > 1) return
        onConfirm()
      }}
    >
      {blockers === null || driven ? verb : `${verb} anyway`}
    </Button>
  )
}
