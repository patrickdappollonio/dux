import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  beginLayoutGesture,
  endLayoutGesture,
  holdLayoutForGesture,
  layoutGestureDepth,
  registerLayoutGestureHolder,
} from "./layoutGesture"

function holder() {
  return { hold: vi.fn(), release: vi.fn() }
}

afterEach(() => {
  // Drain any depth a failing assertion left behind, so tests stay independent.
  while (layoutGestureDepth() > 0) endLayoutGesture()
})

describe("layout gestures", () => {
  it("holds a registered pane for the gesture and releases it exactly once", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    beginLayoutGesture()
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).not.toHaveBeenCalled()
    endLayoutGesture()
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("costs one hold and one release however many gestures overlap", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    beginLayoutGesture()
    beginLayoutGesture()
    endLayoutGesture()
    expect(pane.release).not.toHaveBeenCalled()
    endLayoutGesture()
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("holds a pane that mounts in the middle of a gesture", () => {
    beginLayoutGesture()
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    expect(pane.hold).toHaveBeenCalledTimes(1)
    endLayoutGesture()
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("ignores an end with no gesture in flight", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    endLayoutGesture()
    expect(pane.release).not.toHaveBeenCalled()
    expect(layoutGestureDepth()).toBe(0)
    off()
  })

  it("says nothing to a pane that has unregistered", () => {
    const pane = holder()
    registerLayoutGestureHolder(pane)()
    beginLayoutGesture()
    endLayoutGesture()
    expect(pane.hold).not.toHaveBeenCalled()
    expect(pane.release).not.toHaveBeenCalled()
  })
})

describe("holdLayoutForGesture", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("releases when the animation window elapses, and not before", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    holdLayoutForGesture(300)
    vi.advanceTimersByTime(299)
    expect(pane.release).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("still takes and releases the hold once with no animation at all", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    holdLayoutForGesture(0)
    expect(pane.hold).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(0)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("can be cancelled, which releases immediately rather than leaking a hold", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    const gesture = holdLayoutForGesture(300)
    gesture.cancel()
    expect(pane.release).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(300)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("restarts the window without ever letting the hold go", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    const gesture = holdLayoutForGesture(300)
    vi.advanceTimersByTime(200)
    gesture.restart(300)
    vi.advanceTimersByTime(299)
    // The whole point: a re-toggle mid-animation must not flush a fit at a
    // geometry the layout is still moving through.
    expect(pane.release).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("tells its caller when the window closed, so a stale handle is not restarted", () => {
    const pane = holder()
    const off = registerLayoutGestureHolder(pane)
    const ended = vi.fn()
    const gesture = holdLayoutForGesture(300, ended)
    vi.advanceTimersByTime(300)
    expect(ended).toHaveBeenCalledTimes(1)
    // A restart after the window has closed is a no-op rather than a hold
    // nobody will ever release.
    gesture.restart(300)
    vi.advanceTimersByTime(300)
    expect(pane.release).toHaveBeenCalledTimes(1)
    expect(layoutGestureDepth()).toBe(0)
    off()
  })

  it("reports the end exactly once however it is reached", () => {
    const ended = vi.fn()
    const gesture = holdLayoutForGesture(300, ended)
    gesture.cancel()
    gesture.cancel()
    vi.advanceTimersByTime(300)
    expect(ended).toHaveBeenCalledTimes(1)
  })
})
