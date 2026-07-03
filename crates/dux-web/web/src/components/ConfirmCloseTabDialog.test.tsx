// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView } from "@/lib/types"

// Override only `useDux` so the dialog reads our seeded spine + close target,
// while the real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
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
const { ConfirmCloseTabDialog } = await import("./ConfirmCloseTabDialog")

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

// Seed the dialog to close `tabId` of a session whose tabs are `tabs`.
function seed(tabId: string, tabs: AgentTabView[]) {
  mockState = {
    closeTabTarget: { sessionId: "s1", tabId },
    spine: {
      sessions: [{ id: "s1", tabs }],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmCloseTabDialog", () => {
  it("warns the agent detaches when closing its last LIVE tab (a dormant sibling doesn't count)", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
    // The provider name is named in the body.
    expect(screen.getByText(/ends the claude session/)).toBeTruthy()
  })

  it("shows no detach warning when a live sibling keeps the agent running", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: true }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(screen.queryByText(/the agent detaches/)).toBeNull()
  })
})
