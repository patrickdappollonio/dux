// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { DragEndEvent } from "@dnd-kit/core"
import type { DuxState } from "@/lib/store"
import type { EditedMacro, MacrosEdit } from "@/lib/macros"
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

const saveMacrosMock = vi.fn<(edit: MacrosEdit) => void>()
const closeMacrosDialogMock = vi.fn<() => void>()
const setAsideMock = vi.fn<(edit: MacrosEdit, changes: number) => void>()
const reloadMacrosDraftMock = vi.fn<() => Promise<void>>()
const dismissMacrosConflictMock = vi.fn<() => void>()
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeMacrosDialog: () => closeMacrosDialogMock(),
    saveMacros: (edit: MacrosEdit) => saveMacrosMock(edit),
    setAsideMacrosDraft: (edit: MacrosEdit, changes: number) => setAsideMock(edit, changes),
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

// The dialog's state in the store: `macros` saved at "v1", and `draft` the
// edits it opens on, each naming the saved macro it started as (`origin`),
// by default the saved list unchanged.
function seed(
  macros: MacroView[],
  {
    conflict = null,
    draft = macros.map((m, origin) => ({ ...m, origin })),
    select = null,
  }: { conflict?: string | null; draft?: EditedMacro[] | null; select?: number | null } = {},
) {
  mockState = {
    macrosDialogOpen: true,
    macrosDraft:
      draft === null ? null : { macros: draft, baseline: macros, version: "v1", selected: select },
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

// The browser's Back: the store closes the newest modal holding an entry.
async function back() {
  await act(async () => {
    openModals().at(-1)!.close()
  })
  await settle()
}

// Lets base-ui's focus moves and close transitions land.
async function settle() {
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)))
}

