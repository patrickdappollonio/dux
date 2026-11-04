// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { readFileSync } from "node:fs"

import type { DuxState, SelectedTarget } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

// THE DOCKED FLAP, the phone's action cluster while theater is off. What is
// pinned here is what the mock is a spec about: which four controls it carries,
// that its silhouette is generated from its own measured box rather than drawn
// at a fixed width, and that its body takes the color of whichever band it is
// hanging from.

let mockState: DuxState
const toggleTheaterMock = vi.fn()
const openChangedFilesMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    toggleTheater: (...a: unknown[]) => toggleTheaterMock(...a),
    openChangedFiles: (...a: unknown[]) => openChangedFilesMock(...a),
  }
})

let reducedMotion = false

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
installBootStubs()
const { MobileActionFlap } = await import("./MobileActionFlap")

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
    tabs: [{ id: "s1", provider: "claude" } as unknown as AgentTabView],
  } as unknown as SessionView
}

const target: SelectedTarget = { kind: "agent", sessionId: "s1", tabId: "s1" }

function makeState(over: Partial<DuxState> = {}): DuxState {
  return {
    spine: {
      projects: [{ id: "p1", name: "dux" }],
      sessions: [session()],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: { title: "dux", agent_tabs_max: 4, available_providers: ["claude"] },
    selectedSessionId: "s1",
    selectedTarget: target,
    theater: false,
    createTabInFlight: [],
    changes: {
      sessionId: "s1",
      phase: "loaded",
      staged: [],
      unstaged: [{ path: "a" }, { path: "b" }, { path: "c" }],
    },
    ...over,
  } as unknown as DuxState
}

/// The `z-N` utility the class list carries, as a number to compare.
function zLevel(className: string): number {
  const found = /(?:^| )z-(\d+)(?: |$)/.exec(className)
  if (!found) throw new Error(`no z utility in ${className}`)
  return Number(found[1])
}

/// jsdom lays nothing out, so the flap's own box reads zero and no silhouette
/// is drawn. Give it a real one, the way a browser would.
const realRect = Element.prototype.getBoundingClientRect

/// The flap's painted box. Every other element keeps jsdom's empty one.
function stubBox(width: number, height: number, left = 0, top = 0) {
  Element.prototype.getBoundingClientRect = function (this: Element) {
    if ((this as HTMLElement).dataset?.testid !== "mobile-action-flap") {
      return realRect.call(this)
    }
    return {
      x: left,
      y: top,
      left,
      top,
      right: left + width,
      bottom: top + height,
      width,
      height,
      toJSON: () => ({}),
    } as DOMRect
  }
}

beforeEach(() => {
  reducedMotion = false
  installBootStubs()
  toggleTheaterMock.mockReset()
  openChangedFilesMock.mockReset()
  mockState = makeState()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
})

describe("the docked action flap", () => {
  it("carries the four controls the header gave up, and nothing else", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    expect(screen.getByLabelText("Theater mode")).toBeTruthy()
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
    expect(screen.getByLabelText("3 changed files")).toBeTruthy()
    expect(screen.getByLabelText("Session actions")).toBeTruthy()
  })

  it("prints the BARE count beside the diff glyph, which already draws the ±", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    const count = screen.getByTestId("pane-changes-count")
    expect(count.textContent).toBe("3")
    expect(count.textContent).not.toContain("±")
  })

  it("keeps every control on the 40px touch floor", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    for (const label of ["Theater mode", "Run a macro", "Session actions"]) {
      expect(screen.getByLabelText(label).className).toContain("size-10")
    }
    // The count is the one control wider than it is tall, because it is data.
    expect(screen.getByTestId("pane-changes-count").className).toContain("h-10")
    expect(screen.getByTestId("pane-changes-count").className).toContain("w-auto")
  })

  it("opens the changes screen from the count", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    fireEvent.click(screen.getByTestId("pane-changes-count"))
    expect(openChangedFilesMock).toHaveBeenCalled()
  })

  it("asks the store for theater rather than toggling anything itself", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    fireEvent.click(screen.getByLabelText("Theater mode"))
    expect(toggleTheaterMock).toHaveBeenCalled()
  })

  it("stacks both theater icons so the flight has something to morph between", () => {
    const { container } = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />,
    )
    expect(container.querySelector(".dux-ic-max")).not.toBeNull()
    expect(container.querySelector(".dux-ic-min")).not.toBeNull()
  })

  it("draws no silhouette until it has been measured", () => {
    const { container } = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />,
    )
    expect(container.querySelector("svg path[stroke]")).toBeNull()
  })

  it("generates the silhouette from its own measured box", () => {
    stubBox(196, 50)
    const { container } = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />,
    )
    const svg = container.querySelector("svg[viewBox]") as SVGSVGElement | null
    expect(svg).not.toBeNull()
    // 196 body + a fillet and an overhang on each side.
    expect(svg?.getAttribute("viewBox")).toBe("0 0 226 53")
    // One fill and one OPEN stroke: nothing crosses the top, which is where the
    // band is.
    const stroke = container.querySelector("path[stroke]")
    expect(stroke?.getAttribute("d")).not.toContain("Z")
  })

  it("draws the width the browser paints, not a rounded one, at a fractional size", () => {
    // The landing pill is parked on this box's fractional coordinates and its
    // CSS box is painted between edges snapped to device pixels. A silhouette
    // built from a whole-pixel width put its right side up to half a pixel off
    // that at the swap. At 2x: left 185.3 snaps to 185.5 and the right edge,
    // 378.1125, to 378, so 192.5 is painted.
    vi.stubGlobal("devicePixelRatio", 2)
    stubBox(192.8125, 50, 185.3, 43)
    const { container } = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />,
    )
    const svg = container.querySelector("svg[viewBox]") as SVGSVGElement | null
    expect(svg?.getAttribute("viewBox")).toBe("0 0 222.5 53")
  })

  it("takes the band's own color, and the plain background with no band", () => {
    const strip = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />,
    )
    expect(
      (strip.getByTestId("mobile-action-flap") as HTMLElement).style.getPropertyValue(
        "--dux-flap-fill",
      ),
    ).toBe("var(--dux-flap-bg)")
    cleanup()
    const plain = render(
      <MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="plain" />,
    )
    expect(
      (plain.getByTestId("mobile-action-flap") as HTMLElement).style.getPropertyValue(
        "--dux-flap-fill",
      ),
    ).toBe("var(--background)")
  })

  it("paints over the chrome stack, so it can interrupt the band's hairline", () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    const box = screen.getByTestId("mobile-action-flap")
    expect(box.className).toContain("-top-px")
    expect(box.className).toContain("z-30")
  })

  it("paints over the pane's full-pane covers, which is what it is for", () => {
    // The flap is the ONLY surface carrying these controls while a cover owns
    // the terminal (the pane's overlay slot is withheld there), and a watcher
    // reaching the session's actions is exactly that state. Neither the pane's
    // root nor the shell's column starts a stacking context, so the two levels
    // are compared directly and the later element in the document wins a tie.
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    const flapZ = zLevel(screen.getByTestId("mobile-action-flap").className)
    const pane = readFileSync("src/components/TerminalPane.tsx", "utf8")
    // The pane's floating-overlay layer is the one exemption: it is not a
    // cover, and the pane withholds it whenever a cover owns the pane. Exempted
    // by its own marker, so any other layer above the flap is still caught.
    const overlayLayer = pane.match(
      /data-testid="pane-overlay-layer"\s+className="([^"]*)"/,
    )?.[1]
    expect(overlayLayer, "the overlay layer's class list was not found").toBeDefined()
    const covers = [...pane.matchAll(/className="([^"]*)"/g)]
      .map((m) => m[1])
      .filter((c) => c !== overlayLayer)
      .flatMap((c) => [...c.matchAll(/absolute inset-0 z-(\d+)/g)])
      .map((m) => Number(m[1]))
    expect(covers.length).toBeGreaterThan(0)
    for (const cover of covers) expect(flapZ).toBeGreaterThan(cover)
  })
})

