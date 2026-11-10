// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, screen, within } from "@testing-library/react"
import type { ReactNode } from "react"
import type { PanelProps } from "react-resizable-panels"

import type { DuxState, SessionView } from "@/lib/store"

// THEATER'S COVER EXCEPTION ON THE COMPUTER LAYOUT. That layout is chosen by
// width alone, so a tablet with no keyboard lands on it and has no Escape; under
// a full-pane cover the pill is withheld, and the top bar carrying the Theater
// button is then the only way out. The REAL InsetHeader renders here (its
// sibling shell test stubs it), because the button being on screen is the point.

vi.mock("react-resizable-panels", () => {
  const Group = ({ children }: { children: ReactNode }) => (
    <div data-testid="panel-group">{children}</div>
  )
  const Panel = (props: PanelProps) => (
    <div data-panel-id={String(props.id)}>{props.children}</div>
  )
  const Separator = () => <div data-testid="separator" />
  return { Group, Panel, Separator }
})

vi.mock("@/components/Sidebar", () => ({
  AppSidebar: () => <div data-testid="app-sidebar" />,
}))
vi.mock("@/components/ChangedFiles", () => ({
  ChangedFiles: () => <div data-testid="changed-files" />,
}))
// xterm cannot render in jsdom; the stand-in renders whatever overlay it is
// handed, which is where the floating pill would be.
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: ({ overlay }: { overlay?: ReactNode }) => (
    <div data-testid="pane-stub">{overlay}</div>
  ),
  // An agent's screen is its stack, the same stand-in with the same overlay.
  LazyTerminalStack: ({ overlay }: { overlay?: ReactNode }) => (
    <div data-testid="pane-stub">{overlay}</div>
  ),
}))

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, setSidebarOpen: vi.fn() }
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
const { DesktopShell } = await import("./App")
const { registerPaneCover, resetPaneCovers } = await import("@/lib/paneCover")
const { registerLayoutGestureHolder } = await import("@/lib/layoutGesture")
const { THEATER_TRANSITION_MS } = await import("@/lib/theater")

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [
      {
        id: "s1",
        provider: "claude",
        order: 0,
        working: false,
        typing: false,
        needs_attention: false,
        has_output: false,
        has_live_process: true,
      },
    ],
  } as unknown as SessionView
}

function makeState(theater: boolean): DuxState {
  return {
    spine: {
      projects: [{ id: "p1", name: "dux" }],
      sessions: [session()],
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: { title: "dux", agent_tabs_max: 4, show_changes_pane: true },
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    theater,
    theaterLayout: null,
    sidebarOpen: true,
    sidebarWidth: "18rem",
    changesPaneOverride: null,
    changesPanePercent: 26,
    terminalEpoch: 0,
    startedDormantTabs: [],
    pendingSlotTab: {},
    routeNotFound: null,
    createTabInFlight: [],
    stackLayouts: {},
    stackTerminals: {},
    stackTabs: {},
    pendingTerminalOrder: null,
    mobileScreen: "terminal",
    changes: { bySession: {} },
  } as unknown as DuxState
}

/// The header's own theater trigger. Scoped to the header element, because in
/// theater the pill's exit carries the same name, and the question here is
/// whether the top BAR came back.
function theaterButton(): HTMLElement | null {
  const header = document.querySelector("header")
  if (!header) return null
  return within(header).queryByRole("button", { name: "Leave theater mode" })
}

beforeEach(() => {
  installBootStubs()
  mockState = makeState(true)
})

afterEach(() => {
  cleanup()
  resetPaneCovers()
  vi.unstubAllGlobals()
})

describe("the computer layout under a full-pane cover", () => {
  it("has no way out at all with nothing covering the pane", () => {
    render(<DesktopShell />)
    expect(theaterButton()).toBeNull()
  })

  it("brings the top bar back, with the Theater button in it", () => {
    registerPaneCover("s1", true)
    render(<DesktopShell />)
    expect(theaterButton()).toBeTruthy()
  })

  it("leaves the side panels where the mode put them", () => {
    // Only the way out comes back. The sidebar and the Changes pane are not a
    // way out of anything, and returning them would undo the mode.
    registerPaneCover("s1", true)
    render(<DesktopShell />)
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
    expect(screen.queryByTestId("changed-files")).toBeNull()
  })

  it("keeps the bar away under the transparent spinner cover", () => {
    registerPaneCover("s1", false)
    render(<DesktopShell />)
    expect(theaterButton()).toBeNull()
  })

  it("reads the cover under the SELECTED pane and not some other one", () => {
    registerPaneCover("t2", true)
    render(<DesktopShell />)
    expect(theaterButton()).toBeNull()
  })
})

// The header coming and going changes the terminal's height, and the mode never
// moves while it happens, so the mode's own gesture never sees it.
describe("the refit the returning bar costs", () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it("holds the pane once for the cover, and once again when it goes", () => {
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    render(<DesktopShell />)
    // The pane's own first verdict is initial state, whatever it says.
    let retire = () => {}
    act(() => {
      retire = registerPaneCover("s1", false)
    })
    expect(pane.hold).not.toHaveBeenCalled()

    act(() => {
      retire()
      retire = registerPaneCover("s1", true)
    })
    expect(pane.hold).toHaveBeenCalledTimes(1)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.release).toHaveBeenCalledTimes(1)

    // A cover going is the pane publishing again, not the entry going away: an
    // entry leaves only when the pane itself does, and there is nothing to
    // refit then.
    act(() => {
      retire()
      retire = registerPaneCover("s1", false)
    })
    expect(pane.hold).toHaveBeenCalledTimes(2)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.release).toHaveBeenCalledTimes(2)
    off()
  })
})
