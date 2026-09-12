// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ReconnectPlanEvent } from "@/lib/reconnectingSocket"

// Override `useDux` so the overlay reads our seeded connection state, and stub
// `reconnect` so we can assert the button wiring without touching the real socket.
let mockState: DuxState
const reconnectMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, reconnect: reconnectMock }
})

// The real store boots on import (localStorage + bootstrap fetch). jsdom doesn't
// provide those as bare globals, so stub them before the component loads.
function installBootStubs() {
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
const { OfflineOverlay } = await import("./OfflineOverlay")

function plan(overrides: Partial<ReconnectPlanEvent> = {}): ReconnectPlanEvent {
  return {
    phase: "waiting",
    attempt: 3,
    budget: 8,
    nextAttemptAt: Date.now() + 4_000,
    attemptTimeoutMs: 10_000,
    ...overrides,
  }
}

function seed(overrides: Partial<DuxState>) {
  mockState = {
    offline: false,
    conn: "open",
    reconnectPlan: null,
    ...overrides,
  } as DuxState
}

beforeEach(() => {
  installBootStubs()
  reconnectMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("OfflineOverlay", () => {
  it("renders nothing while online", () => {
    seed({ offline: false, conn: "open" })
    const { container } = render(<OfflineOverlay />)
    expect(container.firstChild).toBeNull()
    expect(screen.queryByRole("alertdialog")).toBeNull()
  })

  it("stays up through a retry's 'connecting' blip", () => {
    // The sticky offline flag keeps the modal mounted even though conn momentarily
    // reads "connecting" during an auto-retry; it must not flicker to a blank.
    seed({ offline: true, conn: "connecting", reconnectPlan: plan() })
    render(<OfflineOverlay />)
    expect(screen.getByRole("alertdialog")).toBeTruthy()
  })
})

// THREE FACES, AND THE BUTTON IS IN ALL OF THEM. The overlay used to say dux
// "keeps trying for as long as this page is open" over an indeterminate spinner,
// which on a remote network was indistinguishable from being stuck: an
// unreachable host does not refuse a connection, it hangs. Each face now says a
// number.
describe("the waiting face", () => {
  it("names the attempt that failed, the budget, and the countdown", () => {
    seed({
      offline: true,
      conn: "closed",
      reconnectPlan: plan({ attempt: 3, budget: 8, nextAttemptAt: Date.now() + 4_000 }),
    })
    render(<OfflineOverlay />)
    expect(screen.getByText("Reconnecting to dux…")).toBeTruthy()
    expect(screen.getByText("Attempt 3 of 8 failed. Retrying in 4 s.")).toBeTruthy()
    expect(
      screen.getByText("The server may be down or this device may be offline."),
    ).toBeTruthy()
    expect(screen.getByRole("button", { name: "Reconnect now" })).toBeTruthy()
  })

  it("drops the budget from the sentence when there is no give-up to count towards", () => {
    seed({
      offline: true,
      conn: "closed",
      reconnectPlan: plan({ attempt: 3, budget: 0, nextAttemptAt: Date.now() + 4_000 }),
    })
    render(<OfflineOverlay />)
    expect(screen.getByText("Attempt 3 failed. Retrying in 4 s.")).toBeTruthy()
  })

  it("counts down once a second on the wall clock", () => {
    vi.useFakeTimers()
    const start = Date.now()
    seed({
      offline: true,
      conn: "closed",
      reconnectPlan: plan({ attempt: 2, budget: 8, nextAttemptAt: start + 4_000 }),
    })
    render(<OfflineOverlay />)
    expect(screen.getByText("Attempt 2 of 8 failed. Retrying in 4 s.")).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(2_000)
    })
    expect(screen.getByText("Attempt 2 of 8 failed. Retrying in 2 s.")).toBeTruthy()
  })

  // MEASURED, AND WRONG BEFORE THE FIX. The clock behind the countdown only
  // ticked while a countdown was on screen, so after ten seconds of the
  // connecting face it was ten seconds stale: the first frame of the next
  // waiting face read "Retrying in 11 s" and only snapped to 1 s at the next
  // sample. Every cycle, on the face the user looks at longest.
  it("is right on its FIRST frame, after a long connecting face", () => {
    vi.useFakeTimers()
    seed({
      offline: true,
      conn: "connecting",
      reconnectPlan: plan({
        phase: "connecting",
        attempt: 4,
        nextAttemptAt: null,
      }),
    })
    const { rerender } = render(<OfflineOverlay />)
    // The attempt sits unopened for the whole configured deadline.
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    // It is abandoned, and the next attempt is a second away.
    seed({
      offline: true,
      conn: "closed",
      reconnectPlan: plan({ attempt: 4, budget: 8, nextAttemptAt: Date.now() + 1_000 }),
    })
    rerender(<OfflineOverlay />)
    expect(screen.getByText("Attempt 4 of 8 failed. Retrying in 1 s.")).toBeTruthy()
  })

  it("says nothing about attempts before the socket has published one", () => {
    // The window between the first drop and the first plan. The overlay still
    // has something true to say; it just cannot count yet.
    seed({ offline: true, conn: "closed", reconnectPlan: null })
    render(<OfflineOverlay />)
    expect(screen.getByText("Reconnecting to dux…")).toBeTruthy()
    expect(
      screen.getByText("The server may be down or this device may be offline."),
    ).toBeTruthy()
    expect(screen.queryByText(/^Attempt /)).toBeNull()
  })
})

describe("the connecting face", () => {
  it("names the attempt in flight and how long it may take", () => {
    seed({
      offline: true,
      conn: "connecting",
      reconnectPlan: plan({
        phase: "connecting",
        attempt: 4,
        budget: 8,
        nextAttemptAt: null,
        attemptTimeoutMs: 10_000,
      }),
    })
    render(<OfflineOverlay />)
    expect(screen.getByText("Reconnecting to dux…")).toBeTruthy()
    expect(screen.getByText("Attempt 4 of 8, connecting…")).toBeTruthy()
    expect(
      screen.getByText("If the server is unreachable this can take up to 10 s."),
    ).toBeTruthy()
    expect(screen.getByRole("button", { name: "Reconnect now" })).toBeTruthy()
  })
})

describe("the given-up face", () => {
  it("stops promising, says how many attempts were made, and offers the way back", () => {
    seed({
      offline: true,
      conn: "failed",
      reconnectPlan: plan({
        phase: "given_up",
        attempt: 8,
        budget: 8,
        nextAttemptAt: null,
      }),
    })
    render(<OfflineOverlay />)
    expect(screen.getByText("Not connected to dux")).toBeTruthy()
    expect(screen.queryByText("Reconnecting to dux…")).toBeNull()
    expect(
      screen.getByText(
        "dux stopped trying after 8 attempts. Reconnect when the server is back, or check that this device is online.",
      ),
    ).toBeTruthy()
    expect(screen.getByRole("button", { name: "Reconnect" })).toBeTruthy()
  })

  it("counts one attempt in the singular", () => {
    seed({
      offline: true,
      conn: "failed",
      reconnectPlan: plan({ phase: "given_up", attempt: 1, budget: 1, nextAttemptAt: null }),
    })
    render(<OfflineOverlay />)
    expect(
      screen.getByText(
        "dux stopped trying after 1 attempt. Reconnect when the server is back, or check that this device is online.",
      ),
    ).toBeTruthy()
  })
})

describe("the button", () => {
  it("forces an attempt now in every face", () => {
    for (const phase of ["waiting", "connecting", "given_up"] as const) {
      seed({ offline: true, conn: "closed", reconnectPlan: plan({ phase }) })
      render(<OfflineOverlay />)
      fireEvent.click(screen.getByRole("button", { name: /^Reconnect/ }))
      cleanup()
    }
    expect(reconnectMock).toHaveBeenCalledTimes(3)
  })
})
