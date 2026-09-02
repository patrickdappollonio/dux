// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, screen } from "@testing-library/react"

import type * as React from "react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

// THE PHONE'S DETACH FLIGHT, as a sequence of states rather than as pixels.
//
// jsdom paints nothing, so what is pinned here is the choreography's SHAPE: at
// every moment, which cluster exists, which one is painted, and which stage the
// travelling one believes it is in. The stages themselves (the transform, the
// radius morph, the shadow fade) are the pure helpers' business and are pinned
// in `theaterFlight.test.ts`.

let mockState: DuxState
let reducedMotion = false
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    toggleTheater: vi.fn(),
    navigateUp: vi.fn(),
  }
})

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: query.includes("prefers-reduced-motion") ? reducedMotion : false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: ({ overlay }: { overlay?: React.ReactNode }) => (
    <div data-testid="pane-stub">{overlay}</div>
  ),
}))
installBootStubs()
const { MobileShell } = await import("./MobileShell")
const { THEATER_TRANSITION_MS } = await import("@/lib/theater")
const { FLIGHT_ATTACH_HOLD_MS, FLIGHT_CHROME_SLACK_MS, FLIGHT_TRAVEL_HOLD_MS } =
  await import("@/lib/theaterFlight")

/// The chrome stage's whole hold: its transition plus the frame of slack the
/// transition starts late by. What the next stage measures is the dock this
/// wait exists to let settle.
const CHROME_HOLD_MS = THEATER_TRANSITION_MS + FLIGHT_CHROME_SLACK_MS

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
  } as AgentTabView
}

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [tab("s1"), tab("t2")],
  } as unknown as SessionView
}

function makeState(theater: boolean): DuxState {
  return {
    spine: {
      projects: [{ id: "p1", name: "dux" }],
      sessions: [session()],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: { title: "dux", agent_tabs_max: 4, available_providers: ["claude"] },
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    theater,
    terminalEpoch: 0,
    startedDormantTabs: [],
    pendingSlotTab: {},
    routeNotFound: null,
    createTabInFlight: [],
    mobileScreen: "terminal",
    mobileTopBarOverride: null,
    changes: { sessionId: "s1", phase: "loaded", staged: [], unstaged: [] },
  } as unknown as DuxState
}

const flap = () => screen.queryByTestId("mobile-action-flap")
const pill = () => screen.queryByTestId("theater-pill")

/// What the two clusters look like right now, as one readable tuple.
function shot() {
  const f = flap()
  const p = pill()
  return {
    flap: f === null ? "gone" : f.className.includes("invisible") ? "hidden" : "shown",
    pill:
      p === null
        ? "gone"
        : p.className.includes("dux-flight-out")
          ? "leaving"
          : p.className.includes("dux-flight-in")
            ? "returning"
            : p.className.includes("dux-flight-attach")
              ? "attaching"
              : "floating",
  }
}

beforeEach(() => {
  reducedMotion = false
  installBootStubs()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  cleanup()
  vi.unstubAllGlobals()
})

function tick(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms)
  })
}

describe("entering theater on a phone", () => {
  it("collapses the chrome first, with the flap still on screen", () => {
    mockState = makeState(false)
    const view = render(<MobileShell />)
    expect(shot()).toEqual({ flap: "shown", pill: "gone" })

    mockState = makeState(true)
    act(() => view.rerender(<MobileShell />))
    // The chrome is leaving and the flap rides up with it: nothing has
    // detached yet, and there is no second cluster on screen.
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("true")
    expect(shot()).toEqual({ flap: "shown", pill: "gone" })
  })

  it("hands the cluster over as one object, never as two on screen at once", () => {
    mockState = makeState(false)
    const view = render(<MobileShell />)
    mockState = makeState(true)
    act(() => view.rerender(<MobileShell />))

    tick(CHROME_HOLD_MS)
    // Mid-flight: the capsule is in the air and the flap is only there to have
    // been measured.
    expect(shot()).toEqual({ flap: "hidden", pill: "leaving" })

    tick(FLIGHT_TRAVEL_HOLD_MS)
    expect(shot()).toEqual({ flap: "gone", pill: "floating" })
  })

  it("keeps the typing surface: only the TOP chrome leaves", () => {
    mockState = makeState(true)
    render(<MobileShell />)
    // The pane keeps its own column, which is where the compose bar and the
    // terminal keys live. Theater takes the header and the tab strip and
    // nothing below the terminal.
    expect(screen.getByTestId("pane-stub")).toBeTruthy()
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("true")
  })
})

