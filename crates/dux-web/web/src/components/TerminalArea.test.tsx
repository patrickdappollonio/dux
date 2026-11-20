// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { useEffect } from "react"
import { fireAnimationStart } from "@/test/animationEvents"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { agentRowKey, registerPaneCover, resetPaneCovers } from "@/lib/paneCover"
import type { DuxState } from "@/lib/store"

// Override only `useDux` (keeping every other real store export intact), plus
// `startDormantTab` as a spy so we can assert the explicit "Start session"
// action (and nothing else) is what launches a dormant tab.
let mockState: DuxState
const startDormantTabMock = vi.fn()
// The stack's own acts, as spies: what a control asks for is the contract here,
// and the store's handling of each is pinned in the store's own tests.
const setStackLayoutMock = vi.fn()
const chooseStackPartMock = vi.fn()
const reconcileStackPartMock = vi.fn()
const startStoppedAgentMock = vi.fn()
const addTabMock = vi.fn()
const createTerminalMock = vi.fn()
const createProjectTerminalMock = vi.fn()
const createStandaloneTerminalMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    startDormantTab: startDormantTabMock,
    setStackLayout: (...args: unknown[]) => setStackLayoutMock(...args),
    settleStackOpening: () => {},
    chooseStackPart: (...args: unknown[]) => chooseStackPartMock(...args),
    reconcileStackPart: (...args: unknown[]) => reconcileStackPartMock(...args),
    startStoppedAgent: (...args: unknown[]) => startStoppedAgentMock(...args),
    addTab: (...args: unknown[]) => addTabMock(...args),
    createTerminal: (...args: unknown[]) => createTerminalMock(...args),
    createProjectTerminal: (...args: unknown[]) => createProjectTerminalMock(...args),
    createStandaloneTerminal: () => createStandaloneTerminalMock(),
  }
})

// Replace the lazy terminal pane with a prop-recording stub: the owner-ref
// test below must prove WHAT the area hands the pane, and mounting
// the real TerminalPane would pull in xterm's canvas renderer, which jsdom
// cannot back. The dormant-gating tests never resolve the lazy chunk, so they
// are unaffected.
const paneProps: unknown[] = []
// The stack the agent's screen mounts: one stub box for the whole stack, with
// a stub per part laid out by the stack's own arrangement, and each part's
// props recorded the way a lone pane's are.
const stackProps: StackStubProps[] = []
// Every part stub that mounted, by pty id: a part the layout remounts shows up
// twice.
const partMounts: string[] = []
function PartStub({
  id,
  onScreen,
  onPress,
}: {
  id: string
  onScreen: boolean
  onPress: () => void
}) {
  useEffect(() => {
    partMounts.push(id)
  }, [id])
  return (
    <div
      data-testid="stack-part-stub"
      data-pty={id}
      data-on-screen={String(onScreen)}
      onPointerDown={onPress}
    />
  )
}
type StackStubProps = {
  parts: ({ slot?: string; instance?: string } & Record<string, unknown>)[]
  overlay?: React.ReactNode
  typingTarget?: string | null
  onChooseTypingTarget?: (ptyId: string) => void
  arrange?: (
    parts: Record<string, React.ReactNode>,
    typingTarget: string | null,
  ) => React.ReactNode
}
// The stack's panel group, with its props captured: jsdom has no layout for the
// panel library to drag in, so a test hands the group the layout a drag would
// have ended on.
let groupProps: { onLayoutChanged?: (layout: Record<string, number>) => void } | null = null
vi.mock("@/components/ui/resizable", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/ui/resizable")>()
  return {
    ...actual,
    ResizablePanelGroup: (props: Parameters<typeof actual.ResizablePanelGroup>[0]) => {
      groupProps = props
      return actual.ResizablePanelGroup(props)
    },
  }
})
vi.mock("@/components/LazyTerminalPane", async () => {
  // The stack's own fallback, so a typing target the store names that is not a
  // part on screen behaves here as it does in the real stack.
  const { stackTypingTarget } = await vi.importActual<typeof import("@/lib/layoutModel")>(
    "@/lib/layoutModel",
  )
  return {
  LazyTerminalPane: (props: unknown) => {
    paneProps.push(props)
    // The overlay is rendered rather than dropped: it is where the floating
    // theater pill lives, and the pane's positioned box is the surface the pill
    // is clamped to on both shells.
    const { overlay } = props as { overlay?: React.ReactNode }
    return <div data-testid="terminal-pane-stub">{overlay}</div>
  },
  LazyTerminalStack: (props: StackStubProps) => {
    stackProps.push(props)
    const rendered: Record<string, React.ReactNode> = {}
    for (const part of props.parts) {
      const { slot, instance: _instance, ...rest } = part
      paneProps.push(rest)
      rendered[slot ?? ""] = (
        <PartStub
          key={`${slot}:${part.instance ?? ""}`}
          id={String(rest.id)}
          onScreen={rest.onScreen !== false}
          onPress={() => props.onChooseTypingTarget?.(String(rest.id))}
        />
      )
    }
    const typing = stackTypingTarget(
      props.parts.map((p) => ({ id: String(p.id), onScreen: p.onScreen !== false })),
      props.typingTarget ?? null,
    )
    return (
      <div data-testid="terminal-pane-stub">
        {props.arrange
          ? props.arrange(rendered, typing === null ? null : String(typing))
          : Object.values(rendered)}
        {props.overlay}
      </div>
    )
  },
  }
})

