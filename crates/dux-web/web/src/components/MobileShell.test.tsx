// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import {
  COARSE_POINTER_QUERY,
  stubCoarsePointer,
  type MatchMediaStub,
} from "@/test/matchMedia"

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

// The real pane pulls in xterm's canvas rendering, which jsdom cannot back, and
// it arrives behind `React.lazy`, so nothing it is handed would ever mount in a
// test. The stub renders the OVERLAY it is given, which is where the floating
// theater pill lives on this shell exactly as it does on the desktop one.
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: (props: { overlay?: React.ReactNode }) => (
    <div data-testid="terminal-pane-stub">{props.overlay}</div>
  ),
}))

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
const { registerPaneInputGroup, resetPaneInputGroups } = await import(
  "@/lib/paneInputGroup"
)
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
// any tab count (including the common 1-tab case).
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
        slot_tab_id: "s1",
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "main",
          initial_branch: "",
          branch_provenance: "created",
          source_branch: "",
          worktree_path: "/tmp/p1",
        },
        title: null,
        provider: "claude",
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

describe("MobileShell home row agent ⋯ menu: Add tab", () => {
  // Mirrors the desktop sidebar: the in-strip "+" only renders at 2+ tabs, so
  // this menu item is the way to a session's first extra tab.
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
    // Without this row a project terminal renders nowhere in the hub,
    // invisible and unreachable on a phone.
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
      pendingSlotTab: {},
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

// The phone header's second lane, for a STANDALONE agent: the folder takes the
// slot a project would. The chip model has always been able to draw it, and the
// helper's own test passed, while this call site never handed it the label. The
// assertion lives at the component so the pure test cannot mask it again.
describe("MobileShell phone header for a standalone agent", () => {
  function standaloneSpine(): DuxState["spine"] {
    return {
      projects: [],
      sessions: [
        {
          id: "sa1",
          workspace: {
            kind: "folder",
            folder_path: "/home/someone/notes",
            folder_label: "~/notes",
            repo_status: "working_repo",
            quiet_reason: "",
          },
          title: "notes",
          provider: "claude",
          status: "active",
          auto_reopen_enabled: false,
          tabs: [
            {
              id: "sa1",
              provider: "claude",
              order: 0,
              working: false,
              has_output: false,
              has_live_process: true,
            },
          ],
          has_output: false,
          working: false,
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as DuxState["spine"]
  }

  it("names the folder in the header", () => {
    mockState = makeState({
      spine: standaloneSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      selectedTarget: { kind: "agent", sessionId: "sa1", tabId: "sa1" },
      selectedSessionId: "sa1",
      mobileScreen: "terminal",
      changes: { sessionId: "sa1", phase: "loaded", staged: [], unstaged: [] },
      startedDormantTabs: [],
      pendingSlotTab: {},
      terminalEpoch: 0,
    } as unknown as Partial<DuxState>)
    render(<MobileShell />)
    expect(screen.getByText("~/notes")).toBeTruthy()
  })
})

// The phone gets the same pill, from the same component, inside the same pane
// box: theater is one mode with one floating control, and the drag it carries
// is the gesture the phone needs most.
describe("MobileShell in theater", () => {
  it("floats the draggable pill inside the pane's own box", () => {
    mockState = makeState({
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      theater: true,
    } as unknown as Partial<DuxState>)
    render(<MobileShell />)
    const pill = screen.getByTestId("theater-pill")
    expect(screen.getByTestId("terminal-pane-stub").contains(pill)).toBe(true)
    expect(screen.getByTestId("theater-pill-grip")).toBeTruthy()
  })
})

// The phone reads the same rule as the desktop, through the same helper: a
// healthy dormant first tab starts on selection, and one whose last run failed
// gets the diagnosis card instead. Two surfaces, one answer.
describe("MobileShell dormant first tab", () => {
  function dormantState(lastRunFailed: boolean): DuxState {
    const spine = makeSessionSpine(1) as unknown as {
      sessions: { tabs: Record<string, unknown>[] }[]
    }
    spine.sessions[0].tabs[0].has_live_process = false
    spine.sessions[0].tabs[0].last_run_failed = lastRunFailed
    return makeState({
      spine: spine as unknown as DuxState["spine"],
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
      pendingSlotTab: {},
      terminalEpoch: 0,
    } as unknown as Partial<DuxState>)
  }

  it("shows no card for a healthy dormant first tab", () => {
    mockState = dormantState(false)
    render(<MobileShell />)
    expect(screen.queryByText("Start session")).toBeNull()
  })

  it("shows the card for a first tab whose last run failed", () => {
    mockState = dormantState(true)
    render(<MobileShell />)
    expect(screen.getByText("Start session")).toBeTruthy()
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
      pendingSlotTab: {},
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
    mockState = makeState({
      routeNotFound: { kind: "agent", sessionId: "s9" },
    })
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
    mockState = makeState({
      routeNotFound: { kind: "agent", sessionId: "s9" },
    })
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

describe("MobileShell phone terminal chrome", () => {
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("shows the header and tab strip", () => {
    mockState = terminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByLabelText("Session actions")).toBeTruthy()
    expect(screen.getAllByRole("tab").length).toBeGreaterThan(0)
  })

  // THEATER IS THE ONE WAY TO HIDE THIS CHROME. `ui.mobile_top_bar` used to
  // hide it too, and a config written while that preference existed is still
  // on disk out there; an old server may even still publish the field. It must
  // now change nothing at all, because a hidden header with no way back is
  // exactly what the retirement removed.
  it("ignores a stored mobile_top_bar preference entirely", () => {
    mockState = terminalState({
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        mobile_top_bar: false,
      },
    })
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getAllByRole("tab").length).toBeGreaterThan(0)
    expect(screen.getByLabelText("Session actions")).toBeTruthy()
  })

  // The agentless (project/standalone) terminal screens are the other surface
  // the retired preference used to reach, so they get the same pair.
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

  it("shows the agentless terminal screen's header", () => {
    mockState = agentlessState({})
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
    expect(screen.getByText("Repo")).toBeTruthy()
  })

  it("ignores a stored mobile_top_bar preference there too", () => {
    mockState = agentlessState({ mobile_top_bar: false })
    render(<MobileShell />)
    expect(screen.getByLabelText("Back")).toBeTruthy()
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("puts the macro trigger in the agent terminal screen's header", () => {
    mockState = terminalState()
    render(<MobileShell />)
    expect(screen.getByLabelText("Run a macro")).toBeTruthy()
  })

  it("puts the macro trigger in the agentless terminal screen's header too", () => {
    // Project/standalone terminal headers get the same treatment as the agent
    // screen.
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
  // THE ITEMS ARE THE PANE'S, published under the pty id (see
  // `lib/paneInputGroup.ts`): only the pane knows whether it owns the input and
  // whether a bottom bar of its own is already carrying these rows. The pane is
  // mocked away in this suite, so each case registers what a real one would
  // publish.
  const desktopWidth = window.innerWidth
  let media: MatchMediaStub
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    media = stubCoarsePointer()
    resetPaneInputGroups()
  })
  afterEach(() => {
    resetPaneInputGroups()
    media.restore()
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
      ...overrides,
    } as unknown as Partial<DuxState>)
  }

  it("offers Hide terminal keys on the terminal screen, and nothing for the top bar", () => {
    mockState = terminalState()
    registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText("Hide terminal keys")).toBeTruthy()
    // Theater mode is the one way to hide the phone's chrome, so this menu
    // carries no second flow for it.
    expect(screen.queryByText("Hide top bar")).toBeNull()
  })

  // NEVER INERT, and never DOUBLED: the pane publishes the keys row only while
  // it has no bottom bar of its own carrying it, so a menu whose pane says
  // nothing offers nothing. The item used to render on a fine-pointer laptop
  // and do nothing at all when pressed.
  it("drops the keys toggle when the pane does not offer it", () => {
    media.set(COARSE_POINTER_QUERY, false)
    mockState = terminalState()
    registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: false })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
    // The rest of the menu is still there, so this is a dropped ITEM rather
    // than a menu that failed to open.
    expect(screen.getByText("New agent tab…")).toBeTruthy()
  })

  it("labels flip to Show when the keys bar is already hidden", () => {
    mockState = terminalState({
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
        mobile_accessory_bar: false,
      },
    })
    registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText("Show terminal keys")).toBeTruthy()
  })

  it("tapping Hide terminal keys persists through the generic settings PATCH", () => {
    mockState = terminalState()
    registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText("Hide terminal keys"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        // `quiet: true` asks the server to skip the "Settings updated."
        // status for this write; the bar disappearing is the feedback.
        body: JSON.stringify({
          ui: { mobile_accessory_bar: false },
          quiet: true,
        }),
      }),
    )
  })

  it("renders no toggles, and no group label, when no pane has published", () => {
    // The gate moved to the PANE, which is the only thing that knows whether a
    // bottom bar is already carrying these rows. A menu whose pane published
    // nothing renders the group not at all, label included.
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    mockState = terminalState()
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText("New agent tab…")).toBeTruthy()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })

  it("gives the submenu trigger rows the same phone touch-target height as sibling items", () => {
    // DropdownMenuItem carries min-h-11 (44px) on phones, undone on desktop
    // via md:min-h-0. The two DropdownMenuSubTrigger rows (New agent tab,
    // Project) must carry the exact same pair, or they render visibly shorter
    // than every sibling item in the same open menu. Measured against a bar
    // toggle, which is a plain shared item and therefore the reference height.
    mockState = terminalState()
    registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Session actions"))
    const item = screen
      .getByText("Hide terminal keys")
      .closest('[role="menuitem"]')!
    const subTriggers = [
      screen.getByText("New agent tab…").closest('[role="menuitem"]')!,
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
    // the toggles are terminal-context-only, so the caller's gate must keep
    // them out of here even on a phone.
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
    expect(screen.getByText("New agent tab…")).toBeTruthy()
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })
})

