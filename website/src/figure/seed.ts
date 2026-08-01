import { seedStaticSnapshot } from "@/lib/store"

import {
  bootstrap,
  focusedSessionId,
  spine,
  stagedFiles,
  unstagedFiles,
} from "./workspace"

// Put the fabricated workspace into the REAL store, so the real components read
// it through the real `useDux()` and have no idea they are being rendered by a
// marketing site. Nothing is mocked and no prop is threaded: the store is where
// the app's state lives, so the store is what gets seeded.
//
// This runs once, at build time, before `renderToStaticMarkup`. In the browser
// the store boots off the server instead and this module is never shipped.
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