// A tracking WebSocket double: it proves a DORMANT tab never opens a PTY
// socket (which would force-launch the provider) merely by being
// focused/rendered; only the explicit "Start session" action may. Every
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
  // The stack's divider is a resizable panel group, which observes its panels.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
}
installBootStubs()
// `TerminalArea` lives in its own module specifically so it can be mounted here
// without pulling in `App.tsx` -> `GlobalOverlays` -> `ConfigEditorDialog`,
// which eagerly imports the multi-MB Monaco bundle; Monaco cannot initialize
// under vitest (see the note in `lib/pathExt.ts`).
const { TerminalArea } = await import("./TerminalArea")
const { terminalAreaModel } = await import("./terminalAreaModel")

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
    pendingSlotTab: {},
    createTabInFlight: [],
    pendingTabOrder: {},
    stackLayouts: {},
    stackTerminals: {},
    stackTabs: {},
    pendingTerminalOrder: null,
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
        // Slot-ness is the session's answer, and the card rule reads it: without
        // this the helper cannot tell the agent's first tab from an extra one.
        slot_tab_id: "s1",
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
  stackProps.length = 0
  partMounts.length = 0
  for (const mock of [
    setStackLayoutMock,
    chooseStackPartMock,
    reconcileStackPartMock,
    startStoppedAgentMock,
    addTabMock,
    createTerminalMock,
  ]) {
    mock.mockClear()
  }
})

afterEach(() => {
  cleanup()
  resetPaneCovers()
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

describe("TerminalArea terminal not-found route", () => {
  it("says the agent's terminal is gone and opens a new one for that agent", () => {
    mockState = makeState({
      spine: dormantSpine(),
      routeNotFound: {
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "session", sessionId: "s1" },
      },
    })
    render(<TerminalArea />)
    expect(screen.getByText("Terminal not found")).toBeTruthy()
    expect(screen.getByText("t9")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "New terminal" }))
    expect(createTerminalMock).toHaveBeenCalledWith("s1")
  })

  it("adds no terminal to an agent another device drives, and says why", () => {
    mockState = makeState({
      spine: dormantSpine(),
      routeNotFound: {
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "session", sessionId: "s1" },
      },
    })
    registerPaneCover("s1-other-tab", "card", { row: agentRowKey("s1"), driver: "other" })
    render(<TerminalArea />)
    const button = screen.getByRole("button", { name: "New terminal" })
    expect(button).toHaveProperty("disabled", true)
    expect(button.getAttribute("aria-describedby")).toBeTruthy()
    expect(screen.getByText(/Take over to add a tab or a terminal/)).toBeTruthy()
  })

  it("opens a new terminal at the project's root, or a new standalone one", () => {
    const spine = dormantSpine()!
    spine.projects = [{ id: "p1", name: "Repo" }] as never
    mockState = makeState({
      spine,
      routeNotFound: { kind: "terminal", terminalId: "t9", owner: { kind: "project", projectId: "p1" } },
    })
    const { rerender } = render(<TerminalArea />)
    fireEvent.click(screen.getByRole("button", { name: "New terminal" }))
    expect(createProjectTerminalMock).toHaveBeenCalledWith("p1")

    mockState = makeState({
      spine,
      routeNotFound: { kind: "terminal", terminalId: "t9", owner: { kind: "standalone" } },
    })
    rerender(<TerminalArea />)
    fireEvent.click(screen.getByRole("button", { name: "New terminal" }))
    expect(createStandaloneTerminalMock).toHaveBeenCalledTimes(1)
  })

  it("offers no new terminal for an owner that is gone too", () => {
    mockState = makeState({
      spine: dormantSpine(),
      routeNotFound: {
        kind: "terminal",
        terminalId: "t9",
        owner: { kind: "project", projectId: "gone" },
      },
    })
    render(<TerminalArea />)
    expect(screen.getByText("Terminal not found")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "New terminal" })).toBeNull()
  })
})