describe("MobileShell agentless terminal screen ⋯ menu", () => {
  // Every mobile terminal screen carries the keys quick toggle; without it,
  // hiding the bar means a trip through Preferences.
  // The toggles read `useIsMobile`, so the viewport shrinks below the 768px
  // breakpoint exactly like the agent-screen quick-toggle suite above, and the
  // keys toggle rides the touch surfaces, hence the pointer stub.
  const desktopWidth = window.innerWidth
  let media: MatchMediaStub
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    media = stubCoarsePointer()
  })
  afterEach(() => {
    media.restore()
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
      pendingSlotTab: {},
      terminalEpoch: 0,
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
      pendingSlotTab: {},
      terminalEpoch: 0,
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

  it("offers the keys quick toggle, exactly as the agent screen words it", () => {
    mockState = projectTerminalState()
    registerPaneInputGroup("pt-1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    expect(screen.getByText("Hide terminal keys")).toBeTruthy()
    expect(screen.queryByText("Hide top bar")).toBeNull()
  })

  it("labels flip to Show when the keys bar is already hidden", () => {
    mockState = projectTerminalState({ mobile_accessory_bar: false })
    registerPaneInputGroup("pt-1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    expect(screen.getByText("Show terminal keys")).toBeTruthy()
  })

  it("tapping Hide terminal keys persists through the generic settings PATCH", () => {
    mockState = projectTerminalState()
    registerPaneInputGroup("pt-1", { surfaceSwitch: false, keysToggle: true })
    render(<MobileShell />)
    fireEvent.click(screen.getByLabelText("Terminal actions"))
    fireEvent.click(screen.getByText("Hide terminal keys"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_accessory_bar: false },
          quiet: true,
        }),
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
    expect(screen.queryByText("New agent tab…")).toBeNull()
    expect(screen.queryByText("Rename agent…")).toBeNull()
  })
})

describe("MobileShell agent header identity", () => {
  // The phone header must say which project and which assistant, the same two
  // facts as the desktop header, as a two-line stack inside the SAME h-11 a
  // single line occupies.
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
    })
  }

  it("stacks the agent name over the project and assistant lane", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    const name = screen.getByText("main")
    const laneTwo = screen.getByText("Repo").closest("div")
    expect(name.className).toContain("text-sm")
    expect(laneTwo?.className).toContain("text-[11px]")
    expect(laneTwo?.className).toContain("text-muted-foreground")
    // Two separate lanes, not one run of text: the name's lane is lane two's
    // sibling.
    expect(name.closest("div")).not.toBe(laneTwo)
    expect(name.closest("div")?.parentElement).toBe(laneTwo?.parentElement)
  })

  it("does not grow the header: same h-11, tight leading, and the name truncates", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    const header = screen.getByLabelText("Back").closest("header")
    expect(header?.className).toContain("h-11")
    const name = screen.getByText("main")
    const laneTwo = screen.getByText("Repo").closest("div")
    // 14px + 11px at leading-tight is about 31px, inside the 44px header, and
    // both glyphs are smaller than their line boxes so they add nothing.
    expect(name.className).toContain("leading-tight")
    expect(laneTwo?.className).toContain("leading-tight")
    expect(name.className).toContain("truncate")
  })

  it("renders the branch without a mono line in the header", () => {
    mockState = agentHeaderState()
    render(<MobileShell />)
    expect(screen.getByText("main").className).not.toContain("font-mono")
  })
})

