// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

let mockState: DuxState
const addTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: (...args: unknown[]) => addTabMock(...args),
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
const { AgentTabsStrip } = await import("./AgentTabsStrip")

function session(): SessionView {
  return {
    id: "s1",
    project_id: "p1",
    provider: "claude",
    tabs: [
      { id: "s1", provider: "claude", order: 0, working: false, has_output: false, has_live_process: true },
      { id: "b2", provider: "codex", order: 1, working: false, has_output: false, has_live_process: true },
    ],
  } as unknown as SessionView
}

beforeEach(() => {
  installBootStubs()
  addTabMock.mockClear()
  mockState = {
    bootstrap: { available_providers: ["claude", "codex", "opencode"] },
    spine: {
      projects: [{ id: "p1", default_provider: "codex" }],
    },
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

  it("also disables the provider-picker caret at the per-agent cap", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={2} />)
    expect(screen.getByLabelText("Choose provider for new tab")).toHaveProperty(
      "disabled",
      true,
    )
  })

  it("clicking the main + quick-adds the project default provider (no provider arg)", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("New tab"))
    expect(addTabMock).toHaveBeenCalledWith("s1")
  })

  it("the caret opens a menu listing configured providers with the default marked", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("Choose provider for new tab"))
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("claude")).toBeTruthy()
    expect(menu.getByText("codex")).toBeTruthy()
    expect(menu.getByText("opencode")).toBeTruthy()
    // The project default (codex, per the mocked spine) is tagged "default".
    expect(menu.getByText("default")).toBeTruthy()
  })

  it("picking a provider from the caret menu adds a tab targeting it", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getByLabelText("Choose provider for new tab"))
    const menu = within(await screen.findByRole("menu"))
    fireEvent.click(menu.getByText("opencode"))
    expect(addTabMock).toHaveBeenCalledWith("s1", "opencode")
  })
})