// THE SAME FLAP OVER A TERMINAL. One component, parameterized by what the pane
// is about: a project or standalone terminal has no changed-file summary, so
// the cluster is three controls instead of four and the silhouette is measured
// off whatever that turns out to be rather than off a fixed path.
describe("the docked action flap over a terminal", () => {
  const terminalTarget = {
    kind: "terminal" as const,
    terminalId: "t1",
    owner: { kind: "standalone" as const },
  }
  const terminalSubject = {
    kind: "terminal" as const,
    terminalId: "t1",
    owner: { kind: "standalone" as const },
  }

  it("carries the pane's controls with no count at all", () => {
    render(
      <MobileActionFlap
        target={terminalTarget}
        subject={terminalSubject}
        band="plain"
      />,
    )
    expect(screen.getByLabelText("Theater mode")).toBeTruthy()
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
    expect(screen.getByLabelText("Terminal actions")).toBeTruthy()
    expect(screen.queryByTestId("pane-changes-count")).toBeNull()
  })

  it("keeps every control on the 40px touch floor", () => {
    render(
      <MobileActionFlap
        target={terminalTarget}
        subject={terminalSubject}
        band="plain"
      />,
    )
    for (const label of ["Theater mode", "Run a macro", "Terminal actions"]) {
      expect(screen.getByLabelText(label).className).toContain("size-10")
    }
  })

  it("measures its silhouette off its own narrower box", () => {
    stubBox(150, 50)
    const { container } = render(
      <MobileActionFlap
        target={terminalTarget}
        subject={terminalSubject}
        band="plain"
      />,
    )
    const svg = container.querySelector("svg[viewBox]") as SVGSVGElement | null
    // 150 body plus the same fillet and overhang on each side the agent's flap
    // gets: the shape follows the cluster, which is the whole reason it is
    // generated rather than drawn once.
    expect(svg?.getAttribute("viewBox")).toBe("0 0 180 53")
  })
})

