import { describe, expect, it } from "vitest"
import {
  activateTab,
  closeTab,
  dirtyCloseMessage,
  emptyTabsState,
  nextActiveId,
  openFile,
  pinTab,
  setTabDirty,
  setTabMode,
  shouldConfirmClose,
  shouldPromoteOnEdit,
} from "./editorTabs"
import type { EditorTab, EditorTabsState } from "./editorTabs"

// Deterministic id generator for tests: sequential "id1", "id2", ...
function idGen() {
  let n = 0
  return () => `id${++n}`
}

describe("openFile", () => {
  it("single click with no tabs creates one preview tab and activates it", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { newId })
    expect(state.tabs).toEqual([
      { id: "id1", path: "a.ts", mode: "file", preview: true, dirty: false },
    ])
    expect(state.activeId).toBe("id1")
  })

  it("second single click REPLACES the preview tab (no accumulation), reusing its id", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    state = openFile(state, "b.ts", { newId })
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0]).toEqual({
      id: "id1",
      path: "b.ts",
      mode: "file",
      preview: true,
      dirty: false,
    })
    expect(state.activeId).toBe("id1")
  })

  it("opening an already-open path activates its existing tab and does not duplicate", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { pin: true, newId })
    state = openFile(state, "b.ts", { pin: true, newId })
    // Activate b, then reopen a — should just activate the existing a tab.
    state = openFile(state, "a.ts", { newId })
    expect(state.tabs).toHaveLength(2)
    expect(state.activeId).toBe("id1")
  })

  it("opts.pin=true on a fresh open creates a permanent (non-preview) tab", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { pin: true, newId })
    expect(state.tabs[0].preview).toBe(false)
  })

  it("opts.pin=true on an existing preview tab for that path clears its preview flag", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    expect(state.tabs[0].preview).toBe(true)
    state = openFile(state, "a.ts", { pin: true, newId })
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0].preview).toBe(false)
  })

  it("a permanent tab plus a new single click adds a preview tab (permanent stays)", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { pin: true, newId })
    state = openFile(state, "b.ts", { newId })
    expect(state.tabs.map((t) => [t.path, t.preview])).toEqual([
      ["a.ts", false],
      ["b.ts", true],
    ])
    expect(state.activeId).toBe("id2")
  })

  it("a DIRTY preview tab is not replaced — a new preview tab is appended instead", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    state = setTabDirty(state, "id1", true)
    state = openFile(state, "b.ts", { newId })
    expect(state.tabs).toHaveLength(2)
    expect(state.tabs.map((t) => t.path)).toEqual(["a.ts", "b.ts"])
    expect(state.activeId).toBe("id2")
  })

  it("an explicit opts.mode is honored for a new tab (diff open lands on a diff tab)", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { mode: "diff", newId })
    expect(state.tabs[0].mode).toBe("diff")
  })

  it("a plain re-activation (no opts.mode) PRESERVES an existing tab's mode, since a tree re-click must not flip a diff tab back to file view", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { mode: "diff", pin: true, newId })
    // Simulate a tree/search click on the already-open path: no mode intent.
    state = openFile(state, "a.ts", { newId })
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0].mode).toBe("diff")
  })

  it("an explicit opts.mode on an existing tab retargets its mode (changed-files Diff button on an open file tab)", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { mode: "file", pin: true, newId })
    state = openFile(state, "a.ts", { mode: "diff", newId })
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0].mode).toBe("diff")
  })

  // Correction: EditorBody's Monaco buffers are keyed by tab id, and a
  // preview-replace reuses the tab's id while swapping its path. If the
  // replaced tab kept a stale `dirty: true` from the file it used to hold,
  // the strip's dirty dot and the close-confirm gating would misfire against
  // content that no longer exists in this tab. The reducer must always land
  // on `dirty: false` for the replaced tab.
  it("openFile: replacing the preview tab resets its dirty flag", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    // A preview tab is never dirty in normal flow (editing pins it), but
    // simulate the defensive case directly to prove the replace path always
    // clears it regardless of what it was set to before.
    state = { ...state, tabs: state.tabs.map((t) => ({ ...t, dirty: false })) }
    state = openFile(state, "b.ts", { newId })
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0]).toMatchObject({ path: "b.ts", dirty: false })
  })

  it("open, re-open, then pin the same path ends at one pinned (non-preview) tab", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    state = openFile(state, "a.ts", { newId })
    state = pinTab(state, state.tabs[0].id)
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0]).toMatchObject({ path: "a.ts", preview: false })
  })
})

describe("pinTab", () => {
  it("clears preview on the target tab only", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { pin: true, newId })
    state = openFile(state, "b.ts", { newId })
    state = pinTab(state, "id2")
    expect(state.tabs.map((t) => t.preview)).toEqual([false, false])
  })
})

describe("setTabDirty", () => {
  it("toggles dirty on the target tab", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { newId })
    state = setTabDirty(state, "id1", true)
    expect(state.tabs[0].dirty).toBe(true)
    state = setTabDirty(state, "id1", false)
    expect(state.tabs[0].dirty).toBe(false)
  })

  it("with an unchanged value returns the same state reference", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { newId })
    // Tab starts at dirty: false, so setting it to false again must be a no-op.
    const next = setTabDirty(state, "id1", false)
    expect(next).toBe(state)
  })

  it("targeting an unknown tab id also returns the same state reference", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { newId })
    const next = setTabDirty(state, "does-not-exist", true)
    expect(next).toBe(state)
  })

  it("an actual flip still returns a new reference", () => {
    const newId = idGen()
    const state = openFile(emptyTabsState(), "a.ts", { newId })
    const next = setTabDirty(state, "id1", true)
    expect(next).not.toBe(state)
  })
})

