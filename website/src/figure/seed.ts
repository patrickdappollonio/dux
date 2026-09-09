import { seedStaticSnapshot } from "@/lib/store"

import {
  bootstrap,
  focusedSessionId,
  spine,
  stagedFiles,
  unstagedFiles,
} from "./workspace"

// Puts the fabricated workspace into the REAL store, so the components read it
// through the real `useDux()` with nothing mocked and no prop threaded. Runs once
// at build time, before `renderToStaticMarkup`; never shipped to a browser.
let seeded = false

export function seedFigureWorkspace(): void {
  if (seeded) return
  seeded = true
  seedStaticSnapshot({
    booted: true,
    conn: "open",
    spine,
    bootstrap,
    selectedTarget: { kind: "agent", sessionId: focusedSessionId, tabId: focusedSessionId },
    selectedSessionId: focusedSessionId,
    changes: {
      sessionId: focusedSessionId,
      phase: "loaded",
      rev: 1,
      staged: stagedFiles,
      unstaged: unstagedFiles,
      error: null,
    },
  })
}
