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

/** Who the server named, listed under the dialog's body. Nothing until then.
 * It appears only after the user pressed the action and the server refused,
 * while focus moves to Cancel, so it is one alert region: the reason is read,
 * not just the button focus lands on. */
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
    <div role="alert" className="space-y-2 text-sm">
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
