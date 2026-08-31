import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  THEATER_QUERY,
  THEATER_STORAGE_PREFIX,
  clearTheaterMemory,
  theaterEscapeAction,
  theaterMemoryKeyForPty,
  theaterOwnershipStep,
  theaterOwnershipWatchStart,
  peekTheaterTabs,
  registerTheaterTabs,
  isTypingSurfaceElement,
  readTheaterMemory,
  splitTheaterHash,
  theaterMemoryKey,
  theaterPillModel,
  theaterSerializable,
  withTheaterHash,
  writeTheaterMemory,
} from "./theater"
import type { AgentTabView } from "./types"

function installStorage(): Map<string, string> {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  return mem
}

const agent = { kind: "agent", sessionId: "s1", tabId: "t7" } as const
const other = { kind: "agent", sessionId: "s1", tabId: "t9" } as const
const terminal = {
  kind: "terminal",
  terminalId: "tm3",
  owner: { kind: "session", sessionId: "s1" },
} as const

function tab(over: Partial<AgentTabView> & { id: string }): AgentTabView {
  return {
    provider: "claude",
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
    ...over,
  } as AgentTabView
}

describe("theaterMemoryKey", () => {
  it("keys an agent pane by its stable tab id, not its session", () => {
    expect(theaterMemoryKey(agent)).toBe(`${THEATER_STORAGE_PREFIX}agent:t7`)
    expect(theaterMemoryKey(other)).toBe(`${THEATER_STORAGE_PREFIX}agent:t9`)
  })

  it("keys a terminal pane by its terminal id, whatever the owner", () => {
    expect(theaterMemoryKey(terminal)).toBe(
      `${THEATER_STORAGE_PREFIX}terminal:tm3`,
    )
    expect(
      theaterMemoryKey({
        kind: "terminal",
        terminalId: "tm3",
        owner: { kind: "standalone" },
      }),
    ).toBe(`${THEATER_STORAGE_PREFIX}terminal:tm3`)
  })

  it("has no key for no target", () => {
    expect(theaterMemoryKey(null)).toBeNull()
  })

  it("builds the same key from a mounted pane's own kind and pty id", () => {
    expect(theaterMemoryKeyForPty("agent", "t7")).toBe(theaterMemoryKey(agent))
    expect(theaterMemoryKeyForPty("terminal", "tm3")).toBe(
      theaterMemoryKey(terminal),
    )
  })
})

describe("theater memory", () => {
  beforeEach(() => {
    installStorage()
    clearTheaterMemory(theaterMemoryKey(agent))
    clearTheaterMemory(theaterMemoryKey(other))
    clearTheaterMemory(theaterMemoryKey(terminal))
  })

  it("reads false for a pane nobody has put in theater", () => {
    expect(readTheaterMemory(theaterMemoryKey(agent))).toBe(false)
  })

  it("round-trips a written choice", () => {
    writeTheaterMemory(theaterMemoryKey(agent), true)
    expect(readTheaterMemory(theaterMemoryKey(agent))).toBe(true)
    writeTheaterMemory(theaterMemoryKey(agent), false)
    expect(readTheaterMemory(theaterMemoryKey(agent))).toBe(false)
  })

  it("is per tab: one tab's memory says nothing about its sibling", () => {
    writeTheaterMemory(theaterMemoryKey(agent), true)
    expect(readTheaterMemory(theaterMemoryKey(other))).toBe(false)
  })

  it("clears a pane's memory outright", () => {
    writeTheaterMemory(theaterMemoryKey(terminal), true)
    clearTheaterMemory(theaterMemoryKey(terminal))
    expect(readTheaterMemory(theaterMemoryKey(terminal))).toBe(false)
  })

  it("degrades to off when storage throws", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied")
      },
      setItem: () => {
        throw new Error("denied")
      },
      removeItem: () => {
        throw new Error("denied")
      },
    })
    expect(() => writeTheaterMemory(theaterMemoryKey(agent), true)).not.toThrow()
    expect(readTheaterMemory(theaterMemoryKey(agent))).toBe(false)
    expect(() => clearTheaterMemory(theaterMemoryKey(agent))).not.toThrow()
  })

  it("writes and reads nothing at all for no target", () => {
    const mem = installStorage()
    writeTheaterMemory(theaterMemoryKey(null), true)
    expect(mem.size).toBe(0)
    expect(readTheaterMemory(theaterMemoryKey(null))).toBe(false)
  })
})

