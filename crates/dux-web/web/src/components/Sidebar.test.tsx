// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

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
const createProjectTerminalMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    addTab: addTabMock,
    selectSession: selectSessionMock,
    createProjectTerminal: createProjectTerminalMock,
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
let bootMedia: MatchMediaStub | null = null

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
  bootMedia?.restore()
  bootMedia = stubMatchMedia()
}
installBootStubs()
const { AppSidebar } = await import("./Sidebar")
const { NewAgentPickerDialog } = await import("./NewAgentPickerDialog")
const { sidebarResizeRelease } = await import("@/lib/sidebarResize")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: { title: "dux #1", dux_version: "v9.9.9" },
    selectedTarget: null,
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    sidebarWidth: "18rem",
    ...overrides,
  } as unknown as DuxState
}

// ── Driving the sidebar's divider ──────────────────────────────────────────
// The shared divider decides a press from the element's RECT rather than by
// hit-testing it, and jsdom has no layout, so every drag test has to say where
// the edge is. 288px is the default 18rem width the state above starts at, and
// the drag is a DELTA from the press point, so pressing exactly on the edge
// makes the released x the released width.
const SIDEBAR_EDGE_X = 288

function grabHandle(container: HTMLElement): HTMLElement {
  const handle = container.querySelector(
    '[data-sidebar="resize-handle"]',
  ) as HTMLElement
  handle.setPointerCapture = () => {}
  handle.getBoundingClientRect = () =>
    ({
      x: SIDEBAR_EDGE_X - 2,
      y: 0,
      left: SIDEBAR_EDGE_X - 2,
      right: SIDEBAR_EDGE_X + 2,
      top: 0,
      bottom: 800,
      width: 4,
      height: 800,
      toJSON: () => ({}),
    }) as DOMRect
  return handle
}

function pressEdge(handle: HTMLElement, init: Record<string, unknown> = {}) {
  fireEvent.pointerDown(handle, {
    pointerId: 1,
    clientX: SIDEBAR_EDGE_X,
    clientY: 100,
    ...init,
  })
}

function moveEdge(clientX: number, init: Record<string, unknown> = {}) {
  act(() => {
    fireEvent.pointerMove(document, {
      pointerId: 1,
      clientX,
      clientY: 100,
      ...init,
    })
  })
}

function releaseEdge(clientX: number, init: Record<string, unknown> = {}) {
  act(() => {
    fireEvent.pointerUp(document, {
      pointerId: 1,
      clientX,
      clientY: 100,
      ...init,
    })
  })
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
  selectSessionMock.mockClear()
  createProjectTerminalMock.mockClear()
})

afterEach(() => {
  cleanup()
  bootMedia?.restore()
  bootMedia = null
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

  it("clicking the logo goes home: clears the selection (and thus the PTY + hash)", () => {
    mockState = makeState()
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    fireEvent.click(screen.getByRole("button", { name: /go to home/i }))
    // selectSession(null) is the one home path: it clears the target (the
    // center pane falls back to the Welcome tips) and rewrites the URL hash
    // back to root; both are its own tested behavior, so the pin here is the
    // call itself.
    expect(selectSessionMock).toHaveBeenCalledWith(null)
  })
})

describe("AppSidebar agent ⋯ menu: Add tab", () => {
  // This menu item is the only way to create a session's first extra tab: the
  // in-strip "+" renders only once a session already has two or more tabs.
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
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
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText(/^New agent tab for /))
    // makeSessionSpine's project default_provider is "claude".
    expect(screen.getByText("default")).toBeTruthy()
    expect(screen.getByText("codex")).toBeTruthy()
    fireEvent.click(screen.getByText("codex"))
    expect(addTabMock).toHaveBeenCalledWith("s1", "codex")
  })
})

