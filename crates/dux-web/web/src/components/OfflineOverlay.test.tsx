// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

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

function seed(overrides: Partial<DuxState>) {
  mockState = { offline: false, conn: "open", ...overrides } as DuxState
}

beforeEach(() => {
  installBootStubs()
  reconnectMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("OfflineOverlay", () => {
  it("renders nothing while online", () => {
    seed({ offline: false, conn: "open" })
    const { container } = render(<OfflineOverlay />)
    expect(container.firstChild).toBeNull()
    expect(screen.queryByRole("alertdialog")).toBeNull()
  })

  it("shows the reconnecting state while the socket is still retrying", () => {
    seed({ offline: true, conn: "closed" })
    render(<OfflineOverlay />)
    expect(screen.getByRole("alertdialog")).toBeTruthy()
    expect(screen.getByText("Reconnecting to dux…")).toBeTruthy()
    // The give-up button label must NOT appear yet.
    expect(
      screen.getByRole("button", { name: /reconnect now/i }),
    ).toBeTruthy()
    expect(screen.queryByText("dux is unreachable")).toBeNull()
  })

  it("stays up (still reconnecting copy) through a retry's 'connecting' blip", () => {
    // The sticky offline flag keeps the modal mounted even though conn momentarily
    // reads "connecting" during an auto-retry — it must not flicker to a blank or
    // a different state.
    seed({ offline: true, conn: "connecting" })
    render(<OfflineOverlay />)
    expect(screen.getByText("Reconnecting to dux…")).toBeTruthy()
  })

  it("switches to the unreachable give-up state once retries are exhausted", () => {
    seed({ offline: true, conn: "failed" })
    render(<OfflineOverlay />)
    expect(screen.getByText("dux is unreachable")).toBeTruthy()
    expect(screen.getByRole("button", { name: /retry/i })).toBeTruthy()
    expect(screen.queryByText("Reconnecting to dux…")).toBeNull()
  })

  it("the button forces a fresh reconnect attempt", () => {
    seed({ offline: true, conn: "failed" })
    render(<OfflineOverlay />)
    fireEvent.click(screen.getByRole("button", { name: /retry/i }))
    expect(reconnectMock).toHaveBeenCalledTimes(1)
  })
})