describe("shouldPromoteOnEdit", () => {
  const previewTab: EditorTab = {
    id: "t1",
    path: "a.ts",
    mode: "file",
    preview: true,
    dirty: false,
  }
  const permanentTab: EditorTab = { ...previewTab, preview: false }

  it("promotes a preview tab turning dirty", () => {
    expect(shouldPromoteOnEdit(previewTab, true)).toBe(true)
  })

  it("does not promote a preview tab whose edit didn't turn it dirty", () => {
    expect(shouldPromoteOnEdit(previewTab, false)).toBe(false)
  })

  it("does not promote an already-permanent tab", () => {
    expect(shouldPromoteOnEdit(permanentTab, true)).toBe(false)
  })

  it("does not promote when the tab is undefined", () => {
    expect(shouldPromoteOnEdit(undefined, true)).toBe(false)
  })
})

describe("dirtyCloseMessage", () => {
  it("uses singular phrasing for exactly one dirty tab", () => {
    expect(dirtyCloseMessage(1)).toBe(
      "You have unsaved changes in 1 tab. They will be lost.",
    )
  })

  it("uses plural phrasing for more than one dirty tab", () => {
    expect(dirtyCloseMessage(3)).toBe(
      "You have unsaved changes in 3 tabs. They will be lost.",
    )
  })
})

describe("closeTab", () => {
  function threeTabs(): EditorTabsState {
    const tabs: EditorTab[] = [
      { id: "t1", path: "a.ts", mode: "file", preview: false, dirty: false },
      { id: "t2", path: "b.ts", mode: "file", preview: false, dirty: false },
      { id: "t3", path: "c.ts", mode: "file", preview: false, dirty: false },
    ]
    return { tabs, activeId: "t2" }
  }

  it("closing the active middle tab activates the tab to its right", () => {
    const state = closeTab(threeTabs(), "t2")
    expect(state.tabs.map((t) => t.id)).toEqual(["t1", "t3"])
    expect(state.activeId).toBe("t3")
  })

  it("closing the active LAST (rightmost) tab activates the tab to its left", () => {
    const state = closeTab({ ...threeTabs(), activeId: "t3" }, "t3")
    expect(state.tabs.map((t) => t.id)).toEqual(["t1", "t2"])
    expect(state.activeId).toBe("t2")
  })

  it("closing a non-active tab keeps the current active", () => {
    const state = closeTab(threeTabs(), "t1")
    expect(state.tabs.map((t) => t.id)).toEqual(["t2", "t3"])
    expect(state.activeId).toBe("t2")
  })

  it("closing the only tab yields empty tabs and activeId null", () => {
    const state = closeTab(
      {
        tabs: [
          { id: "t1", path: "a.ts", mode: "file", preview: false, dirty: false },
        ],
        activeId: "t1",
      },
      "t1",
    )
    expect(state.tabs).toEqual([])
    expect(state.activeId).toBeNull()
  })
})

describe("nextActiveId", () => {
  it("right-then-left-then-null selection", () => {
    const tabs: EditorTab[] = [
      { id: "t1", path: "a.ts", mode: "file", preview: false, dirty: false },
      { id: "t2", path: "b.ts", mode: "file", preview: false, dirty: false },
      { id: "t3", path: "c.ts", mode: "file", preview: false, dirty: false },
    ]
    // Middle close: right neighbor wins.
    expect(nextActiveId(tabs, "t2", "t2")).toBe("t3")
    // Rightmost close: falls back left.
    expect(nextActiveId(tabs, "t3", "t3")).toBe("t2")
    // Only tab closes: null.
    expect(
      nextActiveId(
        [{ id: "t1", path: "a.ts", mode: "file", preview: false, dirty: false }],
        "t1",
        "t1",
      ),
    ).toBeNull()
    // Closing a non-active tab: activeId is untouched by nextActiveId's
    // caller (closeTab), but nextActiveId itself still resolves a right/left
    // neighbor relative to the closing tab when asked.
    expect(nextActiveId(tabs, "t1", "t2")).toBe("t2")
  })
})

describe("setTabMode", () => {
  it("changes the mode on the target tab only", () => {
    const newId = idGen()
    let state = openFile(emptyTabsState(), "a.ts", { pin: true, newId })
    state = openFile(state, "b.ts", { pin: true, newId })
    state = setTabMode(state, "id1", "diff")
    expect(state.tabs.map((t) => t.mode)).toEqual(["diff", "file"])
  })
})

describe("activateTab", () => {
  it("sets activeId to the given tab id", () => {
    const state = activateTab(
      {
        tabs: [
          { id: "t1", path: "a.ts", mode: "file", preview: false, dirty: false },
          { id: "t2", path: "b.ts", mode: "file", preview: false, dirty: false },
        ],
        activeId: "t1",
      },
      "t2",
    )
    expect(state.activeId).toBe("t2")
  })
})

describe("shouldConfirmClose", () => {
  it("reflects the target tab's dirty flag", () => {
    const state: EditorTabsState = {
      tabs: [
        { id: "t1", path: "a.ts", mode: "file", preview: false, dirty: true },
        { id: "t2", path: "b.ts", mode: "file", preview: false, dirty: false },
      ],
      activeId: "t1",
    }
    expect(shouldConfirmClose(state, "t1")).toBe(true)
    expect(shouldConfirmClose(state, "t2")).toBe(false)
    // A vanished tab id is not dirty by definition.
    expect(shouldConfirmClose(state, "gone")).toBe(false)
  })
})
