// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

let mockState: DuxState
const exitTheaterMock = vi.fn()
const selectTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    exitTheater: (...a: unknown[]) => exitTheaterMock(...a),
    selectTab: (...a: unknown[]) => selectTabMock(...a),
  }
})

const notifyInfoMock = vi.fn()
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return { ...actual, notifyInfo: (...a: unknown[]) => notifyInfoMock(...a) }
})

// jsdom has no ResizeObserver, and the pill watches its surface with one. The
// stub keeps every construction so a test can play a rotation.
const resizeCallbacks: Array<(entries: unknown[], observer: unknown) => void> = []

// Every observer in the tree gets the notification a real resize would deliver,
// the tooltip's positioner included, so the entries have to look real enough
// for a library that reads them. WHICH BOX resized is part of the notification:
// the pill watches two, and only the surface changing shape ends a live drag.
function playResize(target: Element = surfaceElement()) {
  const entry = { contentRect: { width: 0, height: 0 }, target }
  resizeCallbacks.forEach((cb) => cb([entry], {}))
}

/// The pill's own box changed, which is what folding the tab strip out does.
function playPillResize() {
  playResize(screen.getByTestId("theater-pill"))
}

/// The surface the pill floats over IS its offset parent, by construction.
function surfaceElement(): Element {
  return screen.getByTestId("theater-pill").parentElement ?? document.body
}

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
  resizeCallbacks.length = 0
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: (entries: unknown[], observer: unknown) => void) {
        resizeCallbacks.push(cb)
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
}
installBootStubs()
const { TheaterPill } = await import("./TheaterPill")
const { peekTheaterTabs } = await import("@/lib/theater")
const { THEATER_PILL_HINT_KEY, THEATER_PILL_POSITION_KEY } = await import(
  "@/lib/theaterPill"
)

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

function session(tabs: AgentTabView[]): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "",
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: "",
    },
    tabs,
  } as unknown as SessionView
}

// The pill measures itself and the surface it floats over, and jsdom lays
// nothing out. One rect stub answers for both: the pill's own box by its test
// id, everything else (its parent, which IS the surface) the pane.
const PILL_BOX = { width: 200, height: 48 }
// The pill's own box is a variable because folding the tab strip out really does
// widen it, and jsdom lays nothing out: a test plays that growth here.
let pillBox = PILL_BOX
let surfaceBox = { width: 800, height: 600 }
const realRect = Element.prototype.getBoundingClientRect

function stubRects() {
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const pill =
      this instanceof HTMLElement && this.dataset.testid === "theater-pill"
    const box = pill ? pillBox : surfaceBox
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: box.width,
      bottom: box.height,
      width: box.width,
      height: box.height,
      toJSON: () => ({}),
    } as DOMRect
  }
}

beforeEach(() => {
  installBootStubs()
  exitTheaterMock.mockReset()
  selectTabMock.mockReset()
  notifyInfoMock.mockReset()
  surfaceBox = { width: 800, height: 600 }
  pillBox = PILL_BOX
  stubRects()
  mockState = { bootstrap: null } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
})

const agentTarget = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }
const terminalTarget = {
  kind: "terminal" as const,
  terminalId: "tm1",
  owner: { kind: "standalone" as const },
}

