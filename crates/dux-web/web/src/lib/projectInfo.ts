// Pure helpers backing the read-only "Project info…" modal. Kept React-free so
// they're trivially unit-testable. Both compute purely from the ViewModel — no
// wire commands, no git reads.

import type { SessionView } from "./types"

// Live agent + companion-terminal counts for a project, derived from the current
// sessions list. `agents` is the number of sessions owned by the project;
// `terminals` is the sum of companion terminals across those sessions.
export interface ProjectLiveCounts {
  agents: number
  terminals: number
}

export function projectLiveCounts(
  projectId: string,
  sessions: SessionView[],
): ProjectLiveCounts {
  let agents = 0
  let terminals = 0
  for (const session of sessions) {
    if (session.project_id !== projectId) continue
    agents += 1
    terminals += session.terminals.length
  }
  return { agents, terminals }
}

// Format an RFC 3339 / ISO 8601 timestamp as a human-readable "added" date
// (e.g. "Feb 3, 2026"). Returns "Unknown" for an empty string (a project with
// no store row yet) or an unparseable value, so the modal never renders a raw
// ISO string or "Invalid Date".
export function formatAddedDate(iso: string): string {
  if (iso.trim() === "") return "Unknown"
  const ms = Date.parse(iso)
  if (Number.isNaN(ms)) return "Unknown"
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  })
}