// THE PHONE HEADER'S CONTROLS ARE ONE FAMILY: one size, one variant, one
// corner radius.
//
// jsdom has no layout engine, so `getBoundingClientRect` is all zeros here and
// these assert the height TOKEN rather than the rendered pixel.
//
// The header no longer holds an action cluster at all: the four controls moved
// into the docked flap so the identity could have the whole remaining width.
// What the header owes now is Back plus the identity plus the pull-request
// chip, and what the FLAP owes is one shared height across a cluster that has
// to be pixel-identical to the floating pill it detaches into.
describe("MobileShell agent header and its flap cluster", () => {
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
    })
  }

  const cluster = () => ({
    theater: screen.getByLabelText("Theater mode"),
    macro: screen.getByLabelText("Run a macro"),
    changes: screen.getByLabelText(/changed files$/),
    actions: screen.getByLabelText("Session actions"),
  })

  it("leaves the header with navigation and identity only", () => {
    mockState = headerState()
    render(<MobileShell />)
    const header = screen.getByLabelText("Back").closest("header")
    expect(header).not.toBeNull()
    // Every action moved out; nothing but Back is left inside the bar.
    for (const el of Object.values(cluster())) {
      expect(header?.contains(el)).toBe(false)
    }
  })

  it("hangs the cluster off the band as one flap", () => {
    mockState = headerState()
    render(<MobileShell />)
    const flap = screen.getByTestId("mobile-action-flap")
    for (const [name, el] of Object.entries(cluster())) {
      expect(flap.contains(el), `${name} belongs to the flap`).toBe(true)
    }
  })

  it("renders every cluster control at the SAME explicit height", () => {
    mockState = headerState()
    render(<MobileShell />)
    // One height token across all four: the cluster is a single row on a
    // single surface, and the flight that carries it into the pill would tear
    // visibly if one control sat at a different offset from the rest.
    for (const [name, el] of Object.entries(cluster())) {
      expect(el.className, `${name} must carry the 40px height`).toMatch(
        /(?:^|\s)(?:size-10|h-10)(?:\s|$)/
      )
    }
  })

  it("keeps the bare count on the changes control, and nothing else wide", () => {
    mockState = headerState()
    render(<MobileShell />)
    const { theater, macro, changes, actions } = cluster()
    // The count is DATA, so it survives on a surface that otherwise prefers
    // icon-only, and it is the one control wider than it is tall. The diff
    // glyph already draws the ±, so the text is the number alone.
    expect(changes.textContent).toMatch(/^\d+$/)
    expect(changes.className).toContain("w-auto")
    for (const [name, el] of Object.entries({ theater, macro, actions })) {
      expect(el.textContent, `${name} is icon-only`).toBe("")
    }
  })

  it("gives the cluster one variant: bare circles on the flap's own surface", () => {
    mockState = headerState()
    render(<MobileShell />)
    // Not outline, deliberately: the flap is ONE surface, and a bordered
    // button inside it reads as two. Back stays ghost in the header for the
    // reason it always has, that it navigates rather than acts.
    for (const [name, el] of Object.entries(cluster())) {
      expect(el.className, `${name} must not be outlined`).not.toContain(
        "border-border"
      )
      expect(el.className, `${name} must be a pill on the flap`).toContain(
        "rounded-full"
      )
    }
    expect(screen.getByLabelText("Back").className).not.toContain("border-border")
  })
})

