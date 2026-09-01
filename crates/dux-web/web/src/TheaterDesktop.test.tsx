// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"
import type { PanelProps } from "react-resizable-panels"

import type { DuxState, SessionView, AgentTabView } from "@/lib/store"

// THEATER IS ONE PANE AND THE WHOLE WINDOW on the desktop shell: the sidebar
// (its collapsed rail included) and the Changes pane are unmounted along with
// the header stack, and everything comes back when the mode ends. What this
// file pins is the shell's own layout; the pane's chrome is pinned in
// Theater.test.tsx and the capture/restore rules in lib/storeTheater.test.ts.

const recordedPanelProps: Array<PanelProps> = []
vi.mock("react-resizable-panels", () => {
  const Group = ({ children }: { children: ReactNode }) => (
    <div data-testid="panel-group">{children}</div>
  )
  const Panel = (props: PanelProps) => {
    recordedPanelProps.push(props)
    return <div data-panel-id={String(props.id)}>{props.children}</div>
  }
  const Separator = () => <div data-testid="separator" />
  return { Group, Panel, Separator }
})

vi.mock("@/components/Sidebar", () => ({
  AppSidebar: () => <div data-testid="app-sidebar" />,
}))
// The header stack is the reopen control's home: hiding the Changes pane
// unmounts the pane's own ⋯ menu, so the way back lives in the header (see
// InsetHeader.test.tsx, which pins the control and its changed-file count).
// Theater takes the whole header away, which is what takes that control with
// it, so the assertion this shell owes is that the header is not on screen.
vi.mock("@/components/InsetHeader", () => ({
  InsetHeader: () => <div data-testid="inset-header" />,
}))
vi.mock("@/components/ChangedFiles", () => ({
  ChangedFiles: () => <div data-testid="changed-files" />,
}))
// The pane itself needs xterm, which jsdom cannot back. The stand-in renders
// whatever overlay the pane was handed, which is where the floating pill lives.
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: ({ overlay }: { overlay?: ReactNode }) => (
    <div data-testid="pane-stub">{overlay}</div>
  ),
}))

let mockState: DuxState
// The one store write this shell makes. Spied rather than run, because what
// this file can prove is that the shortcut REACHES it; whether it then refuses
// is the store's own question, pinned against the real store in
// lib/storeTheater.test.ts.
const setSidebarOpenMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setSidebarOpen: (open: boolean) => setSidebarOpenMock(open),
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
const { useTheaterGesture } = await import("@/hooks/use-theater")
const { registerLayoutGestureHolder } = await import("@/lib/layoutGesture")
const { THEATER_TRANSITION_MS } = await import("@/lib/theater")

function tab(id: string, provider = "claude"): AgentTabView {
  return {
    id,
    provider,
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
  } as AgentTabView
}

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
    tabs: [tab("s1"), tab("t2", "codex")],
  } as unknown as SessionView
}

function makeState(
  theater: boolean,
  over: Partial<DuxState> = {},
): DuxState {
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
    mobileScreen: "terminal",
    mobileTopBarOverride: null,
    changes: { bySession: {} },
    ...over,
  } as unknown as DuxState
}

function terminalPanel() {
  return recordedPanelProps.find((p) => p.id === "terminal-pane")
}

