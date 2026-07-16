// Pure helpers backing the read-only info modals (project and agent). Kept
// React-free so they're trivially unit-testable. All compute purely from the
// ViewModel — no wire commands, no git reads.

import type { SessionView, TerminalView } from "./types"

// Live agent + terminal counts for a project, derived from the current
// sessions list plus the project's own terminals. `agents` is the number of
// sessions owned by the project; `terminals` is the sum of companion terminals
// across those sessions PLUS the project's own project terminals.
export interface ProjectLiveCounts {
  agents: number
  terminals: number
}

export function projectLiveCounts(
  projectId: string,
  sessions: SessionView[],
  projectTerminals: TerminalView[] = [],
): ProjectLiveCounts {
  let agents = 0
  let terminals = projectTerminals.length
  for (const session of sessions) {
    if (session.project_id !== projectId) continue
    agents += 1
    terminals += session.terminals.length
  }
  return { agents, terminals }
}

// Format an RFC 3339 / ISO 8601 timestamp as a human-readable date
// (e.g. "Feb 3, 2026"). A shared date formatter for the info modals (project
// "Added", agent "Created"/"Updated"). Returns "Unknown" for an empty string (a
// record with no store row yet) or an unparseable value, so the modal never
// renders a raw ISO string or "Invalid Date".
export function formatDisplayDate(iso: string): string {
  if (iso.trim() === "") return "Unknown"
  const ms = Date.parse(iso)
  if (Number.isNaN(ms)) return "Unknown"
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  })
}
