import { describe, expect, it } from "vitest"

import {
  applyLayoutAction,
  foldForDivider,
  layoutOnStart,
  partsOnScreen,
  resolvePhoneView,
  resolveStackOpening,
  shouldPersistDividerRatio,
  shouldShowStackStrips,
  type StackOpeningInputs,
} from "./layoutModel"
import type { AgentTabView } from "./types"

function tab(id: string): AgentTabView {
  return {
    id,
    provider: "claude",
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
  }
}

describe("applyLayoutAction", () => {
  it.each([
    ["split", "hideTerminals", "agent"],
    ["split", "maximizeTerminal", "terminal"],
    ["split", "restore", "split"],
    ["agent", "restore", "split"],
    ["agent", "hideTerminals", "agent"],
    ["agent", "maximizeTerminal", "agent"],
    ["terminal", "restore", "split"],
    ["terminal", "maximizeTerminal", "terminal"],
    ["terminal", "hideTerminals", "terminal"],
  ] as const)("%s + %s -> %s", (layout, action, expected) => {
    expect(applyLayoutAction(layout, action)).toBe(expected)
  })
})

describe("layoutOnStart", () => {
  it("returns the remembered layout, else the split", () => {
    expect(layoutOnStart("terminal")).toBe("terminal")
    expect(layoutOnStart(undefined)).toBe("split")
  })
})

describe("foldForDivider", () => {
  const mins = { agentMin: 20, dockMin: 15 }
  it.each([
    [50, 50, null],
    [86, 14, "agent"],
    [85, 15, null],
    [19, 81, "terminal"],
    [20, 80, null],
  ] as const)("agent %i / dock %i -> %s", (agentShare, dockShare, expected) => {
    expect(foldForDivider({ agentShare, dockShare, ...mins })).toBe(expected)
  })

  it("keeps the agent when both parts are below their minimum", () => {
    expect(
      foldForDivider({ agentShare: 10, dockShare: 10, agentMin: 20, dockMin: 20 }),
    ).toBe("agent")
  })
})

describe("shouldPersistDividerRatio", () => {
  it("persists only a release that did not fold", () => {
    expect(shouldPersistDividerRatio(null)).toBe(true)
    expect(shouldPersistDividerRatio("agent")).toBe(false)
    expect(shouldPersistDividerRatio("terminal")).toBe(false)
  })
})

describe("shouldShowStackStrips", () => {
  // The same cases the terminal UI's `agent_tabs::strips_shown` is held to.
  it.each([
    [1, 0, false, false],
    [1, 1, false, false],
    [2, 0, false, true],
    [1, 2, false, true],
    [1, 0, true, true],
    [1, 1, true, true],
  ])(
    "%i tabs, %i terminals, always %s -> %s",
    (tabCount, terminals, always, expected) => {
      const tabs = Array.from({ length: tabCount }, (_, i) => tab(`t${i}`))
      expect(shouldShowStackStrips(tabs, terminals, always)).toBe(expected)
    },
  )
})

describe("resolvePhoneView", () => {
  const base = {
    terminalCount: 1,
    addressNamesTerminal: false,
    agentStopped: false,
    anyTerminalRunning: false,
    remembered: null,
  }

  it("is the agent when there are no terminals and the address names none", () => {
    expect(
      resolvePhoneView({
        ...base,
        terminalCount: 0,
        agentStopped: true,
        anyTerminalRunning: true,
        remembered: "terminals",
      }),
    ).toBe("agent")
  })

  it("opens the terminals for an address naming a terminal that is gone, so its not-found screen is what shows", () => {
    expect(
      resolvePhoneView({ ...base, terminalCount: 0, addressNamesTerminal: true }),
    ).toBe("terminals")
  })

  it.each([
    [{ addressNamesTerminal: true, remembered: "agent" }, "terminals"],
    [
      { agentStopped: true, anyTerminalRunning: true, remembered: "agent" },
      "terminals",
    ],
    [{ agentStopped: true, remembered: "agent" }, "agent"],
    [{ remembered: "terminals" }, "terminals"],
    [{}, "agent"],
  ] as const)("%o -> %s", (overrides, expected) => {
    expect(resolvePhoneView({ ...base, ...overrides })).toBe(expected)
  })
})

