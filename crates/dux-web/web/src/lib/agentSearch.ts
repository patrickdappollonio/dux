// Pure search matching for the flat agent/terminal list, shared by the desktop sidebar and
// the mobile hub. A query matches case-insensitively as a substring against a small set of
// fields per row kind; an empty or whitespace query matches everything.

import type { SessionView, TerminalView } from "@/lib/types"
import { workspaceBranchName, workspaceLocation } from "@/lib/agentWorkspace"

export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase()
}

function haystackHas(query: string, ...fields: (string | null | undefined)[]): boolean {
  if (query === "") return true
  return fields.some((field) => (field ?? "").toLowerCase().includes(query))
}

// Match an agent row against a raw query: display name, location, branch. Provider names are
// deliberately not searched, since "claude"/"codex" would surface almost every agent.
// Mirrors the Rust `matches_session` in dux-core's agent_search.rs, over shared vectors.
export function matchesSessionQuery(
  session: SessionView,
  location: string,
  query: string,
): boolean {
  const q = normalizeQuery(query)
  if (q === "") return true
  // `location` is whatever the row's second line shows, so typing part of a path finds a
  // standalone agent: the field that appears on the row is the field that matches.
  return haystackHas(
    q,
    session.title,
    workspaceBranchName(session.workspace),
    location,
  )
}

// Match a terminal row against a raw query: its label, its running foreground command, the
// owner label, and the project name.
export function matchesTerminalQuery(
  terminal: TerminalView,
  ownerLabel: string,
  projectName: string,
  query: string,
): boolean {
  const q = normalizeQuery(query)
  return haystackHas(q, terminal.label, terminal.foreground_cmd, ownerLabel, projectName)
}

// The code-point range (start inclusive, end exclusive) of the first case-insensitive
// occurrence of `query` in `field`, or null when the query is empty or does not occur. It
// applies the exact normalization the filter does, so what highlights is what matched.
// Code points, because labels carry emoji and CJK and a byte or UTF-16 index would land a
// highlight mid-character; lowercasing can expand a code point (ß to ss), so each lowered
// point records its source index and the range maps back through that.
// The TS twin of dux-core's `match_char_range`, over shared test vectors.
export function matchCharRange(
  field: string,
  query: string,
): { start: number; end: number } | null {
  const q = Array.from(normalizeQuery(query))
  if (q.length === 0) return null
  const lowered: string[] = []
  const sourceIndex: number[] = []
  Array.from(field).forEach((ch, index) => {
    for (const lower of Array.from(ch.toLowerCase())) {
      lowered.push(lower)
      sourceIndex.push(index)
    }
  })
  if (q.length > lowered.length) return null
  for (let start = 0; start <= lowered.length - q.length; start++) {
    let hit = true
    for (let i = 0; i < q.length; i++) {
      if (lowered[start + i] !== q[i]) {
        hit = false
        break
      }
    }
    if (hit) {
      return {
        start: sourceIndex[start],
        end: sourceIndex[start + q.length - 1] + 1,
      }
    }
  }
  return null
}

/** The location field an agent row shows, and therefore the one its search matches: the
 * project's name for a managed agent, the home-collapsed folder for a standalone one. One
 * helper for the filter and the highlight, so neither can use a field the other does not.
 * The Rust twin is `agent_search_location` in the terminal UI. */
export function agentSearchLocation(
  session: SessionView,
  projectName: (id: string) => string,
): string {
  const location = workspaceLocation(session.workspace)
  return location.kind === "project"
    ? projectName(location.projectId)
    : location.label
}