async function press(el: HTMLElement, key: string) {
  await act(async () => {
    fireEvent.keyDown(el, { key })
  })
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
    const sent = saveMacrosMock.mock.calls[0][0]
    expect(sent.macros.map(({ name, text, surface }) => ({ name, text, surface }))).toEqual([
      { name: "Deploy", text: "deploy it", surface: "both" },
      { name: "Review", text: "review this twice", surface: "agent" },
      { name: "Ship", text: "ship it", surface: "agent" },
    ])
    expect(sent.version).toBe("v1")
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

describe("MacrosDialog reopened on edits it did not save", () => {
  it("counts a renamed macro as one change, with its dot", () => {
    seed(seedMacros, {
      draft: [
        { name: "Review2", text: "review this", surface: "agent", origin: 0 },
        { name: "Build", text: "cargo build", surface: "terminal", origin: 1 },
        { name: "Deploy", text: "deploy it", surface: "both", origin: 2 },
      ],
    })
    render(<MacrosDialog />)
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
    expect(within(row("Review2")).getByText("Unsaved changes:")).toBeTruthy()
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
    await waitFor(() => expect(document.activeElement).toBe(keep))

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

  it.each([false, true])(
    "hands focus back to where it was when the discard confirm closes (phone: %s)",
    async (onPhone) => {
      phone = onPhone
      seed(seedMacros, { select: 0 })
      render(<MacrosDialog />)
      await typeInto("Text", "changed")
      const text = screen.getByLabelText("Text")
      text.focus()

      await press(text, "Escape")
      const ask = await screen.findByRole("dialog", { name: "Discard unsaved changes?" })
      await press(ask, "Escape")
      await settle()
      expect(document.activeElement).toBe(text)

      const cancel = screen.getAllByRole("button", { name: "Cancel" }).at(-1)!
      cancel.focus()
      await click(cancel)
      const again = await screen.findByRole("dialog", { name: "Discard unsaved changes?" })
      await click(within(again).getByRole("button", { name: "Keep editing" }))
      await settle()
      // Where the person was typing, not the Cancel that raised the question.
      expect(document.activeElement).toBe(text)
    },
  )

  it("sets unsaved changes aside on Back, which cannot stop to ask", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Text", "changed")

    await act(async () => {
      openModals().at(-1)!.close()
    })
    expect(setAsideMock).toHaveBeenCalledWith(
      {
        macros: [
          { name: "Review", text: "changed", surface: "agent", origin: 0 },
          { name: "Build", text: "cargo build", surface: "terminal", origin: 1 },
          { name: "Deploy", text: "deploy it", surface: "both", origin: 2 },
        ],
        baseline: seedMacros,
        version: "v1",
        selected: 0,
      },
      1,
    )
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
  })
})

describe("MacrosDialog editor", () => {
  it("opens on the macro edits set aside were left on", () => {
    seed(seedMacros, { select: 2 })
    render(<MacrosDialog />)
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Deploy")
    expect(row("Deploy").getAttribute("aria-selected")).toBe("true")
  })

  it("offers where a macro is shown as one choice of three, says what it means, and a press changes it", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    const shownOn = screen.getByRole("radiogroup", { name: "Shown on" })
    expect(within(shownOn).getByRole("radio", { checked: true }).textContent).toBe("Agent")
    expect(screen.getByText("Only shown when the agent pane is focused.")).toBeTruthy()
    await click(within(shownOn).getByRole("radio", { name: "Both" }))
    expect(within(row("Review")).getByText("Both")).toBeTruthy()
    expect(screen.getByText("Shown on both surfaces.")).toBeTruthy()
  })

  it("Delete asks in its own dialog with Cancel focused and the name as a chip", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await menuPick("Delete…")
    const confirm = await screen.findByRole("dialog", { name: "Delete this macro?" })
    const cancel = within(confirm).getByRole("button", { name: "Cancel" })
    await waitFor(() => expect(document.activeElement).toBe(cancel))
    expect(within(confirm).getByText("Review").getAttribute("data-slot")).toBe("inline-code")

    await click(cancel)
    expect(listedNames()).toEqual(["Review", "Build", "Deploy"])
  })

  it("puts focus on the neighbouring macro after a delete, and on New macro after the last one", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Build"))
    await deleteSelected()
    await settle()
    expect(document.activeElement).toBe(row("Deploy"))

    seed([{ name: "Review", text: "review this", surface: "agent" }])
    cleanup()
    render(<MacrosDialog />)
    await deleteSelected()
    await settle()
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "New macro" }))
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
    const filter = screen.getByRole("radiogroup", { name: "Show macros for" })
    const choices = within(filter).getAllByRole("radio").map((t) => t.textContent)
    expect(choices).toEqual(["All3", "Agent2", "Terminal2"])

    await click(within(filter).getByRole("radio", { name: /Terminal/ }))
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

  it("refuses to save an invalid list, says why with the name as a chip, and leads to the macro at fault", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Name", "Build")
    const problem = screen.getByText(/Duplicate macro name/)
    expect(problem.textContent).toBe("Duplicate macro name: Build.")
    expect(within(problem).getByText("Build").getAttribute("data-slot")).toBe("inline-code")
    expect(save()).toHaveProperty("disabled", true)

    // The second of the two is the one at fault.
    await click(row("Deploy"))
    await click(screen.getByRole("button", { name: "Show the macro" }))
    const selected = screen.getAllByRole("option").map((r) => r.getAttribute("aria-selected"))
    expect(selected).toEqual(["false", "true", "false"])
    const name = screen.getByLabelText("Name")
    expect(name).toHaveProperty("value", "Build")
    // The field at fault is read with the sentence, which is no live region.
    expect(name.getAttribute("aria-invalid")).toBe("true")
    expect(name.getAttribute("aria-describedby")).toBe(problem.id)
    expect(problem.getAttribute("role")).toBeNull()
  })

  it("does not call a new macro nameless before anything was typed into it", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(screen.getByRole("button", { name: "New" }))
    expect(screen.queryByText(/needs a name/)).toBeNull()
    expect(save()).toHaveProperty("disabled", true)

    await typeInto("Text", "ship it")
    expect(screen.getByText("Every macro needs a name.")).toBeTruthy()
  })

  it("keeps the macro that blocks Save in the list whatever the search or the filter", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(screen.getByRole("button", { name: "New" }))
    await typeInto("Search macros", "CARGO")
    expect(screen.getAllByRole("option")).toHaveLength(2)
    const filter = screen.getByRole("radiogroup", { name: "Show macros for" })
    await click(within(filter).getByRole("radio", { name: /Terminal/ }))
    expect(screen.getAllByRole("option")).toHaveLength(2)
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "")
  })

  it("says a filter is empty without speaking of a search nobody typed", async () => {
    seed([{ name: "Review", text: "review this", surface: "agent" }])
    render(<MacrosDialog />)
    const filter = screen.getByRole("radiogroup", { name: "Show macros for" })
    await click(within(filter).getByRole("radio", { name: /Terminal/ }))
    expect(screen.queryByText(/search/)).toBeNull()
    expect(screen.getAllByText("No macro is shown on terminals.").length).toBeGreaterThan(0)
  })

  it("duplicates an untitled macro without a leading space in the copy's name", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(screen.getByRole("button", { name: "New" }))
    await menuPick("Duplicate")
    const name = (screen.getByLabelText("Name") as HTMLInputElement).value
    expect(name).toBe(name.trimStart())
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

  it("opens straight on the macro edits set aside were left on", () => {
    phone = true
    seed(seedMacros, { select: 2 })
    render(<MacrosDialog />)
    expect(screen.getByLabelText("Name")).toHaveProperty("value", "Deploy")
  })

  it("Back from a macro's editor returns to the list, and from the list closes", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Build"))
    await typeInto("Text", "cargo build --release")

    await back()
    expect(screen.queryByLabelText("Name")).toBeNull()
    expect(screen.getByRole("listbox")).toBeTruthy()
    expect(setAsideMock).not.toHaveBeenCalled()
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()

    await back()
    expect(setAsideMock).toHaveBeenCalledTimes(1)
  })

  it("Back goes to the list first when it opened straight on a macro", async () => {
    phone = true
    seed(seedMacros, { select: 2 })
    render(<MacrosDialog />)
    await settle()
    await back()
    expect(screen.getByRole("listbox")).toBeTruthy()
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
  })

  it("puts focus in the editor it opens, and back on the row it came from", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await settle()
    await press(row("Build"), "Enter")
    await settle()
    const editor = screen.getByRole("dialog", { name: "Edit macro" })
    expect(editor.contains(document.activeElement)).toBe(true)

    await click(screen.getByRole("button", { name: "Back to all macros" }))
    await settle()
    expect(document.activeElement).toBe(row("Build"))

    await press(row("Build"), "Enter")
    await settle()
    await back()
    expect(document.activeElement).toBe(row("Build"))
  })

  it("lands on the neighbouring row after a delete from the editor", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Build"))
    await deleteSelected()
    await settle()
    expect(screen.queryByLabelText("Name")).toBeNull()
    expect(document.activeElement).toBe(row("Deploy"))
  })

  it("moves the selection with the arrow keys, so focus, the selected row and the tab stop agree", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await settle()
    await press(row("Review"), "ArrowDown")
    expect(document.activeElement).toBe(row("Build"))
    expect(row("Build").getAttribute("aria-selected")).toBe("true")
    expect(row("Review").getAttribute("aria-selected")).toBe("false")
    expect(row("Build").tabIndex).toBe(0)
    expect(row("Review").tabIndex).toBe(-1)
    // Moving does not open it.
    expect(screen.queryByLabelText("Name")).toBeNull()
  })
})

