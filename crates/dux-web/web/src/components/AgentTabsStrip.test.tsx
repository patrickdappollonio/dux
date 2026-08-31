// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import { ONLY_TAB_CLOSE_REFUSAL } from "@/lib/agentTabs"

let mockState: DuxState
const addTabMock = vi.fn()
const openCloseTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: (...args: unknown[]) => addTabMock(...args),
    openCloseTab: (...args: unknown[]) => openCloseTabMock(...args),
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
    slot_tab_id: "s1",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "",
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: "",
    },
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
  openCloseTabMock.mockClear()
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
    // There is no standalone ✕ button (aria-label "Close tab"); closing lives
    // only inside the ⋯ menu's "Close tab…" item (not rendered until open).
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

  // The first tab is closable: closing it hands the slot to the next tab, so its
  // menu item is an ordinary enabled one, exactly like an extra tab's.
  // An agent's ONLY tab has no successor to hand the slot to, so the server
  // refuses that close. The menu refuses it too, in the same words and before
  // any dialog: the alternative is a confirmation promising a detach followed
  // by a 400.
  it("disables the Close item on an agent's only tab and says why", async () => {
    const sole = session()
    sole.tabs = [sole.tabs[0]]
    render(<AgentTabsStrip session={sole} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[0])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item?.getAttribute("aria-disabled")).toBe("true")
    expect(menu.getByText(ONLY_TAB_CLOSE_REFUSAL)).toBeTruthy()
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).not.toHaveBeenCalled()
  })

  it("keeps the first tab's Close item enabled and acting", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[0])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item).toBeTruthy()
    expect(item?.getAttribute("aria-disabled")).not.toBe("true")
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).toHaveBeenCalledWith("s1", "s1")
  })

  it("keeps an extra tab's Close item enabled and acting", async () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    fireEvent.click(screen.getAllByLabelText("Tab actions")[1])
    const menu = within(await screen.findByRole("menu"))
    const item = menu.getByText("Close tab…").closest("[role='menuitem']")
    expect(item?.getAttribute("aria-disabled")).not.toBe("true")
    fireEvent.click(menu.getByText("Close tab…"))
    expect(openCloseTabMock).toHaveBeenCalledWith("s1", "b2")
  })

  it("marks the flagged tab's pill with an attention dot", () => {
    const s = session()
    s.tabs[1].needs_attention = true
    render(<AgentTabsStrip session={s} activeTabId="s1" maxTabs={20} />)
    expect(screen.getAllByLabelText("Needs attention")).toHaveLength(1)
  })

  it("renders no attention dot when no tab needs attention", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    expect(screen.queryByLabelText("Needs attention")).toBeNull()
  })
})

describe("AgentTabsStrip phone height", () => {
  // A deliberate per-axis relaxation of the 40px touch-target floor (see the
  // tenet and the justification at the pill): the pill drops to 36px VERTICALLY
  // because its vertical neighbours are the header above (no tap target beside a
  // pill) and the PTY below (a cheap mis-tap), while every horizontal size is
  // kept because the horizontal neighbours are other tabs.
  it("pins the pill to 36px tall with no padding of its own", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    for (const pill of screen.getAllByRole("tab")) {
      expect(pill.className).toContain("max-md:min-h-9")
      expect(pill.className).toContain("max-md:py-0")
      expect(pill.className).not.toContain("max-md:min-h-11")
    }
  })

  it("halves the strip's own padding so the strip gets shorter, not just the pill", () => {
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const strip = screen.getAllByRole("tab")[0].parentElement
    expect(strip?.className).toContain("max-md:py-0.5")
  })

  it("keeps every control's 44px WIDTH while relaxing its height", () => {
    // `size-11` on any of these would force the pill or the strip back to 44px
    // tall; dropping the width would put a mis-tap on a neighbouring tab or on
    // the wrong provider.
    render(<AgentTabsStrip session={session()} activeTabId="s1" maxTabs={20} />)
    const controls = [
      screen.getByLabelText("New tab"),
      screen.getByLabelText("Choose provider for new tab"),
      ...screen.getAllByLabelText("Tab actions"),
    ]
    for (const control of controls) {
      expect(control.className).toContain("max-md:w-11")
      expect(control.className).not.toContain("max-md:size-11")
    }
  })
})
