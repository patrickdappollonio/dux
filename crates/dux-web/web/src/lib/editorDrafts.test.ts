import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { TabBuffer } from "./editorBuffers"
import { emptyBuffer, fileLoadSeedBuffer } from "./editorBuffers"
import {
  clearRootDrafts,
  loadRootDrafts,
  pruneRootDrafts,
  storeRootDrafts,
  syncBeforeUnloadGuard,
} from "./editorDrafts"

// The draft cache is what lets an unsaved edit survive the editor being
// closed: `EditorBody` seeds its buffer state from here on mount and writes
// every buffer change back, so closing the overlay (which unmounts the body)
// no longer destroys the drafts. It is a module-level cache on purpose, NOT
// store state: file contents in the global store would fan a re-render out to
// every consumer on each keystroke (see lib/editorTabs.ts header comment).

function buf(path: string, draft: string): TabBuffer {
  return { ...emptyBuffer(path), loadedPath: path, loaded: draft, draft }
}

describe("the per-session draft cache", () => {
  afterEach(() => {
    clearRootDrafts("s1")
    clearRootDrafts("s2")
  })

  it("round-trips buffers per session", () => {
    storeRootDrafts("s1", new Map([["t1", buf("a.txt", "hello")]]))
    storeRootDrafts("s2", new Map([["t9", buf("b.txt", "other")]]))
    const restored = loadRootDrafts("s1")
    expect(restored.get("t1")?.draft).toBe("hello")
    expect(restored.has("t9")).toBe(false)
  })

  it("hands out copies, so mutating the loaded map cannot corrupt the cache", () => {
    storeRootDrafts("s1", new Map([["t1", buf("a.txt", "hello")]]))
    const first = loadRootDrafts("s1")
    first.delete("t1")
    expect(loadRootDrafts("s1").get("t1")?.draft).toBe("hello")
  })

  it("drops a buffer whose read was still in flight when it was cached", () => {
    // A buffer cached with `loading: true` holds no content and its fetch
    // resolver died with the component: restoring it would make
    // `shouldSkipFileLoad` skip the re-read and park the tab on a spinner
    // forever. Loading such an entry must yield nothing so the remount
    // re-fetches from scratch.
    storeRootDrafts(
      "s1",
      new Map([
        ["t1", fileLoadSeedBuffer("a.txt")],
        ["t2", buf("b.txt", "kept")],
      ]),
    )
    const restored = loadRootDrafts("s1")
    expect(restored.has("t1")).toBe(false)
    expect(restored.get("t2")?.draft).toBe("kept")
  })

  it("prunes entries for tabs that no longer exist", () => {
    storeRootDrafts(
      "s1",
      new Map([
        ["t1", buf("a.txt", "one")],
        ["t2", buf("b.txt", "two")],
      ]),
    )
    pruneRootDrafts("s1", new Set(["t2"]))
    const restored = loadRootDrafts("s1")
    expect(restored.has("t1")).toBe(false)
    expect(restored.get("t2")?.draft).toBe("two")
  })

  it("clears a whole session (the session-delete path)", () => {
    storeRootDrafts("s1", new Map([["t1", buf("a.txt", "one")]]))
    clearRootDrafts("s1")
    expect(loadRootDrafts("s1").size).toBe(0)
  })
})

describe("the beforeunload guard", () => {
  let added: [string, unknown][]
  let removed: [string, unknown][]

  beforeEach(() => {
    added = []
    removed = []
    vi.stubGlobal("window", {
      addEventListener: (type: string, handler: unknown) => {
        added.push([type, handler])
      },
      removeEventListener: (type: string, handler: unknown) => {
        removed.push([type, handler])
      },
    })
  })

  afterEach(() => {
    // Leave the module-level armed flag down for the next test file.
    syncBeforeUnloadGuard(false)
    vi.unstubAllGlobals()
  })

  it("arms once while dirty, and re-syncing does not stack handlers", () => {
    syncBeforeUnloadGuard(true)
    syncBeforeUnloadGuard(true)
    expect(added.filter(([t]) => t === "beforeunload")).toHaveLength(1)
    expect(removed).toHaveLength(0)
  })

  it("disarms with the same handler it armed with", () => {
    syncBeforeUnloadGuard(true)
    syncBeforeUnloadGuard(false)
    syncBeforeUnloadGuard(false)
    expect(added).toHaveLength(1)
    expect(removed).toHaveLength(1)
    expect(removed[0][1]).toBe(added[0][1])
  })

  it("the handler asks the browser for the leave prompt", () => {
    syncBeforeUnloadGuard(true)
    const handler = added[0][1] as (e: {
      preventDefault: () => void
      returnValue?: string
    }) => void
    const preventDefault = vi.fn()
    const event: { preventDefault: () => void; returnValue?: string } = {
      preventDefault,
    }
    handler(event)
    expect(preventDefault).toHaveBeenCalled()
    // Legacy channel some browsers still require alongside preventDefault.
    expect(event.returnValue).toBe("")
  })

  it("does nothing on a window that cannot unregister", () => {
    // A handler that could be added but never removed would prompt forever,
    // so a window missing either half of the API gets no handler at all
    // (this is also what keeps the store's routing tests, whose window stub
    // only has addEventListener, out of the guard's way).
    vi.stubGlobal("window", {
      addEventListener: (type: string, handler: unknown) => {
        added.push([type, handler])
      },
    })
    syncBeforeUnloadGuard(true)
    expect(added).toHaveLength(0)
  })
})
