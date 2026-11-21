// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import type { DragEndEvent } from "@dnd-kit/core"
import type { DuxState } from "@/lib/store"
import type { MacroView } from "@/lib/types"

// dnd-kit cannot run a real drag under jsdom (it measures rects), so
// DndContext is a passthrough that captures `onDragEnd`, and the tests end a
// drag by calling it, as FlatAgentList.test.tsx does.
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
    attributes: {},
    listeners: {},
    setNodeRef: () => {},
    transform: null,
    transition: undefined,
    isDragging: false,
  }),
}))

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

const saveMacrosMock = vi.fn<(macros: MacroView[], version: string | null) => void>()
const closeMacrosDialogMock = vi.fn<() => void>()
const setAsideMock = vi.fn<(draft: MacroView[], version: string | null, changes: number) => void>()
const reloadMacrosDraftMock = vi.fn<() => Promise<void>>()
const dismissMacrosConflictMock = vi.fn<() => void>()
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeMacrosDialog: () => closeMacrosDialogMock(),
    saveMacros: (macros: MacroView[], version: string | null) => saveMacrosMock(macros, version),
    setAsideMacrosDraft: (draft: MacroView[], version: string | null, changes: number) =>
      setAsideMock(draft, version, changes),
    reloadMacrosDraft: () => reloadMacrosDraftMock(),
    dismissMacrosConflict: () => dismissMacrosConflictMock(),
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
const { openModals } = await import("@/lib/modalHistory")

const seedMacros: MacroView[] = [
  { name: "Review", text: "review this", surface: "agent" },
  { name: "Build", text: "cargo build", surface: "terminal" },
  { name: "Deploy", text: "deploy it", surface: "both" },
]

function seed(
  macros: MacroView[],
  {
    conflict = null,
    draft = macros,
    select = null,
  }: { conflict?: string | null; draft?: MacroView[] | null; select?: string | null } = {},
) {
  mockState = {
    macrosDialogOpen: true,
    macrosDraft: draft,
    macrosDraftVersion: "v1",
    macrosDialogSelect: select,
    macrosConflict: conflict,
    macrosDraftEpoch: 0,
    bootstrap: { macros, macros_version: "v1" },
  } as unknown as DuxState
}

// The listed macro names, in list order.
function listedNames(): string[] {
  return screen
    .getAllByRole("option")
    .map((row) => row.querySelector(".font-medium")?.textContent ?? "(missing)")
}

function row(name: string): HTMLElement {
  const found = screen
    .getAllByRole("option")
    .find((r) => r.querySelector(".font-medium")?.textContent === name)
  if (!found) throw new Error(`no row ${name}`)
  return found
}

async function endDrag(activeKey: number, overKey: number | null) {
  const handler = dragEndHandlers.at(-1)
  expect(handler).toBeDefined()
  await act(async () => {
    handler!({
      active: { id: `macro-${activeKey}` },
      over: overKey === null ? null : { id: `macro-${overKey}` },
    } as DragEndEvent)
  })
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el)
  })
}

async function typeInto(label: string, value: string) {
  await act(async () => {
    fireEvent.change(screen.getByLabelText(label), { target: { value } })
  })
}

// Opens the selected macro's `⋯` and picks `item`.
async function menuPick(item: string) {
  await click(screen.getByRole("button", { name: /^Actions for / }))
  await click(await screen.findByRole("menuitem", { name: item }))
}

async function deleteSelected() {
  await menuPick("Delete…")
  const confirm = await screen.findByRole("dialog", { name: "Delete this macro?" })
  await click(within(confirm).getByRole("button", { name: "Delete" }))
}

function save(): HTMLElement {
  return screen.getByRole("button", { name: /^Save/ })
}

