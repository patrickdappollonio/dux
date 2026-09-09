// Pure comparators backing the shared agent-list display sort, the twin of the core-owned
// ordering in `dux_core::flat_list::order_sessions` and pinned by shared vectors, so a mode
// set on either surface produces the same order. The mode is `config.ui.agent_sort`.

import type { SessionView } from "./types"
import { sessionLabel } from "@/lib/agentWorkspace"

export type SortKey = "updated" | "created" | "name" | "name_desc"

// The name key is the agent's display label lowercased: `sessionLabel` falls back to the
// branch for a managed agent and to the folder's name for a standalone one.
function nameKey(s: SessionView): string {
  return sessionLabel(s).toLowerCase()
}

// Parse an RFC 3339 timestamp to epoch milliseconds. An unparseable value sorts as 0 rather
// than poisoning the comparator with NaN.
function epoch(iso: string): number {
  const ms = Date.parse(iso)
  return Number.isNaN(ms) ? 0 : ms
}

// Compare by name using Unicode code points, not UTF-16 code units: plain JS comparison orders
// an astral-plane char by its surrogate halves, flipping the order Rust's code-point `str::cmp`
// produces, and the two surfaces would then disagree. Returns <0 / 0 / >0 ascending.
function compareName(a: SessionView, b: SessionView): number {
  const ka = [...nameKey(a)]
  const kb = [...nameKey(b)]
  const len = Math.min(ka.length, kb.length)
  for (let i = 0; i < len; i++) {
    const ca = ka[i].codePointAt(0) ?? 0
    const cb = kb[i].codePointAt(0) ?? 0
    if (ca !== cb) return ca - cb
  }
  return ka.length - kb.length
}

// The session ids sorted by `by`, mirroring the TUI comparators:
//   updated / created → newest first
//   name              → case-insensitive ascending on the display label
//   name_desc         → the exact reverse of name; the TUI sets it, the web only displays it
//
// Both surfaces sort stably, so equal keys keep their original relative order. Sorts a copy,
// leaving the caller's array untouched.
export function sortedSessionIds(
  sessions: SessionView[],
  by: SortKey,
): string[] {
  const sorted = sessions.slice()
  switch (by) {
    case "updated":
      sorted.sort((a, b) => epoch(b.updated_at) - epoch(a.updated_at))
      break
    case "created":
      sorted.sort((a, b) => epoch(b.created_at) - epoch(a.created_at))
      break
    case "name":
      sorted.sort(compareName)
      break
    case "name_desc":
      // Exactly the reverse of "name": same code-point key, inverted result.
      sorted.sort((a, b) => -compareName(a, b))
      break
  }
  return sorted.map((s) => s.id)
}
