// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  afterVisibilitySample,
  createVisibleClock,
  elapsedVisibleMs,
  freshSpan,
} from "./visibleClock"

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
}

let clock = 0
beforeEach(() => {
  clock = 0
  setVisibility("visible")
  vi.spyOn(performance, "now").mockImplementation(() => clock)
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe("the pure visible-span accumulator", () => {
  it("counts nothing while hidden and resumes on return", () => {
    // Visible from t=0.
    let span = freshSpan(0, true)
    expect(elapsedVisibleMs(span, 100)).toBe(100)
    // Hidden at t=100: the span banks 100 and stops.
    span = afterVisibilitySample(span, 100, false)
    expect(elapsedVisibleMs(span, 10_000)).toBe(100)
    // Visible again at t=10_000: the hidden stretch contributed nothing.
    span = afterVisibilitySample(span, 10_000, true)
    expect(elapsedVisibleMs(span, 10_050)).toBe(150)
  })

  it("ignores a redundant sample of the state it is already in", () => {
    let span = freshSpan(0, true)
    span = afterVisibilitySample(span, 40, true)
    expect(elapsedVisibleMs(span, 100)).toBe(100)
  })

  it("starts banked at zero when the page begins hidden", () => {
    const span = freshSpan(0, false)
    expect(elapsedVisibleMs(span, 5_000)).toBe(0)
  })
})

describe("createVisibleClock", () => {
  it("accumulates only visible time across a hide and a show", () => {
    const c = createVisibleClock()
    clock = 500
    expect(c.elapsedMs()).toBe(500)
    setVisibility("hidden")
    document.dispatchEvent(new Event("visibilitychange"))
    clock = 60_000
    expect(c.elapsedMs()).toBe(500)
    setVisibility("visible")
    document.dispatchEvent(new Event("visibilitychange"))
    clock = 60_250
    expect(c.elapsedMs()).toBe(750)
    c.dispose()
  })

  it("reset() starts a new epoch from zero", () => {
    const c = createVisibleClock()
    clock = 900
    expect(c.elapsedMs()).toBe(900)
    c.reset()
    expect(c.elapsedMs()).toBe(0)
    clock = 1_000
    expect(c.elapsedMs()).toBe(100)
    c.dispose()
  })

  it("stops tracking visibility after dispose", () => {
    const c = createVisibleClock()
    c.dispose()
    setVisibility("hidden")
    document.dispatchEvent(new Event("visibilitychange"))
    clock = 1_000
    // Dispose detaches the listener rather than freezing the reading, so the
    // span it was left in keeps running.
    expect(c.elapsedMs()).toBe(1_000)
  })
})
