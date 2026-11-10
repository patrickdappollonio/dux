import { describe, expect, it } from "vitest"

import type { SessionView, TerminalView } from "@/lib/types"

import { agentStackModel } from "./agentStackModel"

function session(over: Partial<SessionView> = {}): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    status: "active",
    last_focused_tab: null,
    tabs: [
      { id: "s1", provider: "claude", order: 0, has_live_process: true },
      { id: "b2", provider: "codex", order: 1, has_live_process: true },
    ],
    ...over,
  } as unknown as SessionView
}

function terminal(id: string, sortOrder: number): TerminalView {
  return {
    id,
    owner: { kind: "session", session_id: "s1" },
    label: id,
    has_output: true,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: sortOrder,
    created_at: "",
    updated_at: "",
  } as TerminalView
}

type Ctx = Parameters<typeof agentStackModel>[0]
function model(over: Partial<Ctx> = {}) {
  return agentStackModel({
    target: { kind: "agent", sessionId: "s1", tabId: "s1" },
    session: session(),
    terminals: [terminal("t1", 1)],
    stackLayouts: {},
    stackTerminals: {},
    stackTabs: {},
    memory: { layout: null, terminal: null, tab: null },
    startedDormantTabs: [],
    pendingSlotTab: {},
    endedInViewTab: null,
    pendingTerminalOrder: null,
    alwaysShowTabStrip: false,
    ...over,
  })
}
const dockTarget = {
  kind: "terminal" as const,
  terminalId: "t1",
  owner: { kind: "session" as const, sessionId: "s1" },
}

describe("agentStackModel", () => {
  it("lists the dock in its order, a new terminal joining at the end", () => {
    const m = model({ terminals: [terminal("t3", 7), terminal("t1", 1), terminal("t2", 4)] })
    expect(m.dock.map((t) => t.id)).toEqual(["t1", "t2", "t3"])
  })

  it("sends typing to a part on screen when the chosen one is folded away", () => {
    expect(model({ stackLayouts: { s1: "terminal" } }).typingPtyId).toBe("t1")
    expect(
      model({ target: dockTarget, stackLayouts: { s1: "agent" } }).typingPtyId,
    ).toBe("s1")
    expect(model({ stackLayouts: { s1: "split" } }).typingPtyId).toBe("s1")
  })

  it("sends typing to the dock of a stopped agent opened from its row", () => {
    const stopped = session({
      status: "detached",
      tabs: [{ id: "s1", provider: "claude", order: 0, has_live_process: false }],
    } as Partial<SessionView>)
    const m = model({ session: stopped })
    expect(m.parts.agentLiveSocket).toBe(false)
    expect(m.typingPtyId).toBe("t1")
    // Beside the dock the stopped agent's own first tab gets the short card.
    expect(m.agentCard).toBe("short")
  })

  it("names no part when nothing typeable is on screen, leaving the chosen one", () => {
    const withDormantExtra = session({
      tabs: [
        { id: "s1", provider: "claude", order: 0, has_live_process: true },
        { id: "b2", provider: "codex", order: 1, has_live_process: false },
      ],
    } as Partial<SessionView>)
    const m = model({
      session: withDormantExtra,
      target: { kind: "agent", sessionId: "s1", tabId: "b2" },
      stackLayouts: { s1: "agent" },
    })
    expect(m.typingPtyId).toBeNull()
    expect(m.chosenPtyId).toBe("b2")
  })

  it("keeps the tab this device showed on top after a reload", () => {
    expect(
      model({ target: dockTarget, memory: { layout: null, terminal: null, tab: "b2" } }).tabId,
    ).toBe("b2")
  })

  it("keeps the tab this page showed on top while typing goes to the dock, never the shared one", () => {
    const shared = session({ last_focused_tab: "b2" } as Partial<SessionView>)
    expect(model({ target: dockTarget, session: shared }).tabId).toBe("s1")
    expect(
      model({ target: dockTarget, session: shared, stackTabs: { s1: "b2" } }).tabId,
    ).toBe("b2")
    // A remembered tab that has since closed is not shown.
    expect(
      model({ target: dockTarget, stackTabs: { s1: "gone" } }).tabId,
    ).toBe("s1")
  })

  it("never starts an agent stopped beside its terminals once the last one ends", () => {
    const stopped = session({
      status: "detached",
      tabs: [{ id: "s1", provider: "claude", order: 0, has_live_process: false }],
    } as Partial<SessionView>)
    const m = model({ session: stopped, terminals: [], endedInViewTab: "s1" })
    expect(m.parts.agentLiveSocket).toBe(false)
    expect(m.agentCard).toBe("full")
  })

  it("gives a running agent's dormant extra tab its own card, not the short one", () => {
    const withDormantExtra = session({
      tabs: [
        { id: "s1", provider: "claude", order: 0, has_live_process: true },
        { id: "b2", provider: "codex", order: 1, has_live_process: false },
      ],
    } as Partial<SessionView>)
    const m = model({
      session: withDormantExtra,
      target: { kind: "agent", sessionId: "s1", tabId: "b2" },
    })
    expect(m.parts.agentLiveSocket).toBe(false)
    expect(m.agentCard).toBe("full")
  })
})
