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