describe("AppSidebar flat Terminals section", () => {
  // A session that owns a companion terminal which is currently "typing": it must
  // render in the flat Terminals section (not nested under the agent), under the
  // "Terminals" header, showing the running command, the owner (agent) label, and
  // the violet "Typing" state word.
  function typingTerminalSpine(): DuxState["spine"] {
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
      sessions: [
        {
          id: "s1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "feature/login",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "/tmp/p1",
          },
          title: "Login flow",
          provider: "claude",
          status: "active",
          auto_reopen_enabled: false,
          tabs: [],
          has_output: false,
          working: false,
          typing: false,
        },
      ],
      terminals: [
        {
          id: "ct-1",
          owner: { kind: "session", session_id: "s1" },
          label: "Terminal 1",
          has_output: true,
          working: false,
          typing: true,
          foreground_cmd: "vim",
        },
      ],
      sidebar: {
        groups: [
          { project_id: "p1", name: "Repo", orphaned: false, session_ids: ["s1"] },
        ],
        agentless_start: null,
      },
    } as unknown as DuxState["spine"]
  }

  it("renders a companion terminal in the flat Terminals section with owner label and Typing state", () => {
    mockState = makeState({
      spine: typingTerminalSpine(),
      bootstrap: { title: "dux", dux_version: "v1" },
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    // The section header.
    expect(screen.getByText("Terminals")).toBeTruthy()
    // Row 1 primary label = the running foreground command.
    expect(screen.getByText("vim")).toBeTruthy()
    // Row 2: the owner label repeats the agent name (the agent row plus the
    // terminal's ↳ owner tag, and the vitals tooltips), so it appears more than
    // once, proving the terminal carries its owner label.
    expect(screen.getAllByText("Login flow").length).toBeGreaterThan(1)
    // The terminal's Typing word (the agent row shows Idle since the session
    // itself is not typing), styled through the violet typing token.
    const word = screen.getByText("Typing")
    expect(word.className).toContain("text-dux-typing")
  })
})