describe("the floating theater pill", () => {
  it("always carries the way out", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    fireEvent.click(screen.getByRole("button", { name: "Leave theater mode" }))
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
  })

  it("carries a macros trigger beside it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(screen.getByRole("button", { name: "Run a macro" })).toBeTruthy()
  })

  it("collapses to macros and exit for a terminal, which has no tabs", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(screen.queryByRole("button", { name: /other tab/i })).toBeNull()
  })

  it("collapses for a single-tab agent, so the expander is never empty", () => {
    render(
      <TheaterPill target={agentTarget} session={session([tab({ id: "s1" })])} />,
    )
    expect(screen.queryByRole("button", { name: /other tab/i })).toBeNull()
  })

  it("offers the hidden tabs when there are some, and switches to one", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(status.getAttribute("aria-expanded")).toBe("false")
    fireEvent.click(status)
    expect(status.getAttribute("aria-expanded")).toBe("true")

    // The switch CARRIES the mode rather than reading the destination's memory,
    // so reaching for a sibling that has never been in theater stays in it.
    fireEvent.click(screen.getByRole("tab", { name: /codex/i }))
    expect(selectTabMock).toHaveBeenCalledWith("s1", "t2", { theater: true })
  })

  it("puts the folded-out strip away on a tap anywhere else", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    fireEvent.click(status)
    expect(status.getAttribute("aria-expanded")).toBe("true")
    fireEvent.pointerDown(document.body)
    expect(status.getAttribute("aria-expanded")).toBe("false")
  })

  it("keeps the strip open for a press inside the pill itself", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    fireEvent.click(status)
    fireEvent.pointerDown(screen.getByTestId("theater-pill"))
    expect(status.getAttribute("aria-expanded")).toBe("true")
  })

  it("publishes the strip so the page-wide Escape rule can collapse it", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(peekTheaterTabs()?.expanded()).toBe(false)
    fireEvent.click(status)
    expect(peekTheaterTabs()?.expanded()).toBe(true)
    act(() => peekTheaterTabs()?.collapse())
    expect(status.getAttribute("aria-expanded")).toBe("false")
  })

  it("takes focus onto the way out when the chrome left nothing focused", () => {
    // Entering from the header button destroys that button, so focus falls to
    // the body; the pill is the nearest thing to what the user was doing.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Leave theater mode" }),
    )
  })

  it("leaves focus alone when something else already holds it", () => {
    const field = document.createElement("textarea")
    document.body.appendChild(field)
    field.focus()
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(document.activeElement).toBe(field)
    field.remove()
  })

  it("marks the tab on screen as the selected one", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /other tabs/i }))
    expect(
      screen.getByRole("tab", { name: /claude/i }).getAttribute("aria-selected"),
    ).toBe("true")
  })

  it("shows the attention dot for a background tab that needs you", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1" }),
          tab({ id: "t2", provider: "codex", needs_attention: true }),
        ])}
      />,
    )
    expect(screen.getAllByLabelText("Needs attention").length).toBeGreaterThan(0)
  })

  it("carries both hidden-tab cues in the expander's own row", () => {
    // Never absolutely placed in the corners of its box: a ghost control
    // paints no surface, so a mark parked there floats in the pill's dead
    // space and reads as the neighbouring control's. In the row it is
    // unambiguously the expander's.
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1" }),
          tab({
            id: "t2",
            provider: "codex",
            working: true,
            needs_attention: true,
          }),
        ])}
      />,
    )
    const expander = screen.getByRole("button", { name: /other tabs/i })
    expect(expander.contains(screen.getByLabelText("Needs attention"))).toBe(true)
    expect(expander.querySelectorAll(".absolute").length).toBe(0)
    // The cue-bearing expander keeps the cluster's one height and grows only
    // in width.
    expect(expander.className).toContain("h-10")
  })

  it("stays a bare circle when the hidden tabs have nothing to say", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const expander = screen.getByRole("button", { name: /other tabs/i })
    expect(expander.className).toContain("w-10")
  })

  it("says nothing about the tab already filling the screen", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1", needs_attention: true }),
          tab({ id: "t2", provider: "codex" }),
        ])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    expect(status.getAttribute("aria-label")).not.toMatch(/attention/i)
  })
})

// WHERE THE PILL SITS, AND MOVING IT.
//
// The pill is the one thing left floating over a full-height terminal, so it is
// also the one thing that can cover the line the user is waiting for. The answer
// is a drag, and these pin the two halves that make a drag safe here: the grip's
// hold is what separates it from the taps of the buttons beside it, and nothing
// under the pill may see the pointer while it moves, or the terminal starts a
// long-press selection under the user's finger.
const CORNER = { left: "586px", top: "538px" }

