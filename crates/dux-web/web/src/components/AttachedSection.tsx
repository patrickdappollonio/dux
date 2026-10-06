import type { ReactNode } from "react"

import { Button } from "@/components/ui/button"
import {
  type AttachedBlocker,
  attachedEntries,
  attachedEntryProse,
  attachedLeadProse,
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
  return (
    <div className="space-y-1 text-sm">
      <p className="text-destructive">{renderProse(attachedLeadProse())}</p>
      <ul className="list-disc space-y-1 pl-5 text-muted-foreground">
        {attachedEntries(blockers, spine).map((entry, index) => (
          <li key={index}>{renderProse(attachedEntryProse(entry))}</li>
        ))}
      </ul>
    </div>
  )
}

/** A guarded dialog's destructive confirm. A double click's second click never
 * reaches the override that replaced the button under it. */
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
  return (
    <Button
      variant="destructive"
      disabled={pending}
      onClick={(event) => {
        if (blockers !== null && event.detail > 1) return
        onConfirm()
      }}
    >
      {blockers === null ? verb : `${verb} anyway`}
    </Button>
  )
}
