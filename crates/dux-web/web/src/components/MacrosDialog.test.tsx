// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, screen } from "@testing-library/react"

import type { DragEndEvent } from "@dnd-kit/core"
import type { DuxState } from "@/lib/store"
import type { MacroView } from "@/lib/types"

// Reorder-by-drag contract, the FlatAgentList.test.tsx pattern: dnd-kit cannot
// run a real drag under jsdom (it measures rects), so DndContext is mocked to a
// passthrough that CAPTURES `onDragEnd` and the tests call it with synthetic
// DragEndEvents. useSortable is a static stub; what these tests pin is the
// dialog's own wiring — rows carry the sortable attributes, a drop reorders the
// draft, a persisted reorder goes through `persistMacroOrder`, and a refused
// save snaps the order back — not dnd-kit's internals.
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

const persistMacroOrderMock = vi.fn<(macros: MacroView[]) => Promise<boolean>>()
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeMacrosDialog: vi.fn(),
    saveMacros: vi.fn(),
    persistMacroOrder: (macros: MacroView[]) => persistMacroOrderMock(macros),
  }
})

// The store touches localStorage/fetch at import time; stub the browser
// globals BEFORE the module graph evaluates.
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
const { MacrosDialog } = await import("./MacrosDialog")

const seedMacros: MacroView[] = [
  { name: "Review", text: "review this", surface: "agent" },
  { name: "Build", text: "cargo build", surface: "terminal" },
  { name: "Deploy", text: "deploy it", surface: "both" },
]

function seed(macros: MacroView[], { bootstrap = true } = {}) {
  mockState = {
    macrosDialogOpen: true,
    macrosDraft: macros,
    bootstrap: bootstrap ? { macros } : null,
  } as unknown as DuxState
}

// The rendered macro names, in list DOM order.
function renderedNames(): string[] {
  return screen
    .getAllByRole("listitem")
    .map(
      (li) => li.querySelector("span.truncate")?.textContent ?? "(missing)",
    )
}

async function endDrag(activeId: string, overId: string | null) {
  const handler = dragEndHandlers.at(-1)
  expect(handler).toBeDefined()
  await act(async () => {
    handler!({
      active: { id: activeId },
      over: overId === null ? null : { id: overId },
    } as DragEndEvent)
  })
}

beforeEach(() => {
  installStubs()
  dragEndHandlers.length = 0
  persistMacroOrderMock.mockReset()
  persistMacroOrderMock.mockResolvedValue(true)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("MacrosDialog reorder", () => {
  it("renders the draft rows in order, each carrying the sortable attributes", () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    expect(renderedNames()).toEqual(["Review", "Build", "Deploy"])
    // The drag activator (the row body, per the wrapper/activator split the
    // sidebar rows use) carries the sortable attributes; the listitem itself
    // keeps its list semantics.
    for (const li of screen.getAllByRole("listitem")) {
      expect(li.querySelector('[aria-roledescription="sortable"]')).toBeTruthy()
    }
  })

  it("a drop moves the row and persists the reordered list wholesale", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)

    await endDrag("macro-0", "macro-2")

    expect(renderedNames()).toEqual(["Build", "Deploy", "Review"])
    expect(persistMacroOrderMock).toHaveBeenCalledTimes(1)
    expect(
      persistMacroOrderMock.mock.calls[0][0].map((m) => m.name),
    ).toEqual(["Build", "Deploy", "Review"])
  })

  it("a refused save snaps the order back (optimistic apply, rollback on false)", async () => {
    persistMacroOrderMock.mockResolvedValue(false)
    seed(seedMacros)
    render(<MacrosDialog />)

    await endDrag("macro-0", "macro-2")

    // The store action already toasted; the dialog's job is the snap-back.
    expect(renderedNames()).toEqual(["Review", "Build", "Deploy"])
  })

  it("a no-op drop (same slot or no target) does not save", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)

    await endDrag("macro-1", "macro-1")
    await endDrag("macro-1", null)

    expect(renderedNames()).toEqual(["Review", "Build", "Deploy"])
    expect(persistMacroOrderMock).not.toHaveBeenCalled()
  })

  it("an invalid draft reorders locally but defers persistence to Save", async () => {
    // Duplicate names: the wholesale PUT would be rejected server-side, so the
    // drag must keep the order in the draft (for the eventual Save) without
    // firing a doomed request.
    const dupes: MacroView[] = [
      { name: "Same", text: "a", surface: "agent" },
      { name: "Same", text: "b", surface: "agent" },
      { name: "Other", text: "c", surface: "both" },
    ]
    seed(dupes)
    render(<MacrosDialog />)

    await endDrag("macro-2", "macro-0")

    expect(renderedNames()).toEqual(["Other", "Same", "Same"])
    expect(persistMacroOrderMock).not.toHaveBeenCalled()
  })

  it("with bootstrap unloaded a drop reorders locally without saving", async () => {
    seed(seedMacros, { bootstrap: false })
    render(<MacrosDialog />)

    await endDrag("macro-2", "macro-0")

    expect(renderedNames()).toEqual(["Deploy", "Review", "Build"])
    expect(persistMacroOrderMock).not.toHaveBeenCalled()
  })
})
