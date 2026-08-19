// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  LONG_PRESS_MS,
  SCROLL_THRESHOLD_PX,
  createTouchGesture,
  type TouchGestureOutcome,
} from "./touchGesture"

// The machine is pure disambiguation: it decides scroll vs long press vs tap
// and emits. These tests drive it with real TouchEvents on a real element and
// record the events, so what is pinned is the DECISION and nothing else.
function touch(x: number, y: number): Touch {
  return { clientX: x, clientY: y, identifier: 0 } as Touch
}

function touchEvent(type: string, touches: Touch[], changed = touches) {
  const e = new Event(type, { bubbles: true, cancelable: true }) as TouchEvent & {
    touches: Touch[]
    changedTouches: Touch[]
  }
  e.touches = touches
  e.changedTouches = changed
  return e as unknown as TouchEvent
}

function setup(opts: { scrollAllowed?: () => boolean } = {}) {
  const log: string[] = []
  const lifts: TouchGestureOutcome[] = []
  let remainder = 0
  const container = document.createElement("div")
  const gesture = createTouchGesture({
    scrollAllowed: opts.scrollAllowed ?? (() => true),
    onGestureReset: () => log.push("reset"),
    onGestureFinished: () => log.push("finished"),
    onLongPress: () => log.push("longpress"),
    onSelectMove: () => log.push("selectmove"),
    onScrollStart: () => log.push("scrollstart"),
    onScrollMove: (accum) => {
      log.push(`scrollmove:${accum}`)
      return remainder
    },
    onLift: (outcome) => {
      lifts.push(outcome)
      log.push("lift")
    },
  })
  gesture.attach(container)
  return {
    log,
    lifts,
    gesture,
    container,
    setRemainder: (v: number) => {
      remainder = v
    },
    start: (x: number, y: number, extra: Touch[] = []) =>
      container.dispatchEvent(
        touchEvent("touchstart", [touch(x, y), ...extra]) as unknown as Event,
      ),
    move: (x: number, y: number) =>
      container.dispatchEvent(
        touchEvent("touchmove", [touch(x, y)]) as unknown as Event,
      ),
    end: () => {
      const e = touchEvent("touchend", [], [touch(0, 0)])
      container.dispatchEvent(e as unknown as Event)
      return e
    },
    cancel: () =>
      container.dispatchEvent(touchEvent("touchcancel", []) as unknown as Event),
  }
}

beforeEach(() => {
  vi.useFakeTimers()
})
afterEach(() => {
  vi.useRealTimers()
})

describe("a short still tap", () => {
  it("trips neither branch and lifts as a tap", () => {
    const { start, end, log, lifts } = setup()
    start(10, 10)
    vi.advanceTimersByTime(LONG_PRESS_MS - 1)
    end()
    expect(lifts).toEqual([{ wasTap: true, wasSelecting: false }])
    expect(log).toEqual(["reset", "reset", "finished", "lift"])
  })

  it("survives a move that stays under the scroll threshold", () => {
    const { start, move, end, lifts, log } = setup()
    start(10, 10)
    move(10, 10 + SCROLL_THRESHOLD_PX - 1)
    end()
    expect(lifts).toEqual([{ wasTap: true, wasSelecting: false }])
    expect(log).not.toContain("scrollstart")
  })
})

describe("a long press", () => {
  it("becomes a selection at the delay and every move extends it", () => {
    const { start, move, end, log, lifts } = setup()
    start(10, 10)
    vi.advanceTimersByTime(LONG_PRESS_MS)
    expect(log).toContain("longpress")
    move(10, 200)
    expect(log).toContain("selectmove")
    // Far past the scroll threshold, and still not a scroll.
    expect(log).not.toContain("scrollstart")
    end()
    expect(lifts).toEqual([{ wasTap: false, wasSelecting: true }])
  })

  it("cancels the page's own scrolling while it selects", () => {
    const { start, container } = setup()
    start(10, 10)
    vi.advanceTimersByTime(LONG_PRESS_MS)
    const e = touchEvent("touchmove", [touch(10, 200)])
    container.dispatchEvent(e as unknown as Event)
    expect(e.defaultPrevented).toBe(true)
  })
})

