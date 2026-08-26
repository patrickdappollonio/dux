// @vitest-environment jsdom
//
// THE BROWSER HALF of the heartbeat, which the pure tests beside this file
// cannot see. They inject `visible` and a fake clock, so the module's OWN
// `visibilitychange` listener and its OWN visible clock never run there, and a
// regression in either is invisible: that is the same shape of gap that let a
// first-load regression through once already. These mount the real wiring
// against jsdom's document and drive it with real events.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { createHeartbeat } from "./heartbeat"
import { VIEWED_PING_INTERVAL_MS } from "./viewedPing"

beforeEach(() => {
  vi.useFakeTimers()
  setVisibility("visible")
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  setVisibility("visible")
})

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
  document.dispatchEvent(new Event("visibilitychange"))
}

/// jsdom's `performance.now()` is not moved by fake timers, and the module's own
/// clock is built on it, so the reading is driven by hand alongside the timers.
function setup(opts: { deadlineMs?: number } = {}) {
  let now = 0
  vi.spyOn(performance, "now").mockImplementation(() => now)
  const frames: number[] = []
  let stalls = 0
  const beat = createHeartbeat({
    send: (n) => {
      frames.push(n)
      return true
    },
    isOwner: () => true,
    viewed: () => true,
    onStalled: () => {
      stalls++
    },
    deadlineMs: () => opts.deadlineMs ?? 30_000,
  })
  const advance = (ms: number) => {
    now += ms
    vi.advanceTimersByTime(ms)
  }
  return { beat, frames, advance, stalls: () => stalls }
}

describe("the module's own visibility wiring", () => {
  it("parks itself when the real document goes hidden, with no caller involved", () => {
    const { beat, frames, advance } = setup()
    beat.start()
    advance(VIEWED_PING_INTERVAL_MS)
    expect(frames.length).toBe(1)
    setVisibility("hidden")
    advance(10 * VIEWED_PING_INTERVAL_MS)
    expect(frames.length).toBe(1)
    beat.stop()
  })

  it("picks itself back up on the real return event", () => {
    const { beat, frames, advance } = setup()
    beat.start()
    setVisibility("hidden")
    advance(10 * VIEWED_PING_INTERVAL_MS)
    expect(frames.length).toBe(0)
    setVisibility("visible")
    advance(VIEWED_PING_INTERVAL_MS)
    expect(frames.length).toBe(1)
    beat.stop()
  })

  it("stops listening once it is stopped, so a stopped beat cannot be revived", () => {
    const { beat, frames, advance } = setup()
    beat.start()
    beat.stop()
    setVisibility("hidden")
    setVisibility("visible")
    advance(10 * VIEWED_PING_INTERVAL_MS)
    expect(frames.length).toBe(0)
  })
})

// A pane that stops and restarts its heartbeat (an unmount and remount of the
// same module, a target switch) used to be left with the clock `stop()` had
// disposed: the listener was gone, so the clock counted HIDDEN time as visible
// and the deadline elapsed against a page that had been in a pocket.
describe("a heartbeat that is started again after being stopped", () => {
  it("still pauses its deadline while the page is hidden", () => {
    const { beat, advance, stalls } = setup()
    beat.start()
    beat.stop()
    beat.start()
    advance(VIEWED_PING_INTERVAL_MS) // one beat goes out, unanswered
    setVisibility("hidden")
    // An hour in a pocket. Visible time must not move, so the deadline cannot
    // be reached, and the parked timer sends nothing either.
    advance(60 * 60 * 1000)
    setVisibility("visible")
    advance(VIEWED_PING_INTERVAL_MS)
    expect(stalls()).toBe(0)
    beat.stop()
  })
})