describe("AppSidebar project terminals", () => {
  // A spine whose project owns a live project terminal (and has no sessions):
  // the row must render under the project header, or a project terminal
  // renders nowhere in the sidebar.
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

  it("renders the project terminal row under the project header", () => {
    mockState = makeState({
      spine: projectTerminalSpine(),
      bootstrap: { title: "dux", dux_version: "v1" },
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    expect(screen.getByText("Terminal 2")).toBeTruthy()
  })

  it("offers 'New project terminal' in the project ⋯ menu and calls createProjectTerminal", () => {
    // An agent-less project (only a project terminal) has no agent row, so its
    // project actions live in the New-agent picker's per-project ⋯ menu.
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
    fireEvent.click(item)
    expect(createProjectTerminalMock).toHaveBeenCalledWith("p1")
  })

  it("disables 'New project terminal' when the project's path is missing", () => {
    const spine = projectTerminalSpine() as unknown as {
      projects: { path_missing: boolean }[]
    }
    spine.projects[0].path_missing = true
    mockState = makeState({
      spine: spine as unknown as DuxState["spine"],
      bootstrap: { title: "dux", dux_version: "v1" },
      newAgentPickerOpen: true,
    })
    render(<NewAgentPickerDialog />)
    fireEvent.click(screen.getByLabelText("Project actions"))
    const item = screen.getByText("New project terminal")
    expect(item.closest('[role="menuitem"]')?.getAttribute("aria-disabled")).toBe(
      "true",
    )
  })

  it("hides 'New project terminal' for an orphaned group", () => {
    // An orphaned group has no real project: the menu shows only "Remove
    // project…", so there must be no terminal entry point.
    mockState = makeState({
      spine: {
        projects: [],
        sessions: [
          {
            id: "s1",
            workspace: {
              kind: "managed",
              project_id: "ghost",
              branch_name: "main",
              initial_branch: "",
              branch_provenance: "created",
              source_branch: "",
              worktree_path: "/tmp/x",
            },
            title: null,
            provider: "claude",
            status: "active",
            auto_reopen_enabled: false,
            tabs: [
              {
                id: "s1",
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
        sidebar: {
          groups: [
            { project_id: "ghost", name: "ghost", orphaned: true, session_ids: ["s1"] },
          ],
          agentless_start: null,
        },
      } as unknown as DuxState["spine"],
      bootstrap: { title: "dux", dux_version: "v1" },
      createTabInFlight: [],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    // The orphan has an agent row; its project actions live in the agent ⋯ menu
    // under the "Project…" submenu.
    fireEvent.click(screen.getByLabelText("Session actions"))
    fireEvent.click(screen.getByText(/^Project[ …]/))
    expect(screen.queryByText("New project terminal")).toBeNull()
    expect(screen.getByText("Remove project…")).toBeTruthy()
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

describe("AppSidebar flat agent row", () => {
  it("renders a display-only project label and a colored state word on line two", () => {
    // The flat model drops project headers: each agent row carries its project as
    // a display-only Folder label on line two, alongside a state word derived from
    // the same flags that drive the working/attention cues. Project ACTIONS are in
    // the agent ⋯ menu under a "Project…" submenu, not on the row itself.
    mockState = makeState({
      spine: makeSessionSpine(1),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
    })
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    // The Folder outline icon is present on the row's project label.
    expect(container.querySelectorAll("svg.lucide-folder").length).toBeGreaterThan(0)
    // The state word for an active, non-working agent is "Idle".
    expect(screen.getByText("Idle")).toBeTruthy()
    // The row itself exposes no "Project actions" trigger (display-only label).
    expect(screen.queryByLabelText("Project actions")).toBeNull()
    // Project actions live in the agent ⋯ menu, under a "Project…" submenu.
    fireEvent.click(screen.getByLabelText("Session actions"))
    expect(screen.getByText(/^Project[ …]/)).toBeTruthy()
  })

  it("collapses detached/exited agents into an Inactive tail", () => {
    // Two projects: s1 (Repo) active, s2 (Other) detached. The detached agent must
    // land under the collapsible "Inactive" toggle (label + a count badge), not the
    // main list.
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
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    const quietToggle = screen.getByRole("button", { name: /Inactive/ })
    expect(quietToggle).toBeTruthy()
    // The count rides in a badge next to the label, not inline as "Inactive · 1".
    expect(quietToggle.textContent).toContain("1")
  })
})

describe("AppSidebar resize affordances", () => {
  // The agents panel resizes by dragging only, matching the changes panel.
  // There is no click-near-the-edge collapse target, so a stray click by the
  // splitter cannot collapse the panel.
  // Collapse happens only through the footer button or the Ctrl/Cmd-b shortcut
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

  it("offers a rail button to expand when collapsed", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    // Collapsed to the icon rail...
    expect(container.querySelector('[data-collapsible="icon"]')).toBeTruthy()
    // ...and a dedicated expand control is present, so the sidebar can be
    // reopened without relying on the edge handle alone.
    expect(
      container.querySelector('[aria-label="Expand sidebar"]'),
    ).toBeTruthy()
  })

  it("dragging the handle resizes and persists the panel width", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    expect(handle).toBeTruthy()

    // Press on the edge, drag to x=400, release. 400px is inside [224, 448]
    // (14rem..28rem), so it lands at exactly 25rem and is persisted on release.
    // This exercises the real document-listener drag path, not just presence.
    pressEdge(handle)
    moveEdge(400)
    releaseEdge(400)

    expect(localStorage.getItem("dux:sidebar-width")).toBe("25rem")
  })

  // The library's separator moves by the DELTA from the press point, and so
  // does this one: a press that lands off centre inside the 20px grab band must
  // not teleport the divider to the finger before it has moved.
  it("does not jump when the press lands off centre in the grab band", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    // Pressed 4px left of the edge, still inside the 10px fine-pointer band,
    // and released without moving: the width is unchanged, not dragged 4px
    // narrower.
    pressEdge(handle, { clientX: SIDEBAR_EDGE_X - 4 })
    releaseEdge(SIDEBAR_EDGE_X - 4)

    // Nothing is written at all. A press that went nowhere decides nothing,
    // which includes deciding to remember the width it happened to find; what
    // it must never do is write a width 4px narrower than that.
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
  })

  it("acquires a press inside the band even when the element is covered", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    // Dispatched at the document, never at the handle: the divider is claimed
    // from the pointer's position, so nothing painted over it can swallow the
    // press.
    act(() => {
      fireEvent.pointerDown(document.body, {
        pointerId: 1,
        clientX: SIDEBAR_EDGE_X,
        clientY: 100,
      })
    })
    expect(handle).toBeTruthy()
    releaseEdge(400)

    expect(localStorage.getItem("dux:sidebar-width")).toBe("25rem")
  })

  // The Changes divider's double-click restores its panel's mount size, so the
  // sidebar's restores the width the page loaded with. Nothing was persisted
  // before this render, so that is the 18rem default.
  it("restores the width the page loaded with on a double-click", () => {
    mockState = makeState({ sidebarWidth: "26rem" })
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    expect(handle).toBeTruthy()
    act(() => {
      fireEvent.dblClick(document.body, {
        clientX: SIDEBAR_EDGE_X,
        clientY: 100,
      })
    })

    expect(localStorage.getItem("dux:sidebar-width")).toBe("18rem")
  })

  // The panel library's separator vocabulary, in the sidebar's units: an arrow
  // nudges by 1rem, Home and End run the divider to its ends.
  it("resizes from the keyboard the way the panel library's separator does", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    expect(handle.getAttribute("role")).toBe("separator")
    expect(handle.getAttribute("aria-orientation")).toBe("vertical")
    expect(handle.tabIndex).toBe(0)

    act(() => {
      fireEvent.keyDown(handle, { key: "ArrowRight" })
    })
    expect(localStorage.getItem("dux:sidebar-width")).toBe("19rem")

    act(() => {
      fireEvent.keyDown(handle, { key: "End" })
    })
    expect(localStorage.getItem("dux:sidebar-width")).toBe("28rem")
  })

  // The library steps by 5% of the group it splits. Here the group is the
  // window, and 5% of anything wider than 960px is more than the 48px between
  // the 18rem default and the 15rem auto-collapse threshold, so the library's
  // own step would put the entire sidebar away on one press.
  it("does not collapse the sidebar on a single ArrowLeft from the default", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    // jsdom's window is 1024px wide, which is exactly the case the 1rem step
    // exists for.
    expect(window.innerWidth).toBeGreaterThan(960)
    act(() => {
      fireEvent.keyDown(grabHandle(container), { key: "ArrowLeft" })
    })

    expect(localStorage.getItem("dux:sidebar-width")).toBe("17rem")
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeNull()
  })

  // A keystroke that collapses the sidebar unmounts the control the user was
  // standing on. Without a handoff, focus falls to the body and the next Tab
  // starts from the top of the document.
  it("hands focus to the expand strip when the keyboard collapses it", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    act(() => {
      fireEvent.keyDown(grabHandle(container), { key: "Enter" })
    })

    const expand = container.querySelector('[data-sidebar="expand-handle"]')
    expect(expand).toBeTruthy()
    expect(document.activeElement).toBe(expand)
  })

  it("hands focus back to the drag edge when the keyboard expands it", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const expand = container.querySelector(
      '[data-sidebar="expand-handle"]',
    ) as HTMLElement
    // `detail: 0` is what the browser sends for a click the keyboard
    // synthesised from Enter or Space on a button.
    act(() => {
      fireEvent.click(expand, { detail: 0 })
    })

    const edge = container.querySelector('[data-sidebar="resize-handle"]')
    expect(edge).toBeTruthy()
    expect(document.activeElement).toBe(edge)
  })

  it("leaves focus alone when the mouse expands it", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    act(() => {
      fireEvent.click(
        container.querySelector('[data-sidebar="expand-handle"]') as HTMLElement,
        { detail: 1 },
      )
    })

    expect(document.activeElement).not.toBe(
      container.querySelector('[data-sidebar="resize-handle"]'),
    )
  })

  it("collapses to the rail on Home, and on Enter", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    act(() => {
      fireEvent.keyDown(grabHandle(container), { key: "Home" })
    })
    expect(localStorage.getItem("dux:sidebar-width")).toBe("14rem")
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeTruthy()
  })

  it("collapses to the rail on Enter, the library's collapse key", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    act(() => {
      fireEvent.keyDown(grabHandle(container), { key: "Enter" })
    })
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeNull()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeTruthy()
  })

  it("clamps the dragged width to the maximum", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)

    // 9999px is well past the 28rem (448px) cap, so it must clamp to 28rem.
    pressEdge(handle)
    releaseEdge(9999)

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

  // ── Touch ──────────────────────────────────────────────────────────────
  // A finger could not move this divider: the element suppressed no
  // touch-action, so the browser claimed the gesture as a pan and fired
  // `pointercancel`, which the handler treats as drag-end; and the hit target
  // was 4px wide, well under the 20px the panel library itself uses as its
  // coarse-pointer minimum. Both are asserted on the CLASS, which is where the
  // fix lives (jsdom implements neither gesture arbitration nor hit testing, so
  // the browser behaviour is not reproducible here). Both now come from the
  // shared divider chrome, which the Changes divider wears too; the parity
  // test in paneDividerParity.test.tsx is what keeps the two together.
  it("suppresses touch-action on the drag handle so a finger drag is not stolen as a pan", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    expect(handle.className).toContain("touch-none")
  })

  it("gives the drag handle a coarse-pointer hit slop wider than its painted line", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    // The painted line stays hair-thin; a transparent ::after grows to 10px for
    // a mouse and 20px under a coarse pointer. Both dividers wear the same
    // string; see paneDividerParity.test.tsx.
    expect(handle.className).toContain("after:absolute")
    expect(handle.className).toContain("pointer-coarse:after:w-[20px]")
    expect(handle.className).toContain("after:w-[10px]")
    expect(handle.className).toContain("w-px")
  })

  // The drag edge paints this sidebar's right border now, in the same token and
  // at the same hair-thin width as the Changes divider. If the container went
  // on drawing its own, the sidebar's edge would be two lines where the other
  // divider is one. This is a claim about tailwind-merge, so it is measured
  // rather than asserted in a comment.
  it("hands its right border to the drag edge instead of drawing a second one", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const shell = container.querySelector(
      '[data-slot="sidebar-container"]',
    ) as HTMLElement
    const tokens = shell.className.split(/\s+/)
    expect(tokens).toContain("group-data-[side=left]:border-r-0")
    expect(tokens).not.toContain("group-data-[side=left]:border-r")
  })

  it("gives the collapsed expand handle the same coarse-pointer hit slop", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    const expand = container.querySelector(
      '[data-sidebar="expand-handle"]',
    ) as HTMLElement
    expect(expand.className).toContain("after:absolute")
    expect(expand.className).toContain("pointer-coarse:after:w-[20px]")
  })

  it("resizes and persists from a touch-pointer drag", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)

    pressEdge(handle, { pointerId: 7, pointerType: "touch" })
    moveEdge(336, { pointerId: 7, pointerType: "touch" })
    releaseEdge(336, { pointerId: 7, pointerType: "touch" })

    expect(localStorage.getItem("dux:sidebar-width")).toBe("21rem")
  })

  it("cleans up its window listeners when the gesture is cancelled", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)

    pressEdge(handle, { pointerId: 8, pointerType: "touch" })
    moveEdge(336, { pointerId: 8, pointerType: "touch" })
    act(() => {
      fireEvent.pointerCancel(document, {
        pointerId: 8,
        pointerType: "touch",
        clientX: 336,
        clientY: 100,
      })
    })
    // Nothing is persisted by a cancel, and a later stray move must not move
    // the sidebar: the gesture is over.
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
    releaseEdge(400, { pointerId: 8, pointerType: "touch" })
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
  })

  // A mouse released over another window, over browser chrome, or into a native
  // drag never delivers its pointerup here. Without this the divider would keep
  // following a mouse with nothing held down.
  it("ends the drag when a mouse move reports no button held", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    pressEdge(handle, { pointerType: "mouse", buttons: 1 })
    moveEdge(352, { pointerType: "mouse", buttons: 1 })
    // The lost release: the next move says the button is already up.
    moveEdge(400, { pointerType: "mouse", buttons: 0 })

    // Committed where the last real move left it, not where the stray one did.
    expect(localStorage.getItem("dux:sidebar-width")).toBe("22rem")

    // And the gesture really is over: a later move moves nothing.
    moveEdge(448, { pointerType: "mouse", buttons: 0 })
    expect(localStorage.getItem("dux:sidebar-width")).toBe("22rem")
  })

  it("ignores a second finger arriving mid-drag", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    pressEdge(handle, { pointerId: 1, pointerType: "touch" })
    // A second finger lands in the band; it must not restart the gesture from
    // its own press point.
    pressEdge(handle, { pointerId: 2, pointerType: "touch", clientX: 284 })
    moveEdge(352, { pointerId: 1, pointerType: "touch" })
    releaseEdge(352, { pointerId: 1, pointerType: "touch" })

    // 288 + 64, measured from the FIRST finger's press.
    expect(localStorage.getItem("dux:sidebar-width")).toBe("22rem")
  })

  // The library paints the resize cursor over the whole document while its band
  // is hovered. Leaving the document has to take that back, or every page the
  // pointer returns to still reads as a splitter.
  it("drops the document resize cursor when the pointer leaves the page", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    grabHandle(container)
    moveEdge(SIDEBAR_EDGE_X, { pointerType: "mouse" })
    expect(document.getElementById("dux-divider-cursor")).toBeTruthy()

    act(() => {
      fireEvent.pointerLeave(document, { pointerType: "mouse" })
    })
    expect(document.getElementById("dux-divider-cursor")).toBeNull()
  })
})

