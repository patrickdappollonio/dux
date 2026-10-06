// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView } from "@/lib/types"

// Override `useDux` so the dialog reads our seeded spine and stop target, and
// spy the two store calls the dialog makes; the other real exports stay.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeStopTab: vi.fn(),
    stopTab: vi.fn(),
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
const { ConfirmStopTabDialog } = await import("./ConfirmStopTabDialog")
const store = await import("@/lib/store")
const closeStopTab = vi.mocked(store.closeStopTab)
const stopTab = vi.mocked(store.stopTab)

function tab(overrides: Partial<AgentTabView>): AgentTabView {
  return {
    id: "s1",
    provider: "claude",
    order: 0,
    working: false,
    has_output: false,
    has_live_process: true,
    ...overrides,
  }
}

function seed(tabId: string, tabs: AgentTabView[]) {
  mockState = {
    stopTabTarget: { sessionId: "s1", tabId },
    spine: { sessions: [{ id: "s1", tabs }] },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  closeStopTab.mockClear()
  stopTab.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmStopTabDialog", () => {
  it("names the session it ends and keeps the tab, and Stop stops that tab", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude" }),
      tab({ id: "b2", provider: "codex" }),
    ])
    render(<ConfirmStopTabDialog />)
    expect(screen.getByText("Stop tab?")).toBeTruthy()
    const body = screen.getByText(/This ends the/).textContent ?? ""
    expect(body).toContain("This ends the codex session")
    expect(body).toContain("stays in the strip")
    expect(body).not.toContain("detaches")
    fireEvent.click(screen.getByRole("button", { name: "Stop tab" }))
    expect(stopTab).toHaveBeenCalledWith("s1", "b2")
    expect(closeStopTab).toHaveBeenCalled()
  })

  it("warns the agent detaches when this is its last running tab", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmStopTabDialog />)
    expect(screen.getByText(/last running tab, so the agent detaches/)).toBeTruthy()
  })

  it("closes itself when the tab is gone, and Cancel stops nothing", () => {
    seed("gone", [tab({ id: "s1" })])
    render(<ConfirmStopTabDialog />)
    expect(closeStopTab).toHaveBeenCalled()

    closeStopTab.mockClear()
    seed("s1", [tab({ id: "s1" })])
    render(<ConfirmStopTabDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(closeStopTab).toHaveBeenCalled()
    expect(stopTab).not.toHaveBeenCalled()
  })
})
