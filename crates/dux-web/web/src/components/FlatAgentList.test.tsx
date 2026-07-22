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
  PointerSensor: class {},
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
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    reorderAgents: (...args: unknown[]) => reorderAgentsMock(...args),
    reorderTerminals: (...args: unknown[]) => reorderTerminalsMock(...args),
    setAgentSort: (...args: unknown[]) => setAgentSortMock(...args),
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
}
installStubs()
const { FlatAgentList } = await import("./FlatAgentList")

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    project_id: "p1",
    title: over.id,
    provider: "claude",
    branch_name: over.id,
    initial_branch: over.id,
    source_branch: "main",
    worktree_path: `/tmp/${over.id}`,
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function makeTerminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
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
      projects: [{ id: "p1", name: "Repo", terminals: [] }],
      sessions: [
        makeSession({
          id: "zeta",
          title: "Zeta",
          terminals: [
            makeTerminal({ id: "t-z", label: "zsh", sort_order: 1 }),
            makeTerminal({ id: "t-a", label: "bash", sort_order: 2 }),
          ],
        }),
        makeSession({ id: "gone", title: "Gone", status: "exited" }),
        makeSession({ id: "alpha", title: "Alpha" }),
        makeSession({ id: "mike", title: "Mike" }),
      ],
      sidebar: { groups: [], agentless_start: null },
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
  mockState = makeState("name")
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