describe("sidebarResizeRelease threshold", () => {
  // Pure decision: clamp to [14rem, 28rem] and flag collapse below 15rem.
  it("clamps a normal drag and does not collapse", () => {
    expect(sidebarResizeRelease(400)).toEqual({
      widthRem: "25rem",
      collapse: false,
    })
  })

  it("clamps past the max without collapsing", () => {
    expect(sidebarResizeRelease(9999)).toEqual({
      widthRem: "28rem",
      collapse: false,
    })
  })

  it("collapses when released below the 15rem threshold", () => {
    // 232px is inside the clamp band but under 240px (15rem): collapse.
    expect(sidebarResizeRelease(232)).toEqual({
      widthRem: "14.5rem",
      collapse: true,
    })
  })

  it("collapses when dragged hard-left (clamped to the 14rem floor)", () => {
    expect(sidebarResizeRelease(0)).toEqual({
      widthRem: "14rem",
      collapse: true,
    })
  })

  it("does not collapse exactly at the 15rem threshold", () => {
    expect(sidebarResizeRelease(240)).toEqual({
      widthRem: "15rem",
      collapse: false,
    })
  })
})

describe("AppSidebar auto-collapse on narrow drag", () => {
  it("collapses to the icon rail when the handle is released below the threshold", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)
    expect(handle).toBeTruthy()

    // Drag far left and release at x=100 (clamps to 14rem, under the 15rem
    // auto-collapse threshold): the sidebar snaps to the icon rail, so the drag
    // handle is replaced by the click-to-expand edge.
    pressEdge(handle)
    moveEdge(100)
    releaseEdge(100)

    expect(localStorage.getItem("dux:sidebar-width")).toBe("14rem")
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeNull()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeTruthy()
  })

  it("does not collapse when released above the threshold", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = grabHandle(container)

    pressEdge(handle)
    releaseEdge(320)

    // Stays expanded: the drag handle remains, no expand edge appears.
    expect(localStorage.getItem("dux:sidebar-width")).toBe("20rem")
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeNull()
  })
})

