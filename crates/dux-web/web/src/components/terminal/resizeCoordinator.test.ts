// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Terminal } from "@xterm/xterm"
import type { FitAddon } from "@xterm/addon-fit"

import { createResizeCoordinator } from "./resizeCoordinator"

// The coordinator touches exactly four things: xterm's geometry and its
// `onResize`/`write`, the fit addon, the socket's `sendResize` answer, and the
// ownership verdict. All four are stubbed here, so these tests pin the MACHINE
// (the hold, the dedupe, the plan) without a pane, a socket, or a canvas.
class TermFake {
  rows = 24
  cols = 80
  resizeListeners: (() => void)[] = []
  writes: (() => void)[] = []
  onResize(cb: () => void) {
    this.resizeListeners.push(cb)
    return {
      dispose: () => {
        this.resizeListeners = this.resizeListeners.filter((l) => l !== cb)
      },
    }
  }
  /// A drained write: the callback fires immediately, exactly as the stubbed
  /// terminal in the pane's own suites does.
  write(_data: unknown, cb?: () => void) {
    if (cb) cb()
  }
  /// How many times `resize` was called, so idempotence is checkable.
  resizes = 0
  /// xterm's own signature is resize(columns, rows).
  resize(cols: number, rows: number) {
    this.resizes++
    if (this.cols === cols && this.rows === rows) return
    this.regrid(rows, cols)
  }
  regrid(rows: number, cols: number) {
    this.rows = rows
    this.cols = cols
    for (const cb of [...this.resizeListeners]) cb()
  }
}

class FitFake {
  fits = 0
  /// Arm the NEXT fit to re-grid the terminal, the way a real fit does once the
  /// cell metrics or the container move under it.
  next: { rows: number; cols: number } | null = null
  constructor(private term: TermFake) {}
  fit() {
    this.fits++
    const next = this.next
    if (next) {
      this.next = null
      this.term.regrid(next.rows, next.cols)
    }
  }
}

/// Take the first-frame resize out of the way for the tests that are about
/// something else: land it on the RECONNECT plan (one plain resize, no jiggle)
/// and forget what it sent. Without this the 250ms no-first-frame fallback
/// fires inside every `advanceTimersByTime` and its jiggle shows up in `sent`.
function settleFirstFrame(
  coord: ReturnType<typeof createResizeCoordinator>,
  sent: unknown[],
) {
  coord.noteOpen(false)
  coord.firstFrameLanded()
  sent.length = 0
}

function setup(
  opts: { owner?: boolean; wire?: boolean; faithful?: boolean } = {},
) {
  const term = new TermFake()
  const fit = new FitFake(term)
  const sent: { rows: number; cols: number }[] = []
  const relayouts: number[] = []
  let owner = opts.owner ?? true
  let wire = opts.wire ?? true
  // The `ui.watcher_view` preference. Faithful by default, exactly as the
  // config is; VIEWER mode is that AND not being the owner, which is how the
  // pane wires it.
  let faithful = opts.faithful ?? true
  const coord = createResizeCoordinator({
    term: term as unknown as Terminal,
    fit: fit as unknown as FitAddon,
    sendResize: (rows, cols) => {
      if (!wire) return false
      sent.push({ rows, cols })
      return true
    },
    isOwner: () => owner,
    viewerMode: () => !owner && faithful,
    onViewerLayout: () => relayouts.push(1),
  })
  return {
    term,
    fit,
    sent,
    coord,
    relayouts,
    setOwner: (v: boolean) => {
      owner = v
    },
    setFaithful: (v: boolean) => {
      faithful = v
    },
    setWire: (v: boolean) => {
      wire = v
    },
  }
}

