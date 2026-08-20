// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override only `useDux` (keeping every other real store export intact), plus
// `startDormantTab` as a spy so we can assert the explicit "Start session"
// action (and nothing else) is what launches a dormant tab.
let mockState: DuxState
const startDormantTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, startDormantTab: startDormantTabMock }
})

// Replace the lazy terminal pane with a prop-recording stub: the T15 test
// below must prove WHAT the area hands the pane (the owner ref), and mounting
// the real TerminalPane would pull in xterm's canvas renderer, which jsdom
// cannot back. The dormant-gating tests never resolve the lazy chunk, so they
// are unaffected.
const paneProps: unknown[] = []
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: (props: unknown) => {
    paneProps.push(props)
    return <div data-testid="terminal-pane-stub" />
  },
}))

// A tracking WebSocket double: G-T4 exists to prove that a DORMANT tab never
// opens a PTY socket (which would force-launch the provider) merely by being
// focused/rendered — only the explicit "Start session" action may. Every
// PtySocket construction goes through `new WebSocket(...)`, so counting
// constructions here is a proxy for "was a PTY socket opened."
class TrackingWebSocket {
  static instances: TrackingWebSocket[] = []
  binaryType = ""
  readyState = 0
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  constructor(public url: string) {
    TrackingWebSocket.instances.push(this)
  }
  send(): void {}
  close(): void {}
}

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
  // `TerminalPane` uses `useIsMobile`, which reads `matchMedia`.
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
  TrackingWebSocket.instances = []
  vi.stubGlobal("WebSocket", TrackingWebSocket)
}
installBootStubs()
// `TerminalArea` lives in its own module specifically so it can be mounted here
// without pulling in `App.tsx` -> `GlobalOverlays` -> `ConfigEditorDialog`,
// which eagerly imports the multi-MB Monaco bundle; Monaco cannot initialize
// under vitest (see the note in `lib/pathExt.ts`).
const { TerminalArea } = await import("./TerminalArea")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: {
      title: "dux",
      dux_version: "v1",
      show_changes_pane: false,
      always_show_tab_strip: false,
      available_providers: ["claude", "codex"],
      agent_tabs_max: 20,
    },
    selectedTarget: null,
    selectedSessionId: null,
    terminalEpoch: 0,
    startedDormantTabs: [],
    createTabInFlight: [],
    ...overrides,
  } as unknown as DuxState
}

// A session with a live session-slot tab (s1) and a DORMANT extra tab (b2, no
// live process, never explicitly started).
function dormantSpine(): DuxState["spine"] {
  return {
    projects: [],
    sessions: [
      {
        id: "s1",
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
        terminals: [],
        tabs: [
          {
            id: "s1",
            provider: "claude",
            order: 0,
            working: false,
            has_output: false,
            has_live_process: true,
          },
          {
            id: "b2",
            provider: "codex",
            order: 1,
            working: false,
            has_output: false,
            has_live_process: false,
          },
        ],
        has_output: false,
        working: false,
      },
    ],
    sidebar: { groups: [], agentless_start: null },
  } as unknown as DuxState["spine"]
}

beforeEach(() => {
  installBootStubs()
  startDormantTabMock.mockClear()
  paneProps.length = 0
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("TerminalArea not-found route", () => {
  it("says the agent is gone rather than showing the idle welcome screen", () => {
    // The welcome screen means "nothing is selected", which is the wrong story
    // for a URL that names a specific agent the workspace no longer has.
    mockState = makeState({
      routeNotFound: { kind: "agent", sessionId: "s9" },
    })
    render(<TerminalArea />)
    expect(screen.getByText("Agent not found")).toBeTruthy()
    expect(screen.getByText("s9")).toBeTruthy()
  })
})

describe("TerminalArea dormant-tab gating (G-T4)", () => {
  it("renders the DormantTabCard and opens NO PTY socket for a focused dormant extra tab", async () => {
    mockState = makeState({
      spine: dormantSpine(),
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "b2" },
    })
    render(<TerminalArea />)

    // The dormant card renders (its "Start session" button, provider-agnostic
    // copy) rather than the terminal pane.
    expect(await screen.findByText("Start session")).toBeTruthy()
    expect(screen.getByText(/isn.t running/)).toBeTruthy()

    // No PTY socket was opened: TerminalPane (and therefore PtySocket/WebSocket)
    // never mounted just because the dormant tab is focused.
    expect(TrackingWebSocket.instances).toHaveLength(0)

    // Only the explicit action launches it.
    fireEvent.click(screen.getByText("Start session"))
    expect(startDormantTabMock).toHaveBeenCalledWith("s1", "b2")
  })

  // Control case: the LIVE session-slot tab does NOT get the dormant treatment
  // (mirrors `isExtraTabDormant`'s own "never for the session-slot tab" case,
  // exercised here through the actual component gate rather than only the pure
  // helper). This does not assert a PTY socket opens — mounting a real
  // `TerminalPane` pulls in xterm's canvas rendering, which jsdom cannot back
  // without the (unlisted) `canvas` npm package — only that the gate takes the
  // "not dormant" branch.
  it("does not render the DormantTabCard for the live session-slot tab", () => {
    mockState = makeState({
      spine: dormantSpine(),
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    })
    render(<TerminalArea />)
    expect(screen.queryByText("Start session")).toBeNull()
  })
})

describe("TerminalArea project terminals (T15)", () => {
  it("mounts the pane with the PROJECT owner and never the dormant agent card", async () => {
    // The trap this guards (T15): with a required string `sessionId` on the
    // terminal target, this area compiled unchanged and handed a bogus session
    // id down, so the pane built the session-nested PTY URL and 404'd forever,
    // silently. The pane must receive the owner ref itself.
    mockState = makeState({
      spine: {
        projects: [
          {
            id: "p1",
            name: "Repo",
            terminals: [
              {
                id: "pt-1",
                label: "Terminal 2",
                has_output: true,
                foreground_cmd: null,
              },
            ],
          },
        ],
        sessions: [],
        sidebar: { groups: [], agentless_start: null },
      } as unknown as DuxState["spine"],
      selectedSessionId: null,
      selectedTarget: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
    })
    render(<TerminalArea />)

    expect(await screen.findByTestId("terminal-pane-stub")).toBeTruthy()
    // A project terminal is never an agent's dormant tab.
    expect(screen.queryByText("Start session")).toBeNull()
    expect(paneProps).toHaveLength(1)
    expect(paneProps[0]).toMatchObject({
      kind: "terminal",
      id: "pt-1",
      owner: { kind: "project", projectId: "p1" },
    })
  })
})