describe("TerminalArea in theater", () => {
  it("floats the draggable pill inside the pane's own box", async () => {
    // Inside the pane, not beside it: the pill is clamped to the surface it is
    // rendered in, and that surface is the terminal's box on both shells.
    mockState = makeState({
      spine: dormantSpine(),
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      theater: true,
    } as unknown as Partial<DuxState>)
    render(<TerminalArea />)
    await vi.waitFor(() => expect(paneProps.length).toBeGreaterThan(0))
    const pill = screen.getByTestId("theater-pill")
    expect(screen.getByTestId("terminal-pane-stub").contains(pill)).toBe(true)
    expect(screen.getByTestId("theater-pill-grip")).toBeTruthy()
  })

  // THE ID CONTRACT BETWEEN THE SHELL AND THE PANE. The pane publishes its
  // input group under the id it was handed; the pill reads the id the shell
  // derives from the same target. Nothing would fail to compile if a refactor
  // let those two diverge, and the way back would simply be missing, so the
  // registration here uses the id the SHELL actually gave the pane rather than
  // one the test knows by heart.
  it("reads the input group under the very id it handed the pane", async () => {
    mockState = makeState({
      spine: dormantSpine(),
      selectedSessionId: "s1",
      // An EXTRA tab, so the tab id and the session id are different strings
      // and a menu scanning the wrong one cannot pass by accident.
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "b2" },
      startedDormantTabs: ["b2"],
      theater: true,
    } as unknown as Partial<DuxState>)
    render(<TerminalArea />)
    await vi.waitFor(() => expect(paneProps.length).toBeGreaterThan(0))
    const handed = (paneProps[0] as { id: string }).id
    expect(handed).toBe("b2")
    const groups = await import("@/lib/paneInputGroup")
    groups.registerPaneInputGroup(handed, {
      surfaceSwitch: true,
      keysToggle: false,
    })
    try {
      fireEvent.click(screen.getByLabelText("Session actions"))
      await screen.findByRole("menu")
      expect(screen.getByText("Use virtual input")).toBeTruthy()
    } finally {
      groups.resetPaneInputGroups()
    }
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

  // Control case: the LIVE session-slot tab does NOT get the dormant treatment.
  // This does not assert a PTY socket opens: mounting a real `TerminalPane`
  // pulls in xterm's canvas rendering, which jsdom cannot back without the
  // (unlisted) `canvas` npm package. It only checks that the gate takes the
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

  // ONE CLICK. Selecting an agent whose first tab came back dormant (a restart,
  // a stop) mounts the pane, and the pane's PTY subscription is what starts it.
  // The card here would make starting an agent a two-click job.
  it("mounts the pane for a healthy dormant first tab instead of the card, until its run ends on screen", async () => {
    const spine = dormantSpine()
    spine!.sessions[0].tabs[0].has_live_process = false
    mockState = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    })
    const { rerender } = render(<TerminalArea />)

    await vi.waitFor(() => expect(paneProps.length).toBeGreaterThan(0))
    expect(screen.queryByText("Start session")).toBeNull()
    expect(paneProps.at(-1)).toMatchObject({ kind: "agent", id: "s1" })

    // The user quit it while looking at it: the idle screen and its one act
    // take the pane's place, so nothing re-subscribes and relaunches it.
    mockState = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      endedInViewTab: "s1",
    })
    rerender(<TerminalArea />)
    expect(await screen.findByText("Start session")).toBeTruthy()
    // The card sits in the agent's place in the stack, with no part behind it.
    expect(screen.queryByTestId("stack-part-stub")).toBeNull()
  })

  // ...unless that tab's last run ENDED BADLY. Then the card is the diagnosis
  // surface and nothing opens a socket, because subscribing is what would
  // relaunch it, every single time the user looked at the agent.
  it("renders the DormantTabCard and opens NO PTY socket for a first tab whose last run failed", async () => {
    const spine = dormantSpine()
    spine!.sessions[0].tabs[0].has_live_process = false
    spine!.sessions[0].tabs[0].last_run_failed = true
    mockState = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    })
    render(<TerminalArea />)

    expect(await screen.findByText("Start session")).toBeTruthy()
    expect(TrackingWebSocket.instances).toHaveLength(0)
    expect(paneProps).toHaveLength(0)

    fireEvent.click(screen.getByText("Start session"))
    expect(startDormantTabMock).toHaveBeenCalledWith("s1", "s1")
  })

  // The latch keeps the card off a start the user already pressed, for the gap
  // between the server accepting it and the spine reporting the process live.
  it("suppresses the card for a not-yet-live first tab this client started", () => {
    const spine = dormantSpine()
    spine!.sessions[0].tabs[0].has_live_process = false
    spine!.sessions[0].tabs[0].last_run_failed = true
    mockState = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      startedDormantTabs: ["s1"],
      pendingSlotTab: {},
    })
    render(<TerminalArea />)
    expect(screen.queryByText("Start session")).toBeNull()
  })
})

describe("TerminalArea slot tab identity", () => {
  // The pane decides the PTY route and arms the extra-tab "the tab is gone,
  // stop reconnecting" guard from whether its target is the slot tab. That
  // answer needs the spine's generated slot tab id: without it the pane falls
  // back to comparing the tab id against the SESSION id, which is never true
  // for a slot tab any more, so the agent's first tab is treated as an extra
  // tab and an empty tab list reads as "this tab was closed".
  it("hands the pane the spine's slot tab id for the agent's first tab", async () => {
    const spine = dormantSpine()
    spine!.sessions[0].slot_tab_id = "s1-slot"
    spine!.sessions[0].tabs[0].id = "s1-slot"
    mockState = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1-slot" },
    })
    render(<TerminalArea />)

    expect(await screen.findByTestId("terminal-pane-stub")).toBeTruthy()
    expect(paneProps).toHaveLength(1)
    expect(paneProps[0]).toMatchObject({
      kind: "agent",
      id: "s1-slot",
      sessionId: "s1",
      slotTabId: "s1-slot",
    })
  })
})

