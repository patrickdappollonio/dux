// The changed-files summary a CONTROL carries while the list itself is off
// screen: the phone's ±N button in the agent header (which opens the changes
// screen) and the desktop header's reopen button (which brings the hidden pane
// back). Both answer the same question, "how much has this agent changed", so
// they read one helper and cannot drift into saying different numbers or
// wording the count differently.
//
// It rides the existing changed-files broadcast: the store slice it reads is
// fed by `session.changes` events, so both controls update live with nothing
// polling on their behalf.

import { changesCountFor } from "@/lib/agentVitals"
import type { ChangesSlice } from "@/lib/store"

export interface ChangesSummary {
  // Staged plus unstaged files, every status weighted the same.
  count: number
  // What the control prints: a count, which is DATA, so it survives on
  // surfaces that otherwise prefer icon-only controls.
  label: string
  // What a screen reader is told the number means.
  countLabel: string
}

// The summary for the agent in view, or null when there is no agent for the
// count to be about (a focused project or standalone terminal), where the
// control carries its icon alone.
//
// An unloaded, failed or stale slice reads as zero rather than as "no summary":
// the count is a live figure that arrives moments later, and a control that
// appears and disappears under it would flicker on every selection change.
// Overloaded so a caller that already has an agent (the phone's agent screen)
// gets a summary rather than a maybe-summary it would have to unwrap.
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
    countLabel: `${count} changed files`,
  }
}
