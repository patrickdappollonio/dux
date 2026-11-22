// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, renderHook } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// What a full-pane cover does to a flight that is already running. The cover is
// deliberately an instant swap (there is no pill to fly to under one), so the
// thing to pin is that it lands on a RESTING phase every time: a stage left half
// flown would hold the cluster in the air with neither end painted.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

let reducedMotion = false
vi.mock("@/hooks/use-reduced-motion", () => ({
  REDUCED_MOTION_QUERY: "(prefers-reduced-motion: reduce)",
  usePrefersReducedMotion: () => reducedMotion,
}))

// The real store module is imported for its types and partially mocked, and it
// touches storage and the network on load, neither of which exists here.
function installBootStubs() {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()

const { useTheaterFlight } = await import("./use-theater-flight")
const { THEATER_TRANSITION_MS } = await import("@/lib/theater")
const { FLIGHT_ATTACH_MS, FLIGHT_CHROME_SLACK_MS } = await import("@/lib/theaterFlight")
const { resetFlapHiddenForTests, setFlapHidden } = await import("@/lib/flapHidden")

function state(theater: boolean): DuxState {
  return { theater } as unknown as DuxState
}

/// Mount the machine and hand back a setter for the two inputs it reads.
function mount(theater: boolean, cover: boolean) {
  mockState = state(theater)
  const view = renderHook(
    ({ coverOwnsPane }: { coverOwnsPane: boolean }) =>
      useTheaterFlight(coverOwnsPane),
    { initialProps: { coverOwnsPane: cover } },
  )
  return {
    phase: () => view.result.current,
    set: (nextTheater: boolean, nextCover: boolean) => {
      mockState = state(nextTheater)
      act(() => view.rerender({ coverOwnsPane: nextCover }))
    },
    tick: (ms: number) =>
      act(() => {
        vi.advanceTimersByTime(ms)
      }),
  }
}

beforeEach(() => {
  reducedMotion = false
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  // Every fake clock starts at zero, so the fold's last move is forgotten too.
  setFlapHidden(false)
  resetFlapHiddenForTests()
  vi.useRealTimers()
})

describe("a cover landing on a flight in progress", () => {
  it("puts the cluster back on the band when it arrives mid-collapse", () => {
    const view = mount(false, false)
    expect(view.phase()).toBe("docked")
    view.set(true, false)
    expect(view.phase()).toBe("collapsing")
    view.tick(THEATER_TRANSITION_MS / 2)
    expect(view.phase()).toBe("collapsing")
    // The flap is still on the band at this stage, so resting there is where
    // the user is already looking; nothing jumps.
    view.set(true, true)
    expect(view.phase()).toBe("docked")
    // And it STAYS there: a timer left over from the abandoned stage must not
    // step the machine on into a flight nobody asked for.
    view.tick(THEATER_TRANSITION_MS + FLIGHT_CHROME_SLACK_MS)
    expect(view.phase()).toBe("docked")
  })

  it("puts it back on the band when it arrives with the capsule in the air", () => {
    const view = mount(false, false)
    view.set(true, false)
    view.tick(THEATER_TRANSITION_MS + FLIGHT_CHROME_SLACK_MS)
    expect(view.phase()).toBe("detaching")
    // Mid-travel the pill is the only thing painted, and the cover withholds
    // it, so the one resting phase that paints anything at all is the dock.
    view.set(true, true)
    expect(view.phase()).toBe("docked")
    view.tick(10 * THEATER_TRANSITION_MS)
    expect(view.phase()).toBe("docked")
  })

  it("flies nothing while the cover is up, and swaps to the pill when it goes", () => {
    // Entering theater under a cover moves no chrome, so there is no flight to
    // interrupt; the cover retiring is the swap, and it is instant because the
    // dock it would fly from is the chrome that is leaving in the same commit.
    const view = mount(false, true)
    view.set(true, true)
    expect(view.phase()).toBe("docked")
    view.tick(THEATER_TRANSITION_MS)
    expect(view.phase()).toBe("docked")
    view.set(true, false)
    expect(view.phase()).toBe("floating")
    view.tick(10 * THEATER_TRANSITION_MS)
    expect(view.phase()).toBe("floating")
  })
})

// The flight measures the flap where it rests on the band, at the start of the
// detach and again for the landing. A fold moving under it (the header's
// show/hide stays pressable while the chrome collapses and expands) would hand
// the pill a rect sliding into the band, so a fold that moves during a flight
// lands it at its resting end, and a mode that moves while the fold is still
// sliding swaps with no flight.
describe("the flap's fold and a flight", () => {
  const fold = (hidden: boolean) => act(() => setFlapHidden(hidden))
  it("lands a departure at rest when the flap folds during the collapse", () => {
    const view = mount(false, false)
    view.set(true, false)
    expect(view.phase()).toBe("collapsing")
    fold(true)
    expect(view.phase()).toBe("floating")
    view.tick(10 * THEATER_TRANSITION_MS)
    expect(view.phase()).toBe("floating")
  })

  it("lands a return at rest when the flap folds on the way home", () => {
    const view = mount(true, false)
    view.set(false, false)
    expect(view.phase()).toBe("expanding")
    view.tick(THEATER_TRANSITION_MS + FLIGHT_CHROME_SLACK_MS)
    expect(view.phase()).toBe("returning")
    fold(true)
    expect(view.phase()).toBe("docked")
    view.tick(10 * THEATER_TRANSITION_MS)
    expect(view.phase()).toBe("docked")
  })

  it("swaps into theater while a shown flap is still sliding down, and flies once it rests", () => {
    fold(true)
    const view = mount(false, false)
    fold(false)
    view.tick(FLIGHT_ATTACH_MS / 2)
    view.set(true, false)
    expect(view.phase()).toBe("floating")

    view.tick(FLIGHT_ATTACH_MS)
    view.set(false, false)
    expect(view.phase()).toBe("expanding")
    for (let i = 0; i < 4; i++) view.tick(THEATER_TRANSITION_MS + FLIGHT_CHROME_SLACK_MS)
    expect(view.phase()).toBe("docked")
    view.set(true, false)
    expect(view.phase()).toBe("collapsing")
  })
})