beforeEach(() => {
  installBootStubs()
  recordedPanelProps.length = 0
  setSidebarOpenMock.mockReset()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("the desktop shell outside theater", () => {
  it("paints both side panels and the header", () => {
    mockState = makeState(false)
    render(<DesktopShell />)
    expect(screen.getByTestId("app-sidebar")).toBeTruthy()
    expect(screen.getByTestId("inset-header")).toBeTruthy()
    expect(screen.getByTestId("changed-files")).toBeTruthy()
    expect(screen.getByTestId("separator")).toBeTruthy()
    expect(terminalPanel()?.defaultSize).toBe("74%")
  })
})

describe("the desktop shell in theater", () => {
  beforeEach(() => {
    mockState = makeState(true)
  })

  it("unmounts the sidebar, rail and all", () => {
    render(<DesktopShell />)
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
    // The rail is the sidebar collapsed, so it is chrome too: a sidebar that
    // came back as a strip of icons would still be a second column.
    expect(document.querySelector('[data-slot="sidebar"]')).toBeNull()
  })

  it("unmounts the sidebar even when it was collapsed to the rail", () => {
    mockState = makeState(true, { sidebarOpen: false })
    render(<DesktopShell />)
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
  })

  it("unmounts the Changes pane and its divider", () => {
    render(<DesktopShell />)
    expect(screen.queryByTestId("changed-files")).toBeNull()
    expect(screen.queryByTestId("separator")).toBeNull()
    expect(
      recordedPanelProps.some((p) => p.id === "changes-pane"),
    ).toBe(false)
  })

  it("gives the terminal panel the whole width", () => {
    render(<DesktopShell />)
    expect(terminalPanel()?.defaultSize).toBe("100%")
  })

  it("leaves no way back to the Changes pane on screen", () => {
    // The reopen control lives in the header, which theater unmounts: the pill
    // is the only chrome the mode leaves behind.
    render(<DesktopShell />)
    expect(screen.queryByTestId("inset-header")).toBeNull()
    for (const chrome of screen.getAllByTestId("theater-chrome")) {
      expect(chrome.dataset.hidden).toBe("true")
    }
  })

  it("still hands the pane its floating pill", () => {
    render(<DesktopShell />)
    const pill = screen.getByTestId("theater-pill")
    expect(screen.getByTestId("pane-stub").contains(pill)).toBe(true)
  })

  it("hands the primitive's own shortcut to the store rather than to itself", () => {
    // The provider is CONTROLLED, which is the whole mechanism: its keyboard
    // shortcut can no longer flip a panel of its own behind the mode's back,
    // it can only ask the store, which refuses while theater is on and writes
    // nothing (pinned against the real store in lib/storeTheater.test.ts).
    render(<DesktopShell />)
    fireEvent.keyDown(window, { key: "b", ctrlKey: true })
    expect(setSidebarOpenMock).toHaveBeenCalledWith(false)
    // And with the store answering no, the panel stays gone: the shell renders
    // from the state, so an uncontrolled primitive flipping internally could
    // not bring it back either way.
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
  })

  it("asks the store from the rail state too, rather than toggling itself", () => {
    // The collapsed case takes the OTHER branch of the primitive's toggle, so
    // an uncontrolled provider would ask to open rather than to close. It must
    // still be the store that is asked.
    mockState = makeState(true, { sidebarOpen: false })
    render(<DesktopShell />)
    fireEvent.keyDown(window, { key: "b", ctrlKey: true })
    expect(setSidebarOpenMock).toHaveBeenCalledWith(true)
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
  })
})

describe("the side panels join the chrome's one gesture", () => {
  // Unmounting them widens the pane, so it must happen inside the SAME hold
  // the chrome collapse runs in: a second hold would be a second refit, and a
  // refit lands mid-transition at a geometry the layout is only passing
  // through.
  function Shell() {
    useTheaterGesture()
    return <DesktopShell />
  }

  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("costs one hold and one release entering, and one of each leaving", () => {
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = makeState(false)
    const { rerender } = render(<Shell />)

    mockState = makeState(true)
    rerender(<Shell />)
    expect(screen.queryByTestId("app-sidebar")).toBeNull()
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.release).toHaveBeenCalledTimes(1)

    mockState = makeState(false)
    rerender(<Shell />)
    expect(screen.getByTestId("app-sidebar")).toBeTruthy()
    expect(pane.hold).toHaveBeenCalledTimes(2)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.release).toHaveBeenCalledTimes(2)
    off()
  })
})