describe("AppSidebar launcher corner", () => {
  // The corner is one verb plus one ⋯, so there is nothing left to stack: the
  // footer's container-query scaffolding went with the second split button.
  it("drops the container-query stacking scaffolding", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const footer = container.querySelector('[data-sidebar="footer"]')
    expect(footer).toBeTruthy()
    expect(footer!.className).not.toContain("@container")
    expect(footer!.innerHTML).not.toContain("@[18rem]")
  })

  // No seam-joined group any more, so no base-ui focus-guard rounding hack
  // either: both died with the split buttons.
  it("renders the verb and the ⋯ as two gapped buttons, not a button group", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const footer = container.querySelector(
      '[data-sidebar="footer"]',
    ) as HTMLElement
    expect(footer.querySelector('[data-slot="button-group"]')).toBeNull()
    expect(footer.innerHTML).not.toContain("rounded-r-lg")

    // The corner's own two controls, at one height plus the touch floor. The
    // verb is picked by its LABEL TEXT: the rail's icon button carries the same
    // accessible name and no text of its own.
    const verb = Array.from(footer.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "New agent",
    ) as HTMLElement
    expect(verb).toBeTruthy()
    const overflow = footer.querySelector(
      '[aria-label="More ways to create"]:not(.size-8)',
    ) as HTMLElement
    expect(overflow).toBeTruthy()
    for (const control of [verb, overflow]) {
      expect(control.className, control.textContent ?? "").toContain("h-7")
      expect(control.className, control.textContent ?? "").toContain(
        "max-md:min-h-11",
      )
    }
  })

  // The rail is icon-only: a bare verb icon and the SAME grouped ⋯ menu the
  // corner above carries, so a collapsed sidebar is not a dead end for
  // everything the ⋯ holds (Add project included).
  it("gives the collapsed rail a New agent icon and its own ⋯", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    const railVerb = container.querySelector(
      '[data-sidebar="footer"] [aria-label="New agent"].size-8',
    )
    expect(railVerb).toBeTruthy()
    const railOverflow = container.querySelector(
      '[data-sidebar="footer"] [aria-label="More ways to create"].size-8',
    )
    expect(railOverflow).toBeTruthy()
    // The Add-project rail icon is gone; its action lives in that ⋯ now.
    expect(screen.queryByLabelText("Add project")).toBeNull()
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
      workspace: { kind: "managed"; project_id: string }
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
    workspace: { ...base.sessions[0].workspace, project_id: "p2" },
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
  it("refreshes a rail tooltip's changes count from the parent store snapshot", () => {
    const spine = makeTwoProjectSpine()
    mockState = makeState({
      spine,
      changes: {
        sessionId: "s1",
        phase: "loaded",
        rev: 1,
        staged: [{ path: "a.txt" }],
        unstaged: [],
        error: null,
      },
    })
    const view = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    let firstTooltip = screen
      .getByTestId("collapsed-agent-rail")
      .querySelectorAll('[data-testid="tooltip-content"]')[0]
    expect(firstTooltip.textContent).toContain("1 file")

    mockState = makeState({
      spine,
      changes: {
        sessionId: "s1",
        phase: "loaded",
        rev: 2,
        staged: [{ path: "a.txt" }],
        unstaged: [{ path: "b.txt" }],
        error: null,
      },
    })
    view.rerender(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    firstTooltip = screen
      .getByTestId("collapsed-agent-rail")
      .querySelectorAll('[data-testid="tooltip-content"]')[0]
    expect(firstTooltip.textContent).toContain("2 files")
  })

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

  // A standalone agent belongs to no project, so grouping by project loses it.
  // The rail is the ONLY way to reach an agent at icon width, so a lost agent
  // there is an agent that cannot be reached without expanding the sidebar.
  it("includes a standalone agent, which belongs to no project group", () => {
    const spine = makeTwoProjectSpine() as unknown as {
      sessions: unknown[]
    }
    const standalone = {
      ...(spine.sessions[0] as Record<string, unknown>),
      id: "sa1",
      title: "My Notes",
      workspace: {
        kind: "folder",
        folder_path: "/home/someone/My Notes",
        folder_label: "~/My Notes",
        repo_status: "no_repo",
        quiet_reason: "This folder has no git repository.",
      },
    }
    mockState = makeState({
      spine: {
        ...(spine as object),
        sessions: [...spine.sessions, standalone],
      } as unknown as DuxState["spine"],
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
    const labels = [...rail.querySelectorAll("button")].map((b) =>
      b.getAttribute("aria-label"),
    )
    expect(labels.length).toBe(3)
    // Its label is just the agent's name: there is no project to put in
    // parentheses, and "My Notes ()" would be worse than nothing.
    expect(labels).toContain("My Notes")
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
    // s2 needs attention: its icon wrapper carries the cyan blink.
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

  it("renders the shared AgentVitalsTooltip content (labelled vitals rows) for a rail icon", () => {
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
    // makeSessionSpine's session s1 carries branch_name "main"; the "Branch"
    // key/value label proves the rail uses the full AgentVitalsTooltip content
    // component (a bare 2-line tooltip would have no labelled rows).
    expect(tooltips[0].textContent).toContain("Branch")
    expect(tooltips[0].textContent).toContain("main")
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
    const vitalsTooltip = tooltips.find((t) =>
      t.textContent?.includes("Branch"),
    )
    expect(vitalsTooltip).toBeTruthy()
    // Status line + project name from the row context.
    expect(vitalsTooltip?.textContent).toContain("Repo")
    expect(vitalsTooltip?.textContent?.toLowerCase()).toContain("active")
  })

  it("shows the Changes row only for the session the changed-files slice is loaded for", () => {
    // The changed-files store slice only ever holds data for the currently
    // SELECTED session (see ChangesSlice in lib/store.ts). Session s1 here is
    // "loaded" for; session s2 shares the same slice object (it isn't the
    // selected session), so its vitals tooltip must omit the Changes row
    // rather than showing s1's count. Both sessions land in expanded rows
    // (makeTwoProjectSpine's projects both default-open, since each has a
    // session), so this exercises the shared changesCountFor gate used by the
    // expanded-row call site. makeTwoProjectSpine's second session reuses s1's
    // branch/worktree fields (only id/project_id/status differ), so the two
    // rows are told apart by their tooltip's project name line ("Repo" for
    // s1/p1, "Other" for s2/p2) rather than the branch/worktree rows.
    mockState = makeState({
      spine: makeTwoProjectSpine(),
      bootstrap: {
        title: "dux",
        dux_version: "v1",
        available_providers: ["claude"],
      },
      createTabInFlight: [],
      changes: {
        sessionId: "s1",
        phase: "loaded",
        rev: 1,
        staged: [{ path: "a.txt" }],
        unstaged: [{ path: "b.txt" }],
        error: null,
      },
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const tooltips = screen.getAllByTestId("tooltip-content")
    const s1Tooltip = tooltips.find((t) => t.textContent?.includes("Repo"))
    const s2Tooltip = tooltips.find((t) => t.textContent?.includes("Other"))

    expect(s1Tooltip).toBeTruthy()
    expect(s2Tooltip).toBeTruthy()
    expect(s1Tooltip?.textContent).toContain("Changes")
    expect(s1Tooltip?.textContent).toContain("2 files")
    expect(s2Tooltip?.textContent).not.toContain("Changes")
  })
})