describe("the theater hash modifier", () => {
  it("appends the modifier to a position", () => {
    expect(withTheaterHash("#/agent/s1", true)).toBe(`#/agent/s1${THEATER_QUERY}`)
    expect(withTheaterHash("#/agent/s1/tab/t7", true)).toBe(
      `#/agent/s1/tab/t7${THEATER_QUERY}`,
    )
  })

  it("leaves a position alone when theater is off", () => {
    expect(withTheaterHash("#/agent/s1", false)).toBe("#/agent/s1")
  })

  it("never modifies the empty (home) address", () => {
    expect(withTheaterHash("", true)).toBe("")
  })

  it("splits the modifier back off, round-tripping the position", () => {
    expect(splitTheaterHash(`#/agent/s1/tab/t7${THEATER_QUERY}`)).toEqual({
      hash: "#/agent/s1/tab/t7",
      theater: true,
    })
    expect(splitTheaterHash("#/agent/s1")).toEqual({
      hash: "#/agent/s1",
      theater: false,
    })
  })

  it("leaves an unrecognized query alone rather than swallowing it", () => {
    expect(splitTheaterHash("#/agent/s1?view=nonsense")).toEqual({
      hash: "#/agent/s1?view=nonsense",
      theater: false,
    })
  })

  it("is a modifier on a position, never on the editor or the changes screen", () => {
    const target = agent
    expect(
      theaterSerializable({ target, changes: false, editor: null, standalone: false }),
    ).toBe(true)
    expect(
      theaterSerializable({ target, changes: true, editor: null, standalone: false }),
    ).toBe(false)
    expect(
      theaterSerializable({
        target,
        changes: false,
        editor: { mode: "file", path: null },
        standalone: false,
      }),
    ).toBe(false)
    expect(
      theaterSerializable({ target: null, changes: false, editor: null, standalone: false }),
    ).toBe(false)
  })
})

describe("theaterEscapeAction", () => {
  const base = {
    type: "keydown",
    key: "Escape",
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 27,
    inTypingSurface: false,
    defaultPrevented: false,
    tabsExpanded: false,
    theater: true,
  }

  it("exits on a bare Escape outside every typing surface", () => {
    expect(theaterEscapeAction(base)).toBe("exit")
  })

  it("leaves Escape to the PTY while a typing surface has focus", () => {
    expect(theaterEscapeAction({ ...base, inTypingSurface: true })).toBe("none")
  })

  it("does nothing when theater is not on", () => {
    expect(theaterEscapeAction({ ...base, theater: false })).toBe("none")
  })

  it("ignores every other key", () => {
    expect(theaterEscapeAction({ ...base, key: "Enter" })).toBe("none")
  })

  it("ignores a modified Escape, which is somebody else's chord", () => {
    expect(theaterEscapeAction({ ...base, ctrlKey: true })).toBe("none")
    expect(theaterEscapeAction({ ...base, shiftKey: true })).toBe("none")
    expect(theaterEscapeAction({ ...base, altKey: true })).toBe("none")
    expect(theaterEscapeAction({ ...base, metaKey: true })).toBe("none")
  })

  it("never fires mid-composition, where Escape cancels the composition", () => {
    expect(theaterEscapeAction({ ...base, isComposing: true })).toBe("none")
    expect(theaterEscapeAction({ ...base, keyCode: 229 })).toBe("none")
  })

  it("only matches keydown, so keyup cannot double-fire", () => {
    expect(theaterEscapeAction({ ...base, type: "keyup" })).toBe("none")
  })

  it("abstains once an overlay has already answered the Escape", () => {
    // Base UI's dismiss hook calls preventDefault on the Escape that closed a
    // menu, a popover or a dialog, and it listens on the document like this
    // rule does. Without this the one keystroke would close the menu AND leave
    // theater, which is two acts for one press.
    expect(theaterEscapeAction({ ...base, defaultPrevented: true })).toBe("none")
  })

  it("collapses the pill's tab strip first, and leaves theater on the next press", () => {
    expect(theaterEscapeAction({ ...base, tabsExpanded: true })).toBe(
      "collapse-tabs",
    )
    expect(theaterEscapeAction(base)).toBe("exit")
  })
})

