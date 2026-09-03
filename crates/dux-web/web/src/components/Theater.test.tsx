// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type * as React from "react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"

// What theater mode looks like on both shells: the chrome stacks leave, the
// floating pill arrives, and the one trigger flips. The store is mocked to a
// snapshot so each state is a render rather than a journey.

let mockState: DuxState
const toggleTheaterMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    toggleTheater: (...a: unknown[]) => toggleTheaterMock(...a),
    navigateUp: vi.fn(),
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
// The pane itself is lazy and needs xterm, which jsdom cannot back, so it is
// replaced by a marker that renders whatever OVERLAY the shell handed it. That
// is the point of the stand-in rather than a detail of it: the floating pill
// belongs inside the terminal's own positioned box (the pane column carries the
// compose row and the terminal keys under the terminal), so the assertion these
// shells owe is that they pass it DOWN rather than mounting it beside the pane.
// Where the pane then paints it is pinned against the real component in
// `TerminalPane.test.tsx`.
vi.mock("@/components/LazyTerminalPane", () => ({
  LazyTerminalPane: ({ overlay }: { overlay?: React.ReactNode }) => (
    <div data-testid="pane-stub">{overlay}</div>
  ),
}))
installBootStubs()
const { TerminalArea } = await import("./TerminalArea")
const { MobileShell } = await import("./MobileShell")
const { TheaterToggle } = await import("./TheaterToggle")
const { armTheaterToggleFocus } = await import("@/hooks/use-theater")

function tab(over: Partial<AgentTabView> & { id: string }): AgentTabView {
  return {
    provider: "claude",
    order: 0,
    working: false,
    typing: false,
    needs_attention: false,
    has_output: false,
    has_live_process: true,
    ...over,
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
    tabs: [tab({ id: "s1" }), tab({ id: "t2", provider: "codex" })],
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
    bootstrap: { title: "dux", agent_tabs_max: 4 },
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    theater,
    terminalEpoch: 0,
    startedDormantTabs: [],
    pendingSlotTab: {},
    routeNotFound: null,
    createTabInFlight: [],
    mobileScreen: "terminal",
    changes: { bySession: {} },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  toggleTheaterMock.mockReset()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("the desktop pane in theater", () => {
  it("keeps its chrome and shows no pill outside theater", () => {
    mockState = makeState(false)
    render(<TerminalArea />)
    expect(screen.queryByTestId("theater-pill")).toBeNull()
    expect(screen.getAllByRole("tab").length).toBeGreaterThan(0)
  })

  it("takes the tab strip away and hands the pill to the pane", () => {
    mockState = makeState(true)
    render(<TerminalArea />)
    const pill = screen.getByTestId("theater-pill")
    // Inside the pane, not beside it: beside it is where the input rows are.
    expect(screen.getByTestId("pane-stub").contains(pill)).toBe(true)
    for (const chrome of screen.getAllByTestId("theater-chrome")) {
      expect(chrome.dataset.hidden).toBe("true")
    }
    // The strip's pills are gone; the pill's own mini strip is collapsed until
    // its status half is pressed, so nothing claims to be a tab right now.
    expect(screen.queryAllByRole("tab").length).toBe(0)
  })
})

describe("the phone shell in theater", () => {
  it("keeps its header outside theater", () => {
    mockState = makeState(false)
    render(<MobileShell />)
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy()
    expect(screen.queryByTestId("theater-pill")).toBeNull()
  })

  it("takes the app header and the tab strip away, and hands the pill to the pane", () => {
    mockState = makeState(true)
    render(<MobileShell />)
    expect(
      screen.getByTestId("pane-stub").contains(screen.getByTestId("theater-pill")),
    ).toBe(true)
    expect(screen.getByTestId("theater-chrome").dataset.hidden).toBe("true")
  })
})

describe("the header trigger", () => {
  it("offers the way in, and says so", () => {
    mockState = makeState(false)
    render(<TheaterToggle />)
    const button = screen.getByRole("button", { name: "Theater mode" })
    expect(button.getAttribute("aria-pressed")).toBe("false")
    fireEvent.click(button)
    expect(toggleTheaterMock).toHaveBeenCalledTimes(1)
  })

  it("flips to the way out once theater is on", () => {
    mockState = makeState(true)
    render(<TheaterToggle />)
    const button = screen.getByRole("button", { name: "Leave theater mode" })
    expect(button.getAttribute("aria-pressed")).toBe("true")
  })

  it("takes focus back when an exit it did not press brings it on screen", () => {
    // The pill's own button and Escape both destroy the control that was just
    // used, so a keyboard would otherwise be left on the document body.
    mockState = makeState(true)
    const { rerender } = render(<TheaterToggle />)
    armTheaterToggleFocus()
    mockState = makeState(false)
    rerender(<TheaterToggle />)
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Theater mode" }),
    )
  })

  it("never grabs focus on an ordinary mount", () => {
    // A toggle that focused itself every time would pull the keyboard out of
    // the terminal on every page load.
    mockState = makeState(false)
    render(<TheaterToggle />)
    expect(document.activeElement).not.toBe(
      screen.getByRole("button", { name: "Theater mode" }),
    )
  })

  it("says the word on a computer and keeps the glyph alone on a phone", () => {
    // A mode with no other way in cannot be a lone glyph in a row of glyphs.
    // The word is "Theater", the name the exit, the menus and the docs use.
    // The label changes the WIDTH only: both size tokens are 32px tall.
    mockState = makeState(false)
    const { rerender } = render(<TheaterToggle />)
    const desktop = screen.getByRole("button", { name: "Theater mode" })
    expect(desktop.textContent).toBe("Theater")
    expect(desktop.className).toContain("h-8")

    rerender(<TheaterToggle size="mobile" />)
    const phone = screen.getByRole("button", { name: "Theater mode" })
    expect(phone.textContent).toBe("")
  })

  it("renders nothing with no pane focused, since there is nothing to fill", () => {
    mockState = { ...makeState(false), selectedTarget: null } as DuxState
    const { container } = render(<TheaterToggle />)
    expect(container.textContent).toBe("")
  })
})
