// Pure search matching for the flat agent/terminal list, shared by the desktop
// sidebar and the mobile hub (and unit-tested in isolation, since a filter that
// silently drops a row is a data-loss bug). A query is matched case-insensitively
// as a substring against a small set of fields per row kind. An empty / whitespace
// query matches everything (the list is shown unfiltered).

import type { SessionView, TerminalView } from "@/lib/types"

export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase()
}

function haystackHas(query: string, ...fields: (string | null | undefined)[]): boolean {
  if (query === "") return true
  return fields.some((field) => (field ?? "").toLowerCase().includes(query))
}

// Match an agent row against a raw query. Fields: the display name
// (title, falling back to branch), the project name, the branch, and every
// provider the agent runs (its own provider plus each tab's provider, so a search
// for "codex" finds an agent whose codex tab is the interesting one).
export function matchesSessionQuery(
  session: SessionView,
  projectName: string,
  query: string,
): boolean {
  const q = normalizeQuery(query)
  if (q === "") return true
  const providers = [session.provider, ...session.tabs.map((tab) => tab.provider)]
  return (
    haystackHas(q, session.title, session.branch_name, projectName) ||
    providers.some((provider) => provider.toLowerCase().includes(q))
  )
}

// Match a terminal row against a raw query. Fields: the terminal title (its
// running foreground command or its stable label), the owner label ("agent name"
// or "project"), and the project name.
export function matchesTerminalQuery(
  terminal: TerminalView,
  ownerLabel: string,
  projectName: string,
  query: string,
): boolean {
  const q = normalizeQuery(query)
  return haystackHas(q, terminal.label, terminal.foreground_cmd, ownerLabel, projectName)
}

// The CODE-POINT range (start inclusive, end exclusive) of the first
// case-insensitive occurrence of `query` in `field`, or null when the query is
// empty/whitespace or does not occur. The search-hit highlight uses this to
// emphasize the matched part of a row's name, applying the exact normalization
// the filter applies (normalizeQuery + lowercase includes): what highlights is
// what matched. The TS twin of dux-core's `match_char_range` (the Rust side of
// the shared matcher); the two share test vectors.
//
// Code points, deliberately: labels carry emoji/CJK, and both byte offsets and
// raw UTF-16 indices would land a highlight mid-character. Lowercasing can
// EXPAND a code point (ß to ss), so the haystack is lowered per code point
// while recording each lowered point's SOURCE index; the range maps back
// through that record, keeping the highlight aligned however the case-folding
// reshaped the string.
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
