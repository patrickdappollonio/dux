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
// `openDeleteTerminal` is spied on so the agentless terminal screen's Close…
// entry can be asserted to route into the existing confirm-dialog target
// (ConfirmDeleteTerminalDialog reads `deleteTerminalTarget`; the dialog itself
// has its own tests) rather than closing anything directly.
const openDeleteTerminalMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: addTabMock,
    navigateUp: navigateUpMock,
    openDeleteTerminal: openDeleteTerminalMock,
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
  openDeleteTerminalMock.mockClear()
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
    const item = screen.getByText(/^New agent tab for /)
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
    const item = screen.getByText(/^New agent tab for /)
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
    fireEvent.click(screen.getByText(/^New agent tab for /))
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

describe("MobileShell standalone terminals", () => {
  function standaloneSpine(): DuxState["spine"] {
    return {
      projects: [],
      sessions: [],
      terminals: [
        {
          id: "solo-1",
          owner: { kind: "standalone", cwd_label: "~/code" },
          label: "Terminal 1",
          has_output: true,
          foreground_cmd: null,
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as DuxState["spine"]
  }

  // The state a phone is in while looking at a standalone terminal: focused on
  // it, on the terminal screen, with no session anywhere in sight.
  function standaloneState(): DuxState {
    return makeState({
      spine: standaloneSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      selectedTarget: {
        kind: "terminal",
        terminalId: "solo-1",
        owner: { kind: "standalone" },
      },
      selectedSessionId: null,
      mobileScreen: "terminal",
      changes: { sessionId: null, phase: "empty", staged: [], unstaged: [] },
      startedDormantTabs: [],
      terminalEpoch: 0,
    } as unknown as Partial<DuxState>)
  }

  it("reaches a real screen on a phone, naming the directory, not the hub", () => {
    // The failure this pins: a kind of owner with no screen of its own fell
    // through to the agent branch, which needs a session, and rendered the HUB.
    // On a phone that looks exactly like tapping the terminal did nothing.
    mockState = standaloneState()
    render(<MobileShell />)
    // The screen's own header, naming where the terminal is. The hub would have
    // no Back control and no directory crumb at all.
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByText("~/code")).toBeTruthy()
  })

  it("sends the standalone terminal screen's chevron up instead of back", () => {
    mockState = standaloneState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Back"))
    expect(historyBack).not.toHaveBeenCalled()
    expect(navigateUpMock).toHaveBeenCalledTimes(1)
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

describe("MobileShell Changes-pane show button absence", () => {
  it("renders no 'Show Changes pane' button even when the preference hides the pane", () => {
    // The reopen button is a desktop-only affordance (InsetHeader mounts only
    // in DesktopShell); the mobile shell reaches Changes through its own
    // screen, so a hidden desktop pane must not grow a stray control here.
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux #1",
        dux_version: "v9.9.9",
        show_changes_pane: false,
      },
    })
    render(<MobileShell />)
    expect(
      screen.queryByRole("button", { name: /show changes pane/i }),
    ).toBeNull()
  })
})

describe("MobileShell hideable top bar (ui.mobile_top_bar)", () => {
  function terminalState(overrides: Record<string, unknown> = {}): DuxState {
    return makeState({
      spine: makeSessionSpine(2),
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
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("shows the header and tab strip by default (preference absent falls back to on)", () => {
    mockState = terminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByLabelText("Session actions")).toBeTruthy()
    expect(screen.getAllByRole("tab").length).toBeGreaterThan(0)
  })

  it("hides the header AND the tab strip when the preference is off", () => {
    mockState = terminalState({
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        mobile_top_bar: false,
      },
    })
    render(<MobileShell />)
    expect(screen.queryByLabelText("Back")).toBeNull()
    expect(screen.queryByLabelText("Session actions")).toBeNull()
    expect(screen.queryAllByRole("tab").length).toBe(0)
  })

  it("an optimistic override hides the bar before the bootstrap confirms", () => {
    mockState = terminalState({ mobileTopBarOverride: false })
    render(<MobileShell />)
    expect(screen.queryByLabelText("Back")).toBeNull()
  })

  // The agentless (project/standalone) terminal screens share the same
  // preference; one state builder serves the hidden test and its positive
  // control so the two can only ever differ in the preference itself.
  function agentlessState(bootstrap: Record<string, unknown>): DuxState {
    return terminalState({
      spine: {
        projects: [
          { id: "p1", name: "Repo", path: "/tmp/p1", default_provider: "claude" },
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
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        ...bootstrap,
      },
    })
  }

  it("shows the agentless terminal screen's header while the preference is on (positive control)", () => {
    mockState = agentlessState({ mobile_top_bar: true })
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByText("Repo")).toBeTruthy()
  })

  it("hides the agentless terminal screen's header through the same preference", () => {
    mockState = agentlessState({ mobile_top_bar: false })
    render(<MobileShell />)
    expect(screen.queryByLabelText("Back")).toBeNull()
  })
})

describe("MobileShell terminal-screen macro trigger", () => {
  // On phones the macro quick-picker's trigger is a header icon button, not
  // TerminalPane's floating overlay (which sat on top of the PTY text and
  // made it unreadable). The suite shrinks the viewport like the quick-toggle
  // tests so the shell composes exactly as a phone would.
  const desktopWidth = window.innerWidth
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  })
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  function terminalState(overrides: Record<string, unknown> = {}): DuxState {
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
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("puts the macro trigger in the agent terminal screen's header", () => {
    mockState = terminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
  })

  it("hides the macro trigger together with the hidden top bar", () => {
    // Hiding the top bar states an intent (more space), so the macro trigger
    // goes with the header; restore is the show-bars button or Preferences.
    mockState = terminalState({ mobileTopBarOverride: false })
    render(<MobileShell />)
    expect(screen.queryByLabelText("Run a macro")).toBeNull()
  })

  it("puts the macro trigger in the agentless terminal screen's header too", () => {
    // The floating trigger used to render over project/standalone terminals
    // as well, so their header gets the same treatment as the agent screen.
    mockState = terminalState({
      spine: {
        projects: [
          { id: "p1", name: "Repo", path: "/tmp/p1", default_provider: "claude" },
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
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
  })
})

describe("MobileShell quick toggles in the terminal-screen ⋯ menu", () => {
  // The toggles are gated on `context === "terminal" && isMobile`, and
  // `useIsMobile` reads `window.innerWidth`, so these tests shrink it below
  // the 768px breakpoint (mirroring the TerminalPane compose-bar tests).
  const desktopWidth = window.innerWidth
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  })
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  function terminalState(overrides: Record<string, unknown> = {}): DuxState {
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
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("offers Hide top bar and Hide terminal keys on the terminal screen", () => {
    mockState = terminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText("Hide top bar")).toBeTruthy()
    expect(screen.getByText("Hide terminal keys")).toBeTruthy()
  })

  it("labels flip to Show when a bar is already hidden", () => {
    mockState = terminalState({
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        // The top bar stays visible so its ⋯ menu is still reachable; the
        // ACCESSORY preference is the hidden one whose label must flip.
        mobile_top_bar: true,
        mobile_accessory_bar: false,
      },
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText("Hide top bar")).toBeTruthy()
    expect(screen.getByText("Show terminal keys")).toBeTruthy()
  })

  it("tapping Hide top bar persists through the generic settings PATCH", () => {
    mockState = terminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText("Hide top bar"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        // `quiet: true` asks the server to skip the "Settings updated."
        // status for this write; the bar disappearing is the feedback.
        body: JSON.stringify({ ui: { mobile_top_bar: false }, quiet: true }),
      }),
    )
  })

  it("renders no toggles at desktop width even in the terminal context", () => {
    // The gate is context AND isMobile: the chrome these toggles hide is
    // mobile-only, so a desktop viewport must never see them even when a
    // terminal-context menu renders.
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    mockState = terminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText(/^New agent tab for /)).toBeTruthy()
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })

  it("survives its own menu unmounting when Hide top bar removes the header", () => {
    // Tapping "Hide top bar" hides the header that CONTAINS the open menu's
    // trigger. Simulate the confirmed state landing (the mocked store state
    // flips) and re-render: the menu and header must simply be gone, with no
    // crash from unmounting under an open menu.
    mockState = terminalState()
    const view = render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText("Hide top bar"))
    mockState = terminalState({ mobileTopBarOverride: false })
    view.rerender(<MobileShell />)
    expect(screen.queryByLabelText("Session actions")).toBeNull()
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByLabelText("Back")).toBeNull()
  })

  it("gives the submenu trigger rows the same phone touch-target height as sibling items", () => {
    // DropdownMenuItem carries min-h-11 (44px) on phones, undone on desktop
    // via md:min-h-0. The two DropdownMenuSubTrigger rows (New agent tab,
    // Project) must carry the exact same pair, or they render visibly shorter
    // than every sibling item in the same open menu.
    mockState = terminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    const item = screen.getByText("Hide top bar").closest('[role="menuitem"]')!
    const subTriggers = [
      screen.getByText(/^New agent tab for /).closest('[role="menuitem"]')!,
      screen.getByText(/^Project /).closest('[role="menuitem"]')!,
    ]
    for (const cls of ["min-h-11", "md:min-h-0"]) {
      expect(item.className).toContain(cls)
      for (const trigger of subTriggers) {
        expect(trigger.className).toContain(cls)
      }
    }
  })

  it("does not leak the toggles into the hub's row menus", () => {
    // The hub row's ⋯ menu shares AgentActionsMenu with the terminal screen;
    // the toggles are terminal-context-only, so they must not appear here.
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText(/^New agent tab for /)).toBeTruthy()
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })
})

