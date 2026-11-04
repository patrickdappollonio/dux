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

  it("is already collapsed when the page opens in theater, with nothing to animate", () => {
    // The property the boot flag exists to buy. A pane mounting beside this
    // one has to measure the SETTLED height, so a chrome that renders hidden
    // the first time is collapsed synchronously: no children, no measured
    // natural height, no transition running underneath the pane's first fit.
    render(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    expect(screen.queryByRole("button", { name: "Show changes" })).toBeNull()
    expect(screen.getByTestId("theater-chrome").style.height).toBe("0px")
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

  it("eases every property its collapsed state moves, so nothing jumps in the first frame", () => {
    // Tailwind's translate utilities set the `translate` property, not
    // `transform`. A transition list that names only `transform` lets the
    // slide snap 14px while the height has not started to fall, which paints
    // the band's bottom hairline across the screen above the flap for a frame.
    render(
      <TheaterChrome hidden>
        <button type="button">Show changes</button>
      </TheaterChrome>,
    )
    const classes = screen.getByTestId("theater-chrome").className.split(/\s+/)
    const listed = classes
      .map((c) => /^motion-safe:transition-\[([^\]]+)\]$/.exec(c)?.[1])
      .find(Boolean)
      ?.split(",")
    // Every utility the collapsed state adds, by the CSS property it sets.
    const moved = classes.flatMap((c) =>
      /^-?translate-/.test(c) ? ["translate"] : /^opacity-/.test(c) ? ["opacity"] : [],
    )
    expect(moved.length).toBeGreaterThan(0)
    expect(listed).toEqual(expect.arrayContaining(["height", ...moved]))
  })
})