let observed: Element[] = []
let observers: ResizeObserverStub[] = []
class ResizeObserverStub {
  constructor(private cb: () => void) {
    observers.push(this)
  }
  observe(el: Element) {
    observed.push(el)
  }
  disconnect() {}
  fire() {
    this.cb()
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  observed = []
  observers = []
  vi.stubGlobal("ResizeObserver", ResizeObserverStub)
  vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
    return setTimeout(cb, 0) as unknown as number
  })
  vi.stubGlobal("cancelAnimationFrame", (h: number) => clearTimeout(h))
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("the resize coordinator's record of what the PTY was told", () => {
  it("does not record a resize the OWNER GATE dropped, so it is re-sent once ownership returns", () => {
    const { term, fit, coord, sent, setOwner } = setup({ owner: false })
    coord.start(document.createElement("div"))
    fit.next = { rows: 30, cols: 100 }
    fit.fit()
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([])
    setOwner(true)
    // The same size again: it only reaches the wire because the drop was never
    // booked as a send.
    term.regrid(30, 100)
    term.regrid(30, 101)
    vi.advanceTimersByTime(500)
    expect(sent.at(-1)).toEqual({ rows: 30, cols: 101 })
  })

  it("does not record a resize the SOCKET dropped, so it is re-sent once it reopens", () => {
    const { term, coord, sent, setWire } = setup({ wire: false })
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    term.regrid(30, 100)
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([])
    setWire(true)
    // A LATER debounce of the very same geometry still sends: nothing was
    // booked, so the dedupe has nothing to suppress against.
    term.regrid(24, 80)
    term.regrid(30, 100)
    vi.advanceTimersByTime(500)
    expect(sent.at(-1)).toEqual({ rows: 30, cols: 100 })
  })

  it("sends nothing when the geometry has not moved since the last send", () => {
    const { term, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    term.regrid(30, 100)
    vi.advanceTimersByTime(500)
    expect(sent).toHaveLength(1)
    // xterm only fires onResize on a real change, but the debounce can be armed
    // by the observer too; the dedupe is what keeps a no-op fit off the wire.
    coord.setHolding(false)
    vi.advanceTimersByTime(500)
    expect(sent).toHaveLength(1)
  })
})

describe("the resize coordinator's gesture hold", () => {
  it("performs NO local refit while the gesture holds the pair, then exactly one at the lift", () => {
    const { fit, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const before = fit.fits
    coord.setHolding(true)
    coord.directSend(() => {})
    coord.directSend(() => {})
    expect(fit.fits).toBe(before)
    coord.setHolding(false)
    coord.flushHeld()
    expect(fit.fits).toBe(before + 1)
  })

  it("keeps the FIRST held direct send and drops later ones", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const ran: string[] = []
    coord.setHolding(true)
    coord.directSend(() => ran.push("jiggle"))
    coord.directSend(() => ran.push("plain"))
    coord.setHolding(false)
    coord.flushHeld()
    expect(ran).toEqual(["jiggle"])
  })

  it("defers the DEBOUNCED send until the lift, then sends exactly once", () => {
    const { term, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    coord.setHolding(true)
    term.regrid(30, 100)
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([])
    coord.setHolding(false)
    coord.flushHeld()
    // Re-armed through the same debounce, so it is a settle window like any
    // other rather than an immediate send.
    expect(sent).toEqual([])
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([{ rows: 30, cols: 100 }])
  })

  it("sends nothing at a lift that held nothing", () => {
    const { fit, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const before = fit.fits
    coord.setHolding(true)
    coord.setHolding(false)
    coord.flushHeld()
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([])
    expect(fit.fits).toBe(before)
  })
})

describe("the resize coordinator's debounce hold", () => {
  it("performs NO refit during an observer burst, then exactly one fit+send at the settle", () => {
    // A divider drag. The refit used to run per animation frame while the send
    // waited out the debounce, so the local grid ran ahead of the child's for
    // the whole drag and the child's repaints duplicated rows into scrollback.
    const { fit, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const ro = observers.at(-1)
    if (!ro) throw new Error("the coordinator never constructed a ResizeObserver")
    const before = fit.fits

    for (let i = 0; i < 10; i++) {
      ro.fire()
      vi.advanceTimersByTime(16)
    }
    expect(fit.fits).toBe(before)
    expect(sent).toEqual([])

    // Coalesced, last geometry wins: the one released fit reads the container
    // as the drag left it.
    fit.next = { rows: 30, cols: 100 }
    vi.advanceTimersByTime(500)
    expect(fit.fits).toBe(before + 1)
    expect(sent).toEqual([{ rows: 30, cols: 100 }])
  })

  it("hands a parked fit to a gesture that starts inside the window, and the lift releases the pair", () => {
    const { fit, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const ro = observers.at(-1)
    if (!ro) throw new Error("the coordinator never constructed a ResizeObserver")
    const before = fit.fits

    ro.fire()
    vi.advanceTimersByTime(16)
    expect(fit.fits).toBe(before)

    // The finger lands before the debounce settles: the settle must defer to
    // the gesture rather than let the fit escape on its own.
    coord.setHolding(true)
    fit.next = { rows: 30, cols: 100 }
    vi.advanceTimersByTime(500)
    expect(fit.fits).toBe(before)
    expect(sent).toEqual([])

    coord.setHolding(false)
    coord.flushHeld()
    expect(fit.fits).toBe(before + 1)
    // The send re-arms through the ordinary debounce, as the gesture hold has
    // always done.
    expect(sent).toEqual([])
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([{ rows: 30, cols: 100 }])
  })

  it("does not fit twice when a direct send lands on a parked observer refit", () => {
    const { fit, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const ro = observers.at(-1)
    if (!ro) throw new Error("the coordinator never constructed a ResizeObserver")
    const before = fit.fits

    ro.fire()
    vi.advanceTimersByTime(16)
    expect(fit.fits).toBe(before)

    // A direct send fits for itself, which satisfies the parked refit.
    coord.directSend(() => {})
    expect(fit.fits).toBe(before + 1)
    vi.advanceTimersByTime(500)
    expect(fit.fits).toBe(before + 1)
  })
})

describe("the resize coordinator's first-frame plan", () => {
  it("jiggles the width down one column and back on the very first open", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    coord.noteOpen(true)
    coord.firstFrameLanded()
    expect(sent).toEqual([{ rows: 24, cols: 79 }])
    vi.advanceTimersByTime(60)
    expect(sent).toEqual([
      { rows: 24, cols: 79 },
      { rows: 24, cols: 80 },
    ])
  })

  it("sends a single plain resize on a RECONNECT and never jiggles", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    coord.noteOpen(false)
    coord.firstFrameLanded()
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([{ rows: 24, cols: 80 }])
  })

  it("is idempotent per open: a second first-frame landing does nothing", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    coord.noteOpen(false)
    coord.firstFrameLanded()
    coord.firstFrameLanded()
    expect(sent).toHaveLength(1)
  })

  it("fires the fallback for a session that emits no first frame at all", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    expect(coord.needsFirstFrameResize()).toBe(true)
    vi.advanceTimersByTime(250)
    expect(sent).toHaveLength(1)
    expect(coord.needsFirstFrameResize()).toBe(false)
  })

  it("takes the gesture hold for the jiggle's own 60ms continuation", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    coord.noteOpen(true)
    coord.firstFrameLanded()
    expect(sent).toEqual([{ rows: 24, cols: 79 }])
    // A gesture that starts INSIDE the jiggle window must not catch the
    // continuation's SIGWINCH mid-stream.
    coord.setHolding(true)
    vi.advanceTimersByTime(60)
    expect(sent).toHaveLength(1)
    coord.setHolding(false)
    coord.flushHeld()
    expect(sent).toEqual([
      { rows: 24, cols: 79 },
      { rows: 24, cols: 80 },
    ])
  })
})

describe("the resize coordinator's foreground re-assert", () => {
  it("re-sends an UNCHANGED size, bypassing the dedupe", () => {
    const { term, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    term.regrid(30, 100)
    vi.advanceTimersByTime(500)
    expect(sent).toHaveLength(1)
    coord.resyncToForeground()
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([
      { rows: 30, cols: 100 },
      { rows: 30, cols: 100 },
    ])
  })

  it("defers to the lift when it lands mid-gesture", () => {
    const { coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    coord.setHolding(true)
    coord.resyncToForeground()
    vi.advanceTimersByTime(500)
    expect(sent).toEqual([])
    coord.setHolding(false)
    coord.flushHeld()
    expect(sent).toEqual([{ rows: 24, cols: 80 }])
  })
})

describe("the resize coordinator's teardown", () => {
  it("observes the container it was started on", () => {
    const { coord } = setup()
    const container = document.createElement("div")
    coord.start(container)
    expect(observed).toEqual([container])
  })

  it("drops every timer and subscription it armed", () => {
    const { term, coord, sent } = setup()
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    term.regrid(30, 100)
    coord.resyncToForeground()
    coord.dispose()
    vi.advanceTimersByTime(1000)
    expect(sent).toEqual([])
    expect(term.resizeListeners).toEqual([])
  })
})

// VIEWER MODE: the whole point of the faithful watcher view. One pty has one
// grid, the owner's, and a watcher rendering the same bytes at a different one
// is rendering wrapped, clamped output into a scrollback nothing cleans up. So
// a watcher stops deciding its own geometry: it never fits to its container,
// never sends, and re-grids to whatever the wire says the pty is.
describe("the resize coordinator in VIEWER mode", () => {
  /// A watcher with the faithful preference: not the owner, faithful on.
  const watcher = () => setup({ owner: false, faithful: true })

  it("never fits to its container, and recomputes the shrink instead", () => {
    const { fit, coord, relayouts } = watcher()
    coord.start(document.createElement("div"))
    expect(fit.fits).toBe(0)
    observers[0].fire()
    vi.advanceTimersByTime(500)
    expect(fit.fits).toBe(0)
    // The layout signal is not dropped: it is answered by the pane's font
    // shrink, which is what the container's size decides in this mode.
    expect(relayouts.length).toBeGreaterThan(0)
  })

  it("never sends, on any path", () => {
    const { coord, sent, term } = watcher()
    coord.start(document.createElement("div"))
    coord.noteOpen(true)
    coord.firstFrameLanded()
    coord.resyncToForeground()
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    term.regrid(40, 120)
    vi.advanceTimersByTime(1000)
    expect(sent).toEqual([])
  })

  it("adopts the grid the wire reports, on the handshake seed and on a change", () => {
    const { coord, term } = watcher()
    coord.start(document.createElement("div"))
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 40, cols: 120 })
    coord.noteRemoteGrid({ rows: 50, cols: 132 })
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 50, cols: 132 })
  })

  it("is idempotent: re-adopting the same grid re-grids nothing", () => {
    const { coord, term } = watcher()
    coord.start(document.createElement("div"))
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    const resizes = term.resizes
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    coord.applyViewerGrid()
    expect(term.resizes).toBe(resizes)
  })

  it("reads a null grid as 'nothing known', never as agreement", () => {
    const { coord, term } = watcher()
    coord.start(document.createElement("div"))
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    coord.noteRemoteGrid(null)
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 40, cols: 120 })
  })

  it("adopts on DEMOTION, from the grid it recorded while it was the owner", () => {
    const { coord, term, setOwner } = setup({ owner: true })
    coord.start(document.createElement("div"))
    // The owner is told its own applied grid too, and records it without
    // adopting anything.
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    expect(term.cols).toBe(80)
    setOwner(false)
    coord.applyViewerGrid()
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 40, cols: 120 })
  })

  it("returns to fitting and sending on PROMOTION, through the existing path", () => {
    // A take-over bounces the socket; the new connection's first frame is what
    // fits and claims. Nothing new was added for promotion, so this pins that
    // the existing path still produces exactly one fit and one send.
    const { coord, fit, sent, term, setOwner } = watcher()
    coord.start(document.createElement("div"))
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    expect(fit.fits).toBe(0)
    setOwner(true)
    coord.noteOpen(false)
    coord.firstFrameLanded()
    expect(fit.fits).toBe(1)
    expect(sent).toEqual([{ rows: term.rows, cols: term.cols }])
  })

  it("does none of it in the LEGACY fit-my-window view", () => {
    // The preference is the whole difference: a watcher who asked to fit their
    // own window still fits it, still adopts nothing, and still sends nothing
    // (the owner gate, unchanged).
    const { coord, fit, term, sent, relayouts } = setup({
      owner: false,
      faithful: false,
    })
    coord.start(document.createElement("div"))
    settleFirstFrame(coord, sent)
    const before = fit.fits
    observers[0].fire()
    vi.advanceTimersByTime(500)
    expect(fit.fits).toBeGreaterThan(before)
    expect(relayouts).toEqual([])
    coord.noteRemoteGrid({ rows: 40, cols: 120 })
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 24, cols: 80 })
    expect(sent).toEqual([])
  })
})
