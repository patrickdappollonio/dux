// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` (seeded spine + target) and spy the two store actions the
// dialog fires, while every other store export stays intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    reconnectSession: vi.fn(),
    closeForceReconnect: vi.fn(),
  }
})

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
const { ConfirmForceReconnectDialog } = await import(
  "./ConfirmForceReconnectDialog"
)
const store = await import("@/lib/store")
const reconnectSession = vi.mocked(store.reconnectSession)
const closeForceReconnect = vi.mocked(store.closeForceReconnect)

function seed(target: string | null, sessions: unknown[]) {
  mockState = {
    forceReconnectTarget: target,
    spine: { sessions },
  } as unknown as DuxState
}

const session = { id: "s1", title: "quacky-mallard", branch_name: "dux/s1" }

beforeEach(() => {
  installBootStubs()
  reconnectSession.mockClear()
  closeForceReconnect.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmForceReconnectDialog", () => {
  it("asks for confirmation before force-recreating, naming the agent", () => {
    seed("s1", [session])
    render(<ConfirmForceReconnectDialog />)
    expect(screen.getByText("Force recreate quacky-mallard?")).toBeTruthy()
    expect(
      screen.getByText(/start a fresh session instead of continuing/),
    ).toBeTruthy()
    // Nothing fires until the user confirms.
    expect(reconnectSession).not.toHaveBeenCalled()
  })

  it("force-reconnects (fresh) and closes only on confirm", () => {
    seed("s1", [session])
    render(<ConfirmForceReconnectDialog />)
    fireEvent.click(screen.getByText("Force recreate"))
    expect(reconnectSession).toHaveBeenCalledWith("s1", true)
    expect(closeForceReconnect).toHaveBeenCalled()
  })

  it("cancel closes without reconnecting", () => {
    seed("s1", [session])
    render(<ConfirmForceReconnectDialog />)
    fireEvent.click(screen.getByText("Cancel"))
    expect(reconnectSession).not.toHaveBeenCalled()
    expect(closeForceReconnect).toHaveBeenCalled()
  })

  it("closes itself when the target agent vanishes from the ViewModel", () => {
    seed("s1", [])
    render(<ConfirmForceReconnectDialog />)
    expect(screen.queryByText(/Force recreate/)).toBeNull()
    expect(closeForceReconnect).toHaveBeenCalled()
  })
})