beforeEach(() => {
  installStubs()
  phone = false
  dragEndHandlers.length = 0
  saveMacrosMock.mockReset()
  closeMacrosDialogMock.mockReset()
  setAsideMock.mockReset()
  reloadMacrosDraftMock.mockReset()
  reloadMacrosDraftMock.mockResolvedValue(undefined)
  dismissMacrosConflictMock.mockReset()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("MacrosDialog writes nothing before Save", () => {
  it("a drag after an unsaved delete writes nothing", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)

    await click(row("Deploy"))
    await deleteSelected()
    await endDrag(0, 1)

    expect(saveMacrosMock).not.toHaveBeenCalled()
    expect(listedNames()).toEqual(["Build", "Review"])
  })

  it("a drag after an unsaved edit writes nothing", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)

    await typeInto("Text", "review this twice")
    await endDrag(2, 0)

    expect(saveMacrosMock).not.toHaveBeenCalled()
    expect(listedNames()).toEqual(["Deploy", "Review", "Build"])
  })

  it("Save sends every edit, delete, addition and the order in one request, with the version it read", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)

    await typeInto("Text", "review this twice")
    await click(row("Build"))
    await deleteSelected()
    await click(screen.getByRole("button", { name: "New" }))
    await typeInto("Name", "Ship")
    await typeInto("Text", "ship it")
    await endDrag(2, 0)
    expect(saveMacrosMock).not.toHaveBeenCalled()

    await click(save())

    expect(saveMacrosMock).toHaveBeenCalledTimes(1)
    expect(saveMacrosMock).toHaveBeenCalledWith(
      [
        { name: "Deploy", text: "deploy it", surface: "both" },
        { name: "Review", text: "review this twice", surface: "agent" },
        { name: "Ship", text: "ship it", surface: "agent" },
      ],
      "v1",
    )
  })

  it("counts the unsaved changes and marks each changed macro", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    expect(screen.queryByText(/unsaved change/)).toBeNull()
    expect(save()).toHaveProperty("disabled", true)

    await click(row("Build"))
    await typeInto("Name", "Build all")
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
    expect(within(row("Build all")).getByText("Unsaved changes:")).toBeTruthy()
    expect(within(row("Review")).queryByText("Unsaved changes:")).toBeNull()

    // A reorder is one change for the list, and marks no row.
    await endDrag(2, 0)
    expect(screen.getByText("2 unsaved changes")).toBeTruthy()
    expect(within(row("Deploy")).queryByText("Unsaved changes:")).toBeNull()

    // Undoing the edit by hand takes its change away again.
    await typeInto("Name", "Build")
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
  })
})

describe("MacrosDialog closing", () => {
  it("closes at once with nothing to lose", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(screen.getByRole("button", { name: "Cancel" }))
    expect(closeMacrosDialogMock).toHaveBeenCalledTimes(1)
  })

  it("asks before Cancel drops unsaved changes, and Keep editing keeps them", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Text", "changed")

    await click(screen.getByRole("button", { name: "Cancel" }))
    const ask = await screen.findByRole("dialog", { name: "Discard unsaved changes?" })
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
    const keep = within(ask).getByRole("button", { name: "Keep editing" })
    expect(document.activeElement).toBe(keep)

    await click(keep)
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
    expect(screen.getByLabelText("Text")).toHaveProperty("value", "changed")

    await click(screen.getByRole("button", { name: "Cancel" }))
    const again = await screen.findByRole("dialog", { name: "Discard unsaved changes?" })
    await click(within(again).getByRole("button", { name: "Discard changes" }))
    expect(closeMacrosDialogMock).toHaveBeenCalledTimes(1)
    expect(saveMacrosMock).not.toHaveBeenCalled()
  })

  it("asks before Escape drops unsaved changes", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Text", "changed")

    await act(async () => {
      fireEvent.keyDown(screen.getByLabelText("Text"), { key: "Escape" })
    })
    expect(await screen.findByRole("dialog", { name: "Discard unsaved changes?" })).toBeTruthy()
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
  })

  it("sets unsaved changes aside on Back, which cannot stop to ask", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Text", "changed")

    await act(async () => {
      openModals().at(-1)!.close()
    })
    expect(setAsideMock).toHaveBeenCalledWith(
      [
        { name: "Review", text: "changed", surface: "agent" },
        { name: "Build", text: "cargo build", surface: "terminal" },
        { name: "Deploy", text: "deploy it", surface: "both" },
      ],
      "v1",
      1,
    )
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
  })
})