// The phone header's two lanes. The approved mock draws lane one as the robot
// glyph plus the agent name and lane two as folder+project and chip+assistant
// at 11px muted; the phone deliberately carries only those two secondary
// fields (no branch, no terminal count) because there is no hover on a phone,
// so a glyph there can never explain itself. The lanes must fit the header's
// existing height: two lines in the room one line used.
describe("MobileShell agent header lanes", () => {
  function laneState(): DuxState {
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
      pendingSlotTab: {},
      terminalEpoch: 0,
      mobileAccessoryBarOverride: null,
    })
  }

  const header = () => {
    const el = screen.getByLabelText("Back").closest("header")
    if (!el) throw new Error("no header")
    return el
  }

  it("draws the agent glyph beside the name in lane one", () => {
    mockState = laneState()
    render(<MobileShell />)
    const lead = screen.getByText("main").parentElement
    expect(lead?.querySelector("svg.lucide-bot")).toBeTruthy()
  })

  it("draws folder+project and chip+assistant in lane two", () => {
    mockState = laneState()
    render(<MobileShell />)
    const project = screen.getByText("Repo").parentElement
    const assistant = screen.getByText("claude").parentElement
    expect(project?.querySelector("svg.lucide-folder")).toBeTruthy()
    expect(assistant?.querySelector("svg.lucide-cpu")).toBeTruthy()
  })

  // The second lane is the smaller fact, at the 11px muted size the mock draws.
  it("renders lane two at 11px in the muted tone", () => {
    mockState = laneState()
    render(<MobileShell />)
    const lane = screen.getByText("Repo").closest("div")
    expect(lane?.className).toContain("text-[11px]")
    expect(lane?.className).toContain("text-muted-foreground")
  })

  // It must not grow the header: the stack lives inside the existing h-11.
  it("keeps the header at its existing height", () => {
    mockState = laneState()
    render(<MobileShell />)
    expect(header().className).toContain("h-11")
    for (const lane of header().querySelectorAll(".leading-tight")) {
      expect(lane.className).toContain("leading-tight")
    }
  })

  // The branch is the field most likely to repeat the name above it, and the
  // terminal count has no room; neither belongs on the phone.
  it("shows neither the branch chip nor a terminal count", () => {
    mockState = laneState()
    render(<MobileShell />)
    expect(header().querySelector("svg.lucide-git-branch")).toBeNull()
    expect(header().querySelector("svg.lucide-square-terminal")).toBeNull()
  })
})