describe("MobileShell agentless terminal screen ⋯ menu", () => {
  // The project and standalone terminal screens used to carry NO menu at all,
  // so hiding the bars from them meant a trip through Preferences. That
  // decision changed: every mobile terminal screen carries the quick toggles.
  // The toggles read `useIsMobile`, so the viewport shrinks below the 768px
  // breakpoint exactly like the agent-screen quick-toggle suite above.
  const desktopWidth = window.innerWidth
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  })
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  // A phone focused on a PROJECT terminal: no session anywhere in sight.
  function projectTerminalState(
    bootstrap: Record<string, unknown> = {},
  ): DuxState {
    return makeState({
      spine: {
        projects: [
          { id: "p1", name: "Repo", path: "/tmp/p1", default_provider: "claude" },
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
      mobileScreen: "terminal",
      changes: { sessionId: null, phase: "empty", staged: [], unstaged: [] },
      startedDormantTabs: [],
      terminalEpoch: 0,
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        ...bootstrap,
      },
    } as unknown as Partial<DuxState>)
  }

  // The same phone on a STANDALONE terminal: the other agentless wrapper.
  function standaloneTerminalState(): DuxState {
    return makeState({
      spine: {
        projects: [],
        sessions: [],
        terminals: [
          {
            id: "solo-1",
            owner: { kind: "standalone", cwd_label: "~/code" },
            label: "Terminal 1",
          },
        ],
        sidebar: { groups: [], agentless_start: null },
      },
      selectedTarget: {
        kind: "terminal",
        terminalId: "solo-1",
        owner: { kind: "standalone" },
      },
      selectedSessionId: null,
      mobileScreen: "terminal",
      changes: { sessionId: null, phase: "empty", staged: [], unstaged: [] },
      startedDormantTabs: [],
      terminalEpoch: 0,
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
    } as unknown as Partial<DuxState>)
  }

  it("renders the ⋯ trigger in the project terminal screen's header", () => {
    mockState = projectTerminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Terminal actions")).toBeTruthy()
  })

  it("renders the ⋯ trigger on the standalone terminal screen too", () => {
    mockState = standaloneTerminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Terminal actions")).toBeTruthy()
  })

  it("offers both bar quick toggles, exactly as the agent screen words them", () => {
    mockState = projectTerminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    expect(screen.getByText("Hide top bar")).toBeTruthy()
    expect(screen.getByText("Hide terminal keys")).toBeTruthy()
  })

  it("labels flip to Show when a bar is already hidden", () => {
    mockState = projectTerminalState({
      // The top bar stays visible so its ⋯ menu is still reachable; the
      // ACCESSORY preference is the hidden one whose label must flip.
      mobile_top_bar: true,
      mobile_accessory_bar: false,
    })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    expect(screen.getByText("Hide top bar")).toBeTruthy()
    expect(screen.getByText("Show terminal keys")).toBeTruthy()
  })

  it("tapping Hide top bar persists through the generic settings PATCH", () => {
    mockState = projectTerminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    fireEvent.click(screen.getByText("Hide top bar"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({ ui: { mobile_top_bar: false }, quiet: true }),
      }),
    )
  })

  it("routes Close… into the existing confirm-dialog target, per terminal id", () => {
    // The row menu's one real action, reproduced here so the screen is
    // self-sufficient: it opens the ConfirmDeleteTerminalDialog target, it
    // does not close anything directly.
    mockState = projectTerminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    fireEvent.click(screen.getByText("Close…"))
    expect(openDeleteTerminalMock).toHaveBeenCalledExactlyOnceWith("pt-1")
  })

  it("carries no agent-only entries: this menu is the terminal's, not a session's", () => {
    mockState = projectTerminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    expect(screen.queryByText(/^New agent tab for /)).toBeNull()
    expect(screen.queryByText("Rename agent…")).toBeNull()
  })
})