describe("MacrosDialog stale save", () => {
  const stale =
    "The macro list changed after this dialog read it, so nothing was saved. Reload macros shows the list as it is now and drops your edits. Keep editing keeps them, but saving them is refused until you reload."

  it("says the list changed and offers to reload it or keep editing", async () => {
    seed(seedMacros, { conflict: stale })
    render(<MacrosDialog />)

    const notice = screen.getByRole("alert")
    expect(notice.textContent).toContain("changed after this dialog read it")
    // Reloading throws the edits away, so keeping them is what has focus, and
    // the dialog's own choice of focus on opening leaves it there.
    const keep = screen.getByRole("button", { name: "Keep editing" })
    await settle()
    expect(document.activeElement).toBe(keep)

    await click(screen.getByRole("button", { name: "Reload macros" }))
    expect(reloadMacrosDraftMock).toHaveBeenCalledTimes(1)

    await click(keep)
    expect(dismissMacrosConflictMock).toHaveBeenCalledTimes(1)
  })

  it("starts over from a reloaded list in place, without opening the dialog again", async () => {
    seed(seedMacros)
    const { rerender } = render(<MacrosDialog />)
    await typeInto("Text", "changed")
    expect(screen.getByText("1 unsaved change")).toBeTruthy()
    const dialog = screen.getByRole("dialog", { name: "Macros" })

    // What a reload leaves in the store: the saved list as a new draft.
    const reloaded = [{ name: "Lint", text: "cargo clippy", surface: "terminal" as const }]
    seed(reloaded)
    mockState = { ...mockState, macrosDraftEpoch: 1 }
    rerender(<MacrosDialog />)
    expect(listedNames()).toEqual(["Lint"])
    expect(screen.queryByText(/unsaved change/)).toBeNull()
    expect(screen.getByRole("dialog", { name: "Macros" })).toBe(dialog)
  })

  it("shows the refused edits as unsaved changes", () => {
    seed(seedMacros, {
      conflict: stale,
      draft: [{ name: "Review", text: "edited", surface: "agent", origin: 0 }],
    })
    render(<MacrosDialog />)
    expect(screen.getByText("3 unsaved changes")).toBeTruthy()
  })
})