describe("theaterOwnershipStep", () => {
  it("says nothing about the foreground guess a mount starts from", () => {
    // `isOwner` is optimistic before the handshake answers, so the very first
    // honest verdict is not a transition. A watcher opening a shared theater
    // link used to enter, exit and clear its memory in one breath.
    const first = theaterOwnershipStep(theaterOwnershipWatchStart, {
      handshakeSeen: false,
      isOwner: true,
    })
    expect(first.lost).toBe(false)
    const verdict = theaterOwnershipStep(first.state, {
      handshakeSeen: true,
      isOwner: false,
    })
    expect(verdict.lost).toBe(false)
  })

  it("reports a real demotion once a verdict has landed", () => {
    const owner = theaterOwnershipStep(theaterOwnershipWatchStart, {
      handshakeSeen: true,
      isOwner: true,
    })
    expect(owner.lost).toBe(false)
    const lost = theaterOwnershipStep(owner.state, {
      handshakeSeen: true,
      isOwner: false,
    })
    expect(lost.lost).toBe(true)
  })

  it("reports the spine correction's demotion too, and only once", () => {
    // The refetched spine naming another connection is the other way a pane
    // loses the pty, and it lands as the same verdict flip.
    let step = theaterOwnershipStep(theaterOwnershipWatchStart, {
      handshakeSeen: true,
      isOwner: true,
    })
    step = theaterOwnershipStep(step.state, {
      handshakeSeen: true,
      isOwner: false,
    })
    expect(step.lost).toBe(true)
    step = theaterOwnershipStep(step.state, {
      handshakeSeen: true,
      isOwner: false,
    })
    expect(step.lost).toBe(false)
  })

  it("arms again after a take-over, so the next loss still counts", () => {
    let step = theaterOwnershipStep(theaterOwnershipWatchStart, {
      handshakeSeen: true,
      isOwner: false,
    })
    step = theaterOwnershipStep(step.state, { handshakeSeen: true, isOwner: true })
    expect(step.lost).toBe(false)
    step = theaterOwnershipStep(step.state, { handshakeSeen: true, isOwner: false })
    expect(step.lost).toBe(true)
  })
})

describe("the pill's tab-strip registry", () => {
  it("hands the page-wide Escape rule the strip it cannot see", () => {
    // The strip's expanded state is the pill's own, and the Escape listener
    // lives above both shells, so the two meet through a registration rather
    // than a prop chain crossing every layout.
    expect(peekTheaterTabs()).toBeNull()
    const handle = { expanded: () => true, collapse: vi.fn() }
    const off = registerTheaterTabs(handle)
    expect(peekTheaterTabs()?.expanded()).toBe(true)
    peekTheaterTabs()?.collapse()
    expect(handle.collapse).toHaveBeenCalledTimes(1)
    off()
    expect(peekTheaterTabs()).toBeNull()
  })

  it("retires only its own registration, the way every other one does", () => {
    const first = { expanded: () => false, collapse: vi.fn() }
    const second = { expanded: () => true, collapse: vi.fn() }
    const offFirst = registerTheaterTabs(first)
    const offSecond = registerTheaterTabs(second)
    offFirst()
    expect(peekTheaterTabs()).toBe(second)
    offSecond()
    expect(peekTheaterTabs()).toBeNull()
  })
})

describe("isTypingSurfaceElement", () => {
  it("recognizes the surfaces a keystroke can be typed into", () => {
    for (const tag of ["input", "textarea", "select"]) {
      expect(isTypingSurfaceElement({ tagName: tag.toUpperCase() })).toBe(true)
    }
  })

  it("recognizes a contenteditable host", () => {
    expect(
      isTypingSurfaceElement({ tagName: "DIV", isContentEditable: true }),
    ).toBe(true)
  })

  it("says no for an ordinary element and for nothing at all", () => {
    expect(isTypingSurfaceElement({ tagName: "DIV" })).toBe(false)
    expect(isTypingSurfaceElement(null)).toBe(false)
  })
})

describe("theaterPillModel", () => {
  it("collapses to macros and exit for a terminal, which has no tabs", () => {
    const model = theaterPillModel(undefined, null)
    expect(model.expandable).toBe(false)
    expect(model.tabs).toEqual([])
    expect(model.working).toBe(false)
    expect(model.attention).toBe(false)
  })

  it("collapses for a single-tab agent, so the expander is never empty", () => {
    const model = theaterPillModel([tab({ id: "t1" })], "t1")
    expect(model.expandable).toBe(false)
  })

  it("expands for a multi-tab agent and carries every tab for switching", () => {
    const tabs = [tab({ id: "t1" }), tab({ id: "t2" }), tab({ id: "t3" })]
    const model = theaterPillModel(tabs, "t1")
    expect(model.expandable).toBe(true)
    expect(model.tabs.map((t) => t.id)).toEqual(["t1", "t2", "t3"])
  })

  it("reports the HIDDEN tabs' state, never the one already on screen", () => {
    const tabs = [
      tab({ id: "t1", working: true, needs_attention: true }),
      tab({ id: "t2" }),
    ]
    const model = theaterPillModel(tabs, "t1")
    expect(model.working).toBe(false)
    expect(model.attention).toBe(false)
  })

  it("rolls a background tab's working and attention up onto the pill", () => {
    const tabs = [
      tab({ id: "t1" }),
      tab({ id: "t2", working: true }),
      tab({ id: "t3", needs_attention: true }),
    ]
    const model = theaterPillModel(tabs, "t1")
    expect(model.working).toBe(true)
    expect(model.attention).toBe(true)
  })
})