describe("MobileShell agent header identity", () => {
  // The phone header used to show the BRANCH alone, in mono, so it never said
  // which project or which assistant you were talking to. It now carries the
  // same two facts the desktop header does, as a two-line stack inside the SAME
  // h-11 the one line used.
  function agentHeaderState(): DuxState {
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
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
    })
  }

  it("stacks the agent name over `project · provider`", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    const name = screen.getByText("main")
    const caption = screen.getByText("Repo · claude")
    expect(name.className).toContain("text-sm")
    expect(caption.className).toContain("text-[11px]")
    expect(caption.className).toContain("text-muted-foreground")
    // Two separate lines, not one run of text.
    expect(name).not.toBe(caption)
    expect(name.parentElement).toBe(caption.parentElement)
  })

  it("does not grow the header: same h-11, tight leading, and the name truncates", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    const header = screen.getByLabelText("Back").closest("header")
    expect(header?.className).toContain("h-11")
    const name = screen.getByText("main")
    const caption = screen.getByText("Repo · claude")
    // 14px + 11px at leading-tight is about 31px, inside the 44px header.
    expect(name.className).toContain("leading-tight")
    expect(caption.className).toContain("leading-tight")
    expect(name.className).toContain("truncate")
  })

  it("drops the mono branch line the old header showed", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    expect(screen.getByText("main").className).not.toContain("font-mono")
  })
})

