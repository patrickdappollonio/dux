// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import {
  COARSE_POINTER_QUERY,
  stubMatchMedia,
  type MatchMediaStub,
} from "@/test/matchMedia"

import { useIsCoarsePointer } from "./use-coarse-pointer"

function Probe() {
  const coarse = useIsCoarsePointer()
  return <span data-testid="probe">{coarse ? "coarse" : "fine"}</span>
}

const read = () => screen.getByTestId("probe").textContent

describe("useIsCoarsePointer", () => {
  let stub: MatchMediaStub | null = null
  afterEach(() => {
    cleanup()
    stub?.restore()
    stub = null
  })

  it("reports the pointer capability, not the viewport width", () => {
    stub = stubMatchMedia({ [COARSE_POINTER_QUERY]: true })
    render(<Probe />)
    expect(read()).toBe("coarse")
  })

  it("is false when the primary pointer is fine", () => {
    stub = stubMatchMedia({ [COARSE_POINTER_QUERY]: false })
    render(<Probe />)
    expect(read()).toBe("fine")
  })

  // Gating the compose bar on `useIsMobile()` swaps the user's typing surface
  // mid-session when a tablet rotates across the 768px width breakpoint. A
  // width change must be invisible here.
  it("does not change when only the viewport width changes", () => {
    stub = stubMatchMedia({ [COARSE_POINTER_QUERY]: true })
    const original = window.innerWidth
    try {
      render(<Probe />)
      expect(read()).toBe("coarse")

      for (const width of [1400, 500, 900, 320]) {
        act(() => {
          Object.defineProperty(window, "innerWidth", {
            value: width,
            configurable: true,
          })
          window.dispatchEvent(new Event("resize"))
        })
        expect(read()).toBe("coarse")
      }
    } finally {
      Object.defineProperty(window, "innerWidth", {
        value: original,
        configurable: true,
      })
    }
  })

  it("is SUBSCRIBED: a live capability change re-renders", () => {
    stub = stubMatchMedia({ [COARSE_POINTER_QUERY]: false })
    render(<Probe />)
    expect(read()).toBe("fine")

    act(() => stub!.set(COARSE_POINTER_QUERY, true))
    expect(read()).toBe("coarse")

    act(() => stub!.set(COARSE_POINTER_QUERY, false))
    expect(read()).toBe("fine")
  })

  it("unsubscribes on unmount", () => {
    stub = stubMatchMedia({ [COARSE_POINTER_QUERY]: true })
    const view = render(<Probe />)
    expect(stub.listenerCount()).toBe(1)
    view.unmount()
    expect(stub.listenerCount()).toBe(0)
  })

  // The jsdom-safe shape `use-mobile.ts` documents: a missing matchMedia costs
  // the subscription and reads as not-coarse rather than throwing. Every test
  // in this repo that never stubs matchMedia depends on this.
  it("degrades to false when the browser has no matchMedia", () => {
    const previous = Object.getOwnPropertyDescriptor(window, "matchMedia")
    delete (window as { matchMedia?: unknown }).matchMedia
    try {
      expect(() => render(<Probe />)).not.toThrow()
      expect(read()).toBe("fine")
    } finally {
      if (previous) Object.defineProperty(window, "matchMedia", previous)
    }
  })
})
