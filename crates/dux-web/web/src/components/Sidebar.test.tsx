// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"

// Control exactly what the store hands the component: override only `useDux`
// (keeping every other real store export intact) so the brand-block wiring
// (`bootstrap.title` to `resolveInstanceTitle` to the rendered wordmark) is
// exercised end to end. This guards against a regression that silently swaps the
// title for another field (e.g. the version) or re-hardcodes "dux". `addTab` is
// ALSO overridden (as a spy) so the "Add tab" menu item test can assert it was
// called without making a real network request.
let mockState: DuxState
const addTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, addTab: addTabMock }
})

// The real store module boots on import (it reads localStorage and fires the
// bootstrap fetch + reconnect timers). jsdom doesn't expose localStorage/fetch as bare
// globals, so stub them BEFORE the component (and the store behind it) loads, and
// keep the boot off the network so these render tests stay hermetic.
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
  // jsdom does not implement matchMedia, which the sidebar's responsive hook uses.
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
const { AppSidebar } = await import("./Sidebar")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: { title: "dux #1", dux_version: "v9.9.9" },
    selectedTarget: null,
    pendingSessionOrder: null,
    pendingProjectOrder: null,
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

describe("AppSidebar brand block", () => {
  it("renders the configured instance title with the version below it", () => {
    mockState = makeState()
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    // The configured title is the wordmark; the version is a separate line. Both
    // present and distinct proves the two fields were not swapped.
    expect(screen.getByText("dux #1")).toBeTruthy()
    expect(screen.getByText("v9.9.9")).toBeTruthy()
  })

  it("falls back to 'dux' when no title is configured", () => {
    mockState = makeState({
      bootstrap: { dux_version: "v9.9.9" } as DuxState["bootstrap"],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    expect(screen.getByText("dux")).toBeTruthy()
  })
})

describe("AppSidebar agent ⋯ menu — Add tab (G7)", () => {
  // Before this fix the web had NO way to create a session's first extra tab:
  // `addTab`'s only call site was the in-strip "+", which only renders once a
  // session already has two or more tabs — so a fresh 1-tab session could never
  // reach 2. This menu item is the affordance that closes that gap.
  it("is present and enabled for a session with only one tab, and calls addTab", () => {
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude", "codex"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText("New agent tab…"))
    // makeSessionSpine's project default_provider is "claude".
    expect(screen.getByText("default")).toBeTruthy()
    expect(screen.getByText("codex")).toBeTruthy()
    fireEvent.click(screen.getByText("codex"))
    expect(addTabMock).toHaveBeenCalledWith("s1", "codex")
  })
})

describe("AppSidebar attention dot", () => {
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    expect(screen.queryByLabelText("Needs attention")).toBeNull()
  })
})

describe("AppSidebar resize affordances", () => {
  // The agents panel resizes by dragging only — matching the changes panel. The
  // old shadcn `SidebarRail` doubled as a click-near-the-edge collapse target; it
  // was removed so a stray click by the splitter can no longer collapse the panel.
  // Collapse now happens only through the footer button or the Ctrl/Cmd-B shortcut
  // (the latter lives in SidebarProvider), and the edge offers drag-to-resize when
  // expanded and click-to-expand when collapsed.
  it("exposes the drag handle but not the click-to-collapse rail", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    // No click-to-collapse rail near the splitter.
    expect(container.querySelector('[data-sidebar="rail"]')).toBeNull()

    // Drag-to-resize handle is present.
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()

    // The dedicated collapse button in the footer stays.
    expect(
      container.querySelector('[data-sidebar="trigger"]'),
    ).toBeTruthy()
  })

  it("dragging the handle resizes and persists the panel width", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    expect(handle).toBeTruthy()
    // jsdom doesn't implement pointer capture; the handler calls it on press.
    handle.setPointerCapture = () => {}

    // Press, drag to x=400, release. 400px is inside [224, 448] (14rem..28rem),
    // so it lands at exactly 25rem and is persisted on release. This exercises
    // the real window-listener drag path, not just the element's presence.
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 240 })
    window.dispatchEvent(new MouseEvent("pointermove", { clientX: 400 }))
    window.dispatchEvent(new MouseEvent("pointerup", { clientX: 400 }))

    expect(localStorage.getItem("dux:sidebar-width")).toBe("25rem")
  })

  it("clamps the dragged width to the maximum", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    handle.setPointerCapture = () => {}

    // 9999px is well past the 28rem (448px) cap, so it must clamp to 28rem.
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 240 })
    window.dispatchEvent(new MouseEvent("pointerup", { clientX: 9999 }))

    expect(localStorage.getItem("dux:sidebar-width")).toBe("28rem")
  })

  it("offers a click-to-expand edge when collapsed, never a collapse target", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    // Collapsed: the drag handle is gone, replaced by an expand-only strip; the
    // old collapse rail is still absent.
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeNull()
    expect(container.querySelector('[data-sidebar="rail"]')).toBeNull()
    const expand = container.querySelector(
      '[data-sidebar="expand-handle"]',
    ) as HTMLElement
    expect(expand).toBeTruthy()

    // Clicking the strip expands the panel: the drag handle returns and the
    // expand strip is replaced. (If the strip could collapse, this would loop.)
    fireEvent.click(expand)
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeNull()
  })
})
