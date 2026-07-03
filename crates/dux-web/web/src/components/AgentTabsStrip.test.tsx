// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

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
const { AgentTabsStrip } = await import("./AgentTabsStrip")

function session(): SessionView {
  return {
    id: "s1",
    tabs: [
      { id: "s1", provider: "claude", order: 0, working: false, has_output: false, has_live_process: true },
      { id: "b2", provider: "codex", order: 1, working: false, has_output: false, has_live_process: true },
    ],
  } as unknown as SessionView
}

beforeEach(() => {
  installBootStubs()
  mockState = {
    bootstrap: { available_providers: ["claude", "codex"] },
    createTabInFlight: [],
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AgentTabsStrip", () => {
  it("exposes exactly one close affordance per pill (the ⋯ menu, no standalone ✕)", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    // The old always-visible ✕ button (aria-label "Close tab") is gone; closing
    // lives only inside the ⋯ menu's "Close tab…" item (not rendered until open).
    expect(screen.queryByLabelText("Close tab")).toBeNull()
    expect(screen.getAllByLabelText("Tab actions")).toHaveLength(2)
  })

  it("disables the New tab button at the per-agent cap", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={2} />)
    expect(screen.getByLabelText("New tab")).toHaveProperty("disabled", true)
  })
})