describe("MacrosDialog editor", () => {
  it("opens on the macro the picker's pencil named", () => {
    seed(seedMacros, { select: "Deploy" })
    render(<MacrosDialog />)
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Deploy")
    expect(row("Deploy").getAttribute("aria-selected")).toBe("true")
  })

  it("shows the surface as its word, and a press changes it", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    const shownOn = screen.getByRole("tablist", { name: "Shown on" })
    expect(within(shownOn).getByRole("tab", { selected: true }).textContent).toBe("Agent")
    await click(within(shownOn).getByRole("tab", { name: "Both" }))
    expect(within(row("Review")).getByText("Both")).toBeTruthy()
  })

  it("Delete asks in its own dialog with Cancel focused and the name as a chip", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await menuPick("Delete…")
    const confirm = await screen.findByRole("dialog", { name: "Delete this macro?" })
    const cancel = within(confirm).getByRole("button", { name: "Cancel" })
    expect(document.activeElement).toBe(cancel)
    expect(within(confirm).getByText("Review").getAttribute("data-slot")).toBe("inline-code")

    await click(cancel)
    expect(listedNames()).toEqual(["Review", "Build", "Deploy"])
  })

  it("Duplicate adds an unsaved copy after the macro, and Move to top moves it", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Build"))
    await menuPick("Duplicate")
    expect(listedNames()).toEqual(["Review", "Build", "Build copy", "Deploy"])
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Build copy")

    await menuPick("Move to top")
    expect(listedNames()).toEqual(["Build copy", "Review", "Build", "Deploy"])
  })

  it("counts each filter, a macro on both under both, and lists what it counts", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    const filter = screen.getByRole("tablist", { name: "Show macros for" })
    const tabs = within(filter).getAllByRole("tab").map((t) => t.textContent)
    expect(tabs).toEqual(["All3", "Agent2", "Terminal2"])

    await click(within(filter).getByRole("tab", { name: /Terminal/ }))
    expect(listedNames()).toEqual(["Build", "Deploy"])
    // The selected macro was filtered out, so the first one listed is selected.
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Build")
  })

  it("searches names and text", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Search macros", "CARGO")
    expect(listedNames()).toEqual(["Build"])
  })

  it("moves the selection with the arrow keys", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    // Past the dialog's own initial focus, which lands a moment after opening.
    await act(() => new Promise((resolve) => setTimeout(resolve, 0)))
    await act(async () => {
      fireEvent.keyDown(row("Review"), { key: "ArrowDown" })
    })
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Build")
    expect(document.activeElement).toBe(row("Build"))
    await act(async () => {
      fireEvent.keyDown(row("Build"), { key: "End" })
    })
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Deploy")
  })

  it("refuses to save an invalid list and says why", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Name", "Build")
    expect(screen.getByText('Duplicate macro name: "Build".')).toBeTruthy()
    expect(save()).toHaveProperty("disabled", true)
  })

  it("with no macros offers a New macro, which opens an empty editor", async () => {
    seed([])
    render(<MacrosDialog />)
    expect(screen.getByText("No macros yet")).toBeTruthy()
    await click(screen.getByRole("button", { name: "New macro" }))
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "")
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
  })

  it("seeds from the bootstrap when it opened before the macros loaded", () => {
    seed(seedMacros, { draft: null })
    render(<MacrosDialog />)
    expect(listedNames()).toEqual(["Review", "Build", "Deploy"])
    expect(screen.queryByText(/unsaved change/)).toBeNull()
  })
})

describe("MacrosDialog on a phone", () => {
  it("opens a macro from the list as a second step, and Back returns to the list", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    expect(screen.queryByLabelText("Name")).toBeNull()

    await click(row("Build"))
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Build")
    expect(screen.queryByRole("listbox")).toBeNull()

    await typeInto("Text", "cargo build --release")
    expect(screen.getByRole("button", { name: "Save 1 change" })).toBeTruthy()

    await click(screen.getByRole("button", { name: "Back to all macros" }))
    expect(within(row("Build")).getByText("Unsaved changes:")).toBeTruthy()
    expect(saveMacrosMock).not.toHaveBeenCalled()
  })

  it("opens straight on the macro the pencil named", () => {
    phone = true
    seed(seedMacros, { select: "Deploy" })
    render(<MacrosDialog />)
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Deploy")
  })
})

describe("MacrosDialog stale save", () => {
  const stale =
    "The macro list changed after this dialog read it, so nothing was saved. Reload to see the change, then make your edit again."

  it("says the list changed and offers to reload it or keep editing", async () => {
    seed(seedMacros, { conflict: stale })
    render(<MacrosDialog />)

    const notice = screen.getByRole("alert")
    expect(notice.textContent).toContain("changed after this dialog read it")
    // Reloading throws the edits away, so keeping them is what has focus.
    const keep = screen.getByRole("button", { name: "Keep editing" })
    expect(document.activeElement).toBe(keep)

    await click(screen.getByRole("button", { name: "Reload macros" }))
    expect(reloadMacrosDraftMock).toHaveBeenCalledTimes(1)

    await click(keep)
    expect(dismissMacrosConflictMock).toHaveBeenCalledTimes(1)
  })

  it("shows the refused edits as unsaved changes", () => {
    seed(seedMacros, {
      conflict: stale,
      draft: [{ name: "Review", text: "edited", surface: "agent" }],
    })
    render(<MacrosDialog />)
    expect(screen.getByText("3 unsaved changes")).toBeTruthy()
  })
})
