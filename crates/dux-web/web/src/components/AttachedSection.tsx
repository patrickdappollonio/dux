import type { ReactNode } from "react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { Button } from "@/components/ui/button"
import type { DrivenElsewhere } from "@/hooks/use-agent-active-elsewhere"
import {
  type AttachedBlocker,
  attachedDrivingLeadProse,
  attachedEntries,
  attachedEntryProse,
  attachedLeadProse,
} from "@/lib/attached"
import { chip, renderProse, type Prose } from "@/lib/prose"
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
  // for it (`guardedConfirmAction`), so the list says what to do instead.
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

/** What of the change's target this page already knows another device drives,
 * listed before anything is sent: the server would refuse the change. */
export function DrivenElsewhereSection({ driven }: { driven: readonly DrivenElsewhere[] }) {
  if (driven.length === 0) return null
  return (
    <div className="space-y-1 text-sm">
      <p className="text-destructive">{renderProse(attachedDrivingLeadProse())}</p>
      <ul className="list-disc space-y-1 pl-5 text-muted-foreground">
        {driven.map((part, index) => (
          <li key={index}>{renderProse(drivenElsewhereProse(part))}</li>
        ))}
      </ul>
    </div>
  )
}

function drivenElsewhereProse(part: DrivenElsewhere): Prose {
  return [
    `${part.kind} `,
    chip(part.name),
    ...(part.device === null
      ? [", driven from another device"]
      : [", driven from ", chip(part.device)]),
  ]
}

/** Why a guarded confirm cannot go ahead while a blocker is driving. */
/** `guardedConfirmAction` as a button, for a dialog not built with `Modal`.
 * A double click's second click never reaches the override that replaced the
 * button under it. */
export function GuardedConfirmButton({
  verb,
  blockers,
  pending,
  onConfirm,
  drivenHint,
}: {
  verb: string
  blockers: readonly AttachedBlocker[] | null
  pending: boolean
  onConfirm: () => void
  drivenHint?: string | null
}): ReactNode {
  const action = guardedConfirmAction({ verb, blockers, pending, onConfirm, drivenHint })
  const knownDriven = drivenHint !== undefined && drivenHint !== null
  const button = (
    <Button
      variant="destructive"
      disabled={pending || action.disabledReason !== null}
      className={drivenHint === undefined ? undefined : "w-full"}
      onClick={(event) => {
        if (blockers !== null && event.detail > 1) return
        onConfirm()
      }}
    >
      {action.label}
    </Button>
  )
  if (drivenHint === undefined) return button
  return (
    <ExplainedWhenDisabled disabled={knownDriven} hint={drivenHint} className="rounded-lg">
      {button}
    </ExplainedWhenDisabled>
  )
}
