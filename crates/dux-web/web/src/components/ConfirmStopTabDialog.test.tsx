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
    spine: {
      sessions: [
        {
          id: "s1",
          title: null,
          workspace: { kind: "managed", branch_name: "fix-auth" },
          tabs,
        },
      ],
    },
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
  // Refused while somebody else is attached, it stays open naming them and
  // stops only through Stop tab anyway, which goes ahead over them.
  it("names the tab and the agent as chips, keeps the tab, and Stop stops it, over who is attached only once they are named", async () => {
    stopTab.mockResolvedValueOnce([
      {
        surface: "browser",
        device: "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
        address: "10.0.0.7",
        verified: false,
        driving: true,
        target: { kind: "tab", id: "b2", agent: "s1" },
      },
    ])
    seed("b2", [
      tab({ id: "s1", provider: "claude" }),
      tab({ id: "b2", provider: "codex" }),
    ])
    render(<ConfirmStopTabDialog />)
    expect(screen.getByText("Stop tab?")).toBeTruthy()
    const body = screen.getByText(/This ends its session/).textContent ?? ""
    expect(body).toContain("Stop the Codex tab on fix-auth?")
    expect(screen.getByText("Codex", { selector: "code" })).toBeTruthy()
    expect(screen.getByText("fix-auth", { selector: "code" })).toBeTruthy()
    expect(body).toContain("stays in the strip")
    expect(body).not.toContain("detaches")
    fireEvent.click(screen.getByRole("button", { name: "Stop tab" }))
    expect(stopTab).toHaveBeenCalledWith("s1", "b2", false)
    const override = await screen.findByRole("button", { name: "Stop tab anyway" })
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }))
    expect(screen.getByText("Firefox on Linux").tagName).toBe("CODE")
    expect(closeStopTab).not.toHaveBeenCalled()

    stopTab.mockResolvedValueOnce(null)
    fireEvent.click(override)
    expect(stopTab).toHaveBeenLastCalledWith("s1", "b2", true)
    await vi.waitFor(() => expect(closeStopTab).toHaveBeenCalled())
  })

  it("warns the agent detaches when this is its last running tab", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmStopTabDialog />)
    expect(screen.getByText(/last running tab, so the agent detaches/)).toBeTruthy()
  })

  it("closes itself when the tab is gone, and Cancel has focus and stops nothing", () => {
    seed("gone", [tab({ id: "s1" })])
    render(<ConfirmStopTabDialog />)
    expect(closeStopTab).toHaveBeenCalled()

    closeStopTab.mockClear()
    seed("s1", [tab({ id: "s1" })])
    render(<ConfirmStopTabDialog />)
    const cancel = screen.getByRole("button", { name: "Cancel" })
    expect(document.activeElement).toBe(cancel)
    fireEvent.click(cancel)
    expect(closeStopTab).toHaveBeenCalled()
    expect(stopTab).not.toHaveBeenCalled()
  })
})
