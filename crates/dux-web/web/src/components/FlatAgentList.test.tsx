// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DragEndEvent } from "@dnd-kit/core"
import type { DuxState } from "@/lib/store"
import type { SessionView, TerminalView } from "@/lib/types"

// Drag-from-any-mode contract. dnd-kit cannot run a real drag under jsdom (it
// measures rects), so DndContext is mocked to a passthrough that CAPTURES each
// `onDragEnd` (mount order: agents first, terminals second) and the tests call
// it with synthetic DragEndEvents. useSortable is a static stub; what these
// tests pin about drag AVAILABILITY is therefore the component's own
// `sortable ? dragProps : {}` gate (main rows carry the sortable attributes in
// every mode, quiet-tail rows never do), not dnd-kit's internals.
const dragEndHandlers: ((event: DragEndEvent) => void)[] = []
vi.mock("@dnd-kit/core", () => ({
  DndContext: ({
    children,
    onDragEnd,
  }: {
    children: React.ReactNode
    onDragEnd: (event: DragEndEvent) => void
  }) => {
    dragEndHandlers.push(onDragEnd)
    return children
  },
  MouseSensor: class {},
  TouchSensor: class {},
  closestCenter: () => {},
  useSensor: () => ({}),
  useSensors: (...sensors: unknown[]) => sensors,
}))
vi.mock("@dnd-kit/sortable", () => ({
  SortableContext: ({ children }: { children: React.ReactNode }) => children,
  verticalListSortingStrategy: {},
  useSortable: () => ({
    attributes: { role: "button", "aria-roledescription": "sortable" },
    listeners: {},
    setNodeRef: () => {},
    transform: null,
    transition: undefined,
    isDragging: false,
  }),
}))

const reorderAgentsMock = vi.fn()
const reorderTerminalsMock = vi.fn()
const setAgentSortMock = vi.fn()
const openNewAgentPickerMock = vi.fn()
const openAddProjectMock = vi.fn()
const createStandaloneTerminalMock = vi.fn()
const openEditorMock = vi.fn()
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    reorderAgents: (...args: unknown[]) => reorderAgentsMock(...args),
    reorderTerminals: (...args: unknown[]) => reorderTerminalsMock(...args),
    setAgentSort: (...args: unknown[]) => setAgentSortMock(...args),
    openNewAgentPicker: (...args: unknown[]) => openNewAgentPickerMock(...args),
    openAddProject: (...args: unknown[]) => openAddProjectMock(...args),
    createStandaloneTerminal: (...args: unknown[]) =>
      createStandaloneTerminalMock(...args),
    openEditor: (...args: unknown[]) => openEditorMock(...args),
  }
})

