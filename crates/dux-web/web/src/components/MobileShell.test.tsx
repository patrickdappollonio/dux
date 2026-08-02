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
// `navigateUp` is spied on so the Back chevrons can be asserted to name a
// destination rather than step the browser's history (see the up-navigation
// suite at the bottom of this file).
const navigateUpMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: addTabMock,
    navigateUp: navigateUpMock,
  }
})
const historyBack = vi.fn()

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
const { NewAgentPickerDialog } = await import("./NewAgentPickerDialog")

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
  navigateUpMock.mockClear()
  historyBack.mockClear()
  // jsdom's own history is real; replace only `back` so a stray relative step
  // is observable instead of silently doing nothing.
  vi.spyOn(history, "back").mockImplementation(historyBack)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
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

describe("MobileShell project terminals", () => {
  function projectTerminalSpine(): DuxState["spine"] {
    return {
      projects: [
        {
          id: "p1",
          name: "Repo",
          path: "/tmp/p1",
          path_missing: false,
          default_provider: "claude",
          current_branch: "main",
          branch_status: "leading",
        },
      ],
      sessions: [],
      terminals: [
        {
          id: "pt-1",
          owner: { kind: "project", project_id: "p1" },
          label: "Terminal 2",
          has_output: true,
          foreground_cmd: null,
        },
      ],
      sidebar: {
        groups: [{ project_id: "p1", name: "Repo", orphaned: false, session_ids: [] }],
        agentless_start: null,
      },
    } as unknown as DuxState["spine"]
  }

  it("renders the project terminal row under the project block on the hub", () => {
    // T14's mobile half: before this, a project terminal rendered nowhere in
    // the hub, invisible and unreachable on a phone.
    mockState = makeState({
      spine: projectTerminalSpine(),
      bootstrap: { title: "dux", dux_version: "v1" },
    })
    render(<MobileShell />)
    // Idle in the sidebar reads a plain "Terminal" (the "Terminal N" label still
    // identifies it in the tooltip and the task manager); the row rendering on
    // the hub is what this guards.
    expect(screen.getByText("Terminal")).toBeTruthy()
  })

  it("offers 'New project terminal' in the project ⋯ menu", () => {
    // Agent-less project actions live in the New-agent picker's per-project ⋯.
    mockState = makeState({
      spine: projectTerminalSpine(),
      bootstrap: { title: "dux", dux_version: "v1" },
      newAgentPickerOpen: true,
    })
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getByLabelText("Project actions"))
    const item = screen.getByText("New project terminal")
    expect(
      item.closest('[role="menuitem"]')?.getAttribute("aria-disabled"),
    ).not.toBe("true")
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

// The in-app Back chevrons and the not-found screen's way out are UP controls,
// not history steps. A deep-link boot pushes nothing, so on those screens dux's
// own first entry IS the screen being shown, and a relative step from there
// leaves the application: the reported bug, through a different door. Each
// control names its destination instead and lets the store rewrite the URL.
describe("MobileShell up navigation never steps history", () => {
  function upState(overrides: Record<string, unknown> = {}): DuxState {
    return makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      selectedSessionId: "s1",
      mobileScreen: "terminal",
      changes: { sessionId: "s1", phase: "loaded", staged: [], unstaged: [] },
      startedDormantTabs: [],
      terminalEpoch: 0,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("sends the agent screen's chevron up instead of back", () => {
    mockState = upState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Back"))
    expect(historyBack).not.toHaveBeenCalled()
    expect(navigateUpMock).toHaveBeenCalledTimes(1)
  })

  it("sends the project terminal screen's chevron up instead of back", () => {
    mockState = upState({
      spine: {
        projects: [
          {
            id: "p1",
            name: "Repo",
            path: "/tmp/p1",
            default_provider: "claude",
          },
        ],
        sessions: [],
        terminals: [
          {
            id: "pt-1",
            owner: { kind: "project", project_id: "p1" },
            label: "Terminal 2",
          },
        ],
        sidebar: { groups: [], agentless_start: null },
      },
      selectedTarget: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
      selectedSessionId: null,
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Back"))
    expect(historyBack).not.toHaveBeenCalled()
    expect(navigateUpMock).toHaveBeenCalledTimes(1)
  })

  it("sends the changes screen's chevron up instead of back", () => {
    mockState = upState({ mobileScreen: "changes" })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Back"))
    expect(historyBack).not.toHaveBeenCalled()
    expect(navigateUpMock).toHaveBeenCalledTimes(1)
  })

  it("leaves the not-found screen without keeping it in history", () => {
    // Arriving at not-found is a correction of a bad URL, not a position worth
    // keeping: pushing home from here would put the user's next Back straight
    // back onto the dead end they just left.
    mockState = makeState({ routeNotFound: { kind: "agent", sessionId: "s9" } })
    render(<MobileShell />)
    fireEvent.click(screen.getByText("Back to agents"))
    expect(navigateUpMock).toHaveBeenCalledTimes(1)
  })
})

describe("MobileShell not-found screen", () => {
  it("says the agent is gone instead of quietly showing the hub", () => {
    // A URL naming a deleted agent: the route has no target, so without this
    // branch the shell would fall through to the hub while the address bar
    // still named an agent that is not on screen.
    mockState = makeState({ routeNotFound: { kind: "agent", sessionId: "s9" } })
    render(<MobileShell />)
    expect(screen.getByText("Agent not found")).toBeTruthy()
    expect(screen.getByText("s9")).toBeTruthy()
    expect(screen.queryByText("agent sessions")).toBeNull()
  })
})