describe("TerminalArea pull request banner", () => {
  const pr = (number: number) => ({
    number,
    state: "open",
    title: `Pull request ${number}`,
    url: `https://github.com/acme/api/pull/${number}`,
    overridden: false,
    host: "github.com",
    owner_repo: "acme/api",
    own_repo: true,
  })

  it("forgets the stepped-to pull request once it leaves the list", () => {
    const withPrs = (numbers: number[]) => {
      const spine = dormantSpine()
      spine!.sessions[0].prs = numbers.map(pr) as never
      spine!.sessions[0].pr = pr(numbers[0]) as never
      return makeState({
        spine,
        selectedSessionId: "s1",
        selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
      })
    }
    mockState = withPrs([418, 77, 415])
    const { rerender } = render(<TerminalArea />)
    fireEvent.click(screen.getByRole("button", { name: "Next pull request" }))
    expect(screen.getByText("2 of 3")).toBeTruthy()

    mockState = withPrs([418, 415])
    rerender(<TerminalArea />)
    expect(screen.getByText("1 of 2")).toBeTruthy()

    mockState = withPrs([418, 77, 415])
    rerender(<TerminalArea />)
    expect(screen.getByText("1 of 3")).toBeTruthy()
  })
})

describe("TerminalArea project terminals", () => {
  it("mounts the pane with the PROJECT owner and never the dormant agent card", async () => {
    // With a required string `sessionId` on the terminal target, this area
    // compiles unchanged while handing a bogus session id down, so the pane
    // builds the session-nested PTY URL and 404s forever, silently. The pane
    // must receive the owner ref itself.
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

// The model is what the center pane is decided from; the render below it is a
// layout. These cover the ids and the lanes directly, where the rendered tests
// above cover the dormant-card rule through the real surface.
// AN AGENT AND ITS TERMINAL DOCK. The agent's tab on top, its own terminals in
// a dock below, laid out by the stack's split. The parts themselves are stubs;
// what is pinned here is which parts mount, where they sit, what chrome frames
// them and which controls each layout offers.
describe("TerminalArea dock", () => {
  type DockOpts = {
    terminals?: number
    tabs?: number
    alwaysShow?: boolean
    layout?: "agent" | "split" | "terminal"
    stopped?: boolean
    crashed?: boolean
    target?: "agent" | "terminal"
    working?: boolean
  }
  function dockState(opts: DockOpts = {}): DuxState {
    const spine = dormantSpine()!
    const session = spine.sessions[0]
    const tabCount = opts.tabs ?? 1
    session.tabs = session.tabs.slice(0, tabCount).map((tab) => ({
      ...tab,
      has_live_process: !opts.stopped,
    }))
    if (opts.stopped) {
      session.status = "detached"
      if (opts.crashed) {
        session.tabs[0].last_run_failed = true
        session.tabs[0].last_run_verdict = {
          ending: "exited",
          status: 1,
          ended_at: "2026-11-10T00:00:00Z",
          excerpt: ["panic: boom"],
        } as never
      }
    }
    session.working = opts.working ?? false
    spine.terminals = Array.from({ length: opts.terminals ?? 1 }, (_, i) => ({
      id: `t${i + 1}`,
      owner: { kind: "session", session_id: "s1" },
      label: `Terminal ${i + 1}`,
      has_output: true,
      working: false,
      typing: false,
      foreground_cmd: null,
      sort_order: i + 1,
      created_at: "",
      updated_at: "",
    }))
    const base = makeState({
      spine,
      selectedSessionId: "s1",
      selectedTarget:
        opts.target === "terminal"
          ? { kind: "terminal", terminalId: "t1", owner: { kind: "session", sessionId: "s1" } }
          : { kind: "agent", sessionId: "s1", tabId: "s1" },
      stackLayouts: opts.layout ? { s1: opts.layout } : {},
    } as unknown as Partial<DuxState>)
    base.bootstrap = { ...base.bootstrap!, always_show_tab_strip: opts.alwaysShow ?? false }
    return base
  }
  const box = (part: "agent" | "dock") => screen.getByTestId(`stack-${part}-box`)
  const onScreen = (part: "agent" | "dock") => box(part).dataset.onScreen === "true"
  const pillLabels = () =>
    Array.from(
      screen.getByTestId("stack-layout-pill").querySelectorAll("button"),
    ).map((b) => b.getAttribute("aria-label"))

  it("has no dock for an agent with no terminals of its own", async () => {
    mockState = dockState({ terminals: 0 })
    render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    expect(screen.queryByTestId("stack-dock-box")).toBeNull()
    expect(screen.queryByRole("separator")).toBeNull()
    // The strips are hidden and the + lives in the header, so nothing floats.
    expect(screen.queryByTestId("stack-layout-pill")).toBeNull()
  })

  it.each([
    ["split", true, true],
    ["agent", true, false],
    ["terminal", false, true],
  ] as const)("lays out the %s layout with both strips kept", async (layout, agent, dock) => {
    mockState = dockState({ layout, alwaysShow: true })
    render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    expect([onScreen("agent"), onScreen("dock")]).toEqual([agent, dock])
    // A part folded away keeps its strip: the provider strip's + and the
    // dock's are both there whichever part has the height.
    expect(screen.getByLabelText("New tab")).toBeTruthy()
    expect(screen.getByTestId("dock-strip")).toBeTruthy()
    expect(screen.queryByTestId("stack-layout-pill")).toBeNull()
    // Both parts stay mounted in every layout.
    expect(paneProps.map((p) => (p as { id: string }).id)).toEqual(["s1", "t1"])
  })

  it.each([
    ["split", ["Hide terminals", "Maximize terminal"]],
    ["agent", ["Show terminals"]],
    ["terminal", ["Restore the split"]],
  ] as const)(
    "with the strips hidden floats the %s layout's controls on the divider",
    async (layout, labels) => {
      mockState = dockState({ layout })
      render(<TerminalArea />)
      await screen.findByTestId("stack-agent-box")
      expect(screen.queryByLabelText("New tab")).toBeNull()
      expect(screen.queryByTestId("dock-strip")).toBeNull()
      expect(screen.getByRole("separator")).toBeTruthy()
      expect(pillLabels()).toEqual(labels)
    },
  )

  it("reaches every layout from the controls of every other", async () => {
    for (const [layout, label, next] of [
      ["split", "Hide terminals", "agent"],
      ["split", "Maximize terminal", "terminal"],
      ["agent", "Show terminals", "split"],
      ["terminal", "Restore the split", "split"],
    ] as const) {
      mockState = dockState({ layout, alwaysShow: true })
      render(<TerminalArea />)
      fireEvent.click(await screen.findByLabelText(label))
      expect(setStackLayoutMock).toHaveBeenLastCalledWith("s1", next)
      cleanup()
    }
  })

  it("shows the agent's state on the pill while its terminal is maximized", async () => {
    mockState = dockState({ layout: "terminal", working: true })
    render(<TerminalArea />)
    expect(await screen.findByLabelText("The agent is working")).toBeTruthy()
  })

  it("brings both strips back for a second terminal, or a second tab", async () => {
    for (const opts of [{ terminals: 2 }, { tabs: 2 }]) {
      mockState = dockState({ layout: "split", ...opts })
      render(<TerminalArea />)
      expect(await screen.findByLabelText("New tab")).toBeTruthy()
      expect(screen.getByLabelText("New terminal")).toBeTruthy()
      expect(screen.queryByTestId("stack-layout-pill")).toBeNull()
      cleanup()
    }
  })

  it.each([
    ["the agent", "agent", [true, false]],
    ["the dock terminal", "terminal", [false, true]],
  ] as const)(
    "shows %s typing goes to alone in theater, with no strip, and puts the split back on leaving",
    async (_part, target, shown) => {
      mockState = { ...dockState({ layout: "split", alwaysShow: true, target }), theater: true }
      const { rerender } = render(<TerminalArea />)
      await screen.findByTestId("stack-agent-box")
      expect([onScreen("agent"), onScreen("dock")]).toEqual(shown)
      expect(screen.queryByRole("separator")).toBeNull()
      expect(screen.queryByTestId("stack-layout-pill")).toBeNull()
      expect(screen.getByTestId("theater-pill")).toBeTruthy()
      // Theater never changed the layout the stack is in.
      expect(setStackLayoutMock).not.toHaveBeenCalled()

      mockState = dockState({ layout: "split", alwaysShow: true, target })
      rerender(<TerminalArea />)
      expect([onScreen("agent"), onScreen("dock")]).toEqual([true, true])
      expect(screen.getByRole("separator")).toBeTruthy()
    },
  )

  it("greys the terminal box of the part typing does not go to, never its strip", async () => {
    mockState = dockState({ layout: "split", alwaysShow: true })
    const { rerender } = render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    const grey = (el: HTMLElement) =>
      el.classList.contains("grayscale") && el.classList.contains("opacity-80")
    expect([grey(box("agent")), grey(box("dock"))]).toEqual([false, true])
    expect(grey(screen.getByTestId("dock-strip"))).toBe(false)

    mockState = dockState({ layout: "split", alwaysShow: true, target: "terminal" })
    rerender(<TerminalArea />)
    expect([grey(box("agent")), grey(box("dock"))]).toEqual([true, false])

    // Folded away, the other part is not on screen to grey.
    mockState = dockState({ layout: "agent", alwaysShow: true })
    rerender(<TerminalArea />)
    expect(grey(box("dock"))).toBe(false)
  })

  it("makes a press into a part the one typing goes to", async () => {
    mockState = dockState({ layout: "split" })
    render(<TerminalArea />)
    const parts = await screen.findAllByTestId("stack-part-stub")
    fireEvent.pointerDown(parts[1])
    expect(chooseStackPartMock).toHaveBeenCalledWith("s1", "t1")
  })

  it("tells the store when typing has to move to a part on screen", async () => {
    mockState = dockState({ layout: "split" })
    const { rerender } = render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    expect(reconcileStackPartMock).not.toHaveBeenCalled()
    // Maximizing the terminal folds the agent the store still names away.
    mockState = dockState({ layout: "terminal" })
    rerender(<TerminalArea />)
    expect(reconcileStackPartMock).toHaveBeenLastCalledWith("s1", "t1")
    // And back: hiding the terminals folds the dock the store names away.
    mockState = dockState({ layout: "agent", target: "terminal" })
    rerender(<TerminalArea />)
    expect(reconcileStackPartMock).toHaveBeenLastCalledWith("s1", "s1")
  })

  it("names the dock as the typing target of a stopped agent opened from its row", async () => {
    mockState = dockState({ stopped: true })
    render(<TerminalArea />)
    await screen.findByTestId("stopped-agent-band")
    expect(reconcileStackPartMock).toHaveBeenCalledWith("s1", "t1")
  })

  it("never remounts a part when the layout changes", async () => {
    mockState = dockState({ layout: "split" })
    const { rerender } = render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    for (const layout of ["agent", "terminal", "split"] as const) {
      mockState = dockState({ layout })
      rerender(<TerminalArea />)
    }
    expect(partMounts.sort()).toEqual(["s1", "t1"])
  })

  it("folds the part a divider drag left below its minimum", async () => {
    mockState = dockState({ layout: "split" })
    render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    groupProps?.onLayoutChanged?.({ "stack-agent": 100, "stack-dock": 0 })
    expect(setStackLayoutMock).toHaveBeenLastCalledWith("s1", "agent")
    groupProps?.onLayoutChanged?.({ "stack-agent": 0, "stack-dock": 100 })
    expect(setStackLayoutMock).toHaveBeenLastCalledWith("s1", "terminal")
    setStackLayoutMock.mockClear()
    groupProps?.onLayoutChanged?.({ "stack-agent": 40, "stack-dock": 60 })
    expect(setStackLayoutMock).not.toHaveBeenCalled()
  })

  // jsdom lays nothing out, and the panel library does nothing with a group of
  // no height, so the stack is given one: panels of 250px each and a strip of
  // `stripPx`. A 20px strip is under the half of the 15% minimum where the
  // library snaps a part shut (a fine pointer's strips), a 50px one is over it
  // (a coarse pointer's taller strips).
  it.each([
    ["fine", "terminal", 20],
    ["coarse", "terminal", 50],
    ["fine", "agent", 20],
    ["coarse", "agent", 50],
  ] as const)(
    "brings the split back at its share from the %s pointer's %s layout",
    async (_pointer, folded, stripPx) => {
      const proto = HTMLElement.prototype
      const offset = Object.getOwnPropertyDescriptor(proto, "offsetHeight")
      Object.defineProperty(proto, "offsetHeight", {
        configurable: true,
        get(this: HTMLElement) {
          return this.hasAttribute("data-panel") ? 250 : 0
        },
      })
      const rect = vi
        .spyOn(proto, "getBoundingClientRect")
        .mockImplementation(function (this: HTMLElement) {
          const height =
            this.className === "shrink-0"
              ? stripPx
              : this.className.includes("[container-type:size]")
                ? 500
                : 0
          return { height, width: 0, top: 0, left: 0, right: 0, bottom: height, x: 0, y: 0, toJSON() {} }
        })
      try {
        mockState = dockState({ layout: folded, alwaysShow: true })
        const { rerender } = render(<TerminalArea />)
        await screen.findByTestId("stack-agent-box")
        mockState = dockState({ layout: "split", alwaysShow: true })
        rerender(<TerminalArea />)
        // Nothing folds the split the person just asked for, and nothing
        // remembers a share they never dragged to.
        expect(setStackLayoutMock).not.toHaveBeenCalled()
        expect(localStorage.getItem("dux:stack-agent-percent")).toBeNull()
        const grow = (id: string) => document.getElementById(id)?.style.flexGrow
        expect([grow("stack-agent"), grow("stack-dock")]).toEqual(["50", "50"])
        expect([onScreen("agent"), onScreen("dock")]).toEqual([true, true])
      } finally {
        rect.mockRestore()
        if (offset) Object.defineProperty(proto, "offsetHeight", offset)
      }
    },
  )

  it("rests the pill on the divider in the split and tucks it inside a maximized layout", async () => {
    const pill = () => screen.getByTestId("stack-layout-pill")
    mockState = dockState({ layout: "split" })
    const { rerender } = render(<TerminalArea />)
    expect((await screen.findByTestId("stack-layout-pill")).style.top).toMatch(/^calc\(\d+% \+ 4px\)$/)
    mockState = dockState({ layout: "agent" })
    rerender(<TerminalArea />)
    expect(pill().style.bottom).toBe("12px")
    mockState = dockState({ layout: "terminal" })
    rerender(<TerminalArea />)
    expect(pill().style.top).toBe("12px")
  })

  // The stopped agent's band holds the top of a maximized terminal, with Start
  // at its right end, where the pill tucked 12px into the stack used to sit
  // over Start and its focus ring. A browser must show Start whole, with the
  // pill below the band.
  it("tucks the pill under a stopped agent's band rather than over its Start", async () => {
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        const height = this.querySelector("[data-testid='stopped-agent-band']") ? 37 : 0
        return { height, width: 0, top: 0, left: 0, right: 0, bottom: height, x: 0, y: 0, toJSON() {} }
      })
    try {
      mockState = dockState({ stopped: true, layout: "terminal" })
      render(<TerminalArea />)
      await screen.findByTestId("stopped-agent-band")
      expect(screen.getByTestId("stack-layout-pill").style.top).toBe("49px")
    } finally {
      rect.mockRestore()
    }
  })

  it("puts the attention look on the pill's dot over the working one", async () => {
    const state = dockState({ layout: "terminal", working: true })
    state.spine!.sessions[0].needs_attention = true
    mockState = state
    render(<TerminalArea />)
    const pill = await screen.findByTestId("stack-layout-pill")
    expect(pill.querySelector("[aria-label='Needs attention']")).toBeTruthy()
    expect(pill.querySelector("[aria-label='The agent is working']")).toBeNull()
  })

  it("keeps a running agent's dormant extra tab on its own card, whose Start is that tab's", async () => {
    const state = dockState({ layout: "split", tabs: 2 })
    state.spine!.sessions[0].tabs[1].has_live_process = false
    state.selectedTarget = { kind: "agent", sessionId: "s1", tabId: "b2" }
    mockState = state
    render(<TerminalArea />)
    fireEvent.click(await screen.findByText("Start session"))
    expect(startDormantTabMock).toHaveBeenCalledWith("s1", "b2")
    expect(startStoppedAgentMock).not.toHaveBeenCalled()
    expect(screen.queryByTestId("stopped-agent-short-card")).toBeNull()
  })

  it("leaves a stopped agent on its idle screen once its last terminal ends", async () => {
    const state = dockState({ stopped: true, terminals: 0 })
    state.endedInViewTab = "s1"
    mockState = state
    render(<TerminalArea />)
    expect(await screen.findByText("Start session")).toBeTruthy()
    expect(screen.queryByTestId("stack-part-stub")).toBeNull()
  })

  // Guards the user-locked bounce (CLAUDE.md "Locked by the user"): a working
  // tab's pill must never be remounted by the screen below the strip changing,
  // because a remount restarts its bounce mid-flight. Switching between a live
  // tab and a dormant one swaps a terminal for a card under the same strip.
  it("keeps the tab strip, and a working tab's bounce, mounted across a live and a dormant tab", async () => {
    const state = (tabId: string) => {
      const st = dockState({ terminals: 0, tabs: 2 })
      const tabs = st.spine!.sessions[0].tabs
      tabs[0] = { ...tabs[0], working: true }
      tabs[1] = { ...tabs[1], has_live_process: false }
      st.selectedTarget = { kind: "agent", sessionId: "s1", tabId }
      return st
    }
    mockState = state("s1")
    const { rerender } = render(<TerminalArea />)
    const pill = (await screen.findAllByRole("tab"))[0]
    const frame = pill.querySelector("[data-slot='working-glyph']")!
    expect(frame.getAttribute("class")).toContain("motion-safe:animate-working-bounce")
    fireAnimationStart(frame, "working-bounce")

    mockState = state("b2")
    rerender(<TerminalArea />)
    expect(await screen.findByText("Start session")).toBeTruthy()
    mockState = state("s1")
    rerender(<TerminalArea />)

    expect(screen.getAllByRole("tab")[0]).toBe(pill)
    expect(pill.querySelector("[data-slot='working-glyph']")).toBe(frame)
    expect(frame.getAttribute("class")).toContain("motion-safe:animate-working-bounce")
  })

  // A device that does not drive the row sees the take-over card and nothing
  // that says the agent has terminals: the divider and its grip are painted
  // above the card, and the strip and the pill would be in the tab order under
  // it.
  it.each([
    ["the strips", true],
    ["the layout pill", false],
  ] as const)(
    "shows a watcher only the take-over card, with %s, and gives the stack back as it was",
    async (_chrome, alwaysShow) => {
      mockState = dockState({ layout: "split", alwaysShow })
      render(<TerminalArea />)
      await screen.findByTestId("stack-agent-box")
      let retire = () => {}
      act(() => {
        retire = registerPaneCover("s1", "card", { row: agentRowKey("s1") })
      })
      expect(screen.queryByRole("separator")).toBeNull()
      expect(screen.queryByTestId("dock-strip")).toBeNull()
      expect(screen.queryByTestId("stack-layout-pill")).toBeNull()
      const boxes = () => [screen.getByTestId("stack-agent-box"), screen.getByTestId("stack-dock-box")]
      expect(boxes().map((box) => box.closest("[inert]") !== null)).toEqual([true, true])

      act(() => retire())
      expect(boxes().map((box) => box.closest("[inert]") !== null)).toEqual([false, false])
      expect(screen.getByRole("separator")).toBeTruthy()
      expect(
        screen.queryByTestId(alwaysShow ? "dock-strip" : "stack-layout-pill"),
      ).toBeTruthy()
      expect([onScreen("agent"), onScreen("dock")]).toEqual([true, true])
      expect(setStackLayoutMock).not.toHaveBeenCalled()
      expect(partMounts.sort()).toEqual(["s1", "t1"])
    },
  )

  // The card covers the stack whichever part of the row paints it, and what is
  // under it can be neither reached nor read.
  describe("under the row's take-over card", () => {
    const watch = (ptyId: string) =>
      act(() => void registerPaneCover(ptyId, "card", { row: agentRowKey("s1") }))
    const outOfReach = (el: Element) => el.closest("[inert]") !== null

    it.each(["agent", "split", "terminal"] as const)(
      "takes a stopped agent's Start out of reach in the %s layout",
      async (layout) => {
        mockState = dockState({ stopped: true, layout })
        render(<TerminalArea />)
        await screen.findByTestId("stopped-agent-band")
        watch("t1")
        const starts = screen.getAllByRole("button", { name: "Start", hidden: true })
        expect(starts.length).toBeGreaterThan(0)
        expect(starts.every(outOfReach)).toBe(true)
        expect(stackProps.at(-1)).toMatchObject({ cardTopPx: 0 })
      },
    )

    it("draws the agent's strip only where an agent with no terminals has one, above the card", async () => {
      mockState = dockState({ terminals: 2, layout: "split" })
      const { unmount } = render(<TerminalArea />)
      await screen.findByTestId("stack-agent-box")
      watch("s1")
      expect(screen.queryByLabelText("New tab")).toBeNull()
      unmount()
      resetPaneCovers()

      mockState = dockState({ terminals: 2, tabs: 2, layout: "split" })
      render(<TerminalArea />)
      await screen.findByTestId("stack-agent-box")
      watch("s1")
      expect(outOfReach(screen.getByLabelText("New tab"))).toBe(false)
      expect(outOfReach(screen.getByTestId("stack-agent-box"))).toBe(true)
      expect(outOfReach(screen.getByTestId("stack-dock-box"))).toBe(true)
    })
  })

  it("keeps the stack as it is under a Reconnect box on a row this device drives", async () => {
    mockState = dockState({ layout: "split" })
    render(<TerminalArea />)
    await screen.findByTestId("stack-agent-box")
    act(() => void registerPaneCover("s1", "box"))
    expect(screen.getByRole("separator")).toBeTruthy()
  })

  describe("a stopped agent with a running terminal", () => {
    it("opens on its terminal with the band and never mounts the agent's tab", async () => {
      mockState = dockState({ stopped: true })
      render(<TerminalArea />)
      const band = await screen.findByTestId("stopped-agent-band")
      expect(band.textContent).toContain("isn’t running")
      expect(onScreen("agent")).toBe(false)
      expect(onScreen("dock")).toBe(true)
      expect(paneProps.map((p) => (p as { id: string }).id)).toEqual(["t1"])
      fireEvent.click(screen.getByRole("button", { name: "Start" }))
      expect(startStoppedAgentMock).toHaveBeenCalledWith("s1", "s1")
    })

    it("opens on its terminal from an address naming the terminal too", async () => {
      mockState = dockState({ stopped: true, target: "terminal" })
      render(<TerminalArea />)
      await screen.findByTestId("stopped-agent-band")
      expect(onScreen("agent")).toBe(false)
    })

    it("shows the short card beside the dock, and the band leaves Start to it", async () => {
      mockState = dockState({ stopped: true, layout: "split" })
      render(<TerminalArea />)
      const card = await screen.findByTestId("stopped-agent-short-card")
      expect(card.textContent).toContain("Start picks up its last conversation.")
      expect(screen.getAllByRole("button", { name: "Start" })).toHaveLength(1)
      expect(screen.getByTestId("stopped-agent-band").querySelector("button")).toBeNull()
      expect(paneProps.map((p) => (p as { id: string }).id)).toEqual(["t1"])
    })

    it("says how a crashed first tab ended, with its last output a click away", async () => {
      mockState = dockState({ stopped: true, crashed: true })
      render(<TerminalArea />)
      const band = await screen.findByTestId("stopped-agent-band")
      expect(band.textContent).toContain("Its last run")
      fireEvent.click(screen.getByText("Last output"))
      expect(await screen.findByText("panic: boom")).toBeTruthy()
    })
  })
})

describe("terminalAreaModel", () => {
  const model = (over: Record<string, unknown> = {}) =>
    terminalAreaModel({
      target: { kind: "agent", sessionId: "s1", tabId: "b2" },
      spine: dormantSpine(),
      bootstrap: null,
      selectedSessionId: "s1",
      terminalEpoch: 0,
      startedDormantTabs: [],
      pendingSlotTab: {},
      ...over,
    } as never)

  it("keys an agent pane by the focused TAB and the reconnect epoch", () => {
    expect(model().targetId).toBe("b2")
    expect(model().paneKey).toBe("b2:0")
    expect(model({ terminalEpoch: 4 }).paneKey).toBe("b2:4")
  })

  it("keys a terminal pane by the terminal id, epoch-free", () => {
    const terminal = model({
      target: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
      terminalEpoch: 7,
    })
    expect(terminal.targetId).toBe("pt-1")
    expect(terminal.paneKey).toBe("pt-1")
    expect(terminal.focusedTab).toBeUndefined()
    expect(terminal.session).toBeUndefined()
    expect(terminal.tabs).toEqual([])
  })

  it("resolves the owning session and the focused tab for an agent", () => {
    const m = model()
    expect(m.session?.id).toBe("s1")
    expect(m.tabs.map((t) => t.id)).toEqual(["s1", "b2"])
    expect(m.focusedTab?.id).toBe("b2")
    expect(m.slotTabId).toBe("s1")
  })

  it("puts the pull-request lane below the terminal only on the bottom setting", () => {
    expect(model().bannerAtBottom).toBe(false)
    expect(
      model({ bootstrap: { pr_banner_position: "bottom" } }).bannerAtBottom,
    ).toBe(true)
    expect(
      model({ bootstrap: { pr_banner_position: "top" } }).bannerAtBottom,
    ).toBe(false)
  })

  it("has no pull request when the selected session names none", () => {
    expect(model().prs).toEqual([])
    expect(model({ selectedSessionId: null }).prs).toEqual([])
  })

  it("cards a dormant extra tab and leaves the live slot tab alone", () => {
    expect(model().dormant).toBe(true)
    expect(
      model({ target: { kind: "agent", sessionId: "s1", tabId: "s1" } }).dormant,
    ).toBe(false)
  })
})
