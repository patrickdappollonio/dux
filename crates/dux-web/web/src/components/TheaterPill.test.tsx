// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"
import {
  registerAttachCapability,
  resetAttachCapabilities,
} from "@/lib/attachRegistry"
import {
  registerPaneInputGroup,
  resetPaneInputGroups,
} from "@/lib/paneInputGroup"

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
const { appMenuModel } = await import("@/lib/appMenu")
const {
  THEATER_PILL_GRIP_SLOT_PX,
  THEATER_PILL_GRIP_W_PX,
  THEATER_PILL_HINT_KEY,
  THEATER_PILL_POSITION_KEY,
  THEATER_PILL_ROW_GAP_PX,
} = await import("@/lib/theaterPill")
const { FLAP_FILL_VAR, registerFlapElement } = await import(
  "@/lib/theaterFlight"
)
type FlightPhase = import("@/lib/theaterFlight").FlightPhase

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
const PILL_BOX = { left: 0, top: 0, width: 200, height: 48 }
// The pill's own box is a variable because folding the tab strip out really does
// widen it, and jsdom lays nothing out: a test plays that growth here.
let pillBox = PILL_BOX
let surfaceBox = { left: 0, top: 0, width: 800, height: 600 }
// The docked flap the flight measures. It is not in the pill's tree (the flap is
// a sibling of the pane), so it carries its own test id and answers here.
let flapBox = { left: 0, top: 0, width: 200, height: 48 }
const realRect = Element.prototype.getBoundingClientRect

function rect(box: { left: number; top: number; width: number; height: number }) {
  return {
    x: box.left,
    y: box.top,
    top: box.top,
    left: box.left,
    right: box.left + box.width,
    bottom: box.top + box.height,
    width: box.width,
    height: box.height,
    toJSON: () => ({}),
  } as DOMRect
}

// WHERE AN UNPLACED BOX FALLS. A real browser does not hold an absolutely
// positioned element wherever it last was when the inline insets are taken off
// it: the box drops to wherever static layout puts it, which for the pill's
// overlay is the top-left corner. A stub that answered with the pill's placed
// coordinates whether or not they were on the element could not tell a
// measurement of the real pill from a measurement of that corner, which is
// exactly the difference the return flight got wrong.
const PILL_UNPLACED = { left: 4, top: 6 }

/// The pill's box, DERIVED from what is actually on the element.
function pillRect(el: HTMLElement): DOMRect {
  const placed = el.style.left !== "" && el.style.top !== ""
  return rect({
    left: placed ? Number.parseFloat(el.style.left) : PILL_UNPLACED.left,
    top: placed ? Number.parseFloat(el.style.top) : PILL_UNPLACED.top,
    width: pillBox.width,
    height: pillBox.height,
  })
}

function stubRects() {
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const id = this instanceof HTMLElement ? this.dataset.testid : undefined
    if (id === "theater-pill") return pillRect(this as HTMLElement)
    if (id === "flap-dock") return rect(flapBox)
    return rect(surfaceBox)
  }
}

beforeEach(() => {
  installBootStubs()
  exitTheaterMock.mockReset()
  selectTabMock.mockReset()
  notifyInfoMock.mockReset()
  surfaceBox = { left: 0, top: 0, width: 800, height: 600 }
  pillBox = PILL_BOX
  flapBox = { left: 0, top: 0, width: 200, height: 48 }
  stubRects()
  mockState = { bootstrap: null } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  resetPaneInputGroups()
  resetAttachCapabilities()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
})

const agentTarget = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }
const terminalTarget = {
  kind: "terminal" as const,
  terminalId: "tm1",
  owner: { kind: "standalone" as const },
}