describe("a scroll", () => {
  it("wins the race against the long press and cancels its timer", () => {
    const { start, move, log } = setup()
    start(10, 10)
    move(10, 10 + SCROLL_THRESHOLD_PX + 1)
    vi.advanceTimersByTime(LONG_PRESS_MS * 2)
    expect(log).toContain("scrollstart")
    expect(log).not.toContain("longpress")
  })

  it("starts exactly once, however many moves follow", () => {
    const { start, move, log } = setup()
    start(10, 10)
    move(10, 30)
    move(10, 50)
    move(10, 70)
    expect(log.filter((l) => l === "scrollstart")).toHaveLength(1)
  })

  it("carries the accumulator forward and takes the client's remainder back", () => {
    const { start, move, log, setRemainder } = setup()
    setRemainder(3)
    start(10, 10)
    move(10, 30)
    move(10, 40)
    // 20px accumulated, then the client kept 3 of them, so the next 10px move
    // arrives as 13.
    expect(log.filter((l) => l.startsWith("scrollmove:"))).toEqual([
      "scrollmove:20",
      "scrollmove:13",
    ])
  })

  it("lifts as neither a tap nor a selection", () => {
    const { start, move, end, lifts } = setup()
    start(10, 10)
    move(10, 100)
    end()
    expect(lifts).toEqual([{ wasTap: false, wasSelecting: false }])
  })

  it("does nothing at all where scrolling is not allowed", () => {
    const { start, move, log } = setup({ scrollAllowed: () => false })
    start(10, 10)
    move(10, 200)
    expect(log).not.toContain("scrollstart")
    expect(log.some((l) => l.startsWith("scrollmove"))).toBe(false)
  })

  it("asks whether scrolling is allowed FRESH on every move", () => {
    let allowed = false
    const { start, move, log } = setup({ scrollAllowed: () => allowed })
    start(10, 10)
    move(10, 200)
    allowed = true
    move(10, 240)
    expect(log).toContain("scrollstart")
  })
})

describe("a second finger", () => {
  it("cancels a PENDING long press", () => {
    const { start, log, container } = setup()
    start(10, 10)
    container.dispatchEvent(
      touchEvent("touchstart", [touch(10, 10), touch(50, 50)]) as unknown as Event,
    )
    vi.advanceTimersByTime(LONG_PRESS_MS * 2)
    expect(log).not.toContain("longpress")
  })

  it("cancels an ACTIVE selection, so lifting one finger out of a pinch does not copy", () => {
    const { start, end, log, lifts, container } = setup()
    start(10, 10)
    vi.advanceTimersByTime(LONG_PRESS_MS)
    expect(log).toContain("longpress")
    container.dispatchEvent(
      touchEvent("touchstart", [touch(10, 10), touch(50, 50)]) as unknown as Event,
    )
    end()
    expect(lifts).toEqual([{ wasTap: false, wasSelecting: false }])
  })

  it("leaves the gesture dead: a later move does nothing", () => {
    const { start, move, log, container } = setup()
    container.dispatchEvent(
      touchEvent("touchstart", [touch(10, 10), touch(50, 50)]) as unknown as Event,
    )
    move(10, 200)
    expect(log).not.toContain("scrollstart")
    void start
  })
})

describe("endings", () => {
  it("releases what the gesture held, on a lift", () => {
    const { start, move, end, log } = setup()
    start(10, 10)
    move(10, 100)
    end()
    expect(log.slice(-3)).toEqual(["reset", "finished", "lift"])
  })

  it("releases it on a CANCEL too, but reports no lift", () => {
    const { start, move, cancel, log, lifts } = setup()
    start(10, 10)
    move(10, 100)
    cancel()
    expect(log.slice(-2)).toEqual(["reset", "finished"])
    expect(lifts).toEqual([])
  })

  it("drops its listeners and its pending timer on dispose", () => {
    const { start, gesture, log } = setup()
    start(10, 10)
    gesture.dispose()
    vi.advanceTimersByTime(LONG_PRESS_MS * 2)
    expect(log).not.toContain("longpress")
  })
})
