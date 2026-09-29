import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { ATTENTION_PULSE_PERIOD_MS, attentionFrameAt } from "./attentionPulse"
import {
  createWakeTimer,
  startAttentionBlink,
  windowWakeTimer,
  workerWakeTimer,
  type WakeTimer,
} from "./faviconBlink"

// A stand-in for the dedicated worker: jsdom and node have no Worker, so the
// boundary is faked and the protocol across it is what is asserted.
class FakeWorker {
  static instances: FakeWorker[] = []
  posted: unknown[] = []
  terminated = false
  onmessage: ((ev: { data: unknown }) => void) | null = null
  url: unknown
  constructor(url: unknown) {
    this.url = url
    FakeWorker.instances.push(this)
  }
  postMessage(msg: unknown) {
    this.posted.push(msg)
  }
  terminate() {
    this.terminated = true
  }
  reply(data: unknown) {
    this.onmessage?.({ data })
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeWorker.instances = []
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("startAttentionBlink", () => {
  it("shows exactly the shared rhythm's frames at the shared times while visible", () => {
    const shown: Array<[number, string]> = []
    const start = Date.now()
    const blink = startAttentionBlink({
      timer: windowWakeTimer(),
      show: (f) => shown.push([Date.now() - start, f]),
      hidden: () => false,
    })

    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 2)
    blink.stop()

    const expected: Array<[number, string]> = []
    let t = 0
    let last: string | null = null
    while (t <= ATTENTION_PULSE_PERIOD_MS * 2) {
      const { frame, msUntilChange } = attentionFrameAt(t, "rhythm")
      if (frame !== last) expected.push([t, frame])
      last = frame
      t += msUntilChange
    }
    expect(shown).toEqual(expected)
  })

  it("switches to the steady blink while the page is hidden, and back on resync", () => {
    let hidden = false
    const shown: Array<[number, string]> = []
    const start = Date.now()
    const blink = startAttentionBlink({
      timer: windowWakeTimer(),
      show: (f) => shown.push([Date.now() - start, f]),
      hidden: () => hidden,
    })
    hidden = true
    blink.resync()
    shown.length = 0
    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 2)
    // Only whole half-period flips: on/off at a rate a 1 s throttle keeps.
    const times = shown.map(([t]) => t % (ATTENTION_PULSE_PERIOD_MS / 2))
    expect(new Set(times)).toEqual(new Set([0]))
    expect(shown.length).toBeGreaterThanOrEqual(3)
    blink.stop()
  })

  it("leaves no timer behind once stopped", () => {
    const blink = startAttentionBlink({
      timer: windowWakeTimer(),
      show: () => {},
      hidden: () => false,
    })
    expect(vi.getTimerCount()).toBe(1)
    blink.stop()
    expect(vi.getTimerCount()).toBe(0)
    // A stopped blink never shows another frame.
    const show = vi.fn()
    const again = startAttentionBlink({ timer: windowWakeTimer(), show, hidden: () => false })
    again.stop()
    show.mockClear()
    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 3)
    expect(show).not.toHaveBeenCalled()
  })
})

describe("the wake timers", () => {
  it("drives wakes from a dedicated worker and terminates it on dispose", () => {
    vi.stubGlobal("Worker", FakeWorker)
    const timer = workerWakeTimer()
    expect(timer).not.toBeNull()
    const worker = FakeWorker.instances[0]
    const fire = vi.fn()
    timer!.set(180, fire)
    expect(worker.posted.at(-1)).toMatchObject({ type: "set", delay: 180 })
    const { id } = worker.posted.at(-1) as { id: number }

    // A stale tick (an id the page has moved past) is ignored.
    timer!.set(90, fire)
    worker.reply({ type: "tick", id })
    expect(fire).not.toHaveBeenCalled()
    const { id: current } = worker.posted.at(-1) as { id: number }
    worker.reply({ type: "tick", id: current })
    expect(fire).toHaveBeenCalledTimes(1)

    timer!.clear()
    expect(worker.posted.at(-1)).toEqual({ type: "clear" })
    timer!.dispose()
    expect(worker.terminated).toBe(true)
    // Nothing the worker says after dispose reaches the page.
    worker.reply({ type: "tick", id: current })
    expect(fire).toHaveBeenCalledTimes(1)
  })

  it("prefers the worker, and falls back to the page's own timers without one", () => {
    vi.stubGlobal("Worker", FakeWorker)
    const withWorker = createWakeTimer()
    expect(FakeWorker.instances).toHaveLength(1)
    withWorker.dispose()
    expect(FakeWorker.instances[0].terminated).toBe(true)

    vi.stubGlobal("Worker", undefined)
    expect(workerWakeTimer()).toBeNull()
    const fallback: WakeTimer = createWakeTimer()
    const fire = vi.fn()
    fallback.set(50, fire)
    vi.advanceTimersByTime(50)
    expect(fire).toHaveBeenCalledTimes(1)
    fallback.dispose()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("falls back when constructing the worker throws", () => {
    vi.stubGlobal(
      "Worker",
      class {
        constructor() {
          throw new Error("blocked")
        }
      },
    )
    expect(workerWakeTimer()).toBeNull()
  })
})