describe("the app menu the pill carries", () => {
  // THEATER TAKES EVERY OTHER ANCHOR AWAY. On a computer the mode unmounts the
  // sidebar (the launcher corner's `⋯` with it) and the header (the cog with
  // it), and on a phone the top bar; without this trigger Preferences, New
  // agent and every other global action are unreachable for the duration,
  // against the "exactly one surface-scoped `⋯` on screen" rule.
  const settle = () => new Promise((r) => setTimeout(r, 40))

  async function openMenu() {
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    await screen.findByRole("menu")
    await settle()
  }

  it("carries the trigger even in its collapsed form", () => {
    // A terminal pane has no tab strip, so the pill is grip, macros, `⋯` and
    // the way out. The app menu is not one of the parts that folds away.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(screen.getByRole("button", { name: "Settings" })).toBeTruthy()
  })

  it("opens the same menu the header's cog opens", async () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    // Driven from the model, never a hand-written list: the pill renders the
    // shared body, so a new app-menu entry arrives here with no change of ours.
    const topLevel = appMenuModel({
      ghAvailable: false,
      githubIntegrationEnabled: false,
    }).filter((e) => e.kind !== "separator")
    const rendered = screen.getAllByRole("menuitem").map((e) => e.textContent)
    for (const entry of topLevel) {
      if (entry.kind === "separator") continue
      expect(rendered.some((t) => t?.includes(entry.title))).toBe(true)
    }
  })

  it("keeps the way out inside the one menu as well", async () => {
    // The tenet's own reason: whatever else the mode hides, the way back is
    // inside the `⋯` a user can always find.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()
    const item = screen
      .getAllByRole("menuitem")
      .find((e) => e.textContent?.includes("Leave theater mode"))
    expect(item).toBeTruthy()
    fireEvent.click(item!)
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
  })
})

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

  it("carries no tab status at all, on any shape of agent", () => {
    // The expander came out: the agents list is where tab status lives, and a
    // second, smaller copy of it floating over the terminal was a place for
    // the two to disagree. Attention arrives as a toast, which reaches the
    // user whatever surface they are looking at.
    render(
      <TheaterPill
        target={agentTarget}
        session={session([
          tab({ id: "s1" }),
          tab({ id: "t2", provider: "codex", working: true, needs_attention: true }),
        ])}
      />,
    )
    expect(screen.queryByRole("button", { name: /other tab/i })).toBeNull()
    expect(screen.queryAllByRole("tab").length).toBe(0)
    expect(screen.queryByLabelText("Needs attention")).toBeNull()
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
    surfaceBox = { left: 0, top: 0, width: 390, height: 400 }
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
    surfaceBox = { left: 0, top: 0, width: 390, height: 400 }
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
    // The pill's own box really does change width while it is on screen (the
    // grip slot opens and closes across the phone's detach and return), so
    // treating that as an interruption would end a drag for a reason that has
    // nothing to do with the pointer.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(500, 400)
    pillBox = { left: 0, top: 0, width: 180, height: 48 }
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
    surfaceBox = { left: 0, top: 0, width: 390, height: 400 }
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

  it("gives the corner back when a wider pill that shoved it narrows again", () => {
    // The pill's own box really does change width while it is on screen: the
    // grip slot opens across the detach flight and closes across the return.
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    expect(pill().style.left).toBe(CORNER.left)

    pillBox = expandedPill
    act(() => playPillResize())
    // Still the corner, for a wider pill: an unplaced pill keeps its margin.
    expect(pill().style.left).toBe("366px")

    pillBox = PILL_BOX
    act(() => playPillResize())
    expect(pill().style.left).toBe(CORNER.left)
  })

  it("gives a dragged position back after the same shove", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    down(grip(), { pointerType: "mouse", clientX: 600, clientY: 550 })
    move(900, 550)
    up(900, 550)
    expect(pill().style.left).toBe("600px")

    pillBox = expandedPill
    act(() => playPillResize())
    expect(pill().style.left).toBe("380px")

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

    surfaceBox = { left: 0, top: 0, width: 390, height: 400 }
    act(() => playResize())
    expect(pill().style.left).toBe("190px")
    expect(stored()).toEqual({ x: 500, y: 400 })

    surfaceBox = { left: 0, top: 0, width: 800, height: 600 }
    act(() => playResize())
    expect(pill().style.left).toBe("500px")
  })

  it("writes nothing at all when nobody has ever moved it", () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    surfaceBox = { left: 0, top: 0, width: 390, height: 400 }
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

// ONE `⋯` IN THEATER, and it is this one. On a computer the mode takes the
// whole window, so the pane renders no input row to anchor its own ellipsis and
// publishes the items for the pill instead. Without this the typing-surface
// switch and "Attach a file…" would be unreachable for the duration of a mode
// whose entire point is staying in it.
describe("the input items the pill folds in", () => {
  const settle = () => new Promise((r) => setTimeout(r, 40))

  async function openMenu() {
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    await screen.findByRole("menu")
    await settle()
  }

  function items(): (string | null)[] {
    return screen.getAllByRole("menuitem").map((e) => e.textContent)
  }

  it("shows the switch and the attach the pane published", async () => {
    const attach = vi.fn()
    registerAttachCapability("tm1", attach)
    registerPaneInputGroup("tm1", { surfaceSwitch: true, keysToggle: false })
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    expect(items().some((t) => t?.includes("Use virtual input"))).toBe(true)
    const attachItem = screen
      .getAllByRole("menuitem")
      .find((e) => e.textContent?.includes("Attach a file"))
    expect(attachItem).toBeTruthy()
    fireEvent.click(attachItem!)
    expect(attach).toHaveBeenCalledTimes(1)
  })

  // THE WAY BACK ONLY. The other direction lives in the bottom `⋯`, which
  // exists exactly while the virtual input does, so the pane publishes the
  // switch only once its own rows are gone and this menu never has to guess
  // which surface is live.
  it("offers the way back rather than both directions", async () => {
    registerPaneInputGroup("tm1", { surfaceSwitch: true, keysToggle: false })
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    expect(items().some((t) => t?.includes("Use virtual input"))).toBe(true)
    expect(
      items().some((t) => t?.includes("Type directly in the terminal")),
    ).toBe(false)
  })

  // An attach no mounted owner pane can perform is not an item: it would open
  // nothing. The act and the item are the same registration, so this is the
  // whole gate.
  it("drops the attach when no mounted pane can perform it", async () => {
    registerPaneInputGroup("tm1", { surfaceSwitch: false, keysToggle: false })
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    expect(items().some((t) => t?.includes("Attach a file"))).toBe(false)
  })

  // Nothing published and no attach capability: no group at all, label
  // included, rather than an empty heading over the app menu.
  it("carries no input group while the pane publishes nothing", async () => {
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    expect(items().some((t) => t?.includes("Use virtual input"))).toBe(false)
    expect(items().some((t) => t?.includes("Attach a file"))).toBe(false)
    expect(screen.queryByText("Input")).toBeNull()
    expect(items().some((t) => t?.includes("Leave theater mode"))).toBe(true)
  })

  // It reads the pane it is painted over, never whichever registered last.
  it("ignores a group published by another pane", async () => {
    registerPaneInputGroup("someone-else", {
      surfaceSwitch: true,
      keysToggle: false,
    })
    render(<TheaterPill target={terminalTarget} session={undefined} />)
    await openMenu()

    expect(items().some((t) => t?.includes("Use virtual input"))).toBe(false)
  })
})

// THE FLIGHT'S ARITHMETIC, ACTUALLY RUN.
//
// Everything above pins what the pill IS at each stage. This block runs the
// imperative half: with a measured dock and a measured pill, the choreography
// really writes the transform, the radius, the pinned coordinates and the
// parking, so the numbers are checked rather than assumed. jsdom lays nothing
// out, which is exactly why every box here is stubbed: what is under test is
// the arithmetic, not the browser.
describe("the phone flight, with real boxes to measure", () => {
  const mobileTarget = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }

  /// The docked flap, as the flight sees it: an element that answers with a
  /// rect. It is registered rather than rendered, because the real one is a
  /// sibling of the pane and reaches the pill the same way.
  function mountDock(fill = "var(--dux-flap-bg)") {
    const el = document.createElement("div")
    el.dataset.testid = "flap-dock"
    // The flap publishes its own body colour, which is the band's: the strip's
    // composited tone, or the plain background with no strip on screen.
    el.style.setProperty(FLAP_FILL_VAR, fill)
    document.body.appendChild(el)
    const retire = registerFlapElement(el)
    return () => {
      retire()
      el.remove()
    }
  }

  function flying(flight: FlightPhase) {
    return (
      <TheaterPill
        target={mobileTarget}
        session={session([tab({ id: "s1" })])}
        variant="mobile"
        flight={flight}
      />
    )
  }

  let retireDock = () => {}
  beforeEach(() => {
    mockState = {
      bootstrap: null,
      theater: true,
      changes: { sessionId: "s1", phase: "loaded", staged: [], unstaged: [] },
    } as unknown as DuxState
    // The capsule's radius is HALF THE PAINTED HEIGHT, so the morph needs a
    // height to halve; jsdom reports zero for every box it never laid out.
    Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
      configurable: true,
      get: () => 48,
    })
    retireDock = mountDock()
  })

  afterEach(() => {
    retireDock()
    // @ts-expect-error restoring jsdom's own zero-size getter
    delete HTMLElement.prototype.offsetHeight
  })

  it("measures the slot at the width the class actually draws", () => {
    // The slot is arithmetic in one place and a utility class in another, and
    // nothing else makes the two agree: a grip drawn wider than the resting
    // corner allows for hangs the pill's edge outside the surface again.
    render(flying("floating"))
    const cls = screen.getByTestId("theater-pill-grip").className
    expect(cls).toContain(`w-[${THEATER_PILL_GRIP_W_PX}px]`)
    expect(pill().className).toContain(`gap-${THEATER_PILL_ROW_GAP_PX / 4}`)
    expect(THEATER_PILL_GRIP_SLOT_PX).toBe(
      THEATER_PILL_GRIP_W_PX + THEATER_PILL_ROW_GAP_PX,
    )
  })

  it("leaves room for the grip slot the detach is about to open", () => {
    // The pill starts every detach gripless, and a resting corner derived from
    // that narrower box hangs its right edge outside the surface the moment the
    // slot opens. 800 - (200 + 20) - 14.
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    render(flying("detaching"))
    expect(pill().style.left).toBe("566px")
  })

  it("pulls off wearing the flap's shape and lands as a capsule", () => {
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    render(flying("detaching"))
    const box = pill()
    // The end values of the morph: a capsule at half the painted height, on the
    // travel's own clock, with its own layer while it moves.
    expect(box.style.borderRadius).toBe("24px")
    expect(box.style.transform).toBe("")
    expect(box.style.transition).toContain("transform 320ms")
    expect(box.style.transition).toContain("border-radius 200ms")
    expect(box.style.willChange).toBe("transform")
  })

  it("flies home from where it sits to where the dock is", () => {
    pillBox = { left: 566, top: 538, width: 220, height: 48 }
    flapBox = { left: 200, top: 4, width: 220, height: 48 }
    render(flying("returning"))
    const box = pill()
    // Pinned on the coordinates it is LEAVING, then translated onto the dock's.
    expect(box.style.left).toBe("566px")
    expect(box.style.top).toBe("538px")
    expect(box.style.transform).toBe("translate(-366px, -534px)")
    // Both axes are said, so the fallback corner class cannot fight it.
    expect(box.style.right).toBe("auto")
    expect(box.style.bottom).toBe("auto")
  })

  it("leaves from where the user last saw it, through the real sequence", () => {
    // THE WHOLE EXIT, in order, for a pill that is not in its default corner.
    // The commit that starts the flight is also the commit React stops writing
    // the pill's inline coordinates in, so anything the flight measures off the
    // element by then is the static-layout corner rather than the pill: it flew
    // home from a place the user never saw it in. The state is the truth.
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":120,"y":300}')
    flapBox = { left: 200, top: 4, width: 200, height: 48 }
    const view = render(flying("floating"))
    expect(pill().style.left).toBe("120px")
    expect(pill().style.top).toBe("300px")

    // The top chrome coming back shrinks the surface under the resting pill.
    // This pill still fits, so it does not move, and the flight leaves from the
    // coordinates it has been sitting on all along.
    act(() => view.rerender(flying("expanding")))
    surfaceBox = { left: 0, top: 0, width: 800, height: 380 }
    act(() => playResize())
    expect(pill().style.top).toBe("300px")

    act(() => view.rerender(flying("returning")))
    const box = pill()
    expect(box.style.left).toBe("120px")
    expect(box.style.top).toBe("300px")
    // From THERE to the flap: (200 - 120, 4 - 300).
    expect(box.style.transform).toBe("translate(80px, -296px)")
  })

  it("snaps a pill the returning chrome would push off screen back inside", () => {
    // THE CHROME COMES BACK BEFORE THE FLIGHT DOES, and it takes the surface's
    // height with it. A bottom-hugging pill that is not re-clamped for the
    // shorter surface spends the whole chrome stage hanging below it, out of
    // sight, and the flight then enters from a place the user never saw it in.
    // So it moves, and it moves instantly: the settle is what made re-clamping
    // here look wrong, not the clamp.
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":120,"y":520}')
    flapBox = { left: 200, top: 4, width: 200, height: 48 }
    const view = render(flying("floating"))
    expect(pill().style.top).toBe("520px")

    act(() => view.rerender(flying("expanding")))
    surfaceBox = { left: 0, top: 0, width: 800, height: 380 }
    act(() => playResize())
    // Inside the shrunken surface: 380 - 48.
    expect(pill().style.top).toBe("332px")
    expect(pill().className).not.toContain("transition-[left,top]")

    // And the flight leaves from the clamped spot, which is where it is painted.
    act(() => view.rerender(flying("returning")))
    const box = pill()
    expect(box.style.top).toBe("332px")
    expect(box.style.transform).toBe("translate(80px, -328px)")
  })

  it("flies home in the surface's own coordinate space, not the viewport's", () => {
    // The pill's coordinates are written in its offset parent's space and the
    // dock is measured in the viewport's, so the translation is only right if
    // the parent's own origin is added back. A surface at the origin cannot tell
    // that apart from subtracting it, or from ignoring it altogether.
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":120,"y":300}')
    surfaceBox = { left: 50, top: 30, width: 800, height: 600 }
    flapBox = { left: 200, top: 4, width: 200, height: 48 }
    render(flying("returning"))
    const box = pill()
    expect(box.style.left).toBe("120px")
    expect(box.style.top).toBe("300px")
    // Viewport-wise the pill is at (50 + 120, 30 + 300), and the dock is at
    // (200, 4): translate(200 - 170, 4 - 330).
    expect(box.style.transform).toBe("translate(30px, -326px)")
  })

  it("keeps clamping a pill whose exit was abandoned mid-chrome", () => {
    // A mode flipped back during the chrome stage rests at `floating` with no
    // flight at all. Nothing special is owed to that pill: it was clamped on the
    // way through and it goes on being clamped, with the settle back.
    localStorage.setItem(THEATER_PILL_POSITION_KEY, '{"x":120,"y":520}')
    const view = render(flying("floating"))
    act(() => view.rerender(flying("expanding")))
    surfaceBox = { left: 0, top: 0, width: 800, height: 380 }
    act(() => playResize())
    expect(pill().style.top).toBe("332px")

    act(() => view.rerender(flying("floating")))
    expect(pill().style.top).toBe("332px")
    expect(pill().className).toContain("transition-[left,top]")

    // Still live, and still re-derived from the position the user asked for
    // rather than from the place the last clamp pushed it to: 200 - 48.
    surfaceBox = { left: 0, top: 0, width: 800, height: 200 }
    act(() => playResize())
    expect(pill().style.top).toBe("152px")
  })

  it("parks on the dock's real coordinates before it squares up", () => {
    pillBox = { left: 566, top: 538, width: 220, height: 48 }
    flapBox = { left: 200, top: 4, width: 220, height: 48 }
    render(flying("attaching"))
    const box = pill()
    expect(box.style.left).toBe("200px")
    expect(box.style.top).toBe("4px")
    // The transform is cleared and the compositor layer dropped, so the raster
    // re-snaps to the device pixel grid the in-flow flap will paint on.
    expect(box.style.transform).toBe("")
    expect(box.style.willChange).toBe("auto")
    expect(box.style.borderRadius).toBe("0 0 10px 10px")
    expect(box.style.borderTopColor).toBe("transparent")
  })

  it("keeps its own coordinates once the flight lets go of the box", () => {
    // The settled pill is placed by React, from its own position state, and the
    // stage that ends clears what the FLIGHT wrote. It must not take those with
    // it: React's next diff sees values that never changed and rewrites
    // nothing, so a blanket clear strands the pill at the overlay's corner with
    // no inset at all.
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    const view = render(flying("detaching"))
    act(() => view.rerender(flying("floating")))
    const box = pill()
    expect(box.style.left).toBe("566px")
    expect(box.style.top).toBe(CORNER.top)
    expect(box.style.transform).toBe("")
    expect(box.style.borderRadius).toBe("")
  })

  it("hands the box back mid-air when the mode flips under the flight", () => {
    pillBox = { left: 566, top: 538, width: 220, height: 48 }
    flapBox = { left: 200, top: 4, width: 220, height: 48 }
    const view = render(flying("returning"))
    expect(pill().style.transform).not.toBe("")
    act(() => view.rerender(flying("floating")))
    const box = pill()
    expect(box.style.transform).toBe("")
    expect(box.style.right).toBe("")
    expect(box.style.bottom).toBe("")
    // Its own resting corner for THIS box: 800 - 220 - 14.
    expect(box.style.left).toBe("566px")
  })

  it("falls back to the corner while a fresh pill has nothing to place it", () => {
    // A pane that remounts mid-return mounts a pill with no coordinates of its
    // own, and the flight has not written any yet. Without the fallback that is
    // an absolutely positioned box with no insets at all: the overlay's top-left
    // corner, in front of the terminal.
    surfaceBox = { left: 0, top: 0, width: 0, height: 0 }
    render(flying("returning"))
    expect(pill().className).toContain("right-3.5")
    expect(pill().className).toContain("bottom-3.5")
  })

  it("re-runs the flight for a pill that mounts in the middle of one", () => {
    pillBox = { left: 566, top: 538, width: 220, height: 48 }
    flapBox = { left: 200, top: 4, width: 220, height: 48 }
    const view = render(flying("returning"))
    // An agent switch mid-flight remounts the pane, and the overlay with it.
    act(() =>
      view.rerender(
        <TheaterPill
          key="remounted"
          target={mobileTarget}
          session={session([tab({ id: "s1" })])}
          variant="mobile"
          flight="returning"
        />,
      ),
    )
    expect(pill().style.left).toBe("566px")
    expect(pill().style.transform).toBe("translate(-366px, -534px)")
  })

  it("flies in the colour of the band it is leaving, not the strip's", () => {
    // A single-tab agent, or anyone who hid the top bar, has a flap hanging off
    // the plain background. A flight pinned to the tab strip's tone popped the
    // wrong colour for a frame at both ends of the journey.
    retireDock()
    retireDock = mountDock("var(--background)")
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    const view = render(flying("detaching"))
    expect(pill().style.getPropertyValue(FLAP_FILL_VAR)).toBe(
      "var(--background)",
    )
    // And the same colour on the way back in, read off the dock it is landing
    // on rather than remembered from the way out.
    act(() => view.rerender(flying("attaching")))
    expect(pill().style.getPropertyValue(FLAP_FILL_VAR)).toBe(
      "var(--background)",
    )
    expect(pill().style.backgroundColor).toBe(`var(${FLAP_FILL_VAR})`)
  })

  it("gives the colour back with everything else the flight wrote", () => {
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    const view = render(flying("detaching"))
    act(() => view.rerender(flying("floating")))
    expect(pill().style.getPropertyValue(FLAP_FILL_VAR)).toBe("")
  })

  it("lets go of a flight whose pill is unmounted under it", () => {
    flapBox = { left: 300, top: 0, width: 200, height: 48 }
    const view = render(flying("detaching"))
    expect(() => view.unmount()).not.toThrow()
  })

  it("flies nothing at all when there is no dock to measure", () => {
    // The reduced-motion answer, and the answer for a page that opened in
    // theater: with no flap on screen the cluster simply appears.
    retireDock()
    retireDock = () => {}
    render(flying("detaching"))
    expect(pill().style.transform).toBe("")
    expect(pill().style.transition).toBe("")
  })
})