describe("partsOnScreen", () => {
  it.each([
    ["split", { agent: true, dock: true }],
    ["agent", { agent: true, dock: false }],
    ["terminal", { agent: false, dock: true }],
  ] as const)("computer %s", (layout, expected) => {
    expect(
      partsOnScreen({ surface: "computer", layout, agentTab: "live" }),
    ).toMatchObject(expected)
  })

  it("shows the one part a phone's view names", () => {
    expect(
      partsOnScreen({ surface: "phone", view: "terminals", agentTab: "live" }),
    ).toMatchObject({ agent: false, dock: true })
    expect(
      partsOnScreen({ surface: "phone", view: "agent", agentTab: "live" }),
    ).toMatchObject({ agent: true, dock: false })
  })

  it("puts the dock on screen wherever an address names a gone terminal, on both form factors", () => {
    const opening = resolveStackOpening({
      addressTerminal: "gone",
      rememberedTerminal: null,
      rememberedLayout: "agent",
      dockTerminals: [],
      agentStopped: false,
      anyTerminalRunning: false,
    })
    expect(opening.terminalNotFound).toBe(true)
    expect(
      partsOnScreen({ surface: "computer", layout: opening.layout, agentTab: "live" }).dock,
    ).toBe(true)
    const view = resolvePhoneView({
      terminalCount: 0,
      addressNamesTerminal: true,
      agentStopped: false,
      anyTerminalRunning: false,
      remembered: "agent",
    })
    expect(partsOnScreen({ surface: "phone", view, agentTab: "live" }).dock).toBe(true)
  })

  // Subscribing to a dormant tab's pty launches it, whether or not the pane is
  // on screen, so a stopped agent beside a running terminal must not get a
  // live socket just because its part is mounted off screen.
  it.each([
    ["live", "agent", true],
    ["live", "terminal", true],
    ["startsOnSight", "agent", true],
    ["startsOnSight", "terminal", false],
    ["dormant", "agent", false],
    ["dormant", "split", false],
  ] as const)(
    "gives the agent part a live socket for a %s tab in the %s layout: %s",
    (agentTab, layout, expected) => {
      expect(
        partsOnScreen({ surface: "computer", layout, agentTab }).agentLiveSocket,
      ).toBe(expected)
    },
  )
})

describe("resolveStackOpening", () => {
  const base: StackOpeningInputs = {
    addressTerminal: null,
    rememberedTerminal: null,
    rememberedLayout: null,
    dockTerminals: ["t1", "t2", "t3"],
    agentStopped: false,
    anyTerminalRunning: false,
  }

  it("lets an address naming a live terminal win and overwrite memory", () => {
    expect(
      resolveStackOpening({
        ...base,
        addressTerminal: "t3",
        rememberedTerminal: "t2",
      }),
    ).toEqual({
      activeTerminal: "t3",
      terminalNotFound: false,
      layout: "split",
      rememberTerminal: "t3",
      rewriteAddress: false,
    })
  })

  it("keeps the remembered layout when the address names the terminal it last showed", () => {
    // A reload: the address was written for this device's own dock, so it is
    // not a request to open the terminal, and the stack opens as it was left.
    const opening = resolveStackOpening({
      ...base,
      addressTerminal: "t2",
      rememberedTerminal: "t2",
      rememberedLayout: "agent",
    })
    expect(opening.activeTerminal).toBe("t2")
    expect(opening.layout).toBe("agent")
  })

  it("reports an address naming a gone terminal without picking another", () => {
    const opening = resolveStackOpening({
      ...base,
      addressTerminal: "gone",
      rememberedTerminal: "t2",
    })
    expect(opening.activeTerminal).toBeNull()
    expect(opening.terminalNotFound).toBe(true)
    expect(opening.rewriteAddress).toBe(false)
  })

  it("opens a bare address on a live remembered terminal and rewrites it", () => {
    const opening = resolveStackOpening({ ...base, rememberedTerminal: "t2" })
    expect(opening.activeTerminal).toBe("t2")
    expect(opening.rewriteAddress).toBe(true)
  })

  it("drops a gone remembered terminal quietly and falls back to the first pill", () => {
    expect(
      resolveStackOpening({ ...base, rememberedTerminal: "gone" }),
    ).toEqual({
      activeTerminal: "t1",
      terminalNotFound: false,
      layout: "split",
      rememberTerminal: null,
      rewriteAddress: false,
    })
  })

  it("opens the agent alone when there are no terminals", () => {
    const opening = resolveStackOpening({
      ...base,
      dockTerminals: [],
      rememberedTerminal: "gone",
      rememberedLayout: "terminal",
    })
    expect(opening.activeTerminal).toBeNull()
    expect(opening.terminalNotFound).toBe(false)
    expect(opening.layout).toBe("agent")
  })

  it.each([
    [{ rememberedLayout: "agent" }, "agent"],
    [{ rememberedLayout: "terminal" }, "terminal"],
    [{ addressTerminal: "t1", rememberedLayout: "agent" }, "split"],
    [{ addressTerminal: "t1", rememberedLayout: "terminal" }, "terminal"],
    [
      { agentStopped: true, anyTerminalRunning: true, rememberedLayout: "agent" },
      "terminal",
    ],
    [{ agentStopped: true, rememberedLayout: "agent" }, "agent"],
  ] as const)("layout %o -> %s", (overrides, expected) => {
    expect(resolveStackOpening({ ...base, ...overrides }).layout).toBe(expected)
  })
})
