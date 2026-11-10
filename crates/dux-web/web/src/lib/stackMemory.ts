// What this device remembers about each agent's stack: the layout it was last
// left in and the dock terminal last in front. Per device and per agent, in the
// browser's own storage, because it is a view preference rather than a fact
// about the agent. Every access is guarded: private mode or blocked site data
// throws, and losing the memory costs only the convenience.

import type { ComputerLayout } from "./layoutModel"

export interface StackMemory {
  layout: ComputerLayout | null
  terminal: string | null
  /** The provider tab last on top of the agent's stack. */
  tab: string | null
}

const PREFIX = "dux:stack:"
const LAYOUTS: readonly ComputerLayout[] = ["agent", "split", "terminal"]

function key(sessionId: string): string {
  return PREFIX + sessionId
}

export function readStackMemory(sessionId: string): StackMemory {
  let raw: string | null
  try {
    raw = localStorage.getItem(key(sessionId))
  } catch {
    raw = null
  }
  const empty: StackMemory = { layout: null, terminal: null, tab: null }
  if (raw === null) return empty
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== "object" || parsed === null) return empty
    const { layout, terminal, tab } = parsed as Record<string, unknown>
    return {
      layout: LAYOUTS.find((l) => l === layout) ?? null,
      terminal: typeof terminal === "string" ? terminal : null,
      tab: typeof tab === "string" ? tab : null,
    }
  } catch {
    // A half-written or foreign entry is no memory at all.
    return empty
  }
}

export function writeStackMemory(
  sessionId: string,
  patch: Partial<StackMemory>,
): void {
  const next = { ...readStackMemory(sessionId), ...patch }
  try {
    localStorage.setItem(key(sessionId), JSON.stringify(next))
  } catch {
    // The layout still applies; it just will not survive a reload.
  }
}
