// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, screen } from "@testing-library/react"

import { THEATER_TRANSITION_MS } from "@/lib/theater"
import { TheaterChrome } from "./TheaterChrome"

let reducedMotion = false
vi.mock("@/hooks/use-reduced-motion", () => ({
  REDUCED_MOTION_QUERY: "(prefers-reduced-motion: reduce)",
  usePrefersReducedMotion: () => reducedMotion,
}))

beforeEach(() => {
  reducedMotion = false
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe("the collapsing chrome", () => {
  it("shows its children when theater is off", () => {
    render(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.getByRole("button", { name: "Show changes" })).toBeTruthy()
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("false")
  })

  it("takes the chrome out of the DOM once the collapse finishes", () => {
    const { rerender } = render(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    rerender(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    // Still there while it animates away, so there is something to animate.
    expect(screen.queryByRole("button", { name: "Show changes" })).not.toBeNull()
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    // Gone afterwards: invisible chrome a keyboard can still reach is not
    // hidden, it is only hard to see.
    expect(screen.queryByRole("button", { name: "Show changes" })).toBeNull()
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("true")
  })

  it("brings the chrome back when theater ends", () => {
    const { rerender } = render(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(screen.queryByRole("button", { name: "Show changes" })).toBeNull()
    rerender(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.getByRole("button", { name: "Show changes" })).toBeTruthy()
  })

  it("cuts straight to hidden under reduced motion, with nothing to wait for", () => {
    reducedMotion = true
    const { rerender } = render(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    rerender(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.queryByRole("button", { name: "Show changes" })).toBeNull()
  })

  it("drops the inline height once the chrome is back, so it can grow again", () => {
    const { rerender } = render(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    rerender(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.getByTestId("theater-chrome").style.height).not.toBe("")
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(screen.getByTestId("theater-chrome").style.height).toBe("")
  })

  it("animates for exactly as long as the gesture holds the terminal's refit", () => {
    // The CSS duration is a literal `duration-300` (Tailwind scans source
    // text, so a class built from a variable produces no CSS at all), and the
    // gesture holds the terminal's refit for THEATER_TRANSITION_MS. A drift
    // between them re-grids the terminal mid-transition, so the RENDERED class
    // is what this reads: comparing the constant to itself proved nothing.
    render(
      <TheaterChrome hidden={false}>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.getByTestId("theater-chrome").className).toContain(
      `duration-${THEATER_TRANSITION_MS}`,
    )
  })
})