// The hub's bottom bar renders the SAME launcher corner as the desktop sidebar
// footer, so the verb flip and the sizing are inherited rather than repeated
// here. This pins that the bar carries that one component: the verb, its ⋯, and
// nothing else that could drift back into a second control.
describe("MobileShell hub bottom bar", () => {
  it("carries the launcher corner: one verb, one ⋯, at one height", () => {
    mockState = makeState({ spine: makeSessionSpine(1) })
    render(<MobileShell />)
    const overflow = screen.getByLabelText("More ways to create")
    const corner = overflow.parentElement as HTMLElement
    const controls = Array.from(corner.querySelectorAll("button"))
    expect(controls).toHaveLength(2)
    for (const control of controls) {
      expect(control.className, control.textContent ?? "").toContain("h-7")
      expect(control.className, control.textContent ?? "").toContain(
        "max-md:min-h-11",
      )
    }
    expect(
      controls.map((c) => c.textContent?.trim()).filter(Boolean),
    ).toEqual(["New agent"])
    // There is no second split button.
    expect(screen.queryByLabelText("More ways to add a project")).toBeNull()
  })
})

// The changes screen is the same component the desktop pane renders, so the
// multi-select behaviour is tested there. What matters here is that the bulk
// bar's verbs stay TEXT on a phone: each carries a count, and a count is data
// no icon can say.
describe("MobileShell changes screen", () => {
  it("keeps the bulk bar's verbs as text with their counts", () => {
    mockState = makeState({
      spine: makeSessionSpine(1),
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      selectedSessionId: "s1",
      mobileScreen: "changes",
      changes: {
        sessionId: "s1",
        phase: "loaded",
        staged: [],
        unstaged: [
          { path: "a.ts", status: "M", additions: 1, deletions: 0, binary: false },
        ],
      },
    } as unknown as Partial<DuxState>)
    render(<MobileShell />)

    fireEvent.click(screen.getByLabelText("Select a.ts"))

    const stage = screen.getByRole("button", { name: "Stage 1" })
    expect(stage.textContent).toContain("Stage 1")
  })
})