describe("MacrosDialog brings the selected macro into view", () => {
  let scrolled: Element[]
  beforeEach(() => {
    scrolled = []
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this)
    }
  })
  afterEach(() => {
    delete (Element.prototype as Partial<Element>).scrollIntoView
  })

  it("when it opens on the macro edits set aside were left on", () => {
    seed(seedMacros, { select: 2 })
    render(<MacrosDialog />)
    expect(scrolled).toContain(row("Deploy"))
  })

  it("when Show the macro leads to it, with focus in its name", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await typeInto("Name", "Build")
    await click(row("Deploy"))
    scrolled = []
    await click(screen.getByRole("button", { name: "Show the macro" }))
    await settle()
    const shown = screen.getAllByRole("option").find((r) => r.getAttribute("aria-selected") === "true")
    expect(scrolled).toContain(shown)
    expect(document.activeElement).toBe(screen.getByLabelText("Name"))
  })

  it("not when a press on the row selected it", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    scrolled = []
    await click(row("Deploy"))
    expect(scrolled).not.toContain(row("Deploy"))
  })
})

describe("MacrosDialog focus on a phone", () => {
  it("opens under a finger with focus on the dialog, not on New or the search field", async () => {
    phone = true
    vi.stubGlobal(
      "matchMedia",
      vi.fn((query: string) => ({
        matches: query === "(pointer: coarse)",
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
      })),
    )
    seed(seedMacros)
    render(<MacrosDialog />)
    await settle()
    expect(document.activeElement).toBe(screen.getByRole("dialog", { name: "Macros" }))
  })

  it("opens under a mouse in the phone's layout with focus in the search field", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await settle()
    expect(document.activeElement).toBe(screen.getByLabelText("Search macros"))
  })

  it("hands focus back to the control that opened it after a Save from a macro's editor", async () => {
    phone = true
    const { openMacrosDialog } = await import("@/lib/store")
    const { rerender } = render(<button>Run a macro</button>)
    const trigger = screen.getByRole("button", { name: "Run a macro" })
    openMacrosDialog(() => trigger)
    seed(seedMacros)
    rerender(
      <>
        <button>Run a macro</button>
        <MacrosDialog />
      </>,
    )
    await settle()
    await click(row("Build"))
    await typeInto("Text", "cargo build --release")
    await settle()
    await click(screen.getByRole("button", { name: "Save 1 change" }))
    expect(saveMacrosMock).toHaveBeenCalledTimes(1)
    mockState = { ...mockState, macrosDialogOpen: false }
    rerender(
      <>
        <button>Run a macro</button>
        <MacrosDialog />
      </>,
    )
    await settle()
    expect(document.activeElement).toBe(trigger)
  })

  // The phone's app menu is a sheet that has closed by the time the dialog
  // does, taking the row that opened the dialog with it.
  it("hands focus back to the app menu's button when opened from the menu sheet", async () => {
    phone = true
    const { openMacrosDialog } = await import("@/lib/store")
    const { rerender } = render(
      <>
        <button>Menu</button>
        <div role="dialog">
          <button>Edit macros…</button>
        </div>
      </>,
    )
    const menuButton = screen.getByRole("button", { name: "Menu" })
    fireEvent.pointerDown(menuButton)
    const menuRow = screen.getByRole("button", { name: "Edit macros…" })
    menuRow.focus()
    fireEvent.pointerDown(menuRow)
    openMacrosDialog()
    seed(seedMacros)
    rerender(
      <>
        <button>Menu</button>
        <MacrosDialog />
      </>,
    )
    await settle()
    await click(row("Build"))
    await typeInto("Text", "cargo build --release")
    await settle()
    await click(screen.getByRole("button", { name: "Save 1 change" }))
    mockState = { ...mockState, macrosDialogOpen: false }
    rerender(
      <>
        <button>Menu</button>
        <MacrosDialog />
      </>,
    )
    await settle()
    expect(document.activeElement).toBe(menuButton)
  })
})

