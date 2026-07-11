// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"

// Control exactly what the store hands the component: override only `useDux`
// (keeping every other real store export intact) so the brand-block wiring
// (`bootstrap.title` to `resolveInstanceTitle` to the rendered wordmark) is
// exercised end to end. This guards against a regression that silently swaps the
// title for another field (e.g. the version) or re-hardcodes "dux". `addTab` and
// `selectSession` are ALSO overridden (as spies) so call sites can be asserted
// without making a real network request or mutating the real store singleton.
let mockState: DuxState
const addTabMock = vi.fn()
const selectSessionMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: addTabMock,
    selectSession: selectSessionMock,
  }
})

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what a row's tooltip is wired to reveal.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: ReactNode
    content: ReactNode
  }) => (
    <>
      {children}
      <div data-testid="tooltip-content">{content}</div>
    </>
  ),
}))

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
  selectSessionMock.mockClear()
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
  // Collapse now happens only through the footer button or the Ctrl/Cmd-b shortcut
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

// A second project with its own single agent, so the rail's "project order,
// then agent order" claim is actually exercised across projects (not just
// within one).
function makeTwoProjectSpine(): DuxState["spine"] {
  const base = makeSessionSpine(1) as unknown as {
    projects: unknown[]
    sessions: {
      id: string
      project_id: string
      status: string
      working: boolean
      needs_attention: boolean
    }[]
    sidebar: { groups: unknown[]; agentless_start: null }
  }
  const secondProject = {
    id: "p2",
    name: "Other",
    path: "/tmp/p2",
    default_provider: "claude",
    current_branch: "main",
    branch_status: "leading",
  }
  const secondSession = {
    ...base.sessions[0],
    id: "s2",
    project_id: "p2",
    status: "detached",
    working: false,
    needs_attention: false,
  }
  return {
    ...base,
    projects: [...base.projects, secondProject],
    sessions: [...base.sessions, secondSession],
    sidebar: {
      groups: [
        ...base.sidebar.groups,
        { project_id: "p2", name: "Other", orphaned: false },
      ],
      agentless_start: null,
    },
  } as unknown as DuxState["spine"]
}

describe("AppSidebar collapsed icon rail", () => {
  it("renders one agent icon per session across projects instead of project folders", () => {
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    // One button per agent, in project order then agent order: s1 (Repo) then
    // s2 (Other). No project folder affordance ("Project actions" menu) inside
    // the rail — only agent icons.
    expect(rail.querySelectorAll('[aria-label="Session actions"]').length).toBe(
      0,
    )
    expect(
      rail.querySelectorAll('[aria-label="Project actions"]').length,
    ).toBe(0)
    const buttons = rail.querySelectorAll("button")
    expect(buttons.length).toBe(2)
    expect(buttons[0].getAttribute("aria-label")).toContain("Repo")
    expect(buttons[1].getAttribute("aria-label")).toContain("Other")
  })

  it("clicking an agent icon selects that agent", () => {
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const buttons = rail.querySelectorAll("button")
    fireEvent.click(buttons[1])
    expect(selectSessionMock).toHaveBeenCalledWith("s2")
  })

  it("shows the selected agent's icon in the active state", () => {
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      selectedTarget: { kind: "agent", sessionId: "s2" },
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const buttons = rail.querySelectorAll("button")
    expect(buttons[0].hasAttribute("data-active")).toBe(false)
    expect(buttons[1].hasAttribute("data-active")).toBe(true)
  })

  it("carries the agent name, project name, and status in the tooltip", () => {
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const tooltips = rail.querySelectorAll('[data-testid="tooltip-content"]')
    expect(tooltips.length).toBe(2)
    expect(tooltips[0].textContent).toContain("Repo")
    expect(tooltips[0].textContent?.toLowerCase()).toContain("active")
    expect(tooltips[1].textContent).toContain("Other")
    expect(tooltips[1].textContent?.toLowerCase()).toContain("detached")
  })

  it("carries the working bob and attention blink classes on the rail icon", () => {
    const spine = makeTwoProjectSpine() as unknown as {
      sessions: { working: boolean; needs_attention: boolean }[]
    }
    spine.sessions[0].working = true
    spine.sessions[1].needs_attention = true
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
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const buttons = rail.querySelectorAll("button")
    // s1 is active + working: its Bot icon bobs.
    expect(
      buttons[0].querySelector("svg")?.getAttribute("class"),
    ).toContain("animate-agent-working")
    // s2 needs attention: its icon wrapper carries the amber blink.
    expect(
      buttons[1].querySelector("[aria-label='Needs attention']"),
    ).toBeTruthy()
  })

  it("scrolls internally in icon mode instead of relying on SidebarContent's clipped overflow", () => {
    // jsdom does not lay out or clip content, so it can't observe the actual
    // clipping bug (icons past ~a screenful becoming unreachable below the
    // fold). This asserts the class contract instead: the rail must carry its
    // own bounded, scrollable region (min-h-0 so it can shrink inside the
    // flex column, overflow-y-auto so it scrolls before ever hitting
    // SidebarContent's group-data-[collapsible=icon]:overflow-hidden).
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    expect(rail.className).toContain("overflow-y-auto")
    expect(rail.className).toContain("min-h-0")
    expect(rail.className).toContain("no-scrollbar")
  })

  it("forces the rail icon to size-4.5 with !important so it wins over SidebarMenuButton's descendant [&_svg]:size-4 rule", () => {
    // jsdom doesn't compute layout/CSS cascade, so it can't measure the
    // rendered pixel size. This asserts the class contract instead: the
    // important-variant utility must be present, since a plain `size-4.5`
    // loses to SidebarMenuButton's higher-specificity descendant selector.
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const icon = rail.querySelector("button svg")
    expect(icon?.getAttribute("class")).toContain("size-4.5!")
  })

  it("renders the shared AgentVitalsTooltip content (branch and worktree rows) for a rail icon", () => {
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const rail = screen.getByTestId("collapsed-agent-rail")
    const tooltips = rail.querySelectorAll('[data-testid="tooltip-content"]')
    // makeSessionSpine's session s1 carries branch_name "main" and worktree_path
    // "/tmp/p1" — both should surface as vitals rows, proving the rail uses the
    // full AgentVitalsTooltip content component and not the old 2-line tooltip.
    expect(tooltips[0].textContent).toContain("main")
    expect(tooltips[0].textContent).toContain("/tmp/p1")
  })
})

describe("AppSidebar expanded agent row vitals tooltip", () => {
  it("wraps the agent name in a SimpleTooltip carrying the shared AgentVitalsTooltip content", () => {
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

    const tooltips = screen.getAllByTestId("tooltip-content")
    const vitalsTooltip = tooltips.find(
      (t) => t.textContent?.includes("/tmp/p1"),
    )
    expect(vitalsTooltip).toBeTruthy()
    // Status line + project name from the row context.
    expect(vitalsTooltip?.textContent).toContain("Repo")
    expect(vitalsTooltip?.textContent?.toLowerCase()).toContain("active")
  })
})
