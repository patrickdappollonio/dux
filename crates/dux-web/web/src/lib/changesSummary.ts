// The changed-files summary a control carries while the list itself is off screen: the phone's
// ±N header button and the desktop header's reopen button. One helper, so the two cannot say
// different numbers. It rides the `session.changes` broadcast, so both update with no polling.

import { changesCountFor } from "@/lib/agentVitals"
import { formatRegularCount } from "@/lib/formatRegularCount"
import type { ChangesSlice } from "@/lib/store"

export interface ChangesSummary {
  // Staged plus unstaged files, every status weighted the same.
  count: number
  // What the control prints: a count, which is data, so it survives on
  // surfaces that otherwise prefer icon-only controls.
  label: string
  // What a screen reader is told the number means.
  countLabel: string
}

// The summary for the agent in view, or null when there is no agent for the count to be about
// (a focused project or standalone terminal), where the control carries its icon alone. An
// unloaded, failed or stale slice reads as zero rather than as "no summary": the figure arrives
// moments later, and a control appearing and disappearing under it would flicker.
export function changesSummary(
  changes: ChangesSlice | null | undefined,
  sessionId: string,
): ChangesSummary
export function changesSummary(
  changes: ChangesSlice | null | undefined,
  sessionId: string | null | undefined,
): ChangesSummary | null
export function changesSummary(
  changes: ChangesSlice | null | undefined,
  sessionId: string | null | undefined,
): ChangesSummary | null {
  if (!sessionId) return null
  const count = changesCountFor(changes, sessionId) ?? 0
  return {
    count,
    label: `±${count}`,
    countLabel: formatRegularCount(count, "changed file"),
  }
}