// The store touches localStorage at import time (pulled in transitively), so
// stub the browser globals BEFORE the module graph evaluates.
function installStubs() {
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
  // AgentActionsMenu (rendered by these rows) calls useIsMobile(), whose
  // subscription needs matchMedia; jsdom has none. Same inert stub
  // TerminalPane.test.tsx installs; matches:false = desktop.
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
installStubs()
const { FlatAgentList } = await import("./FlatAgentList")
const { resetQuietTailManualChoiceForTests } = await import("@/lib/quietTailChoice")

function makeSession(
  over: Omit<Partial<SessionView>, "workspace"> & {
    id: string
    project_id?: string
    branch_name?: string
  },
): SessionView {
  return {
    workspace: {
      kind: "managed",
      project_id: over.project_id ?? "p1",
      branch_name: over.branch_name ?? over.id,
      initial_branch: over.branch_name ?? over.id,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${over.id}`,
    },
    title: over.id,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    slot_tab_id: over.id,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function makeTerminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    owner: { kind: "session", session_id: "zeta" },
    label: over.id,
    has_output: false,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: 0,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as TerminalView
}

// Base (persisted) order deliberately differs from name order, and an exited
// session sits in the MIDDLE of the base so the totalized capture is visible:
// name-displayed = [alpha, mike, zeta] main + [gone] tail.
function makeState(sort: string): DuxState {
  return {
    spine: {
      projects: [{ id: "p1", name: "Repo" }],
      // Both terminals are owned by the Zeta agent, in one flat, owner-tagged
      // collection ordered by `sort_order`.
      terminals: [
        makeTerminal({
          id: "t-z",
          label: "zsh",
          sort_order: 1,
          // A running foreground command so the row DISPLAYS "zsh" (an
          // idle terminal reads a plain "Terminal"); the highlight test
          // needs the displayed string to be the matched one.
          foreground_cmd: "zsh",
        }),
        makeTerminal({ id: "t-a", label: "bash", sort_order: 2 }),
      ],
      sessions: [
        makeSession({ id: "zeta", title: "Zeta" }),
        makeSession({ id: "gone", title: "Gone", status: "exited" }),
        // A branch that diverges from the title so line two renders it (the
        // branch-hit highlight test needs a visible branch).
        makeSession({ id: "alpha", title: "Alpha", branch_name: "feat/silver" }),
        makeSession({ id: "mike", title: "Mike" }),
      ],
      // A real sidebar group: `partitionProjects` resolves project display
      // names from here, and the project-hit highlight/filter tests need
      // `projectName("p1")` to be "Repo", not a fallback.
      sidebar: {
        groups: [{ project_id: "p1", name: "Repo", orphaned: false }],
        agentless_start: null,
      },
    },
    bootstrap: null,
    selectedTarget: null,
    agentSearch: "",
    agentSort: sort,
    pendingAgentOrder: null,
    pendingTerminalOrder: null,
    changes: null,
    createTabInFlight: [],
  } as unknown as DuxState
}

const handlers = {
  onSelectSession: vi.fn(),
  onSelectTerminal: vi.fn(),
}

beforeEach(() => {
  dragEndHandlers.length = 0
  reorderAgentsMock.mockClear()
  reorderTerminalsMock.mockClear()
  setAgentSortMock.mockClear()
  openNewAgentPickerMock.mockClear()
  openAddProjectMock.mockClear()
  createStandaloneTerminalMock.mockClear()
  mockState = makeState("name")
  resetQuietTailManualChoiceForTests()
  installStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Handler capture order: the agents DndContext mounts first, the Terminals
// section's second.
function agentsDragEnd(): (event: DragEndEvent) => void {
  const h = dragEndHandlers[0]
  if (!h) throw new Error("agents DndContext not mounted")
  return h
}
function terminalsDragEnd(): (event: DragEndEvent) => void {
  const h = dragEndHandlers[1]
  if (!h) throw new Error("terminals DndContext not mounted")
  return h
}

function drop(handler: (event: DragEndEvent) => void, active: string, over: string) {
  handler({ active: { id: active }, over: { id: over } } as DragEndEvent)
}

describe("FlatAgentList section order", () => {
  it("renders Terminals ABOVE the Inactive quiet tail", () => {
    // Wanted order: main agents, then Terminals, then Inactive. Both section
    // headers exist in this fixture (a quiet session and two terminals), so
    // the DOM order of the two toggles pins the layout.
    render(<FlatAgentList handlers={handlers} />)
    const terminals = screen.getByText("Terminals")
    const inactive = screen.getByText("Inactive")
    const position = terminals.compareDocumentPosition(inactive)
    // DOCUMENT_POSITION_FOLLOWING (4): `inactive` comes after `terminals`.
    expect(position & Node.DOCUMENT_POSITION_FOLLOWING).toBe(4)
  })

  it("keeps the collapse defaults: Terminals open, Inactive closed while an agent is active", () => {
    render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Terminals").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("true")
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
  })
})

// The Inactive tail is auto-managed until the user toggles it by hand,
// mirroring the TUI's rule: a wholly-dormant workspace (the landing screen
// after a restart, which brings agents back dormant) renders the tail OPEN so
// the agents are not hidden behind a collapsed toggle; any active agent
// collapses it; the first manual toggle takes over from the automation.
describe("FlatAgentList inactive tail auto-open", () => {
  function dormantOnlyState() {
    const state = makeState("name")
    state.spine!.sessions = [
      makeSession({ id: "gone", title: "Gone", status: "exited" }),
      makeSession({ id: "away", title: "Away", status: "detached" }),
    ]
    return state
  }

  it("renders the tail OPEN when no agent is active", () => {
    mockState = dormantOnlyState()
    render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("true")
    // The rows themselves are on screen, not just the header.
    expect(screen.getByText("Gone")).toBeTruthy()
    expect(screen.getByText("Away")).toBeTruthy()
  })

  it("a manual collapse wins over the automation and sticks", () => {
    mockState = dormantOnlyState()
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByText("Inactive"))
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
    expect(screen.queryByText("Gone")).toBeNull()
  })

  it("a manual choice survives an activity flip AND a remount", () => {
    mockState = dormantOnlyState()
    const first = render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByText("Inactive"))
    // An agent goes active, then everything goes dormant again: the explicit
    // collapse holds through both flips.
    mockState = makeState("name")
    first.rerender(<FlatAgentList handlers={handlers} />)
    mockState = dormantOnlyState()
    first.rerender(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
    // Navigation unmounts the sidebar list (mobile hub round trip, the
    // nothing-matches search branch); the page-load-scoped choice survives it.
    first.unmount()
    mockState = dormantOnlyState()
    render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
  })

  it("the workspace going dormant while mounted pops an untouched tail open", () => {
    const view = render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
    mockState = dormantOnlyState()
    view.rerender(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("true")
  })

  it("an agent becoming active collapses an untouched tail", () => {
    mockState = dormantOnlyState()
    const view = render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("true")
    // The dormant workspace wakes up: one agent goes active. The untouched
    // tail follows the automation and collapses.
    mockState = dormantOnlyState()
    mockState.spine!.sessions = [
      makeSession({ id: "zeta", title: "Zeta" }),
      ...mockState.spine!.sessions,
    ]
    view.rerender(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
  })
})

// Auto-expanding the Inactive tail on a search hit is DERIVED state: while the
// query matches something quiet the group renders open; clearing the query
// restores the collapsed default; and a manual collapse during a matching query
// wins until the query changes.
describe("FlatAgentList quiet-tail search auto-expand", () => {
  const withQuery = (query: string): DuxState => {
    const state = makeState("name")
    ;(state as unknown as { agentSearch: string }).agentSearch = query
    return state
  }

  it("expands and shows the quiet row when the query hits a quiet agent", () => {
    mockState = withQuery("gone")
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Gone")).toBeTruthy()
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("true")
  })

  it("re-collapses when the query is cleared", () => {
    mockState = withQuery("gone")
    const { rerender } = render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Gone")).toBeTruthy()
    mockState = withQuery("")
    rerender(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByText("Gone")).toBeNull()
    expect(
      screen.getByText("Inactive").closest("button")?.getAttribute("aria-expanded"),
    ).toBe("false")
  })

  it("does not reveal quiet rows for a query matching only main agents", () => {
    mockState = withQuery("alpha")
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByText("Gone")).toBeNull()
  })

  it("a manual collapse during a matching query wins until the query changes", () => {
    mockState = withQuery("gone")
    const { rerender } = render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Gone")).toBeTruthy()
    // Explicit collapse while the search holds it open: the user wins.
    fireEvent.click(screen.getByText("Inactive"))
    expect(screen.queryByText("Gone")).toBeNull()
    // Same query keeps the dismissal.
    rerender(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByText("Gone")).toBeNull()
    // The query changes (cleared, then re-typed): the dismissal expires and
    // the matching query auto-expands again.
    mockState = withQuery("")
    rerender(<FlatAgentList handlers={handlers} />)
    mockState = withQuery("gone")
    rerender(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Gone")).toBeTruthy()
  })
})

// The agent row never prints the agent's branch, not even when the branch has
// drifted away from the agent's name. The branch's one home is the top bar's
// branch chip (see InsetHeader): a long branch inline made the row noisy on a
// tablet, and the row already says everything it needs to about the agent.
describe("FlatAgentList agent row branch", () => {
  it("does not render the branch on a row whose branch diverges from its name", () => {
    // "Alpha" sits on "feat/silver": the classic drifted row.
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Alpha")).toBeTruthy()
    expect(screen.queryByText("feat/silver")).toBeNull()
    expect(document.body.textContent).not.toContain("feat/silver")
  })

  // The row's second line names the FOLDER where an ordinary agent names its
  // project: it is the same fact ("which thing am I in") for the other kind of
  // agent, so it takes the same slot rather than a badge of its own.
  it("names a standalone agent's folder where a managed agent names its project", () => {
    const state = makeState("name")
    state.spine!.sessions = [
      {
        ...state.spine!.sessions[0],
        id: "notes",
        title: "Notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/work/notes",
          folder_label: "~/work/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      } as SessionView,
    ]
    mockState = state
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("notes")).toBeTruthy()
    expect(screen.queryByText("~/work/notes")).toBeNull()
    // And no project name, because it belongs to none.
    expect(screen.queryByText("Repo")).toBeNull()
  })

  // The standalone identity glyph is the literal ✷ star, drawn as text the way
  // the terminal rows draw the ↳ arrow, and the glyph plus the folder label
  // wear the dux-standalone identity token (the web twin of the TUI's
  // standalone-location theme color), never a hardcoded color. The star is
  // aria-hidden with an sr-only word beside it, so screen readers speak the
  // MEANING rather than a Unicode name. A managed agent's project tag keeps
  // its plain folder glyph and its muted tone.
  it("marks a standalone agent's folder with the standalone star in the identity tone", () => {
    const state = makeState("name")
    state.spine!.sessions = [
      state.spine!.sessions[0],
      {
        ...state.spine!.sessions[0],
        id: "notes",
        title: "Notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/work/notes",
          folder_label: "~/work/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      } as SessionView,
    ]
    mockState = state
    render(<FlatAgentList handlers={handlers} />)
    const tag = screen.getByText("notes").closest("span")?.parentElement
    expect(tag).toBeTruthy()
    // The literal star as a text glyph, no icon component behind it.
    const star = tag!.querySelector("[aria-hidden]")
    expect(star?.textContent).toBe("✷")
    expect(tag!.querySelector("svg")).toBeNull()
    // Screen readers get the meaning, not the codepoint.
    expect(tag!.querySelector(".sr-only")?.textContent).toBe("standalone")
    // The identity tone is the dedicated dux-standalone token, scoped to the
    // glyph and the label; the rest of line two stays muted.
    expect(tag!.className).toContain("text-dux-standalone")
    expect(tag!.className).not.toContain("text-muted-foreground")
    // The managed row beside it is untouched: plain folder glyph, no star.
    const project = screen.getByText("Repo").closest("span")?.parentElement
    expect(project!.querySelector("svg.lucide-folder")).toBeTruthy()
    expect(project!.textContent).not.toContain("✷")
    expect(project!.className).toContain("text-muted-foreground")
  })

  // A STANDALONE terminal's owner tag is the same star in the same tone: one
  // indicator, learned once, meaning "this one lives in your folder, not in a
  // dux-managed working copy". An OWNED terminal keeps the ↳ arrow, where it
  // means "owned by".
  it("marks a standalone terminal with the star and keeps the arrow on owned terminals", () => {
    const base = makeState("name")
    mockState = {
      ...base,
      spine: {
        ...base.spine,
        terminals: [
          makeTerminal({
            id: "t-owned",
            label: "owned",
            owner: { kind: "session", session_id: "zeta" },
          }),
          makeTerminal({
            id: "t-solo",
            label: "solo",
            owner: { kind: "standalone", cwd_label: "~/play" },
          }),
        ],
      },
    } as DuxState
    render(<FlatAgentList handlers={handlers} />)
    // The standalone terminal's tag: star, sr-only meaning, identity tone.
    const solo = screen.getByText("play").closest("span")?.parentElement
    expect(solo).toBeTruthy()
    expect(solo!.querySelector("[aria-hidden]")?.textContent).toBe("✷")
    expect(solo!.querySelector(".sr-only")?.textContent).toBe("standalone")
    expect(solo!.className).toContain("text-dux-standalone")
    // The owned terminal keeps the muted arrow and no star.
    const owned = screen
      .getByText("Zeta@Repo")
      .closest("span")?.parentElement
    expect(owned).toBeTruthy()
    expect(owned!.textContent).toContain("↳")
    expect(owned!.textContent).not.toContain("✷")
    expect(owned!.className).not.toContain("text-dux-standalone")
  })

  it("keeps the rest of line two: the project tag, the state word, the tab count", () => {
    const state = makeState("name")
    state.spine!.sessions = state.spine!.sessions.map((s) =>
      s.id === "alpha"
        ? ({ ...s, tabs: [{ id: "alpha" }, { id: "alpha-2" }] } as SessionView)
        : s,
    )
    mockState = state
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getAllByText("Repo").length).toBeGreaterThan(0)
    expect(screen.getAllByText("Idle").length).toBeGreaterThan(0)
    expect(screen.getByText("2 tabs")).toBeTruthy()
  })
})

// The search-match highlight: the matched part of a row's NAME (the field the
// filter searched and the row displays) wraps in a token-styled emphasis span.
describe("FlatAgentList search-match highlight", () => {
  const withQuery = (query: string): DuxState => {
    const state = makeState("name")
    ;(state as unknown as { agentSearch: string }).agentSearch = query
    return state
  }

  it("wraps the matched part of an agent name in the emphasis span", () => {
    mockState = withQuery("alph")
    render(<FlatAgentList handlers={handlers} />)
    const mark = screen.getByText("Alph")
    // Token-derived emphasis only, never a hardcoded color.
    expect(mark.className).toContain("bg-primary")
    // The full label survives around the mark.
    expect(mark.parentElement?.textContent).toBe("Alpha")
  })

  it("renders plain labels when no query is active", () => {
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Alpha")).toBeTruthy()
    expect(screen.queryByText("Alph")).toBeNull()
  })

  it("highlights a project-name hit on the agent row's second line", () => {
    // "rep" hits the project name "Repo" (and nothing on line one), so the
    // result must explain itself by emphasizing the project text.
    mockState = withQuery("rep")
    render(<FlatAgentList handlers={handlers} />)
    const marks = screen.getAllByText("Rep")
    expect(marks.length).toBeGreaterThan(0)
    for (const mark of marks) {
      expect(mark.className).toContain("bg-primary")
    }
    // At least one mark is the project tag itself.
    expect(marks.some((m) => m.parentElement?.textContent === "Repo")).toBe(true)
  })

  it("highlights nothing for a branch-only hit, because the row does not show the branch", () => {
    // The branch stays SEARCHABLE (agentSearch matches on branch_name) but the
    // row never prints it, so a query that hits only the branch filters the row
    // in and highlights nothing. Accepted: the branch's home is the top bar.
    mockState = withQuery("silver")
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("Alpha")).toBeTruthy()
    expect(screen.queryByText("silver")).toBeNull()
  })

  it("highlights an owner-label hit on the terminal row's second line", () => {
    // "zeta@" only occurs in the companion terminal's owner label
    // ("Zeta@Repo"), a searched field the row renders on line two.
    mockState = withQuery("zeta@")
    render(<FlatAgentList handlers={handlers} />)
    // Both of Zeta's terminals carry the owner label, so match all marks.
    const marks = screen.getAllByText("Zeta@")
    expect(marks.length).toBeGreaterThan(0)
    for (const mark of marks) {
      expect(mark.className).toContain("bg-primary")
      expect(mark.parentElement?.textContent).toBe("Zeta@Repo")
    }
  })

  it("highlights the matched terminal label too", () => {
    // t-z runs "zsh" as its foreground command, so the row displays (and the
    // filter matched) that string.
    mockState = withQuery("zs")
    render(<FlatAgentList handlers={handlers} />)
    const mark = screen.getByText("zs")
    expect(mark.className).toContain("bg-primary")
    expect(mark.parentElement?.textContent).toBe("zsh")
  })
})

describe("FlatAgentList drag from any sort mode", () => {
  it("main rows carry drag props in a computed (name) mode; quiet rows never do", () => {
    render(<FlatAgentList handlers={handlers} />)
    // Main rows: the select button is the sortable handle.
    const alphaRow = screen.getByText("Alpha").closest("button")
    expect(alphaRow?.getAttribute("aria-roledescription")).toBe("sortable")
    // The quiet tail (collapsed by default) opens on its toggle; its rows are
    // deliberately NOT sortable in any mode.
    fireEvent.click(screen.getByText("Inactive"))
    const goneRow = screen.getByText("Gone").closest("button")
    expect(goneRow).toBeTruthy()
    expect(goneRow?.getAttribute("aria-roledescription")).toBeNull()
  })

  it("a drop from name mode persists the DISPLAYED order with the move, totally, and flips to manual", () => {
    render(<FlatAgentList handlers={handlers} />)
    // Displayed (name) = [alpha, mike, zeta] with [gone] appended as the tail.
    // Dragging zeta onto alpha's slot within that view:
    drop(agentsDragEnd(), "zeta", "alpha")
    expect(setAgentSortMock).toHaveBeenCalledWith("manual")
    expect(reorderAgentsMock).toHaveBeenCalledWith([
      "zeta",
      "alpha",
      "mike",
      "gone",
    ])
  })

  it("a drop in manual mode keeps the base-order move semantics and does not re-flip", () => {
    mockState = makeState("manual")
    render(<FlatAgentList handlers={handlers} />)
    // Base = [zeta, gone, alpha, mike]; manual moves within it verbatim.
    drop(agentsDragEnd(), "zeta", "alpha")
    expect(setAgentSortMock).not.toHaveBeenCalled()
    expect(reorderAgentsMock).toHaveBeenCalledWith([
      "gone",
      "alpha",
      "zeta",
      "mike",
    ])
  })

  it("a no-op drop (same slot) neither reorders nor flips to manual", () => {
    render(<FlatAgentList handlers={handlers} />)
    drop(agentsDragEnd(), "alpha", "alpha")
    expect(setAgentSortMock).not.toHaveBeenCalled()
    expect(reorderAgentsMock).not.toHaveBeenCalled()
  })

  it("a terminal drop from name mode persists the displayed terminal order and flips to manual", () => {
    render(<FlatAgentList handlers={handlers} />)
    // Base by sort_order = [t-z (zsh), t-a (bash)]; name-displayed =
    // [t-a (bash), t-z (zsh)]. Dragging t-z onto t-a's displayed slot keeps
    // the on-screen arrangement the user produced: [t-z, t-a].
    drop(terminalsDragEnd(), "t-z", "t-a")
    expect(setAgentSortMock).toHaveBeenCalledWith("manual")
    expect(reorderTerminalsMock).toHaveBeenCalledWith(["t-z", "t-a"])
  })
})

// The PR chip is an anchor whose click handler ALSO calls window.open (the
// anchor is nested inside the row's button, where relying on the native default
// alone is not dependable). Exactly one of the two may run, or the click opens
// two tabs.
describe("FlatAgentList PR chip", () => {
  it("opens the PR once, suppressing the anchor's own navigation", () => {
    const open = vi.fn()
    vi.stubGlobal("open", open)
    mockState = makeState("name")
    mockState.spine!.sessions[0].pr = {
      number: 42,
      url: "https://github.com/o/r/pull/42",
      title: "Some PR",
      state: "open",
    } as SessionView["pr"]
    render(<FlatAgentList handlers={handlers} />)
    const chip = screen.getByLabelText("PR #42 (open)")
    const click = new MouseEvent("click", { bubbles: true, cancelable: true })
    fireEvent(chip, click)
    expect(open).toHaveBeenCalledTimes(1)
    // Unprevented, the browser follows the `target="_blank"` href as well and
    // the user gets a second tab.
    expect(click.defaultPrevented).toBe(true)
  })

  it("does not select the agent when the PR chip is clicked", () => {
    vi.stubGlobal("open", vi.fn())
    mockState = makeState("name")
    mockState.spine!.sessions[0].pr = {
      number: 42,
      url: "https://github.com/o/r/pull/42",
      title: "Some PR",
      state: "open",
    } as SessionView["pr"]
    render(<FlatAgentList handlers={handlers} />)
    handlers.onSelectSession.mockClear()
    fireEvent.click(screen.getByLabelText("PR #42 (open)"))
    expect(handlers.onSelectSession).not.toHaveBeenCalled()
  })
})

// (b) the agent menu's editor entries: the in-app item is renamed to
// distinguish surfaces and hidden on phones (where the overlay cannot open,
// so it was a dead no-op), and a new-tab item opens the standalone editor
// address. Every item keeps a leading icon per the menu tenet.
describe("FlatAgentList editor menu entries", () => {
  async function openFirstAgentMenu() {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Session actions")[0])
    await screen.findByRole("menu")
  }

  it("renames the in-app item and hides it on phones via CSS", async () => {
    await openFirstAgentMenu()
    const here = screen
      .getByText("Open editor here")
      .closest('[role="menuitem"]')
    expect(here).not.toBeNull()
    expect(here!.className).toContain("max-md:hidden")
  })

  it("offers Open editor in new tab as a REAL anchor to the standalone address", async () => {
    // An anchor, not a window.open handler, so middle-click and
    // ctrl/cmd-click keep their native new-tab semantics, matching the
    // editor header's affordance.
    const open = vi.fn()
    vi.stubGlobal("open", open)
    await openFirstAgentMenu()
    const item = screen
      .getByText("Open editor in new tab")
      .closest('[role="menuitem"]')
    expect(item).not.toBeNull()
    // Always available, phones included: it is the ONLY editor entry there.
    expect(item!.className).not.toContain("max-md:hidden")
    expect(item!.tagName).toBe("A")
    // The first displayed agent is Alpha (name sort in makeState("name")).
    expect(item!.getAttribute("href")).toBe("#/editor/agent/alpha")
    expect(item!.getAttribute("target")).toBe("_blank")
    expect(item!.getAttribute("rel")).toBe("noopener")
    fireEvent.click(item!)
    expect(open).not.toHaveBeenCalled()
  })
})

// (c) the TERMINAL row menu's editor entries. A terminal gets an editor too,
// rooted at the directory it was spawned in, and the two items follow the
// agent-row idiom exactly. Which addresses they point at is decided by the
// terminal's OWNER: one spawned in an agent's worktree is sent to that agent's
// editor, because it is the same files with the full git surface.
describe("FlatAgentList terminal editor menu entries", () => {
  async function openTerminalMenu(index = 0) {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Terminal actions")[index])
    await screen.findByRole("menu")
  }

  it("offers both items, with the in-app one hidden on phones", async () => {
    await openTerminalMenu()
    const here = screen
      .getByText("Open editor here")
      .closest('[role="menuitem"]')
    expect(here).not.toBeNull()
    expect(here!.className).toContain("max-md:hidden")
    const tab = screen
      .getByText("Open editor in new tab")
      .closest('[role="menuitem"]')
    expect(tab).not.toBeNull()
    expect(tab!.tagName).toBe("A")
    expect(tab!.getAttribute("target")).toBe("_blank")
    expect(tab!.getAttribute("rel")).toBe("noopener")
    expect(tab!.className).not.toContain("max-md:hidden")
  })

  it("sends a SESSION-owned terminal to its agent's editor, not to a terminal root", async () => {
    // Both fixture terminals belong to the Zeta agent, so both rows point at
    // the agent address: same worktree, same editor, no second one.
    await openTerminalMenu()
    const tab = screen
      .getByText("Open editor in new tab")
      .closest('[role="menuitem"]')
    expect(tab!.getAttribute("href")).toBe("#/editor/agent/zeta")
    fireEvent.click(screen.getByText("Open editor here"))
    expect(openEditorMock).toHaveBeenCalledWith({
      kind: "agent",
      sessionId: "zeta",
    })
  })

  it("gives a PROJECT terminal and a STANDALONE terminal their own rooted addresses", async () => {
    mockState = {
      ...makeState("name"),
      spine: {
        ...makeState("name").spine,
        terminals: [
          makeTerminal({
            id: "t-p",
            label: "proj",
            owner: { kind: "project", project_id: "p1" },
          }),
          makeTerminal({
            id: "t-s",
            label: "solo",
            owner: { kind: "standalone", cwd_label: "~" },
          }),
        ],
      },
    } as DuxState
    await openTerminalMenu(0)
    expect(
      screen
        .getByText("Open editor in new tab")
        .closest('[role="menuitem"]')!
        .getAttribute("href"),
    ).toBe("#/editor/project/p1/terminal/t-p")
    cleanup()
    await openTerminalMenu(1)
    expect(
      screen
        .getByText("Open editor in new tab")
        .closest('[role="menuitem"]')!
        .getAttribute("href"),
    ).toBe("#/editor/terminal/t-s")
  })
})

// "ATTACH A FILE…" IN THE ROW MENUS: the desktop and keyboard-only path into
// the upload journey, which before this had none (the only non-drag entry was
// pasting an image). It is borrowed from the pane rather than owned by the row,
// because the upload has to travel through the pane's own gated connection and
// land in its own sink.
describe("FlatAgentList attach-a-file entries", () => {
  const item = () => screen.queryByText("Attach a file…")

  async function openAgentMenu() {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Session actions")[0])
    await screen.findByRole("menu")
  }

  async function openTerminalMenu() {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Terminal actions")[0])
    await screen.findByRole("menu")
  }

  afterEach(async () => {
    const mod = await import("@/lib/attachRegistry")
    mod.resetAttachCapabilities()
  })

  // HIDDEN, not disabled, when no pane is mounted: the row-menu convention is
  // that an inert item does not appear at all.
  it("is absent from the agent menu while no pane of that agent is mounted", async () => {
    await openAgentMenu()
    expect(item()).toBeNull()
  })

  it("appears in the agent menu once one of its panes publishes it", async () => {
    const attach = vi.fn()
    const mod = await import("@/lib/attachRegistry")
    // The first displayed agent is Alpha (name sort), and this fixture gives
    // every session a slot tab id equal to its own id, which is what a mounted
    // pane registers under. A real slot tab id is generated and is not the
    // session id.
    mod.registerAttachCapability("alpha", attach)
    await openAgentMenu()
    fireEvent.click(item()!)
    expect(attach).toHaveBeenCalledTimes(1)
  })

  it("does not borrow another agent's pane", async () => {
    const mod = await import("@/lib/attachRegistry")
    mod.registerAttachCapability("zeta", vi.fn())
    await openAgentMenu()
    expect(item()).toBeNull()
  })

  it("is absent from a terminal menu with no mounted pane, and present with one", async () => {
    await openTerminalMenu()
    expect(item()).toBeNull()
    cleanup()
    const attach = vi.fn()
    const mod = await import("@/lib/attachRegistry")
    // The first displayed terminal row in this fixture is "bash" (t-a).
    mod.registerAttachCapability("t-a", attach)
    await openTerminalMenu()
    fireEvent.click(item()!)
    expect(attach).toHaveBeenCalledTimes(1)
  })
})

// THE ROW MENU IS THE COMPUTER'S HOME FOR THE PANE'S INPUT GROUP. Typing
// directly in the terminal takes the whole bottom bar away, the input `⋯` with
// it, so the way back has to live in a menu that is always reachable. On this
// surface that is the pane's own row menu, and these pin that the desktop half
// is really there: the group's home was previously asserted only on the phone.
describe("FlatAgentList input group in the row menus", () => {
  const label = () => screen.queryByText("Input")
  const wayBack = () => screen.queryByText("Use virtual input")

  async function openAgentMenu() {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Session actions")[0])
    await screen.findByRole("menu")
  }

  async function openTerminalMenu() {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getAllByLabelText("Terminal actions")[0])
    await screen.findByRole("menu")
  }

  afterEach(async () => {
    const attach = await import("@/lib/attachRegistry")
    attach.resetAttachCapabilities()
    const group = await import("@/lib/paneInputGroup")
    group.resetPaneInputGroups()
  })

  // ONE COMPONENT, TWO ANCHORS. The row's `⋯` and the desktop pane header's
  // open the same body, so the Settings drill it ends with is the proof the
  // row is rendering the merged menu rather than a copy of the agent's actions
  // that would drift from it.
  it("opens the same merged pane menu the pane header opens", async () => {
    await openAgentMenu()
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent)
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("carries the group, labelled, once the agent's pane publishes one", async () => {
    const mod = await import("@/lib/paneInputGroup")
    // The first displayed agent is Alpha (name sort), and this fixture gives
    // every session a slot tab id equal to its own id, which is what a mounted
    // pane registers under. A real slot tab id is generated and is not the
    // session id.
    mod.registerPaneInputGroup("alpha", {
      surfaceSwitch: true,
      keysToggle: false,
    })
    await openAgentMenu()
    expect(label()).toBeTruthy()
    expect(wayBack()).toBeTruthy()
  })

  // ABSENT, NEVER DISABLED: the bottom `⋯` owns the other direction while the
  // virtual input is up, and a row offered in both menus at once is the drift
  // the single publisher exists to prevent.
  it("leaves the way back out while the pane says the bottom bar has it", async () => {
    const mod = await import("@/lib/paneInputGroup")
    mod.registerPaneInputGroup("alpha", {
      surfaceSwitch: false,
      keysToggle: false,
    })
    await openAgentMenu()
    expect(wayBack()).toBeNull()
    // Nothing of its own to say and no attach to borrow, so no label and no
    // separator either.
    expect(label()).toBeNull()
  })

  it("renders no group at all for an agent with no mounted pane", async () => {
    await openAgentMenu()
    expect(label()).toBeNull()
    expect(wayBack()).toBeNull()
  })

  it("does not borrow another agent's pane", async () => {
    const mod = await import("@/lib/paneInputGroup")
    mod.registerPaneInputGroup("zeta", {
      surfaceSwitch: true,
      keysToggle: false,
    })
    await openAgentMenu()
    expect(wayBack()).toBeNull()
  })

  // THE ID CONTRACT, from the menu's side: an agent's panes are its session-slot
  // id AND every tab id, and whichever one is mounted answers. A shell that
  // handed its pane a tab id while the menu scanned only the session id would
  // leave the way back unreachable for every extra tab.
  it("reads an EXTRA tab's pane, not only the slot tab's", async () => {
    mockState = {
      ...makeState("name"),
      spine: {
        ...makeState("name").spine,
        sessions: (makeState("name").spine!.sessions as SessionView[]).map((s) =>
          s.id === "alpha"
            ? ({
                ...s,
                tabs: [{ id: "alpha" }, { id: "alpha-2" }],
              } as SessionView)
            : s,
        ),
      },
    } as DuxState
    const mod = await import("@/lib/paneInputGroup")
    mod.registerPaneInputGroup("alpha-2", {
      surfaceSwitch: true,
      keysToggle: false,
    })
    await openAgentMenu()
    expect(wayBack()).toBeTruthy()
  })

  it("carries the group in a terminal's row menu too, under its one id", async () => {
    await openTerminalMenu()
    expect(label()).toBeNull()
    cleanup()
    const mod = await import("@/lib/paneInputGroup")
    // The first displayed terminal row in this fixture is "bash" (t-a).
    mod.registerPaneInputGroup("t-a", {
      surfaceSwitch: true,
      keysToggle: false,
    })
    await openTerminalMenu()
    expect(label()).toBeTruthy()
    expect(wayBack()).toBeTruthy()
  })
})

// The section's own controls: a + that acts and a sort trigger that only
// reveals a menu, sharing one height token at the header's right edge.
describe("FlatAgentList Agents header", () => {
  it("offers a + that opens the new-agent picker in one tap", () => {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByRole("button", { name: "New agent" }))
    expect(openNewAgentPickerMock).toHaveBeenCalledWith("new")
  })

  it("gives the + and the sort trigger one height and the phone floor", () => {
    render(<FlatAgentList handlers={handlers} />)
    const plus = screen.getByRole("button", { name: "New agent" })
    const sort = screen.getByRole("button", { name: "Sort agents" })
    for (const control of [plus, sort]) {
      expect(control.className).toContain("h-7")
      // The sort trigger's old 36px exemption is retired: the + is now its
      // horizontal neighbour, which was the stated basis for the relaxation.
      expect(control.className).toContain("max-md:min-h-10")
      expect(control.className).not.toContain("max-md:min-h-9")
    }
  })

  // The + rides sort's gate: the empty state below carries its own hero button,
  // and two buttons offering the same click on one screen is one too many.
  it("hides both controls when there is no agent to sort", () => {
    mockState = {
      ...makeState("name"),
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        terminals: [],
        sessions: [],
        sidebar: {
          groups: [{ project_id: "p1", name: "Repo", orphaned: false }],
          agentless_start: null,
        },
      },
    } as unknown as DuxState
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByRole("button", { name: "Sort agents" })).toBeNull()
    // The empty state's hero remains, and it is the only "New agent" button.
    expect(screen.getAllByRole("button", { name: /^new agent$/i })).toHaveLength(
      1,
    )
  })
})

describe("FlatAgentList sort control", () => {
  it("reads a static Sort and names the mode in its tooltip instead", () => {
    mockState = makeState("updated")
    render(<FlatAgentList handlers={handlers} />)
    const trigger = screen.getByRole("button", { name: "Sort agents" })
    // Never the mode name: the full label plus the + overflows a narrow
    // sidebar, and a control that changes width when used is its own annoyance.
    expect(trigger.textContent).toBe("Sort")
    expect(screen.queryByText("Recently updated")).toBeNull()
  })

  it("checkmarks the active mode in the menu", async () => {
    mockState = makeState("created")
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByRole("button", { name: "Sort agents" }))
    await screen.findByRole("menu")
    const active = screen
      .getAllByRole("menuitem")
      .find((i) => i.textContent === "Recently created")!
    expect(active.querySelector("svg.lucide-check")).toBeTruthy()
  })

  // The web never OFFERS name_desc (only the TUI cycles into it), so without
  // this row a TUI-set name_desc would be a menu of five unticked rows.
  it("appends the TUI-only Name (Z to A) row while it is the active mode", async () => {
    mockState = makeState("name_desc")
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByRole("button", { name: "Sort agents" }))
    await screen.findByRole("menu")
    const rows = screen.getAllByRole("menuitem")
    const zToA = rows.find((i) => i.textContent === "Name (Z to A)")!
    expect(zToA).toBeTruthy()
    expect(zToA.querySelector("svg.lucide-check")).toBeTruthy()
    // Exactly one checkmark in the menu, and it is that row.
    expect(
      rows.filter((i) => i.querySelector("svg.lucide-check")),
    ).toHaveLength(1)
  })

  it("leaves it out in every mode the web can reach", async () => {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(screen.getByRole("button", { name: "Sort agents" }))
    await screen.findByRole("menu")
    expect(
      screen.getAllByRole("menuitem").map((i) => i.textContent),
    ).toEqual([
      "Active first",
      "Recently updated",
      "Recently created",
      "Name (A to Z)",
      "Manual order",
    ])
  })
})

// One pill style for every section count, immediately after the section word.
// Right edges carry controls only.
describe("FlatAgentList section counters", () => {
  it("renders the Agents, Terminals and Inactive counts in the same pill", () => {
    render(<FlatAgentList handlers={handlers} />)
    for (const word of ["Agents", "Terminals", "Inactive"]) {
      const pill = screen.getByText(word).nextElementSibling as HTMLElement
      expect(pill, word).toBeTruthy()
      expect(pill.className, word).toContain("rounded-full")
      expect(pill.className, word).toContain("bg-muted")
      // Not pushed to the right edge any more.
      expect(pill.className, word).not.toContain("ml-auto")
    }
  })
})

describe("FlatAgentList Terminals divider", () => {
  it("creates a standalone terminal in one tap", () => {
    render(<FlatAgentList handlers={handlers} />)
    fireEvent.click(
      screen.getByRole("button", { name: "New standalone terminal" }),
    )
    expect(createStandaloneTerminalMock).toHaveBeenCalledOnce()
  })

  // Nested interactive elements are invalid HTML and route clicks by coin
  // toss: the + is a SIBLING of the collapse toggle, as on the agent rows.
  it("keeps the + outside the collapse toggle", () => {
    render(<FlatAgentList handlers={handlers} />)
    const toggle = screen.getByText("Terminals").closest("button")!
    const plus = screen.getByRole("button", {
      name: "New standalone terminal",
    })
    expect(toggle.contains(plus)).toBe(false)
    expect(plus.contains(toggle)).toBe(false)
    // Misclick-safe spacing between the two, and the word stays in the toggle
    // so the whole label still expands the section.
    expect((plus.parentElement as HTMLElement).className).toContain("gap-2")
  })

  it("still collapses and expands, and the + does not toggle it", () => {
    render(<FlatAgentList handlers={handlers} />)
    const toggle = () => screen.getByText("Terminals").closest("button")!
    expect(toggle().getAttribute("aria-expanded")).toBe("true")
    fireEvent.click(
      screen.getByRole("button", { name: "New standalone terminal" }),
    )
    expect(toggle().getAttribute("aria-expanded")).toBe("true")
    fireEvent.click(toggle())
    expect(toggle().getAttribute("aria-expanded")).toBe("false")
    fireEvent.click(toggle())
    expect(toggle().getAttribute("aria-expanded")).toBe("true")
  })

  // The section renders nothing at zero terminals, so the divider + can never
  // create the FIRST one; the launcher corner's ⋯ is its zero-state home.
  it("is absent entirely when there is no terminal", () => {
    mockState = {
      ...makeState("name"),
      spine: { ...makeState("name").spine, terminals: [] },
    } as unknown as DuxState
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByText("Terminals")).toBeNull()
    expect(
      screen.queryByRole("button", { name: "New standalone terminal" }),
    ).toBeNull()
  })
})

// An empty workspace is exactly where the next click should be ON SCREEN, so
// the empty state carries a real button rather than a sentence pointing at a
// control somewhere else.
describe("FlatAgentList empty state", () => {
  function emptyState(projects: { id: string; name: string }[]): DuxState {
    return {
      ...makeState("name"),
      spine: {
        projects,
        terminals: [],
        sessions: [],
        sidebar: {
          groups: projects.map((p) => ({
            project_id: p.id,
            name: p.name,
            orphaned: false,
          })),
          agentless_start: null,
        },
      },
    } as unknown as DuxState
  }

  it("offers a New agent button that opens the picker", () => {
    mockState = emptyState([{ id: "p1", name: "Repo" }])
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("No agents yet")).toBeTruthy()
    const button = screen.getByRole("button", { name: /^new agent$/i })
    // The touch floor, matching the launcher corner this button stands in for.
    expect(button.className).toContain("max-md:min-h-11")
    fireEvent.click(button)
    expect(openNewAgentPickerMock).toHaveBeenCalledWith("new")
  })

  // Same pure helper as the launcher corner's verb, so the two buttons on
  // screen can never offer different next steps.
  it("flips to Add project when there is no project to make an agent in", () => {
    mockState = emptyState([])
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.queryByRole("button", { name: /^new agent$/i })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /^add project$/i }))
    expect(openAddProjectMock).toHaveBeenCalledOnce()
    expect(openNewAgentPickerMock).not.toHaveBeenCalled()
  })

  it("keeps the onboarding state ahead of an empty search result", () => {
    mockState = {
      ...emptyState([{ id: "p1", name: "Repo" }]),
      agentSearch: "missing",
    } as DuxState
    render(<FlatAgentList handlers={handlers} />)
    expect(screen.getByText("No agents yet")).toBeTruthy()
    expect(screen.queryByText(/Nothing matches/)).toBeNull()
  })
})

describe("FlatAgentList search result state", () => {
  it("replaces all sortable sections with the unmatched-query message", () => {
    mockState = {
      ...makeState("name"),
      agentSearch: "definitely absent",
    } as DuxState
    render(<FlatAgentList handlers={handlers} />)
    expect(
      screen.getByText("Nothing matches “definitely absent”."),
    ).toBeTruthy()
    expect(screen.queryByText("Terminals")).toBeNull()
    expect(screen.queryByText("Inactive")).toBeNull()
    expect(dragEndHandlers).toHaveLength(0)
  })
})

// Line two carries text in two faces (a mono folder label beside sans state
// words) and boxes with no text at all (the dot, the project glyph). Baseline
// is the only alignment that keeps them on one line through a font change, so
// these pin the alignment classes on the SHARED pieces both row kinds render:
// nothing else jsdom can check, since it computes no layout.
describe("FlatAgentList line two alignment", () => {
  // The line-two container of the row that shows `word`.
  function lineTwo(word: string): HTMLElement {
    const el = screen.getByText(word).parentElement
    if (!el) throw new Error(`no line two around "${word}"`)
    return el
  }

  it("aligns an agent row's line two on one baseline", () => {
    const state = makeState("name")
    state.spine!.sessions = [
      makeSession({ id: "alpha", title: "Alpha" }),
      {
        ...makeSession({ id: "notes", title: "Notes" }),
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/work/notes",
          folder_label: "~/work/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      } as SessionView,
    ]
    mockState = state
    render(<FlatAgentList handlers={handlers} />)

    // The managed row's project tag.
    const managed = screen.getByText("Repo").closest("span")!.parentElement!
    expect(managed.className).toContain("items-baseline")
    // `self-center` here would opt the whole tag out of the row's baseline.
    expect(managed.className).not.toContain("self-center")
    expect(managed.className).not.toContain("items-center")
    // The glyph is the one centered thing: an icon has no baseline to sit on.
    expect(managed.querySelector("svg")!.getAttribute("class")).toContain(
      "self-center",
    )

    // The standalone row: the mono folder label rides the same baseline.
    const solo = screen.getByText("notes").closest("span")!.parentElement!
    expect(solo.className).toContain("items-baseline")
    expect(solo.className).not.toContain("self-center")

    // Both tags hang off a baseline-aligned line-two container.
    for (const container of [managed.parentElement!, solo.parentElement!]) {
      expect(container.className).toContain("items-baseline")
      expect(container.className).not.toContain("items-center")
    }
  })

  it("aligns a terminal row's line two on the same baseline", () => {
    const base = makeState("name")
    mockState = {
      ...base,
      spine: {
        ...base.spine,
        terminals: [
          makeTerminal({
            id: "t-owned",
            label: "owned",
            owner: { kind: "session", session_id: "zeta" },
          }),
          makeTerminal({
            id: "t-solo",
            label: "solo",
            owner: { kind: "standalone", cwd_label: "~/play" },
          }),
        ],
      },
    } as DuxState
    render(<FlatAgentList handlers={handlers} />)

    const owned = screen.getByText("Zeta@Repo").closest("span")!.parentElement!
    const solo = screen.getByText("play").closest("span")!.parentElement!
    for (const tag of [owned, solo]) {
      expect(tag.className).toContain("items-baseline")
      expect(tag.className).not.toContain("items-center")
      expect(tag.parentElement!.className).toContain("items-baseline")
      expect(tag.parentElement!.className).not.toContain("items-center")
    }
  })

  // A terminal row is an agent row: both render the SAME state-word component,
  // so the two can never drift in tone or in motion.
  it("renders the state word the same way on both row kinds", () => {
    const base = makeState("name")
    mockState = {
      ...base,
      spine: {
        ...base.spine,
        sessions: [makeSession({ id: "alpha", title: "Alpha", working: true })],
        terminals: [makeTerminal({ id: "t-owned", label: "owned" })],
      },
    } as DuxState
    render(<FlatAgentList handlers={handlers} />)

    for (const el of [screen.getByText("Working"), screen.getByText("Idle")]) {
      expect(el.className).toContain("motion-safe:animate-state-word")
      expect(el.className).toContain("shrink-0")
      // Nothing may nudge the word off the line the rest of line two sits on.
      expect(el.className).not.toMatch(/translate|self-|align-|mt-|pt-/)
    }
    expect(lineTwo("Working").className).toContain("items-baseline")
    expect(lineTwo("Idle").className).toContain("items-baseline")
  })

  // The swap animation is the one thing that CAN move the word after layout is
  // settled, and it did: a 3px rise left the word below its own line for the
  // first fraction of a second after every state change, which is exactly what
  // a screenshot of a busy sidebar catches. Fade only, forever.
  it("swaps the state word with a fade that never moves it", async () => {
    // Read as text: the bundler hands back a processed stylesheet, and what is
    // being pinned is the authored keyframe. Vitest runs with the web project
    // as its root.
    const { readFileSync } = await import("node:fs")
    const css = readFileSync(`${process.cwd()}/src/index.css`, "utf8")
    const frames = /@keyframes state-word-swap\s*\{([\s\S]*?)\n\}/.exec(css)
    expect(frames).toBeTruthy()
    expect(frames![1]).toContain("opacity")
    expect(frames![1]).not.toMatch(/transform|translate|top:|margin/)
  })
})
