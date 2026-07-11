// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override only `useDux` so the mobile drawer header's wiring (`bootstrap.title`
// to `resolveInstanceTitle` to the rendered wordmark) is exercised end to end,
// keeping the version/subtitle line intact below it. `addTab` is ALSO overridden
// (as a spy) so the "Add tab" menu item test can assert it without a real
// network request.
let mockState: DuxState
const addTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, addTab: addTabMock }
})

// jsdom lacks localStorage/fetch/matchMedia as globals; the real store boots on
// import. Stub them before the component (and the store behind it) loads so the
// render tests are hermetic and off the network.
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
}
installBootStubs()
const { MobileShell } = await import("./MobileShell")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: { title: "dux #1", dux_version: "v9.9.9" },
    selectedTarget: null,
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    createTabInFlight: [],
    mobileScreen: "home",
    ...overrides,
  } as unknown as DuxState
}

// A minimal one-project/one-session spine, with the session's tab count
// configurable so the "Add tab" reachability + cap-disable can be exercised at
// any tab count (including the common 1-tab case, per G7).
function makeSessionSpine(tabCount: number): DuxState["spine"] {
  const tabs = Array.from({ length: tabCount }, (_, i) => ({
    id: i === 0 ? "s1" : `extra-${i}`,
    provider: "claude",
    order: i,
    working: false,
    has_output: false,
    has_live_process: true,
  }))
  return {
    projects: [
      {
        id: "p1",
        name: "Repo",
        path: "/tmp/p1",
        default_provider: "claude",
        current_branch: "main",
        branch_status: "leading",
      },
    ],
    sessions: [
      {
        id: "s1",
        project_id: "p1",
        title: null,
        provider: "claude",
        branch_name: "main",
        worktree_path: "/tmp/p1",
        status: "active",
        auto_reopen_enabled: false,
        terminals: [],
        tabs,
        has_output: false,
        working: false,
      },
    ],
    sidebar: { groups: [{ project_id: "p1", name: "Repo", orphaned: false }], agentless_start: null },
  } as unknown as DuxState["spine"]
}

beforeEach(() => {
  installBootStubs()
  addTabMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("MobileShell home row agent ⋯ menu — Add tab (G7)", () => {
  // Mirrors the desktop sidebar's fix: before it, the web had no way to reach a
  // session's first extra tab (the in-strip "+" only renders at 2+ tabs).
  it("is present and enabled for a session with only one tab, and calls addTab", () => {
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude", "codex"],
      },
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    const item = screen.getByText("New agent tab…")
    expect(
      item.closest('[role="menuitem"]')?.getAttribute("aria-disabled"),
    ).not.toBe("true")
  })

  it("disables Add tab once the session is at the per-agent tab cap", () => {
    mockState = makeState({
      spine: makeSessionSpine(2),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        agent_tabs_max: 2,
        available_providers: ["claude", "codex"],
      },
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    const item = screen.getByText("New agent tab…")
    expect(item.closest('[role="menuitem"]')?.getAttribute("aria-disabled")).toBe(
      "true",
    )
  })

  it("lists the configured providers with the project default marked, and picking one calls addTab", () => {
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude", "codex", "opencode"],
      },
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText("New agent tab…"))
    // makeSessionSpine's project default_provider is "claude".
    expect(screen.getByText("default")).toBeTruthy()
    expect(screen.getByText("codex")).toBeTruthy()
    fireEvent.click(screen.getByText("codex"))
    expect(addTabMock).toHaveBeenCalledWith("s1", "codex")
  })
})

describe("MobileShell attention dot", () => {
  it("renders the attention dot when the agent needs attention", () => {
    const spine = makeSessionSpine(1) as unknown as {
      sessions: { needs_attention: boolean }[]
    }
    spine.sessions[0].needs_attention = true
    mockState = makeState({
      spine: spine as unknown as DuxState["spine"],
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
    })
    render(<MobileShell />)
    expect(screen.getAllByLabelText("Needs attention").length).toBeGreaterThan(0)
  })

  it("renders no attention dot when the agent does not need attention", () => {
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
    })
    render(<MobileShell />)
    expect(screen.queryByLabelText("Needs attention")).toBeNull()
  })
})

describe("MobileShell drawer header", () => {
  it("renders the configured instance title above the 'agent sessions' subtitle", () => {
    mockState = makeState()
    render(<MobileShell />)
    expect(screen.getByText("dux #1")).toBeTruthy()
    // The subtitle is unchanged, proving the title replaced only the wordmark.
    expect(screen.getByText("agent sessions")).toBeTruthy()
  })

  it("falls back to 'dux' when no title is configured", () => {
    mockState = makeState({
      bootstrap: { dux_version: "v9.9.9" } as DuxState["bootstrap"],
    })
    render(<MobileShell />)
    expect(screen.getByText("dux")).toBeTruthy()
  })
})
