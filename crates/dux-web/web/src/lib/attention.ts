// Pure helpers for the "needs attention" chrome, the browser-tab count and the favicon dot.
// The store wires them to the live spine and the favicon module.

import type { SessionView } from "./types"

/**
 * How many agents currently need attention. The server already rolls `needs_attention` up
 * across an agent's tabs, so this is a plain count of flagged sessions.
 */
export function attentionCount(sessions: SessionView[]): number {
  let count = 0
  for (const s of sessions) {
    if (s.needs_attention) count += 1
  }
  return count
}

/**
 * The attention count for the surface the tab renders. The standalone editor tab always
 * reports zero, growing neither the `(N)` title prefix nor the favicon dot: the editor is
 * not the thing needing attention, and the workspace tab is where that signal belongs.
 */
export function attentionCountForSurface(
  sessions: SessionView[],
  standaloneEditor: boolean,
): number {
  return standaloneEditor ? 0 : attentionCount(sessions)
}

/**
 * The browser-tab title: `baseTitle`, the already-resolved instance title, prefixed with the
 * count in parentheses when at least one agent needs attention (`(2) dux`).
 */
export function formatTabTitle(baseTitle: string, count: number): string {
  return count > 0 ? `(${count}) ${baseTitle}` : baseTitle
}
