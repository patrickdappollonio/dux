import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { syncBeforeUnloadGuard } from "./editorDrafts"
import { reloadPage } from "./reloadPage"

// The server-restart reload is silent by tenet: no prompt, no toast, no
// banner. The beforeunload guard (armed while an editor tab is dirty) would
// turn it into a browser prompt, so `reloadPage()` must disarm the guard
// BEFORE calling reload. Drafts are lost on a restart reload, deliberately:
// they live in page memory, and a stale page must never keep running.

describe("reloadPage", () => {
  const calls: string[] = []

  beforeEach(() => {
    calls.length = 0
    vi.stubGlobal("window", {
      addEventListener: () => {
        calls.push("add")
      },
      removeEventListener: () => {
        calls.push("remove")
      },
      location: {
        reload: () => {
          calls.push("reload")
        },
      },
    })
  })

  afterEach(() => {
    syncBeforeUnloadGuard(false)
    vi.unstubAllGlobals()
  })

  it("disarms the beforeunload guard before reloading", () => {
    syncBeforeUnloadGuard(true)
    expect(calls).toEqual(["add"])
    reloadPage()
    expect(calls).toEqual(["add", "remove", "reload"])
  })

  it("reloads cleanly when the guard was never armed", () => {
    reloadPage()
    expect(calls).toEqual(["reload"])
  })
})