function pill() {
  return screen.getByTestId("theater-pill")
}

function grip() {
  return screen.getByTestId("theater-pill-grip")
}

function down(el: Element, opts: Record<string, unknown>) {
  fireEvent.pointerDown(el, { pointerId: 1, isPrimary: true, ...opts })
}

function move(x: number, y: number, pointerType = "mouse") {
  fireEvent.pointerMove(grip(), { pointerId: 1, pointerType, clientX: x, clientY: y })
}

function up(x: number, y: number, pointerType = "mouse") {
  fireEvent.pointerUp(grip(), { pointerId: 1, pointerType, clientX: x, clientY: y })
}

function stored() {
  const raw = localStorage.getItem(THEATER_PILL_POSITION_KEY)
  return raw === null ? null : JSON.parse(raw)
}

describe("moving the theater pill", () => {
  it("starts in the corner it has always sat in", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe(CORNER.left)
    expect(pill().style.top).toBe(CORNER.top)
  })

  it("comes back where this device left it", () => {
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":40,"y":60}')
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe("40px")
    expect(pill().style.top).toBe("60px")
  })

  it("clamps a remembered position into a surface that has since shrunk", () => {
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":700,"y":500}')
    surfaceBox = { width: 390, height: 400 }
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe("190px")
    expect(pill().style.top).toBe("352px")
  })

  it("ignores a stored position that is not one", () => {
    localStorage.setItem(THEATER_PILL_POSITION_KEY, "{oh no}")
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe(CORNER.left)
  })

  it("carries a grip that says what it is for", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(grip().getAttribute("aria-label")).toMatch(/hold to move/i)
  })

  it("moves with a mouse drag and remembers where it landed", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    // Past the travel gate: from here the press is a drag.
    move(590, 540)
    move(500, 400)
    up(500, 400)
    expect(pill().style.left).toBe("486px")
    expect(pill().style.top).toBe("388px")
    expect(stored()).toEqual({ x: 486, y: 388 })
  })

  it("keeps the pill inside the surface however far the pointer goes", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(-900, -900)
    up(-900, -900)
    expect(pill().style.left).toBe("0px")
    expect(pill().style.top).toBe("0px")
  })

  it("does nothing on a plain tap of the grip", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(602, 551)
    up(602, 551)
    expect(pill().style.left).toBe(CORNER.left)
    expect(stored()).toBeNull()
  })

  it("forgets a gesture the browser cancels out from under it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    fireEvent.pointerCancel(grip(), { pointerId: 1, pointerType: "mouse" })
    // The whole gesture is gone, not merely paused: the moves that follow are
    // somebody else's, and the release at the end of them commits nothing.
    move(400, 300)
    up(400, 300)
    expect(pill().style.left).toBe(CORNER.left)
    expect(pill().style.top).toBe(CORNER.top)
    expect(stored()).toBeNull()
  })

  it("lets go of a pending hold when the pill is unmounted under the finger", () => {
    vi.useFakeTimers()
    try {
      const view = render(
        <TheaterPill target={terminalTarget} session={undefined} />,
      )
      // Counted as a DELTA: the tooltip library keeps timers of its own, and
      // the one this test is about is the hold the press arms.
      const idle = vi.getTimerCount()
      down(grip(), { pointerType: "touch", clientX: 600, clientY: 550 })
      expect(vi.getTimerCount()).toBe(idle + 1)
      view.unmount()
      expect(vi.getTimerCount()).toBeLessThanOrEqual(idle)
      act(() => void vi.advanceTimersByTime(600))
      expect(stored()).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  // A SURFACE THAT CHANGES SHAPE MID-DRAG ENDS THE DRAG.
  //
  // The transform the pill is moved by is measured from the base it was pressed
  // at, and a re-clamp moves the coordinates that base is written in: applied
  // together they move the pill twice. Rebasing the live gesture would mean
  // re-projecting the pointer into a surface it never pressed in, for a gesture
  // whose two causes (a rotation, a keyboard coming up) have already interrupted
  // the user. Ending it is the simpler correct answer, and it commits, like
  // every other interruption here.
  it("ends a live drag when the surface changes shape under it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    surfaceBox = { width: 390, height: 400 }
    act(() => playResize())
    expect(pill().style.transform).toBe("")
    expect(stored()).toEqual({ x: 486, y: 388 })
    // Clamped into the surface it now has, and deaf to the rest of the gesture.
    expect(pill().style.left).toBe("190px")
    move(200, 100)
    up(200, 100)
    expect(pill().style.left).toBe("190px")
  })

  it("keeps a live drag through the pill's own box changing size", () => {
    // The strip folds away ON THE LIFT, so the pill's box resizes at the very
    // start of every drag: treating that as an interruption would end them all.
    render(<TheaterPill target={agentTarget} session={twoTabs()} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    pillBox = { width: 180, height: 48 }
    act(() => playPillResize())
    move(400, 300)
    up(400, 300)
    expect(pill().style.left).toBe("386px")
    expect(pill().style.top).toBe("288px")
  })

  // THE TWO WAYS A GESTURE ENDS WITHOUT A RELEASE, both of which the divider
  // drags already handle. A capture the browser hands to something else and a
  // window that loses focus leave the pill mid-drag: the transform is on the
  // element, `dragging` is true, and no pointerup is ever coming.
  //
  // Both COMMIT rather than revert. The pill is already under the pointer, the
  // user watched it get there, and snapping it back to a corner they deliberately
  // moved it off would undo the gesture after the fact. A press that never
  // lifted still commits nothing: that is a tap, and taps do nothing here.
  it("commits and settles when the browser takes the capture away", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    fireEvent.lostPointerCapture(grip(), { pointerId: 1, pointerType: "mouse" })
    expect(pill().style.left).toBe("486px")
    expect(pill().style.transform).toBe("")
    expect(stored()).toEqual({ x: 486, y: 388 })
    // And the gesture is forgotten: a late release moves nothing.
    up(300, 200)
    expect(pill().style.left).toBe("486px")
  })

  it("keeps a tap a tap when the capture goes away before the lift", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    fireEvent.lostPointerCapture(grip(), { pointerId: 1, pointerType: "mouse" })
    expect(pill().style.left).toBe(CORNER.left)
    expect(stored()).toBeNull()
  })

  it("commits and settles when the window loses focus mid-drag", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    fireEvent.blur(window)
    expect(pill().style.left).toBe("486px")
    expect(pill().style.transform).toBe("")
    expect(stored()).toEqual({ x: 486, y: 388 })
  })

  it("lifts under a finger only after the hold, and moves from there", () => {
    vi.useFakeTimers()
    try {
      render(<TheaterPill target={terminalTarget} session={undefined} />)
      down(grip(), { pointerType: "touch", clientX: 600, clientY: 550 })
      act(() => void vi.advanceTimersByTime(300))
      move(560, 500, "touch")
      up(560, 500, "touch")
      expect(pill().style.left).toBe("546px")
      expect(pill().style.top).toBe("488px")
    } finally {
      vi.useRealTimers()
    }
  })

  it("leaves a short finger press alone, so a tap stays a tap", () => {
    vi.useFakeTimers()
    try {
      render(<TheaterPill target={terminalTarget} session={undefined} />)
      down(grip(), { pointerType: "touch", clientX: 600, clientY: 550 })
      act(() => void vi.advanceTimersByTime(120))
      up(600, 550, "touch")
      act(() => void vi.advanceTimersByTime(500))
      expect(pill().style.left).toBe(CORNER.left)
      expect(stored()).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it("abandons a finger that slid away before the hold, which was a scroll", () => {
    vi.useFakeTimers()
    try {
      render(<TheaterPill target={terminalTarget} session={undefined} />)
      down(grip(), { pointerType: "touch", clientX: 600, clientY: 550 })
      move(600, 500, "touch")
      act(() => void vi.advanceTimersByTime(600))
      move(600, 400, "touch")
      up(600, 400, "touch")
      expect(pill().style.left).toBe(CORNER.left)
      expect(pill().style.top).toBe(CORNER.top)
    } finally {
      vi.useRealTimers()
    }
  })

  // NOTHING ABOVE THE PILL SEES THE GESTURE, for either pointer kind.
  //
  // What this protects is the ANCESTORS: the pane box the pill is rendered in
  // and everything above it, which own drag, focus and dismissal handlers of
  // their own. The terminal's long-press selection is NOT among them (xterm's
  // listeners live in a sibling subtree, not an ancestor of the pill); that one
  // is the grip's `touch-none`, which stops the browser scrolling, zooming or
  // raising its own long-press out from under the drag.
  it.each(["mouse", "touch"])(
    "keeps every %s event of the gesture away from the tree above it",
    (pointerType) => {
      vi.useFakeTimers()
      try {
        const seen: string[] = []
        render(
          <div
            onPointerDown={() => seen.push("down")}
            onPointerMove={() => seen.push("move")}
            onPointerUp={() => seen.push("up")}
          >
            <TheaterPill target={terminalTarget} session={undefined} />
          </div>,
        )
        down(grip(), { pointerType, clientX: 600, clientY: 550 })
        // A finger only owns the pill after the hold, so give it one.
        act(() => void vi.advanceTimersByTime(300))
        move(500, 450, pointerType)
        up(500, 450, pointerType)
        expect(seen).toEqual([])
      } finally {
        vi.useRealTimers()
      }
    },
  )

  it("puts the folded-out strip away the moment the pill lifts", () => {
    render(
      <TheaterPill
        target={agentTarget}
        session={session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])}
      />,
    )
    const status = screen.getByRole("button", { name: /other tabs/i })
    fireEvent.click(status)
    expect(status.getAttribute("aria-expanded")).toBe("true")
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(560, 500)
    expect(status.getAttribute("aria-expanded")).toBe("false")
  })

  it("still leaves theater on a tap of the way out", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    fireEvent.click(screen.getByRole("button", { name: "Leave theater mode" }))
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
    expect(pill().style.left).toBe(CORNER.left)
  })

  it("moves a step per arrow key while the grip has focus, and remembers it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    fireEvent.keyDown(grip(), { key: "ArrowLeft" })
    expect(pill().style.left).toBe("570px")
    fireEvent.keyDown(grip(), { key: "ArrowUp" })
    expect(pill().style.top).toBe("522px")
    expect(stored()).toEqual({ x: 570, y: 522 })
  })

  it("leaves every other key on the grip to the page", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(fireEvent.keyDown(grip(), { key: "Enter" })).toBe(true)
    expect(pill().style.left).toBe(CORNER.left)
  })

  it("re-derives its corner when the surface rotates under it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    surfaceBox = { width: 390, height: 400 }
    act(() => playResize())
    // Nobody has placed this pill, so the rotation gets the corner it would
    // have started in on a surface this size, margin and all, rather than the
    // old corner clamped flush against the new edge.
    expect(pill().style.left).toBe("176px")
    expect(pill().style.top).toBe("338px")
  })

  // WHAT THE USER ASKED FOR IS KEPT SEPARATELY FROM WHERE THE PILL FITS TODAY.
  //
  // Every clamp is against a surface and a pill of the moment, and both change:
  // folding the tab strip out widens the pill and shoves it left. Clamping the
  // already-clamped value makes that shove permanent, because the position it
  // would return to has been overwritten by the position it was pushed to. So
  // the intent is what is stored and re-clamped, and the clamp is re-derived.
  const expandedPill = { width: 420, height: 48 }

  function twoTabs() {
    return session([tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })])
  }

  function expander() {
    return screen.getByRole("button", { name: /other tabs/i })
  }

  it("gives the corner back when the expander that shoved it folds away", () => {
    render(<TheaterPill target={agentTarget} session={twoTabs()} />)
    expect(pill().style.left).toBe(CORNER.left)

    fireEvent.click(expander())
    pillBox = expandedPill
    act(() => playPillResize())
    // Still the corner, for a wider pill: an unplaced pill keeps its margin.
    expect(pill().style.left).toBe("366px")

    fireEvent.click(expander())
    pillBox = PILL_BOX
    act(() => playPillResize())
    expect(pill().style.left).toBe(CORNER.left)
  })

  it("gives a dragged position back after the same shove", () => {
    render(<TheaterPill target={agentTarget} session={twoTabs()} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(900, 550)
    up(900, 550)
    expect(pill().style.left).toBe("600px")

    fireEvent.click(expander())
    pillBox = expandedPill
    act(() => playPillResize())
    expect(pill().style.left).toBe("380px")

    fireEvent.click(expander())
    pillBox = PILL_BOX
    act(() => playPillResize())
    expect(pill().style.left).toBe("600px")
    // And the shove was never mistaken for a choice.
    expect(stored()).toEqual({ x: 600, y: 538 })
  })

  it("never writes a re-clamp to storage, because a window moved the pill", () => {
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":500,"y":400}')
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe("500px")

    surfaceBox = { width: 390, height: 400 }
    act(() => playResize())
    expect(pill().style.left).toBe("190px")
    expect(stored()).toEqual({ x: 500, y: 400 })

    surfaceBox = { width: 800, height: 600 }
    act(() => playResize())
    expect(pill().style.left).toBe("500px")
  })

  it("writes nothing at all when nobody has ever moved it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    surfaceBox = { width: 390, height: 400 }
    act(() => playResize())
    expect(stored()).toBeNull()
  })

  it("teaches the grip once per device and never again", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(notifyInfoMock).toHaveBeenCalledTimes(1)
    expect(String(notifyInfoMock.mock.calls[0][0])).toMatch(/grip/i)
    // Not sticky: nothing is lost if it goes unread, and a pinned toast over a
    // mode whose whole purpose is screen space would be its own joke. Asserted
    // as the CALL SHAPE, because reading `.sticky` off an options argument that
    // is not passed at all is undefined either way and can never fail.
    expect(notifyInfoMock.mock.calls[0]).toHaveLength(1)
    cleanup()
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(notifyInfoMock).toHaveBeenCalledTimes(1)
    expect(localStorage.getItem(THEATER_PILL_HINT_KEY)).toBe("shown")
  })

  it("says nothing on a device that has already been told", () => {
    localStorage.setItem(THEATER_PILL_HINT_KEY, "shown")
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(notifyInfoMock).not.toHaveBeenCalled()
  })

  it("drops the settle animation for a viewer who asked for less motion", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn((query: string) => ({
        matches: true,
        media: query,
        onchange: null,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false,
      })),
    )
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().className).not.toContain("transition-[left,top]")
    // The clamp still happens; only the animation is gone.
    expect(pill().style.left).toBe(CORNER.left)
  })

  // THE DROP IS NOT A SETTLE. While the drag is live the pill is moved by a
  // transform, and the drop replaces that transform with real coordinates. Both
  // halves land in one commit, and that commit is also the one that re-enables
  // the settle animation, so an animated drop eases from the corner the pill was
  // dragged OUT of, all the way back to the finger. Measured in a browser at the
  // reviewed commit: 50ms after the release the pill was still 185px from where
  // it was dropped.
  it("lands where it was dropped instead of sliding in from the old corner", async () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    up(500, 400)
    expect(pill().style.left).toBe("486px")
    expect(pill().className).not.toContain("transition-[left,top]")
    // And only for that one commit: every later clamp still eases.
    await act(
      () => new Promise<void>((done) => requestAnimationFrame(() => done())),
    )
    expect(pill().className).toContain("transition-[left,top]")
  })

  it("settles with an animation for everybody else", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().className).toContain("transition-[left,top]")
  })
})
