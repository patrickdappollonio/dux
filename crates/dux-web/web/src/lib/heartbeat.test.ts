import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  DEFAULT_HEARTBEAT_SECONDS,
  publishConnectionTiming,
} from "./connectionTiming"
import { createHeartbeat, heartbeatIntervalMs } from "./heartbeat"
import { VIEWED_PING_INTERVAL_MS } from "./viewedPing"
import type { VisibleClock } from "./visibleClock"

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  publishConnectionTiming(undefined)
})

describe("the one period, from one pure function", () => {
  it("is the viewed ping's 2s while this device is owner AND visible", () => {
    expect(heartbeatIntervalMs({ isOwner: true, visible: true })).toBe(
      VIEWED_PING_INTERVAL_MS,
    )
    expect(VIEWED_PING_INTERVAL_MS).toBe(2000)
  })

  it("is the configured heartbeat period in every other state", () => {
    expect(heartbeatIntervalMs({ isOwner: false, visible: true })).toBe(
      DEFAULT_HEARTBEAT_SECONDS * 1000,
    )
    expect(heartbeatIntervalMs({ isOwner: true, visible: false })).toBe(
      DEFAULT_HEARTBEAT_SECONDS * 1000,
    )
    publishConnectionTiming({ heartbeat_seconds: 5 })
    expect(heartbeatIntervalMs({ isOwner: false, visible: true })).toBe(5000)
  })
})

/// A visible clock the test drives by hand, so a deadline can be walked past
/// without arguing with `performance.now()`.
function fakeClock(): VisibleClock & { advance: (ms: number) => void } {
  let now = 0
  return {
    elapsedMs: () => now,
    reset: () => {
      now = 0
    },
    dispose: () => {},
    advance: (ms: number) => {
      now += ms
    },
  }
}

type Frame = { beat: number; viewed: boolean }

function setup(
  opts: {
    isOwner?: boolean
    visible?: boolean
    viewed?: boolean
    sends?: boolean
  } = {},
) {
  const frames: Frame[] = []
  const clock = fakeClock()
  let stalls = 0
  const state = {
    isOwner: opts.isOwner ?? true,
    visible: opts.visible ?? true,
    viewed: opts.viewed ?? true,
    sends: opts.sends ?? true,
  }
  const beat = createHeartbeat({
    send: (n, viewed) => {
      if (!state.sends) return false
      frames.push({ beat: n, viewed })
      return true
    },
    isOwner: () => state.isOwner,
    viewed: () => state.viewed,
    visible: () => state.visible,
    onStalled: () => {
      stalls++
    },
    deadlineMs: () => 30_000,
    clock,
  })
  // Every tick both fires the timer and lets the clock follow it, so visible
  // time and the timer agree the way they do in a visible page.
  const advance = (ms: number) => {
    clock.advance(ms)
    vi.advanceTimersByTime(ms)
  }
  return { beat, frames, clock, advance, state, stalls: () => stalls }
}

describe("the cadence", () => {
  it("beats every 2s while owner and visible", () => {
    const { beat, frames, advance } = setup()
    beat.start()
    advance(2000)
    advance(2000)
    advance(2000)
    expect(frames.map((f) => f.beat)).toEqual([1, 2, 3])
    beat.stop()
  })

  it("beats on the heartbeat period while merely watching", () => {
    const { beat, frames, advance } = setup({ isOwner: false, viewed: false })
    beat.start()
    advance(14_000)
    expect(frames).toHaveLength(0)
    advance(1000)
    expect(frames).toHaveLength(1)
    beat.stop()
  })

  it("changes cadence live when ownership does", () => {
    const { beat, frames, advance, state } = setup({ isOwner: false, viewed: false })
    beat.start()
    advance(15_000)
    expect(frames).toHaveLength(1)
    state.isOwner = true
    state.viewed = true
    // The gap already scheduled runs out first: the period is re-read at each
    // tick, so a change applies from the tick after it.
    advance(15_000)
    expect(frames).toHaveLength(2)
    advance(2000)
    expect(frames).toHaveLength(3)
    beat.stop()
  })
})

describe("the frame shape", () => {
  it("carries viewed true for a watching owner", () => {
    const { beat, frames, advance } = setup()
    beat.start()
    advance(2000)
    expect(frames[0]).toEqual({ beat: 1, viewed: true })
    beat.stop()
  })

  it("carries viewed false for a watcher, who still beats", () => {
    const { beat, frames, advance } = setup({ isOwner: false, viewed: false })
    beat.start()
    advance(15_000)
    expect(frames[0]).toEqual({ beat: 1, viewed: false })
    beat.stop()
  })

  it("carries viewed false for an owner inside the attention grace window", () => {
    const { beat, frames, advance } = setup({ viewed: false })
    beat.start()
    advance(2000)
    expect(frames[0]).toEqual({ beat: 1, viewed: false })
    beat.stop()
  })
})

describe("the answer deadline", () => {
  it("forces exactly ONE plain reconnect when the answers stop", () => {
    const { beat, advance, stalls } = setup()
    beat.start()
    // The first beat goes out at t=2000 and is never answered, so the 30s
    // deadline is reached at the first tick at or after t=32000.
    while (stalls() === 0) advance(2000)
    expect(stalls()).toBe(1)
    // And the stall is not re-raised on every later tick: the outstanding beat
    // was retired with it, so a fresh one has to go unanswered for a fresh
    // deadline first.
    advance(2000)
    expect(stalls()).toBe(1)
    beat.stop()
  })

  it("never fires while the answers keep coming", () => {
    const { beat, frames, advance, stalls } = setup()
    beat.start()
    for (let i = 0; i < 40; i++) {
      advance(2000)
      const latest = frames.at(-1)
      if (latest) beat.noteAnswer(latest.beat)
    }
    expect(stalls()).toBe(0)
    beat.stop()
  })

  it("ignores an answer to a beat older than the one being waited on", () => {
    const { beat, advance, stalls } = setup()
    beat.start()
    advance(2000) // beat 1 goes out unanswered
    for (let i = 0; i < 20 && stalls() === 0; i++) {
      advance(2000)
      // An echo of a beat that predates the outstanding one proves nothing.
      beat.noteAnswer(0)
    }
    expect(stalls()).toBe(1)
    beat.stop()
  })

  it("does not elapse while the page is HIDDEN, however long that lasts", () => {
    const { beat, advance, state, stalls, clock } = setup()
    beat.start()
    advance(2000) // one unanswered beat
    state.visible = false
    // Hours of wall time pass. Visible time does not advance, so the deadline
    // cannot be reached, and nothing is sent either.
    vi.advanceTimersByTime(60 * 60 * 1000)
    expect(stalls()).toBe(0)
    // Back in the foreground, the deadline resumes from where it paused.
    state.visible = true
    clock.advance(30_000)
    vi.advanceTimersByTime(2000)
    expect(stalls()).toBe(1)
    beat.stop()
  })

  it("starts no deadline for a frame the socket discarded", () => {
    const { beat, advance, stalls, state } = setup({ sends: false })
    beat.start()
    for (let i = 0; i < 40; i++) advance(2000)
    expect(stalls()).toBe(0)
    state.sends = true
    beat.stop()
  })

  it("reset() forgets the outstanding beat, because a reopened socket moots it", () => {
    const { beat, advance, stalls } = setup()
    beat.start()
    advance(2000)
    for (let i = 0; i < 10; i++) advance(2000)
    beat.reset()
    for (let i = 0; i < 10; i++) advance(2000)
    expect(stalls()).toBe(0)
    beat.stop()
  })
})
