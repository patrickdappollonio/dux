// Pure sort helper backing the web's "Sort agents by …" palette commands. These
// mirror the TUI palette commands sort-agents-by-{updated,created,name} EXACTLY
// (dux-tui/src/app/mod.rs sort_sessions_by_*), so a sort on either surface
// produces the same order. The web has no dedicated sort state: it computes the
// sorted id order here and feeds it back through the existing `reorder_sessions`
// wire command (which the server persists into the same shared order the TUI
// uses). Kept React-free so it's trivially unit-testable.

import type { SessionView } from "./types"

export type SortKey = "updated" | "created" | "name"

// The TUI's name key: title.as_deref().unwrap_or(&branch_name), lowercased.
function nameKey(s: SessionView): string {
  return (s.title ?? s.branch_name).toLowerCase()
}

// Parse an RFC 3339 / ISO 8601 timestamp to epoch milliseconds. The server
// always emits valid `to_rfc3339()` output, but guard against NaN (an
// unparseable value sorts as 0 rather than poisoning the comparator).
function epoch(iso: string): number {
  const ms = Date.parse(iso)
  return Number.isNaN(ms) ? 0 : ms
}

// Return the session ids sorted by `by`, mirroring the TUI comparators:
//   updated / created → newest first (Rust `Reverse(timestamp)`)
//   name              → case-insensitive ascending on title-or-branch
//
// Stability: the TUI uses Rust's `sort_by_key` / `sort_by`, which are stable.
// `Array.prototype.sort` is required to be stable by the ECMAScript spec, so
// equal keys (e.g. identical timestamps or identical names) keep their original
// relative order on both surfaces. We sort a COPY so callers' input is untouched.
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
      sorted.sort((a, b) => {
        // Compare by Unicode CODE POINTS, not UTF-16 code units: plain JS
        // string comparison orders an astral-plane char (emoji, U+10000+) by
        // its surrogate halves, flipping the order Rust's String::cmp (code
        // points) produces — the two surfaces would disagree for such titles.
        // Spreading iterates code points, matching Rust exactly.
        const ka = [...nameKey(a)]
        const kb = [...nameKey(b)]
        const len = Math.min(ka.length, kb.length)
        for (let i = 0; i < len; i++) {
          const ca = ka[i].codePointAt(0) ?? 0
          const cb = kb[i].codePointAt(0) ?? 0
          if (ca !== cb) return ca - cb
        }
        return ka.length - kb.length
      })
      break
  }
  return sorted.map((s) => s.id)
}