// THE PHONE HEADER'S CONTROLS ARE ONE FAMILY. They used to be three unrelated
// treatments sitting side by side: a ghost size-10 macro trigger, an outline
// size="sm" ±N (whose `sm` token also swapped the corner RADIUS), and a ghost
// size-10 ⋯.
//
// jsdom has no layout engine, so `getBoundingClientRect` is all zeros here and
// these assert the height TOKEN rather than the rendered pixel. The rendered
// pixels were measured separately in the preview container (all four controls
// at exactly 36px tall, 44px+ wide); this test is what keeps them from
// drifting apart again without anyone re-running that.
describe("MobileShell terminal header control family", () => {
  function headerState(): DuxState {
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
      mobileTopBarOverride: null,
      mobileAccessoryBarOverride: null,
    })
  }

  const controls = () => ({
    macro: screen.getByLabelText("Run a macro"),
    changes: screen.getByLabelText(/changed files$/),
    actions: screen.getByLabelText("Session actions"),
    back: screen.getByLabelText("Back"),
  })

  it("renders every header control at the SAME explicit height", () => {
    mockState = headerState()
    render(<MobileShell />)
    const found = controls()

    // One height token across all four, so an icon-only control and the one
    // carrying text cannot disagree. This is the rule: a control's height is
    // set explicitly, never inherited from its padding or its content.
    for (const [name, el] of Object.entries(found)) {
      expect(el.className, `${name} must carry the h-9 (36px) height`).toContain(
        "h-9"
      )
    }

    // And nothing re-introduces a content- or square-derived height: no
    // `min-h-*` prop-up (what `size="sm"` needed) and no `size-N` square
    // (what `size="icon"` is), either of which ties height to width.
    // Both patterns anchor on a class BOUNDARY. A loose `\bsize-\d` also
    // matches the base button's `[&_svg:not([class*='size-'])]:size-4`, which
    // sizes the ICON inside the control and has nothing to do with the
    // control's own box.
    for (const [name, el] of Object.entries(found)) {
      expect(el.className, `${name} must not force a min height`).not.toMatch(
        /(?:^|\s)min-h-\d/
      )
      expect(el.className, `${name} must not be a fixed square`).not.toMatch(
        /(?:^|\s)size-\d/
      )
    }
  })

  it("gives every header control the 44px per-axis width floor", () => {
    mockState = headerState()
    render(<MobileShell />)
    for (const [name, el] of Object.entries(controls())) {
      expect(el.className, `${name} must carry min-w-11`).toContain("min-w-11")
    }
  })

  // The action cluster is outline (the treatment the desktop AppMenu cog and
  // the Show-Changes button beside it already use for an action cluster). The
  // back chevron deliberately stays ghost because it is NAVIGATION, not an
  // action on this screen; it matches on geometry and differs on weight.
  it("makes the three actions outline and leaves Back ghost", () => {
    mockState = headerState()
    render(<MobileShell />)
    const { macro, changes, actions, back } = controls()

    for (const [name, el] of Object.entries({ macro, changes, actions })) {
      expect(el.className, `${name} must be the outline variant`).toContain(
        "border-border"
      )
    }
    expect(back.className).not.toContain("border-border")
  })

  // ±N is the deliberate exception to phones-prefer-icon-only: the number is
  // DATA, not a label, and no icon can say "3 changes". It keeps its text and
  // therefore its auto width, while the height stays pinned above.
  it("keeps the changed-count text on the ±N control", () => {
    mockState = headerState()
    render(<MobileShell />)
    expect(controls().changes.textContent).toMatch(/^±\d+$/)
  })
})