describe("leaving theater on a phone", () => {
  function enterAndSettle() {
    mockState = makeState(false)
    const view = render(<MobileShell />)
    mockState = makeState(true)
    act(() => view.rerender(<MobileShell />))
    tick(CHROME_HOLD_MS)
    tick(FLIGHT_TRAVEL_HOLD_MS)
    return view
  }

  it("brings the chrome back before the capsule flies to it", () => {
    const view = enterAndSettle()
    mockState = makeState(false)
    act(() => view.rerender(<MobileShell />))
    // The dock has to be a real place before anything aims at it, so the flap
    // is mounted (unpainted) and the pill has not moved yet.
    expect(shot()).toEqual({ flap: "hidden", pill: "floating" })
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("false")
  })

  it("travels, then snaps, then swaps in the real flap", () => {
    const view = enterAndSettle()
    mockState = makeState(false)
    act(() => view.rerender(<MobileShell />))

    tick(CHROME_HOLD_MS)
    expect(shot()).toEqual({ flap: "hidden", pill: "returning" })

    // Arrival is its own stage: the shape morph runs only once the travel is
    // over, so nothing flies through the air wearing a tab shape.
    tick(FLIGHT_TRAVEL_HOLD_MS)
    expect(shot()).toEqual({ flap: "hidden", pill: "attaching" })

    tick(FLIGHT_ATTACH_HOLD_MS)
    expect(shot()).toEqual({ flap: "shown", pill: "gone" })
  })
})

describe("a viewer who asked for less motion", () => {
  it("gets both swaps instantly, with no stage in between", () => {
    reducedMotion = true
    installBootStubs()
    mockState = makeState(false)
    const view = render(<MobileShell />)
    expect(shot()).toEqual({ flap: "shown", pill: "gone" })

    mockState = makeState(true)
    act(() => view.rerender(<MobileShell />))
    expect(shot()).toEqual({ flap: "gone", pill: "floating" })

    mockState = makeState(false)
    act(() => view.rerender(<MobileShell />))
    expect(shot()).toEqual({ flap: "shown", pill: "gone" })
  })
})

describe("a page that opens straight into theater", () => {
  it("runs no flight at all, because there was never a dock on screen", () => {
    mockState = makeState(true)
    render(<MobileShell />)
    expect(shot()).toEqual({ flap: "gone", pill: "floating" })
  })
})

describe("the cluster the pill carries on a phone", () => {
  it("is the flap's four controls, in the flap's order", () => {
    mockState = makeState(true)
    render(<MobileShell />)
    const box = screen.getByTestId("theater-pill")
    // The way out is the theater toggle at the head of the cluster, not a
    // separate exit: the same control, in the same slot, saying it is pressed.
    const toggle = screen.getByLabelText("Leave theater mode")
    expect(box.contains(toggle)).toBe(true)
    expect(box.contains(screen.getByLabelText("Run a macro"))).toBe(true)
    expect(box.contains(screen.getByLabelText(/changed files$/))).toBe(true)
    expect(box.contains(screen.getByLabelText("Settings"))).toBe(true)
    expect(toggle.getAttribute("aria-pressed")).toBe("true")
  })

  it("keeps the grip, at the width the flight grows it from nothing", () => {
    mockState = makeState(true)
    render(<MobileShell />)
    const grip = screen.getByTestId("theater-pill-grip")
    expect(grip.className).toContain("dux-pill-grip")
    expect(grip.className).toContain("w-[18px]")
    // The floor is kept on the axis it can be kept on.
    expect(grip.className).toContain("h-10")
  })
})