// jsdom lays nothing out, so the room and the row report the sizes a browser
// measured: a 403px room over 52px rows with a 2px gap holds seven whole rows.
describe("MacrosDialog list on a computer", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    )
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(403)
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "option" ? 52 : 0
    })
    vi.spyOn(window, "getComputedStyle").mockReturnValue({ rowGap: "2px" } as CSSStyleDeclaration)
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("ends on a whole row", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await settle()
    const scroller = screen.getByRole("listbox").parentElement!
    expect(scroller.style.height).toBe(`${7 * 54 - 2}px`)
  })
})

// A confirm dims what is behind it with its own overlay, which base-ui draws
// only for a dialog that is not nested inside another.
describe("MacrosDialog confirms", () => {
  it("draw an overlay of their own over the dialog", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Deploy"))
    await menuPick("Delete…")
    await screen.findByRole("dialog", { name: "Delete this macro?" })
    expect(document.querySelectorAll('[data-slot="dialog-overlay"]')).toHaveLength(2)
  })

  it("close alone on Escape, leaving the dialog open", async () => {
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Deploy"))
    await menuPick("Delete…")
    const confirm = await screen.findByRole("dialog", { name: "Delete this macro?" })
    await settle()
    await press(within(confirm).getByRole("button", { name: "Cancel" }), "Escape")
    await settle()
    expect(screen.queryByRole("dialog", { name: "Delete this macro?" })).toBeNull()
    expect(screen.queryByRole("dialog", { name: "Discard unsaved changes?" })).toBeNull()
    expect(closeMacrosDialogMock).not.toHaveBeenCalled()
  })

  it("draw an overlay of their own over the phone's editor", async () => {
    phone = true
    seed(seedMacros)
    render(<MacrosDialog />)
    await click(row("Deploy"))
    await settle()
    await menuPick("Delete…")
    await screen.findByRole("dialog", { name: "Delete this macro?" })
    const overlays = document.querySelectorAll('[data-slot="dialog-overlay"], [data-slot="sheet-overlay"]')
    expect(overlays).toHaveLength(2)
  })
})
