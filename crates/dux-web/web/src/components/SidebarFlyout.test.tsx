// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"

// The floating sidebar a mouse opens from the collapsed rail. These tests run the
// REAL tooltip (no SimpleTooltip mock), because what they pin is which cards a
// mouse hover reveals on the rail.
let mockState: DuxState
const selectSessionMock = vi.fn()
const selectTerminalMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    selectSession: selectSessionMock,
    selectTerminal: selectTerminalMock,
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
  // The real tooltip positions its popup with a ResizeObserver, which jsdom lacks.
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
const { AppSidebar } = await import("./Sidebar")

function managedSession(id: string, branch: string, status: string) {
  return {
    id,
    slot_tab_id: id,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: branch,
      initial_branch: "",
      branch_provenance: "created",
      source_branch: "",
      worktree_path: `/tmp/p1/${branch}`,
    },
    title: null,
    provider: "claude",
    status,
    auto_reopen_enabled: false,
    tabs: [
      {
        id,
        provider: "claude",
        order: 0,
        working: false,
        has_output: false,
        has_live_process: status === "active",
      },
    ],
    has_output: false,
    working: false,
    needs_attention: false,
    typing: false,
  }
}

// One project with a running agent and a parked one (the Inactive tail), plus a
// standalone terminal.
function workspace(): DuxState {
  return {
    spine: {
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
        managedSession("s1", "live-work", "active"),
        managedSession("s2", "parked-work", "detached"),
      ],
      terminals: [
        {
          id: "t1",
          owner: { kind: "standalone", cwd_label: "~/work" },
          label: "Terminal 1",
          has_output: false,
          working: false,
          typing: false,
          foreground_cmd: null,
          sort_order: 0,
        },
      ],
      sidebar: {
        groups: [{ project_id: "p1", name: "Repo", orphaned: false }],
        agentless_start: null,
      },
    },
    bootstrap: { title: "dux", dux_version: "v1", available_providers: ["claude"] },
    selectedTarget: null,
    sidebarWidth: "18rem",
    createTabInFlight: [],
  } as unknown as DuxState
}

const tree = () => (
  <SidebarProvider defaultOpen={false}>
    <AppSidebar />
  </SidebarProvider>
)

const sidebar = () => document.querySelector('[data-slot="sidebar"]') as HTMLElement
const container = () =>
  document.querySelector('[data-slot="sidebar-container"]') as HTMLElement
// The full sidebar is what shows whenever the sidebar is not at icon width.
const floating = () => sidebar().getAttribute("data-collapsible") !== "icon"
const railIcon = (label: string) =>
  screen.getByTestId("collapsed-agent-rail").querySelector(
    `button[aria-label="${label}"]`,
  ) as HTMLElement
const listRow = (key: string) =>
  [...document.querySelectorAll(`[data-sidebar-row="${key}"]`)].find(
    (el) => !el.closest('[data-testid="collapsed-agent-rail"]'),
  ) as HTMLElement

function hover(el: HTMLElement) {
  fireEvent.pointerEnter(el, { pointerType: "mouse" })
  fireEvent.mouseEnter(el)
  fireEvent.pointerMove(el, { pointerType: "mouse" })
  fireEvent.mouseMove(el)
}

function advance(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms)
  })
}

beforeEach(() => {
  installBootStubs()
  vi.useFakeTimers()
  selectSessionMock.mockClear()
  selectTerminalMock.mockClear()
  mockState = workspace()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("AppSidebar floating sidebar from the collapsed rail", () => {
  it("opens the whole sidebar over the page after a mouse rests on an icon, and choosing an agent selects it and closes", () => {
    render(tree())
    hover(railIcon("live-work (Repo)"))
    advance(149)
    expect(floating()).toBe(false)
    advance(1)
    expect(floating()).toBe(true)

    // The page layout does not change: the sidebar is still collapsed, and the
    // panel is the fixed container, not the gap that takes layout space.
    expect(sidebar().getAttribute("data-state")).toBe("collapsed")
    expect(container().className).toContain("fixed")
    // Every section of the expanded sidebar is in it.
    expect(listRow("agent:s1")).toBeTruthy()
    expect(screen.getByRole("button", { name: /Inactive/ })).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: /Inactive/ }))
    expect(listRow("agent:s2")).toBeTruthy()
    expect(listRow("terminal:t1")).toBeTruthy()

    fireEvent.click(listRow("agent:s2").querySelector("button")!)
    expect(selectSessionMock).toHaveBeenCalledWith("s2")
    expect(floating()).toBe(false)
  })

  it("stays open while the pointer moves from the icon into the panel, closes after leaving it, and closes on Escape", () => {
    render(tree())
    const icon = railIcon("live-work (Repo)")
    hover(icon)
    advance(150)
    expect(floating()).toBe(true)

    fireEvent.pointerLeave(icon, { pointerType: "mouse" })
    hover(listRow("agent:s1"))
    advance(1000)
    expect(floating()).toBe(true)

    fireEvent.pointerLeave(container(), { pointerType: "mouse" })
    advance(299)
    expect(floating()).toBe(true)
    advance(1)
    expect(floating()).toBe(false)

    hover(railIcon("live-work (Repo)"))
    advance(150)
    expect(floating()).toBe(true)
    fireEvent.keyDown(document, { key: "Escape" })
    expect(floating()).toBe(false)
  })

  it("shows no tooltip card on a mouse hover, and a keyboard focus opens no panel", () => {
    render(tree())
    hover(railIcon("Terminal (~/work)"))
    advance(1000)
    expect(document.querySelector('[data-slot="tooltip-content"]')).toBeNull()

    fireEvent.keyDown(document, { key: "Escape" })
    fireEvent.pointerLeave(container(), { pointerType: "mouse" })
    advance(1000)
    expect(floating()).toBe(false)
    act(() => railIcon("live-work (Repo)").focus())
    advance(1000)
    expect(floating()).toBe(false)
  })
})