describe("the theater toggle's glyph swap", () => {
  /// jsdom has no Web Animations: each one is modelled as a direction, a time
  /// and whether it is still playing, which is all a turn reads or writes.
  interface Glyph {
    el: Element
    keyframes: Keyframe[]
    delay: number
    rate: number
    time: number | null
    state: AnimationPlayState
  }
  let made: Glyph[] = []
  beforeEach(() => {
    made = []
    Object.defineProperty(Element.prototype, "animate", {
      configurable: true,
      writable: true,
      value: function (this: Element, keyframes: Keyframe[], timing: KeyframeAnimationOptions) {
        const a: Glyph = {
          el: this,
          keyframes,
          delay: timing.delay ?? 0,
          rate: 1,
          time: null,
          state: "running",
        }
        made.push(a)
        const end = a.delay + Number(timing.duration ?? 0)
        return {
          get playState() {
            return a.state
          },
          get playbackRate() {
            return a.rate
          },
          set playbackRate(v: number) {
            a.rate = v
          },
          get currentTime() {
            return a.time
          },
          set currentTime(v: number | null) {
            a.time = v
          },
          effect: { getComputedTiming: () => ({ endTime: end }) },
          pause: () => {
            a.state = "paused"
            if (a.time === null) a.time = 0
          },
          play: () => void (a.state = "running"),
          cancel: () => void (a.state = "idle"),
        } as unknown as Animation
      },
    })
  })
  afterEach(() => {
    // @ts-expect-error jsdom has no Web Animations of its own to restore
    delete Element.prototype.animate
  })

  const glyph = (a: Glyph) =>
    a.el.getAttribute("class")?.includes("dux-ic-max") ? "max" : "min"
  const opacityTo = (a: Glyph) => Number(a.keyframes.at(-1)?.opacity)

  function flap() {
    return (
      <MobileActionFlap
        target={target}
        subject={{ kind: "agent", session: session() }}
        band="strip"
      />
    )
  }

  it("starts the swap under the finger, in the same commit the mode flips", () => {
    // The flap stays on screen while the chrome collapses, so the swap the
    // press asked for plays here, at once: not a flat swap now and a second
    // one when the flight takes off.
    const view = render(flap())
    expect(made).toEqual([])
    mockState = makeState({ theater: true })
    view.rerender(flap())
    expect(made.map((a) => [glyph(a), opacityTo(a), a.delay]).sort()).toEqual([
      ["max", 0, 0],
      ["min", 1, 0],
    ])
  })

  it("turns a swap still in flight around from the frame it shows", () => {
    const view = render(flap())
    mockState = makeState({ theater: true })
    view.rerender(flap())
    const first = [...made]
    expect(first.length).toBe(2)
    first.forEach((a) => (a.time = 90))
    mockState = makeState({ theater: false })
    view.rerender(flap())
    expect(made.length).toBe(first.length)
    expect(first.map((a) => [a.rate, a.time, a.state])).toEqual(
      first.map(() => [-1, 90, "running"]),
    )
  })

  it("starts a fresh swap once the last one has finished", () => {
    const view = render(flap())
    mockState = makeState({ theater: true })
    view.rerender(flap())
    made.forEach((a) => (a.state = "finished"))
    mockState = makeState({ theater: false })
    view.rerender(flap())
    expect(made.slice(2).map((a) => [glyph(a), opacityTo(a)]).sort()).toEqual([
      ["max", 1],
      ["min", 0],
    ])
  })

  it("stops a swap already playing when the viewer asks for less motion", () => {
    const view = render(flap())
    mockState = makeState({ theater: true })
    view.rerender(flap())
    expect(made.length).toBe(2)
    reducedMotion = true
    view.rerender(flap())
    expect(made.map((a) => a.state)).toEqual(["idle", "idle"])
  })

  it("swaps flat for a viewer who asked for less motion", () => {
    reducedMotion = true
    const view = render(flap())
    mockState = makeState({ theater: true })
    view.rerender(flap())
    expect(made).toEqual([])
  })
})
