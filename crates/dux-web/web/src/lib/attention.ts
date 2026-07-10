// Pure helpers for the "needs attention" chrome (browser-tab count + favicon
// dot). Kept free of any DOM or store dependency so they are trivially
// unit-testable; the store wires them to the live spine and the favicon module.

import type { SessionView } from "./types"

/**
 * How many agents currently need attention. Rolled up per agent already (the
 * server sets `SessionView.needs_attention` any-tab), so this is a simple count
 * of flagged sessions.
 */
export function attentionCount(sessions: SessionView[]): number {
  let count = 0
  for (const s of sessions) {
    if (s.needs_attention) count += 1
  }
  return count
}

/**
 * The browser-tab title: the resolved instance title, prefixed with the count in
 * parentheses when at least one agent needs attention (e.g. `(2) dux`), and the
 * bare title when the count is zero. `baseTitle` is the already-resolved
 * instance title (see `resolveInstanceTitle`); this only adds the prefix.
 */
export function formatTabTitle(baseTitle: string, count: number): string {
  return count > 0 ? `(${count}) ${baseTitle}` : baseTitle
}
