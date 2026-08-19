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

function setup(opts: { owner?: boolean; wire?: boolean } = {}) {
  const term = new TermFake()
  const fit = new FitFake(term)
  const sent: { rows: number; cols: number }[] = []
  let owner = opts.owner ?? true
  let wire = opts.wire ?? true
  const coord = createResizeCoordinator({
    term: term as unknown as Terminal,
    fit: fit as unknown as FitAddon,
    sendResize: (rows, cols) => {
      if (!wire) return false
      sent.push({ rows, cols })
      return true
    },
    isOwner: () => owner,
  })
  return {
    term,
    fit,
    sent,
    coord,
    setOwner: (v: boolean) => {
      owner = v
    },
    setWire: (v: boolean) => {
      wire = v
    },
  }
}

let observed: Element[] = []
class ResizeObserverStub {
  constructor(private cb: () => void) {}
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
